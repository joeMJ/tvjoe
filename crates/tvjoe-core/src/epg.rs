//! EPG aus XMLTV (Datei oder URL, auch gzip-komprimiert).
//!
//! Zuordnung zu den Sendern (Projektplan M6): manuell gesetzte `epg_id`, sonst normalisierte
//! `tvg-id` gegen die XMLTV-Kanal-ID, sonst normalisierter Name (auch ohne „HD“) gegen die
//! Anzeigenamen. Zeiten werden als Sekunden seit 1970 (UTC) gehalten.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use quick_xml::Reader;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::Event;

use crate::ids::normalize_id;
use crate::library::Channel;
use crate::net;
use crate::pruefung::{self, HostClass};
use crate::{tf, tr, trn};

const MAX_XMLTV_BYTES: u64 = 256 * 1024 * 1024;
/// Ab diesen Mengen meldet das Einlesen eine Warnung (übernommen wird trotzdem).
const MANY_CHANNELS: usize = 20_000;
const MANY_PROGRAMMES: usize = 2_000_000;
/// Längste plausible Sendung.
const MAX_DURATION: i64 = 24 * 3_600;
/// Sendungen vor 2000 oder weiter als das in der Zukunft werden verworfen.
const MIN_START: i64 = 946_684_800;
const MAX_AHEAD: i64 = 366 * 86_400;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Programme {
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub subtitle: Option<String>,
    pub desc: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Guide {
    /// XMLTV-Kanäle in Dateireihenfolge: (ID, Anzeigenamen).
    pub channels: Vec<(String, Vec<String>)>,
    /// Sendungen je Kanal-ID, nach Beginn sortiert.
    pub programmes: HashMap<String, Vec<Programme>>,
    /// Senderlogo je Kanal-ID (`<icon src>` im `<channel>`).
    pub icons: HashMap<String, String>,
    /// Befunde der Sicherheitsprüfung für die Anzeige im EPG: verworfene Einträge, ungewöhnliche
    /// Mengen; nach [`load_sources`] auch abgewiesene Quellen, je mit vorangestellter Quelle.
    pub warnings: Vec<String>,
    /// Anzahl gesäuberter Texte (Steuerzeichen, Länge); nur fürs Protokoll.
    pub cleaned: usize,
    by_id: HashMap<String, String>,
    by_name: HashMap<String, String>,
    /// Kurze Kürzel (2–3 Zeichen) der Kanal-IDs für die unscharfe Zuordnung, z. B. „swr“, „sr“.
    by_stem: Vec<(String, String)>,
}

/// Wie ein Sender zugeordnet wurde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Manual,
    TvgId,
    Name,
    /// Unscharf: das erste Wort des Namens ist das Kürzel eines EPG-Kanals (z. B. „MDR Sachsen“).
    Fuzzy,
}

fn strip_hd(n: &str) -> &str {
    n.strip_suffix("hd").filter(|s| !s.is_empty()).unwrap_or(n)
}

impl Guide {
    fn index(&mut self) {
        self.by_id.clear();
        self.by_name.clear();
        self.by_stem.clear();
        for (id, names) in &self.channels {
            // „SWR/SR.de“ → „swr“, „sr“; Länderkürzel am Ende entfällt.
            let base = match id.rsplit_once('.') {
                Some((b, suffix)) if suffix.len() <= 3 => b,
                _ => id.as_str(),
            };
            for part in base.split('/') {
                let stem = normalize_id(part);
                if (2..=3).contains(&stem.chars().count()) {
                    self.by_stem.push((stem, id.clone()));
                }
            }
            self.by_id
                .entry(normalize_id(id))
                .or_insert_with(|| id.clone());
            for n in names {
                let n = normalize_id(n);
                self.by_name
                    .entry(strip_hd(&n).to_string())
                    .or_insert_with(|| id.clone());
                self.by_name.entry(n).or_insert_with(|| id.clone());
            }
        }
    }

    pub fn has_channel(&self, id: &str) -> bool {
        self.channels.iter().any(|(c, _)| c == id) || self.programmes.contains_key(id)
    }

    pub fn display_name(&self, id: &str) -> Option<&str> {
        self.channels
            .iter()
            .find(|(c, _)| c == id)
            .and_then(|(_, n)| n.first().map(String::as_str))
    }

