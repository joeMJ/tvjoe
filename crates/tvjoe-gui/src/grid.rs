//! Programmraster: je Sender eine Zeile, die nächsten Stunden als Zeitleiste.
//! Laufende Sendungen sind hervorgehoben; ein Klick schaltet auf den Sender um.
//! Erscheint als Ebene im Hauptfenster oder, wenn der Platz nicht reicht, als eigenes Fenster.

use std::rc::{Rc, Weak};

use adw::prelude::*;
use tvjoe_core::epg;

use crate::app::{App, hm};

/// Pixel je Minute.
const PPM: f64 = 5.0;
const HOURS: i64 = 6;
const NAME_WIDTH: i32 = 210;
/// Mindestfläche (Namensspalte und rund zwei Stunden).
const MIN_SIZE: (i32, i32) = (850, 500);

struct Grid {
    title: String,
    body: gtk::Stack,
    /// Reiter Programm · Geplant · Ablage (Nutzer, 2026-10-26: kein weiterer Menüknopf).
    tabs: gtk::Box,
}

impl crate::layer::View for Grid {
    fn title(&self) -> String {
        self.title.clone()
    }
    fn min_size(&self) -> (i32, i32) {
        MIN_SIZE
    }
    fn body(&self) -> gtk::Widget {
        self.body.clone().upcast()
    }
    fn header_center(&self) -> Option<gtk::Widget> {
        Some(self.tabs.clone().upcast())
    }
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

/// Öffnet das Raster (jedes Mal neu, zur aktuellen Uhrzeit) und gibt das Fenster zurück, in
/// dem es erscheint (Selbstprobe).
pub fn open_window(app: &Rc<App>) -> Option<gtk::Window> {
    // Ohne Programmdaten öffnet die Ansicht trotzdem: „Geplant“ und „Ablage“ brauchen keins.
    let guide = app.guide();

    // Schwacher Verweis auf die Ansicht, gesetzt nach ihrem Aufbau.
    let me_slot: Rc<std::cell::OnceCell<Weak<Grid>>> = Rc::new(std::cell::OnceCell::new());
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
    // Ohne Programmdaten keine Zeitleiste, nur der Hinweis.
    if guide.is_some() {
        rows.append(&ticks);
    }

    let sat = app.is_sat();
    let lib = app.library();
    if let Some(guide) = guide.as_deref() {
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
            // Roter Punkt: Aufnahme geplant.
            let dot = gtk::Label::builder()
                .label("●")
                .css_classes(["tvjoe-rec"])
                .visible(crate::ablage::timer_for(ch.id, sat, p.start).is_some())
                .build();
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            content.append(&dot);
            content.append(&label);
            let button = gtk::Button::builder()
                .child(&content)
                .css_classes(["tvjoe-prog"])
                .build();
            // Rechtsklick: Aufnahme planen bzw. Planung löschen.
            {
                let weak = Rc::downgrade(app);
                let (cid, name, title, start, stop) = (
                    ch.id,
                    ch.display_name().to_string(),
                    p.title.clone(),
                    p.start,
                    p.stop,
                );
                let dot = dot.clone();
                crate::ablage::context_menu(&button, move || {
                    let mut out: Vec<(String, Box<dyn Fn()>)> = Vec::new();
                    let Some(app) = weak.upgrade() else { return out };
                    let dot = dot.clone();
                    match crate::ablage::timer_for(cid, sat, start) {
                        Some(t) => out.push((
                            "Planung löschen".into(),
                            Box::new(move || {
                                crate::ablage::unplan(&app, &t);
                                dot.set_visible(false);
                            }),
                        )),
                        None if stop > epg::now() => {
                            let t = tvjoe_core::ablage::Timer {
                                id: 0,
                                channel_id: cid,
                                sat,
                                channel: name.clone(),
                                title: title.clone(),
                                start,
                                stop,
                            };
                            out.push((
                                "Aufnahme planen".into(),
                                Box::new(move || {
                                    crate::ablage::plan(&app, &t);
                                    dot.set_visible(true);
                                }),
                            ));
                        }
                        None => {}
                    }
                    out
                });
            }
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
            let me = me_slot.clone();
            button.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.play_channel(id);
                    if let Some(view) = me.get().and_then(Weak::upgrade) {
                        let view: Rc<dyn crate::layer::View> = view;
                        crate::layer::close_if_layer(&app, &view);
                    }
                }
            });
            line.append(&fixed(&button, width));
            cursor = end;
        }
        rows.append(&line);
    }
    } else {
        rows.append(
            &gtk::Label::builder()
                .label("Kein Programm geladen. Bitte in „Sender verwalten“ eine EPG-Quelle (XMLTV) eintragen oder bei SAT-Sendern „Aus dem SAT-Empfang“ wählen. Ist eine eingetragen, wird sie gerade noch geladen.")
                .css_classes(["tvjoe-sub"])
                .wrap(true)
                .max_width_chars(70)
                .xalign(0.0)
                .halign(gtk::Align::Start)
                .margin_start(16)
                .margin_top(24)
                .build(),
        );
    }
    drop(lib);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&rows)
        .build();
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    stack.add_named(&scroller, Some("programm"));
    stack.add_named(&crate::ablageview::planned_page(app), Some("geplant"));
    stack.add_named(&crate::ablageview::overview_page(app), Some("ablage"));
    let tabs = gtk::Box::builder().css_classes(["linked"]).build();
    let mut first: Option<gtk::ToggleButton> = None;
    for (name, text) in [("programm", "Programm"), ("geplant", "Geplant"), ("ablage", "Ablage")] {
        let b = gtk::ToggleButton::builder()
            .label(text)
            .active(first.is_none())
            .build();
        match &first {
            Some(g) => b.set_group(Some(g)),
            None => first = Some(b.clone()),
        }
        let stack = stack.clone();
        b.connect_toggled(move |b| {
            if b.is_active() {
                stack.set_visible_child_name(name);
            }
        });
        tabs.append(&b);
    }
    let grid = Rc::new(Grid {
        title: format!("Programm {} bis {} Uhr", hm(from), hm(to)),
        body: stack,
        tabs,
    });
    // Die Knöpfe halten die Ansicht nur schwach (zum Schließen nach dem Umschalten).
    let _ = me_slot.set(Rc::downgrade(&grid));
    let view: Rc<dyn crate::layer::View> = grid;
    crate::layer::open(app, view.clone());
    crate::layer::window_of(app, &view)
}
