//! Aufnehmen ohne Fenster (Nutzer, 2026-10-26): `tvjoe --aufnahmedienst` läuft dauerhaft
//! (Linux: systemd-Benutzerdienst) und startet geplante Aufnahmen zur Zeit; `tvjoe --aufnahme
//! <Nummer>` nimmt eine geplante Aufnahme auf und endet (Windows: eine Aufgabe je Aufnahme in
//! der Aufgabenplanung). Beide lesen Plan, Einstellungen und Senderlisten von der Platte und
//! schreiben ins Verzeichnis der Ablage; das Fenster zeigt den Stand nur an.
//!
//! Ein geplanter Eintrag endet mit Sendungsende plus Nachlauf oder sobald er aus dem Plan
//! gelöscht wird (so beendet das Fenster eine Aufnahme mit R).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tvjoe_core::ablage::{self, Art, Index, Progress, State, Timer};
use tvjoe_core::{Library, Settings, paths};

/// Lebenszeichen des Dienstes (Zeitstempel); das Fenster zeigt damit „Dienst läuft“.
pub fn heartbeat_path() -> Option<PathBuf> {
    paths::cache_dir().map(|d| d.join("aufnahmedienst.lebt"))
}

/// Läuft der Dienst (Lebenszeichen jünger als 20 s)?
pub fn alive() -> bool {
    #[cfg(windows)]
    return crate::dienst_win::alive();
    #[cfg(not(windows))]
    heartbeat_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse::<i64>().ok())
        .is_some_and(|t| ablage::now() - t < 20)
}

pub fn index() -> Result<Index, String> {
    let path = paths::config_dir()
        .map(|d| d.join("ablage.sqlite"))
        .ok_or("kein Konfigurationsordner")?;
    Index::open(&path)
}

fn settings() -> Settings {
    paths::settings_path()
        .and_then(|p| Settings::load(&p).ok())
        .unwrap_or_default()
}

/// `tvjoe --aufnahmedienst`: alle 5 s Lebenszeichen schreiben und den Plan abarbeiten.
pub fn run_service() -> i32 {
    eprintln!("tvjoe: Aufnahmedienst gestartet");
    let running: Arc<Mutex<HashSet<i64>>> = Arc::default();
    loop {
        if let Some(p) = heartbeat_path() {
            let _ = p.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(p, ablage::now().to_string());
        }
        if let Ok(ix) = index() {
            let s = settings();
            let now = ablage::now();
            let (pre, post) = (
                i64::from(s.record_pre_min) * 60,
                i64::from(s.record_post_min) * 60,
            );
            for t in ix.timers() {
                let end = t.stop + post;
                let is_running = running.lock().is_ok_and(|r| r.contains(&t.id));
                if now >= end {
                    if !is_running {
                        ix.remove_timer(t.id);
                    }
                } else if now >= t.start - pre && !is_running {
                    if let Ok(mut r) = running.lock() {
                        r.insert(t.id);
                    }
                    let running = running.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = record(&t) {
                            eprintln!("tvjoe: Aufnahme „{}“: {e}", t.title);
                        }
                        if let Ok(ix) = index() {
                            ix.remove_timer(t.id);
                        }
                        if let Ok(mut r) = running.lock() {
                            r.remove(&t.id);
                        }
                    });
                }
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// `tvjoe --aufnahme <Nummer>`: diese geplante Aufnahme jetzt aufnehmen (Aufgabenplanung).
pub fn run_one(timer_id: i64) -> i32 {
    let Ok(ix) = index() else { return 1 };
    let Some(t) = ix.timer(timer_id) else {
        eprintln!("tvjoe: Aufnahme {timer_id} ist nicht (mehr) geplant");
        return 0;
    };
    let r = record(&t);
    ix.remove_timer(t.id);
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("tvjoe: Aufnahme „{}“: {e}", t.title);
            1
        }
    }
}

