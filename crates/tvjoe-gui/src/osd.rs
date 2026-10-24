//! Einblendung über dem Bild: Kanalnummer, Name, Zusatzzeile; verschwindet nach einer Zeit.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

pub struct Osd {
    pub root: gtk::Box,
    number: gtk::Label,
    name: gtk::Label,
    sub: gtk::Label,
    timer: RefCell<Option<glib::SourceId>>,
}

impl Osd {
    pub fn new() -> Rc<Self> {
        let number = gtk::Label::builder()
            .css_classes(["tvjoe-osd-number"])
            .build();
        let name = gtk::Label::builder()
            .css_classes(["tvjoe-osd-name"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(40)
            .build();
        let sub = gtk::Label::builder()
            .css_classes(["tvjoe-osd-sub"])
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(60)
            .build();
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_valign(gtk::Align::Center);
        text.append(&name);
        text.append(&sub);
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(18)
            .css_classes(["tvjoe-osd"])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .margin_start(32)
            .margin_bottom(32)
            .margin_end(32)
            .visible(false)
            .can_target(false)
            .build();
        root.append(&number);
        root.append(&text);
        Rc::new(Self {
            root,
            number,
            name,
            sub,
            timer: RefCell::new(None),
        })
    }

    /// Zeigt die Einblendung für `secs` Sekunden (0 = bis zum nächsten Aufruf).
    pub fn show(self: &Rc<Self>, number: &str, name: &str, sub: &str, secs: u32) {
        self.number.set_label(number);
        self.number.set_visible(!number.is_empty());
        self.name.set_label(name);
        self.sub.set_label(sub);
        self.sub.set_visible(!sub.is_empty());
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
