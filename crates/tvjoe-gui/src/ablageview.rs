//! Reiter „Geplant“ und „Ablage“ im Programm: geplante und laufende Aufnahmen; alles auf der
//! Platte mit Belegung, Fortschritt beim Laden, Abspielen, Fortsetzen, Abbrechen und Löschen.
//! Beide bauen sich neu auf, solange sie sichtbar sind (Fortschritt, Ende einer Aufnahme).

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::ablage::{Art, Entry, State};
use tvjoe_core::{tf, tr, trn};

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
        let size = format!("{gb:.1}").replace('.', ",");
        tf!("{size} GB", size = size)
    } else {
        tf!("{size} MB", size = bytes >> 20)
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
        // Aufnahmedienst: Stand oben; laufende Aufnahmen stehen dann im Verzeichnis.
        if app.settings().record_service {
            let text = if cfg!(windows) {
                tr("Aufnahmen laufen über die Aufgabenplanung von Windows, auch ohne geöffnetes tvjoe (solange Sie angemeldet sind).")
            } else if crate::dienst::alive() {
                tr("Aufnahmedienst läuft: Aufnahmen auch ohne geöffnetes tvjoe.")
            } else {
                tr("Aufnahmedienst antwortet nicht – geplante Aufnahmen finden nicht statt. Einstellungen → Ablage: Schalter aus- und wieder einschalten.")
            };
            content.append(&label(text, "tvjoe-sub"));
            let entries = crate::ablage::all_entries();
            let running: Vec<_> = entries
                .iter()
                .filter(|e| e.art == Art::Recording && e.state == State::Loading)
                .collect();
            if !running.is_empty() {
                let g = group(tr("Läuft"), None);
                for e in running {
                    let row = adw::ActionRow::builder()
                        .title(glib::markup_escape_text(&e.title))
                        .subtitle(glib::markup_escape_text(&tf!(
                            "{subtitle} · seit {time}",
                            subtitle = e.subtitle,
                            time = hm(e.created)
                        )))
                        .build();
                    row.add_prefix(&label("●", "tvjoe-rec"));
                    g.add(&row);
                }
                content.append(&g);
            }
        }
        let running = crate::ablage::running_recordings();
        let entries = crate::ablage::all_entries();
        let now = tvjoe_core::ablage::now();
        if !running.is_empty() {
            let g = group(tr("Läuft"), None);
            for (id, stop_at) in running {
                let Some(e) = entries.iter().find(|e| e.id == id) else {
                    continue;
                };
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&e.title))
                    .subtitle(glib::markup_escape_text(&tf!(
                        "{subtitle} · bis {time} · {size}",
                        subtitle = e.subtitle,
                        time = hm(stop_at),
                        size = size_text(std::fs::metadata(crate::capture::part_path(&e.path))
                            .or_else(|_| std::fs::metadata(tvjoe_core::ablage::part_path(&e.path)))
                            .map(|m| m.len())
                            .unwrap_or(0))
                    )))
                    .build();
                let dot = label("●", "tvjoe-rec");
                row.add_prefix(&dot);
                let stop = small_button(tr("Beenden"));
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
            tr("Geplant"),
            Some(tr("Im Programm per Rechtsklick auf eine Sendung planen. Aufgenommen wird nur, solange tvjoe läuft.")),
        );
        if timers.is_empty() {
            g.add(
                &adw::ActionRow::builder()
                    .title(tr("Keine Aufnahmen geplant"))
                    .build(),
            );
        }
        for t in timers {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&t.title))
                .subtitle(glib::markup_escape_text(&tf!(
                    "{channel} · {day} – {end}",
                    channel = t.channel,
                    day = day(t.start),
                    end = hm(t.stop)
                )))
                .build();
            let remove = small_button(tr("Planung löschen"));
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

