//! Import einer M3U (Datei oder URL) mit Vorschau.
//!
//! Regeln (Projektplan, „M3U-Prüfung beim Import“): Fehler sind nicht auswählbar, Einträge mit
//! Warnungen sind abgewählt, die Erreichbarkeit wird automatisch, parallel und abbrechbar geprüft
//! (nur der Anfang jedes Streams, kein Abspielen). Nicht erreichbare Einträge werden abgewählt.

use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adw::prelude::*;
use gtk::{gio, glib};
use tvjoe_core::net::{self, Probe};
use tvjoe_core::{Playlist, Quality, Severity};

use crate::manage::Manager;

const PROBE_THREADS: usize = 8;
const MAX_M3U_BYTES: u64 = 32 * 1024 * 1024;

/// Gruppen eines Eintrags; mehrere stehen durch `;` getrennt in `group-title`.
pub fn groups_of(e: &tvjoe_core::Entry) -> impl Iterator<Item = &str> {
    e.group()
        .unwrap_or("")
        .split(';')
        .map(str::trim)
        .filter(|g| !g.is_empty())
}

pub fn from_file(mgr: &Rc<Manager>) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("M3U-Listen"));
    for p in ["*.m3u", "*.m3u8", "*.M3U", "*.M3U8"] {
        filter.add_pattern(p);
    }
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title("M3U importieren")
        .modal(true)
        .filters(&filters)
        .default_filter(&filter)
        .build();
    let m = mgr.clone();
    dialog.open(Some(&mgr.win), gio::Cancellable::NONE, move |res| {
        let Ok(file) = res else { return };
        let Some(path) = file.path() else { return };
        match std::fs::read(&path) {
            Ok(bytes) => {
                let location = std::fs::canonicalize(&path)
                    .unwrap_or(path.clone())
                    .to_string_lossy()
                    .into_owned();
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| location.clone());
                preview(&m, &bytes, location, name);
            }
            Err(e) => m.toast(&format!("{}: {e}", path.display())),
        }
    });
}

pub fn from_url(mgr: &Rc<Manager>) {
    let entry = gtk::Entry::builder()
        .placeholder_text("https://…/liste.m3u")
        .activates_default(true)
        .input_purpose(gtk::InputPurpose::Url)
        .build();
    let d = adw::AlertDialog::new(Some("M3U von URL laden"), Some("Adresse der Liste:"));
    d.add_response("cancel", "Abbrechen");
    d.add_response("ok", "Laden");
    d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("ok"));
    d.set_close_response("cancel");
    d.set_extra_child(Some(&entry));
    let m = mgr.clone();
    d.connect_response(Some("ok"), move |_, _| {
        let url = entry.text().trim().to_string();
        if url.is_empty() {
            return;
        }
        m.toast("Lade Liste …");
        let (tx, rx) = async_channel::bounded(1);
        let u = url.clone();
        std::thread::spawn(move || {
            let agent = net::agent(Duration::from_secs(20));
            let _ = tx.send_blocking(net::fetch(&agent, &u, MAX_M3U_BYTES));
        });
        let m = m.clone();
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(bytes)) => {
                    let name = url
                        .rsplit('/')
                        .next()
                        .and_then(|f| f.split('.').next())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Liste")
                        .to_string();
                    preview(&m, &bytes, url, name);
                }
                Ok(Err(e)) => m.toast(&format!("Laden fehlgeschlagen: {e}")),
                Err(_) => {}
            }
        });
    });
    d.present(Some(&mgr.win));
}

pub struct Preview {
    mgr: Rc<Manager>,
    pub win: adw::Window,
    playlist: Playlist,
    location: String,
    source_name: gtk::Entry,
    checks: Vec<gtk::CheckButton>,
    reach: Vec<gtk::Label>,
    status: gtk::Label,
    cancel: Arc<AtomicBool>,
    import_button: gtk::Button,
}

