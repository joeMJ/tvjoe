//! Windows-Aufnahmedienst (Nutzer, 2026-10-27): ein echter Dienst „tvjoe“ unter dem virtuellen
//! Konto `NT SERVICE\tvjoe`, einmal mit Administratorrechten eingerichtet (Kopie nach
//! `Program Files\tvjoe-dienst`, dort keine Rechteänderung). Er liest Aufträge aus
//! `%LOCALAPPDATA%\tvjoe\auftraege` (nur Lesen), prüft jeden vollständig
//! ([`tvjoe_core::auftrag::parse_job`]), nimmt nach `Videos\tvjoe\records` auf und meldet den
//! Stand nach `%LOCALAPPDATA%\tvjoe\stand`. In diesen drei Benutzerordnern bekommt das
//! Dienstkonto je einen zusätzlichen Eintrag; die Vererbung bleibt.
//!
//! Das Fenster schreibt die Aufträge, übernimmt den Stand in die Ablage und zeigt Meldungen
//! (ein Dienst kann keine Desktop-Meldungen zeigen). Der Dienst weckt den Rechner vor der
//! nächsten Aufnahme (Weck-Zeitgeber, „Zeitgeber zur Aktivierung“ im Energieplan).

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tvjoe_core::ablage;
use tvjoe_core::{tf, tr};
use tvjoe_core::auftrag::{self, Job, JobState, ServiceConfig, Status};

const SERVICE_NAME: &str = "tvjoe";
const CONFIG_FILE: &str = "dienst.toml";

// ---------------------------------------------------------------------------------------------
// Wachhalten, Meldungen

/// Rechner während einer Aufnahme nicht in den Ruhezustand gehen lassen.
pub fn keep_awake(on: bool) {
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    unsafe extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }
    let flags = if on {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    unsafe {
        SetThreadExecutionState(flags);
    }
}

/// Standardbenachrichtigung von Windows (Toast über PowerShell, ohne eigenes Fenster); nur aus
/// dem Fenster heraus, der Dienst selbst hat keinen Desktop.
pub fn notify(summary: &str, body: &str) {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\'', "''")
    };
    let script = format!(
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
         [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null; \
         $x = New-Object Windows.Data.Xml.Dom.XmlDocument; \
         $x.LoadXml('<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>'); \
         $id = '{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe'; \
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($id).Show([Windows.UI.Notifications.ToastNotification]::new($x))",
        esc(summary),
        esc(body)
    );
    let _ = powershell(&script, false);
}

/// PowerShell ohne Fenster; `wait`: auf das Ende warten und Erfolg melden.
fn powershell(script: &str, wait: bool) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW);
    if wait {
        cmd.status().is_ok_and(|s| s.success())
    } else {
        cmd.spawn().is_ok()
    }
}

// ---------------------------------------------------------------------------------------------
// Ordner

fn local_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("tvjoe"))
}

pub fn jobs_dir() -> Option<PathBuf> {
    local_dir().map(|d| d.join("auftraege"))
}

pub fn status_dir() -> Option<PathBuf> {
    local_dir().map(|d| d.join("stand"))
}

/// Ordner des eingerichteten Dienstes.
fn install_dir() -> PathBuf {
    std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Program Files"))
        .join("tvjoe-dienst")
}

/// Einstellungen des eingerichteten Dienstes (lesbar auch für das Fenster).
pub fn installed_config() -> Option<ServiceConfig> {
    let text = std::fs::read_to_string(install_dir().join(CONFIG_FILE)).ok()?;
    toml::from_str(&text).ok()
}

/// Läuft der Dienst (Lebenszeichen im Standordner jünger als 20 s)?
pub fn alive() -> bool {
    status_dir()
        .and_then(|d| std::fs::read_to_string(d.join("lebt")).ok())
        .and_then(|s| s.trim().parse::<i64>().ok())
        .is_some_and(|t| ablage::now() - t < 20)
}