    /// EPG-Kanal-ID für einen Sender und die Art der Zuordnung.
    pub fn match_channel(&self, ch: &Channel) -> Option<(String, MatchKind)> {
        let names: Vec<&str> = [ch.custom_name.as_deref(), Some(ch.name.as_str())]
            .into_iter()
            .flatten()
            .collect();
        self.match_parts(ch.epg_id.as_deref(), ch.tvg_id.as_deref(), &names)
    }

    /// Zuordnung aus Einzelteilen (auch für Einträge einer M3U vor dem Import).
    pub fn match_parts(
        &self,
        epg_id: Option<&str>,
        tvg_id: Option<&str>,
        names: &[&str],
    ) -> Option<(String, MatchKind)> {
        if let Some(id) = epg_id.filter(|id| self.has_channel(id)) {
            return Some((id.to_string(), MatchKind::Manual));
        }
        if let Some(id) = tvg_id.and_then(|t| self.by_id.get(&normalize_id(t))) {
            return Some((id.clone(), MatchKind::TvgId));
        }
        for name in names {
            let n = normalize_id(name);
            if let Some(id) = self
                .by_name
                .get(&n)
                .or_else(|| self.by_name.get(strip_hd(&n)))
            {
                return Some((id.clone(), MatchKind::Name));
            }
        }
        self.match_fuzzy(names)
    }

    /// Das erste Wort des Sendernamens ist das Kürzel eines EPG-Kanals („SWR Baden-Württemberg“
    /// → SWR/SR). Nur Kürzel mit 2–3 Zeichen (Anstaltskürzel); passen mehrere Kanäle, keine
    /// Zuordnung. Die Kennung wird hier bewusst nicht verwendet (z. B. „srf-online“ ≠ SR).
    fn match_fuzzy(&self, names: &[&str]) -> Option<(String, MatchKind)> {
        for name in names {
            let Some(word) = name
                .split(|c: char| c.is_whitespace() || c == '-' || c == '/')
                .find(|w| !w.is_empty())
            else {
                continue;
            };
            let word = normalize_id(word);
            let mut ids: Vec<&String> = self
                .by_stem
                .iter()
                .filter(|(s, _)| *s == word)
                .map(|(_, id)| id)
                .collect();
            ids.dedup();
            if let [id] = ids[..] {
                return Some((id.clone(), MatchKind::Fuzzy));
            }
        }
        None
    }

    /// EPG-Kanäle nach Ähnlichkeit zu einem Sender sortiert (Vorschläge für die manuelle
    /// Zuordnung): gleicher Anfang der normalisierten Kennung bzw. des Namens zuerst.
    pub fn suggestions(&self, ch: &Channel) -> Vec<(String, String)> {
        let keys: Vec<String> = [ch.tvg_id.as_deref(), Some(ch.name.as_str())]
            .into_iter()
            .flatten()
            .map(normalize_id)
            .collect();
        let score = |id: &str, names: &[String]| -> usize {
            let cands =
                std::iter::once(normalize_id(id)).chain(names.iter().map(|n| normalize_id(n)));
            cands
                .map(|c| {
                    let stem = c.strip_suffix("de").unwrap_or(&c).to_string();
                    keys.iter()
                        .map(|k| {
                            k.chars()
                                .zip(stem.chars())
                                .take_while(|(a, b)| a == b)
                                .count()
                        })
                        .max()
                        .unwrap_or(0)
                })
                .max()
                .unwrap_or(0)
        };
        let mut list: Vec<(usize, String, String)> = self
            .channels
            .iter()
            .map(|(id, names)| {
                let label = names.first().cloned().unwrap_or_else(|| id.clone());
                (score(id, names), id.clone(), label)
            })
            .collect();
        list.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase()))
        });
        list.into_iter().map(|(_, id, label)| (id, label)).collect()
    }

    /// Laufende und nächste Sendung.
    pub fn now_next(&self, id: &str, now: i64) -> (Option<&Programme>, Option<&Programme>) {
        let Some(list) = self.programmes.get(id) else {
            return (None, None);
        };
        let idx = list.partition_point(|p| p.stop <= now);
        let current = list.get(idx).filter(|p| p.start <= now);
        let next = if current.is_some() {
            list.get(idx + 1)
        } else {
            list.get(idx)
        };
        (current, next)
    }

    /// Sendungen, die sich mit dem Zeitraum überschneiden.
    pub fn range(&self, id: &str, from: i64, to: i64) -> &[Programme] {
        let Some(list) = self.programmes.get(id) else {
            return &[];
        };
        let a = list.partition_point(|p| p.stop <= from);
        let b = list.partition_point(|p| p.start < to);
        &list[a..b.max(a)]
    }

    /// Fügt einen weiteren Guide hinzu (mehrere Quellen).
    pub fn merge(&mut self, other: Guide) {
        for (id, names) in other.channels {
            if !self.channels.iter().any(|(c, _)| *c == id) {
                self.channels.push((id, names));
            }
        }
        for (id, icon) in other.icons {
            self.icons.entry(id).or_insert(icon);
        }
        for (id, mut list) in other.programmes {
            let entry = self.programmes.entry(id).or_default();
            entry.append(&mut list);
            entry.sort_by_key(|p| p.start);
            entry.dedup_by_key(|p| p.start);
        }
        self.index();
    }

    pub fn programme_count(&self) -> usize {
        self.programmes.values().map(Vec::len).sum()
    }
}

