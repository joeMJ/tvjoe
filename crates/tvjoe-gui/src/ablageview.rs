//! Reiter „Geplant“ und „Ablage“ im Programm: geplante und laufende Aufnahmen; alles auf der
//! Platte mit Belegung, Fortschritt beim Laden, Abspielen, Fortsetzen, Abbrechen und Löschen.
//! Beide bauen sich neu auf, solange sie sichtbar sind (Fortschritt, Ende einer Aufnahme).

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::ablage::{Art, Entry, State};

use crate::app::{App, hm};

fn label(text: &str, class: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes([class])
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build()
}

/// „1,2 GB“, „350 MB“.
pub fn size_text(bytes: u64) -> String {
    let gb = bytes as f64 / f64::from(1u32 << 30);
    if gb >= 1.0 {
        format!("{gb:.1} GB").replace('.', ",")
    } else {
        format!("{} MB", bytes >> 20)
    }
}

fn day(ts: i64) -> String {
    glib::DateTime::from_unix_local(ts)
        .ok()
        .and_then(|d| d.format("%a %d.%m. %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn small_button(text: &str) -> gtk::Button {
    gtk::Button::builder()
        .label(text)
        .valign(gtk::Align::Center)
        .build()
}

/// Seite, die `fill` neu aufbaut, solange sie sichtbar ist und sich `signature` ändert
/// (jede Sekunde geprüft).
fn live_page(
    signature: impl Fn() -> String + 'static,
    fill: impl Fn(&gtk::Box) + 'static,
) -> gtk::Widget {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_start(24)
        .margin_end(24)
        .margin_top(16)
        .margin_bottom(24)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&adw::Clamp::builder().maximum_size(1100).child(&content).build())
        .build();
    let fill = Rc::new(fill);
    let last = Rc::new(std::cell::RefCell::new(String::new()));
    let rebuild = {
        let (content, fill, last) = (content.clone(), fill.clone(), last.clone());
        move |force: bool| {
            let sig = signature();
            if !force && *last.borrow() == sig {
                return;
            }
            *last.borrow_mut() = sig;
            while let Some(c) = content.first_child() {
                content.remove(&c);
            }
            fill(&content);
        }
    };
    rebuild(true);
    let rebuild = Rc::new(rebuild);
    {
        let rebuild = rebuild.clone();
        scroller.connect_map(move |s| {
            rebuild(true);
            let (s, rebuild) = (s.downgrade(), rebuild.clone());
            glib::timeout_add_local(Duration::from_secs(1), move || match s.upgrade() {
                Some(s) if s.is_mapped() => {
                    rebuild(false);
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            });
        });
    }
    scroller.upcast()
}

fn group(title: &str, description: Option<&str>) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::builder().title(title).build();
    if let Some(d) = description {
        g.set_description(Some(d));
    }
    g
}

// ---------------------------------------------------------------------------------------------
// Geplant

/// Reiter „Geplant“: laufende Aufnahmen und geplante.
pub fn planned_page(app: &Rc<App>) -> gtk::Widget {
    let weak = Rc::downgrade(app);
    let signature = || {
        let mut s = format!("{:?}", crate::ablage::running_recordings());
        for t in crate::ablage::timers() {
            s.push_str(&format!("|{}:{}", t.id, t.stop > tvjoe_core::ablage::now()));
        }
        // Größe der laufenden Aufnahmen alle 10 Sekunden nachführen.
        s.push_str(&format!("|{}", tvjoe_core::ablage::now() / 10));
        s
    };
    live_page(signature, move |content| {
        let Some(app) = weak.upgrade() else { return };
        let running = crate::ablage::running_recordings();
        let entries = crate::ablage::all_entries();
        let now = tvjoe_core::ablage::now();
        if !running.is_empty() {
            let g = group("Läuft", None);
            for (id, stop_at) in running {
                let Some(e) = entries.iter().find(|e| e.id == id) else {
                    continue;
                };
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&e.title))
                    .subtitle(glib::markup_escape_text(&format!(
                        "{} · bis {} · {}",
                        e.subtitle,
                        hm(stop_at),
                        size_text(std::fs::metadata(crate::capture::part_path(&e.path))
                            .or_else(|_| std::fs::metadata(tvjoe_core::ablage::part_path(&e.path)))
                            .map(|m| m.len())
                            .unwrap_or(0))
                    )))
                    .build();
                let dot = label("●", "tvjoe-rec");
                row.add_prefix(&dot);
                let stop = small_button("Beenden");
                stop.connect_clicked(move |_| crate::ablage::stop_recording(id));
                row.add_suffix(&stop);
                g.add(&row);
            }
            content.append(&g);
        }
        let timers: Vec<_> = crate::ablage::timers()
            .into_iter()
            .filter(|t| t.stop > now)
            .collect();
        let g = group(
            "Geplant",
            Some("Im Programm per Rechtsklick auf eine Sendung planen. Aufgenommen wird nur, solange tvjoe läuft."),
        );
        if timers.is_empty() {
            g.add(
                &adw::ActionRow::builder()
                    .title("Keine Aufnahmen geplant")
                    .build(),
            );
        }
        for t in timers {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&t.title))
                .subtitle(glib::markup_escape_text(&format!(
                    "{} · {} – {}",
                    t.channel,
                    day(t.start),
                    hm(t.stop)
                )))
                .build();
            let remove = small_button("Planung löschen");
            let (app, t) = (app.clone(), t.clone());
            remove.connect_clicked(move |_| crate::ablage::unplan(&app, &t));
            row.add_suffix(&remove);
            g.add(&row);
        }
        content.append(&g);
    })
}