// ---------------------------------------------------------------------------------------------
// Dienst (läuft unter NT SERVICE\tvjoe)

windows_service::define_windows_service!(ffi_service_main, service_main);

/// `tvjoe-gui.exe --windows-dienst`: beim Dienststeuerungsmanager anmelden.
pub fn run_dispatcher() -> i32 {
    match windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("tvjoe: Dienst: {e}");
            1
        }
    }
}

fn service_main(_args: Vec<OsString>) {
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let Ok(handle) = service_control_handler::register(SERVICE_NAME, move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            s.store(true, Ordering::Relaxed);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    }) else {
        return;
    };
    let status = |state: ServiceState| ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::from_secs(10),
        process_id: None,
    };
    let _ = handle.set_service_status(status(ServiceState::Running));
    if let Some(cfg) = installed_config_here() {
        service_loop(&cfg, &stop);
    }
    let _ = handle.set_service_status(status(ServiceState::Stopped));
}

/// Einstellungen neben dem laufenden Programm (der Dienst liest nur seine eigene Kopie).
fn installed_config_here() -> Option<ServiceConfig> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    toml::from_str(&std::fs::read_to_string(dir.join(CONFIG_FILE)).ok()?).ok()
}

fn write_status(cfg: &ServiceConfig, st: &Status) {
    let dir = Path::new(&cfg.status_dir);
    let tmp = dir.join(format!("{}.tmp", st.id));
    if let Ok(text) = serde_json::to_vec(st) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join(format!("{}.json", st.id)));
        }
    }
}

/// Alle 5 s: Lebenszeichen, Aufträge lesen und prüfen, fällige starten, Weck-Zeitgeber setzen.
fn service_loop(cfg: &ServiceConfig, stop: &Arc<AtomicBool>) {
    let running: Arc<Mutex<HashSet<i64>>> = Arc::default();
    // Erledigte Aufträge (auch gescheiterte) nicht erneut starten, solange sie unverändert sind.
    let finished: Arc<Mutex<HashSet<(i64, i64, i64)>>> = Arc::default();
    let mut rejected: HashSet<(i64, String)> = HashSet::new();
    let mut wake = WakeTimer::new();
    while !stop.load(Ordering::Relaxed) {
        let _ = std::fs::write(
            Path::new(&cfg.status_dir).join("lebt"),
            ablage::now().to_string(),
        );
        let now = ablage::now();
        let mut next: Option<i64> = None;
        for (name, bytes) in read_jobs(Path::new(&cfg.jobs_dir)) {
            match auftrag::parse_job(&bytes, cfg, now) {
                Err(e) => {
                    // Nummer aus dem Dateinamen, damit das Fenster den Grund zuordnen kann.
                    let id = name.parse::<i64>().unwrap_or(0);
                    // Ein vorhandener Stand (z. B. eine gescheiterte Aufnahme mit Grund) bleibt.
                    let has_status = Path::new(&cfg.status_dir)
                        .join(format!("{id}.json"))
                        .exists();
                    if id > 0 && !has_status && rejected.insert((id, e.clone())) {
                        write_status(
                            cfg,
                            &Status {
                                id,
                                channel_id: 0,
                                sat: false,
                                channel: String::new(),
                                title: String::new(),
                                state: JobState::Rejected,
                                path: String::new(),
                                size: 0,
                                started: now,
                                error: Some(e),
                            },
                        );
                    }
                }
                Ok(job) if name != job.id.to_string() => {}
                Ok(job) => {
                    let is_running = running.lock().is_ok_and(|r| r.contains(&job.id));
                    let key = (job.id, job.start, job.stop);
                    let done = finished.lock().is_ok_and(|f| f.contains(&key));
                    if now >= job.start && !is_running && !done {
                        if let Ok(mut r) = running.lock() {
                            r.insert(job.id);
                        }
                        let (cfg, running, finished, stop) =
                            (cfg.clone(), running.clone(), finished.clone(), stop.clone());
                        std::thread::spawn(move || {
                            record_job(&job, &cfg, &stop);
                            if let Ok(mut f) = finished.lock() {
                                f.insert(key);
                            }
                            if let Ok(mut r) = running.lock() {
                                r.remove(&job.id);
                            }
                        });
                    } else if job.start > now {
                        next = Some(next.map_or(job.start, |n| n.min(job.start)));
                    }
                }
            }
        }
        // Zwei Minuten vor der nächsten Aufnahme wecken.
        wake.set(next.map(|t| t - 120));
        for _ in 0..5 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

/// `<Nummer>.json` im Auftragsordner (höchstens 4 KB je Datei, andere Dateien bleiben liegen).
fn read_jobs(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_stem()?.to_str()?.to_string();
            (p.extension()? == "json"
                && name.chars().all(|c| c.is_ascii_digit())
                && e.metadata().ok()?.len() <= auftrag::MAX_JOB_BYTES)
                .then_some(())?;
            Some((name, std::fs::read(&p).ok()?))
        })
        .collect()
}

