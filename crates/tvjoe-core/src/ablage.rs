//! Ablage auf der Platte: Verzeichnis (SQLite) der heruntergeladenen, zwischengespeicherten und
//! aufgenommenen Sendungen, geplante Aufnahmen, Dateinamen und Herunterladen mit Fortsetzen.
//!
//! Heruntergeladenes und Aufnahmen bleiben, bis der Nutzer sie löscht; Zwischengespeichertes
//! löscht tvjoe wieder (zuletzt benutzt zuerst behalten), sobald die Grenze erreicht ist.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

/// Art eines Eintrags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Art {
    Download,
    Cache,
    Recording,
}

impl Art {
    fn as_str(self) -> &'static str {
        match self {
            Art::Download => "download",
            Art::Cache => "cache",
            Art::Recording => "recording",
        }
    }

    fn parse(s: &str) -> Art {
        match s {
            "cache" => Art::Cache,
            "recording" => Art::Recording,
            _ => Art::Download,
        }
    }
}

/// Zustand eines Eintrags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Wartet auf einen freien Platz in der Warteschlange.
    Queued,
    /// Wird geladen bzw. aufgenommen.
    Loading,
    /// Vollständig auf der Platte.
    Done,
    /// Abgebrochen (Netz, Platz, Programmende); ein Download lässt sich fortsetzen.
    Partial,
    Failed,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Loading => "loading",
            State::Done => "done",
            State::Partial => "partial",
            State::Failed => "failed",
        }
    }

    fn parse(s: &str) -> State {
        match s {
            "queued" => State::Queued,
            "loading" => State::Loading,
            "done" => State::Done,
            "partial" => State::Partial,
            _ => State::Failed,
        }
    }
}

/// Ein Eintrag der Ablage.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: i64,
    pub art: Art,
    /// Herkunft: `oer`, `jellyfin`, `tv`, `sat`.
    pub source: String,
    /// Kennung in der Herkunft (Film-ID, Jellyfin-ID, Sender und Startzeit).
    pub key: String,
    pub title: String,
    pub subtitle: String,
    pub path: PathBuf,
    /// Bisher geschriebene und erwartete Größe (Bytes, 0 = unbekannt).
    pub size: u64,
    pub total: u64,
    pub state: State,
    pub error: Option<String>,
    /// Angelegt und zuletzt benutzt (Sekunden seit 1970).
    pub created: i64,
    pub used: i64,
}

/// Geplante Aufnahme aus dem Programm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timer {
    pub id: i64,
    /// Sender in der Fernseh- bzw. SAT-Liste.
    pub channel_id: u64,
    pub sat: bool,
    pub channel: String,
    pub title: String,
    /// Sendezeit laut Programm (Sekunden seit 1970), ohne Vor- und Nachlauf.
    pub start: i64,
    pub stop: i64,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS entries(
    id INTEGER PRIMARY KEY, art TEXT NOT NULL, source TEXT NOT NULL, key TEXT NOT NULL,
    title TEXT NOT NULL, subtitle TEXT NOT NULL, path TEXT NOT NULL, size INTEGER NOT NULL,
    total INTEGER NOT NULL, state TEXT NOT NULL, error TEXT, created INTEGER NOT NULL,
    used INTEGER NOT NULL, UNIQUE(art, source, key));
CREATE TABLE IF NOT EXISTS timers(
    id INTEGER PRIMARY KEY, channel_id INTEGER NOT NULL, sat INTEGER NOT NULL,
    channel TEXT NOT NULL, title TEXT NOT NULL, start INTEGER NOT NULL, stop INTEGER NOT NULL,
    UNIQUE(channel_id, sat, start));
";

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Verzeichnis der Ablage.
pub struct Index {
    conn: Connection,
}

fn sql(e: rusqlite::Error) -> String {
    e.to_string()
}

fn entry_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: r.get(0)?,
        art: Art::parse(&r.get::<_, String>(1)?),
        source: r.get(2)?,
        key: r.get(3)?,
        title: r.get(4)?,
        subtitle: r.get(5)?,
        path: PathBuf::from(r.get::<_, String>(6)?),
        size: r.get::<_, i64>(7)? as u64,
        total: r.get::<_, i64>(8)? as u64,
        state: State::parse(&r.get::<_, String>(9)?),
        error: r.get(10)?,
        created: r.get(11)?,
        used: r.get(12)?,
    })
}

