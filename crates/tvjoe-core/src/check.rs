//! Prüfregeln beim Import (Entscheidungen im Projektplan, Abschnitt „M3U-Prüfung beim Import“).
//!
//! Fehler: Der Eintrag ist nicht übernehmbar. Warnungen: übernehmbar, in der Vorschau abgewählt.
//! Die Erreichbarkeitsprüfung (Netz) gehört nicht hierher.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::ids::normalize_id;
use crate::pruefung::{self, HostClass};
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
    /// Fehler: Schema außerhalb der erlaubten Liste (`file://`, `av://`, `edl://` …) oder keine
    /// Adresse mit Schema (lokaler Pfad).
    UnknownScheme(String),
    /// Fehler: Steuerzeichen in der Adresse.
    UnsafeUrl,
    /// Benutzer/Passwort in der Adresse eines fremden Hosts.
    Credentials,
    /// Adresse im eigenen Netz, auf diesem Rechner oder link-lokal.
    LocalAddress(HostClass),
    /// Logo-Adresse nicht erlaubt (Schema oder Ziel auf diesem Rechner), Logo entfernt.
    LogoRemoved,
    /// Logo-Adresse im eigenen Netz.
    LocalLogo,
    /// Steuerzeichen aus Name, Gruppe oder Kennung entfernt, oder Text gekürzt.
    TextCleaned,
    UnknownType,
    UnknownQuality(String),
    MissingQuality,
}

impl FindingKind {
    pub fn severity(&self) -> Severity {
        match self {
            Self::ExtinfWithoutUrl | Self::UnknownScheme(_) | Self::UnsafeUrl => Severity::Error,
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
            Self::UnknownScheme(s) if s.is_empty() => {
                f.write_str(tr("keine Adresse mit Schema (lokaler Pfad?), abgelehnt"))
            }
            Self::UnknownScheme(s) => {
                f.write_str(&tf!("URL-Schema „{scheme}“ nicht erlaubt, abgelehnt", scheme = s))
            }
            Self::UnsafeUrl => f.write_str(tr("Steuerzeichen in der Adresse, abgelehnt")),
            Self::Credentials => f.write_str(tr(
                "Zugangsdaten in der Adresse – für die Nutzung dieser Quelle bist du rechtlich selbst verantwortlich"
            )),
            Self::LocalAddress(HostClass::Private) => f.write_str(tr("Adresse im eigenen Netz")),
            Self::LocalAddress(HostClass::LinkLocal) => {
                f.write_str(tr("link-lokale Adresse (169.254.x.x / fe80::)"))
            }
            Self::LocalAddress(_) => f.write_str(tr("Adresse auf diesem Rechner (localhost)")),
            Self::LogoRemoved => f.write_str(tr("Logo-Adresse nicht erlaubt, entfernt")),
            Self::LocalLogo => f.write_str(tr("Logo-Adresse im eigenen Netz")),
            Self::TextCleaned => f.write_str(tr("Steuerzeichen entfernt oder Text gekürzt")),
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

    sanitize(entries, &mut out);
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

        if pruefung::has_control(&e.url) {
            push(FindingKind::UnsafeUrl);
        }
        match pruefung::scheme(&e.url) {
            Some(s) if !pruefung::STREAM_SCHEMES.contains(&s.as_str()) => {
                push(FindingKind::UnknownScheme(s))
            }
            Some(s) if s == "http" => push(FindingKind::Http),
            Some(_) => {}
            None => push(FindingKind::UnknownScheme(String::new())),
        }
        // Abgelehnte Adressen brauchen keine weitere Einordnung.
        let class = if pruefung::stream_scheme_allowed(&e.url) {
            pruefung::host_class(&e.url)
        } else {
            HostClass::Public
        };
        if class != HostClass::Public {
            push(FindingKind::LocalAddress(class));
        } else if pruefung::has_credentials(&e.url) {
            push(FindingKind::Credentials);
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

/// Säubert Name, Gruppe und Kennungen; entfernt unzulässige Logo-Adressen.
fn sanitize(entries: &mut [Entry], out: &mut Vec<Finding>) {
    for (i, e) in entries.iter_mut().enumerate() {
        let mut cleaned = false;
        let (name, c) = pruefung::clean_text(&e.name, pruefung::MAX_NAME, false);
        e.name = name;
        cleaned |= c;
        for (k, v) in e.attrs.iter_mut() {
            if matches!(k.as_str(), "tvg-logo") {
                continue;
            }
            let (t, c) = pruefung::clean_text(v, pruefung::MAX_NAME, false);
            *v = t;
            cleaned |= c;
        }
        if cleaned {
            out.push(Finding::new(FindingKind::TextCleaned, e.line, Some(i)));
        }
        if let Some(logo) = e.tvg_logo().map(str::to_string) {
            let class = pruefung::host_class(&logo);
            if !pruefung::image_scheme_allowed(&logo)
                || pruefung::has_control(&logo)
                || class.is_host_local()
            {
                e.attrs.retain(|(k, _)| k != "tvg-logo");
                out.push(Finding::new(FindingKind::LogoRemoved, e.line, Some(i)));
            } else if class == HostClass::Private {
                out.push(Finding::new(FindingKind::LocalLogo, e.line, Some(i)));
            }
        }
    }
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