/// Eine geplante Aufnahme bis Sendungsende plus Nachlauf bzw. bis zur Löschung aus dem Plan.
fn record(t: &Timer) -> Result<(), String> {
    let s = settings();
    let lib_path = if t.sat {
        paths::satip_library_path()
    } else {
        paths::library_path()
    };
    let lib = lib_path
        .and_then(|p| Library::load(&p).ok())
        .ok_or("Senderliste nicht lesbar")?;
    let ch = lib.channel(t.channel_id).ok_or("Sender nicht gefunden")?;
    let stream = ch
        .pick_stream_capped(s.quality, s.fallback, s.quality_cap)
        .ok_or("kein Stream")?;
    let (url, bitrate) = (stream.url.clone(), stream.max_bitrate);
    let name = ch.display_name().to_string();
    let dir = crate::ablage::storage_dir_for(&s).join("records");
    let min_free = u64::from(s.min_free_gb) << 30;
    if crate::ablage::free_bytes(&dir).is_some_and(|f| f < min_free) {
        return Err("zu wenig Platz".into());
    }
    let now = ablage::now();
    let path = ablage::unique_path(&dir.join(ablage::show_name(
        &name,
        &t.title,
        &crate::ablage::stamp(now),
        "ts",
    )));
    let ix = index()?;
    let source = if t.sat { "sat" } else { "tv" };
    let id = ix.add(
        Art::Recording,
        source,
        &format!("{}@{now}", t.channel_id),
        &t.title,
        &name,
        &path,
    )?;
    ix.set_state(id, State::Loading, None);
    notify("Aufnahme läuft", &format!("{} – {name}", t.title));
    let _awake = KeepAwake::new();
    let end = t.stop + i64::from(s.record_post_min) * 60;
    let timer_id = t.id;
    let low_dir = dir.clone();
    let low = move || crate::ablage::free_bytes(&low_dir).is_some_and(|f| f < min_free);
    // Endet zur Zeit, bei Löschung aus dem Plan oder wenn der Platz ausgeht.
    let should_stop = move || {
        ablage::now() >= end
            || index().ok().is_some_and(|ix| ix.timer(timer_id).is_none())
            || low()
    };
    let queue = u64::from(s.buffer_mb) << 19;
    let result = record_stream(&url, bitrate, t.sat, &path, queue, &should_stop);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    ix.set_size(id, size, size);
    match &result {
        Ok(()) => ix.set_state(id, State::Done, None),
        Err(e) => ix.set_state(
            id,
            if size > 0 { State::Partial } else { State::Failed },
            Some(e),
        ),
    }
    notify(
        if result.is_ok() {
            "Aufnahme beendet"
        } else {
            "Aufnahme abgebrochen"
        },
        &match &result {
            Ok(()) => format!("{} – {name}", t.title),
            Err(e) => format!("{} – {name}: {e}", t.title),
        },
    );
    result
}

/// Einen Stream nach `path` aufnehmen, bis `should_stop` zutrifft (SAT: Transportstrom über
/// eigene Sitzung; Internet: unsichtbares mpv). Gemeinsam für Linux- und Windows-Dienst.
pub fn record_stream(
    url: &str,
    bitrate: Option<u64>,
    sat: bool,
    path: &std::path::Path,
    queue: u64,
    should_stop: &dyn Fn() -> bool,
) -> Result<(), String> {
    if sat {
        let rec = tvjoe_core::satip::Recorder::start(url, path, queue)?;
        while !should_stop() && rec.error().is_none() {
            std::thread::sleep(Duration::from_secs(2));
        }
        let err = rec.error();
        match (rec.finish(), err) {
            (Ok(_), None) => Ok(()),
            (Ok(_), Some(e)) => Err(e),
            (Err(e), _) => Err(e),
        }
    } else {
        let progress = Arc::new(Progress::default());
        let (p, u, pa) = (progress.clone(), url.to_string(), path.to_path_buf());
        let worker = std::thread::spawn(move || {
            let src = crate::capture::Source {
                url: &u,
                header: None,
                max_bitrate: bitrate,
                live: true,
            };
            crate::capture::run(&src, &pa, &p, &AtomicU64::new(0))
        });
        while !should_stop() && !worker.is_finished() {
            std::thread::sleep(Duration::from_secs(2));
        }
        progress.cancel.store(true, Ordering::Relaxed);
        worker.join().unwrap_or_else(|_| Err("Aufnahme abgestürzt".into()))
    }
}

// ---------------------------------------------------------------------------------------------
// Benachrichtigung und Wachhalten

