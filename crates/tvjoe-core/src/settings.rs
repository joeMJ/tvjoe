//! Einstellungen (`settings.toml`). Fehlende Felder bekommen Standardwerte.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::m3u::Quality;
use crate::store::{StoreError, load_toml, save_toml};

/// Richtung beim Ausweichen, wenn die gewünschte Qualitätsstufe fehlt und zwei Stufen gleich nah sind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fallback {
    #[default]
    Down,
    Up,
}

/// Qualität beim Senderwechsel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    /// Gleiche Qualität halten, soweit möglich.
    #[default]
    Keep,
    /// Immer die höchste verfügbare Stufe.
    Highest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Einträge, die diese Fassung nicht kennt (aus einer neueren): beim Speichern unverändert
    /// zurückschreiben, damit ein Wechsel zwischen Fassungen keine Einstellungen löscht.
    #[serde(flatten, skip_serializing)]
    pub unknown: toml::Table,
    /// Pause nach der letzten Ziffer, bevor umgeschaltet wird.
    pub numpad_pause_ms: u32,
    pub fallback: Fallback,
    pub strategy: Strategy,
    /// Aktuell gewählte, globale Qualität; `None` heißt: höchste.
    pub quality: Option<Quality>,
    pub overlay_secs: u32,
    /// Prüfung auf neue Version über GitHub; standardmäßig aus.
    pub update_check: bool,
    /// Beim Start den letzten Sender abspielen.
    pub resume_last: bool,
    pub last_channel: Option<u64>,
    /// Sprache der Oberfläche (`de`, `en`); leer = Systemsprache. Wirkt nach dem Neustart.
    pub language: String,
    /// Lautstärke in Prozent (0–130).
    pub volume: u32,
    /// Zeilensprung entfernen (mpv `deinterlace=auto`: nur als Zeilensprung gekennzeichnete
    /// Bilder, z. B. 576i/1080i über SAT).
    pub deinterlace: bool,
    /// Untertitel des Streams anzeigen (mpv `sid=auto`); standardmäßig aus.
    pub subtitles: bool,
    /// Ton um 7 dB anheben (Rundfunknorm −23 LUFS → etwa −16 LUFS), Begrenzer gegen
    /// Übersteuern; unter Linux standardmäßig an, unter Windows aus.
    pub loudness: bool,
    /// Schwarze Balken im Bild erkennen und abschneiden.
    pub crop_bars: bool,
    /// Bild füllt das Fenster (Überstand fällt weg) statt eingepasst zu werden.
    pub fill: bool,
    /// Pufferspeicher im Arbeitsspeicher (MB): Pause im Fernsehen, Vorrat bei schwachem Netz.
    pub buffer_mb: u32,
    /// Startpuffer (Sekunden), der vor dem Abspielen und nach einem Abriss gefüllt wird.
    pub start_buffer_secs: u32,
    /// Ablageordner für Heruntergeladenes und Aufnahmen; leer = `Videos/tvjoe`.
    pub storage_dir: Option<String>,
    /// Zwischenspeicher höchstens so groß (GB).
    pub cache_limit_gb: u32,
    /// Darunter (GB frei) beginnt kein neuer Download und keine Aufnahme; eine laufende endet.
    pub min_free_gb: u32,
    /// Qualität beim Herunterladen aus der ÖR-Mediathek (`None` = höchste).
    pub download_quality: Option<crate::Quality>,
    /// Jellyfin: Originaldatei herunterladen, sonst vom Server umgerechnet mit `jellyfin_download_kbps`.
    pub jellyfin_download_original: bool,
    pub jellyfin_download_kbps: u32,
    /// Vor- und Nachlauf geplanter Aufnahmen (Minuten).
    pub record_pre_min: u32,
    pub record_post_min: u32,
    /// Gleichzeitige Aufnahmen (0 = ohne Grenze).
    pub record_max_sat: u32,
    pub record_max_internet: u32,
    /// Aufnahmen auch ohne geöffnetes tvjoe (Linux: Benutzerdienst, Windows: Aufgabenplanung);
    /// dann nimmt nur der Dienst auf, das Fenster plant und zeigt an.
    pub record_service: bool,
    /// tvjoe hat `loginctl enable-linger` gesetzt (beim Abschalten wieder zurücknehmen).
    pub record_service_linger: bool,
    /// XMLTV-Quellen (Dateien oder URLs).
    pub epg_sources: Vec<String>,
    /// EPG aus dem Netz neu laden, wenn der Cache älter ist (Stunden; 0 = nur per Knopf).
    pub epg_refresh_hours: u32,
    /// Programmdaten aus dem SAT-Empfang neu sammeln nach (Stunden; nur für Sender mit
    /// EPG-Quelle „SAT“).
    pub sat_epg_hours: u32,
    /// Zeitpunkt der letzten Updateprüfung (Sekunden seit 1970).
    pub update_last_check: Option<u64>,
    /// Gewählte Qualität als Obergrenze (nie höher, solange es eine Stufe darunter gibt).
    pub quality_cap: bool,
    /// Senderanzeige: welche Teile erscheinen (Kategorie standardmäßig aus).
    pub osd_number: bool,
    pub osd_origin: bool,
    /// Senderlogos bei Internet-Sendern (Bauchbinde, Info, Senderliste).
    pub osd_logo: bool,
    /// Senderlogos bei SAT-Sendern (Bauchbinde, Info, Senderliste).
    pub sat_logo: bool,
    pub osd_category: bool,
    pub osd_quality: bool,
    pub osd_epg: bool,
    pub osd_description: bool,
    /// Bildschirmsperre auch im Fenster verhindern (im Vollbild immer).
    pub inhibit_windowed: bool,
    /// Senderliste beim Start einblenden (sie liegt über dem Bild; Vorgabe aus, L blendet sie ein).
    pub sidebar_on_start: bool,
    /// Mediathek: Filmliste im Hintergrund neu laden, wenn älter (Stunden, mindestens 24).
    pub mediathek_refresh_hours: u32,
    /// Mediathek: so viele Tage zurück behalten.
    pub mediathek_days: u32,
    /// Mediathek: Fassungen mit Audiodeskription ausblenden.
    pub mediathek_hide_ad: bool,
    /// Mediathek: ausgeblendete Sender (Kurzname aus der Filmliste).
    pub mediathek_hidden: Vec<String>,
    /// Mediathek: Serien von A bis Z statt nach der neuesten Folge.
    pub mediathek_series_az: bool,
    /// ÖR-Mediathek eingeschaltet (Knopf in der Kopfleiste, Laden der Filmliste).
    pub mediathek_oer: bool,
    /// Jellyfin eingeschaltet.
    pub jellyfin_enabled: bool,
    /// Jellyfin-Server (Adresse); der Zugang liegt getrennt in `jellyfin.toml`.
    pub jellyfin_server: String,
    /// Jellyfin: Bandbreite begrenzen (kbit/s, 0 = aus; sonst Umrechnen auf dem Server).
    pub jellyfin_max_kbps: u32,
    /// Jellyfin: ausgeblendete Bibliotheken (Namen).
    pub jellyfin_hidden: Vec<String>,
    /// Jellyfin: Filme und Serien von A bis Z statt zuletzt hinzugefügt.
    pub jellyfin_az: bool,
    /// SAT-Quelle (SAT>IP, z. B. Netstream 4Sat) eingeschaltet.
    pub satip_enabled: bool,
    /// Adresse des SAT>IP-Geräts.
    pub satip_host: String,
    /// Zuletzt lief die SAT-Quelle (beim Start wieder).
    pub satip_active: bool,
    /// Zuletzt gesehener Sender der SAT-Quelle.
    pub satip_last_channel: Option<u64>,
    /// XMLTV-Quellen, einmal angelegt und betitelt; die Empfangsquellen wählen daraus.
    #[serde(rename = "xmltv", skip_serializing_if = "Vec::is_empty")]
    pub xmltv_sources: Vec<XmltvSource>,
    /// Empfangsquellen (Reiter „Empfang“). Fehlen sie, entstehen sie aus den älteren Feldern
    /// (`satip_*`, `epg_*`, Logos, Aufnahmegrenzen), die beim Speichern weiter mitgeschrieben
    /// werden, damit ältere Fassungen sie lesen.
    #[serde(rename = "adapter", skip_serializing_if = "Vec::is_empty")]
    pub adapters: Vec<Adapter>,
}