/// Tage seit 1970-01-01 für ein Datum (proleptischer gregorianischer Kalender).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// XMLTV-Zeit `YYYYMMDDhhmmss +zzzz` (Sekunden und Zeitzone optional) in Sekunden seit 1970 (UTC).
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let (dt, off) = match s.split_once(' ') {
        Some((a, b)) => (a, Some(b.trim())),
        None => (s, None),
    };
    if dt.len() < 12 || !dt.as_bytes()[..12].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n = |a: usize, b: usize| dt.get(a..b).and_then(|v| v.parse::<i64>().ok());
    let (y, mo, d, h, mi) = (n(0, 4)?, n(4, 6)?, n(6, 8)?, n(8, 10)?, n(10, 12)?);
    let sec = if dt.len() >= 14 { n(12, 14)? } else { 0 };
    let mut t = days_from_civil(y, mo, d) * 86_400 + h * 3_600 + mi * 60 + sec;
    if let Some(o) = off.filter(|o| o.len() == 5) {
        let sign = if o.starts_with('-') { -1 } else { 1 };
        let oh: i64 = o.get(1..3)?.parse().ok()?;
        let om: i64 = o.get(3..5)?.parse().ok()?;
        t -= sign * (oh * 3_600 + om * 60);
    }
    Some(t)
}

/// Entpackt gzip (erkannt an `1f 8b`), sonst unverändert.
pub fn decompress(bytes: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>, String> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::MultiGzDecoder::new(bytes)
            .take(MAX_XMLTV_BYTES + 1)
            .read_to_end(&mut out)
            .map_err(|e| format!("gzip: {e}"))?;
        if out.len() as u64 > MAX_XMLTV_BYTES {
            return Err(tf!(
                "entpackt größer als {limit} MiB, abgelehnt",
                limit = MAX_XMLTV_BYTES >> 20
            ));
        }
        Ok(out.into())
    } else {
        Ok(bytes.into())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Title,
    SubTitle,
    Desc,
}

#[derive(Default)]
struct Draft {
    channel: String,
    start: Option<i64>,
    stop: Option<i64>,
    title: Option<String>,
    subtitle: Option<String>,
    desc: Option<String>,
}

