//! Abrufvideo im Hauptfenster: ein Beitrag der ÖR-Mediathek oder ein Jellyfin-Eintrag. Beide
//! nutzen dieselbe Bauchbinde, dieselben Tasten und dasselbe Weitersehen; unterschiedlich sind
//! Quelle, Angaben und wohin der Gesehen-Stand geht.

use std::path::PathBuf;

use tvjoe_core::jellyfin::Playback;
use tvjoe_core::jellyfin_lib::Item;
use tvjoe_core::mediathek::Film;
use tvjoe_core::{tf, tr};

use crate::app::App;

// Nur wenige Exemplare zugleich (laufendes Video und Warteschlange): Größe ohne Belang.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Vod {
    Oer(Film),
    Jelly(JellyVod),
    /// Datei aus der Ablage ohne Mediathek-Eintrag (Aufnahme).
    File(FileVod),
}

/// Aufnahme bzw. Datei aus der Ablage: Titel, Sender, Datei.
#[derive(Debug, Clone)]
pub struct FileVod {
    pub title: String,
    pub name: String,
    pub path: PathBuf,
}

/// Laufender Jellyfin-Eintrag; `playback` ist gesetzt, sobald der Server die Quelle genannt hat.
#[derive(Debug, Clone)]
pub struct JellyVod {
    pub item: Item,
    pub session: String,
    pub playback: Option<Playback>,
}

impl JellyVod {
    pub fn new(item: Item) -> Self {
        Self {
            item,
            session: tvjoe_core::jellyfin::new_device_id(),
            playback: None,
        }
    }
}

/// „S01E04“ bei Folgen.
fn code(it: &Item) -> Option<String> {
    match (it.season_no, it.episode_no) {
        (Some(s), Some(e)) => Some(format!("S{s:02}E{e:02}")),
        _ => None,
    }
}

impl Vod {
    /// Titel für Fenster, Bauchbinde und Meldungen.
    pub fn title(&self) -> String {
        match self {
            Vod::Oer(f) => f.display_title(),
            Vod::Jelly(j) => match code(&j.item) {
                Some(c) => format!("{c} · {}", j.item.name),
                None => j.item.name.clone(),
            },
            Vod::File(f) => f.title.clone(),
        }
    }

    /// Dauer laut Daten (Sekunden), falls mpv sie noch nicht kennt.
    pub fn duration(&self) -> f64 {
        match self {
            Vod::Oer(f) => f.duration as f64,
            Vod::Jelly(j) => j.item.runtime_secs as f64,
            Vod::File(_) => 0.0,
        }
    }

    /// Name in der Bauchbinde (Sender bzw. Serie/„Jellyfin“).
    pub fn name(&self) -> String {
        match self {
            Vod::Oer(f) => f.sender.clone(),
            Vod::Jelly(j) => j
                .item
                .series_name
                .clone()
                .unwrap_or_else(|| "Jellyfin".to_string()),
            Vod::File(f) => f.name.clone(),
        }
    }

    /// Zeile unter dem Namen.
    pub fn meta(&self) -> String {
        match self {
            Vod::Oer(f) => tf!(
                "Mediathek · {zeit}",
                zeit = crate::mediathek::day_time(f.timestamp)
            ),
            Vod::Jelly(j) => {
                let mut parts = vec!["Jellyfin".to_string()];
                if let Some(y) = j.item.year {
                    parts.push(y.to_string());
                }
                if j.playback.as_ref().is_some_and(|p| p.transcoded) {
                    parts.push(tr("umgerechnet").into());
                }
                parts.join(" · ")
            }
            Vod::File(_) => tr("Aufnahme").to_string(),
        }
    }

    /// Beschreibung (Bauchbinde und Info).
    pub fn description(&self) -> String {
        match self {
            Vod::Oer(f) if f.description.is_empty() => f.topic.clone(),
            Vod::Oer(f) => f.description.clone(),
            Vod::Jelly(j) => j.item.overview.clone(),
            Vod::File(_) => String::new(),
        }
    }

    /// Zeilen der ganzflächigen Info.
    pub fn info_meta(&self) -> Vec<String> {
        match self {
            Vod::Oer(f) => vec![
                format!("{} · {}", f.sender, f.topic),
                [
                    crate::mediathek::day_time(f.timestamp),
                    tf!("{min} Min.", min = (f.duration + 30) / 60),
                ]
                .join(" · "),
            ],
            Vod::Jelly(j) => {
                let it = &j.item;
                let mut first = Vec::new();
                if let Some(s) = &it.series_name {
                    first.push(s.clone());
                }
                if let Some(y) = it.year {
                    first.push(y.to_string());
                }
                if it.runtime_secs > 0 {
                    first.push(tf!("{min} Min.", min = (it.runtime_secs + 30) / 60));
                }
                let mut lines = vec![first.join(" · ")];
                if !it.genres.is_empty() {
                    lines.push(it.genres.clone());
                }
                if let Some(p) = &j.playback {
                    lines.push(if p.transcoded {
                        tr("Vom Server umgerechnet (Bandbreite begrenzt)").to_string()
                    } else {
                        tf!(
                            "Originaldatei, {rate} Mbit/s",
                            rate = format!("{:.1}", p.bitrate as f64 / 1e6)
                        )
                    });
                }
                lines
            }
            Vod::File(f) => vec![f.name.clone(), f.path.display().to_string()],
        }
    }

    /// Logo für Bauchbinde und Info.
    pub fn logo(&self, app: &App) -> Option<PathBuf> {
        match self {
            Vod::Oer(f) => crate::mediathek::sender_logo(app, &f.sender),
            Vod::Jelly(_) => crate::jellyfin::white_logo_path().filter(|p| p.exists()),
            Vod::File(_) => None,
        }
    }
}