/// Kennung der Empfangsquelle mit der Senderliste `library.toml` (Internet-TV).
pub const ADAPTER_TV: &str = "tv";
/// Kennung der Empfangsquelle mit der Senderliste `satip.toml` (SAT>IP).
pub const ADAPTER_SAT: &str = "sat";

/// Eine XMLTV-Quelle (Datei oder URL) mit eigenem Titel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XmltvSource {
    /// Stabile Nummer (Bezug aus den Empfangsquellen).
    pub id: u32,
    pub title: String,
    pub location: String,
}

/// Eine eingerichtete Empfangsquelle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adapter {
    /// `tv` oder `sat` (bestimmt die Senderliste).
    pub id: String,
    /// Kennung der Beschreibung (`iptv-m3u`, `netstream-4sat`, …).
    pub kind: String,
    pub enabled: bool,
    /// Adresse des Geräts (SAT>IP).
    pub host: String,
    /// Genutzte XMLTV-Quellen (Nummern aus `Settings::xmltv_sources`); leer = kein XMLTV.
    pub xmltv: Vec<u32>,
    /// XMLTV neu laden nach (Stunden; 0 = nur per Knopf).
    pub xmltv_hours: u32,
    /// Programmdaten aus dem Signal sammeln alle (Stunden; nur mit Signal-EPG).
    pub signal_epg_hours: u32,
    /// Senderlogos zeigen (schwarzweiß; Bauchbinde, Info, Senderliste).
    pub logos: bool,
    /// Gleichzeitige Aufnahmen höchstens (0 = ohne Grenze).
    pub record_max: u32,
}

