//! Updateprüfung über GitHub-Releases (Projektplan 5c): standardmäßig aus, keine Telemetrie.
//! Die Abfrage übermittelt die IP-Adresse an GitHub. Herunterladen nur nach Bestätigung;
//! das Paket wird gegen `SHA256SUMS` aus demselben Release geprüft.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::VERSION;
use crate::net;

/// Öffentliches Repository (Veröffentlichung über den gefilterten Branch).
pub const REPO: &str = "joeMJ/tvjoe";
const MAX_PACKAGE_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// `v1.2.3` oder `1.2.3` (Zusätze nach `-`/`+` werden ignoriert).
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        it.next()??,
        it.next().flatten().unwrap_or(0),
        it.next().flatten().unwrap_or(0),
    ))
}

pub fn is_newer(current: &str, candidate: &str) -> bool {
    matches!((parse_version(current), parse_version(candidate)), (Some(a), Some(b)) if b > a)
}

pub fn parse_release(json: &str) -> Result<Release, String> {
    serde_json::from_str(json).map_err(|e| format!("Antwort von GitHub unlesbar: {e}"))
}

/// Distributionskennung aus `/etc/os-release`, z. B. `ubuntu26.04`.
pub fn distro_tag() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    distro_tag_from(&text)
}

fn distro_tag_from(os_release: &str) -> Option<String> {
    let field = |key: &str| {
        os_release.lines().find_map(|l| {
            l.strip_prefix(key)
                .and_then(|v| v.strip_prefix('='))
                .map(|v| v.trim().trim_matches('"').to_string())
        })
    };
    Some(format!("{}{}", field("ID")?, field("VERSION_ID")?))
}

/// Debian-Architektur dieses Programms.
pub fn deb_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

impl Release {
    /// Paket für Distribution und Architektur, Namensschema `tvjoe_<version>_<distro>_<arch>.deb`.
    pub fn package_for(&self, distro: &str, arch: &str) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|a| a.name.ends_with(".deb") && a.name.contains(&format!("_{distro}_{arch}")))
    }

    pub fn sums(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == "SHA256SUMS")
    }
}

/// Prüfsumme zu einem Dateinamen aus einer `sha256sum`-Ausgabe.
pub fn expected_sum(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn agent() -> net::Agent {
    net::agent(Duration::from_secs(30))
}

/// Fragt das neueste Release ab. `Ok(None)`: keine neuere Version (oder noch keine Veröffentlichung).
pub fn check() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = agent()
        .get(&url)
        .header("User-Agent", format!("tvjoe/{VERSION}"))
        .header("Accept", "application/vnd.github+json")
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if status == 404 {
        return Ok(None);
    }
    if status != 200 {
        return Err(format!("GitHub antwortet mit Status {status}"));
    }
    let mut resp = resp;
    let json = resp
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let release = parse_release(&json)?;
    Ok(
        (!release.draft && !release.prerelease && is_newer(VERSION, &release.tag_name))
            .then_some(release),
    )
}

/// Lädt das passende Paket und prüft es gegen `SHA256SUMS`. Gibt den Pfad der Datei zurück.
pub fn download(release: &Release, dir: &Path) -> Result<PathBuf, String> {
    let distro = distro_tag().ok_or("Distribution unbekannt (/etc/os-release)")?;
    let asset = release
        .package_for(&distro, deb_arch())
        .ok_or_else(|| format!("Kein Paket für {distro}/{} im Release", deb_arch()))?;
    let sums = release
        .sums()
        .ok_or("Release ohne SHA256SUMS – Download abgelehnt")?;
    let agent = agent();
    let sums_text = String::from_utf8(net::fetch(&agent, &sums.browser_download_url, 1024 * 1024)?)
        .map_err(|_| "SHA256SUMS unlesbar")?;
    let expected = expected_sum(&sums_text, &asset.name)
        .ok_or_else(|| format!("{} fehlt in SHA256SUMS", asset.name))?;
    let bytes = net::fetch(&agent, &asset.browser_download_url, MAX_PACKAGE_BYTES)?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(format!(
            "Prüfsumme falsch (erwartet {expected}, erhalten {actual})"
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(&asset.name);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Befehl zur Installation eines heruntergeladenen Pakets (wird vor dem Ausführen angezeigt).
pub fn install_command(package: &Path) -> Vec<String> {
    vec![
        "pkexec".into(),
        "apt-get".into(),
        "install".into(),
        "-y".into(),
        package.to_string_lossy().into_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{
        "tag_name": "v0.2.0", "name": "tvjoe 0.2.0", "body": "Neu: Merkliste",
        "html_url": "https://github.com/joeMJ/tvjoe/releases/tag/v0.2.0",
        "draft": false, "prerelease": false,
        "assets": [
            {"name": "tvjoe_0.2.0_ubuntu24.04_amd64.deb", "browser_download_url": "https://x.invalid/a.deb", "size": 10},
            {"name": "tvjoe_0.2.0_ubuntu26.04_amd64.deb", "browser_download_url": "https://x.invalid/b.deb", "size": 11},
            {"name": "SHA256SUMS", "browser_download_url": "https://x.invalid/SHA256SUMS", "size": 1}
        ]
    }"#;

    #[test]
    fn versionen_vergleichen() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10"), Some((0, 10, 0)));
        assert_eq!(parse_version("v2.0.0-rc1"), Some((2, 0, 0)));
        assert!(is_newer("0.0.1", "v0.1.0"));
        assert!(is_newer("0.9.9", "0.10.0"));
        assert!(!is_newer("0.2.0", "v0.2.0"));
        assert!(!is_newer("0.2.0", "kaputt"));
    }

    #[test]
    fn release_und_paketwahl() {
        let r = parse_release(JSON).unwrap();
        assert_eq!(r.tag_name, "v0.2.0");
        assert_eq!(
            r.package_for("ubuntu26.04", "amd64")
                .unwrap()
                .browser_download_url,
            "https://x.invalid/b.deb"
        );
        assert!(r.package_for("ubuntu22.04", "amd64").is_none());
        assert!(r.package_for("ubuntu26.04", "arm64").is_none());
        assert!(r.sums().is_some());
    }

    #[test]
    fn pruefsummen() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let sums = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  tvjoe_0.2.0_ubuntu26.04_amd64.deb\n0000  andere.deb\n";
        assert_eq!(
            expected_sum(sums, "tvjoe_0.2.0_ubuntu26.04_amd64.deb").as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert!(expected_sum(sums, "fehlt.deb").is_none());
    }

    #[test]
    fn distribution_aus_os_release() {
        let text = "NAME=\"Ubuntu\"\nVERSION_ID=\"26.04\"\nID=ubuntu\n";
        assert_eq!(distro_tag_from(text).as_deref(), Some("ubuntu26.04"));
        assert!(distro_tag_from("NAME=x\n").is_none());
    }
}
