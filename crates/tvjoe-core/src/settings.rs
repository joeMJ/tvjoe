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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
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
    /// Lautstärke in Prozent (0–130).
    pub volume: u32,
    /// XMLTV-Quellen (Dateien oder URLs).
    pub epg_sources: Vec<String>,
    /// EPG aus dem Netz neu laden, wenn der Cache älter ist (Stunden; 0 = nur per Knopf).
    pub epg_refresh_hours: u32,
    /// Zeitpunkt der letzten Updateprüfung (Sekunden seit 1970).
    pub update_last_check: Option<u64>,
    /// Gewählte Qualität als Obergrenze (nie höher, solange es eine Stufe darunter gibt).
    pub quality_cap: bool,
    /// Senderanzeige: welche Teile erscheinen (Kategorie standardmäßig aus).
    pub osd_number: bool,
    pub osd_origin: bool,
    pub osd_logo: bool,
    pub osd_category: bool,
    pub osd_quality: bool,
    pub osd_epg: bool,
    pub osd_description: bool,
    /// Bildschirmsperre auch im Fenster verhindern (im Vollbild immer).
    pub inhibit_windowed: bool,
    /// Mediathek: Filmliste im Hintergrund neu laden, wenn älter (Stunden, mindestens 1).
    pub mediathek_refresh_hours: u32,
    /// Mediathek: so viele Tage zurück behalten.
    pub mediathek_days: u32,
    /// Mediathek: Fassungen mit Audiodeskription ausblenden.
    pub mediathek_hide_ad: bool,
    /// Mediathek: ausgeblendete Sender (Kurzname aus der Filmliste).
    pub mediathek_hidden: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            numpad_pause_ms: 1500,
            fallback: Fallback::Down,
            strategy: Strategy::Keep,
            quality: None,
            overlay_secs: 4,
            update_check: false,
            resume_last: true,
            last_channel: None,
            volume: 100,
            epg_sources: Vec::new(),
            epg_refresh_hours: 24,
            update_last_check: None,
            quality_cap: false,
            osd_number: true,
            osd_origin: true,
            osd_logo: true,
            osd_category: false,
            osd_quality: true,
            osd_epg: true,
            osd_description: true,
            inhibit_windowed: false,
            mediathek_refresh_hours: 24,
            mediathek_days: 14,
            mediathek_hide_ad: true,
            mediathek_hidden: crate::mediathek::HIDDEN_DEFAULT
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self, StoreError> {
        load_toml(path)
    }

    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        save_toml(path, self)
    }
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
        assert_eq!(Settings::load(&path).unwrap(), s);
        assert_eq!(
            Settings::load(&dir.join("fehlt.toml")).unwrap(),
            Settings::default()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