fn preview(mgr: &Rc<Manager>, bytes: &[u8], location: String, name: String) -> Option<Rc<Preview>> {
    let playlist = match Playlist::parse(bytes) {
        Ok(p) => p,
        Err(e) => {
            let d = adw::AlertDialog::new(Some("Liste nicht übernehmbar"), Some(&e.to_string()));
            d.add_response("ok", "OK");
            d.present(Some(&mgr.win));
            return None;
        }
    };
    let existing = mgr
        .app
        .library()
        .sources
        .iter()
        .find(|s| s.location == location)
        .map(|s| s.name.clone());

    let win = adw::Window::builder()
        .title(format!("Importieren – {name}"))
        .default_width(1180)
        .default_height(780)
        .modal(true)
        .transient_for(&mgr.win)
        .build();
    let header = adw::HeaderBar::new();
    let cancel_button = gtk::Button::with_label("Abbrechen");
    let import_button = gtk::Button::with_label("Übernehmen");
    import_button.add_css_class("suggested-action");
    header.pack_start(&cancel_button);
    header.pack_end(&import_button);

    let errors = playlist
        .findings
        .iter()
        .filter(|f| f.kind.severity() == Severity::Error)
        .count();
    let summary = gtk::Label::builder()
        .label(format!(
            "{} Einträge · {} Befunde, davon {errors} Fehler{}",
            playlist.entries.len(),
            playlist.findings.len(),
            if existing.is_some() {
                " · Quelle ist schon vorhanden und wird aktualisiert"
            } else {
                ""
            }
        ))
        .css_classes(["tvjoe-name"])
        .xalign(0.0)
        .build();
    let source_name = gtk::Entry::builder()
        .text(existing.as_deref().unwrap_or(&name))
        .width_request(260)
        .build();
    let groups: BTreeSet<String> = playlist
        .entries
        .iter()
        .flat_map(|e| groups_of(e).map(str::to_string).collect::<Vec<_>>())
        .collect();
    let group_list: Vec<String> = groups.into_iter().collect();
    let group_refs: Vec<&str> = group_list.iter().map(String::as_str).collect();
    let group_drop = gtk::DropDown::from_strings(&group_refs);
    let all = gtk::Button::with_label("Alle");
    let none = gtk::Button::with_label("Keine");
    let group_on = gtk::Button::with_label("Gruppe wählen");
    let group_off = gtk::Button::with_label("Gruppe abwählen");
    let status = gtk::Label::builder()
        .css_classes(["tvjoe-sub"])
        .xalign(0.0)
        .build();
    let stop = gtk::Button::with_label("Prüfung abbrechen");

    let top = gtk::Box::new(gtk::Orientation::Vertical, 8);
    top.set_margin_start(16);
    top.set_margin_end(16);
    top.set_margin_top(10);
    top.append(&summary);
    let line1 = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line1.append(&gtk::Label::new(Some("Name der Quelle:")));
    line1.append(&source_name);
    top.append(&line1);
    let line2 = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line2.append(&all);
    line2.append(&none);
    if !group_list.is_empty() {
        line2.append(&gtk::Label::new(Some("Gruppe:")));
        line2.append(&group_drop);
        line2.append(&group_on);
        line2.append(&group_off);
    }
    top.append(&line2);
    let line3 = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line3.append(&status);
    line3.append(&stop);
    top.append(&line3);

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(12)
        .margin_top(8)
        .build();
    let guide = mgr.app.guide();
    let mut checks = Vec::new();
    let mut reach = Vec::new();
    for (i, e) in playlist.entries.iter().enumerate() {
        let own: Vec<_> = playlist
            .findings
            .iter()
            .filter(|f| f.entry == Some(i))
            .collect();
        let has_error = own.iter().any(|f| f.kind.severity() == Severity::Error);
        let check = gtk::CheckButton::builder()
            .active(own.is_empty())
            .sensitive(!has_error)
            .build();
        let label = |t: &str, w: i32, class: &str| {
            gtk::Label::builder()
                .label(t)
                .width_request(w)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes([class])
                .build()
        };
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        hbox.set_margin_top(4);
        hbox.set_margin_bottom(4);
        hbox.set_margin_start(8);
        hbox.set_margin_end(8);
        hbox.append(&check);
        hbox.append(&label(
            &e.chno().map(|n| n.to_string()).unwrap_or_default(),
            40,
            "tvjoe-num",
        ));
        let name_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        name_box.set_width_request(300);
        name_box.append(&label(&e.name, 300, "tvjoe-name"));
        if !own.is_empty() {
            let text: Vec<String> = own.iter().map(|f| f.kind.to_string()).collect();
            let l = label(&text.join(" · "), 300, "tvjoe-sub");
            l.set_tooltip_text(Some(&text.join("\n")));
            name_box.append(&l);
        }
        hbox.append(&name_box);
        hbox.append(&label(e.group().unwrap_or("–"), 150, "tvjoe-sub"));
        hbox.append(&label(
            e.quality().map_or("?", Quality::label),
            70,
            "tvjoe-sub",
        ));
        let r = label("…", 200, "tvjoe-sub");
        hbox.append(&r);
        let epg_text = match &guide {
            Some(g) => {
                let names: Vec<&str> = [Some(e.name.as_str()), e.attr("tvg-name")]
                    .into_iter()
                    .flatten()
                    .collect();
                match g.match_parts(None, e.tvg_id(), &names) {
                    Some((id, _)) => format!("EPG: ✔ {id}"),
                    None => "EPG: ✖ keine Zuordnung".into(),
                }
            }
            None => "EPG: –".into(),
        };
        hbox.append(&label(&epg_text, 200, "tvjoe-sub"));
        list.append(&hbox);
        checks.push(check);
        reach.push(r);
    }
    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&list)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(&top);
    body.append(&scroller);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&body));
    win.set_content(Some(&view));

    let p = Rc::new(Preview {
        mgr: mgr.clone(),
        win,
        playlist,
        location,
        source_name,
        checks,
        reach,
        status,
        cancel: Arc::new(AtomicBool::new(false)),
        import_button,
    });
    p.update_count();
    for c in &p.checks {
        let w = Rc::downgrade(&p);
        c.connect_toggled(move |_| {
            if let Some(p) = w.upgrade() {
                p.update_count();
            }
        });
    }
    let w = Rc::downgrade(&p);
    all.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.set_where(|_| true, true);
        }
    });
    let w = Rc::downgrade(&p);
    none.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.set_where(|_| true, false);
        }
    });
    for (button, on) in [(&group_on, true), (&group_off, false)] {
        let w = Rc::downgrade(&p);
        let drop = group_drop.clone();
        let groups = group_list.clone();
        button.connect_clicked(move |_| {
            let Some(p) = w.upgrade() else { return };
            let Some(g) = groups.get(drop.selected() as usize).cloned() else {
                return;
            };
            p.set_where(|e| groups_of(e).any(|x| x == g), on);
        });
    }
    let c = p.cancel.clone();
    stop.connect_clicked(move |b| {
        c.store(true, Ordering::Relaxed);
        b.set_sensitive(false);
    });
    let w = Rc::downgrade(&p);
    cancel_button.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.win.close();
        }
    });
    let w = Rc::downgrade(&p);
    p.import_button.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.apply();
        }
    });
    let c = p.cancel.clone();
    p.win.connect_close_request(move |_| {
        c.store(true, Ordering::Relaxed);
        glib::Propagation::Proceed
    });
    p.start_probes(stop);
    p.win.present();
    Some(p)
}

