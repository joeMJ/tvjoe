//! Wiedergabe: libmpv mit der Render-API in einem `GtkGLArea`.
//!
//! mpv dekodiert (wenn möglich in Hardware, `hwdec=auto-safe`) und zeichnet direkt in das
//! Framebuffer-Objekt des GLArea. Neue Bilder meldet mpv über den Update-Callback; daraufhin wird
//! nur das GLArea neu gezeichnet.

use std::cell::{Cell, RefCell};
#[cfg(target_os = "linux")]
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use libmpv2::Mpv;
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};

use crate::crop::{self, Area};
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
    /// Untertitel beim Laden eines Streams (Einstellung); `U` wählt nur für den laufenden.
    subtitles: Cell<bool>,
    /// Schwarze Balken abschneiden / Bild füllen (Einstellung, Knöpfe, Taste Z).
    crop_on: Cell<bool>,
    fill: Cell<bool>,
    tracker: RefCell<crop::Tracker>,
    /// Zählt jedes Laden; Messungen eines früheren Streams werden verworfen.
    generation: Cell<u64>,
    /// Sekunden seit dem Laden (Takt der Messungen).
    ticks: Cell<u32>,
    /// Anzeigegröße des Bildes (`video-params/dw`, `dh`), 0 ohne Bild.
    dims: Cell<(f64, f64)>,
    grabbing: Arc<AtomicBool>,
    /// `--protokoll`: Ablauf nach Pause/Fortsetzen bis zu diesem Zeitpunkt mitschreiben.
    protocol: Cell<bool>,
    protocol_start: Cell<Option<std::time::Instant>>,
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
            // Tonspur bevorzugt Deutsch, wenn ein Stream mehrere hat.
            init.set_option("alang", "de,deu,ger")?;
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
            subtitles: Cell::new(false),
            crop_on: Cell::new(false),
            fill: Cell::new(false),
            tracker: RefCell::new(crop::Tracker::default()),
            generation: Cell::new(0),
            ticks: Cell::new(0),
            dims: Cell::new((0.0, 0.0)),
            grabbing: Arc::new(AtomicBool::new(false)),
            protocol: Cell::new(false),
            protocol_start: Cell::new(None),
            area: area.clone(),
        });
        let p = player.clone();
        area.connect_resize(move |_, _, _| p.apply_view());
        player.start_crop_timer();

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
        self.protocol_line("gezeichnet");
        if !self.first_frame.get() && self.mpv.get_property::<i64>("video-params/w").is_ok() {
            self.first_frame.set(true);
            trace("erstes Bild gezeichnet");
        }
    }

    /// Lädt einen Stream. `max_bitrate` begrenzt bei adaptiven HLS-Streams die Stufe
    /// (mpv `hls-bitrate`: höchste Stufe mit Bitrate ≤ Grenze); `None` = beste Stufe.
    pub fn load(&self, url: &str, max_bitrate: Option<u64>) {
        self.load_from(url, max_bitrate, None);
    }

    /// Wie [`Player::load`], ab `start` Sekunden (Weitersehen). Die Startstelle wird bei jedem
    /// Laden neu gesetzt, damit sie nicht für den nächsten Stream gilt.
    pub fn load_from(&self, url: &str, max_bitrate: Option<u64>, start: Option<f64>) {
        self.load_with(url, max_bitrate, start, None);
    }

    /// Wie [`Player::load_from`], mit Kopfzeile für alle Anfragen dieses Streams (Jellyfin:
    /// Anmeldung). Jedes Laden setzt die Kopfzeilen neu, damit der Schlüssel nie an andere
    /// Server geht.
    pub fn load_with(
        &self,
        url: &str,
        max_bitrate: Option<u64>,
        start: Option<f64>,
        header: Option<&str>,
    ) {
        self.load_full(url, max_bitrate, start, header, false);
    }

    /// Transportstrom eines SAT-Senders (Weitergabe von SAT>IP): Format vorgeben und kürzer
    /// erkennen (1 s statt der Vorgabe), das Bild kommt nach etwa 2 statt 4 s.
    pub fn load_ts(&self, url: &str) {
        self.load_full(url, None, None, None, true);
    }

    fn load_full(
        &self,
        url: &str,
        max_bitrate: Option<u64>,
        start: Option<f64>,
        header: Option<&str>,
        ts: bool,
    ) {
        let (format, analyze) = if ts { ("mpegts", "1") } else { ("", "0") };
        let _ = self.mpv.set_property("demuxer-lavf-format", format);
        let _ = self
            .mpv
            .set_property("demuxer-lavf-analyzeduration", analyze);
        let _ = self
            .mpv
            .command("change-list", &["http-header-fields", "clr", ""]);
        if let Some(h) = header {
            if let Err(e) = self
                .mpv
                .command("change-list", &["http-header-fields", "append", h])
            {
                eprintln!("tvjoe: Kopfzeile: {e:?}");
            }
        }
        let start = start.map_or_else(|| "none".to_string(), |s| format!("{s:.1}"));
        if let Err(e) = self.mpv.set_property("start", start.as_str()) {
            eprintln!("tvjoe: start={start}: {e:?}");
        }
        let limit = max_bitrate.map_or_else(|| "max".to_string(), |b| b.to_string());
        if let Err(e) = self.mpv.set_property("hls-bitrate", limit.as_str()) {
            eprintln!("tvjoe: hls-bitrate={limit}: {e:?}");
        }
        // Ausschnitt gilt nur für den bisherigen Stream.
        self.generation.set(self.generation.get() + 1);
        self.ticks.set(0);
        self.tracker.borrow_mut().reset();
        self.dims.set((0.0, 0.0));
        self.apply_view();
        // Eine mit U gewählte Untertitelspur gilt nicht für den nächsten Stream.
        let sid = if self.subtitles.get() { "auto" } else { "no" };
        let _ = self.mpv.set_property("sid", sid);
        // Eine Pause aus einem Mediathek-Video gilt nicht für den nächsten Stream.
        let _ = self.mpv.set_property("pause", false);
        if let Err(e) = self.mpv.command("loadfile", &[url]) {
            eprintln!("tvjoe: Laden fehlgeschlagen: {e:?}");
        }
    }

    /// Zeilensprung entfernen (Kammstreifen bei 576i/1080i): `auto` wirkt nur bei Bildern,
    /// die als Zeilensprung gekennzeichnet sind; mit VAAPI erledigt das die Grafikeinheit.
    pub fn set_deinterlace(&self, on: bool) {
        let v = if on { "auto" } else { "no" };
        if let Err(e) = self.mpv.set_property("deinterlace", v) {
            eprintln!("tvjoe: deinterlace={v}: {e:?}");
        }
    }

    /// Untertitel ein (mpv wählt die Spur, `sid=auto`) oder aus; gilt sofort und für jeden
    /// weiteren Stream.
    pub fn set_subtitles(&self, on: bool) {
        self.subtitles.set(on);
        let v = if on { "auto" } else { "no" };
        if let Err(e) = self.mpv.set_property("sid", v) {
            eprintln!("tvjoe: sid={v}: {e:?}");
        }
    }

    /// Lautstärke angleichen: +7 dB vor einem Begrenzer bei −1 dBFS (ohne automatische
    /// Pegelanpassung, ohne Vorlauf wie bei `loudnorm`); aus = kein Filter.
    pub fn set_loudness(&self, on: bool) {
        let v = if on {
            "lavfi=[alimiter=level_in=2.239:limit=0.891:level=0]"
        } else {
            ""
        };
        if let Err(e) = self.mpv.set_property("af", v) {
            eprintln!("tvjoe: af={v}: {e:?}");
        }
    }

    /// Balken abschneiden und einpassen/füllen; gilt sofort.
    pub fn set_view_mode(&self, crop_on: bool, fill: bool) {
        if !crop_on {
            self.tracker.borrow_mut().reset();
        } else if !self.crop_on.get() {
            // Gleich neu messen statt erst beim nächsten Zehnertakt.
            self.ticks.set(0);
        }
        self.crop_on.set(crop_on);
        self.fill.set(fill);
        self.apply_view();
    }

    /// Zoom und Verschiebung aus Ausschnitt, Bild- und Fenstergröße neu setzen.
    fn apply_view(&self) {
        let area = if self.crop_on.get() {
            self.tracker.borrow().shown()
        } else {
            Area::FULL
        };
        let window = (f64::from(self.area.width()), f64::from(self.area.height()));
        let v = crop::view(area, self.dims.get(), window, self.fill.get());
        let _ = self.mpv.set_property("video-zoom", v.zoom);
        let _ = self.mpv.set_property("video-pan-x", v.pan_x);
        let _ = self.mpv.set_property("video-pan-y", v.pan_y);
    }

    /// Takt je Sekunde: Bildgröße verfolgen, Einzelbilder messen (nach 1, 2 und 4 s, dann alle
    /// 10 s; ein Formatwechsel im Fernsehen kann jederzeit kommen).
    fn start_crop_timer(self: &Rc<Self>) {
        let (tx, rx) = async_channel::unbounded::<(u64, Option<Area>)>();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_secs(1), move || {
            let Some(p) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            p.crop_tick(&tx);
            glib::ControlFlow::Continue
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok((generation, m)) = rx.recv().await {
                let Some(p) = weak.upgrade() else { break };
                p.crop_measured(generation, m);
            }
        });
    }

    fn crop_tick(&self, tx: &async_channel::Sender<(u64, Option<Area>)>) {
        let dw = self.mpv.get_property::<i64>("video-params/dw").unwrap_or(0) as f64;
        let dh = self.mpv.get_property::<i64>("video-params/dh").unwrap_or(0) as f64;
        if (dw, dh) != self.dims.get() {
            self.dims.set((dw, dh));
            self.apply_view();
        }
        if !self.crop_on.get() || dw <= 0.0 || dh <= 0.0 {
            return;
        }
        let t = self.ticks.get() + 1;
        self.ticks.set(t);
        if !(t == 1 || t == 2 || t == 4 || t % 10 == 0) {
            return;
        }
        if self.grabbing.swap(true, Ordering::AcqRel) {
            return;
        }
        let ctx = self.mpv.ctx.as_ptr() as usize;
        let generation = self.generation.get();
        let grabbing = self.grabbing.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            let m = crop::grab(ctx as *mut libmpv2_sys::mpv_handle);
            grabbing.store(false, Ordering::Release);
            let _ = tx.send_blocking((generation, m));
        });
    }

    fn crop_measured(&self, generation: u64, m: Option<Area>) {
        if generation != self.generation.get() || !self.crop_on.get() {
            return;
        }
        if self.tracker.borrow_mut().feed(m).is_some() {
            self.apply_view();
        }
    }

    /// Pufferspeicher: drei Viertel als Vorrat nach vorn (füllt sich auch während einer
    /// Pause), ein Viertel für Gesehenes; vor dem Abspielen und nach einem Abriss wartet mpv,
    /// bis `start_secs` Sekunden im Vorrat sind.
    pub fn set_buffer(&self, mb: u32, start_secs: u32) {
        let total = u64::from(mb.max(32)) << 20;
        let options = [
            ("cache", "yes".to_string()),
            ("demuxer-max-bytes", (total / 4 * 3).to_string()),
            ("demuxer-max-back-bytes", (total / 4).to_string()),
            ("cache-pause-initial", "yes".to_string()),
            ("cache-pause-wait", start_secs.to_string()),
        ];
        for (k, v) in options {
            if let Err(e) = self.mpv.set_property(k, v.as_str()) {
                eprintln!("tvjoe: {k}={v}: {e:?}");
            }
        }
    }

    /// Ans Ende des Vorrats springen (zurück auf live) und weiterspielen.
    pub fn jump_live(&self) {
        let _ = self.mpv.set_property("pause", false);
        let ahead = self.mpv.get_property::<f64>("demuxer-cache-time").unwrap_or(0.0);
        if ahead > 1.0 {
            let _ = self
                .mpv
                .command("seek", &[&format!("{:.1}", ahead - 1.0), "relative"]);
        }
    }

    /// Wiedergabe beenden (kein Stream).
    pub fn stop(&self) {
        let _ = self.mpv.command("stop", &[]);
    }

    /// Bild zeichnen ein/aus (Ebene offen: schwarz, Ton läuft weiter, Dekodieren geht weiter).
    pub fn set_drawing(&self, on: bool) {
        self.area.set_visible(on);
        if on {
            self.area.queue_render();
        }
    }

    /// Springt um `secs` Sekunden (negativ = zurück).
    pub fn seek_relative(&self, secs: f64) {
        let _ = self.mpv.command("seek", &[&format!("{secs}"), "relative"]);
    }

    /// Springt an einen Anteil der Dauer (0.0–1.0).
    pub fn seek_fraction(&self, fraction: f64) {
        let pct = (fraction.clamp(0.0, 1.0) * 100.0).to_string();
        let _ = self.mpv.command("seek", &[&pct, "absolute-percent"]);
    }

    /// Pause umschalten; gibt zurück, ob jetzt pausiert ist.
    pub fn toggle_pause(self: &Rc<Self>) -> Option<bool> {
        self.mpv.command("cycle", &["pause"]).ok()?;
        let paused = self.mpv.get_property::<bool>("pause").ok();
        if self.protocol.get() {
            self.protocol_begin(if paused == Some(true) { "Pause" } else { "weiter" });
        }
        paused
    }

    pub fn set_protocol(&self, on: bool) {
        self.protocol.set(on);
    }

    /// Protokoll: sechs Sekunden lang alle 100 ms Zustand, dazu jedes Neuzeichnen.
    fn protocol_begin(self: &Rc<Self>, what: &str) {
        let start = std::time::Instant::now();
        self.protocol_start.set(Some(start));
        self.protocol_line(what);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(p) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if p.protocol_start.get() != Some(start) {
                return glib::ControlFlow::Break;
            }
            if start.elapsed() > Duration::from_secs(6) {
                p.protocol_start.set(None);
                return glib::ControlFlow::Break;
            }
            p.protocol_line("Zustand");
            glib::ControlFlow::Continue
        });
    }

    fn protocol_line(&self, what: &str) {
        let Some(start) = self.protocol_start.get() else { return };
        let get = |k: &str| {
            self.mpv
                .get_property::<String>(k)
                .unwrap_or_else(|_| "–".into())
        };
        eprintln!(
            "[tvjoe protokoll +{:6.3} s] {what}: pause={} time-pos={} frame={} drop={} cache={}",
            start.elapsed().as_secs_f64(),
            get("pause"),
            get("time-pos"),
            get("estimated-frame-number"),
            get("frame-drop-count"),
            get("demuxer-cache-time"),
        );
    }

    /// Spuren einer Art (`audio`, `sub`) als (ID, Bezeichnung).
    fn tracks(&self, kind: &str) -> Vec<(i64, String)> {
        let count = self.mpv.get_property::<i64>("track-list/count").unwrap_or(0);
        (0..count)
            .filter(|i| {
                self.mpv
                    .get_property::<String>(&format!("track-list/{i}/type"))
                    .is_ok_and(|t| t == kind)
            })
            .map(|i| {
                let get = |k: &str| {
                    self.mpv
                        .get_property::<String>(&format!("track-list/{i}/{k}"))
                        .ok()
                        .filter(|v| !v.is_empty())
                };
                let id = self
                    .mpv
                    .get_property::<i64>(&format!("track-list/{i}/id"))
                    .unwrap_or(0);
                let label = match (get("title"), get("lang")) {
                    (Some(t), Some(l)) => format!("{t} ({l})"),
                    (Some(t), None) => t,
                    (None, Some(l)) => language_name(&l).to_string(),
                    (None, None) => format!("Spur {id}"),
                };
                (id, label)
            })
            .collect()
    }

    /// Nächste Tonspur wählen; gibt (Nummer, Anzahl, Bezeichnung) zurück.
    pub fn cycle_audio(&self) -> Option<(usize, usize, String)> {
        let tracks = self.tracks("audio");
        if tracks.is_empty() {
            return None;
        }
        // Selbst weiterzählen: `cycle aid` würde auch „kein Ton“ durchlaufen.
        let current = self.mpv.get_property::<i64>("aid").ok();
        let mut pos = tracks
            .iter()
            .position(|(id, _)| Some(*id) == current)
            .unwrap_or(0);
        if tracks.len() > 1 {
            pos = (pos + 1) % tracks.len();
            self.mpv.set_property("aid", tracks[pos].0).ok()?;
        }
        Some((pos + 1, tracks.len(), tracks[pos].1.clone()))
    }

    /// Nächste Untertitelspur wählen, nach der letzten wieder aus. `None`: Stream hat keine;
    /// sonst (Nummer, Anzahl, Bezeichnung) oder `Some(None)` für „aus“.
    pub fn cycle_subtitles(&self) -> Option<Option<(usize, usize, String)>> {
        let tracks = self.tracks("sub");
        if tracks.is_empty() {
            return None;
        }
        let current = self.mpv.get_property::<i64>("sid").ok();
        let next = match tracks.iter().position(|(id, _)| Some(*id) == current) {
            None => Some(0),
            Some(p) if p + 1 < tracks.len() => Some(p + 1),
            Some(_) => None,
        };
        match next {
            Some(pos) => {
                self.mpv.set_property("sid", tracks[pos].0).ok()?;
                Some(Some((pos + 1, tracks.len(), tracks[pos].1.clone())))
            }
            None => {
                self.mpv.set_property("sid", "no").ok()?;
                Some(None)
            }
        }
    }

    /// Position, Dauer (Sekunden) und Pause des laufenden Videos.
    pub fn position(&self) -> (Option<f64>, Option<f64>, bool) {
        (
            self.mpv.get_property::<f64>("time-pos").ok(),
            self.mpv.get_property::<f64>("duration").ok(),
            self.mpv.get_property::<bool>("pause").unwrap_or(false),
        )
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

    /// Kurzbericht zur laufenden Wiedergabe (für Messungen).
    pub fn report(&self) -> String {
        let s = |p: &str| {
            self.mpv
                .get_property::<String>(p)
                .unwrap_or_else(|_| "–".into())
        };
        format!(
            "hwdec={} codec={} format={} {}x{} fps={} vo-drop={}",
            s("hwdec-current"),
            s("video-codec"),
            s("video-params/hw-pixelformat"),
            s("video-params/w"),
            s("video-params/h"),
            s("container-fps"),
            s("frame-drop-count"),
        )
    }
}

