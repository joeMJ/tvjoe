//! Senderanzeige über dem Bild, unten über die ganze Bildbreite (abgestimmtes Mockup):
//! links Nummer | Logo in Schwarzweiß | Name, darunter Quellsymbol, Kategorie und Qualität;
//! rechts „Jetzt“ mit gekürzter Beschreibung und „Danach“. Alle Teile sind einzeln abschaltbar.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use tvjoe_core::tr;

/// Breite der Namensspalte (lange Namen werden mit „…“ gekürzt).
const NAME_WIDTH: i32 = 260;

/// Herkunft des Streams (Symbol vor der Qualität).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Internet,
    Satellite,
    /// DVB-Karte (Antenne, Kabel oder Satellit am Kabel).
    Card,
}

/// Stand eines Mediathek-Videos für die Fortschrittszeile.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct VideoState {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    /// Zeitversetztes Fernsehen: Rückstand zu live (Sekunden); Zeit als „−2:15“, Knopf „Live“.
    pub live: Option<f64>,
}

/// Bedienung über die Knöpfe der Video-Bauchbinde.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VideoAction {
    /// Relativ springen (Sekunden).
    Seek(f64),
    /// An einen Anteil der Dauer springen (Klick in die Leiste).
    SeekTo(f64),
    TogglePause,
    /// Nächste Tonspur.
    Audio,
    /// Ganzflächige Info.
    Info,
    /// Laufenden Sender aufnehmen bzw. Aufnahme beenden.
    Record,
    /// Zeitversetztes Fernsehen: zurück auf live.
    Live,
}

type ActionHandler = Box<dyn Fn(VideoAction)>;

/// „1:05:09“ bzw. „5:09“.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Inhalt der Senderanzeige; `None` bzw. leer blendet den Teil aus.
#[derive(Debug, Default, Clone)]
pub struct ChannelInfo {
    pub number: String,
    pub origin: Option<Origin>,
    pub logo: Option<PathBuf>,
    pub name: String,
    /// Kategorie · Qualität (schon nach den Einstellungen zusammengesetzt).
    pub meta: String,
    /// Dritte Zeile links: Empfang eines SAT-Senders; leer hält den Platz frei (Name und
    /// Qualität bleiben beim Nachtragen an ihrer Stelle), `None` blendet die Zeile aus.
    pub reception: Option<String>,
    pub now: Option<String>,
    pub desc: Option<String>,
    pub next: Option<String>,
    /// Zusätzliche Meldung, z. B. „Stream gestört …“ oder Videodaten.
    pub extra: Option<String>,
    /// Mediathek-Video: Fortschrittszeile mit Knöpfen statt „Danach“.
    pub video: Option<VideoState>,
    /// Laufender Sender: Aufnahmeknopf (`Some(true)` = Aufnahme läuft).
    pub record: Option<bool>,
    /// Bauchbinde eines Senders: rechts immer Platz für Jetzt, zwei Zeilen Beschreibung und
    /// Danach, auch wenn nicht gefüllt (gleiche Höhe beim Zappen).
    pub full: bool,
}

pub struct Osd {
    pub root: gtk::Box,
    number: gtk::Label,
    origin: gtk::Image,
    logo: gtk::Picture,
    logo_slot: gtk::Widget,
    sep_logo: gtk::Separator,
    sep_number: gtk::Separator,
    name: gtk::Label,
    meta: gtk::Label,
    reception: gtk::Label,
    sep_right: gtk::Separator,
    right: gtk::Box,
    now: gtk::Label,
    desc: gtk::Label,
    next: gtk::Label,
    extra: gtk::Label,
    video_row: gtk::Box,
    progress: gtk::ProgressBar,
    time: gtk::Label,
    pause: gtk::Button,
    /// Spulknöpfe (Hinweise unterscheiden sich zwischen Video und zeitversetztem Fernsehen).
    seek_buttons: [gtk::Button; 4],
    live: gtk::Button,
    rec: gtk::Button,
    on_action: RefCell<Option<ActionHandler>>,
    timer: RefCell<Option<glib::SourceId>>,
    /// Ebene offen: die Bauchbinde erscheint nicht (Meldungen stehen dann unten links).
    blocked: Cell<bool>,
}

fn osd_button(icon: &str, tip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tip)
        .css_classes(["flat", "circular", "tvjoe-osd-button"])
        .valign(gtk::Align::Center)
        .build()
}

