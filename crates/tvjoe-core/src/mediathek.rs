//! Mediathek der öffentlich-rechtlichen Sender aus der Filmliste von MediathekView.
//!
//! Die Filmliste (xz-komprimiertes JSON, Schlüssel `"X"` mehrfach) wird beim Entpacken
//! stückweise eingelesen, damit der Speicherbedarf klein bleibt. Gespeichert werden nur die
//! letzten N Tage in einer lokalen SQLite-Datenbank mit Volltextsuche (FTS5).
//! Abgestimmt (Projektplan): erst die volle Liste, danach Änderungslisten; Episoden und Filme
//! werden geschätzt (die Liste unterscheidet sie nicht).

use std::fmt;
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::de::{Deserializer, MapAccess, Visitor};

use crate::net;

pub const FULL_URL: &str = "https://liste.mediathekview.de/Filmliste-akt.xz";
pub const DIFF_URL: &str = "https://liste.mediathekview.de/Filmliste-diff.xz";
const MAX_LIST_BYTES: u64 = 400 * 1024 * 1024;
/// Beiträge mit späterem Sendetermin, die vorab in der Mediathek stehen (z. B. neue Staffeln),
/// werden bis so weit in die Zukunft mit angezeigt.
const AHEAD_SECS: i64 = 60 * 86_400;
/// Spielpositionen: erst ab dieser Stelle (Sekunden) merken, ab diesem Anteil als gesehen.
const PROGRESS_MIN: f64 = 60.0;
const PROGRESS_DONE: f64 = 0.95;
const PROGRESS_KEEP_SECS: i64 = 183 * 86_400;
/// Nach dieser Zeit wird statt der Änderungsliste wieder die volle Liste geladen.
const FULL_EVERY_SECS: i64 = 7 * 86_400;

/// Grobe Art eines Beitrags (geschätzt aus Titel, Thema und Dauer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Other = 0,
    Episode = 1,
    Film = 2,
}

impl Kind {
    fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Episode,
            2 => Self::Film,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Film {
    pub id: i64,
    pub sender: String,
    pub topic: String,
    pub title: String,
    /// Sendezeit (Sekunden seit 1970).
    pub timestamp: i64,
    /// Dauer in Sekunden.
    pub duration: i64,
    pub description: String,
    pub url: String,
    pub url_low: Option<String>,
    pub url_hd: Option<String>,
    pub website: Option<String>,
    pub subtitle: Option<String>,
    pub kind: Kind,
}

/// Fassung eines Beitrags (aus dem Zusatz am Titelende bzw. dem Dateinamen).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Version {
    Normal,
    Untertitel,
    KlareSprache,
    Gebaerdensprache,
    Audiodeskription,
    GesprocheneUntertitel,
    OriginalUntertitel,
    Original,
    /// Andere Sprache (deutscher Name, z. B. „Niederländisch“).
    Sprache(&'static str),
}

impl Version {
    /// Bezeichnung für Knöpfe und Titel.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Normal => "Deutsch",
            Self::Untertitel => "Mit Untertitel",
            Self::KlareSprache => "Klare Sprache",
            Self::Gebaerdensprache => "Gebärdensprache",
            Self::Audiodeskription => "Audiodeskription",
            Self::GesprocheneUntertitel => "Gesprochene Untertitel",
            Self::OriginalUntertitel => "Original mit Untertitel",
            Self::Original => "Originalversion",
            Self::Sprache(name) => name,
        }
    }

    /// Kurzzeichen für die Kachel.
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Normal => "DE",
            Self::Untertitel => "UT",
            Self::KlareSprache => "Klare Sprache",
            Self::Gebaerdensprache => "DGS",
            Self::Audiodeskription => "AD",
            Self::GesprocheneUntertitel => "Gespr. UT",
            Self::OriginalUntertitel => "OmU",
            Self::Original => "OV",
            Self::Sprache(name) => name,
        }
    }

    /// Reihenfolge innerhalb einer Sendung (die erste steht auf der Kachel).
    fn rank(&self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Untertitel => 1,
            Self::KlareSprache => 2,
            Self::Gebaerdensprache => 3,
            Self::Audiodeskription => 4,
            Self::GesprocheneUntertitel => 5,
            Self::OriginalUntertitel => 6,
            Self::Original => 7,
            Self::Sprache(_) => 8,
        }
    }
}

/// Zusatz in Klammern (klein geschrieben, ohne Klammern) → Fassung.
fn version_from_note(note: &str) -> Option<Version> {
    use Version::*;
    Some(match note.trim().to_lowercase().as_str() {
        "audiodeskription" | "mit audiodeskription" | "hörfassung" | "ad" => Audiodeskription,
        "gebärdensprache" | "mit gebärdensprache" | "in gebärdensprache" => Gebaerdensprache,
        "originalversion mit untertitel" | "ov mit untertitel" | "omu" | "omdu" => {
            OriginalUntertitel
        }
        "mit untertitel" => Untertitel,
        "mit gesprochenen untertiteln" => GesprocheneUntertitel,
        "originalversion" | "originalfassung" | "original" | "ov" => Original,
        "klare sprache" => KlareSprache,
        "engl." | "englisch" | "eng" | "englische fassung" => Sprache("Englisch"),
        "frz." | "franz." | "fra" | "französisch" => Sprache("Französisch"),
        "nld" | "niederländisch" => Sprache("Niederländisch"),
        "swe" | "schwedisch" => Sprache("Schwedisch"),
        "ita" | "italienisch" => Sprache("Italienisch"),
        "dan" | "dänisch" => Sprache("Dänisch"),
        "nor" | "norwegisch" => Sprache("Norwegisch"),
        "tur" | "türkisch" | "türkische fassung" => Sprache("Türkisch"),
        "rus" | "russisch" => Sprache("Russisch"),
        "ukrainisch" => Sprache("Ukrainisch"),
        "obersorbisch" => Sprache("Obersorbisch"),
        _ => return None,
    })
}

/// Grundtitel und Fassung: ein Zusatz am Titelende wie „(mit Untertitel)“, „(nld)“ oder
/// „- Audiodeskription“ wird abgetrennt.
pub fn split_version(title: &str) -> (String, Version) {
    let t = title.trim_end();
    if let Some(open) = t.ends_with(')').then(|| t.rfind('(')).flatten() {
        if let Some(v) = version_from_note(&t[open + 1..t.len() - 1]) {
            return (t[..open].trim_end().to_string(), v);
        }
    }
    for sep in [" - ", " | ", " – "] {
        if let Some(pos) = t.rfind(sep) {
            if let Some(v) = version_from_note(&t[pos + sep.len()..]) {
                return (t[..pos].trim_end().to_string(), v);
            }
        }
    }
    (t.to_string(), Version::Normal)
}

