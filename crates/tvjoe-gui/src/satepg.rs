//! Programmdaten aus dem SAT-Empfang (EIT) für Sender mit EPG-Quelle „SAT“: eine Minute nach
//! dem Start und danach im eingestellten Abstand je Transponder etwa 40 Sekunden lesen
//! (nacheinander, ein Tuner), im Zwischenspeicher sammeln und das EPG neu laden.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use tvjoe_core::epg::{SAT_PREFIX, SatEpg};
use tvjoe_core::paths;

use crate::app::App;
use crate::trace;

/// Lesedauer je Transponder (Sekunden).
const SECS_PER_TRANSPONDER: u64 = 40;

thread_local! {
    static RUNNING: Cell<bool> = const { Cell::new(false) };
}

/// Zeitgeber starten (beim Programmstart).
pub fn start(app: &Rc<App>) {
    let weak = Rc::downgrade(app);
    glib::timeout_add_seconds_local_once(60, move || {
        let Some(app) = weak.upgrade() else { return };
        maybe_run(&app);
        let weak = Rc::downgrade(&app);
        glib::timeout_add_seconds_local(300, move || match weak.upgrade() {
            Some(app) => {
                maybe_run(&app);
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    });
}

/// Sammeln, wenn der Zwischenspeicher älter ist als eingestellt (oder fehlt).
fn maybe_run(app: &Rc<App>) {
    let Some(dir) = paths::cache_dir() else { return };
    let hours = u64::from(app.settings().sat_epg_hours.max(1));
    let fresh = std::fs::metadata(tvjoe_core::epg::sat_cache_path(&dir))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(hours * 3600));
    if !fresh {
        run(app);
    }
}

/// Jetzt sammeln (z. B. nachdem ein Sender auf „SAT“ gestellt wurde).
pub fn run(app: &Rc<App>) {
    if RUNNING.with(Cell::get) || !app.settings().satip_enabled {
        return;
    }
    // Transponder der Sender mit EPG-Quelle „SAT“, je Transponder einmal.
    let mut tuning: Vec<(String, String)> = Vec::new();
    for ch in app.sat_channels() {
        if !ch
            .epg_id
            .as_deref()
            .is_some_and(|id| id.starts_with(SAT_PREFIX))
        {
            continue;
        }
        if let Some(t) = ch
            .streams
            .iter()
            .find_map(|s| tvjoe_core::satip::tuning_of(&s.url))
        {
            if !tuning.contains(&t) {
                tuning.push(t);
            }
        }
    }
    let Some(dir) = paths::cache_dir() else { return };
    if tuning.is_empty() {
        return;
    }
    RUNNING.with(|r| r.set(true));
    trace(&format!("SAT-EPG: {} Transponder", tuning.len()));
    let (tx, rx) = async_channel::bounded::<Vec<String>>(1);
    std::thread::spawn(move || {
        let mut notes = Vec::new();
        let mut epg = SatEpg::load(&dir);
        for (host, query) in &tuning {
            match tvjoe_core::satip::collect_eit(host, query, SECS_PER_TRANSPONDER) {
                Ok(sections) => {
                    let events: usize = sections.iter().map(|s| s.events.len()).sum();
                    notes.push(format!("{query}: {events} Einträge"));
                    epg.add(&sections, tvjoe_core::epg::now());
                }
                Err(e) => notes.push(format!("{query}: {e}")),
            }
        }
        if let Err(e) = epg.save(&dir) {
            notes.push(format!("Speichern: {e}"));
        }
        let _ = tx.send_blocking(notes);
    });
    let weak = Rc::downgrade(app);
    glib::spawn_future_local(async move {
        let notes = rx.recv().await.unwrap_or_default();
        RUNNING.with(|r| r.set(false));
        for n in &notes {
            trace(&format!("SAT-EPG {n}"));
        }
        if let Some(app) = weak.upgrade() {
            app.load_epg(false);
        }
    });
}
