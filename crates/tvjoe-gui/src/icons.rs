//! Eigene Symbole: liegen im Zwischenspeicher unter `icons/` und werden dem Symbolthema
//! bekannt gemacht (Knöpfe und Reiter nehmen nur Symbolnamen). Als `-symbolic` färbt GTK sie
//! in der Textfarbe, passend zu hell und dunkel.

use std::path::PathBuf;

use tvjoe_core::paths;

/// SAT-Schüssel (SAT-Quelle).
pub const SATELLITE: &str = "tvjoe-satellite-symbolic";

const SATELLITE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><g fill="#2e3436"><path d="M1.5 2.8C0.2 8.6 4.6 13.4 10.8 12.6Z"/><path d="M5.1 7.4L10.4 2.1L11.6 3.3L6.3 8.6Z"/><circle cx="11.9" cy="2.3" r="1.3"/><path d="M4.9 11h1.8l1.1 3.1H3.8z"/><rect x="2" y="14" width="8" height="1.6" rx="0.5"/></g></svg>"##;

/// Ordner der eigenen Symbole (angelegt und im Symbolthema eingetragen).
pub fn dir() -> Option<PathBuf> {
    let dir = paths::cache_dir()?.join("icons");
    std::fs::create_dir_all(&dir).ok()?;
    let display = gtk::gdk::Display::default()?;
    let theme = gtk::IconTheme::for_display(&display);
    if !theme.search_path().contains(&dir) {
        theme.add_search_path(&dir);
    }
    Some(dir)
}

/// Eigene Symbole ablegen (beim Start).
pub fn install() {
    let Some(dir) = dir() else { return };
    let path = dir.join(format!("{SATELLITE}.svg"));
    if std::fs::read_to_string(&path).ok().as_deref() != Some(SATELLITE_SVG) {
        let _ = std::fs::write(&path, SATELLITE_SVG);
    }
}