pub fn parse_xmltv(bytes: &[u8]) -> Result<Guide, String> {
    let data = decompress(bytes)?;
    let mut reader = Reader::from_reader(&data[..]);
    reader.config_mut().trim_text(false);
    let mut guide = Guide::default();
    let mut buf = Vec::new();
    let mut channel: Option<(String, Vec<String>)> = None;
    let mut draft: Option<Draft> = None;
    let mut field: Option<Field> = None;
    let mut text = String::new();
    let now = now();
    let (mut cleaned, mut icons_dropped, mut times_dropped, mut long) = (0usize, 0usize, 0usize, 0usize);

    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| tf!(
                "XML bei Byte {position}: {error}",
                position = reader.buffer_position(),
                error = e
            ))?;
        match ev {
            Event::Start(e) => {
                let attr = |name: &[u8]| {
                    e.try_get_attribute(name)
                        .ok()
                        .flatten()
                        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
                };
                match e.local_name().as_ref() {
                    b"channel" => channel = attr(b"id").map(|id| (id, Vec::new())),
                    b"programme" => {
                        draft = attr(b"channel").map(|c| Draft {
                            channel: c,
                            start: attr(b"start").and_then(|s| parse_time(&s)),
                            stop: attr(b"stop").and_then(|s| parse_time(&s)),
                            ..Draft::default()
                        });
                    }
                    b"display-name" if channel.is_some() => field = Some(Field::Name),
                    b"title" if draft.is_some() => field = Some(Field::Title),
                    b"sub-title" if draft.is_some() => field = Some(Field::SubTitle),
                    b"desc" if draft.is_some() => field = Some(Field::Desc),
                    _ => field = None,
                }
                text.clear();
            }
            // Senderlogo: <icon src="…"/> innerhalb von <channel>.
            Event::Empty(e) if e.local_name().as_ref() == b"icon" => {
                if let (Some((id, _)), Some(src)) = (
                    channel.as_ref(),
                    e.try_get_attribute(b"src")
                        .ok()
                        .flatten()
                        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned())),
                ) {
                    let class = pruefung::host_class(&src);
                    if pruefung::image_scheme_allowed(&src)
                        && !pruefung::has_control(&src)
                        && (class == HostClass::Public || class == HostClass::Private)
                    {
                        guide.icons.entry(id.clone()).or_insert(src);
                    } else {
                        icons_dropped += 1;
                    }
                }
            }
            Event::Text(t) if field.is_some() => {
                text.push_str(&t.decode().map_err(|e| e.to_string())?);
            }
            Event::CData(t) if field.is_some() => {
                text.push_str(&t.decode().map_err(|e| e.to_string())?);
            }
            Event::GeneralRef(r) if field.is_some() => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    text.push(c);
                } else if let Some(s) = r.decode().ok().and_then(|n| resolve_predefined_entity(&n))
                {
                    text.push_str(s);
                }
            }
            Event::End(e) => {
                let (max, multiline) = match field {
                    Some(Field::Desc) => (pruefung::MAX_DESC, true),
                    Some(Field::Name) => (pruefung::MAX_NAME, false),
                    _ => (pruefung::MAX_TITLE, false),
                };
                let (value, c) =
                    pruefung::clean_text(&text.trim().replace("\\n", "\n"), max, multiline);
                if c && field.is_some() {
                    cleaned += 1;
                }
                match (e.local_name().as_ref(), field) {
                    (b"display-name", Some(Field::Name)) => {
                        if let Some((_, names)) = channel.as_mut() {
                            names.push(value);
                        }
                    }
                    (b"title", Some(Field::Title)) => {
                        if let Some(d) = draft.as_mut() {
                            d.title.get_or_insert(value);
                        }
                    }
                    (b"sub-title", Some(Field::SubTitle)) => {
                        if let Some(d) = draft.as_mut() {
                            d.subtitle.get_or_insert(value);
                        }
                    }
                    (b"desc", Some(Field::Desc)) => {
                        if let Some(d) = draft.as_mut() {
                            d.desc.get_or_insert(value);
                        }
                    }
                    (b"channel", _) => {
                        if let Some(c) = channel.take() {
                            guide.channels.push(c);
                        }
                    }
                    (b"programme", _) => {
                        if let Some(d) = draft.take() {
                            if let (Some(start), Some(title)) = (d.start, d.title) {
                                let stop = d.stop.unwrap_or(start);
                                if start < MIN_START || start > now + MAX_AHEAD || stop < start {
                                    times_dropped += 1;
                                } else {
                                    if stop - start > MAX_DURATION {
                                        long += 1;
                                    }
                                    guide
                                        .programmes
                                        .entry(d.channel)
                                        .or_default()
                                        .push(Programme {
                                            start,
                                            stop,
                                            title,
                                            subtitle: d.subtitle.filter(|s| !s.is_empty()),
                                            desc: d.desc.filter(|s| !s.is_empty()),
                                        });
                                }
                            }
                        }
                    }
                    _ => {}
                }
                field = None;
                text.clear();
            }
            // Eigene Entities (Vervielfachung, externe Dateien) werden nicht aufgelöst; eine Datei,
            // die welche festlegt, wird abgewiesen.
            Event::DocType(d) if d.windows(8).any(|w| w.eq_ignore_ascii_case(b"<!ENTITY")) => {
                return Err(tr("legt eigene Entities fest (DOCTYPE mit ENTITY), abgelehnt").to_string());
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    for list in guide.programmes.values_mut() {
        list.sort_by_key(|p| p.start);
    }
    guide.cleaned = cleaned;
    let w = &mut guide.warnings;
    let n = |c: usize| c as u64;
    if icons_dropped > 0 {
        w.push(trn("{n} Logo-Adresse nicht erlaubt, verworfen", "{n} Logo-Adressen nicht erlaubt, verworfen", n(icons_dropped)));
    }
    if times_dropped > 0 {
        w.push(trn("{n} Sendung mit unmöglicher Zeit verworfen", "{n} Sendungen mit unmöglichen Zeiten verworfen", n(times_dropped)));
    }
    if long > 0 {
        w.push(trn("{n} Sendung länger als 24 Stunden", "{n} Sendungen länger als 24 Stunden", n(long)));
    }
    if guide.channels.len() > MANY_CHANNELS {
        w.push(tf!("ungewöhnlich viele Kanäle: {count}", count = guide.channels.len()));
    }
    let total: usize = guide.programmes.values().map(Vec::len).sum();
    if total > MANY_PROGRAMMES {
        w.push(tf!("ungewöhnlich viele Sendungen: {count}", count = total));
    }
    guide.index();
    Ok(guide)
}

