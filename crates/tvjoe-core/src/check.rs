//! Prüfregeln beim Import (Entscheidungen im Projektplan, Abschnitt „M3U-Prüfung beim Import“).
//!
//! Fehler: Der Eintrag ist nicht übernehmbar. Warnungen: übernehmbar, in der Vorschau abgewählt.
//! Die Erreichbarkeitsprüfung (Netz) gehört nicht hierher.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::ids::normalize_id;
use crate::{tf, tr};
use crate::m3u::{Entry, Quality};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingKind {
    /// Fehler: `#EXTINF` ohne folgende URL.
    ExtinfWithoutUrl,
    /// Warnung: URL-Zeile ohne `#EXTINF` (Zeile ignoriert).
    UrlWithoutExtinf,
    MissingTvgId,
    MissingLogo,
    MissingGroup,
    /// Gleiche `tvg-id` mehrfach ohne unterscheidende Qualität.
    DuplicateTvgId,
    DuplicateUrl,
    /// Gleicher Sender mit gleicher Qualitätsstufe mehrfach.
    DuplicateQuality(Quality),
    /// Kanalnummer war bei einem anderen Sender vergeben und wurde neu vergeben.
    DuplicateChno {
        old: u32,
        new: u32,
    },
    /// `http://` statt `https://`.
    Http,
    UnknownScheme(String),
    UnknownType,
    UnknownQuality(String),
    MissingQuality,
}

impl FindingKind {
    pub fn severity(&self) -> Severity {
        match self {
            Self::ExtinfWithoutUrl => Severity::Error,
            _ => Severity::Warning,
        }
    }
}

impl fmt::Display for FindingKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExtinfWithoutUrl => f.write_str(tr("#EXTINF ohne URL")),
            Self::UrlWithoutExtinf => f.write_str(tr("URL ohne #EXTINF-Zeile, ignoriert")),
            Self::MissingTvgId => f.write_str(tr("tvg-id fehlt")),
            Self::MissingLogo => f.write_str(tr("tvg-logo fehlt")),
            Self::MissingGroup => f.write_str(tr("group-title fehlt")),
            Self::DuplicateTvgId => f.write_str(tr("tvg-id doppelt, ohne unterscheidende Qualität")),
            Self::DuplicateUrl => f.write_str(tr("URL doppelt")),
            Self::DuplicateQuality(q) => f.write_str(&tf!("Sender doppelt in der Qualität {quality}", quality = q.label())),
            Self::DuplicateChno { old, new } => {
                f.write_str(&tf!("Kanalnummer {old} doppelt, neu vergeben: {new}", old = old, new = new))
            }
            Self::Http => f.write_str(tr("unverschlüsselte Quelle (http://)")),
            Self::UnknownScheme(s) => f.write_str(&tf!("unbekanntes URL-Schema „{scheme}“", scheme = s)),
            Self::UnknownType => f.write_str(tr("Quelltyp unbekannt (tvjoe-type)")),
            Self::UnknownQuality(v) => f.write_str(&tf!(
                "Qualitätswert „{value}“ unbekannt (erlaubt: sd, 720, 1080, uhd)",
                value = v
            )),
            Self::MissingQuality => f.write_str(tr("Qualität unbekannt (tvjoe-quality fehlt)")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub kind: FindingKind,
    /// Zeilennummer in der Datei (ab 1).
    pub line: usize,
    /// Index in `Playlist::entries`, falls ein Eintrag betroffen ist.
    pub entry: Option<usize>,
}

impl Finding {
    pub fn new(kind: FindingKind, line: usize, entry: Option<usize>) -> Self {
        Self { kind, line, entry }
    }
}

/// Schlüssel für „derselbe Sender“: normalisierte `tvg-id`, sonst normalisierter Name.
pub(crate) fn channel_key(e: &Entry) -> String {
    normalize_id(e.tvg_id().unwrap_or(&e.name))
}

/// Prüft alle Einträge und vergibt doppelte Kanalnummern neu (ändert dabei `tvg-chno`).
pub fn analyze(entries: &mut [Entry]) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut seen_url: HashMap<String, Vec<(String, Option<Quality>)>> = HashMap::new();
    let mut seen_id: HashMap<(String, Option<Quality>), usize> = HashMap::new();

    for (i, e) in entries.iter().enumerate() {
        let mut push = |kind: FindingKind| out.push(Finding::new(kind, e.line, Some(i)));

        if e.tvg_id().is_none() {
            push(FindingKind::MissingTvgId);
        }
        if e.tvg_logo().is_none() {
            push(FindingKind::MissingLogo);
        }
        if e.group().is_none() {
            push(FindingKind::MissingGroup);
        }

        match e.scheme().as_deref() {
            Some("https") | Some("rtsp") | Some("rtsps") => {}
            Some("http") => push(FindingKind::Http),
            Some(other) => push(FindingKind::UnknownScheme(other.to_string())),
            None => push(FindingKind::UnknownScheme(String::new())),
        }

        if e.kind().is_none() {
            push(FindingKind::UnknownType);
        }

        match (e.attr("tvjoe-quality"), e.quality()) {
            (Some(_), Some(_)) => {}
            (Some(v), None) => push(FindingKind::UnknownQuality(v.to_string())),
            (None, _) => push(FindingKind::MissingQuality),
        }

        // Gleiche URL ist nur für denselben Sender in verschiedenen Stufen erlaubt
        // (adaptiver Stream, z. B. aus dem Export).
        let me = (channel_key(e), e.quality());
        let others = seen_url.entry(e.url.clone()).or_default();
        let clash = others.iter().any(|(k, q)| *k != me.0 || *q == me.1);
        others.push(me);
        if clash {
            push(FindingKind::DuplicateUrl);
        }
        let key = (channel_key(e), e.quality());
        if seen_id.insert(key, i).is_some() {
            match e.quality() {
                Some(q) => push(FindingKind::DuplicateQuality(q)),
                None if e.tvg_id().is_some() => push(FindingKind::DuplicateTvgId),
                None => {}
            }
        }
    }

    assign_free_chnos(entries, &mut out);
    out
}

/// Gleiche Nummer bei verschiedenen Sendern: der erste behält sie, die übrigen bekommen die nächste freie.
fn assign_free_chnos(entries: &mut [Entry], out: &mut Vec<Finding>) {
    let mut owner: HashMap<u32, String> = HashMap::new();
    let mut next = entries.iter().filter_map(Entry::chno).max().unwrap_or(0);
    let mut used: HashSet<u32> = entries.iter().filter_map(Entry::chno).collect();

    for (i, e) in entries.iter_mut().enumerate() {
        let Some(n) = e.chno() else { continue };
        let key = channel_key(e);
        match owner.get(&n) {
            None => {
                owner.insert(n, key);
            }
            Some(k) if *k == key => {}
            Some(_) => {
                loop {
                    next += 1;
                    if !used.contains(&next) {
                        break;
                    }
                }
                used.insert(next);
                owner.insert(next, key);
                e.set_attr("tvg-chno", next.to_string());
                out.push(Finding::new(
                    FindingKind::DuplicateChno { old: n, new: next },
                    e.line,
                    Some(i),
                ));
            }
        }
    }
}
