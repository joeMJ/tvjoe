//! Diagnoseprotokoll der Oberfläche (Nutzer, 2026-10-28): mit `TVJOE_DIAGNOSE=1` (oder
//! `tvjoe --diagnose`) entsteht `~/.cache/tvjoe/diagnose-<Datum>-<Uhrzeit>.log` mit
//! - Umgebung: Version, System, Sitzung, GTK/libadwaita/libmpv/ffmpeg, OpenGL, Einstellungen;
//! - allen Schritten von tvjoe (Protokollzeilen, Ladebefehle an den Player, Quellenwechsel);
//! - dem vollständigen Protokoll von libmpv und ffmpeg (Stufe debug) und jedem mpv-Ereignis;
//! - SAT>IP/RTSP und den Weitergaben (aus dem Kern);
//! - Zustandsbildern alle 2 s aus einem eigenen Thread (läuft auch, wenn die Hauptschleife
//!   hängt): Threads des Prozesses mit Zustand, CPU-Zeit und Wartestelle, offene
//!   Netzverbindungen, Zustand von libmpv; dazu ein Lebenszeichen der Hauptschleife.
//! Zugangsschlüssel werden entfernt (`tvjoe_core::diag::redact`).

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gtk::glib;

static FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

/// Eingeschaltet (Datei offen)?
pub fn on() -> bool {
    FILE.get().is_some()
}

/// Beim Start: Datei anlegen, wenn `TVJOE_DIAGNOSE` gesetzt ist; gibt ihren Ort zurück.
pub fn init() -> Option<PathBuf> {
    std::env::var_os("TVJOE_DIAGNOSE")?;
    let dir = tvjoe_core::paths::cache_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = glib::DateTime::now_local()
        .ok()
        .and_then(|d| d.format("%Y%m%d-%H%M%S").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "jetzt".into());
    let path = dir.join(format!("diagnose-{stamp}.log"));
    let file = std::fs::File::create(&path).ok()?;
    let _ = START.set(Instant::now());
    let _ = FILE.set(Mutex::new(file));
    tvjoe_core::diag::set_sink(write);
    environment();
    Some(path)
}

/// Eine Zeile: Uhrzeit, Millisekunden seit dem Start, Bereich, Text (Schlüssel entfernt).
pub fn line(cat: &str, msg: &str) {
    if on() {
        write(cat, &tvjoe_core::diag::redact(msg));
    }
}

fn write(cat: &str, msg: &str) {
    let Some(f) = FILE.get() else { return };
    let ms = START.get().map_or(0.0, |s| s.elapsed().as_secs_f64() * 1e3);
    let clock = glib::DateTime::now_local()
        .ok()
        .and_then(|d| {
            d.format("%H:%M:%S")
                .ok()
                .map(|s| format!("{s}.{:03}", d.microsecond() / 1000))
        })
        .unwrap_or_default();
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("?");
    let mut text = String::new();
    for (i, l) in msg.lines().enumerate() {
        if i == 0 {
            text.push_str(&format!("{clock} {ms:10.1} [{name}] [{cat}] {l}\n"));
        } else {
            text.push_str(&format!("{clock} {ms:10.1} [{name}] [{cat}]   {l}\n"));
        }
    }
    if text.is_empty() {
        text = format!("{clock} {ms:10.1} [{name}] [{cat}]\n");
    }
    if let Ok(mut f) = f.lock() {
        let _ = f.write_all(text.as_bytes());
        let _ = f.flush();
    }
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default().trim().to_string()
}

/// Umgebung: Version, System, Sitzung, Bibliotheken.
fn environment() {
    let os = read("/etc/os-release")
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string()))
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    line("umgebung", &format!("tvjoe {} ({})", tvjoe_core::VERSION, std::env::consts::ARCH));
    line("umgebung", &format!("System: {os}, Kernel {}", read("/proc/sys/kernel/osrelease")));
    line(
        "umgebung",
        &format!("Programm: {:?}, Aufruf: {:?}", std::env::current_exe().ok(), std::env::args().collect::<Vec<_>>()),
    );
    for k in [
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "GDK_BACKEND",
        "GSK_RENDERER",
        "GDK_GL",
        "LIBVA_DRIVER_NAME",
        "LANG",
        "TVJOE_MPV",
        "TVJOE_TRACE",
    ] {
        line("umgebung", &format!("{k}={}", std::env::var(k).unwrap_or_else(|_| "–".into())));
    }
    let cpu = read("/proc/cpuinfo")
        .lines()
        .find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string()))
        .unwrap_or_default();
    line("umgebung", &format!("Prozessor: {cpu}, Kerne: {:?}", std::thread::available_parallelism().ok()));
    for l in read("/proc/meminfo").lines().take(3) {
        line("umgebung", l);
    }
}

/// Einstellungen (ohne Zugangsschlüssel – die liegen in eigenen Dateien) und die Fassungen
/// von GTK und libadwaita (erst nach dem Start von GTK abfragbar).
pub fn settings(s: &tvjoe_core::Settings) {
    if !on() {
        return;
    }
    line(
        "umgebung",
        &format!(
            "GTK {}.{}.{}, libadwaita {}.{}.{}",
            gtk::major_version(),
            gtk::minor_version(),
            gtk::micro_version(),
            adw::major_version(),
            adw::minor_version(),
            adw::micro_version()
        ),
    );
    line("einstellungen", &tvjoe_core::diag::settings_text(s));
}

