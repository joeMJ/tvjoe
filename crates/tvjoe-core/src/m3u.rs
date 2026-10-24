//! Einlesen von Extended-M3U (`#EXTM3U`, `#EXTINF` mit Attributen).
//!
//! Der Parser ist tolerant gegenüber unbekannten Zeilen, meldet aber Auffälligkeiten als
//! [`Finding`]s (siehe `check`). Fehler, die die ganze Datei betreffen, sind [`ParseError`].

use serde::{Deserialize, Serialize};

use crate::check::{self, Finding, FindingKind};

/// Quelltyp eines Eintrags (`tvjoe-type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Hls,
    /// Zurückgestellt; wird nur erkannt, nicht abgespielt.
    SatIp,
}

impl SourceKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "hls" => Some(Self::Hls),
            "satip" => Some(Self::SatIp),
            _ => None,
        }
    }

    /// Rückfall, wenn `tvjoe-type` fehlt: aus dem URL-Schema.
    pub fn infer(url: &str) -> Option<Self> {
        let (scheme, rest) = url.split_once("://")?;
        match scheme.to_ascii_lowercase().as_str() {
            "rtsp" | "rtsps" => Some(Self::SatIp),
            "http" | "https" => {
                let path = rest.split(['?', '#']).next().unwrap_or("");
                path.to_ascii_lowercase()
                    .ends_with(".m3u8")
                    .then_some(Self::Hls)
            }
            _ => None,
        }
    }
}

/// Qualitätsstufe (`tvjoe-quality`), aufsteigend sortiert: SD < 720p < 1080p < UHD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Quality {
    #[serde(rename = "sd")]
    Sd,
    #[serde(rename = "720")]
    P720,
    #[serde(rename = "1080")]
    P1080,
    #[serde(rename = "uhd")]
    Uhd,
}

impl Quality {
    /// Akzeptiert `sd`, `720`, `1080`, `uhd` sowie die Aliasse `720p` und `1080p`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sd" => Some(Self::Sd),
            "720" | "720p" => Some(Self::P720),
            "1080" | "1080p" => Some(Self::P1080),
            "uhd" => Some(Self::Uhd),
            _ => None,
        }
    }

    /// Normalisierter Attributwert beim Schreiben.
    pub fn as_attr(self) -> &'static str {
        match self {
            Self::Sd => "sd",
            Self::P720 => "720",
            Self::P1080 => "1080",
            Self::Uhd => "uhd",
        }
    }

    /// Anzeigename.
    pub fn label(self) -> &'static str {
        match self {
            Self::Sd => "SD",
            Self::P720 => "720p",
            Self::P1080 => "1080p",
            Self::Uhd => "UHD",
        }
    }
}

/// Ein Eintrag der Playlist: `#EXTINF`-Zeile mit zugehöriger URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Zeilennummer der `#EXTINF`-Zeile (ab 1).
    pub line: usize,
    pub name: String,
    pub url: String,
    /// Attribute mit kleingeschriebenen Schlüsseln, in Dateireihenfolge.
    pub attrs: Vec<(String, String)>,
}

impl Entry {
    /// Wert eines Attributs; leere Werte zählen als fehlend. Bei Doppelungen gilt der letzte.
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.trim().is_empty())
    }

    pub fn set_attr(&mut self, key: &str, value: String) {
        self.attrs.retain(|(k, _)| k != key);
        self.attrs.push((key.to_string(), value));
    }

    pub fn tvg_id(&self) -> Option<&str> {
        self.attr("tvg-id")
    }

    pub fn tvg_logo(&self) -> Option<&str> {
        self.attr("tvg-logo")
    }

    pub fn group(&self) -> Option<&str> {
        self.attr("group-title")
    }

    pub fn chno(&self) -> Option<u32> {
        self.attr("tvg-chno")?.trim().parse().ok()
    }

    /// Quelltyp: ausdrücklich über `tvjoe-type`, sonst aus der URL abgeleitet.
    pub fn kind(&self) -> Option<SourceKind> {
        match self.attr("tvjoe-type") {
            Some(v) => SourceKind::parse(v),
            None => SourceKind::infer(&self.url),
        }
    }

    pub fn quality(&self) -> Option<Quality> {
        Quality::parse(self.attr("tvjoe-quality")?)
    }

    /// URL-Schema in Kleinschreibung.
    pub fn scheme(&self) -> Option<String> {
        self.url
            .split_once("://")
            .map(|(s, _)| s.to_ascii_lowercase())
    }
}

/// Fehler, die die ganze Datei betreffen (nichts wird übernommen).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    NotUtf8,
    NotExtM3u,
    NoEntries,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotUtf8 => "Die Datei ist nicht UTF-8-kodiert.",
            Self::NotExtM3u => "Die Datei beginnt nicht mit #EXTM3U.",
            Self::NoEntries => "Die Datei enthält keine auswertbaren Einträge.",
        })
    }
}

impl std::error::Error for ParseError {}

/// Eingelesene und geprüfte Playlist.
#[derive(Debug, Clone, Default)]
pub struct Playlist {
    pub entries: Vec<Entry>,
    /// Nach Zeilennummer sortiert.
    pub findings: Vec<Finding>,
}

impl Playlist {
    /// Liest und prüft eine M3U. Doppelte Kanalnummern werden dabei neu vergeben (Meldung in den Findings).
    pub fn parse(bytes: &[u8]) -> Result<Self, ParseError> {
        let text = std::str::from_utf8(bytes).map_err(|_| ParseError::NotUtf8)?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let first = text.lines().map(str::trim).find(|l| !l.is_empty());
        if !first.is_some_and(|l| l.starts_with("#EXTM3U")) {
            return Err(ParseError::NotExtM3u);
        }
        let (mut entries, mut findings) = parse_entries(text);
        if entries.is_empty() {
            return Err(ParseError::NoEntries);
        }
        findings.extend(check::analyze(&mut entries));
        findings.sort_by_key(|f| f.line);
        Ok(Self { entries, findings })
    }