/// Aufnahme eines geprüften Auftrags bis zum Ende, bis er aus dem Auftragsordner verschwindet
/// oder der Dienst anhält.
fn record_job(job: &Job, cfg: &ServiceConfig, stop: &Arc<AtomicBool>) {
    let dir = PathBuf::from(&cfg.records_dir);
    let now = ablage::now();
    let path = ablage::unique_path(&dir.join(ablage::show_name(
        &job.channel,
        &job.title,
        &crate::ablage::stamp(now),
        "ts",
    )));
    let mut st = Status {
        id: job.id,
        channel_id: job.channel_id,
        sat: job.sat,
        channel: job.channel.clone(),
        title: job.title.clone(),
        state: JobState::Loading,
        path: path.to_string_lossy().to_string(),
        size: 0,
        started: now,
        error: None,
    };
    let min_free = u64::from(cfg.min_free_gb) << 30;
    if crate::ablage::free_bytes(&dir).is_some_and(|f| f < min_free) {
        st.state = JobState::Failed;
        st.error = Some(tr("zu wenig Platz").into());
        write_status(cfg, &st);
        return;
    }
    write_status(cfg, &st);
    let _awake = crate::dienst::KeepAwake::new();
    let job_file = Path::new(&cfg.jobs_dir).join(format!("{}.json", job.id));
    let end = job.stop;
    let stop = stop.clone();
    let low_dir = dir.clone();
    let should_stop = move || {
        ablage::now() >= end
            || !job_file.exists()
            || stop.load(Ordering::Relaxed)
            || crate::ablage::free_bytes(&low_dir).is_some_and(|f| f < min_free)
    };
    let queue = u64::from(cfg.buffer_mb) << 19;
    let r = crate::dienst::record_stream(
        &job.url,
        job.max_bitrate,
        job.sat,
        &path,
        queue,
        &should_stop,
    );
    st.size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    match r {
        Ok(()) => st.state = JobState::Done,
        Err(e) => {
            st.state = if st.size > 0 {
                JobState::Partial
            } else {
                JobState::Failed
            };
            st.error = Some(e);
        }
    }
    write_status(cfg, &st);
}

/// Weck-Zeitgeber: holt den Rechner zur gesetzten Zeit aus dem Standby (sofern im Energieplan
/// erlaubt). Ein neuer Zeitpunkt ersetzt den alten; `None` hebt ihn auf.
struct WakeTimer {
    handle: *mut std::ffi::c_void,
    at: Option<i64>,
}

