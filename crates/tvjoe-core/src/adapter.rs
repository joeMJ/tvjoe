//! Adapter: Beschreibungen der Quellen in zwei Klassen (Nutzer, 2026-10-27/28).
//!
//! - **Fernsehen** (`class = "television"`, Ordner `television/`): Empfangsquellen für lineares
//!   Fernsehen – Internet-TV, SAT>IP-Geräte, DVB-Karten, HDHomeRun.
//! - **Bibliothek** (`class = "library"`, Ordner `library/`): Mediatheken und Medienserver –
//!   ÖR-Mediathek, Jellyfin (Plex geplant).
//!
//! Eine Beschreibung (TOML) legt fest, welcher Treiber im Programm die Quelle bedient und welche
//! Eigenheiten sie hat. Die mitgelieferten liegen in `data/adapter/<klasse>/` und sind eingebaut;
//! eigene liegen im Ordner `adapter/<klasse>/` der Konfiguration (Fernsehen auch direkt in
//! `adapter/`, wie bis 0.1.4) und ersetzen eine eingebaute gleicher Kennung. Ein neuer
//! Übertragungsweg braucht weiterhin einen Treiber im Programm.

use std::path::Path;

use serde::Deserialize;

use crate::satip::Transponder;
use crate::{tf, tr};

/// Eingebaute Beschreibungen (Dateiname, Inhalt).
const BUILTIN: &[(&str, &str)] = &[
    ("iptv-m3u.toml", include_str!("../../../data/adapter/television/iptv-m3u.toml")),
    ("netstream-4sat.toml", include_str!("../../../data/adapter/television/netstream-4sat.toml")),
    ("dvb-linux.toml", include_str!("../../../data/adapter/television/dvb-linux.toml")),
    ("satip-norm.toml", include_str!("../../../data/adapter/television/satip-norm.toml")),
    ("octopus-net.toml", include_str!("../../../data/adapter/television/octopus-net.toml")),
    ("digibit.toml", include_str!("../../../data/adapter/television/digibit.toml")),
    ("kathrein-exip.toml", include_str!("../../../data/adapter/television/kathrein-exip.toml")),
    ("fritzbox-cable.toml", include_str!("../../../data/adapter/television/fritzbox-cable.toml")),
    ("hdhomerun.toml", include_str!("../../../data/adapter/television/hdhomerun.toml")),
    ("tvheadend.toml", include_str!("../../../data/adapter/television/tvheadend.toml")),
    ("multicast-iptv.toml", include_str!("../../../data/adapter/television/multicast-iptv.toml")),
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
    /// HDHomeRun-Netzwerktuner (Senderliste und Streams über HTTP).
    Hdhomerun,
    /// Tvheadend-Server (Senderliste über die HTTP-Schnittstelle, Wiedergabe mit Tickets).
    Tvheadend,
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
    /// Senderliste vom Gerät statt Suchlauf: Adressen von M3U-Listen, `{host}` = Adresse des
    /// Geräts (z. B. FRITZ!Box: `http://{host}/dvb/m3u/tvhd.m3u`).
    pub channel_list: Vec<String>,
    /// Eingebaut (sonst aus dem Ordner `adapter/`).
    pub builtin: bool,
}

