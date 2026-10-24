//! Prüfung der Systembibliotheken, ohne sie zu laden (läuft im Starter vor der Oberfläche).
//!
//! Unter Linux wird nach den Bibliotheksdateien gesucht; fehlen sie, kann der Starter die
//! Ubuntu-Pakete über `pkexec apt-get` nachinstallieren (nur nach Bestätigung). Unter Windows
//! liegen die Bibliotheken dem Programm bei, dort meldet die Prüfung nichts.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Eine Laufzeitbibliothek und das Ubuntu-Paket, das sie liefert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    pub what: &'static str,
    pub soname: &'static str,
    pub package: &'static str,
}

pub const RUNTIME: &[Requirement] = &[
    Requirement {
        what: "GTK 4",
        soname: "libgtk-4.so.1",
        package: "libgtk-4-1",
    },
    Requirement {
        what: "libadwaita",
        soname: "libadwaita-1.so.0",
        package: "libadwaita-1-0",
    },
    Requirement {
        what: "libmpv",
        soname: "libmpv.so.2",
        package: "libmpv2",
    },
    Requirement {
        what: "EGL",
        soname: "libEGL.so.1",
        package: "libegl1",
    },
];

/// Pakete zum Bauen (nur im Entwicklungsmodus geprüft).
pub const DEV_PACKAGES: &[&str] = &[
    "libgtk-4-dev",
    "libadwaita-1-dev",
    "libmpv-dev",
    "pkg-config",
];

fn triplet() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64-linux-gnu"),
        "aarch64" => Some("aarch64-linux-gnu"),
        _ => None,
    }
}

/// Suchorte für Bibliotheken: `LD_LIBRARY_PATH`, dann die üblichen Systemordner.
pub fn lib_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("LD_LIBRARY_PATH")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default();
    if let Some(t) = triplet() {
        dirs.push(Path::new("/usr/lib").join(t));
        dirs.push(Path::new("/lib").join(t));
    }
    dirs.extend(["/usr/lib64", "/usr/lib", "/lib64", "/lib"].map(PathBuf::from));
    dirs
}

fn missing_in<'a>(
    reqs: &'a [Requirement],
    dirs: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
) -> Vec<&'a Requirement> {
    reqs.iter()
        .filter(|r| !dirs.iter().any(|d| exists(&d.join(r.soname))))
        .collect()
}

/// Fehlende Laufzeitbibliotheken (leer, wenn alles da ist).
pub fn missing_runtime() -> Vec<&'static Requirement> {
    if cfg!(windows) {
        return Vec::new();
    }
    missing_in(RUNTIME, &lib_dirs(), Path::exists)
}

/// Fehlende Entwicklungspakete laut `dpkg-query`. Ohne dpkg (kein Debian/Ubuntu) leer.
pub fn missing_dev_packages() -> Vec<&'static str> {
    DEV_PACKAGES
        .iter()
        .copied()
        .filter(|p| {
            Command::new("dpkg-query")
                .args(["-W", "-f=${db:Status-Status}", p])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() != "installed")
                .unwrap_or(false)
        })
        .collect()
}

/// Befehl zur Installation (wird vor dem Ausführen angezeigt).
pub fn install_command(packages: &[&str]) -> Vec<String> {
    let mut cmd: Vec<String> = ["pkexec", "apt-get", "install", "-y"]
        .map(String::from)
        .to_vec();
    cmd.extend(packages.iter().map(|p| p.to_string()));
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn findet_fehlende_bibliotheken() {
        let dirs = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        let vorhanden = ["/b/libgtk-4.so.1", "/a/libEGL.so.1", "/a/libmpv.so.2"];
        let fehlt = missing_in(RUNTIME, &dirs, |p| {
            vorhanden.iter().any(|v| Path::new(v) == p)
        });
        assert_eq!(fehlt.len(), 1);
        assert_eq!(fehlt[0].package, "libadwaita-1-0");
    }

    #[test]
    fn installationsbefehl() {
        assert_eq!(
            install_command(&["libmpv2"]),
            ["pkexec", "apt-get", "install", "-y", "libmpv2"]
        );
    }
}