/// Kennzeichen im Dateinamen der ARD-Anstalten; sie stimmen, wo der Titel in der Filmliste
/// falsch beschriftet ist (z. B. „(Audiodeskription)“ mit Datei `…_internationalerton_…`).
fn version_in_url(url: &str) -> Option<Version> {
    let lower = url.to_ascii_lowercase();
    if lower.contains("_audiodeskription_") {
        Some(Version::Audiodeskription)
    } else if lower.contains("_internationalerton_") || lower.contains("_originalversion_") {
        Some(Version::Original)
    } else if lower.contains("_sendeton_") {
        Some(Version::Normal)
    } else {
        None
    }
}

/// Fassung nach Titel, berichtigt durch den Dateinamen (nur Ton-Fassungen).
pub fn version_of(title: &str, url: &str) -> Version {
    let (_, from_title) = split_version(title);
    match (version_in_url(url), from_title) {
        (None, v) => v,
        (Some(Version::Original), Version::OriginalUntertitel) => Version::OriginalUntertitel,
        (Some(u), Version::Normal | Version::Audiodeskription | Version::Original) => u,
        (Some(_), v) => v,
    }
}

/// Staffel und Folge aus dem Titel: „(S02/E07)“, „Staffel 1, Folge 3“, „Folge 12“.
/// Staffeln ab 100 (Jahreszahlen bei Nachrichten) zählen nicht.
pub fn season_episode(title: &str) -> (Option<u32>, Option<u32>) {
    let t = title.to_lowercase();
    let number_after = |s: &str, key: &str| -> Option<u32> {
        let i = s.find(key)? + key.len();
        let digits: String = s[i..]
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    };
    // „(s02/e07)“
    let mut season = None;
    let mut episode = None;
    if let Some(i) = t.find("(s") {
        let rest = &t[i + 2..];
        let s_digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Some(after) = rest[s_digits.len()..].strip_prefix("/e") {
            let e_digits: String = after.chars().take_while(char::is_ascii_digit).collect();
            season = s_digits.parse().ok();
            episode = e_digits.parse().ok();
        }
    }
    if season.is_none() {
        season = number_after(&t, "staffel ");
    }
    if episode.is_none() {
        episode = number_after(&t, "folge ");
    }
    (season.filter(|s| *s < 100), episode)
}

/// Sport-Zusammenfassungen (Saison „2026/27“, Bundesliga, Liga, Sport im Thema): keine Serie,
/// nicht im Reiter „Episoden“ (Nutzer, 2026-10-25).
pub fn is_sport_topic(topic: &str) -> bool {
    let t = topic.to_lowercase();
    let season_year = t.as_bytes().windows(7).any(|w| {
        w[..4].iter().all(u8::is_ascii_digit)
            && w[4] == b'/'
            && w[5..].iter().all(u8::is_ascii_digit)
    });
    season_year
        || ["bundesliga", "liga", "sport"]
            .iter()
            .any(|k| t.contains(k))
}

/// Folge einer Serie mit Staffelangabe: wird mit allen älteren Folgen behalten, solange die
/// Serie in den letzten Tagen eine Folge hatte.
pub fn is_series_entry(topic: &str, title: &str) -> bool {
    season_episode(title).0.is_some() && !is_sport_topic(topic)
}

/// Eine Sendung mit allen Fassungen; die erste ist die bevorzugte.
#[derive(Debug, Clone)]
pub struct Group {
    pub films: Vec<Film>,
}

impl Group {
    pub fn main(&self) -> &Film {
        &self.films[0]
    }
}

/// Fasst Fassungen derselben Sendung zusammen (gleicher Sender, Grundtitel und Sendezeit).
/// Die Reihenfolge der Sendungen bleibt; je Fassung zählt der erste Eintrag.
pub fn group(films: Vec<Film>) -> Vec<Group> {
    let mut index: std::collections::HashMap<(String, String, i64), usize> =
        std::collections::HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    for f in films {
        let key = (f.sender.clone(), f.base_title(), f.timestamp);
        match index.get(&key) {
            Some(&i) => {
                let v = f.version();
                if !groups[i].films.iter().any(|g| g.version() == v) {
                    groups[i].films.push(f);
                }
            }
            None => {
                index.insert(key, groups.len());
                groups.push(Group { films: vec![f] });
            }
        }
    }
    for g in &mut groups {
        g.films.sort_by_key(|f| f.version().rank());
    }
    groups
}

impl Film {
    pub fn version(&self) -> Version {
        version_of(&self.title, &self.url)
    }

    /// Titel ohne Fassungszusatz.
    pub fn base_title(&self) -> String {
        split_version(&self.title).0
    }

    /// Titel mit der tatsächlichen Fassung, z. B. „Seitensprung (Originalversion)“.
    pub fn display_title(&self) -> String {
        match self.version() {
            Version::Normal => self.base_title(),
            v => format!("{} ({})", self.base_title(), v.label()),
        }
    }

    /// Video-URL zur gewünschten Stufe: SD = kleine, 720p = normale, sonst HD; fehlende
    /// Fassungen werden durch die normale ersetzt.
    pub fn url_for(&self, quality: Option<crate::Quality>) -> &str {
        use crate::Quality;
        let pick = match quality {
            Some(Quality::Sd) => self.url_low.as_deref(),
            Some(Quality::P720) => None,
            Some(Quality::P1080 | Quality::Uhd) | None => self.url_hd.as_deref(),
        };
        pick.unwrap_or(&self.url)
    }
}

/// Vorschaubild einer Sendungsseite: Inhalt von `<meta property="og:image" content="…">`
/// (auch `name=`), nur absolute http(s)-Adressen.
pub fn og_image(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("<meta") {
        let start = pos + i;
        let end = start + lower[start..].find('>')?;
        pos = end;
        let tag = &html[start..end];
        let attr = |name: &str| -> Option<String> {
            let low = tag.to_ascii_lowercase();
            let mut from = 0;
            while let Some(j) = low[from..].find(name) {
                let k = from + j;
                from = k + name.len();
                // Nur ganze Attributnamen (nicht „ng-attr-content“).
                if k > 0 && !low.as_bytes()[k - 1].is_ascii_whitespace() {
                    continue;
                }
                let rest = low[from..].trim_start();
                let Some(rest) = rest.strip_prefix('=') else {
                    continue;
                };
                let rest = rest.trim_start();
                let quote = rest.chars().next()?;
                if quote != '"' && quote != '\'' {
                    continue;
                }
                let value_start = tag.len() - rest.len() + 1;
                let len = tag[value_start..].find(quote)?;
                return Some(tag[value_start..value_start + len].to_string());
            }
            None
        };
        let key = attr("property").or_else(|| attr("name"));
        if key.is_some_and(|k| k.eq_ignore_ascii_case("og:image")) {
            let url = attr("content").map(|u| u.replace("&amp;", "&"));
            if let Some(url) = url.filter(|u| u.starts_with("https://") || u.starts_with("http://"))
            {
                return Some(url);
            }
        }
    }
    None
}

