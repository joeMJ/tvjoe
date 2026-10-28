//! Programmdaten aus dem Signal (EIT) für Sender mit EPG-Quelle „aus dem Signal“, je
//! Empfangsquelle mit Signal-EPG (SAT>IP, DVB-Karte): eine Minute nach dem Start und danach im
//! eingestellten Abstand je Transponder etwa 40 Sekunden lesen (nacheinander, ein Tuner), im
//! Zwischenspeicher der Quelle sammeln und das EPG neu laden.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use tvjoe_core::adapter::Driver;
use tvjoe_core::epg::{self, SatEpg};
use tvjoe_core::paths;

use crate::app::App;
use crate::trace;

/// Lesedauer je Transponder (Sekunden).
const SECS_PER_TRANSPONDER: u64 = 40;

thread_local! {
    /// Quellen, deren Sammeln gerade läuft.
    static RUNNING: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
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

/// Empfangsquellen mit Programmdaten aus dem Signal: Nummer, Kennung, Abstand (Stunden).
fn signal_sources(app: &App) -> Vec<(u32, String, u64)> {
    let list: Vec<(u32, String, u64)> = {
        let s = app.settings();
        s.enabled_sources()
            .into_iter()
            .filter_map(|n| s.source(n))
            .map(|a| (a.num, a.id.clone(), u64::from(a.signal_epg_hours.max(1))))
            .collect()
    };
    list.into_iter()
        .filter(|(n, _, _)| matches!(app.driver_of(*n), Driver::Satip | Driver::LinuxDvb | Driver::Tvheadend))
        .collect()
}

/// Sammeln, wo der Zwischenspeicher älter ist als eingestellt (oder fehlt).
fn maybe_run(app: &Rc<App>) {
    let Some(dir) = paths::cache_dir() else { return };
    for (src, id, hours) in signal_sources(app) {
        let fresh = std::fs::metadata(epg::signal_cache_path(&dir, &id))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(hours * 3600));
        if !fresh {
            run_source(app, src, &id);
        }
    }
}

/// Jetzt sammeln (z. B. nachdem ein Sender auf „aus dem Signal“ gestellt wurde).
pub fn run(app: &Rc<App>) {
    for (src, id, _) in signal_sources(app) {
        run_source(app, src, &id);
    }
}

fn run_source(app: &Rc<App>, src: u32, id: &str) {
    if RUNNING.with(|r| r.borrow().contains(&src)) {
        return;
    }
    let driver = app.driver_of(src);
    // Tvheadend: Programm über die Schnittstelle des Servers (kein Tuner nötig).
    if driver == Driver::Tvheadend {
        run_tvheadend(app, src, id);
        return;
    }
    // Transponder der Sender mit Programm aus dem Signal, je Transponder einmal.
    let mut tuning: Vec<(String, String)> = Vec::new();
    let channels = app.with_list(src, |l| l.channels.clone());
    for ch in channels {
        if !ch.epg_id.as_deref().is_some_and(epg::is_signal_id) {
            continue;
        }
        let t = ch.streams.iter().find_map(|s| match driver {
            Driver::Satip => tvjoe_core::satip::tuning_of(&s.url),
            #[cfg(target_os = "linux")]
            Driver::LinuxDvb => tvjoe_core::dvb::tuning_of(&s.url),
            _ => None,
        });
        if let Some(t) = t {
            if !tuning.contains(&t) {
                tuning.push(t);
            }
        }
    }
    let Some(dir) = paths::cache_dir() else { return };
    if tuning.is_empty() {
        return;
    }
    RUNNING.with(|r| r.borrow_mut().insert(src));
    trace(&format!("Signal-EPG Quelle {src}: {} Transponder", tuning.len()));
    let path = epg::signal_cache_path(&dir, id);
    let (tx, rx) = async_channel::bounded::<Vec<String>>(1);
    std::thread::spawn(move || {
        let mut notes = Vec::new();
        let mut cache = SatEpg::load_from(&path);
        for (dev, query) in &tuning {
            let (result, prefix) = match driver {
                #[cfg(target_os = "linux")]
                Driver::LinuxDvb => (
                    tvjoe_core::dvb::collect_eit(dev, query, SECS_PER_TRANSPONDER),
                    tvjoe_core::dvb::SIGNAL_PREFIX,
                ),
                _ => (
                    tvjoe_core::satip::collect_eit(dev, query, SECS_PER_TRANSPONDER),
                    epg::SAT_PREFIX,
                ),
            };
            match result {
                Ok(sections) => {
                    let events: usize = sections.iter().map(|s| s.events.len()).sum();
                    notes.push(format!("{query}: {events} Einträge"));
                    cache.add_prefixed(&sections, epg::now(), prefix);
                }
                Err(e) => notes.push(format!("{query}: {e}")),
            }
        }
        if let Err(e) = cache.save_to(&path) {
            notes.push(format!("Speichern: {e}"));
        }
        let _ = tx.send_blocking(notes);
    });
    let weak = Rc::downgrade(app);
    glib::spawn_future_local(async move {
        let notes = rx.recv().await.unwrap_or_default();
        RUNNING.with(|r| r.borrow_mut().remove(&src));
        for n in &notes {
            trace(&format!("Signal-EPG {n}"));
        }
        if let Some(app) = weak.upgrade() {
            app.load_epg(false);
        }
    });
}

/// Tvheadend: Programm aller Kanäle vom Server holen, im Zwischenspeicher der Quelle ablegen.
fn run_tvheadend(app: &Rc<App>, src: u32, id: &str) {
    let host = app.settings().source(src).map(|a| a.host.clone()).unwrap_or_default();
    let Some(dir) = paths::cache_dir() else { return };
    if host.trim().is_empty() {
        return;
    }
    RUNNING.with(|r| r.borrow_mut().insert(src));
    let path = epg::signal_cache_path(&dir, id);
    let server = tvjoe_core::tvheadend::normalize(&host);
    let (tx, rx) = async_channel::bounded::<String>(1);
    std::thread::spawn(move || {
        let note = match tvjoe_core::tvheadend::access_path(&server)
            .and_then(|p| tvjoe_core::tvheadend::Access::load(&p))
            .ok_or_else(|| format!("kein Zugang zu {server} gespeichert"))
            .and_then(|a| a.epg(epg::now()))
        {
            Ok(guide) => {
                let n: usize = guide.programmes.values().map(Vec::len).sum();
                match guide.save_to(&path) {
                    Ok(()) => format!("Tvheadend: {n} Sendungen auf {} Kanälen", guide.programmes.len()),
                    Err(e) => format!("Tvheadend: Speichern: {e}"),
                }
            }
            Err(e) => format!("Tvheadend: {e}"),
        };
        let _ = tx.send_blocking(note);
    });
    let weak = Rc::downgrade(app);
    glib::spawn_future_local(async move {
        let note = rx.recv().await.unwrap_or_default();
        RUNNING.with(|r| r.borrow_mut().remove(&src));
        trace(&format!("Signal-EPG {note}"));
        if let Some(app) = weak.upgrade() {
            app.load_epg(false);
        }
    });
}
