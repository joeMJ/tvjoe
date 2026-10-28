//! Tvheadend als Empfangsquelle (Nutzer, 2026-10-28): Senderliste über die HTTP-Schnittstelle
//! (`/api/channel/grid`), Wiedergabe über zeitlich begrenzte Tickets (`/playlist/ticket/…`).
//!
//! Tvheadend nimmt in der Grundeinstellung nur die Anmeldung per HTTP-Digest an. Die Zugangsdaten
//! liegen in einer nur für den Nutzer lesbaren Datei (`konten/tvheadend-<host>.toml`); in der
//! Senderliste steht je Sender nur `tvheadend://<host:port>/<kanal-uuid>`. Beim Umschalten holt
//! tvjoe für den Kanal ein Ticket und spielt `…/stream/channelid/…?ticket=…` ab – das Passwort
//! erscheint so nie in einer Stream-Adresse.

use std::path::{Path, PathBuf};
use std::time::Duration;

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use crate::{tf, tr};

/// Zugang zu einem Tvheadend-Server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Access {
    /// Basisadresse, z. B. `http://192.0.2.5:9981`.
    pub server: String,
    pub user: String,
    pub password: String,
}

/// Basisadresse: ohne abschließenden Schrägstrich, ohne Angabe `http://`, ohne Port `:9981`.
pub fn normalize(host: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    let h = if h.contains("://") { h.to_string() } else { format!("http://{h}") };
    // Ohne Port: Vorgabe 9981.
    let after = h.split_once("://").map_or(h.as_str(), |(_, r)| r);
    if after.contains(':') { h } else { format!("{h}:9981") }
}

/// Host und Port einer Basisadresse (`192.0.2.5:9981`).
fn host_of(server: &str) -> &str {
    server.split_once("://").map_or(server, |(_, r)| r).trim_end_matches('/')
}

/// Datei des Zugangs zu einem Server.
pub fn access_path(server: &str) -> Option<PathBuf> {
    let name: String = host_of(server)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    crate::paths::config_dir().map(|d| d.join("konten").join(format!("tvheadend-{name}.toml")))
}

impl Access {
    pub fn load(path: &Path) -> Option<Self> {
        toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    /// Speichern, nur für den Nutzer lesbar.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
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

    /// Angemeldete GET-Anfrage (Digest, bei Bedarf Basic), Antwort als Bytes.
    fn get(&self, path: &str) -> Result<Vec<u8>, String> {
        let url = format!("{}{path}", self.server);
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .build()
            .into();
        let first = agent.get(&url).call().map_err(|e| e.to_string())?;
        let mut resp = if first.status().as_u16() == 401 {
            let challenge = first
                .headers()
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let auth = if challenge.to_ascii_lowercase().starts_with("digest") {
                digest_header(&challenge, &self.user, &self.password, "GET", path, &cnonce())
            } else {
                basic_header(&self.user, &self.password)
            };
            agent.get(&url).header("Authorization", auth).call().map_err(|e| e.to_string())?
        } else {
            first
        };
        match resp.status().as_u16() {
            200 => resp
                .body_mut()
                .with_config()
                .limit(16 << 20)
                .read_to_vec()
                .map_err(|e| e.to_string()),
            401 | 403 => Err(tr("Tvheadend: Benutzer oder Passwort falsch, oder keine Rechte").into()),
            s => Err(tf!("Tvheadend antwortet mit Status {status}", status = s)),
        }
    }

    /// Name und Fassung des Servers (prüft zugleich den Zugang).
    pub fn server_info(&self) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_slice(&self.get("/api/serverinfo")?).map_err(|e| e.to_string())?;
        Ok(format!(
            "{} {}",
            v["name"].as_str().unwrap_or("Tvheadend"),
            v["sw_version"].as_str().unwrap_or("")
        )
        .trim()
        .to_string())
    }

