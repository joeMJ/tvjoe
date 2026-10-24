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

const MAX_XMLTV_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
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
    by_id: HashMap<String, String>,
    by_name: HashMap<String, String>,
}

/// Wie ein Sender zugeordnet wurde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Manual,
    TvgId,
    Name,
}

fn strip_hd(n: &str) -> &str {
    n.strip_suffix("hd").filter(|s| !s.is_empty()).unwrap_or(n)
}

impl Guide {
    fn index(&mut self) {
        self.by_id.clear();
        self.by_name.clear();
        for (id, names) in &self.channels {
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
            .take(MAX_XMLTV_BYTES)
            .read_to_end(&mut out)
            .map_err(|e| format!("gzip: {e}"))?;
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

    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| format!("XML bei Byte {}: {e}", reader.buffer_position()))?;
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
                let value = text.trim().replace("\\n", "\n");
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
                                guide
                                    .programmes
                                    .entry(d.channel)
                                    .or_default()
                                    .push(Programme {
                                        start,
                                        stop: d.stop.unwrap_or(start),
                                        title,
                                        subtitle: d.subtitle.filter(|s| !s.is_empty()),
                                        desc: d.desc.filter(|s| !s.is_empty()),
                                    });
                            }
                        }
                    }
                    _ => {}
                }
                field = None;
                text.clear();
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    for list in guide.programmes.values_mut() {
        list.sort_by_key(|p| p.start);
    }
    guide.index();
    Ok(guide)
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
                        notes.push(format!("{src}: neu geladen"));
                        Ok(b)
                    }
                    Err(e) => match cache.as_deref().map(std::fs::read) {
                        Some(Ok(b)) => {
                            notes.push(format!("{src}: Laden fehlgeschlagen ({e}), alter Stand"));
                            Ok(b)
                        }
                        _ => Err(e),
                    },
                },
            }
        } else {
            std::fs::read(src).map_err(|e| e.to_string())
        };
        match bytes.and_then(|b| parse_xmltv(&b)) {
            Ok(g) => {
                notes.push(format!(
                    "{src}: {} Kanäle, {} Sendungen",
                    g.channels.len(),
                    g.programme_count()
                ));
                guide.merge(g);
            }
            Err(e) => notes.push(format!("{src}: {e}")),
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