impl Default for Adapter {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: String::new(),
            enabled: true,
            host: String::new(),
            xmltv: Vec::new(),
            xmltv_hours: 24,
            signal_epg_hours: 6,
            logos: true,
            record_max: 0,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            unknown: toml::Table::new(),
            numpad_pause_ms: 1500,
            fallback: Fallback::Down,
            strategy: Strategy::Keep,
            quality: None,
            overlay_secs: 4,
            update_check: false,
            resume_last: true,
            last_channel: None,
            volume: 100,
            deinterlace: true,
            subtitles: false,
            loudness: !cfg!(windows),
            crop_bars: true,
            fill: false,
            buffer_mb: 512,
            start_buffer_secs: 1,
            storage_dir: None,
            cache_limit_gb: 20,
            min_free_gb: 5,
            download_quality: None,
            jellyfin_download_original: true,
            jellyfin_download_kbps: 4000,
            record_pre_min: 2,
            record_post_min: 5,
            record_max_sat: 2,
            record_max_internet: 0,
            record_service: false,
            record_service_linger: false,
            epg_sources: Vec::new(),
            epg_refresh_hours: 24,
            sat_epg_hours: 6,
            update_last_check: None,
            quality_cap: false,
            osd_number: true,
            osd_origin: true,
            osd_logo: true,
            sat_logo: true,
            osd_category: false,
            osd_quality: true,
            osd_epg: true,
            osd_description: true,
            inhibit_windowed: false,
            sidebar_on_start: false,
            mediathek_refresh_hours: 24,
            mediathek_days: 14,
            mediathek_hide_ad: true,
            mediathek_hidden: crate::mediathek::HIDDEN_DEFAULT
                .iter()
                .map(|s| s.to_string())
                .collect(),
            mediathek_series_az: false,
            mediathek_oer: true,
            jellyfin_enabled: false,
            jellyfin_server: String::new(),
            jellyfin_max_kbps: 0,
            jellyfin_az: false,
            satip_enabled: false,
            satip_host: String::new(),
            satip_active: false,
            satip_last_channel: None,
            language: String::new(),
            xmltv_sources: Vec::new(),
            adapters: Vec::new(),
            jellyfin_hidden: crate::jellyfin_lib::HIDDEN_DEFAULT
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self, StoreError> {
        let mut s: Self = load_toml(path)?;
        s.migrate();
        Ok(s)
    }

    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        let copy = self.for_saving();
        let mut table = toml::Table::try_from(&copy).map_err(|e| StoreError::Parse(e.to_string()))?;
        for (k, v) in &self.unknown {
            table.entry(k.clone()).or_insert_with(|| v.clone());
        }
        save_toml(path, &table)
    }

    /// Fassung zum Speichern: Empfangsquellen vollständig, ältere Felder mitgeschrieben.
    pub fn for_saving(&self) -> Self {
        let mut copy = self.clone();
        copy.migrate();
        copy.sync_legacy();
        copy
    }

    /// Empfangsquellen und XMLTV-Quellen aus den älteren Feldern anlegen, falls sie fehlen.
    pub fn migrate(&mut self) {
        if self.xmltv_sources.is_empty() && !self.epg_sources.is_empty() {
            self.xmltv_sources = self
                .epg_sources
                .iter()
                .enumerate()
                .map(|(i, loc)| XmltvSource {
                    id: i as u32 + 1,
                    title: xmltv_title(loc),
                    location: loc.clone(),
                })
                .collect();
        }
        let all: Vec<u32> = self.xmltv_sources.iter().map(|x| x.id).collect();
        if self.adapter(ADAPTER_TV).is_none() {
            self.adapters.push(Adapter {
                id: ADAPTER_TV.into(),
                kind: "iptv-m3u".into(),
                enabled: true,
                xmltv: all.clone(),
                xmltv_hours: self.epg_refresh_hours,
                logos: self.osd_logo,
                record_max: self.record_max_internet,
                ..Adapter::default()
            });
        }
        if self.adapter(ADAPTER_SAT).is_none() {
            self.adapters.push(Adapter {
                id: ADAPTER_SAT.into(),
                kind: "netstream-4sat".into(),
                enabled: self.satip_enabled,
                host: self.satip_host.clone(),
                xmltv: all,
                xmltv_hours: self.epg_refresh_hours,
                signal_epg_hours: self.sat_epg_hours,
                logos: self.sat_logo,
                record_max: self.record_max_sat,
            });
        }
    }

    /// Ältere Felder aus den Empfangsquellen setzen (für ältere Fassungen beim Zurückwechseln).
    fn sync_legacy(&mut self) {
        if let Some(tv) = self.adapter(ADAPTER_TV).cloned() {
            self.epg_refresh_hours = tv.xmltv_hours;
            self.osd_logo = tv.logos;
            self.record_max_internet = tv.record_max;
        }
        if let Some(sat) = self.adapter(ADAPTER_SAT).cloned() {
            self.satip_enabled = sat.enabled;
            self.satip_host = sat.host.clone();
            self.sat_epg_hours = sat.signal_epg_hours;
            self.sat_logo = sat.logos;
            self.record_max_sat = sat.record_max;
        }
        // Ältere Fassungen kennen nur eine gemeinsame Liste: alle genutzten Quellen.
        let mut used = Vec::new();
        for a in &self.adapters {
            for loc in self.xmltv_locations(&a.id) {
                if !used.contains(&loc) {
                    used.push(loc);
                }
            }
        }
        self.epg_sources = used;
    }

    /// Empfangsquelle nach Kennung.
    pub fn adapter(&self, id: &str) -> Option<&Adapter> {
        self.adapters.iter().find(|a| a.id == id)
    }

    /// Empfangsquelle nach Kennung (legt fehlende aus den älteren Feldern an).
    pub fn adapter_mut(&mut self, id: &str) -> &mut Adapter {
        self.migrate();
        let i = self.adapters.iter().position(|a| a.id == id).unwrap_or(0);
        &mut self.adapters[i]
    }

    /// Empfangsquelle eingeschaltet (fehlt sie: wie in den älteren Feldern).
    pub fn adapter_enabled(&self, id: &str) -> bool {
        match self.adapter(id) {
            Some(a) => a.enabled,
            None => id == ADAPTER_TV || self.satip_enabled,
        }
    }

    /// Dateien/URLs der XMLTV-Quellen, die eine Empfangsquelle nutzt.
    pub fn xmltv_locations(&self, adapter: &str) -> Vec<String> {
        let Some(a) = self.adapter(adapter) else {
            return self.epg_sources.clone();
        };
        a.xmltv
            .iter()
            .filter_map(|id| self.xmltv_sources.iter().find(|x| x.id == *id))
            .map(|x| x.location.clone())
            .collect()
    }

    /// Neue XMLTV-Quelle anlegen (gibt die Nummer zurück; vorhandene Adresse: deren Nummer).
    pub fn add_xmltv(&mut self, title: &str, location: &str) -> u32 {
        if let Some(x) = self.xmltv_sources.iter().find(|x| x.location == location) {
            return x.id;
        }
        let id = self.xmltv_sources.iter().map(|x| x.id).max().unwrap_or(0) + 1;
        let title = if title.trim().is_empty() {
            xmltv_title(location)
        } else {
            title.trim().to_string()
        };
        self.xmltv_sources.push(XmltvSource {
            id,
            title,
            location: location.to_string(),
        });
        id
    }
}