/// Öffnet die Vorschau für eine Datei ohne Dateiauswahl (Selbstprobe).
pub fn preview_path(mgr: &Rc<Manager>, path: &std::path::Path) -> Option<Rc<Preview>> {
    let bytes = std::fs::read(path).ok()?;
    let location = std::fs::canonicalize(path)
        .unwrap_or(path.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let name = path.file_stem()?.to_string_lossy().into_owned();
    preview(mgr, &bytes, location, name)
}

impl Preview {
    fn selected(&self) -> Vec<usize> {
        self.checks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_active() && c.is_sensitive())
            .map(|(i, _)| i)
            .collect()
    }

    fn update_count(&self) {
        let n = self.selected().len();
        self.import_button.set_label(&format!("Übernehmen ({n})"));
        self.import_button.set_sensitive(n > 0);
    }

    pub fn set_where(&self, pred: impl Fn(&tvjoe_core::Entry) -> bool, on: bool) {
        for (e, c) in self.playlist.entries.iter().zip(&self.checks) {
            if c.is_sensitive() && pred(e) {
                c.set_active(on);
            }
        }
    }

    fn start_probes(self: &Rc<Self>, stop: gtk::Button) {
        let total = self.playlist.entries.len();
        let queue: Arc<Mutex<VecDeque<(usize, String)>>> = Arc::new(Mutex::new(
            self.playlist
                .entries
                .iter()
                .enumerate()
                .map(|(i, e)| (i, e.url.clone()))
                .collect(),
        ));
        let (tx, rx) = async_channel::unbounded::<(usize, Probe)>();
        for _ in 0..PROBE_THREADS.min(total.max(1)) {
            let queue = queue.clone();
            let tx = tx.clone();
            let cancel = self.cancel.clone();
            std::thread::spawn(move || {
                let agent = net::agent(Duration::from_secs(8));
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let Some((i, url)) = queue.lock().ok().and_then(|mut q| q.pop_front()) else {
                        break;
                    };
                    if tx.send_blocking((i, net::probe(&agent, &url))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (mut done, mut ok) = (0usize, 0usize);
            while let Ok((i, probe)) = rx.recv().await {
                let Some(p) = w.upgrade() else { break };
                done += 1;
                let text = match &probe {
                    Probe::Ok(code) => {
                        ok += 1;
                        format!("✔ erreichbar ({code})")
                    }
                    Probe::Skipped => "– nicht geprüft".to_string(),
                    Probe::Failed(e) => {
                        p.checks[i].set_active(false);
                        format!("✖ {e}")
                    }
                };
                p.reach[i].set_label(&text);
                p.reach[i].set_tooltip_text(Some(&text));
                p.status.set_label(&format!(
                    "Erreichbarkeit: {done} von {total} geprüft, {ok} erreichbar"
                ));
            }
            if let Some(p) = w.upgrade() {
                let cancelled = p.cancel.load(Ordering::Relaxed);
                p.status.set_label(&format!(
                    "Erreichbarkeit {}: {done} von {total} geprüft, {ok} erreichbar",
                    if cancelled { "abgebrochen" } else { "geprüft" }
                ));
                stop.set_sensitive(false);
            }
        });
    }

    pub fn apply(&self) {
        let selected = self.selected();
        if selected.is_empty() {
            return;
        }
        let name = self.source_name.text().trim().to_string();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let all = selected.len() == self.playlist.entries.len();
        let outcome = {
            let mut lib = self.mgr.app.library_mut();
            let source = match lib.sources.iter_mut().find(|s| s.location == self.location) {
                Some(s) => {
                    if !name.is_empty() {
                        s.name = name.clone();
                    }
                    s.id
                }
                None => lib.add_source(
                    if name.is_empty() { "Liste" } else { &name },
                    &self.location,
                ),
            };
            lib.import(source, &self.playlist, (!all).then_some(&selected[..]), now)
        };
        self.cancel.store(true, Ordering::Relaxed);
        self.mgr.app.library_changed();
        self.mgr.refill();
        if let Some(o) = outcome {
            self.mgr.toast(&format!(
                "Übernommen: {} neu, {} aktualisiert{}",
                o.added,
                o.updated,
                if o.missing > 0 {
                    format!(", {} fehlen in der Quelle", o.missing)
                } else {
                    String::new()
                }
            ));
        }
        self.win.close();
    }
}