/// Deutscher Name einer Sprachkennung (ISO 639), sonst die Kennung.
fn language_name(code: &str) -> &str {
    match code {
        "de" | "deu" | "ger" => "Deutsch",
        "en" | "eng" => "Englisch",
        "fr" | "fra" | "fre" => "Französisch",
        "mis" | "qaa" | "und" => "Original",
        "qad" => "Audiodeskription",
        _ => code,
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

    /// Weitersehen: Eigenschaft `start` vor `loadfile` (wie `Player::load_from`), danach
    /// `none` für den nächsten Stream. Netz nötig; `TVJOE_TEST_MP4` für eine andere Datei.
    #[test]
    #[ignore]
    fn start_setzt_einstieg() {
        let url = std::env::var("TVJOE_TEST_MP4").unwrap_or_else(|_| {
            "https://ctv-videos.daserste.de/dach/2023/05/17/8e9e8d1d-92d6-4bd9-be2d-273e63f01886/JOB_547267__sendeton_1280x720-50p-3200kbit.mp4".into()
        });
        unsafe { libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr()) };
        let mpv = libmpv2::Mpv::with_initializer(|i| {
            i.set_option("vo", "null")?;
            i.set_option("ao", "null")?;
            Ok(())
        })
        .unwrap();
        let pos = |start: &str| {
            mpv.set_property("start", start).unwrap();
            mpv.command("loadfile", &[&url]).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            for _ in 0..200 {
                if let Ok(p) = mpv.get_property::<f64>("time-pos") {
                    return Some(p);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            None
        };
        let at = pos("600.0");
        println!("start=600: time-pos {at:?}");
        assert!(at.is_some_and(|p| (599.0..610.0).contains(&p)));
        let at = pos("none");
        println!("start=none: time-pos {at:?}");
        assert!(at.is_some_and(|p| p < 5.0));
    }
}