impl Description {
    /// Empfangsarten mit Suchlauf über einen Frequenzplan (Antenne, Kabel).
    pub fn plan_systems(&self) -> Vec<crate::plan::System> {
        self.systems
            .iter()
            .filter_map(|s| crate::plan::System::from_code(s))
            .filter(|s| !s.satellite())
            .collect()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    /// Klasse; fehlt sie, gilt der Ordner (Fernsehen bis 0.1.4 ohne Angabe).
    #[serde(default)]
    class: Option<String>,
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
    #[serde(default)]
    channel_list: Vec<String>,
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
    check_id(&raw.id)?;
    if raw.class.as_deref().is_some_and(|c| c != "television") {
        return Err(tr("Klasse (class) muss hier „television“ sein").into());
    }
    if raw.driver == Driver::Satip {
        // Der SAT>IP-Treiber kann nur RTP über UDP und eine Verbindung je Anfrage.
        if raw.rtp_tcp || !raw.connection_per_request {
            return Err(tr("SAT>IP: RTP über TCP und dauerhafte RTSP-Verbindungen kann tvjoe noch nicht").into());
        }
        let terrestrial = raw
            .systems
            .iter()
            .filter_map(|s| crate::plan::System::from_code(s))
            .any(|s| !s.satellite());
        if raw.satellite.is_none() && raw.channel_list.is_empty() && !terrestrial {
            return Err(tr("SAT>IP: Abschnitt [satellite], channel_list oder eine Empfangsart für Antenne/Kabel fehlt").into());
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
        channel_list: raw.channel_list,
        builtin,
    })
}

fn check_id(id: &str) -> Result<(), String> {
    if id.trim().is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(tr("Kennung (id) nur aus Buchstaben, Ziffern und Bindestrich").into());
    }
    Ok(())
}

/// `.toml`-Dateien eines Ordners, sortiert.
fn toml_files(dir: Option<&Path>) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = dir
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    files
}

/// Alle Beschreibungen der Klasse Fernsehen: eingebaute, ergänzt bzw. ersetzt durch die aus
/// `dir` (Ordner `adapter/` der Konfiguration: direkt darin und in `television/`). Dazu die
/// Meldungen zu Dateien, die sich nicht lesen ließen.
pub fn all(dir: Option<&Path>) -> (Vec<Description>, Vec<String>) {
    let mut out: Vec<Description> = BUILTIN
        .iter()
        .filter_map(|(_, text)| parse(text, true).ok())
        .collect();
    let mut notes = Vec::new();
    let mut files = toml_files(dir);
    files.extend(toml_files(dir.map(|d| d.join("television")).as_deref()));
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

// ---------------------------------------------------------------------------------------------
// Klasse Bibliothek

/// Eingebaute Beschreibungen der Klasse Bibliothek.
const BUILTIN_LIBRARY: &[(&str, &str)] = &[
    ("oer-mediathek.toml", include_str!("../../../data/adapter/library/oer-mediathek.toml")),
    ("jellyfin.toml", include_str!("../../../data/adapter/library/jellyfin.toml")),
    ("plex.toml", include_str!("../../../data/adapter/library/plex.toml")),
    ("emby.toml", include_str!("../../../data/adapter/library/emby.toml")),
];

/// Treiber einer Bibliothek im Programm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryDriver {
    /// ÖR-Mediathek: Filmliste von MediathekView.
    Oer,
    /// Jellyfin-Server.
    Jellyfin,
    /// Plex Media Server.
    Plex,
    /// Emby Server (Schnittstelle wie Jellyfin).
    Emby,
}

/// Anmeldung an einer Bibliothek.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Login {
    /// Keine Anmeldung.
    None,
    /// Jellyfin Quick Connect: Code in der Weboberfläche bestätigen.
    QuickConnect,
    /// Plex: vierstelliger Code unter plex.tv/link.
    Pin,
    /// Emby: Benutzer und Passwort oder API-Schlüssel.
    Password,
}

/// Verweis einer Bibliothek (`[link]`, `[thanks]`: `label`, `url`; wahlweise `label_en`,
/// `subtitle`, `subtitle_en` – ohne Untertitel steht die Adresse in der zweiten Zeile).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryLink {
    pub label: String,
    #[serde(default)]
    pub label_en: Option<String>,
    pub url: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub subtitle_en: Option<String>,
}

impl LibraryLink {
    /// Titel in der Sprache der Oberfläche.
    pub fn label(&self, english: bool) -> &str {
        if english { self.label_en.as_deref().unwrap_or(&self.label) } else { &self.label }
    }

    /// Zweite Zeile: Untertitel in der Sprache der Oberfläche, sonst die Adresse.
    pub fn subtitle(&self, english: bool) -> &str {
        let s = if english { self.subtitle_en.as_ref().or(self.subtitle.as_ref()) } else { self.subtitle.as_ref() };
        s.map(String::as_str).unwrap_or(&self.url)
    }
}