fn label(class: &str) -> gtk::Label {
    gtk::Label::builder()
        .css_classes([class])
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build()
}

fn vsep() -> gtk::Separator {
    gtk::Separator::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["tvjoe-osd-sep"])
        .build()
}

impl Osd {
    pub fn new() -> Rc<Self> {
        // Feste Spaltenbreiten, damit der Programmtext bei jedem Sender an derselben Stelle beginnt.
        let number = gtk::Label::builder()
            .css_classes(["tvjoe-osd-number"])
            .width_chars(3)
            .xalign(0.5)
            .build();
        number.set_valign(gtk::Align::Center);

        let logo = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .height_request(40)
            .build();
        // In Breite und Höhe begrenzt: hochkante Logos (z. B. ARTE) dürfen die Anzeige nicht strecken.
        let logo_slot: gtk::Widget = adw::Clamp::builder()
            .orientation(gtk::Orientation::Vertical)
            .maximum_size(40)
            .tightening_threshold(40)
            .child(&crate::logo_box(&logo, 64))
            .build()
            .upcast();
        logo_slot.set_valign(gtk::Align::Center);

        let name = label("tvjoe-osd-name");
        // Zweite Zeile unter dem Namen: Quellsymbol, dann Kategorie/Qualität („🌐 720p“).
        let origin = gtk::Image::builder()
            .css_classes(["tvjoe-osd-origin"])
            .pixel_size(14)
            .valign(gtk::Align::Center)
            .build();
        let meta = label("tvjoe-osd-sub");
        let meta_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        meta_row.append(&origin);
        meta_row.append(&meta);
        let name_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        name_box.set_valign(gtk::Align::Center);
        name_box.append(&name);
        name_box.append(&meta_row);
        let reception = label("tvjoe-osd-sub");
        reception.set_visible(false);
        name_box.append(&reception);
        let name_slot = adw::Clamp::builder()
            .orientation(gtk::Orientation::Horizontal)
            .maximum_size(NAME_WIDTH)
            .tightening_threshold(NAME_WIDTH)
            .width_request(NAME_WIDTH)
            .child(&name_box)
            .build();
        name_slot.set_valign(gtk::Align::Center);

        let now = label("tvjoe-osd-now");
        let desc = gtk::Label::builder()
            .css_classes(["tvjoe-osd-sub"])
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let next = label("tvjoe-osd-sub");
        // „Danach“ und die Steuerzeile haben in der CSS dieselbe feste Höhe (`tvjoe-osd-line`):
        // die Bauchbinde bleibt gleich groß, egal welche gezeigt wird (Nutzer, 2026-10-26).
        next.add_css_class("tvjoe-osd-line");
        let extra = label("tvjoe-osd-extra");
        extra.set_wrap(true);

        // Mediathek: Knöpfe, Fortschrittsleiste, Zeit (anstelle von „Danach“).
        let back_min = osd_button("media-skip-backward-symbolic", tr("1 Minute zurück (Bild ↑)"));
        let back = osd_button("media-seek-backward-symbolic", tr("10 Sekunden zurück (←)"));
        let pause = osd_button("media-playback-pause-symbolic", tr("Pause (Leertaste)"));
        let fwd = osd_button("media-seek-forward-symbolic", tr("10 Sekunden vor (→)"));
        let fwd_min = osd_button("media-skip-forward-symbolic", tr("1 Minute vor (Bild ↓)"));
        let audio = osd_button(
            "preferences-desktop-locale-symbolic",
            tr("Tonspur wechseln (A)"),
        );
        let info = osd_button("dialog-information-symbolic", tr("Info (I)"));
        let progress = gtk::ProgressBar::builder()
            .hexpand(true)
            .valign(gtk::Align::Center)
            .css_classes(["tvjoe-osd-progress"])
            .build();
        let time = gtk::Label::builder()
            .css_classes(["tvjoe-osd-sub", "tvjoe-osd-time"])
            .xalign(1.0)
            .build();
        let video_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        video_row.add_css_class("tvjoe-osd-line");
        for b in [&back_min, &back, &pause, &fwd, &fwd_min] {
            video_row.append(b);
        }
        progress.set_margin_start(8);
        progress.set_margin_end(8);
        video_row.append(&progress);
        video_row.append(&time);
        let live = gtk::Button::builder()
            .label(tr("Live"))
            .tooltip_text(tr("Zurück auf live (Ende)"))
            .css_classes(["flat", "tvjoe-osd-button"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        video_row.append(&live);
        video_row.append(&audio);
        video_row.append(&info);
        video_row.set_visible(false);

        let right = gtk::Box::new(gtk::Orientation::Vertical, 2);
        right.set_hexpand(true);
        right.set_valign(gtk::Align::Center);
        for w in [&now, &desc, &next] {
            right.append(w);
        }
        right.append(&video_row);
        right.append(&extra);

        let sep_number = vsep();
        let sep_logo = vsep();
        let sep_right = vsep();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(16)
            .css_classes(["tvjoe-osd"])
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::End)
            .margin_start(16)
            .margin_end(16)
            .margin_bottom(16)
            .visible(false)
            .can_target(false)
            .build();
        root.append(&number);
        root.append(&sep_number);
        root.append(&logo_slot);
        root.append(&sep_logo);
        root.append(&name_slot);
        root.append(&sep_right);
        root.append(&right);
        // Aufnahmeknopf wie an einer Kamera: weißes Feld „REC“; während der Aufnahme rot.
        let rec = gtk::Button::builder()
            .label(tr("REC"))
            .tooltip_text(tr("Aufnehmen bis Sendungsende (R)"))
            .css_classes(["tvjoe-rec-button"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        root.append(&rec);

        let osd = Rc::new(Self {
            blocked: Cell::new(false),
            root,
            number,
            origin,
            logo,
            logo_slot,
            sep_logo,
            sep_number,
            name,
            meta,
            reception,
            sep_right,
            right,
            now,
            desc,
            next,
            extra,
            video_row,
            progress: progress.clone(),
            time,
            pause: pause.clone(),
            seek_buttons: [back_min.clone(), back.clone(), fwd.clone(), fwd_min.clone()],
            live: live.clone(),
            rec: rec.clone(),
            on_action: RefCell::new(None),
            timer: RefCell::new(None),
        });
        let wire = |b: &gtk::Button, action: VideoAction| {
            let weak = Rc::downgrade(&osd);
            b.connect_clicked(move |_| {
                if let Some(osd) = weak.upgrade() {
                    osd.act(action);
                }
            });
        };
        wire(&back_min, VideoAction::Seek(-60.0));
        wire(&back, VideoAction::Seek(-10.0));
        wire(&pause, VideoAction::TogglePause);
        wire(&fwd, VideoAction::Seek(10.0));
        wire(&fwd_min, VideoAction::Seek(60.0));
        wire(&audio, VideoAction::Audio);
        wire(&info, VideoAction::Info);
        wire(&rec, VideoAction::Record);
        wire(&live, VideoAction::Live);
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(&osd);
        click.connect_pressed(move |g, _, x, _| {
            // Beansprucht: sonst schaltet ein Doppelklick in die Leiste auf Vollbild.
            g.set_state(gtk::EventSequenceState::Claimed);
            let Some(osd) = weak.upgrade() else { return };
            let width = g.widget().map_or(0, |w| w.width());
            if width > 0 {
                osd.act(VideoAction::SeekTo(x / f64::from(width)));
            }
        });
        progress.add_controller(click);
        osd
    }

    /// Empfänger für die Knöpfe der Video-Bauchbinde.
    pub fn set_action_handler(&self, f: impl Fn(VideoAction) + 'static) {
        *self.on_action.borrow_mut() = Some(Box::new(f));
    }

    fn act(&self, action: VideoAction) {
        if let Some(f) = self.on_action.borrow().as_ref() {
            f(action);
        }
    }

    pub fn is_shown(&self) -> bool {
        self.root.is_visible()
    }

    /// Nur die Fortschrittszeile auffrischen (ohne neue Anzeigedauer).
    pub fn update_video(&self, v: &VideoState) {
        let fraction = if v.duration > 0.0 {
            v.position / v.duration
        } else {
            0.0
        };
        self.progress.set_fraction(fraction.clamp(0.0, 1.0));
        match v.live {
            Some(behind) => {
                let d = behind.round() as u64;
                self.time.set_label(&format!("−{}:{:02}", d / 60, d % 60));
            }
            None => self
                .time
                .set_label(&format!("{} / {}", clock(v.position), clock(v.duration))),
        }
        self.live.set_visible(v.live.is_some());
        let tips = if v.live.is_some() {
            [
                tr("1 Minute zurück (Bild ↑)"),
                tr("10 Sekunden zurück"),
                tr("10 Sekunden vor"),
                tr("1 Minute vor (Bild ↓)"),
            ]
        } else {
            [
                tr("1 Minute zurück (Bild ↑)"),
                tr("10 Sekunden zurück (←)"),
                tr("10 Sekunden vor (→)"),
                tr("1 Minute vor (Bild ↓)"),
            ]
        };
        for (b, t) in self.seek_buttons.iter().zip(tips) {
            b.set_tooltip_text(Some(t));
        }
        let (icon, tip) = if v.paused {
            ("media-playback-start-symbolic", tr("Weiter (Leertaste)"))
        } else {
            ("media-playback-pause-symbolic", tr("Pause (Leertaste)"))
        };
        self.pause.set_icon_name(icon);
        self.pause.set_tooltip_text(Some(tip));
    }

    fn set_text(l: &gtk::Label, text: Option<&str>) {
        let text = text.filter(|t| !t.is_empty());
        l.set_label(text.unwrap_or(""));
        l.set_visible(text.is_some());
    }

    /// Empfangszeile nachführen, ohne die Anzeigedauer zu ändern.
    pub fn set_reception(&self, text: Option<&str>) {
        // Leerzeichen statt leer: die Zeile behält ihre Höhe.
        self.reception
            .set_label(text.filter(|t| !t.is_empty()).unwrap_or(" "));
        self.reception.set_visible(text.is_some());
    }

    /// Senderanzeige für `secs` Sekunden (0 = bis zur nächsten Anzeige).
    /// Bauchbinde sperren (Ebene offen) bzw. wieder zulassen.
    pub fn set_blocked(&self, on: bool) {
        self.blocked.set(on);
        if on {
            self.hide();
        }
    }

    pub fn show_channel(self: &Rc<Self>, info: &ChannelInfo, secs: u32) {
        if self.blocked.get() {
            return;
        }
        let has_number = !info.number.is_empty();
        self.number.set_label(&info.number);
        self.number.set_visible(has_number);
        match info.origin {
            Some(o) => {
                self.origin.set_icon_name(Some(match o {
                    Origin::Internet => "web-browser-symbolic",
                    Origin::Satellite => crate::icons::SATELLITE,
                    Origin::Card => crate::icons::ANTENNA,
                }));
                self.origin.set_tooltip_text(Some(match o {
                    Origin::Internet => tr("Internet"),
                    Origin::Satellite => tr("Satellit"),
                    Origin::Card => tr("DVB-Karte"),
                }));
                self.origin.set_visible(true);
            }
            None => self.origin.set_visible(false),
        }
        self.sep_number.set_visible(has_number);
        match &info.logo {
            Some(p) => {
                crate::logos::set_bright(&self.logo, p);
                self.logo_slot.set_visible(true);
                self.sep_logo.set_visible(true);
            }
            None => {
                self.logo_slot.set_visible(false);
                self.sep_logo.set_visible(false);
            }
        }
        self.name.set_label(&info.name);
        Self::set_text(&self.meta, Some(&info.meta));
        if info.full {
            // Leere Zeilen behalten ihre Höhe; die Beschreibung ist immer zwei Zeilen hoch.
            let fill = |l: &gtk::Label, t: Option<&str>| {
                l.set_label(t.filter(|t| !t.is_empty()).unwrap_or(" "));
                l.set_visible(true);
            };
            fill(&self.now, info.now.as_deref());
            fill(&self.desc, info.desc.as_deref());
            fill(&self.next, info.next.as_deref());
            let two_lines = self.desc.create_pango_layout(Some("Ag\nAg")).pixel_size().1;
            self.desc.set_size_request(-1, two_lines);
        } else {
            Self::set_text(&self.now, info.now.as_deref());
            Self::set_text(&self.desc, info.desc.as_deref());
            Self::set_text(&self.next, info.next.as_deref());
            self.desc.set_size_request(-1, -1);
        }
        Self::set_text(&self.extra, info.extra.as_deref());
        self.set_reception(info.reception.as_deref());
        // Mediathek: Fortschrittszeile statt „Danach“; nur dann nimmt die Anzeige Klicks an.
        match &info.video {
            Some(v) => {
                self.next.set_visible(false);
                self.video_row.set_visible(true);
                self.update_video(v);
                self.root.set_can_target(true);
            }
            None => {
                self.video_row.set_visible(false);
                self.root.set_can_target(false);
            }
        }
        // Laufender Sender: roter Aufnahmeknopf; dann nimmt die Anzeige Klicks an.
        match info.record {
            Some(on) => {
                self.rec.set_tooltip_text(Some(if on {
                    tr("Aufnahme beenden (R)")
                } else {
                    tr("Aufnehmen bis Sendungsende (R)")
                }));
                if on {
                    self.rec.add_css_class("tvjoe-rec-on");
                } else {
                    self.rec.remove_css_class("tvjoe-rec-on");
                }
                self.rec.set_visible(true);
                self.root.set_can_target(true);
            }
            None => self.rec.set_visible(false),
        }
        // Aus dem Inhalt bestimmen: `is_visible()` meldet bei noch ausgeblendeter Anzeige immer „nein“.
        let right_visible = info.full
            || [&info.now, &info.desc, &info.next, &info.extra]
                .iter()
                .any(|t| t.as_deref().is_some_and(|t| !t.is_empty()));
        self.right.set_visible(right_visible);
        self.sep_right.set_visible(right_visible);
        self.present(secs);
    }

    /// Kurze Meldung (Lautstärke, Ziffernwahl …): links Nummer und Titel, rechts Text.
    pub fn show(self: &Rc<Self>, number: &str, title: &str, text: &str, secs: u32) {
        let info = ChannelInfo {
            number: number.to_string(),
            name: title.to_string(),
            extra: (!text.is_empty()).then(|| text.to_string()),
            ..ChannelInfo::default()
        };
        self.show_channel(&info, secs);
    }

    pub fn hide(&self) {
        if let Some(id) = self.timer.take() {
            id.remove();
        }
        self.root.set_visible(false);
    }

    fn present(self: &Rc<Self>, secs: u32) {
        self.root.set_visible(true);
        if let Some(id) = self.timer.take() {
            id.remove();
        }
        if secs > 0 {
            let weak = Rc::downgrade(self);
            let id = glib::timeout_add_seconds_local_once(secs, move || {
                if let Some(osd) = weak.upgrade() {
                    osd.timer.take();
                    osd.root.set_visible(false);
                }
            });
            *self.timer.borrow_mut() = Some(id);
        }
    }
}

/// Meldungsstreifen oben im Bild: eine Zeile über die Bildbreite, gleiches Aussehen wie die
/// Meldungszeile bisher unten (Lautstärke, Ton, Qualität, Störungen, Ziffernwahl ohne Sender).
/// Mindestabstand des kleinen Meldungskastens (unten links) von oben: Kopfzeile der Ansicht
/// (etwa 55 px) und Filter- bzw. Senderleiste (etwa 55 px), falls das Fenster sehr niedrig ist.
const COMPACT_TOP: i32 = 124;

pub struct MessageBar {
    pub root: gtk::Box,
    text: gtk::Label,
    timer: RefCell<Option<glib::SourceId>>,
    /// Bleibende Meldung (fehlendes OpenGL): andere Meldungen erscheinen nur vorübergehend,
    /// danach steht wieder sie da.
    standing: RefCell<Option<String>>,
    /// Klein links oben (Programm, Mediathek offen): die Kopfzeile darunter bleibt bedienbar.
    compact: Cell<bool>,
}

impl MessageBar {
    pub fn new() -> Rc<Self> {
        let text = label("tvjoe-osd-name");
        // Lange Meldungen (z. B. fehlendes OpenGL) umbrechen statt kürzen, höchstens drei Zeilen.
        text.set_wrap(true);
        text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        text.set_lines(3);
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .css_classes(["tvjoe-osd"])
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Start)
            .margin_start(16)
            .margin_end(16)
            .margin_top(16)
            .visible(false)
            .can_target(false)
            .build();
        root.append(&text);
        Rc::new(Self {
            root,
            text,
            timer: RefCell::new(None),
            standing: RefCell::new(None),
            compact: Cell::new(false),
        })
    }

    /// Klein links oben mit Breite `width` (Ebene offen) bzw. wieder als Balken über die Breite.
    pub fn set_compact(&self, width: Option<i32>) {
        self.compact.set(width.is_some());
        match width {
            Some(w) => {
                self.root.set_halign(gtk::Align::Start);
                self.root.set_size_request(w, -1);
                // Unten links (Nutzer, 2026-10-28): oben liegen Kopf- und Filterzeilen, in der
                // Ablage Überschrift und Belegungsbalken; die Bauchbinde ist bei offener Ebene
                // gesperrt und kann dort nicht erscheinen.
                self.root.set_valign(gtk::Align::End);
                self.root.set_margin_top(COMPACT_TOP);
                self.root.set_margin_bottom(24);
                self.root.add_css_class("tvjoe-msg-compact");
                // Breite vorgeben: der Text nutzt die Kastenbreite (abzüglich des Innenabstands
                // von 2 × 22 px) und bricht dort um, statt den Kasten zu verbreitern.
                self.text.set_max_width_chars(1);
                self.text.set_size_request((w - 44).max(100), -1);
            }
            None => {
                self.root.set_halign(gtk::Align::Fill);
                self.root.set_valign(gtk::Align::Start);
                self.root.set_size_request(-1, -1);
                self.root.set_margin_top(16);
                self.root.set_margin_bottom(0);
                self.root.remove_css_class("tvjoe-msg-compact");
                self.text.set_max_width_chars(-1);
                self.text.set_size_request(-1, -1);
            }
        }
        let current = self.text.label().to_string();
        self.set_text(&current);
    }

    /// Bleibende Meldung setzen und zeigen.
    pub fn show_standing(self: &Rc<Self>, text: &str) {
        *self.standing.borrow_mut() = Some(text.to_string());
        self.show(text, 0);
    }

    /// Zeigt die Meldung für `secs` Sekunden (0 = bis zur nächsten Meldung); gibt es eine
    /// bleibende Meldung, steht danach wieder sie da.
    pub fn show(self: &Rc<Self>, text: &str, secs: u32) {
        let secs = if secs == 0 && self.standing.borrow().as_deref().is_some_and(|s| s != text) {
            5
        } else {
            secs
        };
        self.set_text(text);
        self.root.set_visible(true);
        if let Some(id) = self.timer.take() {
            id.remove();
        }
        if secs > 0 {
            let weak = Rc::downgrade(self);
            let id = glib::timeout_add_seconds_local_once(secs, move || {
                if let Some(bar) = weak.upgrade() {
                    bar.timer.take();
                    let standing = bar.standing.borrow().clone();
                    match standing {
                        Some(s) => bar.set_text(&s),
                        None => bar.root.set_visible(false),
                    }
                }
            });
            *self.timer.borrow_mut() = Some(id);
        }
    }

    fn set_text(&self, text: &str) {
        self.text.set_label(text);
        // Kurze Meldungen groß (Lautstärke, Ton …), lange und im kleinen Kasten in der Schrift
        // der Beschreibung.
        let long = self.compact.get() || text.chars().count() > 60;
        for (class, on) in [("tvjoe-osd-name", !long), ("tvjoe-osd-now", long)] {
            if on {
                self.text.add_css_class(class);
            } else {
                self.text.remove_css_class(class);
            }
        }
    }
}

/// Inhalt der ganzflächigen Info (Taste I, Knopf (i)).
#[derive(Debug, Default, Clone)]
pub struct InfoContent {
    pub logo: Option<PathBuf>,
    pub title: String,
    /// Zeilen unter dem Titel (Sender · Thema, Zeit · Dauer …).
    pub meta: Vec<String>,
    pub desc: String,
    /// Fußzeile, z. B. „Danach 21:45 Tagesthemen“.
    pub next: Option<String>,
    /// Fernsehen: REC-Knopf (`Some(true)` = Aufnahme läuft).
    pub record: Option<bool>,
}

/// Ganzflächige Info über dem Bild: Logo, Titel, Angaben, volle Beschreibung.
/// Ein Klick, I oder Esc schließt sie.
pub struct InfoPanel {
    pub root: gtk::Box,
    logo: gtk::Picture,
    logo_slot: gtk::Widget,
    title: gtk::Label,
    meta: gtk::Label,
    desc: gtk::Label,
    next: gtk::Label,
    rec: gtk::Button,
    on_record: RefCell<Option<Box<dyn Fn()>>>,
}

impl InfoPanel {
    pub fn new() -> Rc<Self> {
        let logo = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .height_request(72)
            .build();
        let logo_slot: gtk::Widget = adw::Clamp::builder()
            .orientation(gtk::Orientation::Vertical)
            .maximum_size(72)
            .tightening_threshold(72)
            .valign(gtk::Align::Start)
            .child(&crate::logo_box(&logo, 120))
            .build()
            .upcast();
        let wrap = |class: &str| {
            gtk::Label::builder()
                .css_classes([class])
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .build()
        };
        let title = wrap("tvjoe-info-title");
        let meta = wrap("tvjoe-info-meta");
        let head_text = gtk::Box::new(gtk::Orientation::Vertical, 6);
        head_text.set_hexpand(true);
        head_text.append(&title);
        head_text.append(&meta);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        head.append(&logo_slot);
        head.append(&head_text);
        let rec = gtk::Button::builder()
            .label(tr("REC"))
            .css_classes(["tvjoe-rec-button"])
            .valign(gtk::Align::Start)
            .visible(false)
            .build();
        head.append(&rec);

        let desc = wrap("tvjoe-info-desc");
        desc.set_valign(gtk::Align::Start);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&desc)
            .build();
        let next = wrap("tvjoe-info-meta");
        let hint = gtk::Label::builder()
            .label(tr("I, Esc oder Klick schließt"))
            .css_classes(["tvjoe-osd-sub"])
            .xalign(1.0)
            .build();

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(20)
            .css_classes(["tvjoe-info"])
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .visible(false)
            .build();
        root.append(&head);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&scroller);
        root.append(&next);
        root.append(&hint);

        let panel = Rc::new(Self {
            root,
            logo,
            logo_slot,
            title,
            meta,
            desc,
            next,
            rec,
            on_record: RefCell::new(None),
        });
        let weak = Rc::downgrade(&panel);
        panel.rec.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                if let Some(f) = p.on_record.borrow().as_ref() {
                    f();
                }
            }
        });
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(&panel);
        click.connect_pressed(move |g, _, x, y| {
            let Some(p) = weak.upgrade() else { return };
            // Klick auf den REC-Knopf gehört dem Knopf, sonst schließt ein Klick die Info.
            let on_rec = g
                .widget()
                .and_then(|w| w.pick(x, y, gtk::PickFlags::DEFAULT))
                .is_some_and(|w| w == p.rec.clone().upcast::<gtk::Widget>() || w.is_ancestor(&p.rec));
            if on_rec {
                return;
            }
            g.set_state(gtk::EventSequenceState::Claimed);
            p.hide();
        });
        panel.root.add_controller(click);
        panel
    }

    pub fn is_shown(&self) -> bool {
        self.root.is_visible()
    }

    pub fn show(&self, c: &InfoContent) {
        match &c.logo {
            Some(p) => {
                crate::logos::set_bright(&self.logo, p);
                self.logo_slot.set_visible(true);
            }
            None => self.logo_slot.set_visible(false),
        }
        self.title.set_label(&c.title);
        self.meta.set_label(&c.meta.join("\n"));
        self.meta.set_visible(!c.meta.is_empty());
        self.desc.set_label(if c.desc.is_empty() {
            tr("Keine Beschreibung vorhanden.")
        } else {
            &c.desc
        });
        self.next.set_label(c.next.as_deref().unwrap_or(""));
        self.next.set_visible(c.next.is_some());
        match c.record {
            Some(on) => {
                self.rec.set_tooltip_text(Some(if on {
                    tr("Aufnahme beenden (R)")
                } else {
                    tr("Aufnehmen bis Sendungsende (R)")
                }));
                if on {
                    self.rec.add_css_class("tvjoe-rec-on");
                } else {
                    self.rec.remove_css_class("tvjoe-rec-on");
                }
                self.rec.set_visible(true);
            }
            None => self.rec.set_visible(false),
        }
        self.root.set_visible(true);
    }

    pub fn hide(&self) {
        self.root.set_visible(false);
    }

    /// Empfänger für den REC-Knopf.
    pub fn set_record_handler(&self, f: impl Fn() + 'static) {
        *self.on_record.borrow_mut() = Some(Box::new(f));
    }
}