/// Abschnitt eines Eintrags in der Übersicht: `oer`, die Bibliothek eines Servers (Jellyfin,
/// Plex …), `~rec` (Aufnahmen) oder `~cache` (Zwischenspeicher).
fn section(e: &Entry) -> String {
    match (e.art, e.source.as_str()) {
        (Art::Cache, _) => "~cache".into(),
        (Art::Recording, _) => "~rec".into(),
        (_, s) if crate::ablage::is_server_source(s) => s.to_string(),
        _ => "oer".into(),
    }
}

/// Abschnitte in fester Reihenfolge: ÖR, die Server in der Reihenfolge der Einstellungen
/// (dann unbekannte), Aufnahmen, Zwischenspeicher.
fn section_order(app: &App, entries: &[Entry]) -> Vec<String> {
    let mut order = vec!["oer".to_string()];
    for l in &app.settings().libraries {
        if crate::ablage::is_server_source(&l.id) {
            order.push(l.id.clone());
        }
    }
    for e in entries {
        let k = section(e);
        if !order.contains(&k) && !k.starts_with('~') {
            order.push(k);
        }
    }
    order.push("~rec".into());
    order.push("~cache".into());
    order
}

/// Überschrift eines Abschnitts (Server: Name der Mediathek wie in der Kopfleiste).
fn section_name(app: &App, key: &str) -> String {
    match key {
        "oer" => tr("ÖR-Mediathek").to_string(),
        "~rec" => tr("Aufnahmen").to_string(),
        "~cache" => tr("Zwischenspeicher").to_string(),
        id => match app.settings().library(id) {
            Some(l) if !l.title.is_empty() => l.title.clone(),
            Some(l) => crate::jellyfin::kind_name(&l.kind).into(),
            None => "Jellyfin".into(),
        },
    }
}

/// Folge einer Serie (sonst Film)? Server: Art im Zwischenspeicher, sonst „S01E02“ im Titel.
fn is_episode(e: &Entry) -> bool {
    crate::jfview::item(&e.source, &e.key)
        .map(|it| it.kind == "Episode")
        .unwrap_or_else(|| series_of(&e.title).is_some())
}

/// Belegungsbalken: Anteile nebeneinander (Breite nach Größe), darunter die Legende.
fn usage_bar(parts: &[(String, (f64, f64, f64), u64)]) -> gtk::Box {
    let total: u64 = parts.iter().map(|p| p.2).sum();
    let bars: Vec<((f64, f64, f64), f64)> = parts
        .iter()
        .filter(|p| p.2 > 0)
        .map(|p| (p.1, p.2 as f64 / total.max(1) as f64))
        .collect();
    let area = gtk::DrawingArea::builder().content_height(18).hexpand(true).build();
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        // Abgerundeter Rahmen, darin die Anteile.
        let r = 5.0;
        cr.new_sub_path();
        cr.arc(w - r, r, r, -90f64.to_radians(), 0.0);
        cr.arc(w - r, h - r, r, 0.0, 90f64.to_radians());
        cr.arc(r, h - r, r, 90f64.to_radians(), 180f64.to_radians());
        cr.arc(r, r, r, 180f64.to_radians(), 270f64.to_radians());
        cr.close_path();
        cr.clip();
        cr.set_source_rgba(0.5, 0.5, 0.5, 0.25);
        let _ = cr.paint();
        let mut x = 0.0;
        for ((red, green, blue), f) in &bars {
            let width = (w * f).max(2.0);
            cr.set_source_rgb(*red, *green, *blue);
            cr.rectangle(x, 0.0, width, h);
            let _ = cr.fill();
            x += width;
        }
    });
    let legend = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(14)
        .row_spacing(2)
        .max_children_per_line(4)
        .build();
    for (name, (red, green, blue), bytes) in parts {
        let item = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let swatch = gtk::DrawingArea::builder().content_width(10).content_height(10).valign(gtk::Align::Center).build();
        let (red, green, blue) = (*red, *green, *blue);
        swatch.set_draw_func(move |_, cr, w, h| {
            cr.set_source_rgb(red, green, blue);
            cr.rectangle(0.0, 0.0, w as f64, h as f64);
            let _ = cr.fill();
        });
        item.append(&swatch);
        item.append(&label(&format!("{name} {}", size_text(*bytes)), "tvjoe-sub"));
        legend.insert(&item, -1);
    }
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    b.set_margin_top(10);
    b.set_margin_bottom(6);
    b.append(&area);
    b.append(&legend);
    b
}

