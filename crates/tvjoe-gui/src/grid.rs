//! Programmraster: je Sender eine Zeile, die nächsten Stunden als Zeitleiste.
//! Laufende Sendungen sind hervorgehoben; ein Klick schaltet auf den Sender um.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::epg;

use crate::app::{App, hm};

/// Pixel je Minute.
const PPM: f64 = 5.0;
const HOURS: i64 = 6;
const NAME_WIDTH: i32 = 210;

thread_local! {
    static OPEN: RefCell<Option<glib::WeakRef<adw::Window>>> = const { RefCell::new(None) };
}

fn px(secs: i64) -> i32 {
    ((secs as f64 / 60.0) * PPM).round() as i32
}

/// Feste Breite (Text wird gekürzt statt die Zeile zu verschieben).
fn fixed(child: &impl IsA<gtk::Widget>, width: i32) -> adw::Clamp {
    adw::Clamp::builder()
        .orientation(gtk::Orientation::Horizontal)
        .maximum_size(width)
        .tightening_threshold(width)
        .width_request(width)
        .child(child)
        .build()
}

pub fn open(app: &Rc<App>) {
    open_window(app);
}

/// Öffnet das Raster und gibt das Fenster zurück (Selbstprobe).
pub fn open_window(app: &Rc<App>) -> Option<adw::Window> {
    if let Some(w) = OPEN.with(|o| o.borrow().as_ref().and_then(|w| w.upgrade())) {
        w.present();
        return Some(w);
    }
    let Some(guide) = app.guide() else {
        let d = adw::AlertDialog::new(
            Some("Kein Programm geladen"),
            Some(
                "Bitte in „Sender verwalten“ eine EPG-Quelle (XMLTV) eintragen. Ist eine eingetragen, wird sie gerade noch geladen.",
            ),
        );
        d.add_response("ok", "OK");
        d.present(Some(&app.window));
        return None;
    };

    let now = epg::now();
    let from = now - now.rem_euclid(1800);
    let to = from + HOURS * 3600;

    let rows = gtk::Box::new(gtk::Orientation::Vertical, 2);
    rows.set_margin_start(8);
    rows.set_margin_end(8);
    rows.set_margin_bottom(8);

    let ticks = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    ticks.append(&fixed(&gtk::Label::new(None), NAME_WIDTH));
    let mut t = from;
    while t < to {
        let l = gtk::Label::builder()
            .label(hm(t))
            .xalign(0.0)
            .css_classes(["tvjoe-name"])
            .build();
        ticks.append(&fixed(&l, px(1800)));
        t += 1800;
    }
    rows.append(&ticks);

    let lib = app.library();
    for ch in lib
        .channels
        .iter()
        .filter(|c| c.active && !c.missing && !c.streams.is_empty())
    {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let name = gtk::Label::builder()
            .label(format!(
                "{} {}",
                ch.number.map(|n| n.to_string()).unwrap_or_default(),
                ch.display_name()
            ))
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["tvjoe-name"])
            .build();
        line.append(&fixed(&name, NAME_WIDTH));

        let matched = guide.match_channel(ch).map(|(id, _)| id);
        let programmes = matched
            .as_deref()
            .map(|id| guide.range(id, from, to))
            .unwrap_or(&[]);
        if programmes.is_empty() {
            line.append(
                &gtk::Label::builder()
                    .label(if matched.is_some() {
                        "keine Sendungen im Zeitraum"
                    } else {
                        "kein EPG zugeordnet"
                    })
                    .css_classes(["tvjoe-sub"])
                    .build(),
            );
        }
        let mut cursor = from;
        for p in programmes {
            let start = p.start.max(from);
            let end = p.stop.min(to);
            if start > cursor {
                line.append(&fixed(&gtk::Label::new(None), px(start - cursor)));
            }
            let width = px(end - start).max(4);
            let label = gtk::Label::builder()
                .label(&p.title)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let button = gtk::Button::builder()
                .child(&label)
                .css_classes(["tvjoe-prog"])
                .build();
            if p.start <= now && now < p.stop {
                button.add_css_class("tvjoe-prog-now");
            }
            let mut tip = format!("{}–{}  {}", hm(p.start), hm(p.stop), p.title);
            if let Some(s) = &p.subtitle {
                tip.push_str(&format!("\n{s}"));
            }
            if let Some(d) = &p.desc {
                let short: String = d.chars().take(400).collect();
                tip.push_str(&format!("\n\n{short}"));
            }
            button.set_tooltip_text(Some(&tip));
            let id = ch.id;
            let weak = Rc::downgrade(app);
            button.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.play_channel(id);
                }
            });
            line.append(&fixed(&button, width));
            cursor = end;
        }
        rows.append(&line);
    }
    drop(lib);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&rows)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(
        &adw::HeaderBar::builder()
            .title_widget(&adw::WindowTitle::new(
                "Programm",
                &format!("{} bis {} Uhr", hm(from), hm(to)),
            ))
            .build(),
    );
    view.set_content(Some(&scroller));
    let win = adw::Window::builder()
        .title("Programm – tvjoe")
        .default_width(1400)
        .default_height(820)
        .transient_for(&app.window)
        .content(&view)
        .build();
    OPEN.with(|o| *o.borrow_mut() = Some(win.downgrade()));
    // Beim Schließen vergessen: das nächste Öffnen baut das Raster neu (aktuelle Uhrzeit)
    // und holt kein zerstörtes Fenster zurück.
    win.connect_close_request(|_| {
        OPEN.with(|o| o.borrow_mut().take());
        glib::Propagation::Proceed
    });
    win.present();
    Some(win)
}
