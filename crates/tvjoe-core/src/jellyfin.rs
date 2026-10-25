//! Jellyfin-Server: Anmeldung per Quick Connect und gespeicherter Zugang.
//!
//! tvjoe fragt nie nach einem Passwort: Quick Connect zeigt einen Code, der in einem schon
//! angemeldeten Jellyfin bestätigt wird; danach gilt ein Zugangsschlüssel. Er liegt in
//! `jellyfin.toml` im Konfigurationsordner, nur für den Nutzer lesbar.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::net;

/// Gespeicherter Zugang.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub server: String,
    pub user_id: String,
    pub user_name: String,
    pub token: String,
    /// Gerätekennung dieser Installation (bleibt über Anmeldungen hinweg gleich).
    pub device_id: String,
}

/// Öffentliche Angaben des Servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// Laufende Quick-Connect-Anfrage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickConnect {
    pub code: String,
    secret: String,
}

/// Adresse vereinheitlichen: ohne abschließenden Schrägstrich, ohne `/web`.
pub fn normalize_server(url: &str) -> String {
    let mut s = url.trim().trim_end_matches('/').to_string();
    if let Some(stripped) = s.strip_suffix("/web") {
        s = stripped.to_string();
    }
    if !s.contains("://") && !s.is_empty() {
        s = format!("https://{s}");
    }
    s
}