/// Farbe je Anteil: Serien blau, Filme grün (je Server heller), ÖR türkis, Aufnahmen bernstein,
/// Zwischenspeicher grau.
fn hex(c: u32) -> (f64, f64, f64) {
    (((c >> 16) & 255) as f64 / 255.0, ((c >> 8) & 255) as f64 / 255.0, (c & 255) as f64 / 255.0)
}
const SERIES_COLORS: [u32; 3] = [0x185FA5, 0x378ADD, 0x85B7EB];
const FILM_COLORS: [u32; 3] = [0x0F6E56, 0x1D9E75, 0x5DCAA5];

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

/// Gruppe der Ablage: Serie bzw. Sendung und Staffel; bei Jellyfin die Serie für den Abgleich.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupKey {
    /// Herkunft (Bibliothek) der Einträge.
    library: String,
    title: String,
    season: Option<i64>,
    series_id: Option<String>,
}

/// Staffel aus „… S01E02 …“.
fn season_of_title(title: &str) -> Option<i64> {
    let b = title.as_bytes();
    (0..b.len().saturating_sub(5)).find_map(|i| {
        (b[i] == b'S' && b[i + 1].is_ascii_digit() && b[i + 2].is_ascii_digit() && b[i + 3] == b'E')
            .then(|| title[i + 1..i + 3].parse().ok())
            .flatten()
    })
}

fn group_key(e: &Entry) -> Option<GroupKey> {
    match e.source.as_str() {
        s if crate::ablage::is_server_source(s) => {
            if let Some(it) = crate::jfview::item(s, &e.key).filter(|it| it.kind == "Episode") {
                if let Some(series) = it.series_name.clone() {
                    return Some(GroupKey {
                        library: s.to_string(),
                        title: series,
                        season: it.season_no,
                        series_id: it.series_id.clone(),
                    });
                }
            }
            series_of(&e.title).map(|t| GroupKey {
                library: s.to_string(),
                title: t.to_string(),
                season: season_of_title(&e.title),
                series_id: None,
            })
        }
        "oer" if e.art == Art::Download => {
            let topic = e.subtitle.trim();
            (!topic.is_empty()).then(|| GroupKey {
                library: e.source.clone(),
                title: topic.to_string(),
                season: tvjoe_core::mediathek::season_episode(&e.title)
                    .0
                    .map(i64::from),
                series_id: None,
            })
        }
        _ => None,
    }
}

/// Reihenfolge in einer Gruppe: Staffel, Folge, Titel.
fn episode_order(e: &Entry) -> (u32, u32, String) {
    let (se, ep) = match crate::jfview::item(&e.source, &e.key) {
        Some(it) if crate::ablage::is_server_source(&e.source) => (
            it.season_no.map(|n| n as u32),
            it.episode_no.map(|n| n as u32),
        ),
        _ => tvjoe_core::mediathek::season_episode(&e.title),
    };
    (se.unwrap_or(u32::MAX), ep.unwrap_or(u32::MAX), e.title.clone())
}

/// Anteil eines Eintrags am Gruppenfortschritt (fertig 1, sonst Geladenes).
fn entry_fraction(e: &Entry) -> f64 {
    match crate::ablage::running_fraction(e.id) {
        Some(Some(f)) => f,
        Some(None) => 0.0,
        None if e.state == State::Done => 1.0,
        None if e.total > 0 => (e.size as f64 / e.total as f64).min(1.0),
        None => 0.0,
    }
}

