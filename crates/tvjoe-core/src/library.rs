//! Die Bibliothek (`library.toml`): Quellen, Sender mit Nutzereinstellungen, Streams je Qualität.
//!
//! Regeln (Projektplan, M2):
//! - Die Ziffernblocknummer ist eine eigene Nummer je Sender, getrennt von der Reihenfolge.
//! - Beim Aktualisieren kommen URLs, Logo und Name aus der Quelle; eigener Name, eigene Gruppe,
//!   Nummer, Favorit und „aktiv“ bleiben erhalten.
//! - Sender, die bei einer vollständigen Aktualisierung in der Quelle fehlen, bleiben erhalten
//!   und werden als `missing` markiert. Ein Teilimport markiert nichts.
//! - Derselbe Sender in zwei Quellen bleibt getrennt.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::check::channel_key;
use crate::ids::normalize_id;
use crate::m3u::{Entry, Playlist, Quality, SourceKind};
use crate::net::Variant;
use crate::settings::Fallback;
use crate::store::{StoreError, load_toml, save_toml};

pub const FORMAT_VERSION: u32 = 1;

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub id: u64,
    pub name: String,
    /// Datei oder URL der M3U.
    pub location: String,
    /// Sekunden seit 1970 beim letzten Import.
    pub last_import: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stream {
    pub quality: Option<Quality>,
    pub kind: Option<SourceKind>,
    pub url: String,
    /// Bei adaptiven HLS-Streams: höchste Bitrate für diese Stufe (mpv `hls-bitrate`);
    /// `None` = beste Stufe des Streams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bitrate: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// Stabile ID, bleibt bei Aktualisierungen gleich.
    pub id: u64,
    pub source: u64,
    /// Normalisierte `tvg-id` (sonst Name) zur Wiedererkennung.
    pub key: String,
    pub name: String,
    pub custom_name: Option<String>,
    pub tvg_id: Option<String>,
    /// Gruppe aus der Quelle.
    pub group: Option<String>,
    /// Eigene Gruppe, überschreibt `group`.
    pub custom_group: Option<String>,
    pub logo: Option<String>,
    /// Manuell gesetzte EPG-Kanal-ID (sonst automatische Zuordnung).
    pub epg_id: Option<String>,
    /// Ziffernblocknummer (in der Bibliothek eindeutig).
    pub number: Option<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub favorite: bool,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub active: bool,
    /// Platzhalter für Radiosender (nachrangig).
    #[serde(default, skip_serializing_if = "is_false")]
    pub radio: bool,
    /// Bei der letzten vollständigen Aktualisierung nicht mehr in der Quelle.
    #[serde(default, skip_serializing_if = "is_false")]
    pub missing: bool,
    #[serde(default, rename = "stream")]
    pub streams: Vec<Stream>,
}

impl Channel {
    pub fn display_name(&self) -> &str {
        self.custom_name.as_deref().unwrap_or(&self.name)
    }

    pub fn display_group(&self) -> Option<&str> {
        self.custom_group.as_deref().or(self.group.as_deref())
    }

    /// Wählt den Stream zur gewünschten Qualität.
    ///
    /// - `wanted = None`: die höchste Stufe.
    /// - Fehlt die Stufe: die nächstgelegene; bei Gleichstand entscheidet `fallback`.
    /// - Streams ohne Qualitätsangabe zählen nur, wenn kein Stream eine Qualität hat.
    pub fn pick_stream(&self, wanted: Option<Quality>, fallback: Fallback) -> Option<&Stream> {
        self.streams_by_preference(wanted, fallback)
            .into_iter()
            .next()
    }

    /// Alle Streams in der Reihenfolge, in der sie versucht werden (Ausweichen bei Fehlern):
    /// erst die Streams mit Qualitätsangabe nach Nähe zur Wunschstufe, dann die ohne Angabe.
    pub fn streams_by_preference(
        &self,
        wanted: Option<Quality>,
        fallback: Fallback,
    ) -> Vec<&Stream> {
        let mut known: Vec<&Stream> = self
            .streams
            .iter()
            .filter(|s| s.quality.is_some())
            .collect();
        match wanted {
            None => known.sort_by_key(|s| std::cmp::Reverse(s.quality)),
            Some(wanted) => known.sort_by_key(|s| {
                let diff = s.quality.map_or(0, |q| q as i32) - wanted as i32;
                let preferred = match fallback {
                    Fallback::Down => diff < 0,
                    Fallback::Up => diff > 0,
                };
                (diff.abs(), u8::from(!preferred))
            }),
        }
        known.extend(self.streams.iter().filter(|s| s.quality.is_none()));
        known
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportOutcome {
    pub added: usize,
    pub updated: usize,
    /// Neu als „fehlt in Quelle“ markiert.
    pub missing: usize,
}

/// Die Nummer ist schon bei einem anderen Sender vergeben.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumberTaken(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub version: u32,
    pub next_id: u64,
    #[serde(rename = "source", skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<Source>,
    /// Die Reihenfolge der Liste ist die Anzeigereihenfolge.
    #[serde(rename = "channel", skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<Channel>,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            next_id: 0,
            sources: Vec::new(),
            channels: Vec::new(),
        }
    }
}