    /// Eingeschaltete Kanäle als Senderliste (nach Nummer, dann Name).
    pub fn channels(&self) -> Result<Vec<crate::ScannedChannel>, String> {
        let v: serde_json::Value =
            serde_json::from_slice(&self.get("/api/channel/grid?limit=100000")?).map_err(|e| e.to_string())?;
        let mut list: Vec<(i64, crate::ScannedChannel)> = v["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["enabled"].as_bool() != Some(false))
            .filter_map(|c| {
                let uuid = c["uuid"].as_str()?;
                let name = c["name"].as_str()?.trim();
                (!name.is_empty()).then(|| {
                    (
                        c["number"].as_i64().unwrap_or(0),
                        crate::ScannedChannel {
                            key: format!("tvh:{uuid}"),
                            name: name.to_string(),
                            group: None,
                            stream: crate::Stream {
                                quality: Some(if name.contains("HD") { crate::Quality::P720 } else { crate::Quality::Sd }),
                                kind: None,
                                url: format!("tvheadend://{}/{uuid}", host_of(&self.server)),
                                max_bitrate: None,
                            },
                            radio: false,
                            epg_id: None,
                        },
                    )
                })
            })
            .collect();
        if list.is_empty() {
            return Err(tr("Tvheadend meldet keine Kanäle (Suchlauf und Kanalzuordnung in Tvheadend nötig?)").into());
        }
        // Ohne Nummer (0) hinter die nummerierten.
        list.sort_by(|a, b| (a.0 == 0, a.0, &a.1.name).cmp(&(b.0 == 0, b.0, &b.1.name)));
        Ok(list.into_iter().map(|(_, c)| c).collect())
    }

    /// Abspieladresse eines Kanals mit frischem Ticket.
    pub fn ticket_url(&self, uuid: &str) -> Result<String, String> {
        let text = String::from_utf8_lossy(&self.get(&format!("/playlist/ticket/channel/{uuid}"))?).to_string();
        text.lines()
            .map(str::trim)
            .find(|l| l.starts_with("http://") || l.starts_with("https://"))
            .map(str::to_string)
            .ok_or_else(|| tr("Tvheadend: keine Abspieladresse für den Kanal").to_string())
    }
}

/// `tvheadend://host:port/uuid` → (Basisadresse, uuid).
pub fn split_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("tvheadend://")?;
    let (host, uuid) = rest.rsplit_once('/')?;
    (!host.is_empty() && !uuid.is_empty()).then(|| (format!("http://{host}"), uuid.to_string()))
}

/// Abspieladresse für einen Sender `tvheadend://…` (Zugang aus der Datei des Servers).
pub fn resolve(url: &str) -> Result<String, String> {
    let (server, uuid) = split_url(url).ok_or_else(|| tr("keine Tvheadend-Adresse").to_string())?;
    let access = access_path(&server)
        .and_then(|p| Access::load(&p))
        .ok_or_else(|| tf!("Kein Zugang zu Tvheadend {server} gespeichert", server = server))?;
    access.ticket_url(&uuid)
}

fn md5_hex(s: &str) -> String {
    Md5::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Zufällige Client-Kennung für Digest (Zeit und Prozess genügen).
fn cnonce() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    md5_hex(&format!("{t}-{}", std::process::id()))[..16].to_string()
}

/// Wert eines Parameters aus `WWW-Authenticate: Digest realm="…", nonce="…", …`.
fn param(challenge: &str, key: &str) -> Option<String> {
    let lower = challenge.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(&format!("{key}=")) {
        let at = from + i;
        // Ganzer Name (nicht „cnonce“ für „nonce“).
        let ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let start = at + key.len() + 1;
        if ok {
            let rest = &challenge[start..];
            return Some(if let Some(r) = rest.strip_prefix('"') {
                r.split('"').next().unwrap_or("").to_string()
            } else {
                rest.split(',').next().unwrap_or("").trim().to_string()
            });
        }
        from = start;
    }
    None
}