// ---------------------------------------------------------------------------------------------
// Ablage

/// Gruppe eines Eintrags in der Übersicht.
fn section(e: &Entry) -> usize {
    match (e.art, e.source.as_str()) {
        (Art::Cache, _) => 3,
        (Art::Recording, _) => 2,
        (_, "jellyfin") => 1,
        _ => 0,
    }
}

const SECTIONS: [&str; 4] = ["ÖR-Mediathek", "Jellyfin", "Aufnahmen", "Zwischenspeicher"];

/// Serie eines Jellyfin-Titels „Serie S01E02 Name“ (sonst `None`).
fn series_of(title: &str) -> Option<&str> {
    let b = title.as_bytes();
    (0..b.len().saturating_sub(4)).find_map(|i| {
        (b[i] == b' '
            && b[i + 1] == b'S'
            && b[i + 2].is_ascii_digit()
            && b[i + 3].is_ascii_digit())
        .then(|| &title[..i])
    })
}

/// Reiter „Ablage“: Belegung, dann je Gruppe die Einträge (Jellyfin-Folgen nach Serien).
pub fn overview_page(app: &Rc<App>) -> gtk::Widget {
    let weak = Rc::downgrade(app);
    // Aufgeklappte Serien bleiben beim Neuaufbau aufgeklappt.
    let expanded: Rc<std::cell::RefCell<std::collections::HashSet<String>>> = Rc::default();
    let signature = || {
        crate::ablage::all_entries()
            .iter()
            .map(|e| {
                let run = crate::ablage::running_fraction(e.id)
                    .map(|f| format!("{:.0}", f.unwrap_or(0.0) * 100.0));
                format!("{}:{:?}:{:?}:{}", e.id, e.state, run, e.size >> 20)
            })
            .collect::<Vec<_>>()
            .join("|")
    };
    live_page(signature, move |content| {
        let Some(app) = weak.upgrade() else { return };
        let entries: Vec<Entry> = crate::ablage::all_entries()
            .into_iter()
            .filter(|e| e.state != State::Failed)
            .collect();
        let size = |e: &Entry| {
            std::fs::metadata(&e.path)
                .or_else(|_| std::fs::metadata(tvjoe_core::ablage::part_path(&e.path)))
                .or_else(|_| std::fs::metadata(crate::capture::part_path(&e.path)))
                .map(|m| m.len())
                .unwrap_or(e.size)
        };
        let mut totals = [0u64; 4];
        for e in &entries {
            totals[section(e)] += size(e);
        }
        let sum: u64 = totals.iter().sum();
        let parts: Vec<String> = SECTIONS
            .iter()
            .zip(totals)
            .map(|(n, t)| format!("{n} {}", size_text(t)))
            .collect();
        let free = crate::ablage::free_space(&app)
            .map(|f| format!(" · {} frei", size_text(f)))
            .unwrap_or_default();
        content.append(&label(
            &format!("Belegt {}{free}", size_text(sum)),
            "tvjoe-mt-detail-title",
        ));
        content.append(&label(&parts.join(" · "), "tvjoe-sub"));
        content.append(&label(
            &crate::ablage::storage_dir(&app).display().to_string(),
            "tvjoe-sub",
        ));

        for (i, name) in SECTIONS.iter().enumerate() {
            let list: Vec<&Entry> = entries.iter().filter(|e| section(e) == i).collect();
            if list.is_empty() {
                continue;
            }
            let g = group(name, None);
            if i == 1 {
                // Jellyfin: Folgen einer Serie zusammen, mit Summe.
                let mut done: Vec<&str> = Vec::new();
                for e in &list {
                    match series_of(&e.title) {
                        Some(s) if !done.contains(&s) => {
                            done.push(s);
                            let eps: Vec<&&Entry> =
                                list.iter().filter(|x| series_of(&x.title) == Some(s)).collect();
                            let total: u64 = eps.iter().map(|x| size(x)).sum();
                            let exp = adw::ExpanderRow::builder()
                                .expanded(expanded.borrow().contains(s))
                                .title(glib::markup_escape_text(s))
                                .subtitle(format!(
                                    "Serie · {} {} · {}",
                                    eps.len(),
                                    if eps.len() == 1 { "Folge" } else { "Folgen" },
                                    size_text(total)
                                ))
                                .build();
                            for x in eps {
                                exp.add_row(&entry_row(&app, x, size(x)));
                            }
                            let (set, name) = (expanded.clone(), s.to_string());
                            exp.connect_expanded_notify(move |x| {
                                if x.is_expanded() {
                                    set.borrow_mut().insert(name.clone());
                                } else {
                                    set.borrow_mut().remove(&name);
                                }
                            });
                            g.add(&exp);
                        }
                        Some(_) => {}
                        None => g.add(&entry_row(&app, e, size(e))),
                    }
                }
            } else {
                for e in list {
                    g.add(&entry_row(&app, e, size(e)));
                }
            }
            content.append(&g);
        }
        if entries.is_empty() {
            content.append(&label(
                "Noch nichts in der Ablage. Rechtsklick auf eine Kachel in der Mediathek lädt herunter, Taste R nimmt die laufende Sendung auf.",
                "tvjoe-sub",
            ));
        }
    })
}