/// Balken einer Gruppe: Titel, Folgen · Größe (· Stand in Jellyfin), Fortschritt beim
/// Laden, Nachladen und Abo (Server), Löschen; aufgeklappt die Folgen.
fn group_row(
    app: &Rc<App>,
    key: &GroupKey,
    members: &[&Entry],
    size: &dyn Fn(&Entry) -> u64,
    expanded: &Rc<std::cell::RefCell<std::collections::HashSet<String>>>,
) -> gtk::Widget {
    let title = match key.season {
        Some(0) => tf!("{title} · Extras", title = key.title),
        Some(n) => tf!("{title} · Staffel {nr}", title = key.title, nr = n),
        None => key.title.clone(),
    };
    let total: u64 = members.iter().map(|x| size(x)).sum();
    let in_library = key
        .series_id
        .as_deref()
        .map(|id| crate::jfview::season_count(&key.library, id, key.season));
    let mut sub = vec![
        trn("{n} Folge", "{n} Folgen", members.len() as u64),
        size_text(total),
    ];
    if let Some(n) = in_library.filter(|n| *n > 0) {
        sub.push(tf!(
            "{have} von {all} Folgen der Staffel",
            have = members.len(),
            all = n
        ));
    }
    let exp = adw::ExpanderRow::builder()
        .expanded(expanded.borrow().contains(&title))
        .title(glib::markup_escape_text(&title))
        .subtitle(glib::markup_escape_text(&sub.join(" · ")))
        .build();
    let ids: Vec<i64> = members.iter().map(|x| x.id).collect();
    // Libadwaita reiht spätere Suffixe links an: Löschen ganz rechts, davor Nachladen, Abo, Fortschritt.
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text(tr("Alle Folgen löschen"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    {
        let (title, ids) = (title.clone(), ids.clone());
        delete.connect_clicked(move |b| {
            let dialog = adw::AlertDialog::builder()
                .heading(tr("Alle Folgen löschen?"))
                .body(tf!(
                    "{folgen} von „{title}“ werden von der Platte gelöscht. Das lässt sich nicht rückgängig machen.",
                    folgen = trn("{n} Folge", "{n} Folgen", ids.len() as u64),
                    title = title
                ))
                .build();
            dialog.add_responses(&[("keep", tr("Behalten")), ("delete", tr("Löschen"))]);
            dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("keep"));
            let ids = ids.clone();
            dialog.connect_response(None, move |_, r| {
                if r == "delete" {
                    for id in &ids {
                        crate::ablage::delete_entry(*id);
                    }
                }
            });
            dialog.present(Some(b));
        });
    }
    exp.add_suffix(&delete);
    // Server-Staffel: fehlende Folgen nachladen und Abo (Nutzer, 2026-10-28; je Staffel).
    if let Some(series_id) = key.series_id.clone() {
        let missing = crate::ablage::missing_episodes(&key.library, &series_id, key.season).len();
        let reload = small_button(&if missing > 0 {
            tf!("Nachladen ({n})", n = missing)
        } else {
            tr("Nachladen").to_string()
        });
        reload.set_sensitive(missing > 0);
        reload.set_tooltip_text(Some(tr("Fehlende Folgen der Staffel herunterladen")));
        {
            let (weak, library, series_id, season) = (Rc::downgrade(app), key.library.clone(), series_id.clone(), key.season);
            reload.connect_clicked(move |b| {
                if let Some(app) = weak.upgrade() {
                    let n = crate::ablage::reload_season(&app, &library, &series_id, season);
                    b.set_sensitive(false);
                    app.message(&trn("{n} Folge wird geladen", "{n} Folgen werden geladen", n as u64));
                }
            });
        }
        exp.add_suffix(&reload);
        let modes = [tr("Abo aus"), tr("Abo manuell"), tr("Abo automatisch")];
        let abo = gtk::DropDown::from_strings(&modes);
        abo.set_valign(gtk::Align::Center);
        abo.set_tooltip_text(Some(tr("Neue Folgen dieser Staffel: melden (manuell) oder selbst laden (automatisch)")));
        use tvjoe_core::ablage::Subscription;
        abo.set_selected(match crate::ablage::subscription(&key.library, &series_id, key.season) {
            None => 0,
            Some(Subscription::Manual) => 1,
            Some(Subscription::Auto) => 2,
        });
        {
            let (weak, library, season) = (Rc::downgrade(app), key.library.clone(), key.season);
            abo.connect_selected_notify(move |d| {
                let mode = match d.selected() {
                    1 => Some(Subscription::Manual),
                    2 => Some(Subscription::Auto),
                    _ => None,
                };
                crate::ablage::set_subscription(&library, &series_id, season, mode);
                // Automatisch: Fehlendes gleich laden, nicht erst nach dem nächsten Abgleich.
                if mode == Some(Subscription::Auto) {
                    if let Some(app) = weak.upgrade() {
                        crate::ablage::reload_season(&app, &library, &series_id, season);
                    }
                }
            });
        }
        exp.add_suffix(&abo);
        if missing > 0 {
            sub.push(tf!("{n} fehlen", n = missing));
            exp.set_subtitle(&glib::markup_escape_text(&sub.join(" · ")));
        }
    }
    let loading = members.iter().any(|x| {
        crate::ablage::running_fraction(x.id).is_some()
            || matches!(x.state, State::Queued | State::Loading)
    });
    if loading {
        let fraction =
            members.iter().map(|x| entry_fraction(x)).sum::<f64>() / members.len() as f64;
        let ready = members.iter().filter(|x| x.state == State::Done).count();
        let bar = gtk::ProgressBar::builder()
            .fraction(fraction)
            .width_request(160)
            .valign(gtk::Align::Center)
            .show_text(true)
            .text(tf!(
                "{done} von {all} · {percent} %",
                done = ready,
                all = members.len(),
                percent = format!("{:.0}", fraction * 100.0)
            ))
            .build();
        exp.add_suffix(&bar);
    }
    for x in members {
        exp.add_row(&entry_row(app, x, size(x)));
    }
    let (set, name) = (expanded.clone(), title);
    exp.connect_expanded_notify(move |x| {
        if x.is_expanded() {
            set.borrow_mut().insert(name.clone());
        } else {
            set.borrow_mut().remove(&name);
        }
    });
    exp.upcast()
}


/// „Alles prüfen“: Befunde mit Nachladen bzw. Löschen je Zeile und „Alle korrigieren“.
fn check_all(app: &Rc<App>, parent: &gtk::Button) {
    let (found, entries, files) = crate::ablage::check_all(app);
    let checked = tf!(
        "Geprüft: {e}, {f}",
        e = trn("{n} Eintrag", "{n} Einträge", entries as u64),
        f = trn("{n} Datei", "{n} Dateien", files as u64)
    );
    let dialog = adw::AlertDialog::builder()
        .heading(if found.is_empty() {
            tr("Alles in Ordnung").to_string()
        } else {
            trn("{n} Problem gefunden", "{n} Probleme gefunden", found.len() as u64)
        })
        .body(&checked)
        .build();
    if found.is_empty() {
        dialog.add_response("close", tr("OK"));
        dialog.present(Some(parent));
        return;
    }
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
    for f in &found {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&f.title))
            .subtitle(glib::markup_escape_text(&f.problem))
            .build();
        let make = |icon: &str, tip: &str, reload: bool| {
            let b = gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tip)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let (weak, f) = (Rc::downgrade(app), f.clone());
            b.connect_clicked(move |b| {
                if let Some(app) = weak.upgrade() {
                    crate::ablage::fix_finding(&app, &f, reload);
                }
                // Zeile als erledigt kennzeichnen.
                if let Some(r) = b.ancestor(adw::ActionRow::static_type()) {
                    r.set_sensitive(false);
                }
            });
            b
        };
        if f.reload {
            row.add_suffix(&make("folder-download-symbolic", tr("Nachladen"), true));
        }
        row.add_suffix(&make("user-trash-symbolic", tr("Löschen"), false));
        list.append(&row);
    }
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(360)
        .child(&list)
        .build();
    dialog.set_extra_child(Some(&scroll));
    // Breiter ab libadwaita 1.6 (Ubuntu 24.04: Eigenschaft fehlt).
    if dialog.find_property("prefer-wide-layout").is_some() {
        dialog.set_property("prefer-wide-layout", true);
    }
    dialog.add_response("close", tr("Schließen"));
    dialog.add_response("fix", tr("Alle korrigieren"));
    dialog.set_response_appearance("fix", adw::ResponseAppearance::Suggested);
    dialog.set_body(&tf!(
        "{checked}\n„Alle korrigieren“ lädt nach, wo es geht, und löscht den Rest (Aufnahmen ohne Datei, Dateien ohne Eintrag).",
        checked = checked
    ));
    let weak = Rc::downgrade(app);
    dialog.connect_response(Some("fix"), move |_, _| {
        if let Some(app) = weak.upgrade() {
            for f in &found {
                crate::ablage::fix_finding(&app, f, true);
            }
        }
    });
    dialog.present(Some(parent));
}