unsafe extern "system" {
    fn CreateWaitableTimerW(
        attrs: *mut std::ffi::c_void,
        manual_reset: i32,
        name: *const u16,
    ) -> *mut std::ffi::c_void;
    fn SetWaitableTimer(
        timer: *mut std::ffi::c_void,
        due: *const i64,
        period: i32,
        routine: *mut std::ffi::c_void,
        arg: *mut std::ffi::c_void,
        resume: i32,
    ) -> i32;
    fn CancelWaitableTimer(timer: *mut std::ffi::c_void) -> i32;
}

impl WakeTimer {
    fn new() -> Self {
        let handle = unsafe { CreateWaitableTimerW(std::ptr::null_mut(), 1, std::ptr::null()) };
        Self { handle, at: None }
    }

    fn set(&mut self, at: Option<i64>) {
        if self.handle.is_null() || self.at == at {
            return;
        }
        self.at = at;
        unsafe {
            match at {
                // Absolute Zeit als FILETIME (100 ns seit 1601).
                Some(t) if t > ablage::now() => {
                    let due = (t + 11_644_473_600) * 10_000_000;
                    SetWaitableTimer(
                        self.handle,
                        &due,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        1,
                    );
                }
                _ => {
                    CancelWaitableTimer(self.handle);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Fenster: Aufträge schreiben, Stand übernehmen

/// Aufträge an den Plan angleichen: je geplanter Aufnahme `<Nummer>.json`, verwaiste weg.
pub fn sync_jobs(app: &crate::app::App) {
    let Some(dir) = jobs_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let (pre, post) = {
        let s = app.settings();
        (
            i64::from(s.record_pre_min) * 60,
            i64::from(s.record_post_min) * 60,
        )
    };
    let Ok(ix) = crate::dienst::index() else { return };
    let mut keep: HashSet<String> = HashSet::new();
    for t in ix.timers() {
        let Some((_, url, bitrate)) = app.record_source(t.channel_id, t.sat) else {
            continue;
        };
        let job = Job {
            id: t.id,
            channel_id: t.channel_id,
            sat: t.sat,
            channel: t.channel.clone(),
            title: t.title.clone(),
            url,
            max_bitrate: bitrate,
            start: t.start - pre,
            stop: t.stop + post,
        };
        let name = format!("{}.json", t.id);
        keep.insert(name.clone());
        if let Ok(text) = serde_json::to_vec(&job) {
            let path = dir.join(&name);
            if std::fs::read(&path).ok().as_deref() != Some(text.as_slice()) {
                let tmp = dir.join(format!("{}.tmp", t.id));
                if std::fs::write(&tmp, &text).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".json") && !keep.contains(&name) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

thread_local! {
    /// Zuletzt übernommener Zustand je Auftrag (Meldungen nur bei Wechsel).
    static SEEN: std::cell::RefCell<HashMap<i64, JobState>> = Default::default();
}

/// Stand des Dienstes in die Ablage übernehmen; fertige Aufträge aus Plan und Auftragsordner
/// nehmen; Meldungen bei Beginn und Ende.
pub fn import_status(app: &crate::app::App) {
    let (Some(dir), Some(jobs)) = (status_dir(), jobs_dir()) else {
        return;
    };
    let Ok(ix) = crate::dienst::index() else { return };
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(st) = std::fs::read(&p)
            .ok()
            .and_then(|b| serde_json::from_slice::<Status>(&b).ok())
        else {
            continue;
        };
        let before = SEEN.with(|s| s.borrow().get(&st.id).copied());
        if before == Some(st.state) && st.state != JobState::Loading {
            continue;
        }
        if st.state == JobState::Rejected {
            if before != Some(JobState::Rejected) {
                app.message(&tf!(
                    "Aufnahmedienst hat einen Auftrag abgelehnt: {fehler}",
                    fehler = st.error.as_deref().unwrap_or("")
                ));
            }
            SEEN.with(|s| s.borrow_mut().insert(st.id, st.state));
            continue;
        }
        let source = if st.sat { "sat" } else { "tv" };
        let key = format!("{}@{}", st.channel_id, st.started);
        let id = match ix.find(source, &key).into_iter().find(|x| x.art == ablage::Art::Recording) {
            Some(x) => x.id,
            None => match ix.add(
                ablage::Art::Recording,
                source,
                &key,
                &st.title,
                &st.channel,
                Path::new(&st.path),
            ) {
                Ok(id) => id,
                Err(_) => continue,
            },
        };
        let size = std::fs::metadata(&st.path).map(|m| m.len()).unwrap_or(st.size);
        ix.set_size(id, size, size);
        let state = match st.state {
            JobState::Loading => ablage::State::Loading,
            JobState::Done => ablage::State::Done,
            JobState::Partial => ablage::State::Partial,
            _ => ablage::State::Failed,
        };
        ix.set_state(id, state, st.error.as_deref());
        if before != Some(st.state) {
            let what = tf!("{titel} – {sender}", titel = st.title, sender = st.channel);
            match st.state {
                JobState::Loading => notify(tr("Aufnahme läuft"), &what),
                JobState::Done => notify(tr("Aufnahme beendet"), &what),
                _ => notify(
                    tr("Aufnahme abgebrochen"),
                    &tf!(
                        "{was}: {fehler}",
                        was = what,
                        fehler = st.error.as_deref().unwrap_or("")
                    ),
                ),
            }
        }
        SEEN.with(|s| s.borrow_mut().insert(st.id, st.state));
        if st.state != JobState::Loading {
            ix.remove_timer(st.id);
            let _ = std::fs::remove_file(jobs.join(format!("{}.json", st.id)));
            let _ = std::fs::remove_file(&p);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Einrichten und Entfernen (einmal mit Administratorrechten)

/// UTF-16LE in Base64 (für `powershell -EncodedCommand`).
fn encode_command(script: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Skript mit Administratorrechten ausführen (eine UAC-Bestätigung) und auf das Ende warten.
fn run_elevated(script: &str) -> Result<(), String> {
    let inner = encode_command(script);
    let outer = format!(
        "$p = Start-Process powershell.exe -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList '-NoProfile','-NonInteractive','-EncodedCommand','{inner}'; exit $p.ExitCode"
    );
    if powershell(&outer, true) {
        Ok(())
    } else {
        Err(tr("abgebrochen oder vom System abgelehnt").into())
    }
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Dienst einrichten bzw. auf den Stand dieses Programms bringen.
pub fn install(s: &tvjoe_core::Settings) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let src = exe.parent().ok_or(tr("kein Programmordner"))?.to_path_buf();
    let (jobs, status) = (
        jobs_dir().ok_or(tr("kein LOCALAPPDATA"))?,
        status_dir().ok_or(tr("kein LOCALAPPDATA"))?,
    );
    let records = crate::ablage::storage_dir_for(s).join("records");
    for d in [&jobs, &status, &records] {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    // Nur das Gerät einer eingeschalteten SAT-Empfangsquelle ist erlaubt.
    let host = s
        .adapter(tvjoe_core::settings::ADAPTER_SAT)
        .filter(|a| a.enabled)
        .map(|a| a.host.trim().to_string())
        .unwrap_or_default();
    let cfg = ServiceConfig {
        version: tvjoe_core::VERSION.to_string(),
        jobs_dir: jobs.to_string_lossy().to_string(),
        status_dir: status.to_string_lossy().to_string(),
        records_dir: records.to_string_lossy().to_string(),
        satip_host: host.clone(),
        min_free_gb: s.min_free_gb,
        buffer_mb: s.buffer_mb,
    };
    let toml = toml::to_string(&cfg).map_err(|e| e.to_string())?;
    let dst = install_dir();
    let exe_name = exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let firewall = if host.is_empty() {
        String::new()
    } else {
        format!(
            "Remove-NetFirewallRule -DisplayName 'tvjoe Aufnahmedienst SAT>IP' -ErrorAction SilentlyContinue; \
             New-NetFirewallRule -DisplayName 'tvjoe Aufnahmedienst SAT>IP' -Direction Inbound -Protocol UDP \
             -Program (Join-Path $dst {exe}) -RemoteAddress {host} -Action Allow | Out-Null;",
            exe = ps_quote(&exe_name),
            host = ps_quote(&host),
        )
    };
    let script = format!(
        "$ErrorActionPreference = 'Stop'; $src = {src}; $dst = {dst}; \
         if (Get-Service {name} -ErrorAction SilentlyContinue) {{ Stop-Service {name} -Force -ErrorAction SilentlyContinue; sc.exe delete {name} | Out-Null; Start-Sleep -Seconds 2 }}; \
         New-Item -ItemType Directory -Force -Path $dst | Out-Null; \
         robocopy $src $dst /MIR /R:2 /W:1 /NFL /NDL /NJH /NJS | Out-Null; if ($LASTEXITCODE -ge 8) {{ exit 2 }}; \
         [IO.File]::WriteAllText((Join-Path $dst {cfgfile}), {toml}); \
         $bin = '\\\"' + (Join-Path $dst {exe}) + '\\\" --windows-dienst'; \
         sc.exe create {name} binPath= $bin start= auto obj= 'NT SERVICE\\{name}' DisplayName= 'tvjoe Aufnahmedienst' | Out-Null; \
         if ($LASTEXITCODE -ne 0) {{ exit 3 }}; \
         sc.exe description {name} 'Geplante Aufnahmen von tvjoe (Aufträge aus dem Benutzerordner)' | Out-Null; \
         icacls {jobs} /grant 'NT SERVICE\\{name}:(OI)(CI)RX' | Out-Null; \
         icacls {status} /grant 'NT SERVICE\\{name}:(OI)(CI)M' | Out-Null; \
         icacls {records} /grant 'NT SERVICE\\{name}:(OI)(CI)M' | Out-Null; \
         {firewall} \
         Start-Service {name}; exit 0",
        src = ps_quote(&src.to_string_lossy()),
        dst = ps_quote(&dst.to_string_lossy()),
        name = SERVICE_NAME,
        cfgfile = ps_quote(CONFIG_FILE),
        toml = ps_quote(&toml),
        exe = ps_quote(&exe_name),
        jobs = ps_quote(&jobs.to_string_lossy()),
        status = ps_quote(&status.to_string_lossy()),
        records = ps_quote(&records.to_string_lossy()),
    );
    run_elevated(&script)?;
    if installed_config().is_none() {
        return Err(tr("Einrichtung nicht abgeschlossen").into());
    }
    Ok(())
}

/// Dienst anhalten und entfernen, Rechte und Firewall-Regel zurücknehmen.
pub fn uninstall(s: &tvjoe_core::Settings) -> Result<(), String> {
    let records = crate::ablage::storage_dir_for(s).join("records");
    let dirs: Vec<String> = [jobs_dir(), status_dir(), Some(records)]
        .into_iter()
        .flatten()
        .map(|d| ps_quote(&d.to_string_lossy()))
        .collect();
    let script = format!(
        "$dst = {dst}; Stop-Service {name} -Force -ErrorAction SilentlyContinue; sc.exe delete {name} | Out-Null; \
         Remove-NetFirewallRule -DisplayName 'tvjoe Aufnahmedienst SAT>IP' -ErrorAction SilentlyContinue; \
         foreach ($d in @({dirs})) {{ icacls $d /remove 'NT SERVICE\\{name}' | Out-Null }}; \
         Start-Sleep -Seconds 2; Remove-Item -Recurse -Force $dst -ErrorAction SilentlyContinue; exit 0",
        dst = ps_quote(&install_dir().to_string_lossy()),
        name = SERVICE_NAME,
        dirs = dirs.join(","),
    );
    run_elevated(&script)
}
