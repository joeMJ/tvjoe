//! Eigene Symbole: liegen im Zwischenspeicher unter `icons/hicolor/symbolic/apps/` und werden
//! dem Symbolthema bekannt gemacht (Knöpfe und Reiter nehmen nur Symbolnamen). Als `-symbolic`
//! färbt GTK sie in der Textfarbe, passend zu hell und dunkel. Die Themen-Ordnerstruktur ist
//! nötig: GTK 4.14 (Ubuntu 24.04) färbt lose Dateien im Suchpfad nicht ein, GTK 4.22 schon.

use std::path::PathBuf;

use tvjoe_core::paths;

/// SAT-Schüssel (SAT-Quelle).
pub const SATELLITE: &str = "tvjoe-satellite-symbolic";

const SATELLITE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><g fill="#2e3436"><path d="M1.5 2.8C0.2 8.6 4.6 13.4 10.8 12.6Z"/><path d="M5.1 7.4L10.4 2.1L11.6 3.3L6.3 8.6Z"/><circle cx="11.9" cy="2.3" r="1.3"/><path d="M4.9 11h1.8l1.1 3.1H3.8z"/><rect x="2" y="14" width="8" height="1.6" rx="0.5"/></g></svg>"##;

/// Dachantenne (Empfangsquellen über eine DVB-Karte: Antenne, Kabel, Satellit am Kabel).
pub const ANTENNA: &str = "tvjoe-antenna-symbolic";

const ANTENNA_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><g fill="#2e3436"><rect x="7.2" y="4" width="1.6" height="11.5" rx="0.5"/><rect x="1" y="4" width="14" height="1.5" rx="0.5"/><rect x="2" y="1" width="1.4" height="7" rx="0.5"/><rect x="5.2" y="1.5" width="1.4" height="6" rx="0.5"/><rect x="9.4" y="2" width="1.4" height="5" rx="0.5"/><rect x="12.6" y="2.5" width="1.4" height="4" rx="0.5"/></g></svg>"##;

/// Schwarze Balken abschneiden (Bildrahmen mit Balken oben und unten).
pub const CROP: &str = "tvjoe-crop-symbolic";

const CROP_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><g fill="#2e3436"><path fill-rule="evenodd" d="M3.5 1h9A2.5 2.5 0 0 1 15 3.5v9a2.5 2.5 0 0 1-2.5 2.5h-9A2.5 2.5 0 0 1 1 12.5v-9A2.5 2.5 0 0 1 3.5 1zM3.5 2.5a1 1 0 0 0-1 1v9a1 1 0 0 0 1 1h9a1 1 0 0 0 1-1v-9a1 1 0 0 0-1-1z"/><path d="M3.5 2.5h9a1 1 0 0 1 1 1v2h-11v-2a1 1 0 0 1 1-1z"/><path d="M2.5 10.5h11v2a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1z"/></g></svg>"##;

/// Ordner der eigenen Symbole (angelegt; sein Wurzelordner ist im Symbolthema eingetragen).
pub fn dir() -> Option<PathBuf> {
    let base = paths::cache_dir()?.join("icons");
    let dir = base.join("hicolor/symbolic/apps");
    std::fs::create_dir_all(&dir).ok()?;
    let display = gtk::gdk::Display::default()?;
    let theme = gtk::IconTheme::for_display(&display);
    if !theme.search_path().contains(&base) {
        theme.add_search_path(&base);
    }
    Some(dir)
}

/// Eigene Symbole ablegen (beim Start).
pub fn install() {
    let Some(dir) = dir() else { return };
    for (name, svg) in [(SATELLITE, SATELLITE_SVG), (ANTENNA, ANTENNA_SVG), (CROP, CROP_SVG)] {
        let path = dir.join(format!("{name}.svg"));
        if std::fs::read_to_string(&path).ok().as_deref() != Some(svg) {
            let _ = std::fs::write(&path, svg);
        }
    }
}