/// Aus den Einträgen einer Quelle zusammengefasster Sender (alle Qualitäten).
struct Candidate {
    key: String,
    name: String,
    tvg_id: Option<String>,
    group: Option<String>,
    logo: Option<String>,
    chno: Option<u32>,
    streams: Vec<Stream>,
}

impl Candidate {
    fn new(key: String) -> Self {
        Self {
            key,
            name: String::new(),
            tvg_id: None,
            group: None,
            logo: None,
            chno: None,
            streams: Vec::new(),
        }
    }

    /// `variants`: beim Import erkannte Stufen des Streams (HLS-Master-Playlist).
    fn add(&mut self, e: &Entry, variants: &[Variant]) {
        if self.streams.is_empty() {
            self.name = e.attr("tvg-name").unwrap_or(&e.name).to_string();
        }
        self.tvg_id = self
            .tvg_id
            .take()
            .or_else(|| e.tvg_id().map(str::to_string));
        self.group = self.group.take().or_else(|| e.group().map(str::to_string));
        self.logo = self
            .logo
            .take()
            .or_else(|| e.tvg_logo().map(str::to_string));
        self.chno = self.chno.or_else(|| e.chno());
        let quality = e.quality();
        if quality.is_none() && !variants.is_empty() {
            self.add_variants(e, variants);
            return;
        }
        // Pro Qualitätsstufe nur ein Stream (der erste); ohne Angabe bleiben alle.
        if quality.is_some() && self.streams.iter().any(|s| s.quality == quality) {
            return;
        }
        self.streams.push(Stream {
            quality,
            kind: e.kind(),
            url: e.url.clone(),
            max_bitrate: e.attr("tvjoe-bitrate").and_then(|b| b.trim().parse().ok()),
        });
    }

    /// Ein adaptiver Stream wird je enthaltener Stufe ein eigener Stream mit derselben URL;
    /// die Stufe wählt mpv über die höchste Bitrate dieser Stufe. Die beste Stufe bleibt offen.
    fn add_variants(&mut self, e: &Entry, variants: &[Variant]) {
        let mut levels: std::collections::BTreeMap<Quality, u64> =
            std::collections::BTreeMap::new();
        for v in variants {
            let bw = levels.entry(Quality::from_height(v.height)).or_insert(0);
            *bw = (*bw).max(v.bandwidth);
        }
        let top = levels.keys().next_back().copied();
        for (q, bw) in levels {
            if self.streams.iter().any(|s| s.quality == Some(q)) {
                continue;
            }
            self.streams.push(Stream {
                quality: Some(q),
                kind: e.kind(),
                url: e.url.clone(),
                max_bitrate: (Some(q) != top).then_some(bw),
            });
        }
    }

    fn finish(mut self) -> Self {
        self.streams
            .sort_by_key(|s| (s.quality.is_none(), s.quality));
        self
    }
}