// ---------------------------------------------------------------------------------------------
// EPG aus dem SAT-Empfang (EIT)

/// Präfix der Kanal-IDs aus dem SAT-Empfang; die ID ist der Schlüssel des SAT-Senders
/// („satip:<onid>:<tsid>:<sid>“). Ein Sender nutzt sie, wenn seine `epg_id` darauf gesetzt ist.
pub const SAT_PREFIX: &str = "satip:";

/// Zwischenspeicher der SAT-Programmdaten.
pub fn sat_cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("epg").join("sat.json")
}

/// Zwischenspeicher der Programmdaten aus dem Signal einer Empfangsquelle (SAT: `sat.json`,
/// weitere: `<kennung>.json`).
pub fn signal_cache_path(cache_dir: &Path, adapter_id: &str) -> PathBuf {
    if adapter_id == crate::settings::ADAPTER_SAT {
        sat_cache_path(cache_dir)
    } else {
        cache_dir.join("epg").join(format!("{adapter_id}.json"))
    }
}

/// Präfix der Kanal-IDs mit Programm von Tvheadend („tvh:<kanal-uuid>“, gleich dem Senderschlüssel).
pub const TVH_PREFIX: &str = "tvh:";

/// Kanal-ID aus dem Signal (SAT>IP, DVB-Karte) oder von Tvheadend?
pub fn is_signal_id(id: &str) -> bool {
    id.starts_with(SAT_PREFIX) || id.starts_with("dvb:") || id.starts_with(TVH_PREFIX)
}

/// Gesammelte SAT-Programmdaten je Kanal-ID.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SatEpg {
    pub programmes: HashMap<String, Vec<Programme>>,
}

impl SatEpg {
    pub fn load(cache_dir: &Path) -> SatEpg {
        Self::load_from(&sat_cache_path(cache_dir))
    }

    pub fn save(&self, cache_dir: &Path) -> Result<(), String> {
        self.save_to(&sat_cache_path(cache_dir))
    }