/// Standardbenachrichtigung des Desktops (Linux: freedesktop-Benachrichtigungen über D-Bus).
pub fn notify(summary: &str, body: &str) {
    #[cfg(unix)]
    {
        use gtk::gio;
        use gtk::glib::prelude::*;
        let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
            return;
        };
        let hints: std::collections::HashMap<String, gtk::glib::Variant> = Default::default();
        let params = (
            "tvjoe",
            0u32,
            "media-record-symbolic",
            summary,
            body,
            Vec::<String>::new(),
            hints,
            -1i32,
        )
            .to_variant();
        let _ = bus.call_sync(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Notify",
            Some(&params),
            None,
            gio::DBusCallFlags::NONE,
            5000,
            gio::Cancellable::NONE,
        );
    }
    #[cfg(windows)]
    {
        crate::dienst_win::notify(summary, body);
    }
}

/// Hält den Rechner wach, solange es lebt (Linux: logind-Sperre „sleep“; Windows:
/// SetThreadExecutionState).
pub struct KeepAwake {
    #[cfg(unix)]
    _fds: Option<gtk::gio::UnixFDList>,
}

impl KeepAwake {
    pub fn new() -> Self {
        #[cfg(unix)]
        {
            use gtk::gio;
            use gtk::glib::prelude::*;
            let fds = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE)
                .ok()
                .and_then(|bus| {
                    bus.call_with_unix_fd_list_sync(
                        Some("org.freedesktop.login1"),
                        "/org/freedesktop/login1",
                        "org.freedesktop.login1.Manager",
                        "Inhibit",
                        Some(&("sleep", "tvjoe", "Aufnahme läuft", "block").to_variant()),
                        None,
                        gio::DBusCallFlags::NONE,
                        5000,
                        None::<&gio::UnixFDList>,
                        gio::Cancellable::NONE,
                    )
                    .map_err(|e| eprintln!("tvjoe: Ruhezustand nicht gesperrt: {e}"))
                    .ok()
                })
                .and_then(|(_, fds)| fds);
            Self { _fds: fds }
        }
        #[cfg(windows)]
        {
            crate::dienst_win::keep_awake(true);
            Self {}
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        #[cfg(windows)]
        crate::dienst_win::keep_awake(false);
    }
}

// ---------------------------------------------------------------------------------------------
// Einrichten (Linux: systemd-Benutzerdienst)

#[cfg(target_os = "linux")]
const UNIT: &str = "tvjoe-aufnahme.service";

#[cfg(target_os = "linux")]
fn unit_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("systemd/user").join(UNIT))
}

