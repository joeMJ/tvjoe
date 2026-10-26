//! Mitschnitt eines Streams mit einer unsichtbaren mpv-Instanz (`stream-record`): Internet-TV
//! (live) und HLS-Beiträge aus Mediathek und Jellyfin. mpv schreibt die Pakete so, wie sie
//! aus dem Demuxer kommen, ohne neu zu kodieren; das Format folgt der Dateiendung (`.ts`).
//!
//! Abrufbeiträge laufen ohne Zeittakt (`untimed`) und ohne Bild- und Tondekodierung, also so
//! schnell, wie das Netz liefert. Live-Sender laufen im Takt der Sendung.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use libmpv2::Mpv;
use tvjoe_core::ablage::Progress;

/// Zahl hinter `"name":` in einer JSON-Ausgabe von mpv.
fn json_number(text: &str, name: &str) -> Option<f64> {
    let key = format!("\"{name}\":");
    let rest = &text[text.find(&key)? + key.len()..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].trim().parse().ok()
}

/// Zwischendatei für mpv: Endung muss bleiben (`Film.part.ts`), sonst kennt mpv das Format nicht.
pub fn part_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| "ts".into());
    path.with_file_name(format!("{stem}.part.{ext}"))
}

/// Was mitgeschnitten wird.
pub struct Source<'a> {
    pub url: &'a str,
    /// Zusätzliche Kopfzeile „Name: Wert“ (Jellyfin-Anmeldung).
    pub header: Option<&'a str>,
    /// HLS-Stufe begrenzen (bit/s).
    pub max_bitrate: Option<u64>,
    /// Live-Sender: im Takt der Sendung, bis `progress.cancel`.
    pub live: bool,
}

/// Schneidet `src` nach `path` mit (blockiert bis zum Ende, für einen eigenen Thread).
/// `progress.done` sind die geschriebenen Bytes, `progress.total` bei Abrufbeiträgen die Dauer
/// in Millisekunden und `progress` der Anteil über die Position (siehe [`position_ms`]).
pub fn run(src: &Source<'_>, path: &Path, progress: &Progress, position_ms: &std::sync::atomic::AtomicU64) -> Result<(), String> {
    use libmpv2::events::Event;
    use libmpv2::mpv_end_file_reason::{Eof, Error};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = part_path(path);
    let _ = std::fs::remove_file(&part);
    let live = src.live;
    let mpv = Mpv::with_initializer(|i| {
        i.set_option("vo", "null")?;
        i.set_option("ao", "null")?;
        i.set_option("terminal", false)?;
        i.set_option("input-default-bindings", false)?;
        i.set_option("hwdec", "no")?;
        if !live {
            // So schnell wie das Netz: kein Takt, nichts dekodieren.
            i.set_option("untimed", true)?;
            i.set_option("ao-null-untimed", true)?;
            i.set_option("vd-lavc-skipframe", "all")?;
            i.set_option("ad-lavc-downmix", false)?;
        }
        i.set_option("cache", "yes")?;
        Ok(())
    })
    .map_err(|e| format!("mpv: {e:?}"))?;
    let _ = mpv.disable_deprecated_events();
    if let Some(h) = src.header {
        let _ = mpv.command("change-list", &["http-header-fields", "append", h]);
    }
    if let Some(b) = src.max_bitrate {
        let _ = mpv.set_property("hls-bitrate", b.to_string().as_str());
    }
    mpv.set_property("stream-record", part.to_string_lossy().as_ref())
        .map_err(|e| format!("mpv stream-record: {e:?}"))?;
    mpv.command("loadfile", &[src.url])
        .map_err(|e| format!("mpv loadfile: {e:?}"))?;
    let mut result: Result<(), String> = Err("abgebrochen".into());
    loop {
        if progress.cancelled() {
            // Live-Aufnahme regulär beenden: Datei bleibt gültig.
            result = if live { Ok(()) } else { Err("abgebrochen".into()) };
            let _ = mpv.command("stop", &[]);
            break;
        }
        match mpv.wait_event(0.5) {
            Some(Ok(Event::EndFile(r))) if r == Eof => {
                result = Ok(());
                break;
            }
            Some(Ok(Event::EndFile(r))) if r == Error => {
                result = Err("Stream ließ sich nicht öffnen oder brach ab".into());
                break;
            }
            Some(Ok(Event::Shutdown)) => break,
            _ => {}
        }
        if let Ok(d) = mpv.get_property::<f64>("duration") {
            progress.total.store((d * 1000.0) as u64, Ordering::Relaxed);
        }
        if let Ok(m) = std::fs::metadata(&part) {
            progress.done.store(m.len(), Ordering::Relaxed);
        }
        // Mitgeschnitten wird beim Lesen des Demuxers. Ohne Dekodierung erreicht die Wiedergabe
        // das Ende nicht von selbst; fertig ist es, wenn der Demuxer am Ende steht und ruht.
        if let Ok(state) = mpv.get_property::<String>("demuxer-cache-state") {
            if let Some(end) = json_number(&state, "cache-end") {
                position_ms.store((end * 1000.0) as u64, Ordering::Relaxed);
            }
            if !live && state.contains("\"eof\":true") && state.contains("\"idle\":true") {
                result = Ok(());
                break;
            }
        }
    }
    // stream-record schließt die Datei beim Beenden der Instanz.
    let _ = mpv.set_property("stream-record", "");
    drop(mpv);
    std::thread::sleep(Duration::from_millis(100));
    if result.is_ok() {
        std::fs::rename(&part, path).map_err(|e| e.to_string())?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Netztest: `TVJOE_CAPTURE_URL=… cargo test -p tvjoe-gui capture -- --ignored`.
    #[test]
    #[ignore]
    fn capture_vod() {
        let Ok(url) = std::env::var("TVJOE_CAPTURE_URL") else { return };
        let dir = std::env::temp_dir().join("tvjoe-capture-test");
        let path = dir.join("probe.ts");
        let progress = Progress::default();
        let pos = std::sync::atomic::AtomicU64::new(0);
        let t = std::time::Instant::now();
        let r = run(
            &Source {
                url: &url,
                header: None,
                max_bitrate: None,
                live: false,
            },
            &path,
            &progress,
            &pos,
        );
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        eprintln!(
            "Ergebnis {r:?}, {size} Bytes, Dauer {} ms, Position {} ms, {:.1} s gebraucht",
            progress.total.load(Ordering::Relaxed),
            pos.load(Ordering::Relaxed),
            t.elapsed().as_secs_f64()
        );
        assert!(r.is_ok() && size > 0);
    }
}
