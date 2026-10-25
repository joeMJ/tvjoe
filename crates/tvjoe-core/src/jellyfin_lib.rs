//! Zwischenspeicher der Jellyfin-Bibliothek (SQLite im Cache): Filme, Sammlungen, Serien,
//! Staffeln, Folgen mit Gesehen-Stand. Abgleich voll (alle Einträge, gelöschte fallen weg)
//! oder nur Änderungen seit dem letzten Abgleich. Angaben zu Datei und Tonspuren holt die
//! Wiedergabe erst beim Abspielen.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::jellyfin::Account;

/// Bibliotheksarten, die Videos enthalten (Musik und Live-TV bleiben außen vor).
const VIDEO_KINDS: &[&str] = &["movies", "tvshows", "boxsets", "homevideos", "mixed", ""];
const PAGE: usize = 500;
const FIELDS: &str = "Overview,SortName,PremiereDate,ProductionYear,DateCreated,ParentId,Genres";
const TYPES: &str = "Movie,Series,Season,Episode,BoxSet,Video";

/// Bibliotheken, die standardmäßig ausgeblendet sind (Nutzer, 2026-10-25).
pub const HIDDEN_DEFAULT: &[&str] = &["Versteckt"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
}

/// Ein Eintrag der Bibliothek.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    /// Movie, Series, Season, Episode, BoxSet, Video.
    pub kind: String,
    pub name: String,
    pub sort_name: String,
    pub series_id: Option<String>,
    pub series_name: Option<String>,
    pub season_id: Option<String>,
    pub season_no: Option<i64>,
    pub episode_no: Option<i64>,
    pub year: Option<i64>,
    pub premiere: Option<String>,
    pub runtime_secs: i64,
    pub overview: String,
    pub genres: String,
    pub image_tag: Option<String>,
    pub date_created: Option<String>,
    pub played: bool,
    pub position_secs: f64,
    pub last_played: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub items: usize,
    pub collections: usize,
    pub removed: usize,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS libraries(id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS items(
    id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL, sort_name TEXT NOT NULL,
    series_id TEXT, series_name TEXT, season_id TEXT, season_no INTEGER, episode_no INTEGER,
    year INTEGER, premiere TEXT, runtime_secs INTEGER NOT NULL, overview TEXT NOT NULL,
    genres TEXT NOT NULL, image_tag TEXT, date_created TEXT, played INTEGER NOT NULL,
    position_secs REAL NOT NULL, last_played TEXT);
CREATE INDEX IF NOT EXISTS items_kind ON items(kind, date_created);
CREATE INDEX IF NOT EXISTS items_series ON items(series_id, season_no, episode_no);
CREATE TABLE IF NOT EXISTS item_libraries(item_id TEXT NOT NULL, library_id TEXT NOT NULL,
    PRIMARY KEY(item_id, library_id));
CREATE TABLE IF NOT EXISTS collection_items(collection_id TEXT NOT NULL, item_id TEXT NOT NULL,
    PRIMARY KEY(collection_id, item_id));
CREATE INDEX IF NOT EXISTS collection_items_item ON collection_items(item_id);
CREATE VIRTUAL TABLE IF NOT EXISTS items_fts USING fts5(
    name, series_name, overview, content='items', content_rowid='rowid',
    tokenize='unicode61 remove_diacritics 2');
CREATE TRIGGER IF NOT EXISTS items_ai AFTER INSERT ON items BEGIN
    INSERT INTO items_fts(rowid, name, series_name, overview)
    VALUES (new.rowid, new.name, coalesce(new.series_name, ''), new.overview); END;
CREATE TRIGGER IF NOT EXISTS items_ad AFTER DELETE ON items BEGIN
    INSERT INTO items_fts(items_fts, rowid, name, series_name, overview)
    VALUES ('delete', old.rowid, old.name, coalesce(old.series_name, ''), old.overview); END;
CREATE TRIGGER IF NOT EXISTS items_au AFTER UPDATE ON items BEGIN
    INSERT INTO items_fts(items_fts, rowid, name, series_name, overview)
    VALUES ('delete', old.rowid, old.name, coalesce(old.series_name, ''), old.overview);
    INSERT INTO items_fts(rowid, name, series_name, overview)
    VALUES (new.rowid, new.name, coalesce(new.series_name, ''), new.overview); END;
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

const COLUMNS: &str = "id, kind, name, sort_name, series_id, series_name, season_id, season_no, \
    episode_no, year, premiere, runtime_secs, overview, genres, image_tag, date_created, played, \
    position_secs, last_played";

fn db_err(e: rusqlite::Error) -> String {
    format!("Jellyfin-Zwischenspeicher: {e}")
}

fn row_to_item(r: &rusqlite::Row) -> rusqlite::Result<Item> {
    Ok(Item {
        id: r.get(0)?,
        kind: r.get(1)?,
        name: r.get(2)?,
        sort_name: r.get(3)?,
        series_id: r.get(4)?,
        series_name: r.get(5)?,
        season_id: r.get(6)?,
        season_no: r.get(7)?,
        episode_no: r.get(8)?,
        year: r.get(9)?,
        premiere: r.get(10)?,
        runtime_secs: r.get(11)?,
        overview: r.get(12)?,
        genres: r.get(13)?,
        image_tag: r.get(14)?,
        date_created: r.get(15)?,
        played: r.get::<_, i64>(16)? != 0,
        position_secs: r.get(17)?,
        last_played: r.get(18)?,
    })
}

/// Ein Eintrag aus der Antwort des Servers.
pub fn item_from_json(v: &Value) -> Option<Item> {
    let s = |k: &str| v[k].as_str().map(str::to_string);
    let ticks = |x: &Value| x.as_i64().unwrap_or(0);
    let ud = &v["UserData"];
    Some(Item {
        id: s("Id")?,
        kind: s("Type")?,
        name: s("Name").unwrap_or_default().trim().to_string(),
        sort_name: s("SortName").or_else(|| s("Name")).unwrap_or_default(),
        series_id: s("SeriesId"),
        series_name: s("SeriesName"),
        season_id: s("SeasonId"),
        season_no: v["ParentIndexNumber"].as_i64(),
        episode_no: v["IndexNumber"].as_i64(),
        year: v["ProductionYear"].as_i64(),
        premiere: s("PremiereDate"),
        runtime_secs: ticks(&v["RunTimeTicks"]) / 10_000_000,
        overview: s("Overview").unwrap_or_default(),
        genres: v["Genres"]
            .as_array()
            .map(|g| {
                g.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        image_tag: v["ImageTags"]["Primary"].as_str().map(str::to_string),
        date_created: s("DateCreated"),
        played: ud["Played"].as_bool().unwrap_or(false),
        position_secs: ticks(&ud["PlaybackPositionTicks"]) as f64 / 10_000_000.0,
        last_played: ud["LastPlayedDate"].as_str().map(str::to_string),
    })
}

pub struct JellyfinLibrary {
    conn: Connection,
}

impl JellyfinLibrary {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(db_err)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(db_err)?;
        conn.execute_batch(SCHEMA).map_err(db_err)?;
        Ok(Self { conn })
    }

    fn meta(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
            .map_err(db_err)
    }

    /// Zeitpunkt des letzten vollen Abgleichs (Sekunden seit 1970).
    pub fn last_full(&self) -> Option<i64> {
        self.meta("last_full").and_then(|v| v.parse().ok())
    }

    /// Zeitpunkt des letzten Abgleichs (voll oder Änderungen).
    pub fn last_sync(&self) -> Option<i64> {
        self.meta("last_sync").and_then(|v| v.parse().ok())
    }

    pub fn count(&self, kind: &str) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM items WHERE kind = ?1", [kind], |r| {
                r.get(0)
            })
            .unwrap_or(0)
    }

    pub fn libraries(&self) -> Vec<LibraryInfo> {
        let Ok(mut st) = self
            .conn
            .prepare("SELECT id, name, kind FROM libraries ORDER BY name")
        else {
            return Vec::new();
        };
        st.query_map([], |r| {
            Ok(LibraryInfo {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
            })
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    fn upsert(conn: &Connection, it: &Item) -> rusqlite::Result<usize> {
        conn.execute(
            &format!(
                "INSERT INTO items({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
                 ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, name = excluded.name,
                 sort_name = excluded.sort_name, series_id = excluded.series_id,
                 series_name = excluded.series_name, season_id = excluded.season_id,
                 season_no = excluded.season_no, episode_no = excluded.episode_no,
                 year = excluded.year, premiere = excluded.premiere,
                 runtime_secs = excluded.runtime_secs, overview = excluded.overview,
                 genres = excluded.genres, image_tag = excluded.image_tag,
                 date_created = excluded.date_created, played = excluded.played,
                 position_secs = excluded.position_secs, last_played = excluded.last_played"
            ),
            params![
                it.id,
                it.kind,
                it.name,
                it.sort_name,
                it.series_id,
                it.series_name,
                it.season_id,
                it.season_no,
                it.episode_no,
                it.year,
                it.premiere,
                it.runtime_secs,
                it.overview,
                it.genres,
                it.image_tag,
                it.date_created,
                it.played as i64,
                it.position_secs,
                it.last_played
            ],
        )
    }

    /// Voller Abgleich: alle Video-Bibliotheken seitenweise, Sammlungen mit ihren Teilen;
    /// Einträge, die es auf dem Server nicht mehr gibt, fallen weg.
    pub fn sync_full(&mut self, acc: &Account, now: i64) -> Result<SyncStats, String> {
        let views = acc.get_json(&format!("/Users/{}/Views", acc.user_id), &[])?;
        let libs: Vec<LibraryInfo> = views["Items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                Some(LibraryInfo {
                    id: v["Id"].as_str()?.to_string(),
                    name: v["Name"].as_str()?.to_string(),
                    kind: v["CollectionType"].as_str().unwrap_or("").to_string(),
                })
            })
            .filter(|l| VIDEO_KINDS.contains(&l.kind.as_str()))
            .collect();

        // Erst alles holen, dann in einem Rutsch schreiben (Leser sehen keinen Zwischenstand).
        let mut items: Vec<(String, Item)> = Vec::new();
        for lib in &libs {
            let mut start = 0;
            loop {
                let page = acc.get_json(
                    &format!("/Users/{}/Items", acc.user_id),
                    &[
                        ("ParentId", &lib.id),
                        ("Recursive", "true"),
                        ("IncludeItemTypes", TYPES),
                        ("Fields", FIELDS),
                        ("EnableUserData", "true"),
                        ("EnableImageTypes", "Primary"),
                        ("ImageTypeLimit", "1"),
                        ("StartIndex", &start.to_string()),
                        ("Limit", &PAGE.to_string()),
                    ],
                )?;
                let batch: Vec<Item> = page["Items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(item_from_json)
                    .collect();
                let n = batch.len();
                items.extend(batch.into_iter().map(|it| (lib.id.clone(), it)));
                start += n;
                let total = page["TotalRecordCount"].as_u64().unwrap_or(0) as usize;
                if n == 0 || start >= total {
                    break;
                }
            }
        }
        // Teile der Sammlungen (je Sammlung eine schlanke Abfrage).
        let mut collections: Vec<(String, String)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (_, it) in items.iter().filter(|(_, it)| it.kind == "BoxSet") {
            if !seen.insert(it.id.clone()) {
                continue;
            }
            let ch = acc.get_json(
                &format!("/Users/{}/Items", acc.user_id),
                &[
                    ("ParentId", &it.id),
                    ("EnableImages", "false"),
                    ("EnableUserData", "false"),
                ],
            )?;
            for c in ch["Items"].as_array().into_iter().flatten() {
                if let Some(id) = c["Id"].as_str() {
                    collections.push((it.id.clone(), id.to_string()));
                }
            }
        }

        let tx = self.conn.transaction().map_err(db_err)?;
        tx.execute_batch(
            "DELETE FROM libraries; DELETE FROM item_libraries; DELETE FROM collection_items;
             CREATE TEMP TABLE IF NOT EXISTS keep(id TEXT PRIMARY KEY); DELETE FROM temp.keep;",
        )
        .map_err(db_err)?;
        for l in &libs {
            tx.execute(
                "INSERT INTO libraries(id, name, kind) VALUES (?1, ?2, ?3)",
                params![l.id, l.name, l.kind],
            )
            .map_err(db_err)?;
        }
        for (lib, it) in &items {
            Self::upsert(&tx, it).map_err(db_err)?;
            tx.execute(
                "INSERT OR IGNORE INTO item_libraries(item_id, library_id) VALUES (?1, ?2)",
                params![it.id, lib],
            )
            .map_err(db_err)?;
            tx.execute("INSERT OR IGNORE INTO temp.keep(id) VALUES (?1)", [&it.id])
                .map_err(db_err)?;
        }
        for (c, i) in &collections {
            tx.execute(
                "INSERT OR IGNORE INTO collection_items(collection_id, item_id) VALUES (?1, ?2)",
                params![c, i],
            )
            .map_err(db_err)?;
        }
        let removed = tx
            .execute(
                "DELETE FROM items WHERE id NOT IN (SELECT id FROM temp.keep)",
                [],
            )
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        self.set_meta("last_full", &now.to_string())?;
        self.set_meta("last_sync", &now.to_string())?;
        let distinct: std::collections::HashSet<&str> =
            items.iter().map(|(_, it)| it.id.as_str()).collect();
        Ok(SyncStats {
            items: distinct.len(),
            collections: seen.len(),
            removed,
        })
    }

    /// Nur Änderungen seit dem letzten Abgleich: neue/geänderte Einträge und geänderter
    /// Gesehen-Stand (Jellyfin: `MinDateLastSaved` bzw. `MinDateLastSavedForUser`).
    pub fn sync_changes(&mut self, acc: &Account, now: i64) -> Result<usize, String> {
        let Some(since) = self.last_sync() else {
            return self.sync_full(acc, now).map(|s| s.items);
        };
        // Etwas Überlappung gegen Uhrabweichungen zwischen Server und Rechner.
        let since = iso8601(since - 600);
        let mut changed = 0;
        for param in ["MinDateLastSaved", "MinDateLastSavedForUser"] {
            let page = acc.get_json(
                &format!("/Users/{}/Items", acc.user_id),
                &[
                    ("Recursive", "true"),
                    ("IncludeItemTypes", TYPES),
                    ("Fields", FIELDS),
                    ("EnableUserData", "true"),
                    ("EnableImageTypes", "Primary"),
                    ("ImageTypeLimit", "1"),
                    (param, &since),
                ],
            )?;
            let tx = self.conn.transaction().map_err(db_err)?;
            for it in page["Items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(item_from_json)
            {
                Self::upsert(&tx, &it).map_err(db_err)?;
                changed += 1;
            }
            tx.commit().map_err(db_err)?;
        }
        self.set_meta("last_sync", &now.to_string())?;
        Ok(changed)
    }

    /// Gesehen-Stand lokal nachführen (nach dem Abspielen, bevor der Server abgeglichen ist).
    pub fn set_progress(&self, id: &str, position_secs: f64, played: bool) {
        let _ = self.conn.execute(
            "UPDATE items SET position_secs = ?2, played = ?3 WHERE id = ?1",
            params![id, position_secs, played as i64],
        );
    }

    pub fn item(&self, id: &str) -> Option<Item> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM items WHERE id = ?1"),
                [id],
                row_to_item,
            )
            .optional()
            .ok()
            .flatten()
    }

    fn query(&self, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Vec<Item> {
        let Ok(mut st) = self.conn.prepare(sql) else {
            return Vec::new();
        };
        st.query_map(args, row_to_item)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Bedingung „in einer sichtbaren Bibliothek“ (`hidden`: Namen ausgeblendeter Bibliotheken).
    fn visible(hidden: &[String]) -> String {
        if hidden.is_empty() {
            return String::new();
        }
        let list: Vec<String> = hidden
            .iter()
            .map(|h| format!("'{}'", h.replace('\'', "''")))
            .collect();
        format!(
            " AND EXISTS (SELECT 1 FROM item_libraries il JOIN libraries l ON l.id = il.library_id
               WHERE il.item_id = i.id AND l.name NOT IN ({}))",
            list.join(",")
        )
    }

    /// Einzelne Filme ohne Sammlung (Teile einer Sammlung stehen nur dort), neueste zuerst
    /// bzw. A–Z. `library`: nur diese Bibliothek (Name).
    pub fn single_movies(&self, hidden: &[String], library: Option<&str>, az: bool) -> Vec<Item> {
        self.movie_query(hidden, library, az, false)
    }

    /// Nur Sammlungen; „neueste“ nach dem zuletzt hinzugefügten Teil.
    pub fn collections(&self, hidden: &[String], az: bool) -> Vec<Item> {
        let order = if az {
            "i.sort_name COLLATE NOCASE".to_string()
        } else {
            "(SELECT max(m.date_created) FROM collection_items c JOIN items m ON m.id = c.item_id
               WHERE c.collection_id = i.id) DESC"
                .to_string()
        };
        let cols: Vec<String> = COLUMNS
            .split(", ")
            .map(|c| format!("i.{}", c.trim()))
            .collect();
        self.query(
            &format!(
                "SELECT {} FROM items i WHERE i.kind = 'BoxSet'{} ORDER BY {order}",
                cols.join(", "),
                Self::visible(hidden)
            ),
            &[],
        )
    }

    /// Filme und Sammlungen (eine Sammlung ersetzt ihre Teile), neueste zuerst bzw. A–Z.
    /// `library`: nur diese Bibliothek (Name).
    pub fn movies(&self, hidden: &[String], library: Option<&str>, az: bool) -> Vec<Item> {
        self.movie_query(hidden, library, az, true)
    }

    fn movie_query(
        &self,
        hidden: &[String],
        library: Option<&str>,
        az: bool,
        with_sets: bool,
    ) -> Vec<Item> {
        let order = if az {
            "i.sort_name COLLATE NOCASE"
        } else {
            "i.date_created DESC"
        };
        let lib = library
            .map(|l| {
                format!(
                    " AND EXISTS (SELECT 1 FROM item_libraries il JOIN libraries l ON l.id = il.library_id
                       WHERE il.item_id = i.id AND l.name = '{}')",
                    l.replace('\'', "''")
                )
            })
            .unwrap_or_default();
        let cols: Vec<String> = COLUMNS
            .split(", ")
            .map(|c| format!("i.{}", c.trim()))
            .collect();
        self.query(
            &format!(
                "SELECT {} FROM items i
                 WHERE ((i.kind IN ('Movie', 'Video')
                         AND NOT EXISTS (SELECT 1 FROM collection_items c WHERE c.item_id = i.id))
                        OR (?1 AND i.kind = 'BoxSet')){}{lib}
                 ORDER BY {order}",
                cols.join(", "),
                Self::visible(hidden)
            ),
            &[&with_sets],
        )
    }

    /// Teile einer Sammlung, nach Erscheinungsjahr.
    pub fn collection(&self, id: &str) -> Vec<Item> {
        let cols: Vec<String> = COLUMNS
            .split(", ")
            .map(|c| format!("i.{}", c.trim()))
            .collect();
        self.query(
            &format!(
                "SELECT {} FROM items i JOIN collection_items c ON c.item_id = i.id
                 WHERE c.collection_id = ?1 ORDER BY coalesce(i.year, 9999), i.premiere, i.sort_name",
                cols.join(", ")
            ),
            &[&id],
        )
    }

    /// Serien, zuletzt hinzugefügte zuerst bzw. A–Z.
    pub fn series(&self, hidden: &[String], az: bool) -> Vec<Item> {
        let order = if az {
            "i.sort_name COLLATE NOCASE"
        } else {
            "i.date_created DESC"
        };
        let cols: Vec<String> = COLUMNS
            .split(", ")
            .map(|c| format!("i.{}", c.trim()))
            .collect();
        self.query(
            &format!(
                "SELECT {} FROM items i WHERE i.kind = 'Series'{} ORDER BY {order}",
                cols.join(", "),
                Self::visible(hidden)
            ),
            &[],
        )
    }

    /// Anzahl der Folgen einer Serie.
    pub fn episode_count(&self, series_id: &str) -> i64 {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM items WHERE kind = 'Episode' AND series_id = ?1",
                [series_id],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    /// Anzahl der Teile einer Sammlung.
    pub fn collection_count(&self, id: &str) -> i64 {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM collection_items WHERE collection_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    /// Folgen einer Serie in Reihenfolge (Staffel, Folge).
    pub fn episodes(&self, series_id: &str) -> Vec<Item> {
        self.query(
            &format!(
                "SELECT {COLUMNS} FROM items WHERE kind = 'Episode' AND series_id = ?1
                 ORDER BY coalesce(season_no, 9999), coalesce(episode_no, 9999), premiere"
            ),
            &[&series_id],
        )
    }

    /// Volltextsuche über Titel, Serie und Beschreibung (Filme, Serien, Folgen).
    pub fn search(&self, text: &str, hidden: &[String], limit: usize) -> Vec<Item> {
        let query: String = text
            .split_whitespace()
            .map(|w| format!("\"{}\"*", w.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" ");
        if query.is_empty() {
            return Vec::new();
        }
        let cols: Vec<String> = COLUMNS
            .split(", ")
            .map(|c| format!("i.{}", c.trim()))
            .collect();
        self.query(
            &format!(
                "SELECT {} FROM items_fts JOIN items i ON i.rowid = items_fts.rowid
                 WHERE items_fts MATCH ?1 AND i.kind IN ('Movie', 'Video', 'Series', 'Episode', 'BoxSet'){}
                 ORDER BY rank LIMIT ?2",
                cols.join(", "),
                Self::visible(hidden)
            ),
            &[&query, &(limit as i64)],
        )
    }
}

/// Zeitpunkt im Format, das Jellyfin erwartet (UTC).
fn iso8601(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // Zivilkalender aus Tagen seit 1970 (Algorithmus von H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Echter Server: `TVJOE_TEST_JELLYFIN=/pfad/jellyfin.toml cargo test -p tvjoe-core
    /// --release -- --ignored --nocapture echter_server` (nur lesend).
    #[test]
    #[ignore]
    fn echter_server() {
        let Ok(path) = std::env::var("TVJOE_TEST_JELLYFIN") else {
            return;
        };
        let acc = Account::load(Path::new(&path)).expect("Zugang");
        let dir = std::env::temp_dir().join(format!("tvjoe-jfreal-{}", std::process::id()));
        let db = dir.join("jf.sqlite");
        let mut lib = JellyfinLibrary::open(&db).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let t = std::time::Instant::now();
        let st = lib.sync_full(&acc, now).unwrap();
        let size = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);
        println!(
            "Voller Abgleich: {st:?} in {:.1} s, Datenbank {:.1} MB",
            t.elapsed().as_secs_f64(),
            size as f64 / 1e6
        );
        for k in ["Movie", "BoxSet", "Series", "Season", "Episode", "Video"] {
            println!("  {k}: {}", lib.count(k));
        }
        println!(
            "  Bibliotheken: {:?}",
            lib.libraries()
                .iter()
                .map(|l| (&l.name, &l.kind))
                .collect::<Vec<_>>()
        );
        let hidden = vec!["Versteckt".to_string()];
        let t = std::time::Instant::now();
        let movies = lib.movies(&hidden, None, false);
        let series = lib.series(&hidden, false);
        println!(
            "Abfragen in {:.1} ms: {} Kacheln Filme/Sammlungen, {} Serien",
            t.elapsed().as_secs_f64() * 1e3,
            movies.len(),
            series.len()
        );
        if let Some(c) = movies.iter().find(|m| m.name.contains("Fast")) {
            println!(
                "  {} → {:?}",
                c.name,
                lib.collection(&c.id)
                    .iter()
                    .map(|i| (&i.name, i.year))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(s) = series.first() {
            let eps = lib.episodes(&s.id);
            println!(
                "  Serie {}: {} Folgen, erste {:?}",
                s.name,
                eps.len(),
                eps.first().map(|e| (&e.name, e.season_no, e.episode_no))
            );
        }
        let t = std::time::Instant::now();
        let changed = lib.sync_changes(&acc, now + 60).unwrap();
        println!(
            "Änderungsabgleich: {changed} Einträge in {:.1} s",
            t.elapsed().as_secs_f64()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zeit_im_jellyfin_format() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_792_886_400), "2026-10-25T00:00:00Z");
        assert_eq!(iso8601(951_782_400 + 3661), "2000-02-29T01:01:01Z");
    }

    #[test]
    fn eintrag_aus_json() {
        let v: Value = serde_json::from_str(
            r#"{"Id":"a","Type":"Episode","Name":" Pilot","SeriesId":"s","SeriesName":"Gilmore Girls",
            "SeasonId":"x","IndexNumber":1,"ParentIndexNumber":0,"RunTimeTicks":26000000000,
            "ImageTags":{"Primary":"t"},"UserData":{"Played":false,"PlaybackPositionTicks":21883348670}}"#,
        )
        .unwrap();
        let it = item_from_json(&v).unwrap();
        assert_eq!(it.name, "Pilot");
        assert_eq!(
            (it.season_no, it.episode_no, it.runtime_secs),
            (Some(0), Some(1), 2600)
        );
        assert!((it.position_secs - 2188.334867).abs() < 0.001);
        assert_eq!(it.image_tag.as_deref(), Some("t"));
    }

    #[test]
    fn sammlung_ersetzt_ihre_teile() {
        let dir = std::env::temp_dir().join(format!("tvjoe-jflib-{}", std::process::id()));
        let lib = JellyfinLibrary::open(&dir.join("jf.sqlite")).unwrap();
        let mk = |id: &str, kind: &str, name: &str, year: i64| Item {
            id: id.into(),
            kind: kind.into(),
            name: name.into(),
            sort_name: name.into(),
            series_id: None,
            series_name: None,
            season_id: None,
            season_no: None,
            episode_no: None,
            year: Some(year),
            premiere: None,
            runtime_secs: 0,
            overview: String::new(),
            genres: String::new(),
            image_tag: None,
            date_created: Some(format!("{year}-01-01")),
            played: false,
            position_secs: 0.0,
            last_played: None,
        };
        for it in [
            mk("t1", "Movie", "96 Hours", 2008),
            mk("t2", "Movie", "96 Hours - Taken 2", 2012),
            mk("c", "BoxSet", "96 Hours Filmreihe", 2014),
            mk("e", "Movie", "Versteckter Film", 2020),
            mk("x", "Movie", "Einzelfilm", 2010),
        ] {
            JellyfinLibrary::upsert(&lib.conn, &it).unwrap();
        }
        lib.conn
            .execute_batch(
                "INSERT INTO libraries VALUES ('L1','Filme','movies'), ('L2','Versteckt','movies');
                 INSERT INTO item_libraries VALUES ('t1','L1'),('t2','L1'),('c','L1'),('x','L1'),('e','L2');
                 INSERT INTO collection_items VALUES ('c','t2'),('c','t1');",
            )
            .unwrap();
        let hidden = vec!["Versteckt".to_string()];
        let names: Vec<String> = lib
            .movies(&hidden, None, true)
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(names, ["96 Hours Filmreihe", "Einzelfilm"]);
        assert_eq!(lib.movies(&[], None, true).len(), 3);
        let singles: Vec<String> = lib
            .single_movies(&hidden, None, true)
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(singles, ["Einzelfilm"]);
        assert_eq!(lib.collections(&hidden, false).len(), 1);
        let parts: Vec<String> = lib.collection("c").into_iter().map(|i| i.name).collect();
        assert_eq!(parts, ["96 Hours", "96 Hours - Taken 2"]);
        assert_eq!(lib.search("taken", &hidden, 10).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
