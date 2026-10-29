//! Hauptfenster: Senderliste links, Bild rechts, Einblendung über dem Bild, Tastensteuerung.
//!
//! Tasten (Projektplan M4): Ziffern = Sender wählen (Pause aus den Einstellungen, Enter sofort),
//! Pfeil links/rechts bzw. Bild hoch/runter = Senderwechsel, F = Vollbild, Esc = Vollbild aus,
//! M = stumm, Q = Qualität eine Stufe höher (mit Umbruch), L = Senderliste, I = ganzflächige Info,
//! Pfeil hoch/runter, +/− und Mausrad = Lautstärke, Strg+Q = Beenden.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};
use tvjoe_core::epg::{self, Guide};
use tvjoe_core::jellyfin_lib::Item;
use tvjoe_core::mediathek::Film;
use std::collections::{HashMap, HashSet};

use tvjoe_core::adapter::Driver;
use tvjoe_core::settings::{ADAPTER_TV, SRC_SAT, SRC_TV};
use tvjoe_core::{Channel, Library, Quality, Settings, SourceKind, Strategy, Stream, net, paths};
use tvjoe_core::{tf, tr};

use crate::logos::Logos;
use crate::osd::{
    ChannelInfo, InfoContent, InfoPanel, MessageBar, Origin, Osd, VideoAction, VideoState,
};
use crate::player::{Player, PlayerEvent};
use crate::trace;
use crate::vod::{JellyVod, Vod};
use tvjoe_core::jellyfin::Report;

/// Mindestgröße des Bildbereichs für die Senderliste über dem Bild (sonst eigenes Fenster).
const SIDEBAR_MIN: (i32, i32) = (600, 360);

fn usable(c: &Channel) -> bool {
    c.active && !c.missing && !c.streams.is_empty()
}

fn quality_label(q: Option<Quality>) -> &'static str {
    q.map_or(tr("Qualität unbekannt"), Quality::label)
}

fn number_text(ch: &Channel) -> String {
    ch.number.map(|n| n.to_string()).unwrap_or_default()
}

struct Row {
    id: u64,
    row: gtk::ListBoxRow,
    logo: gtk::Picture,
    /// Laufende Sendung und nächste Sendung (EPG).
    now: gtk::Label,
    next: gtk::Label,
}

/// Laufende Weitergabe des Transportstroms an den Player: SAT>IP-Sitzung bzw. DVB-Karte (mit
/// lesbarer Abstimmung für die Info).
pub enum LiveRelay {
    Sat(tvjoe_core::satip::Relay),
    #[cfg(target_os = "linux")]
    Dvb(tvjoe_core::dvb::Relay, String),
}

impl LiveRelay {
    /// Mitgelesene Programmdaten (EIT) seit dem letzten Abholen.
    fn take_eit(&self) -> Vec<tvjoe_core::ts::EitSection> {
        match self {
            LiveRelay::Sat(r) => r.take_eit(),
            #[cfg(target_os = "linux")]
            LiveRelay::Dvb(r, _) => r.take_eit(),
        }
    }

    /// Vorsilbe der Kanal-IDs aus dem Signal.
    fn signal_prefix(&self) -> &'static str {
        match self {
            LiveRelay::Sat(_) => epg::SAT_PREFIX,
            #[cfg(target_os = "linux")]
            LiveRelay::Dvb(..) => tvjoe_core::dvb::SIGNAL_PREFIX,
        }
    }

    /// Bisher nichts empfangen.
    fn silent(&self) -> bool {
        match self {
            LiveRelay::Sat(r) => r.packets() == 0,
            #[cfg(target_os = "linux")]
            LiveRelay::Dvb(..) => false,
        }
    }

    /// Empfang im Format des SAT>IP-Tunerstatus (Pegel 0–255, Qualität 0–15).
    fn status(&self) -> Option<tvjoe_core::satip::TunerStatus> {
        match self {
            LiveRelay::Sat(r) => r.status(),
            #[cfg(target_os = "linux")]
            LiveRelay::Dvb(r, _) => r.signal().map(|s| tvjoe_core::satip::TunerStatus {
                tuner: 1,
                level: s.level * 255 / 100,
                lock: s.lock,
                quality: s.quality * 15 / 100,
                freq: 0,
            }),
        }
    }

    /// Beenden und auf die Freigabe warten.
    fn close(self) {
        match self {
            LiveRelay::Sat(r) => r.close(),
            #[cfg(target_os = "linux")]
            LiveRelay::Dvb(r, _) => r.close(),
        }
    }
}

/// Uhrzeit in Ortszeit, z. B. „18:10“.
/// Meldung, wenn GTK kein OpenGL für das Videobild bekommt.
fn gl_error_text(detail: &str) -> String {
    let hint = if cfg!(windows) {
        tr(
            "In einer Remotedesktop-Sitzung, in einer VM ohne 3D-Beschleunigung oder ohne Grafiktreiber stellt Windows kein taugliches OpenGL bereit; tvjoe bitte direkt am Rechner starten.",
        )
    } else {
        tr("Grafiktreiber bzw. Mesa prüfen (tvjoe --check).")
    };
    // Unter Windows sagt die GTK-Begründung („libEGL ist nicht verfügbar“) Anwendern nichts.
    if cfg!(windows) {
        tf!("Kein Videobild: OpenGL ist nicht verfügbar. {hint}", hint = hint)
    } else {
        tf!(
            "Kein Videobild: OpenGL ist nicht verfügbar ({detail}). {hint}",
            detail = detail,
            hint = hint
        )
    }
}

/// Empfang eines SAT>IP-Tuners für Bauchbinde und Info.
fn reception_text(t: &tvjoe_core::satip::TunerStatus) -> String {
    if t.lock {
        tf!(
            "Signal {signal} % · Qualität {quality} %",
            signal = t.level_percent(),
            quality = t.quality * 100 / 15
        )
    } else {
        tr("Kein Empfang").to_string()
    }
}