const ENTRY_COLS: &str =
    "id, art, source, key, title, subtitle, path, size, total, state, error, created, used";

impl Index {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(sql)?;
        conn.execute_batch(SCHEMA).map_err(sql)?;
        Ok(Self { conn })
    }

    pub fn in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("SQLite im Speicher");
        conn.execute_batch(SCHEMA).expect("Schema");
        Self { conn }
    }

    /// Legt einen Eintrag an oder setzt einen vorhandenen (gleiche Art, Herkunft, Kennung) zurück
    /// auf „wartet“; gibt die Kennung zurück.
    pub fn add(
        &self,
        art: Art,
        source: &str,
        key: &str,
        title: &str,
        subtitle: &str,
        path: &Path,
    ) -> Result<i64, String> {
        let t = now();
        self.conn
            .execute(
                "INSERT INTO entries(art, source, key, title, subtitle, path, size, total, state,
                    error, created, used) VALUES(?1, ?2, ?3, ?4, ?5, ?6, 0, 0, 'queued', NULL, ?7, ?7)
                 ON CONFLICT(art, source, key) DO UPDATE SET title = ?4, subtitle = ?5,
                    path = ?6, state = 'queued', error = NULL, used = ?7",
                params![
                    art.as_str(),
                    source,
                    key,
                    title,
                    subtitle,
                    path.to_string_lossy(),
                    t
                ],
            )
            .map_err(sql)?;
        self.conn
            .query_row(
                "SELECT id FROM entries WHERE art = ?1 AND source = ?2 AND key = ?3",
                params![art.as_str(), source, key],
                |r| r.get(0),
            )
            .map_err(sql)
    }

    pub fn get(&self, id: i64) -> Option<Entry> {
        self.conn
            .query_row(
                &format!("SELECT {ENTRY_COLS} FROM entries WHERE id = ?1"),
                [id],
                entry_row,
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Alle Einträge zu einer Kennung (Download und Zwischenspeicher derselben Sendung).
    pub fn find(&self, source: &str, key: &str) -> Vec<Entry> {
        self.query(
            &format!("SELECT {ENTRY_COLS} FROM entries WHERE source = ?1 AND key = ?2"),
            params![source, key],
        )
    }

    /// Alle Einträge, neueste zuerst.
    pub fn all(&self) -> Vec<Entry> {
        self.query(
            &format!("SELECT {ENTRY_COLS} FROM entries ORDER BY created DESC"),
            [],
        )
    }

    fn query(&self, q: &str, p: impl rusqlite::Params) -> Vec<Entry> {
        let Ok(mut st) = self.conn.prepare(q) else {
            return Vec::new();
        };
        st.query_map(p, entry_row)
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn set_state(&self, id: i64, state: State, error: Option<&str>) {
        let _ = self.conn.execute(
            "UPDATE entries SET state = ?2, error = ?3 WHERE id = ?1",
            params![id, state.as_str(), error],
        );
    }

    pub fn set_size(&self, id: i64, size: u64, total: u64) {
        let _ = self.conn.execute(
            "UPDATE entries SET size = ?2, total = ?3 WHERE id = ?1",
            params![id, size as i64, total as i64],
        );
    }

    /// Endgültige Datei (Endung erst nach dem Laden bekannt, z. B. Jellyfin-Original).
    pub fn set_path(&self, id: i64, path: &Path) {
        let _ = self.conn.execute(
            "UPDATE entries SET path = ?2 WHERE id = ?1",
            params![id, path.to_string_lossy()],
        );
    }

    /// Beim Abspielen aus der Ablage: zuletzt benutzt (Zwischenspeicher behält das zuerst).
    pub fn touch(&self, id: i64) {
        let _ = self
            .conn
            .execute("UPDATE entries SET used = ?2 WHERE id = ?1", params![id, now()]);
    }

    pub fn remove(&self, id: i64) {
        let _ = self.conn.execute("DELETE FROM entries WHERE id = ?1", [id]);
    }

    /// Belegung des Zwischenspeichers (Bytes).
    pub fn cache_bytes(&self) -> u64 {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(size), 0) FROM entries WHERE art = 'cache'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0) as u64
    }

    /// Zwischengespeichertes, das weichen muss, damit `incoming` Bytes unter `limit` passen:
    /// am längsten nicht benutzt zuerst; laufende Einträge bleiben.
    pub fn evictable(&self, limit: u64, incoming: u64) -> Vec<Entry> {
        let mut used = self.cache_bytes() + incoming;
        let mut out = Vec::new();
        for e in self.query(
            &format!(
                "SELECT {ENTRY_COLS} FROM entries WHERE art = 'cache' AND state != 'loading'
                 ORDER BY used ASC"
            ),
            [],
        ) {
            if used <= limit {
                break;
            }
            used = used.saturating_sub(e.size);
            out.push(e);
        }
        out
    }

    /// Laufende oder wartende Einträge aus einer früheren Sitzung gelten als abgebrochen.
    pub fn mark_interrupted(&self) {
        let _ = self.conn.execute(
            "UPDATE entries SET state = 'partial' WHERE state IN ('loading', 'queued')",
            [],
        );
    }

    // -----------------------------------------------------------------------------------------
    // Geplante Aufnahmen

    pub fn add_timer(&self, t: &Timer) -> Result<i64, String> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO timers(channel_id, sat, channel, title, start, stop)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    t.channel_id as i64,
                    t.sat,
                    t.channel,
                    t.title,
                    t.start,
                    t.stop
                ],
            )
            .map_err(sql)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn remove_timer(&self, id: i64) {
        let _ = self.conn.execute("DELETE FROM timers WHERE id = ?1", [id]);
    }

    pub fn timers(&self) -> Vec<Timer> {
        let Ok(mut st) = self.conn.prepare(
            "SELECT id, channel_id, sat, channel, title, start, stop FROM timers ORDER BY start",
        ) else {
            return Vec::new();
        };
        st.query_map([], |r| {
            Ok(Timer {
                id: r.get(0)?,
                channel_id: r.get::<_, i64>(1)? as u64,
                sat: r.get(2)?,
                channel: r.get(3)?,
                title: r.get(4)?,
                start: r.get(5)?,
                stop: r.get(6)?,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    /// Geplante Aufnahme einer Sendung (Sender und Beginn).
    pub fn timer_for(&self, channel_id: u64, sat: bool, start: i64) -> Option<Timer> {
        self.timers()
            .into_iter()
            .find(|t| t.channel_id == channel_id && t.sat == sat && t.start == start)
    }
}

// ---------------------------------------------------------------------------------------------
// Dateinamen

/// Zeichen, die in Dateinamen unter Windows oder Linux stören, durch Ersatz; höchstens 120
/// Zeichen.
pub fn clean_name(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let out = out.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    let out: String = out.chars().take(120).collect();
    if out.is_empty() { "Unbenannt".into() } else { out }
}

/// Sendung: „Sender – Titel – JJJJ-MM-TT HH-MM.<ext>“ (`when` in Ortszeit, von der Oberfläche
/// formatiert).
pub fn show_name(channel: &str, title: &str, when: &str, ext: &str) -> String {
    format!(
        "{} – {} – {}.{ext}",
        clean_name(channel),
        clean_name(title),
        clean_name(when)
    )
}

/// Film: „Titel (Jahr).<ext>“, ohne Jahr nur der Titel.
pub fn film_name(title: &str, year: Option<i64>, ext: &str) -> String {
    match year {
        Some(y) => format!("{} ({y}).{ext}", clean_name(title)),
        None => format!("{}.{ext}", clean_name(title)),
    }
}

/// Endung aus einer Adresse (`.mp4`, `.mkv` …), sonst `fallback`.
pub fn extension_of(url: &str, fallback: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    path.rsplit('/')
        .next()
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| (2..=4).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| fallback.to_string())
}

/// Liegt unter `path` schon eine Datei, wird „ (2)“, „ (3)“ … angehängt.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() && !part_path(path).exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().map(|e| e.to_string_lossy().to_string());
    for n in 2.. {
        let name = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let p = path.with_file_name(name);
        if !p.exists() && !part_path(&p).exists() {
            return p;
        }
    }
    unreachable!()
}

/// Zwischendatei während des Ladens: `<datei>.part`.
pub fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

// ---------------------------------------------------------------------------------------------
// Herunterladen

/// Fortschritt und Abbruch eines laufenden Ladevorgangs (von der Oberfläche gelesen).
#[derive(Debug, Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Lädt `url` nach `path` (über `<datei>.part`, dann umbenannt). Liegt schon ein Teil da,
/// wird mit `Range` fortgesetzt; antwortet der Server ohne Teilinhalt, von vorn. `header`
/// ist eine zusätzliche Kopfzeile „Name: Wert“ (Jellyfin-Anmeldung).
pub fn download(
    url: &str,
    header: Option<&str>,
    path: &Path,
    progress: &Progress,
) -> Result<(), String> {
    // Jede Anfrage hat eine Zeitgrenze für den ganzen Empfang (ureq kennt keine für eine
    // stockende Verbindung). Bricht sie ab, nachdem etwas angekommen ist, geht es mit `Range`
    // weiter; kam bei einem Versuch nichts an, ist die Verbindung gestört.
    loop {
        let before = std::fs::metadata(part_path(path)).map(|m| m.len()).unwrap_or(0);
        match download_once(url, header, path, progress) {
            Ok(()) => return Ok(()),
            Err(e) if progress.cancelled() => return Err(e),
            Err(e) => {
                let after = std::fs::metadata(part_path(path)).map(|m| m.len()).unwrap_or(0);
                if after <= before {
                    return Err(e);
                }
            }
        }
    }
}

/// Ein Versuch von [`download`] (höchstens zehn Minuten Empfang).
fn download_once(
    url: &str,
    header: Option<&str>,
    path: &Path,
    progress: &Progress,
) -> Result<(), String> {
    use std::io::{Read, Write};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = part_path(path);
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(600)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(url);
    if let Some((k, v)) = header.and_then(|h| h.split_once(':')) {
        req = req.header(k.trim(), v.trim());
    }
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let mut resp = req.call().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if status == 416 && have > 0 {
        // Teil ist schon vollständig.
        std::fs::rename(&part, path).map_err(|e| e.to_string())?;
        return Ok(());
    }
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    let resume = status == 206 && have > 0;
    let length = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let start = if resume { have } else { 0 };
    progress
        .total
        .store(if length > 0 { start + length } else { 0 }, Ordering::Relaxed);
    progress.done.store(start, Ordering::Relaxed);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resume)
        .truncate(!resume)
        .open(&part)
        .map_err(|e| e.to_string())?;
    let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if progress.cancelled() {
            return Err("abgebrochen".into());
        }
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    let total = progress.total.load(Ordering::Relaxed);
    if total > 0 && progress.done.load(Ordering::Relaxed) < total {
        return Err("Verbindung vorzeitig beendet".into());
    }
    std::fs::rename(&part, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(clean_name("Tatort: Der/Fall?"), "Tatort- Der-Fall-");
        assert_eq!(clean_name("  ..  "), "Unbenannt");
        assert_eq!(film_name("Heat", Some(1995), "mkv"), "Heat (1995).mkv");
        assert_eq!(extension_of("https://x/a/b/film.MP4?x=1", "mp4"), "mp4");
        assert_eq!(extension_of("https://x/master.m3u8", "ts"), "m3u8");
        assert_eq!(extension_of("https://x/stream", "ts"), "ts");
        assert_eq!(
            show_name("3sat HD", "Kulturzeit", "2026-10-26 19-20", "ts"),
            "3sat HD – Kulturzeit – 2026-10-26 19-20.ts"
        );
    }

    #[test]
    fn index_and_eviction() {
        let ix = Index::in_memory();
        let a = ix
            .add(Art::Cache, "oer", "1", "A", "", Path::new("/t/a"))
            .unwrap();
        let b = ix
            .add(Art::Cache, "oer", "2", "B", "", Path::new("/t/b"))
            .unwrap();
        ix.set_size(a, 600, 600);
        ix.set_size(b, 600, 600);
        ix.set_state(a, State::Done, None);
        ix.set_state(b, State::Done, None);
        ix.touch(b);
        // Gleiche Kennung: derselbe Eintrag.
        assert_eq!(
            ix.add(Art::Cache, "oer", "1", "A", "", Path::new("/t/a"))
                .unwrap(),
            a
        );
        ix.set_state(a, State::Done, None);
        assert_eq!(ix.cache_bytes(), 1200);
        let out = ix.evictable(1000, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(ix.find("oer", "2").len(), 1);
        let t = Timer {
            id: 0,
            channel_id: 7,
            sat: true,
            channel: "3sat HD".into(),
            title: "Kulturzeit".into(),
            start: 100,
            stop: 200,
        };
        let id = ix.add_timer(&t).unwrap();
        assert!(ix.timer_for(7, true, 100).is_some());
        ix.remove_timer(id);
        assert!(ix.timers().is_empty());
    }
}