/// „Url Klein“/„Url HD“ sind als `Anzahl|Rest` codiert: die ersten *Anzahl* Zeichen der Url plus Rest.
fn expand_url(base: &str, short: &str) -> Option<String> {
    if short.is_empty() {
        return None;
    }
    let Some((n, rest)) = short.split_once('|') else {
        return Some(short.to_string());
    };
    let n: usize = n.parse().ok()?;
    base.get(..n).map(|p| format!("{p}{rest}"))
}

/// „00:43:20“ → Sekunden.
fn parse_duration(s: &str) -> i64 {
    s.split(':')
        .map(|p| p.trim().parse::<i64>().unwrap_or(0))
        .fold(0, |acc, v| acc * 60 + v)
}

/// Schätzt Episode oder Film (die Filmliste unterscheidet das nicht).
pub fn classify(topic: &str, title: &str, duration: i64) -> Kind {
    let t = title.to_lowercase();
    let has_se = {
        // „(S01/E17)“, „S2 E5“ u. ä.
        let b = t.as_bytes();
        b.windows(2).enumerate().any(|(i, w)| {
            w[0] == b's'
                && w[1].is_ascii_digit()
                && t[i..].find('e').is_some_and(|e| {
                    t[i + e + 1..].starts_with(|c: char| c.is_ascii_digit()) && e <= 5
                })
        })
    };
    if has_se || t.contains("folge ") || t.contains("staffel ") || t.contains("episode ") {
        return Kind::Episode;
    }
    let both = format!("{} {}", topic.to_lowercase(), t);
    let film_word = [
        "spielfilm",
        "fernsehfilm",
        "film",
        "kino",
        "tatort",
        "polizeiruf",
        "thriller",
        "krimi",
    ]
    .iter()
    .any(|w| both.contains(w));
    if duration >= 70 * 60 && film_word {
        Kind::Film
    } else {
        Kind::Other
    }
}

/// Ein Eintrag der Filmliste in der Reihenfolge der Felder (Kopfzeile „Sender, Thema, …“).
fn to_film(v: &[String], last_sender: &mut String, last_topic: &mut String) -> Option<Film> {
    let f = |i: usize| v.get(i).map(String::as_str).unwrap_or("");
    if !f(0).is_empty() {
        *last_sender = f(0).to_string();
    }
    if !f(1).is_empty() {
        *last_topic = f(1).to_string();
    }
    let url = f(8);
    if url.is_empty() {
        return None;
    }
    let timestamp: i64 = f(16).parse().ok()?;
    let duration = parse_duration(f(5));
    let nonempty = |s: &str| (!s.is_empty()).then(|| s.to_string());
    Some(Film {
        id: 0,
        sender: last_sender.clone(),
        topic: last_topic.clone(),
        title: f(2).to_string(),
        timestamp,
        duration,
        description: f(7).to_string(),
        url: url.to_string(),
        url_low: expand_url(url, f(12)),
        url_hd: expand_url(url, f(14)),
        website: nonempty(f(9)),
        subtitle: nonempty(f(10)),
        kind: classify(last_topic, f(2), duration),
    })
}

struct ListVisitor<'a, F: FnMut(Vec<String>)>(&'a mut F);

impl<'de, F: FnMut(Vec<String>)> Visitor<'de> for ListVisitor<'_, F> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("Filmliste (JSON-Objekt)")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            let value: Vec<String> = map.next_value()?;
            if key == "X" {
                (self.0)(value);
            }
        }
        Ok(())
    }
}

/// Liest eine (entpackte) Filmliste stückweise und ruft `on_film` je Film auf.
pub fn parse_filmliste<R: Read>(reader: R, mut on_film: impl FnMut(Film)) -> Result<(), String> {
    let mut de = serde_json::Deserializer::from_reader(BufReader::with_capacity(1 << 20, reader));
    let (mut sender, mut topic) = (String::new(), String::new());
    let mut cb = |v: Vec<String>| {
        if let Some(film) = to_film(&v, &mut sender, &mut topic) {
            on_film(film);
        }
    };
    de.deserialize_map(ListVisitor(&mut cb))
        .map_err(|e| format!("Filmliste unlesbar: {e}"))
}

/// Ausländische bzw. fremdsprachige Sender, standardmäßig ausgeblendet (Einstellung je Sender).
pub const HIDDEN_DEFAULT: &[&str] = &[
    "ORF", "SRF", "ARTE.FR", "ARTE.EN", "ARTE.ES", "ARTE.IT", "ARTE.PL",
];

/// Anzeigefilter aus den Einstellungen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Fassungen mit „Audiodeskription“ im Titel ausblenden.
    pub hide_ad: bool,
    /// Ausgeblendete Sender (Kurzname aus der Filmliste).
    pub hidden: Vec<String>,
}

impl Filter {
    /// Ohne Einschränkung (alles anzeigen).
    pub const ALL: Filter = Filter {
        hide_ad: false,
        hidden: Vec::new(),
    };