impl Library {
    pub fn load(path: &Path) -> Result<Self, StoreError> {
        let lib: Self = load_toml(path)?;
        if lib.version > FORMAT_VERSION {
            return Err(StoreError::TooNew(lib.version));
        }
        Ok(lib)
    }

    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        save_toml(path, self)
    }

    fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn add_source(&mut self, name: &str, location: &str) -> u64 {
        let id = self.alloc_id();
        self.sources.push(Source {
            id,
            name: name.to_string(),
            location: location.to_string(),
            last_import: None,
        });
        id
    }

    /// Entfernt die Quelle samt ihren Sendern.
    pub fn remove_source(&mut self, id: u64) -> bool {
        let before = self.sources.len();
        self.sources.retain(|s| s.id != id);
        self.channels.retain(|c| c.source != id);
        self.sources.len() != before
    }

    pub fn source(&self, id: u64) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    pub fn channel(&self, id: u64) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn channel_mut(&mut self, id: u64) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.id == id)
    }

    pub fn by_number(&self, number: u32) -> Option<&Channel> {
        self.channels.iter().find(|c| c.number == Some(number))
    }

    /// Setzt die Ziffernblocknummer; schlägt fehl, wenn sie bei einem anderen Sender vergeben ist.
    pub fn set_number(&mut self, id: u64, number: u32) -> Result<(), NumberTaken> {
        if let Some(other) = self.by_number(number).filter(|c| c.id != id) {
            return Err(NumberTaken(other.id));
        }
        if let Some(c) = self.channel_mut(id) {
            c.number = Some(number);
        }
        Ok(())
    }

    /// Verschiebt einen Sender an eine Position der Anzeigereihenfolge (Nummern bleiben unverändert).
    pub fn move_channel(&mut self, id: u64, to: usize) -> bool {
        let Some(from) = self.channels.iter().position(|c| c.id == id) else {
            return false;
        };
        let c = self.channels.remove(from);
        let to = to.min(self.channels.len());
        self.channels.insert(to, c);
        true
    }

    /// Verschiebt einen Sender vor (oder hinter) einen anderen (Ziehen und Ablegen).
    pub fn move_relative(&mut self, id: u64, target: u64, after: bool) -> bool {
        if id == target {
            return false;
        }
        let Some(from) = self.channels.iter().position(|c| c.id == id) else {
            return false;
        };
        let c = self.channels.remove(from);
        let Some(t) = self.channels.iter().position(|c| c.id == target) else {
            self.channels.insert(from, c);
            return false;
        };
        self.channels.insert(if after { t + 1 } else { t }, c);
        true
    }

    /// Nummern nach Position neu vergeben (1, 2, 3 … in der Anzeigereihenfolge), wie bei EyeTV.
    pub fn renumber(&mut self) {
        for (i, c) in self.channels.iter_mut().enumerate() {
            c.number = Some(i as u32 + 1);
        }
    }

    pub fn remove_channel(&mut self, id: u64) -> bool {
        let before = self.channels.len();
        self.channels.retain(|c| c.id != id);
        self.channels.len() != before
    }

    /// Exportiert Sender als M3U8: je Stream ein Eintrag mit `tvjoe-type`/`tvjoe-quality`,
    /// eigene Namen und Gruppen werden übernommen. `ids = None` exportiert alle.
    pub fn export_m3u(&self, ids: Option<&[u64]>) -> String {
        let mut out = String::from("#EXTM3U\n");
        for ch in self
            .channels
            .iter()
            .filter(|c| ids.is_none_or(|ids| ids.contains(&c.id)))
        {
            for s in &ch.streams {
                let mut line = String::from("#EXTINF:-1");
                let mut add = |k: &str, v: Option<&str>| {
                    if let Some(v) = v {
                        line.push_str(&format!(" {k}=\"{}\"", v.replace('"', "'")));
                    }
                };
                add("tvg-chno", ch.number.map(|n| n.to_string()).as_deref());
                add("tvg-id", ch.tvg_id.as_deref());
                add("tvg-name", Some(ch.display_name()));
                add("tvg-logo", ch.logo.as_deref());
                add("group-title", ch.display_group());
                add(
                    "tvjoe-type",
                    s.kind.map(|k| match k {
                        SourceKind::Hls => "hls",
                        SourceKind::SatIp => "satip",
                    }),
                );
                add("tvjoe-quality", s.quality.map(Quality::as_attr));
                add(
                    "tvjoe-bitrate",
                    s.max_bitrate.map(|b| b.to_string()).as_deref(),
                );
                let name = match s.quality {
                    Some(q) if ch.streams.len() > 1 => {
                        format!("{} {}", ch.display_name(), q.label())
                    }
                    _ => ch.display_name().to_string(),
                };
                out.push_str(&line);
                out.push(',');
                out.push_str(&name.replace(['\n', '\r'], " "));
                out.push('\n');
                out.push_str(&s.url);
                out.push('\n');
            }
        }
        out
    }

    /// Gewünschte Nummer, wenn frei, sonst die nächste freie über der höchsten vergebenen.
    fn free_number(&self, desired: Option<u32>) -> u32 {
        let used: HashSet<u32> = self.channels.iter().filter_map(|c| c.number).collect();
        if let Some(n) = desired.filter(|n| !used.contains(n)) {
            return n;
        }
        used.into_iter().max().unwrap_or(0) + 1
    }

    /// Wiedererkennung innerhalb derselben Quelle: Schlüssel, dann URL, dann Name.
    fn find_existing(
        &self,
        source: u64,
        cand: &Candidate,
        claimed: &HashSet<u64>,
    ) -> Option<usize> {
        let mine = |c: &Channel| c.source == source && !claimed.contains(&c.id);
        let by =
            |pred: &dyn Fn(&Channel) -> bool| self.channels.iter().position(|c| mine(c) && pred(c));
        by(&|c| c.key == cand.key)
            .or_else(|| {
                by(&|c| {
                    c.streams
                        .iter()
                        .any(|s| cand.streams.iter().any(|n| n.url == s.url))
                })
            })
            .or_else(|| {
                let n = normalize_id(&cand.name);
                by(&|c| normalize_id(&c.name) == n)
            })
    }

    /// Übernimmt Einträge einer geprüften M3U.
    ///
    /// `selection`: Indizes in `playlist.entries` (Teilimport), `None` = alle (vollständige
    /// Aktualisierung). Einträge mit Fehlern werden nie übernommen. `now` in Sekunden seit 1970.
    /// Gibt `None` zurück, wenn die Quelle unbekannt ist.
    pub fn import(
        &mut self,
        source: u64,
        playlist: &Playlist,
        selection: Option<&[usize]>,
        now: u64,
    ) -> Option<ImportOutcome> {
        self.import_with(source, playlist, selection, now, &HashMap::new())
    }

    /// Wie [`Library::import`], mit beim Import erkannten Stufen je Eintrag (Index → Varianten).
    /// Einträge ohne `tvjoe-quality` bekommen daraus ihre Stufen (adaptive Streams: mehrere).
    pub fn import_with(
        &mut self,
        source: u64,
        playlist: &Playlist,
        selection: Option<&[usize]>,
        now: u64,
        variants: &HashMap<usize, Vec<Variant>>,
    ) -> Option<ImportOutcome> {
        self.source(source)?;

        let mut idxs: Vec<usize> = match selection {
            Some(s) => s.to_vec(),
            None => (0..playlist.entries.len()).collect(),
        };
        idxs.retain(|&i| i < playlist.entries.len() && !playlist.has_errors_for(i));
        idxs.sort_unstable();
        idxs.dedup();

        let mut cands: Vec<Candidate> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for i in idxs {
            let e = &playlist.entries[i];
            let key = channel_key(e);
            let pos = *index.entry(key.clone()).or_insert_with(|| {
                cands.push(Candidate::new(key));
                cands.len() - 1
            });
            cands[pos].add(e, variants.get(&i).map(Vec::as_slice).unwrap_or(&[]));
        }

        let mut out = ImportOutcome::default();
        let mut claimed: HashSet<u64> = HashSet::new();
        for cand in cands.into_iter().map(Candidate::finish) {
            match self.find_existing(source, &cand, &claimed) {
                Some(pos) => {
                    let number = self.channels[pos]
                        .number
                        .is_none()
                        .then(|| self.free_number(cand.chno));
                    let ch = &mut self.channels[pos];
                    ch.key = cand.key;
                    ch.name = cand.name;
                    ch.tvg_id = cand.tvg_id;
                    ch.group = cand.group;
                    ch.logo = cand.logo;
                    ch.streams = cand.streams;
                    ch.missing = false;
                    if number.is_some() {
                        ch.number = number;
                    }
                    claimed.insert(ch.id);
                    out.updated += 1;
                }
                None => {
                    let id = self.alloc_id();
                    let number = Some(self.free_number(cand.chno));
                    self.channels.push(Channel {
                        id,
                        source,
                        key: cand.key,
                        name: cand.name,
                        custom_name: None,
                        tvg_id: cand.tvg_id,
                        group: cand.group,
                        custom_group: None,
                        logo: cand.logo,
                        epg_id: None,
                        number,
                        favorite: false,
                        active: true,
                        radio: false,
                        missing: false,
                        streams: cand.streams,
                    });
                    claimed.insert(id);
                    out.added += 1;
                }
            }
        }

        if selection.is_none() {
            for c in self
                .channels
                .iter_mut()
                .filter(|c| c.source == source && !claimed.contains(&c.id))
            {
                if !c.missing {
                    c.missing = true;
                    out.missing += 1;
                }
            }
        }
        if let Some(s) = self.sources.iter_mut().find(|s| s.id == source) {
            s.last_import = Some(now);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zeilen: (Kanalnummer, tvg-id, Qualität, URL); leere Texte lassen das Attribut weg.
    fn pl(rows: &[(&str, &str, &str, &str)]) -> Playlist {
        let mut t = String::from("#EXTM3U\n");
        for (chno, id, q, url) in rows {
            t.push_str("#EXTINF:-1");
            if !chno.is_empty() {
                t.push_str(&format!(" tvg-chno=\"{chno}\""));
            }
            t.push_str(&format!(" tvg-id=\"{id}\""));
            if !q.is_empty() {
                t.push_str(&format!(" tvjoe-quality=\"{q}\""));
            }
            t.push_str(&format!(",{id}\n{url}\n"));
        }
        Playlist::parse(t.as_bytes()).unwrap()
    }

    fn base() -> Playlist {
        pl(&[
            ("1", "Eins.de", "720", "https://x.invalid/e-720.m3u8"),
            ("1", "Eins.de", "sd", "https://x.invalid/e-sd.m3u8"),
            ("2", "Zwei.de", "1080", "https://x.invalid/z.m3u8"),
        ])
    }

    fn name<'a>(lib: &'a Library, n: &str) -> &'a Channel {
        lib.channels.iter().find(|c| c.name == n).unwrap()
    }

    #[test]
    fn import_fasst_qualitaeten_zu_einem_sender_zusammen() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        let out = lib.import(src, &base(), None, 100).unwrap();
        assert_eq!(
            out,
            ImportOutcome {
                added: 2,
                updated: 0,
                missing: 0
            }
        );
        let eins = name(&lib, "Eins.de");
        assert_eq!(eins.number, Some(1));
        let q: Vec<_> = eins.streams.iter().map(|s| s.quality).collect();
        assert_eq!(q, [Some(Quality::Sd), Some(Quality::P720)]);
        assert_eq!(name(&lib, "Zwei.de").number, Some(2));
        assert_eq!(lib.source(src).unwrap().last_import, Some(100));
    }

    #[test]
    fn aktualisieren_erhaelt_nutzereinstellungen() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        lib.import(src, &base(), None, 1).unwrap();
        let id = name(&lib, "Eins.de").id;
        {
            let c = lib.channel_mut(id).unwrap();
            c.custom_name = Some("Mein Eins".into());
            c.custom_group = Some("Favoriten".into());
            c.favorite = true;
            c.active = false;
        }
        lib.set_number(id, 10).unwrap();

        let neu = pl(&[
            ("1", "Eins.de", "sd", "https://x.invalid/e2-sd.m3u8"),
            ("2", "Zwei.de", "1080", "https://x.invalid/z.m3u8"),
        ]);
        let out = lib.import(src, &neu, None, 2).unwrap();
        assert_eq!(
            out,
            ImportOutcome {
                added: 0,
                updated: 2,
                missing: 0
            }
        );

        let c = lib.channel(id).unwrap();
        assert_eq!(c.display_name(), "Mein Eins");
        assert_eq!(c.display_group(), Some("Favoriten"));
        assert!(c.favorite && !c.active);
        assert_eq!(c.number, Some(10));
        assert_eq!(c.streams.len(), 1);
        assert_eq!(c.streams[0].url, "https://x.invalid/e2-sd.m3u8");
        assert_eq!(lib.channels.len(), 2);
    }

    #[test]
    fn fehlende_sender_werden_markiert_nicht_geloescht() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        lib.import(src, &base(), None, 1).unwrap();
        let nur_eins = pl(&[("1", "Eins.de", "sd", "https://x.invalid/e-sd.m3u8")]);
        let out = lib.import(src, &nur_eins, None, 2).unwrap();
        assert_eq!(out.missing, 1);
        assert!(name(&lib, "Zwei.de").missing);
        assert_eq!(lib.channels.len(), 2);

        // Wieder in der Quelle: Markierung verschwindet.
        lib.import(src, &base(), None, 3).unwrap();
        assert!(!name(&lib, "Zwei.de").missing);
    }

    #[test]
    fn teilimport_markiert_nichts() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        lib.import(src, &base(), None, 1).unwrap();
        // Nur der erste Eintrag (Eins, 720) wird aktualisiert.
        let out = lib.import(src, &base(), Some(&[0]), 2).unwrap();
        assert_eq!(out.missing, 0);
        assert!(!name(&lib, "Zwei.de").missing);
        // Erster Import als Teilimport: nur ausgewählte Sender entstehen.
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        let out = lib.import(src, &base(), Some(&[2]), 1).unwrap();
        assert_eq!(out.added, 1);
        assert_eq!(lib.channels[0].name, "Zwei.de");
    }

    #[test]
    fn nummern_sind_ueber_quellen_eindeutig() {
        let mut lib = Library::default();
        let a = lib.add_source("A", "a.m3u");
        let b = lib.add_source("B", "b.m3u");
        lib.import(a, &base(), None, 1).unwrap();
        let drei = pl(&[("1", "Drei.de", "sd", "https://x.invalid/d.m3u8")]);
        lib.import(b, &drei, None, 1).unwrap();
        // 1 und 2 sind vergeben: Drei bekommt die nächste freie Nummer.
        assert_eq!(name(&lib, "Drei.de").number, Some(3));
        // Gleicher Sender in zwei Quellen bleibt getrennt.
        lib.import(b, &base(), None, 2).unwrap();
        assert_eq!(
            lib.channels.iter().filter(|c| c.name == "Eins.de").count(),
            2
        );
        let nums: HashSet<_> = lib.channels.iter().filter_map(|c| c.number).collect();
        assert_eq!(nums.len(), lib.channels.len());
    }

    #[test]
    fn ohne_kanalnummer_wird_fortlaufend_vergeben() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        let p = pl(&[
            ("", "A.de", "", "https://x.invalid/a.m3u8"),
            ("", "B.de", "", "https://x.invalid/b.m3u8"),
        ]);
        lib.import(src, &p, None, 1).unwrap();
        assert_eq!(name(&lib, "A.de").number, Some(1));
        assert_eq!(name(&lib, "B.de").number, Some(2));
    }

    #[test]
    fn wiedererkennung_ueber_url_wenn_der_name_wechselt() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        let alt =
            Playlist::parse(b"#EXTM3U\n#EXTINF:-1,Alt Name\nhttps://x.invalid/a.m3u8\n").unwrap();
        let neu =
            Playlist::parse(b"#EXTM3U\n#EXTINF:-1,Neuer Name\nhttps://x.invalid/a.m3u8\n").unwrap();
        lib.import(src, &alt, None, 1).unwrap();
        let id = lib.channels[0].id;
        let out = lib.import(src, &neu, None, 2).unwrap();
        assert_eq!(out.updated, 1);
        assert_eq!(lib.channels.len(), 1);
        assert_eq!(lib.channels[0].id, id);
        assert_eq!(lib.channels[0].name, "Neuer Name");
    }

    #[test]
    fn unbekannte_quelle_wird_abgelehnt() {
        let mut lib = Library::default();
        assert!(lib.import(99, &base(), None, 1).is_none());
    }

    #[test]
    fn reihenfolge_und_nummern_sind_getrennt() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        lib.import(src, &base(), None, 1).unwrap();
        let zwei = name(&lib, "Zwei.de").id;
        assert!(lib.move_channel(zwei, 0));
        assert_eq!(lib.channels[0].name, "Zwei.de");
        assert_eq!(lib.channels[0].number, Some(2));
        assert!(!lib.move_channel(999, 0));
        let eins = name(&lib, "Eins.de").id;
        assert_eq!(lib.set_number(eins, 2), Err(NumberTaken(zwei)));
        assert_eq!(lib.by_number(2).unwrap().id, zwei);
    }

    #[test]
    fn quelle_entfernen_loescht_ihre_sender() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        lib.import(src, &base(), None, 1).unwrap();
        assert!(lib.remove_source(src));
        assert!(lib.channels.is_empty() && lib.sources.is_empty());
    }

    fn channel_with(qs: &[Option<Quality>]) -> Channel {
        Channel {
            id: 1,
            source: 1,
            key: "k".into(),
            name: "K".into(),
            custom_name: None,
            tvg_id: None,
            group: None,
            custom_group: None,
            logo: None,
            epg_id: None,
            number: Some(1),
            favorite: false,
            active: true,
            radio: false,
            missing: false,
            streams: qs
                .iter()
                .enumerate()
                .map(|(i, q)| Stream {
                    quality: *q,
                    kind: None,
                    url: format!("u{i}"),
                    max_bitrate: None,
                })
                .collect(),
        }
    }

    #[test]
    fn stufenwahl_mit_ausweichen() {
        use Quality::*;
        let c = channel_with(&[Some(Sd), Some(P1080)]);
        let url = |q, f| c.pick_stream(q, f).unwrap().url.clone();
        // Gleichstand zwischen SD und 1080p bei Wunsch 720p.
        assert_eq!(url(Some(P720), Fallback::Down), "u0");
        assert_eq!(url(Some(P720), Fallback::Up), "u1");
        // Vorhandene Stufe wird genommen.
        assert_eq!(url(Some(Sd), Fallback::Up), "u0");
        // Über dem Angebot: die nächste darunter; ohne Wunsch: die höchste.
        assert_eq!(url(Some(Uhd), Fallback::Down), "u1");
        assert_eq!(url(None, Fallback::Down), "u1");
    }

    #[test]
    fn adaptiver_stream_wird_in_stufen_geteilt() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        let p = pl(&[
            ("1", "Eins.de", "", "https://x.invalid/eins/master.m3u8"),
            ("2", "Zwei.de", "", "https://x.invalid/zwei/master.m3u8"),
            ("3", "Drei.de", "720", "https://x.invalid/drei/master.m3u8"),
        ]);
        let v = |h, b| Variant {
            height: h,
            bandwidth: b,
        };
        let mut variants = HashMap::new();
        // Eins: 270p…1080p wie Das Erste; Zwei: nur 720p wie ZDF.
        variants.insert(
            0,
            vec![
                v(270, 385_000),
                v(540, 880_000),
                v(720, 1_760_000),
                v(720, 2_675_200),
                v(1080, 4_180_000),
            ],
        );
        variants.insert(1, vec![v(720, 3_544_154)]);
        // Ausdrückliche Angabe in der M3U hat Vorrang vor der Erkennung.
        variants.insert(2, vec![v(1080, 5_000_000)]);
        lib.import_with(src, &p, None, 1, &variants).unwrap();

        let eins = name(&lib, "Eins.de");
        let stufen: Vec<_> = eins
            .streams
            .iter()
            .map(|s| (s.quality, s.max_bitrate))
            .collect();
        assert_eq!(
            stufen,
            [
                (Some(Quality::Sd), Some(880_000)),
                (Some(Quality::P720), Some(2_675_200)),
                (Some(Quality::P1080), None),
            ]
        );
        assert!(
            eins.streams
                .iter()
                .all(|s| s.url == "https://x.invalid/eins/master.m3u8")
        );
        let zwei = name(&lib, "Zwei.de");
        assert_eq!(zwei.streams.len(), 1);
        assert_eq!(
            (zwei.streams[0].quality, zwei.streams[0].max_bitrate),
            (Some(Quality::P720), None)
        );
        let drei = name(&lib, "Drei.de");
        assert_eq!(
            (drei.streams.len(), drei.streams[0].quality),
            (1, Some(Quality::P720))
        );
        // Wunsch 720p trifft beim adaptiven Stream die 720p-Stufe.
        let s = eins
            .pick_stream(Some(Quality::P720), Fallback::Down)
            .unwrap();
        assert_eq!(s.max_bitrate, Some(2_675_200));

        // Export und Wiedereinlesen behalten Stufen und Bitraten, ohne Befund „URL doppelt“.
        let text = lib.export_m3u(Some(&[eins.id]));
        let p2 = Playlist::parse(text.as_bytes()).unwrap();
        assert!(
            p2.findings
                .iter()
                .all(|f| f.kind != crate::check::FindingKind::DuplicateUrl),
            "{:?}",
            p2.findings
        );
        let mut neu = Library::default();
        let s2 = neu.add_source("Export", "export.m3u");
        neu.import(s2, &p2, None, 2).unwrap();
        assert_eq!(neu.channels[0].streams, name(&lib, "Eins.de").streams);
    }

    #[test]
    fn export_und_wiedereinlesen() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        lib.import(src, &base(), None, 1).unwrap();
        let id = name(&lib, "Eins.de").id;
        lib.channel_mut(id).unwrap().custom_name = Some("Mein \"Eins\"".into());
        let text = lib.export_m3u(None);
        assert!(text.contains("tvjoe-quality=\"720\""), "{text}");

        let p = Playlist::parse(text.as_bytes()).unwrap();
        assert_eq!(p.entries.len(), 3);
        let mut neu = Library::default();
        let s2 = neu.add_source("Export", "export.m3u");
        neu.import(s2, &p, None, 2).unwrap();
        assert_eq!(neu.channels.len(), 2);
        let eins = neu
            .channels
            .iter()
            .find(|c| c.tvg_id.as_deref() == Some("Eins.de"))
            .unwrap();
        assert_eq!(eins.name, "Mein 'Eins'");
        assert_eq!(eins.number, Some(1));
        assert_eq!(eins.streams.len(), 2);
        // Nur ausgewählte Sender.
        let nur = lib.export_m3u(Some(&[id]));
        assert_eq!(nur.matches("#EXTINF").count(), 2);
    }

    #[test]
    fn ziehen_und_neu_nummerieren() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        let p = pl(&[
            ("1", "A.de", "sd", "https://x.invalid/a.m3u8"),
            ("2", "B.de", "sd", "https://x.invalid/b.m3u8"),
            ("3", "C.de", "sd", "https://x.invalid/c.m3u8"),
        ]);
        lib.import(src, &p, None, 1).unwrap();
        let id = |lib: &Library, n: &str| name(lib, n).id;
        let (a, b, c) = (id(&lib, "A.de"), id(&lib, "B.de"), id(&lib, "C.de"));
        // C vor A ziehen.
        assert!(lib.move_relative(c, a, false));
        let order: Vec<_> = lib.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(order, ["C.de", "A.de", "B.de"]);
        // A hinter B ziehen.
        assert!(lib.move_relative(a, b, true));
        let order: Vec<_> = lib.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(order, ["C.de", "B.de", "A.de"]);
        assert!(!lib.move_relative(a, a, false));
        assert!(!lib.move_relative(a, 999, false));
        assert_eq!(lib.channels.len(), 3);
        lib.renumber();
        let nums: Vec<_> = lib
            .channels
            .iter()
            .map(|c| (c.name.as_str(), c.number))
            .collect();
        assert_eq!(
            nums,
            [("C.de", Some(1)), ("B.de", Some(2)), ("A.de", Some(3))]
        );
    }

    #[test]
    fn sender_loeschen() {
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        lib.import(src, &base(), None, 1).unwrap();
        let id = name(&lib, "Zwei.de").id;
        assert!(lib.remove_channel(id));
        assert!(!lib.remove_channel(id));
        assert_eq!(lib.channels.len(), 1);
    }

    #[test]
    fn reihenfolge_beim_ausweichen() {
        use Quality::*;
        let c = channel_with(&[None, Some(Sd), Some(P1080), Some(Uhd)]);
        let urls = |q, f| -> Vec<String> {
            c.streams_by_preference(q, f)
                .iter()
                .map(|s| s.url.clone())
                .collect()
        };
        // Wunsch 720p, nach unten: SD, dann 1080p, dann UHD, zuletzt ohne Angabe.
        assert_eq!(urls(Some(P720), Fallback::Down), ["u1", "u2", "u3", "u0"]);
        assert_eq!(urls(Some(P720), Fallback::Up), ["u2", "u1", "u3", "u0"]);
        assert_eq!(urls(None, Fallback::Down), ["u3", "u2", "u1", "u0"]);
    }

    #[test]
    fn stufenwahl_ohne_qualitaetsangaben() {
        let c = channel_with(&[None, None]);
        assert_eq!(
            c.pick_stream(Some(Quality::P720), Fallback::Down)
                .unwrap()
                .url,
            "u0"
        );
        assert!(
            channel_with(&[])
                .pick_stream(None, Fallback::Down)
                .is_none()
        );
        // Unbekannte Streams zählen nicht, sobald es bekannte gibt.
        let c = channel_with(&[None, Some(Quality::Sd)]);
        assert_eq!(
            c.pick_stream(Some(Quality::Uhd), Fallback::Down)
                .unwrap()
                .url,
            "u1"
        );
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tvjoe-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn toml_hin_und_zurueck_mit_sicherung() {
        let dir = tmp("lib");
        let path = dir.join("sub").join("library.toml");
        let mut lib = Library::default();
        let src = lib.add_source("Test", "test.m3u");
        lib.import(src, &base(), None, 5).unwrap();
        let id = name(&lib, "Eins.de").id;
        lib.channel_mut(id).unwrap().favorite = true;

        lib.save(&path).unwrap();
        assert_eq!(Library::load(&path).unwrap(), lib);
        assert!(!path.with_file_name("library.toml.bak").exists());
        lib.save(&path).unwrap();
        assert!(path.with_file_name("library.toml.bak").exists());
        assert!(!path.with_file_name("library.toml.tmp").exists());

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[[channel]]") && text.contains("[[channel.stream]]"),
            "{text}"
        );
        assert!(text.contains("quality = \"720\""), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn laden_fehlende_kaputte_und_neue_datei() {
        let dir = tmp("lib-err");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            Library::load(&dir.join("fehlt.toml")).unwrap(),
            Library::default()
        );

        let kaputt = dir.join("kaputt.toml");
        std::fs::write(&kaputt, "das ist [kein toml").unwrap();
        assert!(matches!(Library::load(&kaputt), Err(StoreError::Parse(_))));

        let neu = dir.join("neu.toml");
        std::fs::write(&neu, "version = 99\n").unwrap();
        assert!(matches!(Library::load(&neu), Err(StoreError::TooNew(99))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