/// Beschreibung einer Bibliothek.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryDescription {
    pub class: String,
    pub id: String,
    pub name: String,
    pub driver: LibraryDriver,
    pub login: Login,
    /// Braucht die Adresse eines Servers.
    #[serde(default)]
    pub server: bool,
    /// Hinweis zur Herkunft der Daten (Seite der Mediathek in den Einstellungen).
    #[serde(default)]
    pub note: Option<String>,
    /// Hinweis auf Englisch.
    #[serde(default)]
    pub note_en: Option<String>,
    /// Verweis zur Herkunft der Daten (Zeile mit Link auf der Seite der Mediathek).
    #[serde(default)]
    pub link: Option<LibraryLink>,
    /// Danksagung oder Spendenhinweis an die Quelle (zweite Zeile mit Link unter `[link]`).
    #[serde(default)]
    pub thanks: Option<LibraryLink>,
    /// Zusätzliche Angaben in der Filminfo (Anfrage und Fundstellen).
    #[serde(default)]
    pub detail: Option<crate::detail::Detail>,
    /// Eingebaut (sonst aus dem Ordner `adapter/library/`).
    #[serde(skip)]
    pub builtin: bool,
}

/// Beschreibung einer Bibliothek lesen und prüfen.
pub fn parse_library(text: &str, builtin: bool) -> Result<LibraryDescription, String> {
    let mut d: LibraryDescription = toml::from_str(text).map_err(|e| e.to_string())?;
    check_id(&d.id)?;
    if d.class != "library" {
        return Err(tr("Klasse (class) muss hier „library“ sein").into());
    }
    d.builtin = builtin;
    Ok(d)
}

/// Alle Beschreibungen der Klasse Bibliothek: eingebaute, ergänzt bzw. ersetzt durch die aus
/// `dir/library/`. Dazu die Meldungen zu Dateien, die sich nicht lesen ließen.
pub fn libraries(dir: Option<&Path>) -> (Vec<LibraryDescription>, Vec<String>) {
    let mut out: Vec<LibraryDescription> = BUILTIN_LIBRARY
        .iter()
        .filter_map(|(_, text)| parse_library(text, true).ok())
        .collect();
    let mut notes = Vec::new();
    for path in toml_files(dir.map(|d| d.join("library")).as_deref()) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| parse_library(&t, false))
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
    fn library_descriptions() {
        for (file, text) in BUILTIN_LIBRARY {
            parse_library(text, true).unwrap_or_else(|e| panic!("{file}: {e}"));
        }
        let j = parse_library(BUILTIN_LIBRARY[1].1, true).unwrap();
        assert_eq!((j.driver, j.login, j.server), (LibraryDriver::Jellyfin, Login::QuickConnect, true));
        // Klasse muss passen; Fernsehen-Beschreibung ist keine Bibliothek und umgekehrt.
        assert!(parse_library(BUILTIN[0].1, false).is_err());
        assert!(parse(BUILTIN_LIBRARY[0].1, false).is_err());
        let dir = std::env::temp_dir().join(format!("tvjoe-library-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("library")).unwrap();
        std::fs::write(
            dir.join("library/zweiter.toml"),
            BUILTIN_LIBRARY[1].1.replace("id = \"jellyfin\"", "id = \"jellyfin-2\""),
        )
        .unwrap();
        let (all, notes) = libraries(Some(&dir));
        assert_eq!(all.len(), BUILTIN_LIBRARY.len() + 1);
        assert!(notes.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn own_descriptions_in_class_folder() {
        let dir = std::env::temp_dir().join(format!("tvjoe-television-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("television")).unwrap();
        std::fs::write(
            dir.join("television/eigen.toml"),
            BUILTIN[1].1.replace("id = \"netstream-4sat\"", "id = \"meine-box\""),
        )
        .unwrap();
        let (all, notes) = all(Some(&dir));
        assert!(all.iter().any(|d| d.id == "meine-box" && !d.builtin));
        assert!(notes.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
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