/// Zeile eines Eintrags: Titel, Zustand · Größe · Datum, Fortschritt, Knöpfe.
fn entry_row(app: &Rc<App>, e: &Entry, size: u64) -> gtk::Widget {
    let running = crate::ablage::running_fraction(e.id);
    let recording = crate::ablage::running_recordings()
        .iter()
        .any(|(id, _)| *id == e.id);
    let state = if recording {
        "Aufnahme läuft".to_string()
    } else if running.is_some() {
        "Wird geladen".to_string()
    } else {
        match e.state {
            State::Done => "Vollständig".into(),
            State::Partial => "Teilweise".into(),
            State::Queued => "Wartet".into(),
            State::Loading => "Wird geladen".into(),
            State::Failed => "Fehlgeschlagen".into(),
        }
    };
    let mut sub = vec![state, size_text(size), day(e.created)];
    if !e.subtitle.is_empty() && e.source != "jellyfin" {
        sub.insert(0, e.subtitle.clone());
    }
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&e.title))
        .subtitle(glib::markup_escape_text(&sub.join(" · ")))
        .build();
    if let Some(f) = running {
        let bar = gtk::ProgressBar::builder()
            .fraction(f.unwrap_or(0.0))
            .width_request(160)
            .valign(gtk::Align::Center)
            .show_text(true)
            .text(match f {
                Some(f) => format!("{:.0} %", f * 100.0),
                None => "lädt".to_string(),
            })
            .build();
        if f.is_none() {
            bar.pulse();
        }
        row.add_suffix(&bar);
    }
    let id = e.id;
    if e.state == State::Done && !recording {
        let play = small_button("Abspielen");
        let app = app.clone();
        play.connect_clicked(move |_| {
            crate::ablage::play_entry(&app, id);
            crate::layer::close_layer(&app);
        });
        row.add_suffix(&play);
    }
    if e.state == State::Partial && running.is_none() && e.art != Art::Recording {
        let resume = small_button("Fortsetzen");
        let app = app.clone();
        resume.connect_clicked(move |_| crate::ablage::resume_entry(&app, id));
        row.add_suffix(&resume);
    }
    if running.is_some() {
        let cancel = small_button("Abbrechen");
        let (s, k) = (e.source.clone(), e.key.clone());
        cancel.connect_clicked(move |_| crate::ablage::cancel(&s, &k));
        row.add_suffix(&cancel);
    }
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Löschen")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let title = e.title.clone();
    delete.connect_clicked(move |b| {
        let dialog = adw::AlertDialog::builder()
            .heading("Wirklich löschen?")
            .body(format!(
                "„{title}“ wird von der Platte gelöscht. Das lässt sich nicht rückgängig machen."
            ))
            .build();
        dialog.add_responses(&[("keep", "Behalten"), ("delete", "Löschen")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("keep"));
        dialog.connect_response(None, move |_, r| {
            if r == "delete" {
                crate::ablage::delete_entry(id);
            }
        });
        dialog.present(Some(b));
    });
    row.add_suffix(&delete);
    row.upcast()
}
