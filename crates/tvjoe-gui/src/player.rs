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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerEvent {
    /// Stream geöffnet.
    Loaded,
    /// Stream ließ sich nicht öffnen oder brach mit Fehler ab.
    Failed,
    /// Stream endete (bei Live-TV unerwartet).
    Ended,
}

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
    pub fn new(mute: bool, volume: u32) -> Result<Rc<Self>, libmpv2::Error> {
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
            init.set_option("volume", i64::from(volume.min(130)))?;
            // Stellschraube für Messungen: TVJOE_MPV="option=wert;option=wert".
            for pair in std::env::var("TVJOE_MPV").unwrap_or_default().split(';') {
                if let Some((k, v)) = pair.split_once('=') {
                    if let Err(e) = init.set_option(k.trim(), v.trim()) {
                        eprintln!("tvjoe: mpv-Option {k}={v}: {e:?}");
                    }
                }
            }
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

    /// Lädt einen Stream. `max_bitrate` begrenzt bei adaptiven HLS-Streams die Stufe
    /// (mpv `hls-bitrate`: höchste Stufe mit Bitrate ≤ Grenze); `None` = beste Stufe.
    pub fn load(&self, url: &str, max_bitrate: Option<u64>) {
        let limit = max_bitrate.map_or_else(|| "max".to_string(), |b| b.to_string());
        if let Err(e) = self.mpv.set_property("hls-bitrate", limit.as_str()) {
            eprintln!("tvjoe: hls-bitrate={limit}: {e:?}");
        }
        if let Err(e) = self.mpv.command("loadfile", &[url]) {
            eprintln!("tvjoe: Laden fehlgeschlagen: {e:?}");
        }
    }

    /// Ereignisse aus mpv (eigener Thread, `wait_event` darf nur dort laufen).
    pub fn spawn_events(&self) -> async_channel::Receiver<PlayerEvent> {
        use libmpv2::events::Event;
        use libmpv2::mpv_end_file_reason::{Eof, Error};
        let (tx, rx) = async_channel::unbounded();
        let mpv = self.mpv;
        let _ = mpv.disable_deprecated_events();
        std::thread::spawn(move || {
            loop {
                let ev = match mpv.wait_event(-1.0) {
                    Some(Ok(Event::FileLoaded)) => PlayerEvent::Loaded,
                    Some(Ok(Event::EndFile(r))) if r == Error => PlayerEvent::Failed,
                    Some(Ok(Event::EndFile(r))) if r == Eof => PlayerEvent::Ended,
                    Some(Ok(Event::Shutdown)) => break,
                    _ => continue,
                };
                if tx.send_blocking(ev).is_err() {
                    break;
                }
            }
        });
        rx
    }

    /// Ändert die Lautstärke und gibt die neue zurück.
    pub fn add_volume(&self, delta: i64) -> Option<i64> {
        self.mpv
            .command("add", &["volume", &delta.to_string()])
            .ok()?;
        self.mpv
            .get_property::<f64>("volume")
            .ok()
            .map(|v| v.round() as i64)
    }

    pub fn toggle_mute(&self) -> Option<bool> {
        self.mpv.command("cycle", &["mute"]).ok()?;
        self.mpv.get_property::<bool>("mute").ok()
    }

    /// Auflösung und Codec der laufenden Wiedergabe, z. B. „1920×1080 · H.264“.
    pub fn video_info(&self) -> Option<String> {
        let w = self.mpv.get_property::<i64>("video-params/w").ok()?;
        let h = self.mpv.get_property::<i64>("video-params/h").ok()?;
        let codec = self
            .mpv
            .get_property::<String>("video-format")
            .unwrap_or_default();
        let hw = self
            .mpv
            .get_property::<String>("hwdec-current")
            .unwrap_or_default();
        let hw = if hw.is_empty() || hw == "no" {
            "Software".into()
        } else {
            hw
        };
        Some(format!("{w}×{h} · {} · {hw}", codec.to_uppercase()))
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    /// Prüft ohne Fenster und ohne Ton, ob `hls-bitrate` die Stufe eines adaptiven Streams
    /// wählt (Netz nötig). `cargo test -p tvjoe-gui -- --ignored --nocapture`
    /// Andere URL: `TVJOE_TEST_HLS`; Grenzen und erwartete Höhen passen zu Das Erste.
    #[test]
    #[ignore]
    fn hls_bitrate_waehlt_stufe() {
        let url = std::env::var("TVJOE_TEST_HLS").unwrap_or_else(|_| {
            "https://daserste-live.ard-mcdn.de/daserste/live/hls/de/master.m3u8".into()
        });
        for (limit, expected) in [("2816000", 540), ("5491200", 720), ("max", 1080)] {
            unsafe { libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr()) };
            let mpv = libmpv2::Mpv::with_initializer(|i| {
                i.set_option("vo", "null")?;
                i.set_option("ao", "null")?;
                Ok(())
            })
            .unwrap();
            mpv.set_property("hls-bitrate", limit).unwrap();
            mpv.command("loadfile", &[&url]).unwrap();
            let mut height = None;
            for _ in 0..300 {
                if let Ok(h) = mpv.get_property::<i64>("video-params/h") {
                    height = Some(h);
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            println!("hls-bitrate={limit}: Bildhöhe {height:?}");
            assert_eq!(height, Some(expected));
        }
    }
}