/// Ebene „Ablage“ (Festplatte in der Kopfleiste, Taste D).
struct AblageView {
    body: gtk::Widget,
}

impl crate::layer::View for AblageView {
    fn title(&self) -> String {
        tr("Ablage").to_string()
    }
    fn min_size(&self) -> (i32, i32) {
        (600, 400)
    }
    fn body(&self) -> gtk::Widget {
        self.body.clone()
    }
}

/// Öffnet die Ablage als Ebene (jedes Mal neu, mit aktueller Belegung).
pub fn open(app: &Rc<App>) {
    let view: Rc<dyn crate::layer::View> = Rc::new(AblageView { body: overview_page(app) });
    crate::layer::open(app, view);
}

/// Seite „Ablage“: Belegung, dann je Gruppe die Einträge (Jellyfin-Folgen nach Serien).
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
        let order = section_order(&app, &entries);
        let sum: u64 = entries.iter().map(|e| size(e)).sum();
        // Anteile für den Balken: je Server Serien und Filme, dann ÖR, Aufnahmen, Zwischenspeicher.
        let mut parts: Vec<(String, (f64, f64, f64), u64)> = Vec::new();
        let mut server = 0;
        for key in &order {
            let in_section: Vec<&Entry> = entries.iter().filter(|e| &section(e) == key).collect();
            let bytes = |f: &dyn Fn(&Entry) -> bool| in_section.iter().filter(|e| f(e)).map(|e| size(e)).sum::<u64>();
            let name = section_name(&app, key);
            match key.as_str() {
                "oer" => parts.push((name, hex(0x9FE1CB), bytes(&|_| true))),
                "~rec" => parts.push((name, hex(0xBA7517), bytes(&|_| true))),
                "~cache" => parts.push((name, hex(0x888780), bytes(&|_| true))),
                _ => {
                    let i = server.min(2);
                    server += 1;
                    parts.push((tf!("Serien {quelle}", quelle = name.as_str()), hex(SERIES_COLORS[i]), bytes(&|e| is_episode(e))));
                    parts.push((tf!("Filme {quelle}", quelle = name.as_str()), hex(FILM_COLORS[i]), bytes(&|e| !is_episode(e))));
                }
            }
        }
        let free = crate::ablage::free_space(&app)
            .map(size_text);
        let used = match free {
            Some(f) => tf!("Belegt {used} · {free} frei", used = size_text(sum), free = f),
            None => tf!("Belegt {used}", used = size_text(sum)),
        };
        content.append(&label(&used, "tvjoe-mt-detail-title"));
        content.append(&usage_bar(&parts));
        content.append(&label(
            &crate::ablage::storage_dir(&app).display().to_string(),
            "tvjoe-sub",
        ));
        let check = gtk::Button::builder()
            .label(tr("Alles prüfen"))
            .tooltip_text(tr("Alle lokalen Videodaten prüfen: fehlende, unvollständige und verwaiste Dateien"))
            .halign(gtk::Align::Start)
            .margin_top(8)
            .build();
        {
            let weak = Rc::downgrade(&app);
            check.connect_clicked(move |b| {
                if let Some(app) = weak.upgrade() {
                    check_all(&app, b);
                }
            });
        }
        content.append(&check);

        for key in &order {
            let list: Vec<&Entry> = entries.iter().filter(|e| &section(e) == key).collect();
            if list.is_empty() {
                continue;
            }
            let g = group(&section_name(&app, key), None);
            // Folgen einer Staffel (Jellyfin) bzw. einer Sendung (ÖR, ab zwei Beiträgen)
            // zusammen, je Gruppe mit Fortschritt, Prüfen und Löschen (Nutzer, 2026-10-27).
            let keys: Vec<Option<GroupKey>> = list.iter().map(|e| group_key(e)).collect();
            let mut done: Vec<GroupKey> = Vec::new();
            for (e, key) in list.iter().zip(&keys) {
                let Some(key) = key else {
                    g.add(&entry_row(&app, e, size(e)));
                    continue;
                };
                if done.contains(key) {
                    continue;
                }
                let mut members: Vec<&Entry> = list
                    .iter()
                    .zip(&keys)
                    .filter(|(_, k)| k.as_ref() == Some(key))
                    .map(|(x, _)| *x)
                    .collect();
                if !crate::ablage::is_server_source(&e.source) && members.len() < 2 {
                    g.add(&entry_row(&app, e, size(e)));
                    continue;
                }
                done.push(key.clone());
                members.sort_by_cached_key(|x| episode_order(x));
                g.add(&group_row(&app, key, &members, &size, &expanded));
            }
            content.append(&g);
        }
        if entries.is_empty() {
            content.append(&label(
                tr("Noch nichts in der Ablage. Rechtsklick auf eine Kachel in der Mediathek lädt herunter, Taste R nimmt die laufende Sendung auf."),
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
        tr("Aufnahme läuft").to_string()
    } else if running.is_some() {
        tr("Wird geladen").to_string()
    } else {
        match e.state {
            State::Done => tr("Vollständig").into(),
            State::Partial => tr("Teilweise").into(),
            State::Queued => tr("Wartet").into(),
            State::Loading => tr("Wird geladen").into(),
            State::Failed => tr("Fehlgeschlagen").into(),
        }
    };
    let mut sub = vec![state, size_text(size), day(e.created)];
    if !e.subtitle.is_empty() && !crate::ablage::is_server_source(&e.source) {
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
                Some(f) => tf!("{percent} %", percent = format!("{:.0}", f * 100.0)),
                None => tr("lädt").to_string(),
            })
            .build();
        if f.is_none() {
            bar.pulse();
        }
        row.add_suffix(&bar);
    }
    let id = e.id;
    if e.state == State::Done && !recording {
        let play = small_button(tr("Abspielen"));
        let app = app.clone();
        play.connect_clicked(move |_| {
            crate::ablage::play_entry(&app, id);
            crate::layer::close_layer(&app);
        });
        row.add_suffix(&play);
    }
    if e.state == State::Partial && running.is_none() && e.art != Art::Recording {
        let resume = small_button(tr("Fortsetzen"));
        let app = app.clone();
        resume.connect_clicked(move |_| crate::ablage::resume_entry(&app, id));
        row.add_suffix(&resume);
    }
    if running.is_some() {
        let cancel = small_button(tr("Abbrechen"));
        let (s, k) = (e.source.clone(), e.key.clone());
        cancel.connect_clicked(move |_| crate::ablage::cancel(&s, &k));
        row.add_suffix(&cancel);
    }
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text(tr("Löschen"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let title = e.title.clone();
    delete.connect_clicked(move |b| {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Wirklich löschen?"))
            .body(tf!(
                "„{title}“ wird von der Platte gelöscht. Das lässt sich nicht rückgängig machen.",
                title = title
            ))
            .build();
        dialog.add_responses(&[("keep", tr("Behalten")), ("delete", tr("Löschen"))]);
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