    fn sql(&self, alias: &str) -> String {
        let mut s = String::new();
        if self.hide_ad {
            // Wie `version_of`: Kennzeichen im Dateinamen gehen dem Titel vor.
            let u = format!("lower({alias}url)");
            s.push_str(&format!(
                " AND NOT ({u} LIKE '%\\_audiodeskription\\_%' ESCAPE '\\' OR (({alias}title LIKE '%Audiodeskription%' OR {alias}title LIKE '%(Hörfassung)' OR {alias}title LIKE '%(AD)') \
                 AND {u} NOT LIKE '%\\_internationalerton\\_%' ESCAPE '\\' \
                 AND {u} NOT LIKE '%\\_originalversion\\_%' ESCAPE '\\' \
                 AND {u} NOT LIKE '%\\_sendeton\\_%' ESCAPE '\\'))"
            ));
        }
        if !self.hidden.is_empty() {
            let list: Vec<String> = self
                .hidden
                .iter()
                .map(|f| format!("'{}'", f.replace('\'', "''")))
                .collect();
            s.push_str(&format!(" AND {alias}sender NOT IN ({})", list.join(",")));
        }
        s
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportStats {
    /// Einträge in der Liste.
    pub seen: usize,
    /// Neu übernommen.
    pub added: usize,
    /// Wegen Alter (älter als die eingestellten Tage) entfernt.
    pub purged: usize,
    /// Bestand danach.
    pub total: i64,
}

pub struct Mediathek {
    conn: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS films(
    id INTEGER PRIMARY KEY, sender TEXT NOT NULL, topic TEXT NOT NULL, title TEXT NOT NULL,
    ts INTEGER NOT NULL, duration INTEGER NOT NULL, description TEXT NOT NULL,
    url TEXT NOT NULL UNIQUE, url_low TEXT, url_hd TEXT, website TEXT, subtitle TEXT,
    kind INTEGER NOT NULL, series INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS films_ts ON films(ts);
CREATE INDEX IF NOT EXISTS films_sender_ts ON films(sender, ts);
CREATE INDEX IF NOT EXISTS films_kind_ts ON films(kind, ts);
CREATE VIRTUAL TABLE IF NOT EXISTS films_fts USING fts5(
    title, topic, description, content='films', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2');
CREATE TRIGGER IF NOT EXISTS films_ai AFTER INSERT ON films BEGIN
    INSERT INTO films_fts(rowid, title, topic, description)
    VALUES (new.id, new.title, new.topic, new.description); END;
CREATE TRIGGER IF NOT EXISTS films_ad AFTER DELETE ON films BEGIN
    INSERT INTO films_fts(films_fts, rowid, title, topic, description)
    VALUES ('delete', old.id, old.title, old.topic, old.description); END;
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS progress(url TEXT PRIMARY KEY, position REAL NOT NULL,
    duration REAL NOT NULL, updated INTEGER NOT NULL);
";

const COLUMNS: &str = "id, sender, topic, title, ts, duration, description, url, url_low, url_hd, website, subtitle, kind";

fn row_to_film(r: &rusqlite::Row) -> rusqlite::Result<Film> {
    Ok(Film {
        id: r.get(0)?,
        sender: r.get(1)?,
        topic: r.get(2)?,
        title: r.get(3)?,
        timestamp: r.get(4)?,
        duration: r.get(5)?,
        description: r.get(6)?,
        url: r.get(7)?,
        url_low: r.get(8)?,
        url_hd: r.get(9)?,
        website: r.get(10)?,
        subtitle: r.get(11)?,
        kind: Kind::from_i64(r.get(12)?),
    })
}

fn db_err(e: rusqlite::Error) -> String {
    format!("Mediathek-Datenbank: {e}")
}

impl Mediathek {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(db_err)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(db_err)?;
        conn.execute_batch(SCHEMA).map_err(db_err)?;
        // Ältere Datenbanken: Spalte `series` nachrüsten; die nächste Aktualisierung lädt dann
        // die volle Liste, damit ältere Serienfolgen dazukommen.
        let has_series = conn.prepare("SELECT series FROM films LIMIT 0").is_ok();
        if !has_series {
            conn.execute_batch(
                "ALTER TABLE films ADD COLUMN series INTEGER NOT NULL DEFAULT 0;
                 DELETE FROM meta WHERE key = 'last_full';",
            )
            .map_err(db_err)?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS films_series ON films(series, sender, topic, ts);",
        )
        .map_err(db_err)?;
        Ok(Self { conn })
    }

    fn meta(&self, key: &str) -> Option<i64> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    fn set_meta(&self, key: &str, value: i64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
            .map_err(db_err)
    }

    /// Zeitpunkt der letzten Aktualisierung (Sekunden seit 1970).
    pub fn last_update(&self) -> Option<i64> {
        self.meta("last_update")
    }

    /// Volle Liste nötig? (leer oder letzte volle Liste älter als eine Woche)
    pub fn needs_full(&self, now: i64) -> bool {
        self.count() == 0
            || self
                .meta("last_full")
                .is_none_or(|t| now - t > FULL_EVERY_SECS)
    }

    /// Spielposition merken (ab 1 Minute); ab 95 % gilt der Beitrag als gesehen. Unabhängig von der Filmliste (bleibt bei neuer Liste erhalten).
    pub fn save_progress(&self, url: &str, position: f64, duration: f64, now: i64) {
        // Gesehen (ab 95 %) wird als Position = Dauer gemerkt (für „nächste Folge“).
        let done = duration > 0.0 && position >= duration * PROGRESS_DONE;
        let position = if done { duration } else { position };
        let result = if position < PROGRESS_MIN && !done {
            self.conn
                .execute("DELETE FROM progress WHERE url = ?1", [url])
        } else {
            self.conn.execute(
                "INSERT INTO progress(url, position, duration, updated) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(url) DO UPDATE SET position = excluded.position,
                 duration = excluded.duration, updated = excluded.updated",
                params![url, position, duration, now],
            )
        };
        if let Err(e) = result {
            eprintln!("tvjoe: {}", db_err(e));
        }
    }

    /// Zuletzt geänderte Position (Sekunden seit 1970) – für „Weitersehen“ in Serien.
    pub fn progress_updated(&self, url: &str) -> Option<i64> {
        self.conn
            .query_row("SELECT updated FROM progress WHERE url = ?1", [url], |r| {
                r.get(0)
            })
            .optional()
            .ok()
            .flatten()
    }

    /// Alle Beiträge eines Themas (Serie), auch vorab veröffentlichte, mit Anzeigefilter.
    pub fn topic_films(&self, sender: &str, topic: &str, filter: &Filter, now: i64) -> Vec<Film> {
        let sql = format!(
            "SELECT {COLUMNS} FROM films WHERE sender = ?1 AND topic = ?2 AND ts <= ?3 + {AHEAD_SECS}{}
             ORDER BY ts",
            filter.sql("")
        );
        let Ok(mut st) = self.conn.prepare(&sql) else {
            return Vec::new();
        };
        st.query_map(params![sender, topic, now], row_to_film)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Anzahl der Beiträge eines Themas.
    pub fn topic_count(&self, sender: &str, topic: &str, filter: &Filter) -> i64 {
        let sql = format!(
            "SELECT COUNT(*) FROM films WHERE sender = ?1 AND topic = ?2{}",
            filter.sql("")
        );
        self.conn
            .query_row(&sql, params![sender, topic], |r| r.get(0))
            .unwrap_or(0)
    }

    /// Gemerkte Position und Dauer eines Videos.
    pub fn progress(&self, url: &str) -> Option<(f64, f64)> {
        self.conn
            .query_row(
                "SELECT position, duration FROM progress WHERE url = ?1",
                [url],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn count(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM films", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// Übernimmt eine entpackte Filmliste. `full`: Bestand vorher leeren (volle Liste),
    /// sonst nur ergänzen (Änderungsliste). Behalten werden die letzten `days` Tage.
    pub fn import<R: Read>(
        &mut self,
        reader: R,
        full: bool,
        days: u32,
        now: i64,
    ) -> Result<ImportStats, String> {
        let cutoff = now - i64::from(days.max(1)) * 86_400;
        let tx = self.conn.transaction().map_err(db_err)?;
        if full {
            tx.execute("DELETE FROM films", []).map_err(db_err)?;
        }
        // Ältere Serienfolgen zuerst in eine Zwischentabelle; übernommen werden nur die von
        // Serien mit aktueller Folge (spart Schreiben und Volltextindex).
        tx.execute_batch(
            "DROP TABLE IF EXISTS temp.old_series;
             CREATE TEMP TABLE old_series AS SELECT * FROM films WHERE 0;",
        )
        .map_err(db_err)?;
        let mut stats = ImportStats::default();
        let mut failure: Option<String> = None;
        {
            const COLS: &str = "(sender, topic, title, ts, duration, description, url, url_low,
                url_hd, website, subtitle, kind, series)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)";
            let mut insert = tx
                .prepare(&format!("INSERT OR IGNORE INTO films{COLS}"))
                .map_err(db_err)?;
            let mut insert_old = tx
                .prepare(&format!("INSERT INTO temp.old_series{COLS}"))
                .map_err(db_err)?;
            parse_filmliste(reader, |f| {
                stats.seen += 1;
                let series = is_series_entry(&f.topic, &f.title);
                let old = f.timestamp < cutoff;
                if (old && !series) || failure.is_some() {
                    return;
                }
                let target = if old { &mut insert_old } else { &mut insert };
                match target.execute(params![
                    f.sender,
                    f.topic,
                    f.title,
                    f.timestamp,
                    f.duration,
                    f.description,
                    f.url,
                    f.url_low,
                    f.url_hd,
                    f.website,
                    f.subtitle,
                    f.kind as i64,
                    series
                ]) {
                    Ok(n) if !old => stats.added += n,
                    Ok(_) => {}
                    Err(e) => failure = Some(db_err(e)),
                }
            })?;
        }
        if let Some(e) = failure {
            return Err(e);
        }
        stats.added += tx
            .execute(
                "INSERT OR IGNORE INTO films(sender, topic, title, ts, duration, description, url,
                     url_low, url_hd, website, subtitle, kind, series)
                 SELECT sender, topic, title, ts, duration, description, url, url_low, url_hd,
                     website, subtitle, kind, series FROM temp.old_series o
                 WHERE EXISTS (SELECT 1 FROM films r WHERE r.series = 1 AND r.ts >= ?1
                     AND r.sender = o.sender AND r.topic = o.topic)",
                [cutoff],
            )
            .map_err(db_err)?;
        tx.execute("DROP TABLE temp.old_series", [])
            .map_err(db_err)?;
        // Positionen, die länger als ein halbes Jahr nicht angefasst wurden.
        tx.execute(
            "DELETE FROM progress WHERE updated < ?1",
            [now - PROGRESS_KEEP_SECS],
        )
        .map_err(db_err)?;
        stats.purged = tx
            .execute(
                "DELETE FROM films WHERE ts < ?1 AND NOT (series = 1 AND EXISTS (
                     SELECT 1 FROM films r WHERE r.series = 1 AND r.ts >= ?1
                     AND r.sender = films.sender AND r.topic = films.topic))",
                [cutoff],
            )
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        if full {
            self.set_meta("last_full", now)?;
        }
        self.set_meta("last_update", now)?;
        stats.total = self.count();
        Ok(stats)
    }

    /// Neueste Beiträge, optional nach Art und Sender (auch vorab veröffentlichte).
    pub fn latest(
        &self,
        kind: Option<Kind>,
        sender: Option<&str>,
        now: i64,
        limit: usize,
    ) -> Vec<Film> {
        self.latest_filtered(kind, sender, &Filter::ALL, now, limit)
    }

    /// Wie [`Mediathek::latest`], mit Anzeigefilter.
    pub fn latest_filtered(
        &self,
        kind: Option<Kind>,
        sender: Option<&str>,
        filter: &Filter,
        now: i64,
        limit: usize,
    ) -> Vec<Film> {
        let sql = format!(
            "SELECT {COLUMNS} FROM films
             WHERE ts <= ?1 + {AHEAD_SECS} AND (?2 IS NULL OR kind = ?2) AND (?3 IS NULL OR sender = ?3){}
             ORDER BY ts DESC LIMIT ?4",
            filter.sql("")
        );
        let Ok(mut st) = self.conn.prepare(&sql) else {
            return Vec::new();
        };
        st.query_map(
            params![now, kind.map(|k| k as i64), sender, limit as i64],
            row_to_film,
        )
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    /// Volltextsuche in Titel, Thema und Beschreibung (alle Wörter, Wortanfänge genügen).
    pub fn search(&self, text: &str, sender: Option<&str>, limit: usize) -> Vec<Film> {
        self.search_filtered(text, sender, &Filter::ALL, limit)
    }

    /// Wie [`Mediathek::search`], mit Anzeigefilter.
    pub fn search_filtered(
        &self,
        text: &str,
        sender: Option<&str>,
        filter: &Filter,
        limit: usize,
    ) -> Vec<Film> {
        let query: Vec<String> = text
            .split_whitespace()
            .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
            .collect();
        if query.is_empty() {
            return Vec::new();
        }
        let sql = format!(
            "SELECT {} FROM films_fts JOIN films f ON f.id = films_fts.rowid
             WHERE films_fts MATCH ?1 AND (?2 IS NULL OR f.sender = ?2){}
             ORDER BY f.ts DESC LIMIT ?3",
            COLUMNS
                .split(", ")
                .map(|c| format!("f.{c}"))
                .collect::<Vec<_>>()
                .join(", "),
            filter.sql("f.")
        );
        let Ok(mut st) = self.conn.prepare(&sql) else {
            return Vec::new();
        };
        st.query_map(params![query.join(" "), sender, limit as i64], row_to_film)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Sender mit Anzahl der Beiträge (absteigend).
    pub fn senders(&self) -> Vec<(String, i64)> {
        self.senders_filtered(&Filter::ALL)
    }

    /// Wie [`Mediathek::senders`], mit Anzeigefilter.
    pub fn senders_filtered(&self, filter: &Filter) -> Vec<(String, i64)> {
        let sql = format!(
            "SELECT sender, COUNT(*) AS n FROM films WHERE 1=1{} GROUP BY sender ORDER BY n DESC",
            filter.sql("")
        );
        let Ok(mut st) = self.conn.prepare(&sql) else {
            return Vec::new();
        };
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }
}

/// Aktualisiert die Datenbank aus dem Netz: volle Liste, wenn nötig, sonst Änderungsliste.
/// Die geladene Datei liegt im Cacheordner und wird danach gelöscht.
pub fn update_from_net(
    db: &Path,
    cache_dir: &Path,
    days: u32,
    now: i64,
) -> Result<ImportStats, String> {
    let mut mt = Mediathek::open(db)?;
    let full = mt.needs_full(now);
    let (url, name) = if full {
        (FULL_URL, "Filmliste-akt.xz")
    } else {
        (DIFF_URL, "Filmliste-diff.xz")
    };
    let file = cache_dir.join(name);
    let agent = net::agent(Duration::from_secs(600));
    net::download_to(&agent, url, &file, MAX_LIST_BYTES)?;
    let reader = std::fs::File::open(&file).map_err(|e| e.to_string())?;
    let result = mt.import(liblzma::read::XzDecoder::new(reader), full, days, now);
    let _ = std::fs::remove_file(&file);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"Filmliste":["24.10.2026, 22:19","x","3","MSearch","id"],
"Filmliste":["Sender","Thema","Titel","Datum","Zeit","Dauer","Größe [MB]","Beschreibung","Url","Website","Url Untertitel","Url RTMP","Url Klein","Url RTMP Klein","Url HD","Url RTMP HD","DatumL","Url History","Geo","neu"],
"X":["3Sat","...von oben","Südspanien von oben (S01/E17)","26.12.2022","07:35:00","00:43:20","870","Spaniens Süden.","https://a.invalid/x/221201_3360k.mp4","https://3sat.invalid/s","","","22|_808k.mp4","","22|_hd.mp4","","1792800000","","DE-AT-CH","false"],
"X":["","","Portugal von oben","16.10.2024","16:18:00","00:42:52","881","Strände und Dörfer","https://a.invalid/x/portugal.mp4","","","","","","","","1792700000","","","false"],
"X":["ZDF","Fernsehfilm der Woche","Das Geheimnis","24.10.2026","20:15:00","01:28:00","1","Ein Krimi","https://z.invalid/film.mp4","","","","","","","","1792790000","","","false"],
"X":["ARD","tagesschau","alt","01.01.2020","20:00:00","00:15:00","1","alt","https://alt.invalid/a.mp4","","","","","","","","1577905200","","","false"],
"X":["ARD","Ohne Url","x","1","1","00:01:00","1","","","","","","","","","","1792800000","","","false"]}"#;

    #[test]
    fn hilfsfunktionen() {
        assert_eq!(
            expand_url("https://a.invalid/x/221201_3360k.mp4", "22|_808k.mp4").as_deref(),
            Some("https://a.invalid/x/22_808k.mp4")
        );
        assert_eq!(expand_url("https://a", ""), None);
        assert_eq!(parse_duration("00:43:20"), 2600);
        assert_eq!(parse_duration("01:28:00"), 5280);
        assert_eq!(
            classify("...von oben", "Südspanien von oben (S01/E17)", 2600),
            Kind::Episode
        );
        assert_eq!(classify("Serie", "Folge 12: Der Fund", 2600), Kind::Episode);
        assert_eq!(
            classify("Fernsehfilm der Woche", "Das Geheimnis", 5280),
            Kind::Film
        );
        assert_eq!(
            classify("tagesschau", "tagesschau 20:00 Uhr", 900),
            Kind::Other
        );
    }

    #[test]
    fn liste_einlesen_mit_vererbung() {
        let mut films = Vec::new();
        parse_filmliste(LIST.as_bytes(), |f| films.push(f)).unwrap();
        assert_eq!(films.len(), 4, "Eintrag ohne Url wird übersprungen");
        assert_eq!(films[1].sender, "3Sat");
        assert_eq!(films[1].topic, "...von oben");
        assert_eq!(
            films[0].url_low.as_deref(),
            Some("https://a.invalid/x/22_808k.mp4")
        );
        assert_eq!(films[2].kind, Kind::Film);
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tvjoe-mt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("mediathek.sqlite")
    }

    #[test]
    fn datenbank_neueste_suche_und_alter() {
        let path = tmp("db");
        let mut mt = Mediathek::open(&path).unwrap();
        let now = 1_792_900_000;
        assert!(mt.needs_full(now));
        let s = mt.import(LIST.as_bytes(), true, 14, now).unwrap();
        // Der Eintrag von 2020 ist älter als 14 Tage.
        assert_eq!((s.seen, s.added, s.total), (4, 3, 3));
        assert!(!mt.needs_full(now));

        let latest = mt.latest(None, None, now, 10);
        assert_eq!(latest[0].title, "Südspanien von oben (S01/E17)");
        assert_eq!(mt.latest(Some(Kind::Film), None, now, 10)[0].sender, "ZDF");
        assert_eq!(mt.latest(None, Some("3Sat"), now, 10).len(), 2);

        assert_eq!(mt.search("portugal", None, 10).len(), 1);
        assert_eq!(mt.search("Südsp", None, 10).len(), 1, "Wortanfang genügt");
        assert_eq!(
            mt.search("strande dorfer", None, 10).len(),
            1,
            "ohne Umlaute"
        );
        assert!(mt.search("gibtsnicht", None, 10).is_empty());
        assert_eq!(mt.senders()[0], ("3Sat".to_string(), 2));

        // Änderungsliste: nichts doppelt, Alter wird wieder bereinigt.
        let s = mt.import(LIST.as_bytes(), false, 14, now).unwrap();
        assert_eq!((s.added, s.total), (0, 3));
        let s = mt.import(&b"{}"[..], false, 1, now).unwrap();
        assert_eq!(s.total, 0, "mit 1 Tag bleibt nichts übrig");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn vorschaubild_aus_og_image() {
        let html = r#"<head><meta data-rh="true" content="image/jpeg" property="og:image:type"/>
<meta property="og:image" ng-attr-content="{{ meta.image }}" />
<meta name="og:image" content="https://www.kika.invalid/bild.jpg?v=1"/>"#;
        assert_eq!(
            og_image(html).as_deref(),
            Some("https://www.kika.invalid/bild.jpg?v=1")
        );
        let arte = r#"<meta property="og:image" content="https://api.invalid/1920x1080?type=TEXT&amp;watermark=true"/>"#;
        assert_eq!(
            og_image(arte).as_deref(),
            Some("https://api.invalid/1920x1080?type=TEXT&watermark=true")
        );
        assert_eq!(
            og_image("<meta property='og:image' content='/rel.jpg'>"),
            None
        );
        assert_eq!(og_image("<html>"), None);
    }

    #[test]
    fn fassung_nach_dateiname() {
        let film = |title: &str, url: &str| Film {
            id: 0,
            sender: String::new(),
            topic: String::new(),
            title: title.into(),
            timestamp: 0,
            duration: 0,
            description: String::new(),
            url: url.into(),
            url_low: None,
            url_hd: None,
            website: None,
            subtitle: None,
            kind: Kind::Episode,
        };
        // Filmliste vertauscht: „(Audiodeskription)“ ist der internationale Ton.
        let f = film(
            "Kommissar Van der Valk (S03/E09) (Audiodeskription)",
            "https://x.invalid/JOB_547268__internationalerton_1280x720.mp4",
        );
        assert_eq!(f.version(), Version::Original);
        assert_eq!(
            f.display_title(),
            "Kommissar Van der Valk (S03/E09) (Originalversion)"
        );
        let f = film(
            "Kommissar Van der Valk (S03/E09) (Originalversion)",
            "https://x.invalid/JOB_547269__audiodeskription_1280x720.mp4",
        );
        assert_eq!(
            f.display_title(),
            "Kommissar Van der Valk (S03/E09) (Audiodeskription)"
        );
        let f = film(
            "Folge 1 (Audiodeskription)",
            "https://x.invalid/0242432_sendeton_1280.mp4",
        );
        assert_eq!(f.display_title(), "Folge 1");
        let f = film(
            "Film (Audiodeskription)",
            "https://zdf.invalid/a3a4_3360k.mp4",
        );
        assert_eq!(f.version(), Version::Audiodeskription);
        assert_eq!(f.display_title(), "Film (Audiodeskription)");
    }

    #[test]
    fn spielposition_merken_und_loeschen() {
        let path = tmp("progress");
        let mt = Mediathek::open(&path).unwrap();
        mt.save_progress("u", 30.0, 1000.0, 1);
        assert_eq!(mt.progress("u"), None);
        mt.save_progress("u", 300.0, 1000.0, 1);
        assert_eq!(mt.progress("u"), Some((300.0, 1000.0)));
        mt.save_progress("u", 400.0, 1000.0, 2);
        assert_eq!(mt.progress("u"), Some((400.0, 1000.0)));
        mt.save_progress("u", 960.0, 1000.0, 3);
        assert_eq!(mt.progress("u"), Some((1000.0, 1000.0)));
        assert_eq!(mt.progress_updated("u"), Some(3));
        mt.save_progress("u", 10.0, 1000.0, 4);
        assert_eq!(mt.progress("u"), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn staffel_folge_und_sport() {
        assert_eq!(
            season_episode("Folge 7: Der Sturm (S02/E07)"),
            (Some(2), Some(7))
        );
        assert_eq!(
            season_episode("Der Fall (Staffel 1, Folge 3)"),
            (Some(1), Some(3))
        );
        assert_eq!(season_episode("Folge 12: Der Fund"), (None, Some(12)));
        assert_eq!(season_episode("heute (S2026/E290)"), (None, Some(290)));
        assert!(is_sport_topic("2. Bundesliga - 2026/27"));
        assert!(is_sport_topic("BR24Sport"));
        assert!(!is_sport_topic("Notruf Hafenkante"));
        assert!(is_series_entry("Notruf Hafenkante", "Abgetaucht (S20/E05)"));
        assert!(!is_series_entry(
            "Bundesliga - 2026/27",
            "Torspektakel (S06/E09)"
        ));
    }

    #[test]
    fn aeltere_serienfolgen_bleiben_wenn_serie_aktuell() {
        let x = |sender: &str, topic: &str, title: &str, ts: i64, url: &str| {
            format!(
                r#""X":["{sender}","{topic}","{title}","1","1","00:45:00","1","","https://x.invalid/{url}.mp4","","","","","","","","{ts}","","",""]"#
            )
        };
        let now = 1_792_900_000;
        let old = now - 300 * 86_400;
        let list = format!(
            "{{{}}}",
            [
                x("ZDF", "Serie A", "Neu (S02/E01)", now - 3600, "a1"),
                x("ZDF", "Serie A", "Alt (S01/E01)", old, "a2"),
                x("ZDF", "Serie B", "Alt (S01/E01)", old, "b1"),
                x("ZDF", "Liga - 2026/27", "Neu (S06/E09)", now - 3600, "c1"),
                x("ZDF", "Liga - 2026/27", "Alt (S06/E01)", old, "c2"),
                x("ZDF", "Magazin", "Alt ohne Staffel", old, "d1"),
            ]
            .join(",")
        );
        let path = tmp("serien");
        let mut mt = Mediathek::open(&path).unwrap();
        mt.import(list.as_bytes(), true, 14, now).unwrap();
        let topics: Vec<String> = mt
            .topic_films("ZDF", "Serie A", &Filter::ALL, now)
            .into_iter()
            .map(|f| f.title)
            .collect();
        assert_eq!(topics, ["Alt (S01/E01)", "Neu (S02/E01)"]);
        assert_eq!(mt.topic_count("ZDF", "Serie B", &Filter::ALL), 0);
        assert_eq!(mt.topic_count("ZDF", "Liga - 2026/27", &Filter::ALL), 1);
        assert_eq!(mt.count(), 3);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn zusaetze_am_titelende() {
        let v = |t: &str| split_version(t);
        assert_eq!(
            v("Halt nicht an!(nld)"),
            ("Halt nicht an!".into(), Version::Sprache("Niederländisch"))
        );
        assert_eq!(
            v("Sanfte Monster (Originalversion mit Untertitel)"),
            ("Sanfte Monster".into(), Version::OriginalUntertitel)
        );
        assert_eq!(
            v("Folge 2 (S01/E02) - Audiodeskription"),
            ("Folge 2 (S01/E02)".into(), Version::Audiodeskription)
        );
        assert_eq!(
            v("Wiesn (6) (Hörfassung)"),
            ("Wiesn (6)".into(), Version::Audiodeskription)
        );
        assert_eq!(
            v("Folge 7 (S02/E07)"),
            ("Folge 7 (S02/E07)".into(), Version::Normal)
        );
        assert_eq!(
            v("Stummfilm (stumm)"),
            ("Stummfilm (stumm)".into(), Version::Normal)
        );
    }

    #[test]
    fn fassungen_werden_zusammengefasst() {
        let film = |title: &str, ts: i64| Film {
            id: 0,
            sender: "ARTE.DE".into(),
            topic: String::new(),
            title: title.into(),
            timestamp: ts,
            duration: 0,
            description: String::new(),
            url: format!("https://x.invalid/{title}.mp4"),
            url_low: None,
            url_hd: None,
            website: None,
            subtitle: None,
            kind: Kind::Film,
        };
        let groups = group(vec![
            film("Sanfte Monster (mit Untertitel)", 10),
            film("Sanfte Monster", 10),
            film("Sanfte Monster (Originalversion mit Untertitel)", 10),
            film("Sanfte Monster", 99),
            film("Anderer Film", 10),
        ]);
        assert_eq!(groups.len(), 3);
        let versions: Vec<Version> = groups[0].films.iter().map(Film::version).collect();
        assert_eq!(
            versions,
            [
                Version::Normal,
                Version::Untertitel,
                Version::OriginalUntertitel
            ]
        );
        assert_eq!(groups[0].main().display_title(), "Sanfte Monster");
        assert_eq!(
            groups[0].films[2].display_title(),
            "Sanfte Monster (Original mit Untertitel)"
        );
    }

    #[test]
    fn url_nach_qualitaet() {
        let f = Film {
            id: 0,
            sender: String::new(),
            topic: String::new(),
            title: String::new(),
            timestamp: 0,
            duration: 0,
            description: String::new(),
            url: "n".into(),
            url_low: Some("k".into()),
            url_hd: None,
            website: None,
            subtitle: None,
            kind: Kind::Other,
        };
        assert_eq!(f.url_for(Some(crate::Quality::Sd)), "k");
        assert_eq!(f.url_for(Some(crate::Quality::P720)), "n");
        assert_eq!(f.url_for(None), "n");
    }

    #[test]
    fn filter_audiodeskription_und_ausland() {
        let list = r#"{"X":["ARD","T","Film","1","1","00:10:00","1","","https://x.invalid/1.mp4","","","","","","","","1792800000","","",""],
"X":["ARD","T","Film (Audiodeskription)","1","1","00:10:00","1","","https://x.invalid/2.mp4","","","","","","","","1792800001","","",""],
"X":["ORF","T","Sendung","1","1","00:10:00","1","","https://x.invalid/3.mp4","","","","","","","","1792800002","","",""],
"X":["ARD","T","Falsch (Audiodeskription)","1","1","00:10:00","1","","https://x.invalid/J_internationalerton_1.mp4","","","","","","","","1792700000","","",""],
"X":["ARD","T","Falsch (Originalversion)","1","1","00:10:00","1","","https://x.invalid/J_audiodeskription_1.mp4","","","","","","","","1792700001","","",""]}"#;
        let path = tmp("filter");
        let mut mt = Mediathek::open(&path).unwrap();
        let now = 1_792_900_000;
        mt.import(list.as_bytes(), true, 14, now).unwrap();
        let standard = Filter {
            hide_ad: false,
            hidden: vec!["ORF".into(), "O'Bscure".into()],
        };
        let titles = |f: &Filter| -> Vec<String> {
            mt.latest_filtered(None, None, f, now, 10)
                .into_iter()
                .map(|x| x.title)
                .collect()
        };
        assert_eq!(
            titles(&standard),
            [
                "Film (Audiodeskription)",
                "Film",
                "Falsch (Originalversion)",
                "Falsch (Audiodeskription)"
            ]
        );
        assert_eq!(
            titles(&Filter {
                hide_ad: true,
                ..standard.clone()
            }),
            ["Film", "Falsch (Audiodeskription)"]
        );
        assert_eq!(titles(&Filter::ALL).len(), 5);
        assert_eq!(mt.senders_filtered(&standard), [("ARD".to_string(), 4)]);
        assert_eq!(mt.search_filtered("sendung", None, &standard, 10).len(), 0);
        assert_eq!(
            mt.search_filtered("sendung", None, &Filter::ALL, 10).len(),
            1
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Echte Filmliste: `TVJOE_TEST_FILMLISTE=/pfad/Filmliste-akt.xz cargo test -p tvjoe-core
    /// --release -- --ignored --nocapture echte_filmliste`
    #[test]
    #[ignore]
    fn echte_filmliste() {
        let Ok(file) = std::env::var("TVJOE_TEST_FILMLISTE") else {
            return;
        };
        let path = tmp("echt");
        let mut mt = Mediathek::open(&path).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let start = std::time::Instant::now();
        let reader = liblzma::read::XzDecoder::new(std::fs::File::open(&file).unwrap());
        let s = mt.import(reader, true, 14, now).unwrap();
        let secs = start.elapsed().as_secs_f64();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!(
            "{s:?} in {secs:.1} s, Datenbank {:.0} MB",
            size as f64 / 1e6
        );
        let t = std::time::Instant::now();
        let ep = mt.latest(Some(Kind::Episode), None, now, 40);
        let fi = mt.latest(Some(Kind::Film), None, now, 40);
        let se = mt.search("tatort", None, 40);
        println!(
            "Abfragen in {:.1} ms: {} Episoden, {} Filme, {} Treffer „tatort“",
            t.elapsed().as_secs_f64() * 1e3,
            ep.len(),
            fi.len(),
            se.len()
        );
        for f in fi.iter().take(5) {
            println!(
                "  Film: {} | {} | {} | {} min",
                f.sender,
                f.topic,
                f.title,
                f.duration / 60
            );
        }
        for f in ep.iter().take(5) {
            println!("  Episode: {} | {} | {}", f.sender, f.topic, f.title);
        }
        println!(
            "Sender: {:?}",
            mt.senders().iter().take(25).collect::<Vec<_>>()
        );
        let standard = Filter {
            hide_ad: false,
            hidden: HIDDEN_DEFAULT.iter().map(|s| s.to_string()).collect(),
        };
        println!(
            "Sender mit Standardfilter: {:?}",
            mt.senders_filtered(&standard)
                .iter()
                .map(|(s, _)| s.as_str())
                .collect::<Vec<_>>()
        );
        let films = mt.latest_filtered(Some(Kind::Film), None, &standard, now, 600);
        let n = films.len();
        let groups = group(films);
        println!("Filme: {n} Einträge → {} Sendungen", groups.len());
        for g in groups.iter().filter(|g| g.films.len() > 1).take(8) {
            let v: Vec<&str> = g.films.iter().map(|f| f.version().badge()).collect();
            println!("  {} | {}", g.main().base_title(), v.join(" · "));
        }
        if std::env::var_os("TVJOE_TEST_OHNE_NETZ").is_some() {
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
            return;
        }
        // Vorschaubilder: je Sender die Seite des neuesten Beitrags.
        let agent = net::agent(Duration::from_secs(10));
        for (sender, _) in mt.senders_filtered(&standard) {
            let Some(site) = mt
                .latest_filtered(None, Some(&sender), &standard, now, 1)
                .into_iter()
                .find_map(|f| f.website)
            else {
                println!("  {sender}: keine Website");
                continue;
            };
            let image = net::fetch_prefix(&agent, &site, 512 * 1024)
                .map(|p| og_image(&String::from_utf8_lossy(&p)));
            println!("  {sender}: {image:?}  ({site})");
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