/// Titel aus einer Adresse: Rechnername bzw. Dateiname.
pub fn xmltv_title(location: &str) -> String {
    let rest = location
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(location);
    let host = rest.split('/').next().unwrap_or(rest);
    if location.contains("://") && !host.is_empty() {
        return host.to_string();
    }
    Path::new(location)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| location.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standardwerte_laut_plan() {
        let s = Settings::default();
        assert_eq!(s.numpad_pause_ms, 1500);
        assert_eq!(s.fallback, Fallback::Down);
        assert_eq!(s.strategy, Strategy::Keep);
        assert!(!s.update_check);
        assert!(s.resume_last);
        assert_eq!(s.overlay_secs, 4);
        assert!(!s.osd_category && s.osd_logo && s.osd_description);
    }

    #[test]
    fn fehlende_felder_bekommen_standard() {
        let s: Settings = toml::from_str("numpad_pause_ms = 900\nquality = \"720\"\n").unwrap();
        assert_eq!(s.numpad_pause_ms, 900);
        assert_eq!(s.quality, Some(Quality::P720));
        assert_eq!(s.overlay_secs, 4);
    }

    #[test]
    fn empfangsquellen_aus_aelteren_feldern() {
        let text = "satip_enabled = true\nsatip_host = \"10.0.0.5\"\nsat_logo = false\n\
                    record_max_sat = 2\nepg_sources = [\"https://epg.example/a.xml.gz\", \"/x/b.xml\"]\n";
        let dir = std::env::temp_dir().join(format!("tvjoe-settings-a-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        std::fs::write(&path, text).unwrap();
        let mut s = Settings::load(&path).unwrap();
        let sat = s.adapter(ADAPTER_SAT).unwrap().clone();
        assert!(sat.enabled && sat.host == "10.0.0.5" && !sat.logos && sat.record_max == 2);
        assert_eq!(sat.xmltv, vec![1, 2]);
        assert!(s.adapter(ADAPTER_TV).unwrap().logos);
        assert_eq!(s.xmltv_sources[0].title, "epg.example");
        assert_eq!(s.xmltv_sources[1].title, "b.xml");
        // Änderungen landen beim Speichern auch in den älteren Feldern.
        s.adapter_mut(ADAPTER_SAT).enabled = false;
        s.adapter_mut(ADAPTER_SAT).xmltv = vec![2];
        s.adapter_mut(ADAPTER_TV).xmltv.clear();
        s.save(&path).unwrap();
        let old: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(old["satip_enabled"].as_bool(), Some(false));
        assert_eq!(old["epg_sources"].as_array().unwrap().len(), 1);
        let back = Settings::load(&path).unwrap();
        assert!(!back.adapter_enabled(ADAPTER_SAT));
        assert_eq!(back.xmltv_locations(ADAPTER_SAT), vec!["/x/b.xml".to_string()]);
        assert!(back.xmltv_locations(ADAPTER_TV).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unbekannte_eintraege_bleiben() {
        let dir = std::env::temp_dir().join(format!("tvjoe-settings-u-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "volume = 80\nkuenftig_neu = 42\n[kuenftig_tabelle]\na = 1\n").unwrap();
        let s = Settings::load(&path).unwrap();
        assert_eq!(s.volume, 80);
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("kuenftig_neu = 42"), "{text}");
        assert!(text.contains("[kuenftig_tabelle]"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hin_und_zurueck() {
        let dir = std::env::temp_dir().join(format!("tvjoe-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.toml");
        let s = Settings {
            quality: Some(Quality::Uhd),
            fallback: Fallback::Up,
            strategy: Strategy::Highest,
            last_channel: Some(7),
            ..Settings::default()
        };
        s.save(&path).unwrap();
        // Beim Laden entstehen die Empfangsquellen aus den älteren Feldern.
        let mut expected = s.clone();
        expected.migrate();
        assert_eq!(Settings::load(&path).unwrap(), expected);
        let mut default = Settings::default();
        default.migrate();
        assert_eq!(Settings::load(&dir.join("fehlt.toml")).unwrap(), default);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
