//! Logos für Sender ohne Logo (SAT-Liste), in dieser Reihenfolge: gleicher Programmkanal in
//! einer anderen Senderliste (TV), Senderlogo aus dem Programm (XMLTV `<icon>`), Logo aus der
//! Kodinerds-Sammlung nach dem Namen. Vorhandene Logos bleiben unangetastet.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::epg::Guide;
use crate::library::Library;
use crate::net;

const KODINERDS_TREE: &str =
    "https://api.github.com/repos/jnk22/kodinerds-iptv/git/trees/master?recursive=1";
const KODINERDS_RAW: &str = "https://raw.githubusercontent.com/jnk22/kodinerds-iptv/master/";
/// Das Verzeichnis der Sammlung wird höchstens einmal pro Woche neu geladen.
const KODINERDS_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Vergleichsschlüssel eines Namens: klein, Umlaute ausgeschrieben, nur Buchstaben und Ziffern.
fn key(name: &str) -> String {
    name.to_lowercase()
        .replace('ä', "ae")
        .replace('ö', "oe")
        .replace('ü', "ue")
        .replace('ß', "ss")
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

/// TV-Logos der Sammlung aus dem Verzeichnisbaum (GitHub-API): Schlüssel → Adresse.
pub fn kodinerds_index(tree_json: &[u8]) -> HashMap<String, String> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(tree_json) else {
        return HashMap::new();
    };
    v["tree"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["path"].as_str())
        .filter_map(|p| {
            let file = p.strip_prefix("logos/tv/")?;
            let stem = file.strip_suffix(".png")?;
            Some((key(stem), format!("{KODINERDS_RAW}{p}")))
        })
        .collect()
}

/// Verzeichnis der Sammlung aus dem Zwischenspeicher, ggf. neu geladen (blockierend).
pub fn load_kodinerds(cache_dir: &Path) -> HashMap<String, String> {
    let path = cache_dir.join("kodinerds-logos.json");
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < KODINERDS_MAX_AGE);
    if !fresh {
        let agent = net::agent(Duration::from_secs(15));
        if let Ok(data) = net::fetch(&agent, KODINERDS_TREE, 8 * 1024 * 1024) {
            if !kodinerds_index(&data).is_empty() {
                let _ = std::fs::create_dir_all(cache_dir);
                let _ = std::fs::write(&path, &data);
            }
        }
    }
    std::fs::read(&path)
        .map(|d| kodinerds_index(&d))
        .unwrap_or_default()
}

/// Logos für Sender von `lib` ohne Logo eintragen; gibt die Zahl der neu gesetzten zurück.
pub fn assign(
    lib: &mut Library,
    other: &Library,
    guide: Option<&Guide>,
    kodinerds: &HashMap<String, String>,
) -> usize {
    // Programmkanal → Logo eines Senders der anderen Liste.
    let by_epg: HashMap<String, String> = guide
        .map(|g| {
            other
                .channels
                .iter()
                .filter_map(|c| Some((g.match_channel(c)?.0, c.logo.clone()?)))
                .collect()
        })
        .unwrap_or_default();
    let mut n = 0;
    for ch in lib.channels.iter_mut().filter(|c| c.logo.is_none()) {
        let epg = guide.and_then(|g| g.match_channel(ch).map(|(id, _)| (g, id)));
        let from_guide =
            epg.and_then(|(g, id)| by_epg.get(&id).or_else(|| g.icons.get(&id)).cloned());
        let from_name = || {
            let k = key(ch.display_name());
            let short = k.strip_suffix("hd").filter(|s| !s.is_empty());
            kodinerds
                .get(&k)
                .or_else(|| short.and_then(|s| kodinerds.get(s)))
                .or_else(|| kodinerds.get(&format!("{k}hd")))
                .cloned()
        };
        if let Some(url) = from_guide.or_else(from_name) {
            ch.logo = Some(url);
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schluessel() {
        assert_eq!(key("Das Erste HD"), "dasersteHD".to_lowercase());
        assert_eq!(key("Welt der Wunder"), "weltderwunder");
        assert_eq!(key("münchen.tv"), "muenchentv");
    }

    #[test]
    fn sammlung_lesen() {
        let tree = br#"{"tree":[{"path":"logos/tv/daserstehd.png"},{"path":"logos/radio/x.png"},{"path":"logos/tv/3sat.png"}]}"#;
        let idx = kodinerds_index(tree);
        assert_eq!(idx.len(), 2);
        assert!(idx["3sat"].ends_with("logos/tv/3sat.png"));
    }

    #[test]
    fn zuordnung_nach_namen() {
        let mut lib = Library::default();
        for name in ["3sat HD", "Das Erste HD", "Unbekannt"] {
            lib.sync_scanned(
                "SAT>IP",
                "satip://x",
                &[crate::ScannedChannel {
                    key: name.into(),
                    name: name.into(),
                    group: None,
                    stream: crate::Stream {
                        quality: None,
                        kind: None,
                        url: "rtsp://x/".into(),
                        max_bitrate: None,
                    },
                }],
            );
        }
        let idx = kodinerds_index(
            br#"{"tree":[{"path":"logos/tv/daserstehd.png"},{"path":"logos/tv/3sat.png"}]}"#,
        );
        assert_eq!(assign(&mut lib, &Library::default(), None, &idx), 2);
        assert!(
            lib.channels
                .iter()
                .any(|c| c.name == "Unbekannt" && c.logo.is_none())
        );
    }
}
