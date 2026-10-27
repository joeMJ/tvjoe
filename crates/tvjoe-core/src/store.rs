//! Lesen und atomares Schreiben der TOML-Dateien.

use std::fmt;

use crate::tf;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    /// Datei ist kein gültiges TOML oder passt nicht zum Format.
    Parse(String),
    /// Datei stammt von einer neueren tvjoe-Version.
    TooNew(u32),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => f.write_str(&tf!("Dateifehler: {error}", error = e)),
            Self::Parse(e) => f.write_str(&tf!("Datei nicht lesbar: {error}", error = e)),
            Self::TooNew(v) => f.write_str(&tf!(
                "Datei hat das neuere Format {version}, bitte tvjoe aktualisieren",
                version = v
            )),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Fehlt die Datei, kommt `T::default()` zurück.
pub fn load_toml<T: DeserializeOwned + Default>(path: &Path) -> Result<T, StoreError> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| StoreError::Parse(e.to_string())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.into()),
    }
}

/// Schreibt über eine Temp-Datei und benennt sie um; eine vorhandene Datei bleibt als `.bak` erhalten.
pub fn save_toml<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let text = toml::to_string_pretty(value).map_err(|e| StoreError::Parse(e.to_string()))?;
    atomic_write(path, text.as_bytes())?;
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if path.exists() {
        fs::copy(path, with_suffix(path, ".bak"))?;
    }
    let tmp = with_suffix(path, ".tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)
}