/// Zustandsbilder alle 2 s aus einem eigenen Thread (Threads, Netzverbindungen, libmpv) und
/// ein Lebenszeichen der Hauptschleife.
pub fn start_snapshots(mpv: &'static libmpv2::Mpv, renders: impl Fn() -> u64 + 'static) {
    if !on() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("diagnose".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(2));
                line("zustand", &threads());
                line("zustand", &sockets());
                line("zustand", "libmpv-Zustand wird gelesen …");
                line("zustand", &mpv_state(mpv));
            }
        });
    glib::timeout_add_local(Duration::from_secs(2), move || {
        line("hauptschleife", &format!("lebt, bisher {} Bilder gezeichnet", renders()));
        glib::ControlFlow::Continue
    });
}

/// Threads des Prozesses: Kennung, Name, Zustand, CPU-Zeit (Takte), Wartestelle im Kernel.
fn threads() -> String {
    let mut out = String::from("Threads:");
    let Ok(dir) = std::fs::read_dir("/proc/self/task") else {
        return out + " (nicht lesbar)";
    };
    let mut tids: Vec<String> = dir
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .collect();
    tids.sort_by_key(|t| t.parse::<u64>().unwrap_or(0));
    for tid in tids {
        let base = format!("/proc/self/task/{tid}");
        let comm = read(&format!("{base}/comm"));
        let stat = read(&format!("{base}/stat"));
        // Nach dem Namen in Klammern: Zustand, …, utime (14), stime (15).
        let after = stat.rsplit_once(')').map(|(_, r)| r.trim().to_string()).unwrap_or_default();
        let f: Vec<&str> = after.split_whitespace().collect();
        let state = f.first().copied().unwrap_or("?");
        let utime = f.get(11).copied().unwrap_or("?");
        let stime = f.get(12).copied().unwrap_or("?");
        let wchan = read(&format!("{base}/wchan"));
        out.push_str(&format!("\n{tid} {comm:<16} {state} cpu {utime}+{stime} warte {wchan}"));
    }
    out
}

/// Offene TCP-Verbindungen dieses Prozesses (aus /proc/self/net/tcp, tcp6).
fn sockets() -> String {
    let mut inodes = std::collections::HashSet::new();
    if let Ok(fds) = std::fs::read_dir("/proc/self/fd") {
        for fd in fds.flatten() {
            if let Ok(link) = std::fs::read_link(fd.path()) {
                let l = link.to_string_lossy();
                if let Some(n) = l.strip_prefix("socket:[").and_then(|r| r.strip_suffix(']')) {
                    inodes.insert(n.to_string());
                }
            }
        }
    }
    let states = [
        "?", "ESTABLISHED", "SYN_SENT", "SYN_RECV", "FIN_WAIT1", "FIN_WAIT2", "TIME_WAIT", "CLOSE",
        "CLOSE_WAIT", "LAST_ACK", "LISTEN", "CLOSING",
    ];
    let mut out = format!("Netzverbindungen ({} Sockets offen):", inodes.len());
    for (file, v6) in [("/proc/self/net/tcp", false), ("/proc/self/net/tcp6", true)] {
        for l in read(file).lines().skip(1) {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 10 || !inodes.contains(f[9]) {
                continue;
            }
            let st = usize::from_str_radix(f[3], 16).unwrap_or(0);
            out.push_str(&format!(
                "\n{} {} → {} {}",
                if v6 { "tcp6" } else { "tcp" },
                addr(f[1], v6),
                addr(f[2], v6),
                states.get(st).unwrap_or(&"?")
            ));
        }
    }
    out
}

fn addr(hex: &str, v6: bool) -> String {
    let Some((ip, port)) = hex.split_once(':') else { return hex.into() };
    let port = u16::from_str_radix(port, 16).unwrap_or(0);
    if !v6 && ip.len() == 8 {
        let n = u32::from_str_radix(ip, 16).unwrap_or(0).to_le_bytes();
        return format!("{}.{}.{}.{}:{port}", n[0], n[1], n[2], n[3]);
    }
    format!("[{ip}]:{port}")
}

/// Zustand von libmpv (jede Eigenschaft einzeln; hängt eine, sieht man es an der letzten Zeile).
fn mpv_state(mpv: &libmpv2::Mpv) -> String {
    let mut out = String::from("libmpv:");
    for p in [
        "path",
        "stream-open-filename",
        "file-format",
        "current-demuxer",
        "idle-active",
        "core-idle",
        "eof-reached",
        "seeking",
        "paused-for-cache",
        "cache-buffering-state",
        "demuxer-cache-duration",
        "demuxer-cache-idle",
        "time-pos",
        "estimated-frame-number",
        "frame-drop-count",
        "decoder-frame-drop-count",
        "vo-configured",
        "video-codec",
        "hwdec-current",
        "video-params/w",
        "video-params/h",
        "video-params/pixelformat",
        "audio-codec-name",
        "current-ao",
        "deinterlace",
        "hwdec",
    ] {
        let v = mpv
            .get_property::<String>(p)
            .unwrap_or_else(|e| format!("– ({e:?})"));
        out.push_str(&format!("\n{p} = {v}"));
    }
    out
}