pub fn hm(ts: i64) -> String {
    glib::DateTime::from_unix_local(ts)
        .ok()
        .and_then(|d| d.format("%H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

pub struct App {
    pub window: adw::ApplicationWindow,
    split: adw::OverlaySplitView,
    /// Behälter der Senderliste über dem Bild und die Liste selbst (wandert bei zu kleinem
    /// Fenster in ein eigenes Fenster).
    sidebar: gtk::Box,
    list_scroller: gtk::ScrolledWindow,
    list_window: RefCell<Option<adw::Window>>,
    content_bar: adw::ToolbarView,
    list: gtk::ListBox,
    rows: RefCell<Vec<Row>>,
    osd: Rc<Osd>,
    /// Knöpfe der eingeschalteten Empfangsquellen in der Kopfleiste (Nummer, Knopf).
    source_buttons: RefCell<Vec<(u32, gtk::Button)>>,
    /// Quellenumschalter: Empfangsquellen, dann Mediathek und Jellyfin.
    sources_box: gtk::Box,
    /// Knöpfe der eingeschalteten Bibliotheken (Kennung, Knopf, Bild für das Server-Logo).
    library_buttons: RefCell<Vec<(String, gtk::Button, gtk::Image)>>,
    /// „<“ in der Kopfleiste, solange eine Ebene offen ist: zurück bzw. schließen (Nutzer,
    /// 2026-10-26: die Meldungsleiste überdeckte das Kreuz der Ebene).
    pub layer_back: gtk::Button,
    crop_button: gtk::Button,
    fill_button: gtk::Button,
    /// Ebene über dem Bild (Programm, Mediathek, Verwaltung).
    pub layer: Rc<crate::layer::Layer>,
    /// Bildbereich (für die Mindestgröße der Ebenen).
    video: gtk::Overlay,
    /// Ganzflächige Info (Taste I).
    info: Rc<InfoPanel>,
    /// Meldungsstreifen oben (Lautstärke, Ton, Qualität, Störungen).
    msg: Rc<MessageBar>,
    pub player: Option<Rc<Player>>,
    /// Senderliste der gezeigten Empfangsquelle.
    lib: RefCell<Library>,
    /// Die gerade nicht gezeigten Senderlisten (nach Nummer der Empfangsquelle); beim
    /// Umschalten getauscht, beim ersten Gebrauch geladen.
    others: RefCell<HashMap<u32, Library>>,
    /// Nummer der gezeigten Empfangsquelle (Internet-TV 0, SAT 1, weitere ab 2).
    active: Cell<u32>,
    /// Laufende SAT>IP-Sitzung (Weitergabe an den Player).
    relay: RefCell<Option<LiveRelay>>,
    settings: RefCell<Settings>,
    settings_path: Option<PathBuf>,
    current: Cell<Option<u64>>,
    /// Laufender Stream: URL und ggf. Bitratengrenze (Stufe eines adaptiven Streams).
    current_stream: RefCell<Option<(String, Option<u64>)>>,
    /// Gewünschte Stufe für den laufenden Sender (Ausweichen bei Störungen).
    wanted: Cell<Option<Quality>>,
    /// Für den laufenden Sender schon versuchte URLs.
    tried: RefCell<Vec<String>>,
    digits: RefCell<String>,
    digit_timer: RefCell<Option<glib::SourceId>>,
    sidebar_before_fullscreen: Cell<bool>,
    /// Kennung der Meldung „Leerlauf verhindern“ an GNOME (keine Bildschirmsperre).
    inhibit_cookie: Cell<Option<u32>>,
    /// EPG je Empfangsquelle (Nummer).
    guides: RefCell<HashMap<u32, Rc<Guide>>>,
    /// Weggeklickte Befunde der EPG-Prüfung (bis zum Programmende).
    epg_dismissed: RefCell<HashSet<String>>,
    epg_timer: Cell<bool>,
    /// Laufender Mediathek-Beitrag (statt eines Senders).
    vod: RefCell<Option<Vod>>,
    /// Für die offene Mediathek angehaltener Sender (läuft beim Schließen wieder an).
    held_live: Cell<Option<u64>>,
    /// Prüfer „Bild bewegt sich“: Durchlauf (neuer Sender = neuer Durchlauf) und der Sender,
    /// der nach stehendem Bild einmal neu geladen wurde.
    watch: Cell<u32>,
    picture_retried: Cell<Option<u64>>,
    /// Zeitgeber für die Fortschrittszeile läuft.
    vod_timer: Cell<bool>,
    vod_ticks: Cell<u32>,
    /// Folgende Folgen einer Serie (laufen nacheinander).
    vod_next: RefCell<Vec<Vod>>,
    /// Fernsehen zeitversetzt: Summe der Pausen seit dem Laden (Sekunden) und Beginn der
    /// laufenden Pause.
    /// Zeitversetzt: Abstand zwischen geladenem Ende und Abspielstelle im normalen Live-Betrieb
    /// (Vorrat von mpv), gemessen beim Anhalten bzw. ersten Spulen; `None` = live.
    live_gap: Cell<Option<f64>>,
    /// Zeitgeber für die Steuerzeile im zeitversetzten Fernsehen läuft.
    live_timer: Cell<bool>,
    /// Laufendes Abrufvideo kommt aus der Ablage (lokale Datei).
    vod_local: Cell<bool>,
}

impl App {
    pub fn new(
        application: &adw::Application,
        lib: Library,
        settings: Settings,
        settings_path: Option<PathBuf>,
        player: Option<Rc<Player>>,
    ) -> Rc<Self> {
        crate::icons::install();
        let window = adw::ApplicationWindow::builder()
            .application(application)
            .default_width(1280)
            .default_height(760)
            .title("tvjoe")
            .build();

        let list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar", "tvjoe-list"])
            .selection_mode(gtk::SelectionMode::Single)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        let sidebar = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["tvjoe-sidebar"])
            .build();
        sidebar.append(&scroller);
        crate::dnd::autoscroll(&scroller);

        let osd = Osd::new();
        let msg = MessageBar::new();
        let overlay = gtk::Overlay::new();
        match &player {
            Some(p) => overlay.set_child(Some(&p.area)),
            None => overlay.set_child(Some(&status_page(
                tr("Wiedergabe nicht möglich"),
                tr("libmpv ließ sich nicht starten."),
            ))),
        }
        overlay.add_overlay(&osd.root);
        let info = InfoPanel::new();
        overlay.add_overlay(&info.root);
        let layer = crate::layer::Layer::new();
        overlay.add_overlay(&layer.root);
        overlay.add_overlay(&msg.root);

        let header = adw::HeaderBar::new();
        let sidebar_button = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text(tr("Senderliste (L)"))
            .build();
        header.pack_start(&sidebar_button);
        let manage_button = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text(tr("Sender verwalten (Strg+E)"))
            .build();
        header.pack_start(&manage_button);
        let layer_back = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text(tr("Schließen (Esc)"))
            .visible(false)
            .build();
        header.pack_start(&layer_back);
        // Rechts (pack_end von außen nach innen): Bildgruppe Vollbild | Balken abschneiden |
        // Bild füllen (eingeschaltet mit weißer Linie wie beim Quellenumschalter), Einstellungen,
        // Programm, Info, davor abgesetzt der Quellenumschalter TV | Mediathek | Jellyfin
        // (Nutzer, 2026-10-25 und 2026-10-26).
        let fullscreen_button = gtk::Button::builder()
            .icon_name("view-fullscreen-symbolic")
            .tooltip_text(tr("Vollbild (F)"))
            .css_classes(["tvjoe-source"])
            .build();
        let crop_button = gtk::Button::builder()
            .icon_name(crate::icons::CROP)
            .tooltip_text(tr("Schwarze Balken abschneiden"))
            .css_classes(["tvjoe-source"])
            .build();
        let fill_button = gtk::Button::builder()
            .icon_name("zoom-fit-best-symbolic")
            .tooltip_text(tr("Bild füllen (Z)"))
            .css_classes(["tvjoe-source"])
            .build();
        for (b, on) in [(&crop_button, settings.crop_bars), (&fill_button, settings.fill)] {
            if on {
                b.add_css_class("tvjoe-source-active");
            }
        }
        let view_group = gtk::Box::builder()
            .css_classes(["linked"])
            .margin_start(6)
            .build();
        view_group.append(&fullscreen_button);
        view_group.append(&crop_button);
        view_group.append(&fill_button);
        header.pack_end(&view_group);
        let prefs_button = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text(tr("Einstellungen (Strg+,)"))
            .build();
        header.pack_end(&prefs_button);
        let grid_button = gtk::Button::builder()
            .icon_name("x-office-calendar-symbolic")
            .tooltip_text(tr("Programm (G)"))
            .build();
        header.pack_end(&grid_button);
        // Ablage (Nutzer, 2026-10-28): Festplatte an der Stelle von „Info“; Info bleibt über I
        // und die Bauchbinde.
        let ablage_button = gtk::Button::builder()
            .icon_name(crate::icons::fa_name("solid/hard-drive"))
            .tooltip_text(tr("Ablage (D)"))
            .build();
        header.pack_end(&ablage_button);
        let sources = gtk::Box::builder()
            .css_classes(["linked"])
            .margin_end(18)
            .build();
        // Knöpfe der Empfangsquellen und Bibliotheken setzt `update_media_buttons`.
        header.pack_end(&sources);
        // Kopfleiste über die ganze Breite; darunter liegt die Senderliste über dem Bild
        // (eingeklappter Modus), das Bild behält seine Größe.
        let split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar)
            .content(&overlay)
            .collapsed(true)
            .show_sidebar(settings.sidebar_on_start)
            .min_sidebar_width(280.0)
            .max_sidebar_width(380.0)
            .build();
        let content_bar = adw::ToolbarView::new();
        content_bar.add_top_bar(&header);
        content_bar.set_content(Some(&split));
        split
            .bind_property("show-sidebar", &sidebar_button, "active")
            .bidirectional()
            .sync_create()
            .build();
        window.set_content(Some(&content_bar));

        let app = Rc::new(Self {
            window,
            split,
            sidebar: sidebar.clone(),
            list_scroller: scroller.clone(),
            list_window: RefCell::new(None),
            content_bar,
            list,
            rows: RefCell::new(Vec::new()),
            osd,
            source_buttons: RefCell::new(Vec::new()),
            sources_box: sources.clone(),
            library_buttons: RefCell::new(Vec::new()),
            layer_back: layer_back.clone(),
            crop_button: crop_button.clone(),
            fill_button: fill_button.clone(),
            layer,
            video: overlay.clone(),
            info,
            msg,
            player,
            lib: RefCell::new(lib),
            others: RefCell::new(HashMap::new()),
            active: Cell::new(SRC_TV),
            relay: RefCell::new(None),
            settings: RefCell::new(settings),
            settings_path,
            current: Cell::new(None),
            current_stream: RefCell::new(None),
            wanted: Cell::new(None),
            tried: RefCell::new(Vec::new()),
            digits: RefCell::new(String::new()),
            digit_timer: RefCell::new(None),
            sidebar_before_fullscreen: Cell::new(true),
            inhibit_cookie: Cell::new(None),
            guides: RefCell::new(HashMap::new()),
            epg_dismissed: RefCell::new(HashSet::new()),
            epg_timer: Cell::new(false),
            vod: RefCell::new(None),
            held_live: Cell::new(None),
            watch: Cell::new(0),
            picture_retried: Cell::new(None),
            vod_timer: Cell::new(false),
            vod_ticks: Cell::new(0),
            vod_next: RefCell::new(Vec::new()),
            live_gap: Cell::new(None),
            live_timer: Cell::new(false),
            vod_local: Cell::new(false),
        });
        let weak = Rc::downgrade(&app);
        app.layer_back.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::layer::back_or_close(&app);
            }
        });
        let weak = Rc::downgrade(&app);
        app.osd.set_action_handler(move |a| {
            if let Some(app) = weak.upgrade() {
                app.video_action(a);
            }
        });
        // REC in der Info (Taste I): aufnehmen bzw. beenden, Info mit neuem Stand zeigen.
        let weak = Rc::downgrade(&app);
        app.info.set_record_handler(move || {
            if let Some(app) = weak.upgrade() {
                crate::ablage::toggle_record(&app);
                app.info.hide();
                app.toggle_info();
            }
        });

        // Mit der zuletzt gezeigten Empfangsquelle starten, ist sie aus, mit der ersten
        // eingeschalteten.
        let first = {
            let s = app.settings.borrow();
            let on = s.enabled_sources();
            s.active_source
                .filter(|n| on.contains(n))
                .or(on.first().copied())
                .unwrap_or(SRC_TV)
        };
        if first != SRC_TV {
            app.swap_to(first);
        }
        app.build_rows();
        app.connect_signals(&fullscreen_button, &overlay);
        // Kein taugliches OpenGL (z. B. Remotedesktop, VM ohne 3D, fehlender Grafiktreiber): klare
        // Meldung statt der knappen Fehlerzeile von GTK im Bildfeld.
        if let Some(p) = &app.player {
            let weak = Rc::downgrade(&app);
            p.area.connect_realize(move |area| {
                let (Some(app), Some(e)) = (weak.upgrade(), area.error()) else {
                    return;
                };
                app.msg.show_standing(&gl_error_text(&e.to_string()));
            });
        }
        let weak = Rc::downgrade(&app);
        prefs_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::prefs::open(&app);
            }
        });
        let weak = Rc::downgrade(&app);
        grid_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::grid::open(&app);
            }
        });
        crate::layer::connect(&app);
        // Senderliste: passt sie nicht mehr sinnvoll über das Bild, als eigenes Fenster.
        let weak = Rc::downgrade(&app);
        app.split.connect_show_sidebar_notify(move |split| {
            let Some(app) = weak.upgrade() else { return };
            if split.shows_sidebar() && !app.sidebar_fits() {
                split.set_show_sidebar(false);
                app.open_list_window();
            }
            app.place_message_bar();
        });
        app.place_message_bar();
        // Verkleinert: eine offene Ebene, die nicht mehr passt, wandert in ein Fenster.
        let check = {
            let weak = Rc::downgrade(&app);
            move || {
                let weak = weak.clone();
                glib::idle_add_local_once(move || {
                    if let Some(app) = weak.upgrade() {
                        crate::layer::check_size(&app);
                    }
                });
            }
        };
        let c = check.clone();
        app.window.connect_default_width_notify(move |_| c());
        let c = check.clone();
        app.window.connect_default_height_notify(move |_| c());
        app.window.connect_maximized_notify(move |_| check());
        let weak = Rc::downgrade(&app);
        ablage_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::ablageview::open(&app);
            }
        });
        app.update_media_buttons();
        let weak = Rc::downgrade(&app);
        manage_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::manage::open(&app);
            }
        });
        // Alle Signale halten nur schwache Referenzen. Das Fenster hält das Programmobjekt,
        // bis es zerstört wird; sonst wäre es nach den Start-Zeitgebern freigegeben und
        // Knöpfe und Tasten blieben stumm.
        let keep = app.clone();
        app.window.connect_destroy(move |_| {
            if let Some(r) = keep.relay.take() {
                r.close();
            }
        });
        app
    }

    /// Quellenumschalter (eine Empfangsquelle): offene Ebene schließen, ggf. die Senderliste
    /// tauschen und den zuletzt gesehenen Sender dieser Quelle zeigen. Läuft sie schon,
    /// nichts weiter.
    pub fn switch_live(self: &Rc<Self>, src: u32) {
        crate::diag::line(
            "app",
            &format!(
                "Quellenwechsel von {} nach {src} (Sender {:?}, Abrufvideo {})",
                self.active.get(),
                self.current.get(),
                self.vod.borrow().is_some()
            ),
        );
        crate::layer::close_layer(self);
        if src != self.active.get() {
            self.relay.replace(None);
            self.current.set(None);
            self.swap_to(src);
            self.settings.borrow_mut().active_source = Some(src);
            self.rebuild_list();
            if self.current.get().is_some() {
                return;
            }
        } else if self.vod.borrow().is_none() && self.current.get().is_some() {
            return;
        }
        let id = self
            .current
            .get()
            .or(self.last_channel())
            .filter(|id| self.lib.borrow().channel(*id).is_some_and(usable));
        match id {
            Some(id) => self.play_channel(id),
            None => self.start(None),
        }
    }

    /// Senderliste der Empfangsquelle `src` zeigen (die bisherige wandert zu den übrigen).
    fn swap_to(&self, src: u32) {
        let old = self.active.get();
        if src == old {
            return;
        }
        let next = self
            .others
            .borrow_mut()
            .remove(&src)
            .unwrap_or_else(|| self.load_list(src));
        let prev = std::mem::replace(&mut *self.lib.borrow_mut(), next);
        self.others.borrow_mut().insert(old, prev);
        self.active.set(src);
    }

    /// Senderliste einer Empfangsquelle aus ihrer Datei.
    fn load_list(&self, src: u32) -> Library {
        self.list_path(src)
            .and_then(|p| Library::load(&p).map_err(|e| eprintln!("tvjoe: {e}")).ok())
            .unwrap_or_default()
    }

    /// Datei der Senderliste einer Empfangsquelle.
    fn list_path(&self, src: u32) -> Option<PathBuf> {
        let id = match self.settings.borrow().source(src) {
            Some(a) => a.id.clone(),
            None if src == SRC_TV => ADAPTER_TV.into(),
            // Noch nicht übernommene Empfangsquelle: keine Liste.
            None => return None,
        };
        paths::source_library_path(&id)
    }

    /// Nicht gezeigte Senderlisten vergessen (beim nächsten Gebrauch neu aus der Datei).
    pub fn forget_lists(&self) {
        self.others.borrow_mut().clear();
    }

    /// Treiber einer Empfangsquelle (laut Beschreibung).
    pub fn driver_of(&self, src: u32) -> Driver {
        match src {
            SRC_TV => Driver::Iptv,
            SRC_SAT => Driver::Satip,
            _ => {
                let kind = self.settings.borrow().source(src).map(|a| a.kind.clone());
                let dir = paths::config_dir().map(|d| d.join("adapter"));
                kind.and_then(|k| tvjoe_core::adapter::find(&k, dir.as_deref()))
                    .map_or(Driver::Iptv, |d| d.driver)
            }
        }
    }

    /// Zuletzt gesehener Sender der laufenden Empfangsquelle.
    fn last_channel(&self) -> Option<u64> {
        let s = self.settings.borrow();
        s.source(self.active.get()).and_then(|a| a.last_channel)
    }

    /// Für die Sicherung: Einstellungen und die Senderlisten von Internet-TV und SAT.
    pub fn backup(&self) -> tvjoe_core::backup::Backup {
        let tv = self.with_list(SRC_TV, Library::clone);
        let sat = self.with_list(SRC_SAT, Library::clone);
        tvjoe_core::backup::Backup::new(self.settings.borrow().clone(), tv, sat)
    }

    /// Sicherung übernehmen: Einstellungen und Senderlisten ersetzen und speichern (die
    /// bisherigen Dateien bleiben als `.bak`), Senderliste neu aufbauen, Sender starten.
    pub fn apply_backup(self: &Rc<Self>, b: tvjoe_core::backup::Backup) {
        self.relay.replace(None);
        self.current.set(None);
        for (lib, path) in [
            (&b.tv, paths::library_path()),
            (&b.sat, paths::satip_library_path()),
        ] {
            if let Some(p) = path {
                if let Err(e) = lib.save(&p) {
                    eprintln!("tvjoe: {e}");
                }
            }
        }
        *self.settings.borrow_mut() = b.settings;
        self.settings.borrow_mut().active_source = Some(self.active.get());
        self.save_settings();
        for (src, lib) in [(SRC_TV, b.tv), (SRC_SAT, b.sat)] {
            self.set_list(src, lib);
        }
        self.rebuild_list();
        self.update_media_buttons();
        self.update_inhibit();
        if let Some(p) = &self.player {
            p.set_deinterlace(self.settings.borrow().deinterlace);
        }
        self.load_epg(false);
    }

    /// Senderliste einer Empfangsquelle ersetzen (gezeigt oder nicht).
    fn set_list(&self, src: u32, lib: Library) {
        if src == self.active.get() {
            *self.lib.borrow_mut() = lib;
        } else {
            self.others.borrow_mut().insert(src, lib);
        }
    }

    /// Nach einem Suchlauf: Senderliste der Empfangsquelle neu einlesen.
    pub fn reload_list(self: &Rc<Self>, src: u32) {
        let lib = self.load_list(src);
        self.set_list(src, lib);
        if src == self.active.get() {
            self.rebuild_list();
        }
        if src == SRC_SAT {
            self.assign_sat_logos();
        }
    }

    /// SAT-Sender ohne Logo: Logo des gleichen Programmkanals der TV-Liste, aus dem Programm
    /// oder aus der Kodinerds-Sammlung (deren Verzeichnis im Hintergrund, höchstens wöchentlich).
    fn assign_sat_logos(self: &Rc<Self>) {
        let Some(cache) = paths::cache_dir() else {
            return;
        };
        let sat_empty = self.with_list(SRC_SAT, |l| l.channels.is_empty());
        if sat_empty {
            return;
        }
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(tvjoe_core::logomatch::load_kodinerds(&cache));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(kodinerds) = rx.recv().await else {
                return;
            };
            let Some(app) = weak.upgrade() else { return };
            let guide = app.guide_of(SRC_SAT).or_else(|| app.guide_of(SRC_TV));
            let tv = app.with_list(SRC_TV, Library::clone);
            let n = {
                let mut others = app.others.borrow_mut();
                let mut shown = app.lib.borrow_mut();
                let sat: &mut Library = if app.active.get() == SRC_SAT {
                    &mut shown
                } else {
                    others.entry(SRC_SAT).or_insert_with(|| app.load_list(SRC_SAT))
                };
                let n = tvjoe_core::logomatch::assign(sat, &tv, guide.as_deref(), &kodinerds);
                if n > 0 {
                    if let Some(p) = paths::satip_library_path() {
                        if let Err(e) = sat.save(&p) {
                            eprintln!("tvjoe: {e}");
                        }
                    }
                }
                n
            };
            trace(&format!("SAT-Logos zugeordnet: {n}"));
            if n > 0 && app.active.get() == SRC_SAT {
                if let Some(dir) = paths::logo_cache_dir() {
                    app.load_logos(dir);
                }
            }
        });
    }

    /// Mediathek oder Jellyfin geöffnet bzw. geschlossen: weiße Linie nachziehen; nach der
    /// Einstellung das Fernsehen anhalten und beim Schließen wieder starten.
    pub fn media_changed(self: &Rc<Self>) {
        let open = crate::layer::open_media(self).is_some();
        if open {
            if self.settings.borrow().stop_live_for_media {
                if let Some((id, _)) = self.current_live() {
                    self.held_live.set(Some(id));
                    self.stop_live();
                }
            }
        } else if let Some(id) = self.held_live.take() {
            // Inzwischen etwas anderes gestartet (Film, Sender): nicht dazwischenfunken.
            if self.vod.borrow().is_none() && self.current.get().is_none() {
                self.play_channel(id);
            }
        }
        self.update_source_indicator();
    }

    /// Weiße Linie unter der laufenden Quelle (TV, SAT, Mediathek, Jellyfin); ist eine
    /// Mediathek offen, unter deren Knopf (Nutzer, 2026-10-27).
    fn update_source_indicator(&self) {
        let media = crate::layer::open_media(self);
        let live = media.is_none() && self.vod.borrow().is_none() && self.current.get().is_some();
        // Kennung der Bibliothek, unter deren Knopf die Linie steht.
        let shown: Option<String> = match (media, self.vod.borrow().as_ref()) {
            (Some(crate::layer::Media::Oer), _) => Some(tvjoe_core::settings::LIBRARY_OER.into()),
            (Some(crate::layer::Media::Server(id)), _) => Some(id),
            (None, Some(Vod::Oer(_))) => Some(tvjoe_core::settings::LIBRARY_OER.into()),
            (None, Some(Vod::Jelly(j))) => Some(j.library.clone()),
            (None, Some(Vod::File(_)) | None) => None,
        };
        let mut buttons: Vec<(gtk::Button, bool)> = self
            .source_buttons
            .borrow()
            .iter()
            .map(|(n, b)| (b.clone(), live && *n == self.active.get()))
            .collect();
        for (id, b, _) in self.library_buttons.borrow().iter() {
            buttons.push((b.clone(), shown.as_deref() == Some(id.as_str())));
        }
        for (b, on) in buttons {
            if on {
                b.add_css_class("tvjoe-source-active");
            } else {
                b.remove_css_class("tvjoe-source-active");
            }
        }
    }

    /// Knöpfe der Mediatheken nach den Einstellungen ein-/ausblenden; Jellyfin-Logo laden.
    pub fn update_media_buttons(self: &Rc<Self>) {
        let (libraries, on) = {
            let s = self.settings.borrow();
            (s.enabled_libraries(), s.enabled_sources())
        };
        self.build_source_buttons(&on);
        // Gezeigte Empfangsquelle ausgeschaltet: auf die erste eingeschaltete wechseln; sind
        // alle aus, nur ein Hinweis (Nutzer, 2026-10-27).
        if !on.contains(&self.active.get()) && self.vod.borrow().is_none() {
            if let Some(&first) = on.first() {
                self.switch_live(first);
            } else {
                self.stop_live();
                self.message(tr("Keine Empfangsquelle eingeschaltet – Einstellungen → Empfang"));
            }
        }
        self.build_library_buttons(&libraries);
        if libraries.iter().any(|l| crate::jellyfin::is_server_kind(&l.kind)) {
            // Logo vom Server nur bei Jellyfin (Plex: Server-Symbol bzw. eigenes Symbol).
            for l in libraries.iter().filter(|l| l.kind == "jellyfin") {
                crate::jellyfin::load_logo(self, &l.id);
            }
            crate::jellyfin::start_background(self);
        }
    }

    /// Knöpfe der eingeschalteten Bibliotheken hinter die der Empfangsquellen setzen.
    fn build_library_buttons(self: &Rc<Self>, libraries: &[tvjoe_core::settings::Library]) {
        for (_, b, _) in self.library_buttons.borrow_mut().drain(..) {
            self.sources_box.remove(&b);
        }
        for l in libraries {
            let jelly = crate::jellyfin::is_server_kind(&l.kind);
            // Server: Logo des Servers (einfarbig), bis es geladen ist ein Server-Symbol.
            // Plex und Emby: Filmklappe (deren Logos nicht verändert werden dürfen); Jellyfin: bis
            // das Logo des Servers geladen ist ein Server-Symbol.
            let clapper = crate::icons::fa_name("solid/clapperboard");
            let icon = if !l.icon.is_empty() {
                l.icon.as_str()
            } else if matches!(l.kind.as_str(), "plex" | "emby") {
                clapper.as_str()
            } else if jelly {
                "network-server-symbolic"
            } else {
                "folder-videos-symbolic"
            };
            let image = gtk::Image::builder().icon_name(icon).pixel_size(18).build();
            let title = if !l.title.is_empty() {
                l.title.clone()
            } else if jelly {
                crate::jellyfin::kind_name(&l.kind).to_string()
            } else {
                tr("ÖR-Mediathek").to_string()
            };
            let b = gtk::Button::builder()
                .child(&image)
                .tooltip_text(&title)
                .css_classes(["tvjoe-source"])
                .build();
            let (weak, id) = (Rc::downgrade(self), l.id.clone());
            b.connect_clicked(move |_| {
                let Some(app) = weak.upgrade() else { return };
                if !jelly {
                    crate::mediathek::open(&app);
                } else if crate::jellyfin::account(&id).is_none() {
                    crate::mediatheken::open(&app, &id, &app.window);
                } else {
                    crate::jfview::open(&app, &id);
                }
            });
            self.sources_box.append(&b);
            self.library_buttons
                .borrow_mut()
                .push((l.id.clone(), b, image));
        }
        self.update_source_indicator();
    }

    /// Knöpfe der eingeschalteten Empfangsquellen vor Mediathek und Jellyfin setzen.
    fn build_source_buttons(self: &Rc<Self>, on: &[u32]) {
        let current: Vec<u32> = self.source_buttons.borrow().iter().map(|(n, _)| *n).collect();
        let titles: Vec<(u32, String, Driver)> = on
            .iter()
            .map(|n| (*n, self.source_title(*n), self.driver_of(*n)))
            .collect();
        // Gewähltes Symbol, sonst die Vorgabe nach Treiber.
        let icons: Vec<String> = titles
            .iter()
            .map(|(n, _, driver)| {
                let s = self.settings.borrow();
                s.source(*n)
                    .map(|a| a.icon.clone())
                    .filter(|i| !i.is_empty())
                    .unwrap_or_else(|| default_source_icon(*driver))
            })
            .collect();
        // Gleiche Quellen, gleiche Symbole: nur Titel nachziehen.
        let same_icons = self.source_buttons.borrow().len() == icons.len()
            && self
                .source_buttons
                .borrow()
                .iter()
                .zip(&icons)
                .all(|((_, b), i)| b.icon_name().as_deref() == Some(i.as_str()));
        if current == on && same_icons {
            for ((_, b), (_, t, _)) in self.source_buttons.borrow().iter().zip(&titles) {
                b.set_tooltip_text(Some(t));
            }
            return;
        }
        for (_, b) in self.source_buttons.borrow_mut().drain(..) {
            self.sources_box.remove(&b);
        }
        let mut after: Option<gtk::Button> = None;
        for ((n, title, _), icon) in titles.into_iter().zip(icons) {
            let b = gtk::Button::builder()
                .icon_name(icon.as_str())
                .tooltip_text(&title)
                .css_classes(["tvjoe-source"])
                .build();
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.switch_live(n);
                }
            });
            self.sources_box.insert_child_after(&b, after.as_ref());
            after = Some(b.clone());
            self.source_buttons.borrow_mut().push((n, b));
        }
        self.update_source_indicator();
    }

    /// Anzeigename einer Empfangsquelle (Kopfleiste, Meldungen).
    pub fn source_title(&self, src: u32) -> String {
        match src {
            SRC_TV => tr("Fernsehen").to_string(),
            SRC_SAT => tr("Satellit (SAT>IP)").to_string(),
            _ => {
                let kind = self.settings.borrow().source(src).map(|a| a.kind.clone());
                let dir = paths::config_dir().map(|d| d.join("adapter"));
                kind.and_then(|k| tvjoe_core::adapter::find(&k, dir.as_deref()))
                    .map_or_else(|| format!("#{src}"), |d| d.name)
            }
        }
    }

    /// Logo eines Servers als weiße Fläche in den Knopf seiner Bibliothek (nur ohne eigenes
    /// Symbol).
    pub fn set_library_logo(&self, library: &str, path: &Path) {
        let own_icon = self
            .settings
            .borrow()
            .library(library)
            .is_some_and(|l| !l.icon.is_empty());
        if own_icon {
            return;
        }
        let Some(t) = crate::logos::white_from_dark(path) else { return };
        for (id, _, image) in self.library_buttons.borrow().iter() {
            if id == library {
                image.set_paintable(Some(&t));
            }
        }
    }

    /// Größe des Bildbereichs (logische Pixel).
    pub fn video_area_size(&self) -> (i32, i32) {
        (self.video.width(), self.video.height())
    }

    /// Ebene offen: Bild nicht zeichnen (schwarz, Ton läuft weiter), Einblendungen und
    /// Senderliste weg. Geschlossen: Bild wieder zeichnen.
    pub fn set_layer_active(&self, on: bool) {
        if let Some(p) = &self.player {
            p.set_drawing(!on);
        }
        // Bauchbinde bei offener Ebene gesperrt (sie läge sonst unter der Meldung unten links).
        self.osd.set_blocked(on);
        if on {
            self.info.hide();
            self.split.set_show_sidebar(false);
        }
        // Meldungen bei offener Ebene klein unten links, etwa ein Viertel der Breite
        // (Nutzer, 2026-10-27: der Balken lag über Reitern und Knöpfen; 2026-10-28: oben links
        // verdeckte er die Ablage).
        let width = (self.video.width() / 4).max(280);
        self.msg.set_compact(on.then_some(width));
    }

    /// EPG der gezeigten Empfangsquelle.
    pub fn guide(&self) -> Option<Rc<Guide>> {
        self.guide_of(self.active.get())
    }

    /// Befunde der EPG-Prüfung für die gezeigte Empfangsquelle, ohne weggeklickte.
    pub fn epg_warnings(&self) -> Vec<String> {
        let dismissed = self.epg_dismissed.borrow();
        self.guide()
            .map(|g| g.warnings.iter().filter(|w| !dismissed.contains(*w)).cloned().collect())
            .unwrap_or_default()
    }

    /// Klickt die gezeigten Befunde der EPG-Prüfung weg; neue Befunde erscheinen wieder.
    pub fn dismiss_epg_warnings(&self) {
        let shown = self.epg_warnings();
        self.epg_dismissed.borrow_mut().extend(shown);
    }

    /// EPG einer Empfangsquelle (Nummer).
    pub fn guide_of(&self, src: u32) -> Option<Rc<Guide>> {
        self.guides.borrow().get(&src).cloned()
    }

    pub fn settings(&self) -> std::cell::Ref<'_, Settings> {
        self.settings.borrow()
    }

    pub fn settings_mut(&self) -> std::cell::RefMut<'_, Settings> {
        self.settings.borrow_mut()
    }

    /// Lädt das EPG je eingeschalteter Empfangsquelle im Hintergrund: ihre XMLTV-Quellen
    /// (Cache, bei Bedarf aus dem Netz, im Abstand der Quelle), bei SAT dazu die Programmdaten
    /// aus dem Signal. `force` lädt neu.
    pub fn load_epg(self: &Rc<Self>, force: bool) {
        let jobs: Vec<(u32, Vec<String>, u32, String)> = {
            let s = self.settings.borrow();
            s.enabled_sources()
                .into_iter()
                .filter_map(|n| s.source(n))
                .map(|a| (a.num, s.xmltv_locations(&a.id), a.xmltv_hours, a.id.clone()))
                .collect()
        };
        let cache = paths::cache_dir();
        for (src, sources, hours, id) in jobs {
            // Programmdaten aus dem Signal (SAT>IP, DVB-Karte; Zwischenspeicher je Quelle).
            let signal = cache
                .as_deref()
                .map(|d| epg::signal_cache_path(d, &id))
                .filter(|p| p.exists());
            if sources.is_empty() && signal.is_none() {
                // Ohne Quelle kein EPG (wie „nicht geladen“).
                self.guides.borrow_mut().remove(&src);
                continue;
            }
            let max_age = if force {
                Some(Duration::ZERO)
            } else {
                (hours > 0).then(|| Duration::from_secs(u64::from(hours) * 3600))
            };
            let cache = cache.clone();
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let (mut guide, notes) = epg::load_sources(&sources, cache.as_deref(), max_age);
                // Programmdaten aus dem Signal dazu (Sender mit EPG-Quelle „aus dem Signal“).
                if let Some(path) = signal {
                    guide.merge(epg::SatEpg::load_from(&path).guide());
                }
                let _ = tx.send_blocking((guide, notes));
            });
            let weak = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                if let Ok((guide, notes)) = rx.recv().await {
                    for n in &notes {
                        trace(&format!("EPG Quelle {src}: {n}"));
                    }
                    if let Some(app) = weak.upgrade() {
                        app.set_guide(src, guide);
                    }
                }
            });
        }
    }

    fn set_guide(self: &Rc<Self>, src: u32, guide: Guide) {
        self.guides.borrow_mut().insert(src, Rc::new(guide));
        self.refresh_epg();
        self.assign_sat_logos();
        if !self.epg_timer.replace(true) {
            let weak = Rc::downgrade(self);
            glib::timeout_add_seconds_local(60, move || match weak.upgrade() {
                Some(app) => {
                    app.merge_live_eit();
                    app.refresh_epg();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
        }
    }

    /// Beim Ansehen eines SAT-Senders mitgelesene Programmdaten übernehmen (nur für Sender mit
    /// EPG-Quelle „SAT“ sichtbar): in den Zwischenspeicher und ins laufende EPG.
    fn merge_live_eit(&self) {
        let (sections, prefix) = match self.relay.borrow().as_ref() {
            Some(r) => (r.take_eit(), r.signal_prefix()),
            None => return,
        };
        if sections.is_empty() {
            return;
        }
        let Some(dir) = paths::cache_dir() else { return };
        let src = self.active.get();
        let id = self.settings.borrow().source(src).map(|a| a.id.clone()).unwrap_or_default();
        let path = epg::signal_cache_path(&dir, &id);
        let now = epg::now();
        let mut cache = epg::SatEpg::load_from(&path);
        cache.add_prefixed(&sections, now, prefix);
        let _ = cache.save_to(&path);
        let mut fresh = epg::SatEpg::default();
        fresh.add_prefixed(&sections, now, prefix);
        let merged = match self.guide_of(src) {
            Some(g) => {
                let mut g = (*g).clone();
                g.merge(fresh.guide());
                g
            }
            None => fresh.guide(),
        };
        self.guides.borrow_mut().insert(src, Rc::new(merged));
    }

    /// Aktualisiert die Zeile „laufende Sendung“ in der Senderliste.
    pub fn refresh_epg(&self) {
        let Some(guide) = self.guide() else { return };
        let lib = self.lib.borrow();
        let now = epg::now();
        for r in self.rows.borrow().iter() {
            let (current, next) = lib
                .channel(r.id)
                .and_then(|c| guide.match_channel(c))
                .map(|(id, _)| {
                    let (c, n) = guide.now_next(&id, now);
                    (c.cloned(), n.cloned())
                })
                .unwrap_or((None, None));
            match current {
                Some(p) => {
                    r.now
                        .set_label(&tf!(
                            "{title} · bis {end}",
                            title = p.title,
                            end = hm(p.stop)
                        ));
                    r.now.set_tooltip_text(Some(&p.title));
                    r.now.set_visible(true);
                }
                None => r.now.set_visible(false),
            }
            match next {
                Some(p) => {
                    r.next
                        .set_label(&tf!(
                            "Danach {start}  {title}",
                            start = hm(p.start),
                            title = p.title
                        ));
                    r.next.set_tooltip_text(Some(&p.title));
                    r.next.set_visible(true);
                }
                None => r.next.set_visible(false),
            }
        }
    }

    /// Der gerade laufende Stream eines Senders.
    fn current_stream_of<'a>(&self, ch: &'a Channel) -> Option<&'a Stream> {
        let cur = self.current_stream.borrow();
        ch.streams
            .iter()
            .find(|s| cur.as_ref() == Some(&(s.url.clone(), s.max_bitrate)))
    }

    /// Inhalt der Senderanzeige nach den Einstellungen (abgestimmtes Mockup).
    fn channel_info(
        &self,
        ch: &Channel,
        stream: Option<&Stream>,
        extra: Option<String>,
    ) -> ChannelInfo {
        let st = self.settings.borrow();
        let mut meta: Vec<String> = Vec::new();
        if st.osd_category {
            meta.extend(ch.display_group().map(String::from));
        }
        if st.osd_quality {
            meta.push(quality_label(stream.and_then(|s| s.quality)).to_string());
        }
        let is_current = self.vod.borrow().is_none() && self.current.get() == Some(ch.id);
        let shift = if is_current { self.live_state() } else { None };
        // Der Rückstand steht rechts in der Steuerzeile und läuft dort mit (Nutzer, 2026-10-26).
        let reception = stream
            .is_some_and(|s| matches!(s.kind, Some(SourceKind::SatIp | SourceKind::Dvb)))
            .then(|| {
                self.relay
                    .borrow()
                    .as_ref()
                    .and_then(|r| r.status())
                    .map(|t| reception_text(&t))
                    .unwrap_or_default()
            });
        let origin = stream.map(|s| match s.kind {
            Some(SourceKind::SatIp) => Origin::Satellite,
            Some(SourceKind::Dvb) => Origin::Card,
            _ => Origin::Internet,
        });
        let logo = ch
            .logo
            .as_deref()
            .filter(|_| self.logos_shown())
            .and_then(|u| paths::logo_cache_dir().map(|d| d.join(net::cache_name(u, "png"))))
            .filter(|p| p.exists());
        let (mut now, mut desc, mut next) = (None, None, None);
        if let Some(guide) = self.guide() {
            if let Some((id, _)) = guide.match_channel(ch) {
                let (cur, nxt) = guide.now_next(&id, epg::now());
                if st.osd_epg {
                    now = cur.map(|p| {
                        tf!(
                            "Jetzt {start}–{end}  {title}",
                            start = hm(p.start),
                            end = hm(p.stop),
                            title = p.title
                        )
                    });
                    next = nxt.map(|p| {
                        tf!(
                            "Danach {start}  {title}",
                            start = hm(p.start),
                            title = p.title
                        )
                    });
                }
                if st.osd_description {
                    // Absätze zu einem Text zusammenziehen: die Anzeige kürzt dann auf
                    // insgesamt zwei Zeilen (die Zeilengrenze von GTK gilt je Absatz).
                    desc = cur
                        .and_then(|p| p.desc.as_deref().or(p.subtitle.as_deref()))
                        .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "));
                }
            }
        }
        // Aufnahmeknopf nur für den laufenden Sender (nicht bei der Vorschau der Ziffernwahl).
        let record = (self.vod.borrow().is_none() && self.current.get() == Some(ch.id))
            .then(|| crate::ablage::is_recording(ch.id, self.active.get()));
        if record == Some(true) {
            meta.push(tr("● Aufnahme").to_string());
        }
        ChannelInfo {
            number: if st.osd_number {
                number_text(ch)
            } else {
                String::new()
            },
            record,
            origin: origin.filter(|_| st.osd_origin),
            logo,
            name: ch.display_name().to_string(),
            meta: meta.join(" · "),
            reception,
            now,
            desc,
            next,
            extra,
            // Zeitversetzt: Steuerzeile statt „Danach“.
            video: shift,
            full: true,
        }
    }

    /// Nach geänderter Qualitätseinstellung: laufenden Sender neu wählen, falls sich der
    /// passende Stream geändert hat.
    pub fn reapply_quality(self: &Rc<Self>) {
        if self.vod.borrow().is_some() {
            return;
        }
        let Some(id) = self.current.get() else { return };
        let target = {
            let lib = self.lib.borrow();
            let st = self.settings.borrow();
            let wanted = match st.strategy {
                Strategy::Keep => st.quality,
                Strategy::Highest => None,
            };
            lib.channel(id)
                .and_then(|ch| ch.pick_stream_capped(wanted, st.fallback, st.quality_cap))
                .map(|s| (s.url.clone(), s.max_bitrate))
        };
        if target.is_some() && target != *self.current_stream.borrow() {
            self.play_channel(id);
        }
    }

    /// Ziehen und Ablegen: Sender verschieben, Nummern nach Position neu vergeben.
    pub fn move_channel_relative(self: &Rc<Self>, id: u64, target: u64, after: bool) {
        let moved = {
            let mut lib = self.lib.borrow_mut();
            let moved = lib.move_relative(id, target, after);
            if moved {
                lib.renumber();
            }
            moved
        };
        if moved {
            self.library_changed();
            crate::manage::refresh_if_open();
        }
    }

    /// Die SAT-Quelle läuft (Verwaltung und Programm zeigen deren Senderliste).
    /// Nummer der gezeigten Empfangsquelle.
    pub fn active_source(&self) -> u32 {
        self.active.get()
    }

    /// Senderliste einer Empfangsquelle lesen, gleich ob gezeigt.
    pub fn with_list<R>(&self, src: u32, f: impl FnOnce(&Library) -> R) -> R {
        if src == self.active.get() {
            return f(&self.lib.borrow());
        }
        let mut others = self.others.borrow_mut();
        let lib = others.entry(src).or_insert_with(|| self.load_list(src));
        f(lib)
    }

    /// Zahl der Sender einer Empfangsquelle (ohne nicht mehr gefundene).
    pub fn list_len(&self, src: u32) -> usize {
        self.with_list(src, |l| l.channels.iter().filter(|c| !c.missing).count())
    }

    /// M3U-Liste samt ihren Sendern aus der Senderliste einer Empfangsquelle nehmen und speichern.
    pub fn remove_list_source(self: &Rc<Self>, src: u32, id: u64) {
        let shown = self.active.get() == src;
        {
            let mut others = self.others.borrow_mut();
            let mut current = self.lib.borrow_mut();
            let lib: &mut Library = if shown {
                &mut current
            } else {
                others.entry(src).or_insert_with(|| self.load_list(src))
            };
            lib.remove_source(id);
            if let Some(p) = self.list_path(src) {
                if let Err(e) = lib.save(&p) {
                    eprintln!("tvjoe: {e}");
                }
            }
        }
        if shown {
            self.rebuild_list();
        }
    }


    pub fn library(&self) -> std::cell::Ref<'_, Library> {
        self.lib.borrow()
    }

    pub fn library_mut(&self) -> std::cell::RefMut<'_, Library> {
        self.lib.borrow_mut()
    }

    /// Nach Änderungen an der Bibliothek: speichern und die Senderliste neu aufbauen.
    pub fn library_changed(self: &Rc<Self>) {
        if let Some(p) = self.list_path(self.active.get()) {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = self.lib.borrow().save(&p) {
                eprintln!("tvjoe: {e}");
            }
        }
        self.rebuild_list();
    }

    /// Fernsehen beenden (keine Empfangsquelle eingeschaltet): kein Stream, keine SAT-Sitzung.
    fn stop_live(&self) {
        self.relay.replace(None);
        self.current.set(None);
        if let Some(p) = &self.player {
            p.stop();
        }
        self.window.set_title(Some("tvjoe"));
        self.update_source_indicator();
    }

    /// Meldungsleiste neben der Senderliste beginnen lassen, wenn diese über dem Bild liegt (sonst
    /// verdeckt sie den Anfang der Meldung).
    fn place_message_bar(&self) {
        let start = if self.split.shows_sidebar() {
            self.split.max_sidebar_width() as i32 + 16
        } else {
            16
        };
        self.msg.root.set_margin_start(start);
    }

    /// Senderlogos für die laufende Fernsehquelle zeigen (getrennt für Internet und SAT).
    fn logos_shown(&self) -> bool {
        let s = self.settings.borrow();
        s.source(self.active.get()).map_or(true, |a| a.logos)
    }

    /// Senderliste neu aufbauen (Logos, Programm), ggf. ersten Sender starten.
    pub fn rebuild_list(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.rows.borrow_mut().clear();
        self.build_rows();
        if let Some(r) = self
            .rows
            .borrow()
            .iter()
            .find(|r| Some(r.id) == self.current.get())
        {
            self.list.select_row(Some(&r.row));
        }
        if let Some(dir) = tvjoe_core::paths::logo_cache_dir() {
            self.load_logos(dir);
        }
        self.refresh_epg();
        if self.current.get().is_none() {
            self.start(None);
        }
    }

    fn build_rows(self: &Rc<Self>) {
        let lib = self.lib.borrow();
        let mut rows = self.rows.borrow_mut();
        for ch in lib.channels.iter().filter(|c| usable(c)) {
            let num = gtk::Label::builder()
                .label(number_text(ch))
                .css_classes(["tvjoe-num"])
                .width_chars(3)
                .xalign(1.0)
                .build();
            let logo = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Contain)
                .can_shrink(true)
                .width_request(56)
                .height_request(36)
                .build();
            let name = gtk::Label::builder()
                .label(ch.display_name())
                .css_classes(["tvjoe-name"])
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            // Unter dem Namen: laufende Sendung, darunter „Danach …“ (aus dem EPG).
            let epg_line = || {
                gtk::Label::builder()
                    .css_classes(["tvjoe-sub"])
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .visible(false)
                    .build()
            };
            let now = epg_line();
            let next = epg_line();
            let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
            text.set_hexpand(true);
            text.set_valign(gtk::Align::Center);
            text.append(&name);
            text.append(&now);
            text.append(&next);
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            hbox.append(&num);
            if self.logos_shown() {
                hbox.append(&crate::logo_box(&logo, 56));
            }
            hbox.append(&text);
            if ch.favorite {
                hbox.append(
                    &gtk::Label::builder()
                        .label("★")
                        .css_classes(["tvjoe-fav"])
                        .tooltip_text(tr("Favorit"))
                        .build(),
                );
            }
            let row = gtk::ListBoxRow::builder().child(&hbox).build();
            let weak = Rc::downgrade(self);
            crate::dnd::attach(&row, ch.id, move |src, target, after| {
                if let Some(app) = weak.upgrade() {
                    app.move_channel_relative(src, target, after);
                }
            });
            self.list.append(&row);
            rows.push(Row {
                id: ch.id,
                row,
                logo,
                now,
                next,
            });
        }
    }

    fn connect_signals(self: &Rc<Self>, fullscreen_button: &gtk::Button, overlay: &gtk::Overlay) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            let Some(app) = weak.upgrade() else { return };
            let id = app
                .rows
                .borrow()
                .iter()
                .find(|r| &r.row == row)
                .map(|r| r.id);
            if let Some(id) = id {
                app.play_channel(id);
            }
        });

        let weak = Rc::downgrade(self);
        self.crop_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                let (crop, fill) = app.view_mode();
                app.set_view(!crop, fill);
            }
        });
        let weak = Rc::downgrade(self);
        self.fill_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                let (crop, fill) = app.view_mode();
                app.set_view(crop, !fill);
            }
        });

        let w = self.window.clone();
        fullscreen_button.connect_clicked(move |_| w.set_fullscreened(!w.is_fullscreen()));

        let weak = Rc::downgrade(self);
        self.window.connect_fullscreened_notify(move |w| {
            let Some(app) = weak.upgrade() else { return };
            if w.is_fullscreen() {
                app.sidebar_before_fullscreen.set(app.split.shows_sidebar());
                app.split.set_show_sidebar(false);
                app.content_bar.set_reveal_top_bars(false);
            } else {
                app.split
                    .set_show_sidebar(app.sidebar_before_fullscreen.get());
                app.content_bar.set_reveal_top_bars(true);
            }
            app.update_inhibit();
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| match weak.upgrade() {
            Some(app) => app.on_key(key, state),
            None => glib::Propagation::Proceed,
        });
        self.window.add_controller(keys);

        let click = gtk::GestureClick::new();
        let w = self.window.clone();
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |_, n, _, _| {
            let Some(app) = weak.upgrade() else { return };
            // Klicks in einer offenen Ebene gehören ihr.
            if app.layer.is_open() {
                return;
            }
            match n {
                1 => app.show_info(),
                2 => w.set_fullscreened(!w.is_fullscreen()),
                _ => {}
            }
        });
        overlay.add_controller(click);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let weak = Rc::downgrade(self);
        scroll.connect_scroll(move |_, _, dy| {
            let Some(app) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if app.layer.is_open() {
                return glib::Propagation::Proceed;
            }
            app.change_volume(if dy < 0.0 { 5 } else { -5 });
            glib::Propagation::Stop
        });
        overlay.add_controller(scroll);
        // Diagnose: Bedienung aufzeichnen (zuletzt angemeldet, damit vor den eigenen Tasten).
        crate::diag::watch_input(&self.window);

        if let Some(player) = &self.player {
            let p = player.clone();
            crate::diag::start_snapshots(player.mpv, move || p.renders());
            let events = player.spawn_events();
            let weak = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                while let Ok(ev) = events.recv().await {
                    let Some(app) = weak.upgrade() else { break };
                    match ev {
                        PlayerEvent::Failed | PlayerEvent::Ended => app.on_stream_failed(ev),
                        PlayerEvent::Loaded => trace("Stream geöffnet"),
                    }
                }
            });
        }

        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |w| {
            // Laufende oder geplante Aufnahmen, Downloads: erst nachfragen (geplante
            // Aufnahmen gibt es nur, solange tvjoe läuft).
            if let Some(text) = crate::ablage::busy_text().filter(|_| !crate::ablage::close_confirmed()) {
                let dialog = adw::AlertDialog::builder()
                    .heading(tr("tvjoe beenden?"))
                    .body(tf!(
                        "{text}. Aufnahmen gibt es nur, solange tvjoe läuft; laufende werden beim Beenden abgeschlossen, Downloads lassen sich später fortsetzen.",
                        text = text
                    ))
                    .build();
                dialog.add_responses(&[
                    ("cancel", tr("Weiterlaufen lassen")),
                    ("quit", tr("Beenden")),
                ]);
                dialog.set_response_appearance("quit", adw::ResponseAppearance::Destructive);
                dialog.set_default_response(Some("cancel"));
                let win = w.clone();
                dialog.connect_response(None, move |_, r| {
                    if r == "quit" {
                        crate::ablage::confirm_close();
                        win.close();
                    }
                });
                dialog.present(Some(w));
                return glib::Propagation::Stop;
            }
            if let Some(app) = weak.upgrade() {
                app.save_settings();
                app.finish_vod();
            }
            crate::ablage::shutdown();
            glib::Propagation::Proceed
        });
    }

    /// Verbindet den Logo-Thread mit den Zeilen und fordert alle Logos an (nach dem Start).
    pub fn load_logos(self: &Rc<Self>, cache_dir: PathBuf) {
        if !self.logos_shown() {
            return;
        }
        let (logos, results) = Logos::spawn(cache_dir);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok((id, path)) = results.recv().await {
                let Some(app) = weak.upgrade() else { break };
                app.set_logo(id, &path);
            }
        });
        let lib = self.lib.borrow();
        for r in self.rows.borrow().iter() {
            if let Some(url) = lib.channel(r.id).and_then(|c| c.logo.as_deref()) {
                logos.request(r.id, url);
            }
        }
        // Der Thread endet, wenn der Sender fallen gelassen wird; Anfragen sind schon gestellt.
        drop(logos);
    }

    fn set_logo(&self, id: u64, path: &Path) {
        if let Some(r) = self.rows.borrow().iter().find(|r| r.id == id) {
            r.logo.set_filename(Some(path));
        }
    }

    pub fn save_settings(&self) {
        if let Some(p) = &self.settings_path {
            if let Err(e) = self.settings.borrow().save(p) {
                eprintln!("tvjoe: {e}");
            }
        }
    }

    /// Laufender Sender (Kennung, SAT-Liste), nicht bei Abrufvideos.
    pub fn current_live(&self) -> Option<(u64, u32)> {
        if self.vod.borrow().is_some() {
            return None;
        }
        self.current.get().map(|id| (id, self.active.get()))
    }

    /// Stream zum Aufnehmen eines Senders der Fernseh- bzw. SAT-Liste nach den
    /// Qualitätseinstellungen: Name, URL, Bitratengrenze.
    pub fn record_source(&self, channel_id: u64, src: u32) -> Option<(String, String, Option<u64>)> {
        let (q, f, cap) = {
            let st = self.settings.borrow();
            (st.quality, st.fallback, st.quality_cap)
        };
        self.with_list(src, |lib| {
            let ch = lib.channel(channel_id)?;
            let s = ch.pick_stream_capped(q, f, cap)?;
            Some((ch.display_name().to_string(), s.url.clone(), s.max_bitrate))
        })
    }

    /// Laufende Sendung eines Senders laut EPG: Titel, Beginn, Ende.
    pub fn programme_now(&self, channel_id: u64, src: u32) -> Option<(String, i64, i64)> {
        let guide = self.guide_of(src)?;
        self.with_list(src, |lib| {
            let ch = lib.channel(channel_id)?;
            let (id, _) = guide.match_channel(ch)?;
            let (cur, _) = guide.now_next(&id, epg::now());
            cur.map(|p| (p.title.clone(), p.start, p.stop))
        })
    }

    /// Höhe der Bauchbinde (Selbstprobe), 0 wenn ausgeblendet.
    pub fn osd_height(&self) -> i32 {
        if self.osd.is_shown() {
            self.osd.root.height()
        } else {
            0
        }
    }

    /// Kurze Meldung oben im Bild.
    pub fn message(&self, text: &str) {
        self.msg.show(text, self.overlay_secs());
    }

    fn wanted_for_switch(&self) -> Option<Quality> {
        let s = self.settings.borrow();
        match s.strategy {
            Strategy::Keep => s.quality,
            Strategy::Highest => None,
        }
    }

    fn overlay_secs(&self) -> u32 {
        self.settings.borrow().overlay_secs
    }

    /// Startet den ersten Sender: Kanalnummer aus der Kommandozeile, sonst der letzte, sonst der erste.
    pub fn start(self: &Rc<Self>, number: Option<u32>) {
        {
            let s = self.settings.borrow();
            if s.enabled_sources().is_empty() {
                drop(s);
                self.stop_live();
                self.message(tr("Keine Empfangsquelle eingeschaltet – Einstellungen → Empfang"));
                return;
            }
        }
        let id = {
            let lib = self.lib.borrow();
            let s = self.settings.borrow();
            number
                .and_then(|n| lib.by_number(n))
                .or_else(|| {
                    s.resume_last
                        .then_some(self.last_channel())
                        .flatten()
                        .and_then(|id| lib.channel(id))
                })
                .filter(|c| usable(c))
                .or_else(|| lib.channels.iter().find(|c| usable(c)))
                .map(|c| c.id)
        };
        match id {
            Some(id) => self.play_channel(id),
            None if self.active.get() != SRC_TV => {
                if let Some(p) = &self.player {
                    p.stop();
                }
                self.window.set_title(Some("tvjoe"));
                self.update_source_indicator();
                let title = if self.active.get() == SRC_SAT {
                    tr("Keine SAT-Sender").to_string()
                } else {
                    tf!("Keine Sender: {source}", source = self.source_title(self.active.get()))
                };
                self.osd.show(
                    "",
                    &title,
                    tr("Suchlauf unter Einstellungen → Empfang"),
                    0,
                );
            }
            None => {
                self.osd.show(
                    "",
                    tr("Keine Sender"),
                    tr("Sender aus einer M3U übernehmen: tvjoe --import liste.m3u --mit-warnungen"),
                    0,
                );
            }
        }
    }

    pub fn play_channel(self: &Rc<Self>, id: u64) {
        let Some(player) = self.player.clone() else {
            return;
        };
        // Alte SAT>IP-Sitzung zuerst freigeben (Tuner, Empfangsanzeige des neuen Senders).
        self.relay.replace(None);
        let wanted = self.wanted_for_switch();
        let picked = {
            let lib = self.lib.borrow();
            let (fallback, cap) = {
                let st = self.settings.borrow();
                (st.fallback, st.quality_cap)
            };
            lib.channel(id).and_then(|ch| {
                ch.pick_stream_capped(wanted, fallback, cap).map(|s| {
                    (
                        s.url.clone(),
                        s.max_bitrate,
                        ch.display_name().to_string(),
                        self.channel_info(ch, Some(s), None),
                    )
                })
            })
        };
        let Some((url, bitrate, name, info)) = picked else {
            return;
        };
        self.finish_vod();
        self.info.hide();
        self.load_live(&player, &url, bitrate);
        trace(&format!("lade {name} ({url}, hls-bitrate {bitrate:?})"));
        self.tried.replace(vec![url.clone()]);
        self.current_stream.replace(Some((url, bitrate)));
        self.current.set(Some(id));
        self.wanted.set(wanted);
        {
            let mut s = self.settings.borrow_mut();
            let src = self.active.get();
            if let Some(a) = s.source_mut(src) {
                a.last_channel = Some(id);
            }
        }
        self.window
            .set_title(Some(&tf!("{name} – tvjoe", name = name)));
        if let Some(r) = self.rows.borrow().iter().find(|r| r.id == id) {
            self.list.select_row(Some(&r.row));
        }
        self.osd.show_channel(&info, self.overlay_secs());
        self.update_inhibit();
        self.update_source_indicator();
        let retried = self.picture_retried.take() == Some(id);
        self.watch_picture(id, retried);
    }

    /// Prüfer „Bild bewegt sich“ (Nutzer, 2026-10-28; unter Ubuntu 24.04 blieb das Bild nach dem
    /// Wechsel von SAT zu Internet-TV stehen): 8 s nach dem Start zweimal im Abstand von 3 s die
    /// dekodierten und die gezeichneten Bilder zählen. Steht das Bild, den Sender einmal neu
    /// laden; steht es danach immer noch, ein Hinweis. Nicht bei Radio, offener Ebene oder
    /// verkleinertem Fenster.
    fn watch_picture(self: &Rc<Self>, id: u64, retried: bool) {
        let run = self.watch.get().wrapping_add(1);
        self.watch.set(run);
        let sample = move |app: &App| -> Option<(i64, u64)> {
            let p = app.player.as_ref()?;
            let still_ours = app.watch.get() == run
                && app.current.get() == Some(id)
                && app.vod.borrow().is_none();
            let visible = app.window.is_visible()
                && !app.window.is_suspended()
                && !app.layer.is_open();
            (still_ours && visible && p.has_video()).then(|| (p.frames().unwrap_or(0), p.renders()))
        };
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(8), move || {
            let Some(app) = weak.upgrade() else { return };
            let Some(first) = sample(&app) else { return };
            let weak = Rc::downgrade(&app);
            glib::timeout_add_local_once(Duration::from_secs(3), move || {
                let Some(app) = weak.upgrade() else { return };
                let Some(second) = sample(&app) else { return };
                let (frames, renders) = (second.0 - first.0, second.1 - first.1);
                if frames > 0 && renders > 0 {
                    return;
                }
                trace(&format!(
                    "Prüfer: Bild steht (in 3 s {frames} Bilder dekodiert, {renders} gezeichnet){}",
                    if retried { ", schon neu geladen" } else { ", lade den Sender neu" }
                ));
                if retried {
                    app.msg.show(
                        tr("Das Bild steht. Bitte den Sender neu wählen oder tvjoe neu starten."),
                        app.overlay_secs() * 2,
                    );
                } else {
                    app.picture_retried.set(Some(id));
                    app.play_channel(id);
                }
            });
        });
    }

    /// Sender laden; SAT>IP-Adressen über die eigene Sitzung (libmpv kann kein SAT>IP).
    /// Nach 3 s den Empfang in der Senderanzeige nachtragen.
    fn load_live(self: &Rc<Self>, player: &Player, url: &str, bitrate: Option<u64>) {
        crate::diag::line(
            "app",
            &format!("Sender laden: {url}; bisherige Weitergabe vorhanden: {}", self.relay.borrow().is_some()),
        );
        self.relay.replace(None);
        self.live_gap.set(None);
        #[cfg(target_os = "linux")]
        if let Some((_, _, tuning, _)) = tvjoe_core::dvb::split_url(url) {
            // DVB-Karte: Abstimmen kann einige Sekunden dauern (ohne Empfang bis 3 s).
            match tvjoe_core::dvb::Relay::start(url) {
                Ok(r) => {
                    player.load_ts(&r.local_url);
                    self.relay.replace(Some(LiveRelay::Dvb(r, tuning.label())));
                }
                Err(e) => {
                    self.msg.show(
                        &tf!("DVB-Karte: {error}", error = e),
                        self.overlay_secs() * 2,
                    );
                }
            }
            return;
        }
        // Tvheadend: Abspieladresse mit Ticket im Hintergrund holen, dann laden (sofern der
        // Sender inzwischen nicht gewechselt wurde).
        if tvjoe_core::tvheadend::split_url(url).is_some() {
            let (tx, rx) = async_channel::bounded(1);
            let u = url.to_string();
            std::thread::spawn(move || {
                let _ = tx.send_blocking(tvjoe_core::tvheadend::resolve(&u));
            });
            let (weak, url) = (Rc::downgrade(self), url.to_string());
            glib::spawn_future_local(async move {
                let Ok(r) = rx.recv().await else { return };
                let Some(app) = weak.upgrade() else { return };
                let same = app.current_stream.borrow().as_ref().map(|(u, _)| u.as_str()) == Some(url.as_str());
                if !same {
                    return;
                }
                let Some(player) = app.player.as_ref() else { return };
                match r {
                    Ok(play) => player.load(&play, bitrate),
                    Err(e) => app.msg.show(&tf!("Tvheadend: {error}", error = e), app.overlay_secs() * 2),
                }
            });
            return;
        }
        if tvjoe_core::satip::split_url(url).is_none() {
            player.load(url, bitrate);
            return;
        }
        match tvjoe_core::satip::Relay::start(url) {
            Ok(r) => {
                player.load_ts(&r.local_url);
                self.relay.replace(Some(LiveRelay::Sat(r)));
            }
            Err(e) => {
                self.msg
                    .show(&tf!("SAT>IP: {error}", error = e), self.overlay_secs());
                return;
            }
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(3), move || {
            let Some(app) = weak.upgrade() else { return };
            if !app.osd.is_shown() || app.vod.borrow().is_some() {
                return;
            }
            let Some(id) = app.current.get() else { return };
            let lib = app.lib.borrow();
            let Some(ch) = lib.channel(id) else { return };
            let info = app.channel_info(ch, app.current_stream_of(ch), None);
            app.osd.set_reception(info.reception.as_deref());
        });
        // Kommt nach einigen Sekunden nichts an, verwirft meist die Firewall (ufw) den UDP-Strom
        // des Geräts, oder ein WireGuard-Tunnel mit Adressübersetzung (Masquerade) lässt ihn
        // nicht zurück (das Gerät schickt an die übersetzte Adresse; Abhilfe in docs/SAT-IP.md).
        let weak = Rc::downgrade(self);
        let url = url.to_string();
        glib::timeout_add_local_once(Duration::from_secs(6), move || {
            let Some(app) = weak.upgrade() else { return };
            let silent = app
                .relay
                .borrow()
                .as_ref()
                .is_some_and(LiveRelay::silent);
            let same = app
                .current_stream
                .borrow()
                .as_ref()
                .map(|(u, _)| u.as_str())
                == Some(&url);
            if silent && same && app.vod.borrow().is_none() {
                app.msg.show(
                    tr(
                        "Kein Datenstrom vom SAT>IP-Gerät (UDP). Firewall: Einstellungen → Empfang → Prüfen. Über WireGuard: Masquerade im Tunnel verhindert den Rückweg (docs/SAT-IP.md)",
                    ),
                    app.overlay_secs() * 2,
                );
            }
        });
    }

    /// Spielt einen Mediathek-Beitrag (Stufe nach der Qualitätseinstellung). Zurück zum
    /// Fernsehen über Ziffern oder die Senderliste.
    pub fn play_film(self: &Rc<Self>, film: &Film, start: Option<f64>) {
        self.play_vod(Vod::Oer(film.clone()), start, Vec::new());
    }

    /// Serie: erste Folge abspielen, die übrigen laufen danach automatisch.
    pub fn play_series(self: &Rc<Self>, first: &Film, start: Option<f64>, rest: Vec<Film>) {
        let rest = rest.into_iter().map(Vod::Oer).collect();
        self.play_vod(Vod::Oer(first.clone()), start, rest);
    }

    /// Jellyfin-Eintrag abspielen; `rest` (Folgen, Teile einer Sammlung) laufen danach.
    pub fn play_jellyfin(
        self: &Rc<Self>,
        library: &str,
        item: &Item,
        start: Option<f64>,
        rest: Vec<Item>,
    ) {
        let rest = rest
            .into_iter()
            .map(|i| Vod::Jelly(JellyVod::new(library, i)))
            .collect();
        self.play_vod(Vod::Jelly(JellyVod::new(library, item.clone())), start, rest);
    }

    /// Datei aus der Ablage abspielen (Aufnahme).
    pub fn play_file(self: &Rc<Self>, title: &str, name: &str, path: &Path) {
        let vod = Vod::File(crate::vod::FileVod {
            title: title.to_string(),
            name: name.to_string(),
            path: path.to_path_buf(),
        });
        self.play_vod(vod, None, Vec::new());
    }

    fn play_vod(self: &Rc<Self>, vod: Vod, start: Option<f64>, rest: Vec<Vod>) {
        *self.vod_next.borrow_mut() = rest;
        self.start_vod(vod, start);
    }

    fn start_vod(self: &Rc<Self>, vod: Vod, start: Option<f64>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        self.finish_vod();
        self.relay.replace(None);
        // Liegt die Sendung vollständig in der Ablage, spielt die lokale Datei.
        let local = match &vod {
            Vod::Oer(film) => crate::ablage::local_file("oer", &film.url),
            Vod::Jelly(j) => crate::ablage::local_file(&j.library, &j.item.id),
            Vod::File(f) => Some(f.path.clone()),
        };
        self.vod_local.set(local.is_some());
        match (&vod, local) {
            (_, Some(path)) => {
                player.load_from(&path.to_string_lossy(), None, start);
                trace(&format!("lade aus der Ablage {}", path.display()));
            }
            (Vod::Oer(film), None) => {
                let url = film.url_for(self.wanted_for_switch()).to_string();
                player.load_from(&url, None, start);
                trace(&format!("lade Mediathek {} ({url})", film.display_title()));
            }
            (Vod::Jelly(j), None) => self.load_jellyfin(j, start),
            (Vod::File(_), None) => {}
        }
        if let Some(s) = start {
            self.msg.show(
                &tf!("Weiter ab {time}", time = crate::osd::clock(s)),
                self.overlay_secs(),
            );
        }
        self.window
            .set_title(Some(&tf!("{name} – tvjoe", name = vod.title())));
        self.vod.replace(Some(vod));
        self.info.hide();
        self.list.unselect_all();
        self.show_vod_info(self.overlay_secs());
        self.update_inhibit();
        self.update_source_indicator();
        if !self.vod_timer.replace(true) {
            let weak = Rc::downgrade(self);
            glib::timeout_add_local(Duration::from_millis(500), move || {
                let Some(app) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if app.vod.borrow().is_none() {
                    app.vod_timer.set(false);
                    return glib::ControlFlow::Break;
                }
                if app.osd.is_shown() {
                    app.osd.update_video(&app.video_state());
                }
                // Alle 10 s die Spielposition merken bzw. an Jellyfin melden.
                let ticks = app.vod_ticks.get() + 1;
                app.vod_ticks.set(ticks);
                if ticks % 20 == 0 {
                    app.save_vod_progress(false);
                }
                glib::ControlFlow::Continue
            });
        }
    }

    /// Jellyfin: Quelle beim Server erfragen (Original oder umgerechnet), dann laden und den
    /// Start melden. Der Anmeldekopf geht nur mit diesem Stream an den Jellyfin-Server.
    fn load_jellyfin(self: &Rc<Self>, j: &JellyVod, start: Option<f64>) {
        let Some(acc) = crate::jellyfin::account(&j.library) else {
            self.msg.show(
                &tf!("{server}: nicht angemeldet", server = crate::jellyfin::server_name(&j.library)),
                self.overlay_secs(),
            );
            return;
        };
        let max_kbps = self
            .settings
            .borrow()
            .library(&j.library)
            .map_or(0, |l| l.max_kbps);
        let (id, session) = (j.item.id.clone(), j.session.clone());
        let (tx, rx) = async_channel::bounded(1);
        let a = acc.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(a.playback(&id, max_kbps, &session));
        });
        let weak = Rc::downgrade(self);
        let session = j.session.clone();
        let library = j.library.clone();
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else { return };
            let Some(app) = weak.upgrade() else { return };
            // Inzwischen etwas anderes gewählt?
            let current = match app.vod.borrow().as_ref() {
                Some(Vod::Jelly(j)) if j.session == session => Some(j.item.id.clone()),
                _ => None,
            };
            let Some(item_id) = current else { return };
            match result {
                Ok(pb) => {
                    trace(&format!(
                        "lade {library} {item_id} ({}, {:.1} Mbit/s)",
                        if pb.transcoded {
                            "umgerechnet"
                        } else {
                            "Original"
                        },
                        pb.bitrate as f64 / 1e6
                    ));
                    if let Some(p) = &app.player {
                        p.load_with(
                            &pb.url,
                            None,
                            start,
                            Some(&acc.header_line()),
                        );
                    }
                    if let Some(Vod::Jelly(j)) = app.vod.borrow_mut().as_mut() {
                        j.playback = Some(pb.clone());
                    }
                    let pos = start.unwrap_or(0.0);
                    std::thread::spawn(move || {
                        let r = acc.report(Report::Start, &item_id, &pb, &session, pos, false);
                        if let Err(e) = r {
                            eprintln!("tvjoe: Jellyfin-Start melden: {e}");
                        }
                    });
                    app.show_vod_info(app.overlay_secs());
                }
                Err(e) => app
                    .msg
                    .show(&tf!("{server}: nicht abspielbar ({error})", server = crate::jellyfin::server_name(&library), error = e), 0),
            }
        });
    }

    /// Spielposition merken: ÖR lokal, Jellyfin an den Server (und in den Zwischenspeicher).
    /// `stop`: Wiedergabe endet (Jellyfin: „gestoppt“ melden, Umrechnung beenden).
    pub fn save_vod_progress(&self, stop: bool) {
        let Some(vod) = self.vod.borrow().clone() else {
            return;
        };
        let (pos, dur, paused) = self
            .player
            .as_ref()
            .map_or((None, None, false), |p| p.position());
        let Some(pos) = pos else { return };
        let dur = dur.unwrap_or(vod.duration());
        match vod {
            Vod::Oer(film) => crate::mediathek::save_progress(&film, pos, dur),
            Vod::Jelly(j) => crate::jellyfin::report_position(&j, pos, dur, paused, stop),
            Vod::File(_) => {}
        }
    }

    /// Laufendes Video verlassen (anderer Sender, anderes Video, Beenden).
    pub fn finish_vod(&self) {
        self.save_vod_progress(true);
        self.vod.replace(None);
    }

    fn video_state(&self) -> VideoState {
        let (pos, dur, paused) = self
            .player
            .as_ref()
            .map_or((None, None, false), |p| p.position());
        let duration = dur
            .or_else(|| self.vod.borrow().as_ref().map(Vod::duration))
            .unwrap_or(0.0);
        VideoState {
            position: pos.unwrap_or(0.0),
            duration,
            paused,
            live: None,
        }
    }

    /// Bauchbinde eines Videos im Format der Senderanzeige: Logo | Sender bzw. Serie, darunter
    /// Herkunft und Datum | Titel, Beschreibung, Fortschrittszeile.
    fn vod_info(&self, vod: &Vod) -> ChannelInfo {
        let (osd_logo, osd_description) = {
            let st = self.settings.borrow();
            (st.adapter(ADAPTER_TV).map_or(st.osd_logo, |a| a.logos), st.osd_description)
        };
        let desc = osd_description
            .then(|| {
                vod.description()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|d| !d.is_empty());
        ChannelInfo {
            logo: vod.logo(self).filter(|_| osd_logo),
            name: vod.name(),
            meta: if self.vod_local.get() {
                tf!("{meta} · aus der Ablage", meta = vod.meta())
            } else {
                vod.meta()
            },
            now: Some(vod.title()),
            desc,
            video: Some(self.video_state()),
            full: true,
            ..ChannelInfo::default()
        }
    }

    /// Bauchbinde des laufenden Videos für `secs` Sekunden; bei Pause bleibt sie stehen.
    fn show_vod_info(self: &Rc<Self>, secs: u32) {
        let Some(vod) = self.vod.borrow().clone() else {
            return;
        };
        let info = self.vod_info(&vod);
        let paused = info.video.is_some_and(|v| v.paused);
        self.osd.show_channel(&info, if paused { 0 } else { secs });
    }

    /// Knöpfe und Tasten der Video-Bauchbinde.
    fn video_action(self: &Rc<Self>, action: VideoAction) {
        let Some(player) = self.player.clone() else {
            return;
        };
        if matches!(action, VideoAction::Record) {
            crate::ablage::toggle_record(self);
            self.show_info();
            return;
        }
        // Zeitversetztes Fernsehen: dieselben Knöpfe, bezogen auf den Pufferspeicher.
        if self.vod.borrow().is_none() {
            match action {
                VideoAction::Seek(s) => self.live_seek(s),
                VideoAction::SeekTo(f) => self.live_seek_to(f),
                VideoAction::TogglePause => self.toggle_live_pause(),
                VideoAction::Live => self.go_live(),
                VideoAction::Audio => self.cycle_audio(),
                VideoAction::Info => self.toggle_info(),
                VideoAction::Record => {}
            }
            return;
        }
        match action {
            VideoAction::Seek(secs) => player.seek_relative(secs),
            VideoAction::SeekTo(f) => player.seek_fraction(f),
            VideoAction::TogglePause => {
                player.toggle_pause();
            }
            VideoAction::Audio => self.cycle_audio(),
            // Oben schon behandelt (gilt nur beim Fernsehen).
            VideoAction::Record | VideoAction::Live => {}
            VideoAction::Info => {
                self.toggle_info();
                return;
            }
        }
        self.show_vod_info(self.overlay_secs());
    }

    /// Bildschirmsperre verhindern: im Vollbild immer, im Fenster nur mit der Einstellung
    /// „Bildschirmsperre auch im Fenster verhindern“, solange ein Sender läuft.
    pub fn update_inhibit(&self) {
        let playing = self.current.get().is_some() || self.vod.borrow().is_some();
        let want =
            self.window.is_fullscreen() || (self.settings.borrow().inhibit_windowed && playing);
        let Some(application) = self.window.application() else {
            return;
        };
        match (want, self.inhibit_cookie.get()) {
            (true, None) => {
                let cookie = application.inhibit(
                    Some(&self.window),
                    gtk::ApplicationInhibitFlags::IDLE,
                    Some(tr("Fernsehen läuft")),
                );
                self.inhibit_cookie.set((cookie != 0).then_some(cookie));
                trace(&format!("Bildschirmsperre verhindert (Kennung {cookie})"));
            }
            (false, Some(cookie)) => {
                application.uninhibit(cookie);
                self.inhibit_cookie.set(None);
                trace("Bildschirmsperre wieder erlaubt");
            }
            _ => {}
        }
    }

    /// Stream gestört: nächsten noch nicht versuchten Stream des Senders nehmen.
    fn on_stream_failed(self: &Rc<Self>, ev: PlayerEvent) {
        if self.vod.borrow().is_some() {
            trace(&format!("Mediathek-Video: {ev:?}"));
            if ev == PlayerEvent::Ended {
                // Zu Ende gesehen merken; bei einer Serie bzw. Sammlung der nächste Teil.
                if let Some(vod) = self.vod.borrow_mut().take() {
                    let d = vod.duration().max(1.0);
                    match &vod {
                        Vod::Oer(f) => crate::mediathek::save_progress(f, d, d),
                        Vod::Jelly(j) => crate::jellyfin::report_position(j, d, d, false, true),
                        Vod::File(_) => {}
                    }
                }
                let next = {
                    let mut q = self.vod_next.borrow_mut();
                    (!q.is_empty()).then(|| q.remove(0))
                };
                if let Some(next) = next {
                    let title = next.title();
                    self.start_vod(next, None);
                    self.msg
                        .show(&tf!("Als Nächstes: {title}", title = title), self.overlay_secs());
                    return;
                }
            }
            let text = match ev {
                PlayerEvent::Ended => tr("Beitrag zu Ende"),
                _ => tr("Video nicht abspielbar"),
            };
            self.msg.show(text, self.overlay_secs());
            return;
        }
        let (Some(player), Some(id)) = (self.player.clone(), self.current.get()) else {
            return;
        };
        trace(&format!("Stream gestört: {ev:?}"));
        let found = {
            let lib = self.lib.borrow();
            let (fallback, cap) = {
                let st = self.settings.borrow();
                (st.fallback, st.quality_cap)
            };
            let tried = self.tried.borrow();
            lib.channel(id).map(|ch| {
                let next = ch
                    .streams_ordered(self.wanted.get(), fallback, cap)
                    .into_iter()
                    .find(|s| !tried.contains(&s.url))
                    .map(|s| (s.url.clone(), s.max_bitrate, s.quality));
                let text = match &next {
                    Some((_, _, q)) => {
                        tf!(
                            "Stream gestört – weiche aus auf {quality}",
                            quality = quality_label(*q)
                        )
                    }
                    None => tr("Sender nicht erreichbar – alle Streams versucht").to_string(),
                };
                let stream = next.as_ref().and_then(|(u, b, _)| {
                    ch.streams
                        .iter()
                        .find(|s| &s.url == u && s.max_bitrate == *b)
                });
                (self.channel_info(ch, stream, None), text, next)
            })
        };
        let Some((info, text, next)) = found else {
            return;
        };
        match next {
            Some((url, bitrate, _)) => {
                self.load_live(&player, &url, bitrate);
                self.tried.borrow_mut().push(url.clone());
                self.current_stream.replace(Some((url, bitrate)));
                self.osd.show_channel(&info, self.overlay_secs());
                self.msg.show(&text, self.overlay_secs());
            }
            None => {
                self.osd.show_channel(&info, 0);
                self.msg.show(&text, 0);
            }
        }
    }

    pub fn step(self: &Rc<Self>, delta: i64) {
        let next = {
            let lib = self.lib.borrow();
            let ids: Vec<u64> = lib
                .channels
                .iter()
                .filter(|c| usable(c))
                .map(|c| c.id)
                .collect();
            if ids.is_empty() {
                return;
            }
            let pos = self
                .current
                .get()
                .and_then(|id| ids.iter().position(|&x| x == id));
            let idx = match pos {
                Some(p) => (p as i64 + delta).rem_euclid(ids.len() as i64) as usize,
                None => 0,
            };
            ids[idx]
        };
        self.play_channel(next);
    }

    pub fn digit(self: &Rc<Self>, d: u32) {
        let typed = {
            let mut b = self.digits.borrow_mut();
            if b.len() >= 4 {
                b.clear();
            }
            b.extend(char::from_digit(d, 10));
            b.clone()
        };
        // Vorschau des Zielsenders schon in voller Bauchbinde (alle Daten liegen vor);
        // nach der Pause wird umgeschaltet, die Bauchbinde bleibt gleich.
        let preview = {
            let lib = self.lib.borrow();
            let (wanted, fallback, cap) = {
                let st = self.settings.borrow();
                let wanted = match st.strategy {
                    Strategy::Keep => st.quality,
                    Strategy::Highest => None,
                };
                (wanted, st.fallback, st.quality_cap)
            };
            typed
                .parse::<u32>()
                .ok()
                .and_then(|n| lib.by_number(n))
                .filter(|c| usable(c))
                .map(|ch| {
                    let stream = ch.pick_stream_capped(wanted, fallback, cap);
                    self.channel_info(ch, stream, None)
                })
        };
        match preview {
            Some(info) => self.osd.show_channel(&info, 0),
            None => self.osd.show(&typed, "…", "", 0),
        }
        if let Some(id) = self.digit_timer.take() {
            id.remove();
        }
        let pause = self.settings.borrow().numpad_pause_ms;
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_millis(u64::from(pause)), move || {
            if let Some(app) = weak.upgrade() {
                app.digit_timer.take();
                app.commit_digits();
            }
        });
        *self.digit_timer.borrow_mut() = Some(id);
    }

    fn commit_digits(self: &Rc<Self>) {
        if let Some(id) = self.digit_timer.take() {
            id.remove();
        }
        let typed = self.digits.take();
        let Ok(n) = typed.parse::<u32>() else { return };
        let target = self
            .lib
            .borrow()
            .by_number(n)
            .filter(|c| usable(c))
            .map(|c| c.id);
        match target {
            Some(id) => self.play_channel(id),
            None => {
                self.osd.hide();
                self.msg
                    .show(tr("Kein Sender mit dieser Nummer"), self.overlay_secs());
            }
        }
    }

    pub fn cycle_quality(self: &Rc<Self>) {
        if self.vod.borrow().is_some() {
            return;
        }
        let (Some(player), Some(id)) = (self.player.clone(), self.current.get()) else {
            return;
        };
        let current_q = {
            let lib = self.lib.borrow();
            let cur = self.current_stream.borrow();
            lib.channel(id)
                .and_then(|ch| {
                    ch.streams
                        .iter()
                        .find(|s| cur.as_ref() == Some(&(s.url.clone(), s.max_bitrate)))
                })
                .and_then(|s| s.quality)
        };
        let next = match self.settings.borrow().quality.or(current_q) {
            None | Some(Quality::Uhd) => Quality::Sd,
            Some(Quality::Sd) => Quality::P720,
            Some(Quality::P720) => Quality::P1080,
            Some(Quality::P1080) => Quality::Uhd,
        };
        self.settings.borrow_mut().quality = Some(next);
        self.wanted.set(Some(next));
        let picked = {
            let lib = self.lib.borrow();
            let (fallback, cap) = {
                let st = self.settings.borrow();
                (st.fallback, st.quality_cap)
            };
            lib.channel(id).and_then(|ch| {
                ch.pick_stream_capped(Some(next), fallback, cap).map(|s| {
                    let text = if s.quality == Some(next) {
                        tf!("Qualität {quality}", quality = next.label())
                    } else {
                        tf!(
                            "Qualität {wanted} nicht vorhanden – läuft {actual}",
                            wanted = next.label(),
                            actual = quality_label(s.quality)
                        )
                    };
                    (
                        s.url.clone(),
                        s.max_bitrate,
                        self.channel_info(ch, Some(s), None),
                        text,
                    )
                })
            })
        };
        let Some((url, bitrate, info, text)) = picked else {
            return;
        };
        let target = (url.clone(), bitrate);
        if self.current_stream.borrow().as_ref() != Some(&target) {
            player.load(&url, bitrate);
            self.tried.replace(vec![url]);
            self.current_stream.replace(Some(target));
        }
        self.osd.show_channel(&info, self.overlay_secs());
        self.msg.show(&text, self.overlay_secs());
    }

    pub fn change_volume(&self, delta: i64) {
        let Some(v) = self.player.as_ref().and_then(|p| p.add_volume(delta)) else {
            return;
        };
        self.settings.borrow_mut().volume = v.clamp(0, 130) as u32;
        self.msg.show(&tf!("Lautstärke {volume} %", volume = v), 2);
    }

    /// Nächste Tonspur (Taste A, Knopf in der Video-Bauchbinde).
    fn cycle_audio(&self) {
        let Some(player) = &self.player else { return };
        let text = match player.cycle_audio() {
            Some((n, count, label)) if count > 1 => {
                tf!("Ton: {label} ({n}/{count})", label = label, n = n, count = count)
            }
            Some((_, _, label)) => tf!("Nur eine Tonspur: {label}", label = label),
            None => tr("Keine Tonspur").to_string(),
        };
        self.msg.show(&text, self.overlay_secs());
    }

    fn cycle_subtitles(&self) {
        let Some(player) = &self.player else { return };
        let text = match player.cycle_subtitles() {
            Some(Some((n, count, label))) => {
                tf!("Untertitel: {label} ({n}/{count})", label = label, n = n, count = count)
            }
            Some(None) => tr("Untertitel aus").to_string(),
            None => tr("Keine Untertitel").to_string(),
        };
        self.msg.show(&text, self.overlay_secs());
    }

    fn view_mode(&self) -> (bool, bool) {
        let s = self.settings();
        (s.crop_bars, s.fill)
    }

    /// Balken abschneiden / Bild füllen: Einstellung, Player und Knöpfe abgleichen, Meldung.
    pub fn set_view(&self, crop: bool, fill: bool) {
        let (old_crop, old_fill) = {
            let s = self.settings();
            (s.crop_bars, s.fill)
        };
        if (crop, fill) == (old_crop, old_fill) {
            return;
        }
        {
            let mut s = self.settings_mut();
            s.crop_bars = crop;
            s.fill = fill;
        }
        self.save_settings();
        if let Some(p) = &self.player {
            p.set_view_mode(crop, fill);
        }
        for (b, on) in [(&self.crop_button, crop), (&self.fill_button, fill)] {
            if on {
                b.add_css_class("tvjoe-source-active");
            } else {
                b.remove_css_class("tvjoe-source-active");
            }
        }
        let text = if crop != old_crop {
            if crop {
                tr("Schwarze Balken werden abgeschnitten")
            } else {
                tr("Schwarze Balken bleiben")
            }
        } else if fill {
            tr("Bild füllt das Fenster")
        } else {
            tr("Bild eingepasst")
        };
        self.msg.show(text, 2);
    }

    /// Zeitversetztes Fernsehen: Stand für die Steuerzeile (Leiste vom ältesten Zeitpunkt im
    /// Pufferspeicher bis live, Rückstand in `live`); `None`, solange live geschaut wird.
    fn live_state(&self) -> Option<VideoState> {
        let gap = self.live_gap.get()?;
        let player = self.player.as_ref()?;
        let (start, end, pos) = player.live_window()?;
        let live_edge = (end - gap).max(pos);
        let (_, _, paused) = player.position();
        Some(VideoState {
            position: pos - start,
            duration: (live_edge - start).max(0.0),
            paused,
            live: Some((live_edge - pos).max(0.0)),
        })
    }

    /// Zeitversetzt beginnen: Abstand im Live-Betrieb merken.
    fn enter_timeshift(self: &Rc<Self>) {
        if self.live_gap.get().is_some() {
            return;
        }
        let Some((_, end, pos)) = self.player.as_ref().and_then(|p| p.live_window()) else {
            return;
        };
        self.live_gap.set(Some((end - pos).max(0.0)));
        if !self.live_timer.replace(true) {
            let weak = Rc::downgrade(self);
            glib::timeout_add_local(Duration::from_millis(500), move || {
                let Some(app) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let state = if app.vod.borrow().is_none() {
                    app.live_state()
                } else {
                    None
                };
                match state {
                    Some(v) => {
                        if app.osd.is_shown() {
                            app.osd.update_video(&v);
                        }
                        glib::ControlFlow::Continue
                    }
                    None => {
                        app.live_timer.set(false);
                        glib::ControlFlow::Break
                    }
                }
            });
        }
    }

    /// Leertaste im Fernsehen: anhalten (der Pufferspeicher füllt sich weiter) und
    /// zeitversetzt fortsetzen.
    pub fn toggle_live_pause(self: &Rc<Self>) {
        let Some(player) = self.player.clone() else { return };
        if self.current.get().is_none() {
            return;
        }
        self.enter_timeshift();
        if player.toggle_pause().is_none() {
            return;
        }
        self.show_live_controls();
    }

    /// Im zeitversetzten Fernsehen spulen; vorwärts höchstens bis live.
    pub fn live_seek(self: &Rc<Self>, secs: f64) {
        let Some(player) = self.player.clone() else { return };
        if self.current.get().is_none() {
            return;
        }
        self.enter_timeshift();
        let Some(v) = self.live_state() else { return };
        if secs >= v.live.unwrap_or(0.0) {
            self.go_live();
            return;
        }
        let Some((start, _, pos)) = player.live_window() else { return };
        player.seek_absolute((pos + secs).max(start));
        self.show_live_controls();
    }

    /// Klick in die Leiste: Anteil zwischen ältestem Zeitpunkt und live.
    fn live_seek_to(self: &Rc<Self>, fraction: f64) {
        let Some(player) = self.player.clone() else { return };
        let Some(v) = self.live_state() else { return };
        let Some((start, _, _)) = player.live_window() else { return };
        if fraction >= 0.99 {
            self.go_live();
            return;
        }
        player.seek_absolute(start + fraction.clamp(0.0, 1.0) * v.duration);
        self.show_live_controls();
    }

    /// Bauchbinde mit Steuerzeile; angehalten bleibt sie stehen.
    fn show_live_controls(self: &Rc<Self>) {
        let Some(id) = self.current.get() else { return };
        let paused = self.player.as_ref().is_some_and(|p| p.position().2);
        let info = {
            let lib = self.lib.borrow();
            let Some(ch) = lib.channel(id) else { return };
            self.channel_info(ch, self.current_stream_of(ch), None)
        };
        self.osd
            .show_channel(&info, if paused { 0 } else { self.overlay_secs() * 2 });
    }

    /// Taste Ende, Knopf „Live“: zurück auf live.
    pub fn go_live(self: &Rc<Self>) {
        let Some(player) = self.player.clone() else { return };
        let Some(gap) = self.live_gap.get() else { return };
        if let Some((_, end, _)) = player.live_window() {
            player.seek_absolute((end - gap).max(0.0));
        }
        player.set_paused(false);
        self.live_gap.set(None);
        self.show_info();
    }

    fn toggle_mute(&self) {
        if let Some(muted) = self.player.as_ref().and_then(|p| p.toggle_mute()) {
            self.msg.show(if muted { tr("Ton aus") } else { tr("Ton an") }, 2);
        }
    }

    /// Reicht der Bildbereich für die Senderliste über dem Bild? (Vor dem ersten Anzeigen ja.)
    fn sidebar_fits(&self) -> bool {
        let (w, h) = self.video_area_size();
        (w == 0 && h == 0) || (w >= SIDEBAR_MIN.0 && h >= SIDEBAR_MIN.1)
    }

    /// Senderliste als eigenes Fenster (Hauptfenster zu klein); beim Schließen zurück.
    fn open_list_window(self: &Rc<Self>) {
        if let Some(w) = self.list_window.borrow().as_ref() {
            w.present();
            return;
        }
        self.sidebar.remove(&self.list_scroller);
        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&adw::HeaderBar::new());
        tv.set_content(Some(&self.list_scroller));
        tv.add_css_class("tvjoe-sidebar");
        let win = adw::Window::builder()
            .title(tr("Sender – tvjoe"))
            .default_width(360)
            .default_height(640)
            .content(&tv)
            .build();
        let weak = Rc::downgrade(self);
        win.connect_close_request(move |_| {
            if let Some(app) = weak.upgrade() {
                tv.set_content(None::<&gtk::Widget>);
                app.sidebar.append(&app.list_scroller);
                app.list_window.borrow_mut().take();
            }
            glib::Propagation::Proceed
        });
        *self.list_window.borrow_mut() = Some(win.clone());
        win.present();
    }

    pub fn toggle_sidebar(&self) {
        self.split.set_show_sidebar(!self.split.shows_sidebar());
    }

    /// Ganzflächige Info ein/aus (Taste I, Knopf (i)): Mediathek-Beitrag oder laufende
    /// Sendung aus dem EPG.
    pub fn toggle_info(self: &Rc<Self>) {
        if self.info.is_shown() {
            self.info.hide();
            return;
        }
        let mut content = match self.vod.borrow().clone() {
            Some(vod) => InfoContent {
                logo: vod.logo(self),
                title: vod.title(),
                meta: {
                    let mut m = vod.info_meta();
                    if self.vod_local.get() {
                        m.push(tr("Aus der Ablage (lokale Datei)").to_string());
                    }
                    m
                },
                desc: vod.description().trim().to_string(),
                next: self
                    .vod_next
                    .borrow()
                    .first()
                    .map(|n| tf!("Danach: {title}", title = n.title())),
                record: None,
            },
            None => {
                let Some(id) = self.current.get() else { return };
                let lib = self.lib.borrow();
                let Some(ch) = lib.channel(id) else { return };
                let logo = ch
                    .logo
                    .as_deref()
                    .filter(|_| self.logos_shown())
                    .and_then(|u| {
                        paths::logo_cache_dir().map(|d| d.join(net::cache_name(u, "png")))
                    })
                    .filter(|p| p.exists());
                let (cur, nxt) = self
                    .guide()
                    .and_then(|g| {
                        let (id, _) = g.match_channel(ch)?;
                        let (c, n) = g.now_next(&id, epg::now());
                        Some((c.cloned(), n.cloned()))
                    })
                    .unwrap_or((None, None));
                match cur {
                    Some(p) => {
                        let mut meta = vec![tf!(
                            "{channel} · {start}–{end}",
                            channel = ch.display_name(),
                            start = hm(p.start),
                            end = hm(p.stop)
                        )];
                        meta.extend(p.subtitle.clone());
                        InfoContent {
                            logo,
                            title: p.title.clone(),
                            meta,
                            desc: p.desc.clone().unwrap_or_default().trim().to_string(),
                            next: nxt.map(|n| {
                                tf!("Danach {start}  {title}", start = hm(n.start), title = n.title)
                            }),
                            record: Some(crate::ablage::is_recording(id, self.active.get())),
                        }
                    }
                    None => InfoContent {
                        logo,
                        title: ch.display_name().to_string(),
                        meta: vec![tr("Kein Programm (EPG) für diesen Sender").to_string()],
                        desc: String::new(),
                        next: None,
                        record: Some(crate::ablage::is_recording(id, self.active.get())),
                    },
                }
            }
        };
        if self.vod.borrow().is_none() {
            if let Some(r) = self.relay.borrow().as_ref() {
                if let Some(t) = r.status() {
                    content.meta.push(match r {
                        LiveRelay::Sat(_) => tf!(
                            "{reception} · Tuner {tuner} · {freq} MHz",
                            reception = reception_text(&t),
                            tuner = t.tuner,
                            freq = t.freq
                        ),
                        #[cfg(target_os = "linux")]
                        LiveRelay::Dvb(_, label) => tf!(
                            "{reception} · {tuning}",
                            reception = reception_text(&t),
                            tuning = label
                        ),
                    });
                }
            }
        }
        self.osd.hide();
        self.info.show(&content);
    }

    /// Senderanzeige erneut zeigen (Mausklick aufs Bild).
    pub fn show_info(self: &Rc<Self>) {
        if self.vod.borrow().is_some() {
            self.show_vod_info(self.overlay_secs() * 2);
            return;
        }
        let Some(id) = self.current.get() else { return };
        let lib = self.lib.borrow();
        let Some(ch) = lib.channel(id) else { return };
        let info = self.channel_info(ch, self.current_stream_of(ch), None);
        self.osd.show_channel(&info, self.overlay_secs() * 2);
    }

    fn on_key(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        use gdk::Key;
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        // Offene Ebene: Tasten gehören ihr (Pfeile, Enter, Suche); Esc führt zurück/schließt.
        if self.layer.is_open() {
            if key == Key::Escape {
                crate::layer::back_or_close(self);
                return glib::Propagation::Stop;
            }
            if !(ctrl && key.to_lower() == Key::q) {
                return glib::Propagation::Proceed;
            }
        }
        if let Some(d) = key.to_unicode().and_then(|c| c.to_digit(10)) {
            if !ctrl {
                self.digit(d);
                return glib::Propagation::Stop;
            }
        }
        // Mediathek-Video: Leertaste Pause, ←/→ 10 s, Bild ↑/↓ 1 min (Ziffern führen zurück
        // zum Fernsehen).
        if self.vod.borrow().is_some() && !ctrl {
            let action = match key.to_lower() {
                Key::space => Some(VideoAction::TogglePause),
                Key::Left | Key::KP_Left => Some(VideoAction::Seek(-10.0)),
                Key::Right | Key::KP_Right => Some(VideoAction::Seek(10.0)),
                Key::Page_Up | Key::KP_Page_Up => Some(VideoAction::Seek(-60.0)),
                Key::Page_Down | Key::KP_Page_Down => Some(VideoAction::Seek(60.0)),
                _ => None,
            };
            if let Some(a) = action {
                self.video_action(a);
                return glib::Propagation::Stop;
            }
        }
        // Zeitversetztes Fernsehen: Bild ↑/↓ spult eine Minute (← → wechseln weiter den Sender).
        if self.live_gap.get().is_some() && !ctrl {
            let secs = match key.to_lower() {
                Key::Page_Up | Key::KP_Page_Up => Some(-60.0),
                Key::Page_Down | Key::KP_Page_Down => Some(60.0),
                _ => None,
            };
            if let Some(s) = secs {
                self.live_seek(s);
                return glib::Propagation::Stop;
            }
        }
        match key.to_lower() {
            Key::Return | Key::KP_Enter if !self.digits.borrow().is_empty() => self.commit_digits(),
            Key::Left | Key::KP_Left | Key::Page_Up => self.step(-1),
            Key::Right | Key::KP_Right | Key::Page_Down => self.step(1),
            Key::Down | Key::KP_Down | Key::KP_Subtract | Key::minus => self.change_volume(-5),
            Key::Up | Key::KP_Up | Key::KP_Add | Key::plus => self.change_volume(5),
            Key::q if ctrl => self.window.close(),
            Key::e if ctrl => crate::manage::open(self),
            Key::comma if ctrl => crate::prefs::open(self),
            Key::q => self.cycle_quality(),
            Key::f => self.window.set_fullscreened(!self.window.is_fullscreen()),
            Key::Escape if self.info.is_shown() => self.info.hide(),
            Key::Escape => self.window.set_fullscreened(false),
            Key::m => self.toggle_mute(),
            Key::a => self.cycle_audio(),
            Key::u => self.cycle_subtitles(),
            Key::space => self.toggle_live_pause(),
            Key::r => {
                crate::ablage::toggle_record(self);
                self.show_info();
            }
            Key::End | Key::KP_End => self.go_live(),
            Key::z => {
                let (crop, fill) = self.view_mode();
                self.set_view(crop, !fill);
            }
            Key::l => self.toggle_sidebar(),
            Key::i => self.toggle_info(),
            Key::g => crate::grid::open(self),
            Key::d => crate::ablageview::open(self),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }
}

pub fn status_page(title: &str, text: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("video-display-symbolic")
        .title(title)
        .description(text)
        .build()
}

/// Vorgabe-Symbol einer Empfangsquelle nach Treiber (Kopfleiste, Symbolwahl); Internet-TV und
/// SAT aus Font Awesome Free (Nutzer, 2026-10-28: „network-wired“ und „satellite-dish“).
pub fn default_source_icon(driver: Driver) -> String {
    match driver {
        Driver::Iptv => crate::icons::fa_name("solid/network-wired"),
        Driver::Satip => crate::icons::fa_name("solid/satellite-dish"),
        Driver::LinuxDvb | Driver::Hdhomerun => crate::icons::ANTENNA.to_string(),
        Driver::Tvheadend => crate::icons::fa_name("solid/wand-magic-sparkles"),
    }
}
