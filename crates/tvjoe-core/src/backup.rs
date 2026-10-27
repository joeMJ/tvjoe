//! Sicherung: alle Einstellungen und beide Senderlisten (TV, SAT) in einer TOML-Datei. Nicht
//! enthalten: die Jellyfin-Anmeldung (Token), Zwischenspeicher (Logos, Programm, Filmliste)
//! und Weiterseh-Positionen.

use serde::{Deserialize, Serialize};

use crate::library::Library;
use crate::{tf, tr};
use crate::settings::Settings;

/// Kennung und Fassung des Formats (erste Zeile der Datei).
const FORMAT: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Backup {
    pub tvjoe_sicherung: u32,
    pub settings: Settings,
    pub tv: Library,
    pub sat: Library,
}

impl Backup {
    pub fn new(settings: Settings, tv: Library, sat: Library) -> Self {
        Self {
            tvjoe_sicherung: FORMAT,
            settings,
            tv,
            sat,
        }
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        let b: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        match b.tvjoe_sicherung {
            0 => Err(tr("keine tvjoe-Sicherung").into()),
            FORMAT => Ok(b),
            v => Err(tf!("Sicherung aus einer neueren Fassung (Format {version})", version = v)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hin_und_zurueck() {
        let s = Settings {
            satip_host: "192.0.2.5".into(),
            ..Settings::default()
        };
        let mut tv = Library::default();
        tv.add_source("Liste", "liste.m3u");
        let b = Backup::new(s, tv, Library::default());
        let text = b.to_toml().unwrap();
        assert!(text.starts_with("tvjoe_sicherung = 1"));
        let back = Backup::from_toml(&text).unwrap();
        assert_eq!(back.settings.satip_host, "192.0.2.5");
        assert_eq!(back.tv.sources.len(), 1);
        assert!(Backup::from_toml("volume = 3").is_err());
    }
}