/// Kopfzeile `Authorization: Digest …` (MD5, `qop=auth` wenn angeboten).
fn digest_header(challenge: &str, user: &str, password: &str, method: &str, uri: &str, cnonce: &str) -> String {
    let realm = param(challenge, "realm").unwrap_or_default();
    let nonce = param(challenge, "nonce").unwrap_or_default();
    let opaque = param(challenge, "opaque");
    let qop_auth = param(challenge, "qop").is_some_and(|q| q.split(',').any(|x| x.trim() == "auth"));
    let ha1 = md5_hex(&format!("{user}:{realm}:{password}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let nc = "00000001";
    let response = if qop_auth {
        md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"))
    } else {
        md5_hex(&format!("{ha1}:{nonce}:{ha2}"))
    };
    let mut h = format!(
        "Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\""
    );
    if qop_auth {
        h.push_str(&format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\""));
    }
    if let Some(o) = opaque {
        h.push_str(&format!(", opaque=\"{o}\""));
    }
    h
}

/// `Authorization: Basic …` (falls Tvheadend auf „einfach“ gestellt ist).
fn basic_header(user: &str, password: &str) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let data = format!("{user}:{password}").into_bytes();
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    format!("Basic {out}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adressen() {
        assert_eq!(normalize("192.0.2.5"), "http://192.0.2.5:9981");
        assert_eq!(normalize("http://tvh.example:9981/"), "http://tvh.example:9981");
        assert_eq!(normalize("https://tvh.example"), "https://tvh.example:9981");
        assert_eq!(
            split_url("tvheadend://192.0.2.5:9981/46a8fa84"),
            Some(("http://192.0.2.5:9981".into(), "46a8fa84".into()))
        );
        assert_eq!(split_url("http://x/y"), None);
        assert!(access_path("http://192.0.2.5:9981").unwrap().ends_with("konten/tvheadend-192.0.2.5_9981.toml"));
    }

    /// Beispiel aus RFC 2617, Abschnitt 3.5.
    #[test]
    fn digest_wie_rfc2617() {
        let ch = r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#;
        let h = digest_header(ch, "Mufasa", "Circle Of Life", "GET", "/dir/index.html", "0a4f113b");
        assert!(h.contains("response=\"6629fae49393a05397450978507c4ef1\""), "{h}");
        assert!(h.contains("qop=auth, nc=00000001, cnonce=\"0a4f113b\""));
        assert!(h.contains("opaque=\"5ccc069c403ebaf9f0171e9517f40e41\""));
        assert_eq!(param(ch, "nonce").as_deref(), Some("dcd98b7102dd2f0e8b11d0f600bfb0c093"));
        assert_eq!(basic_header("Aladdin", "open sesame"), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
        assert_eq!(basic_header("a", "b"), "Basic YTpi");
    }

    /// Gegen einen echten Server: `TVJOE_TEST_TVH=http://host:9981`,
    /// `TVJOE_TEST_TVH_KEY=<Datei mit „Benutzer:Passwort“>`.
    #[test]
    #[ignore]
    fn echter_server() {
        let server = normalize(&std::env::var("TVJOE_TEST_TVH").unwrap());
        let line = std::fs::read_to_string(std::env::var("TVJOE_TEST_TVH_KEY").unwrap()).unwrap();
        let (user, password) = line.trim().split_once(':').unwrap();
        let a = Access { server, user: user.into(), password: password.into() };
        eprintln!("Server: {}", a.server_info().unwrap());
        let list = a.channels().unwrap();
        eprintln!("{} Kanäle, erste: {:?}", list.len(), list.iter().take(5).map(|c| &c.name).collect::<Vec<_>>());
        let (_, uuid) = split_url(&list[0].stream.url).unwrap();
        let url = a.ticket_url(&uuid).unwrap();
        assert!(url.contains("ticket="));
        eprintln!("Ticket-Adresse erhalten ({} Zeichen)", url.len());
    }
}
