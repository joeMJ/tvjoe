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

/// Film (Filmstreifen): Reiter „Filme“.
pub const FILM: &str = "tvjoe-film-symbolic";

const FILM_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path fill="#2e3436" fill-rule="evenodd" d="M2.5 1.5h11A1.5 1.5 0 0 1 15 3v10a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 1 13V3a1.5 1.5 0 0 1 1.5-1.5zM4.6 3v10h6.8V3zM2.3 3.1v1.6h1.3V3.1zm0 2.8v1.6h1.3V5.9zm0 2.8v1.6h1.3V8.7zm0 2.8v1.6h1.3v-1.6zm10.1-8.4v1.6h1.3V3.1zm0 2.8v1.6h1.3V5.9zm0 2.8v1.6h1.3V8.7zm0 2.8v1.6h1.3v-1.6z"/></svg>"##;

/// Folgen (Bildschirme hintereinander mit Abspielzeichen): Reiter „Episoden“ und „Serien“.
pub const SERIES: &str = "tvjoe-series-symbolic";

const SERIES_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><g fill="#2e3436"><path fill-rule="evenodd" d="M2.2 5.5h9.6a1.2 1.2 0 0 1 1.2 1.2v7.1a1.2 1.2 0 0 1-1.2 1.2H2.2A1.2 1.2 0 0 1 1 13.8V6.7a1.2 1.2 0 0 1 1.2-1.2zM5.5 7.8v4.9l4-2.45z"/><path d="M3.5 3h10.3A1.2 1.2 0 0 1 15 4.2v7.3h-1.3V4.3H3.5z"/><path d="M6 0.6h8.8A1.2 1.2 0 0 1 16 1.8V9h-1.2V1.8H6z" opacity="0.6"/></g></svg>"##;

/// Sammlung (Plakate hintereinander): Reiter „Sammlungen“.
pub const COLLECTION: &str = "tvjoe-collection-symbolic";

const COLLECTION_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><g fill="#2e3436"><rect x="1" y="4.5" width="8.5" height="11" rx="1.1"/><path d="M3.4 2.6h7.3a1.1 1.1 0 0 1 1.1 1.1v10.1h-1.3V3.9H3.4z"/><path d="M5.8 0.5h7.3a1.1 1.1 0 0 1 1.1 1.1v10.1h-1.3V1.8H5.8z" opacity="0.6"/></g></svg>"##;

/// Heruntergeladen (Pfeil in die Ablage): Schalter „Geladen“.
pub const DOWNLOADED: &str = "tvjoe-downloaded-symbolic";

const DOWNLOADED_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><g fill="#2e3436"><path d="M7.2 1h1.6v6.9l2.3-2.3 1.1 1.1L8 10.9 3.8 6.7l1.1-1.1 2.3 2.3z"/><path d="M1 9.5h1.6v3.9h10.8V9.5H15V15H1z"/></g></svg>"##;

/// Umschaltknopf mit Symbol und Beschriftung (Reiter, Schalter in der Kopfleiste).
pub fn toggle(icon: &str, label: &str) -> gtk::ToggleButton {
    gtk::ToggleButton::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name(icon)
                .label(label)
                .use_underline(false)
                .build(),
        )
        .build()
}

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
    for (name, svg) in [
        (SATELLITE, SATELLITE_SVG),
        (ANTENNA, ANTENNA_SVG),
        (CROP, CROP_SVG),
        (FILM, FILM_SVG),
        (SERIES, SERIES_SVG),
        (COLLECTION, COLLECTION_SVG),
        (DOWNLOADED, DOWNLOADED_SVG),
    ] {
        let path = dir.join(format!("{name}.svg"));
        if std::fs::read_to_string(&path).ok().as_deref() != Some(svg) {
            let _ = std::fs::write(&path, svg);
        }
    }
}
