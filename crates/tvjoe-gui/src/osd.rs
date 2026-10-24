//! Senderanzeige über dem Bild, unten über die ganze Bildbreite (abgestimmtes Mockup):
//! links Nummer | Logo in Schwarzweiß | Name, darunter Quellsymbol, Kategorie und Qualität;
//! rechts „Jetzt“ mit gekürzter Beschreibung und „Danach“. Alle Teile sind einzeln abschaltbar.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

/// Breite der Namensspalte (lange Namen werden mit „…“ gekürzt).
const NAME_WIDTH: i32 = 260;

/// Herkunft des Streams (Symbol vor der Qualität).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Internet,
    Satellite,
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
    pub now: Option<String>,
    pub desc: Option<String>,
    pub next: Option<String>,
    /// Zusätzliche Meldung, z. B. „Stream gestört …“ oder Videodaten.
    pub extra: Option<String>,
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
    sep_right: gtk::Separator,
    right: gtk::Box,
    now: gtk::Label,
    desc: gtk::Label,
    next: gtk::Label,
    extra: gtk::Label,
    timer: RefCell<Option<glib::SourceId>>,
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
            .css_classes(["tvjoe-osd-logo"])
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
        let extra = label("tvjoe-osd-extra");
        extra.set_wrap(true);
        let right = gtk::Box::new(gtk::Orientation::Vertical, 2);
        right.set_hexpand(true);
        right.set_valign(gtk::Align::Center);
        for w in [&now, &desc, &next, &extra] {
            right.append(w);
        }

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

        Rc::new(Self {
            root,
            number,
            origin,
            logo,
            logo_slot,
            sep_logo,
            sep_number,
            name,
            meta,
            sep_right,
            right,
            now,
            desc,
            next,
            extra,
            timer: RefCell::new(None),
        })
    }

    fn set_text(l: &gtk::Label, text: Option<&str>) {
        let text = text.filter(|t| !t.is_empty());
        l.set_label(text.unwrap_or(""));
        l.set_visible(text.is_some());
    }

    /// Senderanzeige für `secs` Sekunden (0 = bis zur nächsten Anzeige).
    pub fn show_channel(self: &Rc<Self>, info: &ChannelInfo, secs: u32) {
        let has_number = !info.number.is_empty();
        self.number.set_label(&info.number);
        self.number.set_visible(has_number);
        match info.origin {
            Some(o) => {
                self.origin.set_icon_name(Some(match o {
                    Origin::Internet => "web-browser-symbolic",
                    Origin::Satellite => "network-transmit-receive-symbolic",
                }));
                self.origin.set_tooltip_text(Some(match o {
                    Origin::Internet => "Internet",
                    Origin::Satellite => "Satellit",
                }));
                self.origin.set_visible(true);
            }
            None => self.origin.set_visible(false),
        }
        self.sep_number.set_visible(has_number);
        match &info.logo {
            Some(p) => {
                self.logo.set_filename(Some(p));
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
