//! Aufträge an den Windows-Aufnahmedienst und sein Stand (Nutzer, 2026-10-27).
//!
//! tvjoe schreibt je geplanter Aufnahme eine JSON-Datei in den Auftragsordner des Benutzers;
//! der Dienst (eigenes Dienstkonto, nur Leserecht dort) prüft jeden Auftrag vollständig, bevor
//! er etwas tut, und meldet den Stand als JSON im Standordner. Ein Auftrag kann nur bewirken,
//! dass eine Sendung in den Aufnahmeordner aufgenommen wird.

use serde::{Deserialize, Serialize};

use crate::{tf, tr};

/// Größte Auftragsdatei (Bytes).
pub const MAX_JOB_BYTES: u64 = 4096;
/// Längste Aufnahme (Sekunden).
pub const MAX_DURATION: i64 = 12 * 3600;
/// Längste Texte (Zeichen).
const MAX_TEXT: usize = 200;

/// Auftrag: eine Sendung aufnehmen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    /// Nummer der Planung im Fenster.
    pub id: i64,
    /// Sender (Kennung in der Senderliste, Anzeigename) und SAT-Liste ja/nein.
    pub channel_id: u64,
    pub sat: bool,
    pub channel: String,
    pub title: String,
    pub url: String,
    /// HLS-Stufe begrenzen (bit/s).
    pub max_bitrate: Option<u64>,
    /// Beginn und Ende einschließlich Vor- und Nachlauf (Sekunden seit 1970).
    pub start: i64,
    pub stop: i64,
}

/// Zustand einer Aufnahme des Dienstes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Loading,
    Done,
    Partial,
    Failed,
    /// Auftrag verworfen (Prüfung fehlgeschlagen).
    Rejected,
}

/// Stand eines Auftrags, vom Dienst geschrieben.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub id: i64,
    pub channel_id: u64,
    pub sat: bool,
    pub channel: String,
    pub title: String,
    pub state: JobState,
    /// Aufnahmedatei (bei verworfenen Aufträgen leer).
    pub path: String,
    pub size: u64,
    pub started: i64,
    pub error: Option<String>,
}

/// Einstellungen des Dienstes, von der Einrichtung (mit Administratorrechten) neben das
/// Programm geschrieben.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// Programmversion bei der Einrichtung (für „Dienst aktualisieren“).
    pub version: String,
    pub jobs_dir: String,
    pub status_dir: String,
    pub records_dir: String,
    /// Einziges erlaubtes SAT>IP-Gerät (Adresse ohne Anschluss); leer = kein SAT.
    pub satip_host: String,
    pub min_free_gb: u32,
    pub buffer_mb: u32,
}

/// Text ohne Steuerzeichen, gekürzt.
fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX_TEXT)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Auftrag lesen und vollständig prüfen. Gibt den bereinigten Auftrag zurück oder den Grund
/// der Ablehnung. `now`: jetzt (Sekunden seit 1970).
pub fn parse_job(bytes: &[u8], cfg: &ServiceConfig, now: i64) -> Result<Job, String> {
    if bytes.len() as u64 > MAX_JOB_BYTES {
        return Err(tr("Auftrag zu groß").into());
    }
    let mut job: Job =
        serde_json::from_slice(bytes).map_err(|e| tf!("Auftrag nicht lesbar: {error}", error = e))?;
    if job.id <= 0 {
        return Err(tr("ungültige Nummer").into());
    }
    if job.stop <= job.start {
        return Err(tr("Ende vor Beginn").into());
    }
    if job.stop - job.start > MAX_DURATION {
        return Err(tr("länger als zwölf Stunden").into());
    }
    if job.stop <= now {
        return Err(tr("schon vorbei").into());
    }
    job.channel = clean(&job.channel);
    job.title = clean(&job.title);
    if job.channel.is_empty() {
        return Err(tr("kein Sender").into());
    }
    check_url(&job.url, job.sat, &cfg.satip_host)?;
    Ok(job)
}

/// Nur SAT>IP zum eingetragenen Gerät oder HTTP(S); keine Dateien, keine anderen Schemata,
/// keine Leer- oder Steuerzeichen.
fn check_url(url: &str, sat: bool, satip_host: &str) -> Result<(), String> {
    if url.len() > 2048 || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(tr("ungültige Adresse").into());
    }
    if sat {
        if satip_host.is_empty() {
            return Err(tr("kein SAT>IP-Gerät eingerichtet").into());
        }
        let (host, _) = crate::satip::split_url(url).ok_or(tr("keine SAT>IP-Adresse"))?;
        if !host.eq_ignore_ascii_case(satip_host) {
            return Err(tr("anderes SAT>IP-Gerät als eingerichtet").into());
        }
        Ok(())
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(tr("nur http(s) erlaubt").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ServiceConfig {
        ServiceConfig {
            version: "0".into(),
            jobs_dir: String::new(),
            status_dir: String::new(),
            records_dir: String::new(),
            satip_host: "10.0.0.5".into(),
            min_free_gb: 5,
            buffer_mb: 512,
        }
    }

    fn job(url: &str, sat: bool) -> String {
        format!(
            r#"{{"id":3,"channel_id":7,"sat":{sat},"channel":"Das Erste\u0007 HD","title":"Tatort","url":"{url}","max_bitrate":null,"start":1000,"stop":2000}}"#
        )
    }

    #[test]
    fn accepts_valid_jobs_and_cleans_texts() {
        let j = parse_job(
            job("rtsp://10.0.0.5:554/?src=1&freq=11494&pol=h&pids=0,18", true).as_bytes(),
            &cfg(),
            500,
        )
        .unwrap();
        assert_eq!(j.channel, "Das Erste HD");
        assert!(parse_job(job("https://x.de/a.m3u8", false).as_bytes(), &cfg(), 500).is_ok());
    }

    #[test]
    fn rejects_everything_else() {
        let c = cfg();
        for (url, sat) in [
            ("file:///C:/Windows/x", false),
            ("rtsp://10.0.0.6:554/?src=1&freq=1", true),
            ("https://x.de/a b", false),
            ("\\\\server\\share", false),
        ] {
            assert!(parse_job(job(url, sat).as_bytes(), &c, 500).is_err(), "{url}");
        }
        // Unbekanntes Feld, zu lang, vorbei, Ende vor Beginn.
        let extra = job("https://x.de/a", false).replace("}", r#","path":"C:\\x"}"#);
        assert!(parse_job(extra.as_bytes(), &c, 500).is_err());
        let long = job("https://x.de/a", false).replace("2000", "99999");
        assert!(parse_job(long.as_bytes(), &c, 500).is_err());
        assert!(parse_job(job("https://x.de/a", false).as_bytes(), &c, 3000).is_err());
        let back = job("https://x.de/a", false).replace("2000", "900");
        assert!(parse_job(back.as_bytes(), &c, 500).is_err());
        assert!(parse_job(&vec![b' '; 5000], &c, 500).is_err());
    }
}