/// Neue Gerätekennung (zufällig genug aus Zeit und Prozess, ohne weitere Abhängigkeit).
pub fn new_device_id() -> String {
    use sha2::{Digest, Sha256};
    let seed = format!(
        "{:?}-{}-{:?}",
        std::time::SystemTime::now(),
        std::process::id(),
        std::thread::current().id()
    );
    let hash = Sha256::digest(seed.as_bytes());
    hash.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

fn auth_header(device_id: &str, token: Option<&str>) -> String {
    let mut h = format!(
        "MediaBrowser Client=\"tvjoe\", Device=\"tvjoe\", DeviceId=\"{device_id}\", Version=\"{}\"",
        crate::VERSION
    );
    if let Some(t) = token {
        h.push_str(&format!(", Token=\"{t}\""));
    }
    h
}

fn agent() -> net::Agent {
    net::agent(Duration::from_secs(20))
}

fn read_json(
    resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<serde_json::Value, String> {
    let mut resp = resp.map_err(|e| e.to_string())?;
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// Öffentliche Angaben (ohne Anmeldung) – prüft zugleich die Adresse.
pub fn server_info(server: &str) -> Result<ServerInfo, String> {
    let v = read_json(agent().get(format!("{server}/System/Info/Public")).call())?;
    Ok(ServerInfo {
        name: v["ServerName"].as_str().unwrap_or("").to_string(),
        version: v["Version"].as_str().unwrap_or("").to_string(),
    })
}

/// Ist Quick Connect auf dem Server eingeschaltet?
pub fn quick_connect_enabled(server: &str) -> Result<bool, String> {
    let v = read_json(agent().get(format!("{server}/QuickConnect/Enabled")).call())?;
    Ok(v.as_bool().unwrap_or(false))
}

/// Quick Connect starten: liefert den Code zum Bestätigen.
pub fn quick_connect_start(server: &str, device_id: &str) -> Result<QuickConnect, String> {
    let v = read_json(
        agent()
            .post(format!("{server}/QuickConnect/Initiate"))
            .header("Authorization", auth_header(device_id, None))
            .send_empty(),
    )?;
    Ok(QuickConnect {
        code: v["Code"].as_str().ok_or("kein Code")?.to_string(),
        secret: v["Secret"].as_str().ok_or("kein Secret")?.to_string(),
    })
}

/// Wurde der Code bestätigt?
pub fn quick_connect_confirmed(
    server: &str,
    device_id: &str,
    qc: &QuickConnect,
) -> Result<bool, String> {
    let v = read_json(
        agent()
            .get(format!("{server}/QuickConnect/Connect"))
            .query("Secret", &qc.secret)
            .header("Authorization", auth_header(device_id, None))
            .call(),
    )?;
    Ok(v["Authenticated"].as_bool().unwrap_or(false))
}

/// Nach der Bestätigung: Zugangsschlüssel holen.
pub fn quick_connect_finish(
    server: &str,
    device_id: &str,
    qc: &QuickConnect,
) -> Result<Account, String> {
    let v = read_json(
        agent()
            .post(format!("{server}/Users/AuthenticateWithQuickConnect"))
            .header("Authorization", auth_header(device_id, None))
            .header("Content-Type", "application/json")
            .send(serde_json::json!({ "Secret": qc.secret }).to_string()),
    )?;
    Ok(Account {
        server: server.to_string(),
        user_id: v["User"]["Id"]
            .as_str()
            .ok_or("keine Benutzerkennung")?
            .to_string(),
        user_name: v["User"]["Name"].as_str().unwrap_or("").to_string(),
        token: v["AccessToken"]
            .as_str()
            .ok_or("kein Zugangsschlüssel")?
            .to_string(),
        device_id: device_id.to_string(),
    })
}

impl Account {
    /// Kopfzeile für angemeldete Anfragen.
    pub fn header(&self) -> String {
        auth_header(&self.device_id, Some(&self.token))
    }

    /// Angemeldete GET-Anfrage, Antwort als JSON.
    pub fn get_json(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<serde_json::Value, String> {
        let mut req = net::agent(Duration::from_secs(60))
            .get(format!("{}{path}", self.server))
            .header("Authorization", self.header());
        for (k, v) in query {
            req = req.query(*k, *v);
        }
        read_json(req.call())
    }

    /// Abmelden: Schlüssel beim Server ungültig machen (Fehler werden ignoriert).
    pub fn logout(&self) {
        let _ = agent()
            .post(format!("{}/Sessions/Logout", self.server))
            .header("Authorization", self.header())
            .send_empty();
    }

    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        toml::from_str(&text).ok()
    }

    /// Speichert nur für den Nutzer lesbar (Rechte 600).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp).map_err(|e| e.to_string())?;
            f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

/// Adresse des Logos auf der Startseite des Servers (`touchicon….png`, Name wechselt je
/// Version).
pub fn logo_url(server: &str) -> Result<String, String> {
    let html = net::fetch_prefix(&agent(), &format!("{server}/web/"), 512 * 1024)?;
    let html = String::from_utf8_lossy(&html);
    let start = html
        .find("touchicon")
        .ok_or("kein Logo auf der Startseite")?;
    let end = start
        + html[start..]
            .find(['"', '\''])
            .ok_or("Logo-Adresse unvollständig")?;
    Ok(format!("{server}/web/{}", &html[start..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adresse_vereinheitlichen() {
        assert_eq!(
            normalize_server("https://jelly.example/"),
            "https://jelly.example"
        );
        assert_eq!(
            normalize_server("jelly.example/web"),
            "https://jelly.example"
        );
        assert_eq!(
            normalize_server(" http://10.0.0.2:8096/web/ "),
            "http://10.0.0.2:8096"
        );
    }

    #[test]
    fn kopfzeile_mit_und_ohne_schluessel() {
        let h = auth_header("abc", None);
        assert!(h.starts_with("MediaBrowser Client=\"tvjoe\"") && h.contains("DeviceId=\"abc\""));
        assert!(!h.contains("Token"));
        assert!(auth_header("abc", Some("t")).ends_with(", Token=\"t\""));
    }

    #[test]
    fn zugang_speichern_nur_fuer_nutzer() {
        let dir = std::env::temp_dir().join(format!("tvjoe-jf-{}", std::process::id()));
        let path = dir.join("jellyfin.toml");
        let acc = Account {
            server: "https://x.invalid".into(),
            user_id: "u".into(),
            user_name: "n".into(),
            token: "geheim".into(),
            device_id: "d".into(),
        };
        acc.save(&path).unwrap();
        assert_eq!(Account::load(&path), Some(acc));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