#[cfg(target_os = "linux")]
fn run_cmd(cmd: &str, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Dienst einrichten und starten; `linger`: auch ohne Anmeldung (loginctl enable-linger).
/// Gibt zurück, ob tvjoe das Weiterlaufen ohne Anmeldung eingeschaltet hat.
#[cfg(target_os = "linux")]
pub fn install(linger: bool) -> Result<bool, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = unit_path().ok_or("kein Konfigurationsordner")?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let unit = format!(
        "[Unit]\nDescription=tvjoe Aufnahmedienst (geplante Aufnahmen)\nAfter=network-online.target\n\n\
         [Service]\nExecStart=\"{}\" --aufnahmedienst\nRestart=on-failure\nRestartSec=10\n\n\
         [Install]\nWantedBy=default.target\n",
        exe.display()
    );
    std::fs::write(&path, unit).map_err(|e| e.to_string())?;
    run_cmd("systemctl", &["--user", "daemon-reload"])?;
    run_cmd("systemctl", &["--user", "enable", "--now", UNIT])?;
    // Ohne Anmeldung: nur einschalten, wenn es noch nicht an ist (dann bleibt es beim Abschalten).
    let mut set_linger = false;
    if linger {
        let user = std::env::var("USER").unwrap_or_default();
        let already = std::process::Command::new("loginctl")
            .args(["show-user", &user, "--property=Linger"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "Linger=yes");
        if !already {
            run_cmd("loginctl", &["enable-linger", &user])?;
            set_linger = true;
        }
    }
    Ok(set_linger)
}

/// Dienst anhalten und entfernen; `undo_linger`: von tvjoe gesetztes Weiterlaufen zurücknehmen.
#[cfg(target_os = "linux")]
pub fn uninstall(undo_linger: bool) -> Result<(), String> {
    let _ = run_cmd("systemctl", &["--user", "disable", "--now", UNIT]);
    if let Some(p) = unit_path() {
        let _ = std::fs::remove_file(p);
    }
    let _ = run_cmd("systemctl", &["--user", "daemon-reload"]);
    if undo_linger {
        let user = std::env::var("USER").unwrap_or_default();
        run_cmd("loginctl", &["disable-linger", &user])?;
    }
    Ok(())
}

/// Ist der Aufnahmedienst eingerichtet (Linux: Dienstdatei, Windows: Dienst in Program Files)?
/// Der Schalter richtet sich danach, nicht nur nach der Einstellung (die eine ältere Fassung
/// beim Speichern verlieren kann).
pub fn installed() -> bool {
    #[cfg(target_os = "linux")]
    return unit_path().is_some_and(|p| p.exists());
    #[cfg(windows)]
    return crate::dienst_win::installed_config().is_some();
    #[allow(unreachable_code)]
    false
}

/// Schalter „Aufnahmen auch ohne geöffnetes tvjoe“ umgelegt: im Hintergrund einrichten bzw.
/// entfernen (unter Windows mit Administrator-Bestätigung), damit das Fenster bedienbar bleibt.
/// Die Einstellung gilt erst, wenn es geklappt hat.
pub fn set_enabled(s: &mut Settings, on: bool) {
    let snapshot = s.clone();
    crate::ablage::report(if on {
        if cfg!(windows) {
            "Aufnahmedienst wird eingerichtet – bitte die Administrator-Bestätigung beantworten"
        } else {
            "Aufnahmedienst wird eingerichtet …"
        }
    } else if cfg!(windows) {
        "Aufnahmedienst wird entfernt – bitte die Administrator-Bestätigung beantworten"
    } else {
        "Aufnahmedienst wird entfernt …"
    });
    let (tx, rx) = async_channel::bounded::<Result<bool, String>>(1);
    std::thread::spawn(move || {
        let r = change_service(&snapshot, on);
        let _ = tx.send_blocking(r);
    });
    gtk::glib::spawn_future_local(async move {
        let Ok(r) = rx.recv().await else { return };
        let Some(app) = crate::ablage::app() else { return };
        match r {
            Ok(linger) => {
                {
                    let mut st = app.settings_mut();
                    st.record_service = on;
                    st.record_service_linger = on && (linger || st.record_service_linger);
                }
                app.save_settings();
                app.message(if on {
                    "Aufnahmedienst eingerichtet: geplante Aufnahmen laufen auch ohne geöffnetes tvjoe"
                } else {
                    "Aufnahmedienst entfernt: aufgenommen wird wieder nur bei geöffnetem tvjoe"
                });
                sync_tasks(&app);
            }
            Err(e) => app.message(&format!("Aufnahmedienst: {e}")),
        }
    });
}

/// Einrichten bzw. entfernen (im Hintergrund-Thread). Linux: gibt zurück, ob tvjoe das
/// Weiterlaufen ohne Anmeldung eingeschaltet hat.
fn change_service(s: &Settings, on: bool) -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        if on {
            install(true)
        } else {
            uninstall(s.record_service_linger).map(|()| false)
        }
    }
    #[cfg(windows)]
    {
        if on {
            crate::dienst_win::install(s).map(|()| false)
        } else {
            crate::dienst_win::uninstall(s).map(|()| false)
        }
    }
}

/// Windows: Aufträge für den Dienst an den Plan angleichen (Linux: der Dienst liest den Plan
/// selbst).
pub fn sync_tasks(app: &crate::app::App) {
    #[cfg(windows)]
    if app.settings().record_service {
        crate::dienst_win::sync_jobs(app);
    }
    #[cfg(not(windows))]
    let _ = app;
}

/// Sofortaufnahme an den Dienst übergeben (Windows: eigener Vorgang; Linux: der Dienst liest
/// den Plan alle 5 s).
pub fn start_now(timer_id: i64) {
    // Windows: der Auftrag geht mit `sync_tasks` hinaus, der Dienst startet ihn binnen 5 s.
    let _ = timer_id;
}