    pub fn load_from(path: &Path) -> SatEpg {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    /// Neue Abschnitte übernehmen: je Sendung nach Beginn, Neues ersetzt Altes; Vergangenes
    /// (älter als sechs Stunden) fällt weg.
    pub fn add(&mut self, sections: &[crate::ts::EitSection], now: i64) {
        self.add_prefixed(sections, now, SAT_PREFIX);
    }

    /// Wie [`SatEpg::add`], Kanal-IDs mit eigener Vorsilbe (`dvb:` bei DVB-Karten).
    pub fn add_prefixed(&mut self, sections: &[crate::ts::EitSection], now: i64, prefix: &str) {
        for sec in sections {
            let id = format!("{prefix}{}:{}:{}", sec.onid, sec.tsid, sec.service_id);
            let list = self.programmes.entry(id).or_default();
            for ev in &sec.events {
                list.retain(|p| p.start != ev.start);
                list.push(Programme {
                    start: ev.start,
                    stop: ev.stop,
                    title: ev.title.clone(),
                    subtitle: ev.subtitle.clone(),
                    desc: ev.desc.clone(),
                });
            }
        }
        for list in self.programmes.values_mut() {
            list.retain(|p| p.stop > now - 6 * 3600);
            list.sort_by_key(|p| p.start);
            // Überschneidungen (verschobene Sendungen): die spätere Angabe gilt.
            let mut out: Vec<Programme> = Vec::with_capacity(list.len());
            for p in list.drain(..) {
                if let Some(last) = out.last() {
                    if p.start < last.stop && p.start != last.start {
                        out.pop();
                    }
                }
                out.push(p);
            }
            *list = out;
        }
        self.programmes.retain(|_, l| !l.is_empty());
    }

    /// Als Guide (Kanalname „SAT-Empfang“), zum Zusammenführen mit XMLTV.
    pub fn guide(&self) -> Guide {
        let mut g = Guide::default();
        for (id, list) in &self.programmes {
            let name = if id.starts_with(SAT_PREFIX) {
                crate::tr("SAT-Empfang")
            } else if id.starts_with(TVH_PREFIX) {
                "Tvheadend"
            } else {
                crate::tr("Aus dem Signal")
            };
            g.channels.push((id.clone(), vec![name.to_string()]));
            g.programmes.insert(id.clone(), list.clone());
        }
        g.index();
        g
    }
}

/// Cachedatei für eine EPG-Quelle.
pub fn cache_path(cache_dir: &Path, source: &str) -> PathBuf {
    cache_dir.join("epg").join(net::cache_name(source, "xmltv"))
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Lädt alle Quellen: Dateien direkt, URLs aus dem Cache, sofern jünger als `max_age`
/// (`None` = Cache immer verwenden), sonst neu herunterladen; scheitert das, gilt der alte Cache.
/// Gibt den zusammengeführten Guide und Meldungen je Quelle zurück.
pub fn load_sources(
    sources: &[String],
    cache_dir: Option<&Path>,
    max_age: Option<Duration>,
) -> (Guide, Vec<String>) {
    let mut guide = Guide::default();
    let mut notes = Vec::new();
    let agent = net::agent(Duration::from_secs(60));
    for src in sources {
        let bytes = if is_url(src) {
            let cache = cache_dir.map(|d| cache_path(d, src));
            let fresh = cache.as_deref().and_then(|p| {
                let age = std::fs::metadata(p).ok()?.modified().ok()?.elapsed().ok()?;
                max_age.is_none_or(|m| age < m).then_some(())
            });
            match (fresh, cache.as_deref()) {
                (Some(()), Some(p)) => std::fs::read(p).map_err(|e| e.to_string()),
                _ => match net::fetch(&agent, src, MAX_XMLTV_BYTES) {
                    Ok(b) => {
                        if let Some(p) = cache.as_deref() {
                            let _ = p.parent().map(std::fs::create_dir_all);
                            let _ = std::fs::write(p, &b);
                        }
                        notes.push(tf!("{source}: neu geladen", source = src));
                        Ok(b)
                    }
                    Err(e) => match cache.as_deref().map(std::fs::read) {
                        Some(Ok(b)) => {
                            notes.push(tf!(
                                "{source}: Laden fehlgeschlagen ({error}), alter Stand",
                                source = src,
                                error = e
                            ));
                            Ok(b)
                        }
                        _ => Err(e),
                    },
                },
            }
        } else {
            std::fs::read(src).map_err(|e| e.to_string())
        };
        let bytes = match bytes {
            Ok(b) => b,
            Err(e) => {
                notes.push(tf!("{source}: {error}", source = src, error = e));
                continue;
            }
        };
        // Ab hier betreffen Fehler die Prüfung der Datei; sie erscheinen auch im EPG.
        match parse_xmltv(&bytes) {
            Ok(mut g) => {
                notes.push(tf!(
                    "{source}: {channels} Kanäle, {programmes} Sendungen",
                    source = src,
                    channels = g.channels.len(),
                    programmes = g.programme_count()
                ));
                if g.cleaned > 0 {
                    notes.push(tf!(
                        "{source}: {count} Texte gesäubert (Steuerzeichen oder Länge)",
                        source = src,
                        count = g.cleaned
                    ));
                }
                for w in std::mem::take(&mut g.warnings) {
                    let w = tf!("{source}: {warning}", source = src, warning = w);
                    notes.push(w.clone());
                    guide.warnings.push(w);
                }
                guide.merge(g);
            }
            Err(e) => {
                let w = tf!("{source}: {error}", source = src, error = e);
                notes.push(w.clone());
                guide.warnings.push(w);
            }
        }
    }
    (guide, notes)
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tv>
  <channel id="Das.Erste.de"><display-name lang="de">Das Erste</display-name></channel>
  <channel id="ARD.alpha.de"><display-name>ARD alpha</display-name></channel>
  <channel id="NDR.de"><display-name>NDR Fernsehen</display-name><display-name>NDR Hamburg</display-name></channel>
  <programme start="20261024160000 +0200" stop="20261024170000 +0200" channel="Das.Erste.de">
    <title lang="de">Tagesschau &amp; Wetter</title><desc>Erste Zeile\nZweite &#252;</desc>
  </programme>
  <programme start="20261024150000 +0000" stop="20261024160000 +0000" channel="Das.Erste.de">
    <title>Quiz</title><sub-title>Folge 3</sub-title>
  </programme>
  <programme start="20261024140000 +0000" stop="20261024150000 +0000" channel="Unbekannt"><title>X</title></programme>
</tv>"#;

    fn channel(name: &str, tvg_id: Option<&str>) -> Channel {
        Channel {
            id: 1,
            source: 1,
            key: String::new(),
            name: name.into(),
            custom_name: None,
            tvg_id: tvg_id.map(String::from),
            group: None,
            custom_group: None,
            logo: None,
            epg_id: None,
            number: None,
            favorite: false,
            active: true,
            radio: false,
            missing: false,
            streams: Vec::new(),
        }
    }

    #[test]
    fn zeiten_mit_zeitzone() {
        // 2026-10-24 14:00 UTC
        let utc = parse_time("20261024140000 +0000").unwrap();
        assert_eq!(utc, 1_792_850_400);
        assert_eq!(parse_time("20261024160000 +0200"), Some(utc));
        assert_eq!(parse_time("202610241400"), Some(utc));
        assert_eq!(parse_time("1970010100000"), Some(0));
        assert_eq!(parse_time("kaputt"), None);
    }

    #[test]
    fn xmltv_lesen() {
        let g = parse_xmltv(XML.as_bytes()).unwrap();
        assert_eq!(g.channels.len(), 3);
        let list = &g.programmes["Das.Erste.de"];
        assert_eq!(list.len(), 2);
        // Sortiert nach Beginn: Tagesschau 16:00 +0200 = 14:00 UTC vor Quiz 15:00 UTC.
        assert_eq!(list[0].title, "Tagesschau & Wetter");
        assert_eq!(list[0].desc.as_deref(), Some("Erste Zeile\nZweite ü"));
        assert_eq!(list[1].subtitle.as_deref(), Some("Folge 3"));
    }

    #[test]
    fn jetzt_und_danach() {
        let g = parse_xmltv(XML.as_bytes()).unwrap();
        let t = parse_time("20261024143000 +0000").unwrap();
        let (cur, next) = g.now_next("Das.Erste.de", t);
        assert_eq!(cur.unwrap().title, "Tagesschau & Wetter");
        assert_eq!(next.unwrap().title, "Quiz");
        let vorher = parse_time("20261024100000 +0000").unwrap();
        let (cur, next) = g.now_next("Das.Erste.de", vorher);
        assert!(cur.is_none());
        assert_eq!(next.unwrap().title, "Tagesschau & Wetter");
        assert_eq!(g.range("Das.Erste.de", t, t + 3600).len(), 2);
        assert_eq!(g.range("Das.Erste.de", t + 7200, t + 9000).len(), 0);
    }

    #[test]
    fn zuordnung() {
        let g = parse_xmltv(XML.as_bytes()).unwrap();
        // tvg-id mit anderer Schreibweise.
        let ch = channel("Das Erste HD", Some("DasErste.de"));
        assert_eq!(
            g.match_channel(&ch),
            Some(("Das.Erste.de".into(), MatchKind::TvgId))
        );
        let ch = channel("ARD-alpha", Some("ARD-alpha.de"));
        assert_eq!(g.match_channel(&ch).unwrap().0, "ARD.alpha.de");
        // Name ohne „HD“ gegen zweiten Anzeigenamen.
        let ch = channel("NDR Hamburg HD", Some("NDRHamburg.de"));
        assert_eq!(
            g.match_channel(&ch),
            Some(("NDR.de".into(), MatchKind::Name))
        );
        // Manuell gesetzt.
        let mut ch = channel("Irgendwas", None);
        ch.epg_id = Some("NDR.de".into());
        assert_eq!(g.match_channel(&ch).unwrap().1, MatchKind::Manual);
        assert!(g.match_channel(&channel("Gibt es nicht", None)).is_none());
    }

    #[test]
    fn vorschlaege_fuer_regionalprogramme() {
        let g = parse_xmltv(XML.as_bytes()).unwrap();
        let ch = channel("NDR Niedersachsen HD", Some("NDRFernsehen.de"));
        assert_eq!(g.suggestions(&ch)[0].0, "NDR.de");
        assert_eq!(g.suggestions(&ch).len(), 3);
    }

    #[test]
    fn unscharfe_zuordnung_fuer_dritte_programme() {
        let xml = r#"<tv>
          <channel id="SWR/SR.de"><display-name>SWR/SR</display-name></channel>
          <channel id="MDR.de"><display-name>MDR</display-name></channel>
          <channel id="RBB.de"><display-name>RBB</display-name></channel>
          <channel id="BR.de"><display-name>BR</display-name></channel>
          <channel id="WELT.de"><display-name>WELT</display-name></channel>
          <channel id="ZDF.de"><display-name>ZDF</display-name></channel>
          <channel id="ZDFneo.de"><display-name>ZDFneo</display-name></channel>
        </tv>"#;
        let g = parse_xmltv(xml.as_bytes()).unwrap();
        let m = |name: &str, tvg: &str| g.match_channel(&channel(name, Some(tvg)));
        assert_eq!(
            m("SWR Baden-Württemberg HD", "SWRFernsehen.de"),
            Some(("SWR/SR.de".into(), MatchKind::Fuzzy))
        );
        assert_eq!(
            m("SR Fernsehen HD", "SRFernsehen.de"),
            Some(("SWR/SR.de".into(), MatchKind::Fuzzy))
        );
        assert_eq!(m("MDR Sachsen HD", "MDRSachsen.de").unwrap().0, "MDR.de");
        assert_eq!(m("rbb Berlin HD", "rbbBerlin.de").unwrap().0, "RBB.de");
        assert_eq!(
            m("BR Fernsehen Süd HD", "BRFernsehen.de").unwrap().0,
            "BR.de"
        );
        // Genaue Treffer gehen vor; vier Zeichen lange IDs werden nicht unscharf genutzt.
        assert_eq!(m("ZDFneo HD", "ZDFneo.de").unwrap().1, MatchKind::TvgId);
        assert!(m("Welt der Wunder", "WeltderWunder.de").is_none());
        assert!(m("Radio Bremen TV HD", "RadioBremen.de").is_none());
        // Kennung beginnt zufällig mit einem Kürzel: keine Zuordnung.
        assert!(m("Südthüringer Regionalfernsehen", "srf-online.de").is_none());
    }

    #[test]
    fn gzip_wird_erkannt() {
        use std::io::Write;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(XML.as_bytes()).unwrap();
        let gz = enc.finish().unwrap();
        assert_eq!(parse_xmltv(&gz).unwrap().channels.len(), 3);
    }

    #[test]
    fn mehrere_quellen_zusammenfuehren() {
        let mut a = parse_xmltv(XML.as_bytes()).unwrap();
        let b = parse_xmltv(XML.as_bytes()).unwrap();
        a.merge(b);
        assert_eq!(a.channels.len(), 3);
        assert_eq!(a.programmes["Das.Erste.de"].len(), 2);
    }
}
