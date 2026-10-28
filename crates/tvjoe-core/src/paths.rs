//! Ablageorte für Konfiguration und Cache (Linux: XDG, Windows: `%APPDATA%`/`%LOCALAPPDATA%`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Konfigurationsordner, z. B. `~/.config/tvjoe`. `TVJOE_CONFIG_DIR` überschreibt (Tests, Messungen).
pub fn config_dir() -> Option<PathBuf> {
    nonempty(std::env::var_os("TVJOE_CONFIG_DIR"))
        .or_else(|| config_dir_from(cfg!(windows), |k| std::env::var_os(k)))
}

/// Cacheordner, z. B. `~/.cache/tvjoe`. `TVJOE_CACHE_DIR` überschreibt.
pub fn cache_dir() -> Option<PathBuf> {
    nonempty(std::env::var_os("TVJOE_CACHE_DIR"))
        .or_else(|| cache_dir_from(cfg!(windows), |k| std::env::var_os(k)))
}

pub fn library_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("library.toml"))
}

pub fn settings_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.toml"))
}

/// Senderliste einer Empfangsquelle: Internet-TV `library.toml`, SAT `satip.toml`, weitere
/// `listen/<kennung>.toml`.
pub fn source_library_path(adapter_id: &str) -> Option<PathBuf> {
    match adapter_id {
        crate::settings::ADAPTER_TV => library_path(),
        crate::settings::ADAPTER_SAT => satip_library_path(),
        id => config_dir().map(|d| d.join("listen").join(format!("{id}.toml"))),
    }
}

/// Senderliste der SAT-Quelle (aus dem Suchlauf).
pub fn satip_library_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("satip.toml"))
}

/// Zwischenspeicher der Jellyfin-Bibliothek.
pub fn jellyfin_db() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("jellyfin.sqlite"))
}

/// Zugang zum Jellyfin-Server (nur für den Nutzer lesbar).
pub fn jellyfin_account_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("jellyfin.toml"))
}

/// Zwischenspeicher einer Server-Bibliothek; die erste Jellyfin-Bibliothek behält
/// `jellyfin.sqlite`.
pub fn library_db(id: &str) -> Option<PathBuf> {
    if id == crate::settings::LIBRARY_JELLYFIN {
        return jellyfin_db();
    }
    cache_dir().map(|d| d.join(format!("bibliothek-{id}.sqlite")))
}

/// Zugang zu einer Server-Bibliothek; die erste Jellyfin-Bibliothek behält `jellyfin.toml`.
pub fn library_account_path(id: &str) -> Option<PathBuf> {
    if id == crate::settings::LIBRARY_JELLYFIN {
        return jellyfin_account_path();
    }
    config_dir().map(|d| d.join("konten").join(format!("{id}.toml")))
}

/// Ort einer Datei ohne das Windows-Präfix `\\?\` (Langform aus `canonicalize`), damit
/// gespeicherte Orte lesbar bleiben und gleiche Dateien gleich heißen.
pub fn plain_location(location: &str) -> String {
    if let Some(rest) = location.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = location.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        location.to_string()
    }
}

/// Vollständiger Ort einer Datei (ohne Windows-Präfix `\\?\`).
pub fn file_location(path: &Path) -> String {
    let full = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    plain_location(&full.to_string_lossy())
}

pub fn logo_cache_dir() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("logos"))
}

/// Datenbank der Mediathek (aus der Filmliste neu aufbaubar, daher im Cache).
pub fn mediathek_db() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("mediathek.sqlite"))
}

/// Vorschaubilder der Mediathek.
pub fn thumb_cache_dir() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("thumbs"))
}

/// Nicht leer und (laut XDG) absolut.
fn absolute(v: Option<OsString>) -> Option<PathBuf> {
    let p = PathBuf::from(v.filter(|s| !s.is_empty())?);
    p.is_absolute().then_some(p)
}

fn nonempty(v: Option<OsString>) -> Option<PathBuf> {
    v.filter(|s| !s.is_empty()).map(PathBuf::from)
}

fn config_dir_from(windows: bool, get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if windows {
        return nonempty(get("APPDATA")).map(|p| p.join("tvjoe"));
    }
    if let Some(x) = absolute(get("XDG_CONFIG_HOME")) {
        return Some(x.join("tvjoe"));
    }
    nonempty(get("HOME")).map(|h| h.join(".config").join("tvjoe"))
}

fn cache_dir_from(windows: bool, get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if windows {
        return nonempty(get("LOCALAPPDATA")).map(|p| p.join("tvjoe").join("cache"));
    }
    if let Some(x) = absolute(get("XDG_CACHE_HOME")) {
        return Some(x.join("tvjoe"));
    }
    nonempty(get("HOME")).map(|h| h.join(".cache").join("tvjoe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn linux_nimmt_xdg_sonst_home() {
        let e = env(&[("XDG_CONFIG_HOME", "/x/cfg"), ("HOME", "/h")]);
        assert_eq!(
            config_dir_from(false, e),
            Some(PathBuf::from("/x/cfg/tvjoe"))
        );
        let e = env(&[("HOME", "/h")]);
        assert_eq!(
            config_dir_from(false, e),
            Some(PathBuf::from("/h/.config/tvjoe"))
        );
        let e = env(&[("HOME", "/h")]);
        assert_eq!(
            cache_dir_from(false, e),
            Some(PathBuf::from("/h/.cache/tvjoe"))
        );
    }

    #[test]
    fn relative_xdg_wird_ignoriert() {
        let e = env(&[("XDG_CONFIG_HOME", "relativ"), ("HOME", "/h")]);
        assert_eq!(
            config_dir_from(false, e),
            Some(PathBuf::from("/h/.config/tvjoe"))
        );
    }

    #[test]
    fn windows_nimmt_appdata() {
        let e = env(&[("APPDATA", r"C:\Users\x\AppData\Roaming")]);
        assert_eq!(
            config_dir_from(true, e),
            Some(PathBuf::from(r"C:\Users\x\AppData\Roaming").join("tvjoe"))
        );
        let e = env(&[("LOCALAPPDATA", "L")]);
        assert_eq!(
            cache_dir_from(true, e),
            Some(PathBuf::from("L").join("tvjoe").join("cache"))
        );
    }

    #[test]
    fn ohne_umgebung_kein_pfad() {
        assert_eq!(config_dir_from(false, env(&[])), None);
    }
}
