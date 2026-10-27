//! Beschreibungen der Empfangsquellen (Nutzer, 2026-10-27).
//!
//! Eine Beschreibung (TOML) legt fest, welcher Treiber im Programm eine Empfangsquelle bedient
//! und welche Eigenheiten das Gerät hat. Die mitgelieferten liegen in `data/adapter/` und sind
//! eingebaut; eigene liegen im Ordner `adapter/` der Konfiguration und ersetzen eine
//! eingebaute gleicher Kennung. Ein neuer Übertragungsweg braucht weiterhin einen Treiber im
//! Programm; eine Beschreibung kann nur vorhandene Treiber nutzen.

use std::path::Path;

use serde::Deserialize;

use crate::satip::Transponder;
use crate::{tf, tr};

/// Eingebaute Beschreibungen (Dateiname, Inhalt).
const BUILTIN: &[(&str, &str)] = &[
    ("iptv-m3u.toml", include_str!("../../../data/adapter/iptv-m3u.toml")),
    ("netstream-4sat.toml", include_str!("../../../data/adapter/netstream-4sat.toml")),
    ("dvb-linux.toml", include_str!("../../../data/adapter/dvb-linux.toml")),
];

/// Treiber im Programm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    /// Internet-TV aus M3U-Listen.
    Iptv,
    /// SAT>IP (RTSP/RTP).
    Satip,
    /// DVB-Karte unter Linux (`/dev/dvb`; experimentell).
    #[serde(rename = "linux-dvb")]
    LinuxDvb,
}

/// Satellit einer SAT>IP-Beschreibung.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Satellite {
    pub name: String,
    /// Orbitalposition in Zehntelgrad (192 = 19,2°).
    pub orbital: u16,
    pub east: bool,
    /// Starttransponder des Suchlaufs (sendet die Netzwerktabelle).
    pub seed: Transponder,
}

/// Beschreibung einer Empfangsquelle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Description {
    pub id: String,
    pub name: String,
    pub driver: Driver,
    /// Empfangsarten (`dvbs`, `dvbs2`, …); bei IPTV leer.
    pub systems: Vec<String>,
    pub tuners: u32,
    /// Programmdaten kommen mit dem Signal (EIT).
    pub signal_epg: bool,
    pub satellite: Option<Satellite>,
    /// Eingebaut (sonst aus dem Ordner `adapter/`).
    pub builtin: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    id: String,
    name: String,
    driver: Driver,
    #[serde(default)]
    systems: Vec<String>,
    #[serde(default)]
    tuners: u32,
    #[serde(default)]
    signal_epg: bool,
    #[serde(default)]
    rtp_tcp: bool,
    #[serde(default = "yes")]
    connection_per_request: bool,
    satellite: Option<RawSatellite>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSatellite {
    name: String,
    orbital: u16,
    east: bool,
    seed: RawTransponder,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTransponder {
    freq: u32,
    pol: String,
    dvbs2: bool,
    mtype: String,
    sr: u32,
    fec: String,
}

/// Modulation und Coderate als feste Texte (die SAT>IP-Adresse kennt nur diese).
fn modulation(s: &str) -> Option<&'static str> {
    ["qpsk", "8psk", "16apsk", "32apsk"].into_iter().find(|m| *m == s)
}

fn coderate(s: &str) -> Option<&'static str> {
    ["12", "23", "34", "35", "45", "56", "78", "89", "910"].into_iter().find(|m| *m == s)
}

