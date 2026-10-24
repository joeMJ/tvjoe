//! Wiedergabe: libmpv mit der Render-API in einem `GtkGLArea`.
//!
//! mpv dekodiert (wenn möglich in Hardware, `hwdec=auto-safe`) und zeichnet direkt in das
//! Framebuffer-Objekt des GLArea. Neue Bilder meldet mpv über den Update-Callback; daraufhin wird
//! nur das GLArea neu gezeichnet.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use libmpv2::Mpv;
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};

use crate::gl;
use crate::trace;

pub struct Player {
    pub mpv: &'static Mpv,
    render: RefCell<Option<RenderContext<'static>>>,
    first_frame: Cell<bool>,
    pub area: gtk::GLArea,
}

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn gdk_wayland_display_get_wl_display(display: *mut c_void) -> *mut c_void;
    fn gdk_x11_display_get_xdisplay(display: *mut c_void) -> *mut c_void;
}

/// Wayland- bzw. X11-Display für mpv (nötig für die VAAPI-Übergabe an EGL ohne Kopie).
fn native_display(widget: &impl IsA<gtk::Widget>) -> Option<RenderParam<()>> {
    #[cfg(target_os = "linux")]
    {
        use gtk::glib::translate::ToGlibPtr;
        let display = widget.as_ref().display();
        let raw: *mut gtk::gdk::ffi::GdkDisplay = display.to_glib_none().0;
        match display.type_().name() {
            "GdkWaylandDisplay" => {
                let p = unsafe { gdk_wayland_display_get_wl_display(raw.cast()) };
                (!p.is_null()).then_some(RenderParam::WaylandDisplay(p.cast_const()))
            }
            "GdkX11Display" => {
                let p = unsafe { gdk_x11_display_get_xdisplay(raw.cast()) };
                (!p.is_null()).then_some(RenderParam::X11Display(p.cast_const()))
            }
            _ => None,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = widget;
        None
    }
}

impl Player {
    pub fn new(mute: bool) -> Result<Rc<Self>, libmpv2::Error> {
        // libmpv verlangt Punkt als Dezimaltrenner; GTK hat die Locale aus der Umgebung gesetzt.
        unsafe { libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr()) };
        let mpv = Mpv::with_initializer(|init| {
            init.set_option("vo", "libmpv")?;
            init.set_option("hwdec", "auto-safe")?;
            // Rendern blockiert sonst bis zum Anzeigezeitpunkt (GTK-Hauptschleife).
            init.set_option("video-timing-offset", 0.0)?;
            init.set_option("input-default-bindings", false)?;
            init.set_option("input-vo-keyboard", false)?;
            init.set_option("osc", false)?;
            init.set_option("mute", mute)?;
            Ok(())
        })?;
        trace("mpv bereit");
        // Lebt so lange wie das Programm; der RenderContext leiht es mit 'static.
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));

        let area = gtk::GLArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        let player = Rc::new(Self {
            mpv,
            render: RefCell::new(None),
            first_frame: Cell::new(false),
            area: area.clone(),
        });

        let p = player.clone();
        area.connect_realize(move |a| p.on_realize(a));
        let p = player.clone();
        area.connect_unrealize(move |a| {
            a.make_current();
            p.render.borrow_mut().take();
        });
        let p = player.clone();
        area.connect_render(move |a, _| {
            p.on_render(a);
            glib::Propagation::Stop
        });
        Ok(player)
    }

    fn on_realize(&self, area: &gtk::GLArea) {
        area.make_current();
        if let Some(err) = area.error() {
            eprintln!("tvjoe: OpenGL nicht verfügbar: {err}");
            return;
        }
        let mut params = vec![
            RenderParam::ApiType(RenderParamApiType::OpenGl),
            RenderParam::InitParams(OpenGLInitParams {
                get_proc_address: gl::get_proc_address,
                ctx: (),
            }),
        ];
        params.extend(native_display(area));
        let mut ctx = match self.mpv.create_render_context(params) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("tvjoe: mpv-Render-Kontext fehlgeschlagen: {e:?}");
                return;
            }
        };
        let (tx, rx) = async_channel::bounded::<()>(1);
        ctx.set_update_callback(move || {
            let _ = tx.try_send(());
        });
        let weak = area.downgrade();
        glib::spawn_future_local(async move {
            while rx.recv().await.is_ok() {
                match weak.upgrade() {
                    Some(a) => a.queue_render(),
                    None => break,
                }
            }
        });
        *self.render.borrow_mut() = Some(ctx);
        trace("Render-Kontext bereit");
    }

    fn on_render(&self, area: &gtk::GLArea) {
        let render = self.render.borrow();
        let Some(ctx) = render.as_ref() else { return };
        let scale = area.scale_factor();
        let fbo = gl::current_framebuffer();
        if let Err(e) = ctx.render::<()>(fbo, area.width() * scale, area.height() * scale, true) {
            eprintln!("tvjoe: Rendern fehlgeschlagen: {e:?}");
        }
        if !self.first_frame.get() && self.mpv.get_property::<i64>("video-params/w").is_ok() {
            self.first_frame.set(true);
            trace("erstes Bild gezeichnet");
        }
    }

    pub fn load(&self, url: &str) {
        if let Err(e) = self.mpv.command("loadfile", &[url]) {
            eprintln!("tvjoe: Laden fehlgeschlagen: {e:?}");
        }
    }

    /// Kurzbericht zur laufenden Wiedergabe (für Messungen).
    pub fn report(&self) -> String {
        let s = |p: &str| {
            self.mpv
                .get_property::<String>(p)
                .unwrap_or_else(|_| "–".into())
        };
        format!(
            "hwdec={} codec={} {}x{} fps={} vo-drop={}",
            s("hwdec-current"),
            s("video-codec"),
            s("video-params/w"),
            s("video-params/h"),
            s("container-fps"),
            s("frame-drop-count"),
        )
    }
}