    /// Ob zu dem Eintrag ein Fehler (nicht nur Warnungen) vorliegt.
    pub fn has_errors_for(&self, entry: usize) -> bool {
        self.findings
            .iter()
            .any(|f| f.entry == Some(entry) && f.kind.severity() == check::Severity::Error)
    }
}

fn parse_entries(text: &str) -> (Vec<Entry>, Vec<Finding>) {
    let mut entries = Vec::new();
    let mut findings = Vec::new();
    /// Gelesene `#EXTINF`-Zeile, die noch auf ihre URL wartet: Zeile, Attribute, Name.
    type Pending = (usize, Vec<(String, String)>, String);
    let mut pending: Option<Pending> = None;

    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            if let Some((l, _, _)) = pending.take() {
                findings.push(Finding::new(FindingKind::ExtinfWithoutUrl, l, None));
            }
            let (head, name) = split_name(rest);
            pending = Some((line_no, parse_attrs(head), name.to_string()));
        } else if line.starts_with('#') {
            // #EXTM3U, #EXTGRP, #EXTVLCOPT und Kommentare: ignoriert.
        } else if let Some((l, attrs, name)) = pending.take() {
            entries.push(Entry {
                line: l,
                name,
                url: line.to_string(),
                attrs,
            });
        } else {
            findings.push(Finding::new(FindingKind::UrlWithoutExtinf, line_no, None));
        }
    }
    if let Some((l, _, _)) = pending {
        findings.push(Finding::new(FindingKind::ExtinfWithoutUrl, l, None));
    }
    (entries, findings)
}

/// Trennt `Attribute…,Name` am ersten Komma außerhalb von Anführungszeichen.
fn split_name(s: &str) -> (&str, &str) {
    let mut in_quotes = false;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'"' => in_quotes = !in_quotes,
            b',' if !in_quotes => return (&s[..i], s[i + 1..].trim()),
            _ => {}
        }
    }
    (s, "")
}

/// Liest `key="value"`-Paare (auch ohne Anführungszeichen) nach dem Längenfeld.
fn parse_attrs(head: &str) -> Vec<(String, String)> {
    let b = head.as_bytes();
    let mut i = 0;
    while i < b.len() && !b[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut out = Vec::new();
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let key_start = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = head[key_start..i].to_ascii_lowercase();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            let value = if i < b.len() && b[i] == b'"' {
                i += 1;
                let vs = i;
                while i < b.len() && b[i] != b'"' {
                    i += 1;
                }
                let v = &head[vs..i];
                if i < b.len() {
                    i += 1;
                }
                v
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                &head[vs..i]
            };
            out.push((key, value.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn komma_in_anfuehrungszeichen_trennt_nicht() {
        let (head, name) = split_name(r#"-1 tvg-logo="https://x.invalid/a,b.png",Name, mit Komma"#);
        assert_eq!(head, r#"-1 tvg-logo="https://x.invalid/a,b.png""#);
        assert_eq!(name, "Name, mit Komma");
    }

    #[test]
    fn attribute_mit_und_ohne_anfuehrungszeichen() {
        let a = parse_attrs(r#"-1 TVG-ID="Eins.de" tvg-chno=7 group-title="A B""#);
        assert_eq!(
            a,
            vec![
                ("tvg-id".into(), "Eins.de".into()),
                ("tvg-chno".into(), "7".into()),
                ("group-title".into(), "A B".into()),
            ]
        );
    }

    #[test]
    fn qualitaet_aliasse_und_reihenfolge() {
        assert_eq!(Quality::parse("720p"), Some(Quality::P720));
        assert_eq!(Quality::parse(" 1080P "), Some(Quality::P1080));
        assert_eq!(Quality::parse("hd"), None);
        assert!(Quality::Sd < Quality::P720 && Quality::P1080 < Quality::Uhd);
        assert_eq!(Quality::P720.as_attr(), "720");
    }

    #[test]
    fn typ_aus_url_abgeleitet() {
        assert_eq!(
            SourceKind::infer("https://x.invalid/a/master.m3u8?t=1"),
            Some(SourceKind::Hls)
        );
        assert_eq!(
            SourceKind::infer("rtsp://x.invalid/?src=1"),
            Some(SourceKind::SatIp)
        );
        assert_eq!(SourceKind::infer("https://x.invalid/stream"), None);
    }

    #[test]
    fn dateifehler() {
        assert_eq!(
            Playlist::parse(&[0xff, 0xfe, 0x00]).unwrap_err(),
            ParseError::NotUtf8
        );
        assert_eq!(
            Playlist::parse(b"hallo").unwrap_err(),
            ParseError::NotExtM3u
        );
        assert_eq!(
            Playlist::parse(b"#EXTM3U\n").unwrap_err(),
            ParseError::NoEntries
        );
    }

    #[test]
    fn bom_und_crlf_werden_vertragen() {
        let p = Playlist::parse(
            "\u{feff}#EXTM3U\r\n#EXTINF:-1,Eins\r\nhttps://x.invalid/a.m3u8\r\n".as_bytes(),
        )
        .unwrap();
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.entries[0].name, "Eins");
        assert_eq!(p.entries[0].url, "https://x.invalid/a.m3u8");
    }
}