/// Beschreibung lesen und prüfen.
pub fn parse(text: &str, builtin: bool) -> Result<Description, String> {
    let raw: Raw = toml::from_str(text).map_err(|e| e.to_string())?;
    if raw.id.trim().is_empty() || !raw.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(tr("Kennung (id) nur aus Buchstaben, Ziffern und Bindestrich").into());
    }
    if raw.driver == Driver::Satip {
        // Der SAT>IP-Treiber kann nur RTP über UDP und eine Verbindung je Anfrage.
        if raw.rtp_tcp || !raw.connection_per_request {
            return Err(tr("SAT>IP: RTP über TCP und dauerhafte RTSP-Verbindungen kann tvjoe noch nicht").into());
        }
        if raw.satellite.is_none() {
            return Err(tr("SAT>IP: Abschnitt [satellite] fehlt").into());
        }
    }
    let satellite = match raw.satellite {
        Some(s) => {
            let t = s.seed;
            let pol = match t.pol.as_str() {
                "h" => 'h',
                "v" => 'v',
                p => return Err(tf!("Polarisation „{pol}“ unbekannt (h oder v)", pol = p)),
            };
            let mtype = modulation(&t.mtype)
                .ok_or_else(|| tf!("Modulation „{mtype}“ unbekannt", mtype = t.mtype))?;
            let fec = coderate(&t.fec)
                .ok_or_else(|| tf!("Coderate „{fec}“ unbekannt", fec = t.fec))?;
            Some(Satellite {
                name: s.name,
                orbital: s.orbital,
                east: s.east,
                seed: Transponder {
                    freq: t.freq,
                    pol,
                    dvbs2: t.dvbs2,
                    mtype,
                    sr: t.sr,
                    fec,
                },
            })
        }
        None => None,
    };
    Ok(Description {
        id: raw.id,
        name: raw.name,
        driver: raw.driver,
        systems: raw.systems,
        tuners: raw.tuners,
        signal_epg: raw.signal_epg,
        satellite,
        builtin,
    })
}

/// Alle Beschreibungen: eingebaute, ergänzt bzw. ersetzt durch die aus `dir`. Dazu die
/// Meldungen zu Dateien, die sich nicht lesen ließen.
pub fn all(dir: Option<&Path>) -> (Vec<Description>, Vec<String>) {
    let mut out: Vec<Description> = BUILTIN
        .iter()
        .filter_map(|(_, text)| parse(text, true).ok())
        .collect();
    let mut notes = Vec::new();
    let mut files: Vec<_> = dir
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    for path in files {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| parse(&t, false))
        {
            Ok(d) => {
                out.retain(|o| o.id != d.id);
                out.push(d);
            }
            Err(e) => notes.push(tf!("{file}: {error}", file = name, error = e)),
        }
    }
    (out, notes)
}

/// Beschreibung zu einer Kennung (eingebaute und eigene).
pub fn find(id: &str, dir: Option<&Path>) -> Option<Description> {
    all(dir).0.into_iter().find(|d| d.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_descriptions_parse() {
        for (file, text) in BUILTIN {
            parse(text, true).unwrap_or_else(|e| panic!("{file}: {e}"));
        }
        let n = parse(BUILTIN[1].1, true).unwrap();
        assert_eq!(n.driver, Driver::Satip);
        assert_eq!(n.tuners, 4);
        let sat = n.satellite.unwrap();
        assert_eq!((sat.orbital, sat.east), (192, true));
        // Starttransponder wie bisher fest im Programm (ARD, 11494 H).
        assert_eq!(sat.seed, crate::satip::CHECK_TRANSPONDERS[0]);
    }

    #[test]
    fn rejects_unsupported() {
        let base = BUILTIN[1].1;
        assert!(parse(&base.replace("rtp_tcp = false", "rtp_tcp = true"), false).is_err());
        assert!(parse(&base.replace("\"8psk\"", "\"9psk\""), false).is_err());
        assert!(parse(&base.replace("driver = \"satip\"", "driver = \"dvbt\""), false).is_err());
        assert!(parse(&format!("{base}\nunbekannt = 1\n"), false).is_err());
        assert!(parse("id = \"a b\"\nname = \"x\"\ndriver = \"iptv\"\n", false).is_err());
    }

    #[test]
    fn own_descriptions_replace_builtin() {
        let dir = std::env::temp_dir().join(format!("tvjoe-adapter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("eigen.toml"),
            BUILTIN[1].1.replace("EyeTV Netstream 4Sat", "Meine Netstream"),
        )
        .unwrap();
        std::fs::write(dir.join("kaputt.toml"), "id = 3").unwrap();
        let (all, notes) = all(Some(&dir));
        assert_eq!(all.len(), BUILTIN.len());
        let n = all.iter().find(|d| d.id == "netstream-4sat").unwrap();
        assert_eq!(n.name, "Meine Netstream");
        assert!(!n.builtin);
        assert_eq!(notes.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
