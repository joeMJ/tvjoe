//! Jellyfin-Server: Anmeldung per Quick Connect und gespeicherter Zugang.
//!
//! tvjoe fragt nie nach einem Passwort: Quick Connect zeigt einen Code, der in einem schon
//! angemeldeten Jellyfin bestätigt wird; danach gilt ein Zugangsschlüssel. Er liegt in
//! `jellyfin.toml` im Konfigurationsordner, nur für den Nutzer lesbar.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::net;
use crate::{tf, tr};

/// Art des Medienservers eines Zugangs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    #[default]
    Jellyfin,
    /// Plex Media Server (`crate::plex`); `user_id` ist dort die Serverkennung, `device_id` die
    /// Client-Kennung bei plex.tv.
    Plex,
    /// Emby Server: Schnittstelle wie Jellyfin (gleiche Pfade und Anmeldekopfzeile), Anmeldung
    /// mit Benutzer und Passwort oder API-Schlüssel statt Quick Connect.
    Emby,
}

/// Gespeicherter Zugang.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    #[serde(default)]
    pub kind: ServerKind,
    pub server: String,
    pub user_id: String,
    pub user_name: String,
    pub token: String,
    /// Gerätekennung dieser Installation (bleibt über Anmeldungen hinweg gleich).
    pub device_id: String,
    /// Emby: Zugang mit einem API-Schlüssel (beim Abmelden nur vergessen, nicht beim Server
    /// abmelden – das würde den Schlüssel entwerten).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub api_key: bool,
}

/// Abspielquelle eines Eintrags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playback {
    pub url: String,
    pub source_id: String,
    /// Vom Server umgerechnet (Bandbreitengrenze); braucht den Anmeldekopf auch für Teilstücke.
    pub transcoded: bool,
    /// Bitrate der Originaldatei (bit/s, 0 = unbekannt).
    pub bitrate: u64,
    /// Laufzeit in Millisekunden (Plex braucht sie für Meldungen; 0 = unbekannt).
    pub duration_ms: u64,
}

/// Art der Meldung an Jellyfin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    Start,
    Progress,
    Stop,
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

pub(crate) fn read_json(
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
        code: v["Code"].as_str().ok_or(tr("kein Code"))?.to_string(),
        secret: v["Secret"].as_str().ok_or(tr("kein Secret"))?.to_string(),
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
        kind: ServerKind::Jellyfin,
        server: server.to_string(),
        user_id: v["User"]["Id"]
            .as_str()
            .ok_or(tr("keine Benutzerkennung"))?
            .to_string(),
        user_name: v["User"]["Name"].as_str().unwrap_or("").to_string(),
        token: v["AccessToken"]
            .as_str()
            .ok_or(tr("kein Zugangsschlüssel"))?
            .to_string(),
        device_id: device_id.to_string(),
        api_key: false,
    })
}

/// Emby: Anmeldung mit Benutzer und Passwort (`/Users/AuthenticateByName`).
pub fn emby_login(server: &str, device_id: &str, user: &str, password: &str) -> Result<Account, String> {
    let v = read_json(
        agent()
            .post(format!("{server}/Users/AuthenticateByName"))
            .header("Authorization", auth_header(device_id, None))
            .header("Content-Type", "application/json")
            .send(serde_json::json!({ "Username": user, "Pw": password }).to_string()),
    )
    .map_err(|e| {
        if e.contains("401") {
            tr("Benutzer oder Passwort falsch").to_string()
        } else {
            e
        }
    })?;
    Ok(Account {
        kind: ServerKind::Emby,
        server: server.to_string(),
        user_id: v["User"]["Id"].as_str().ok_or(tr("keine Benutzerkennung"))?.to_string(),
        user_name: v["User"]["Name"].as_str().unwrap_or(user).to_string(),
        token: v["AccessToken"].as_str().ok_or(tr("kein Zugangsschlüssel"))?.to_string(),
        device_id: device_id.to_string(),
        api_key: false,
    })
}

/// Emby: Anmeldung mit einem API-Schlüssel (Einstellungen → Erweitert → API-Schlüssel) für den
/// genannten Benutzer; ohne Namen der einzige bzw. erste Administrator.
pub fn emby_login_key(server: &str, device_id: &str, key: &str, user: &str) -> Result<Account, String> {
    let users = read_json(
        agent()
            .get(format!("{server}/Users"))
            .header("Authorization", auth_header(device_id, Some(key)))
            .call(),
    )
    .map_err(|e| {
        if e.contains("401") {
            tr("API-Schlüssel ungültig").to_string()
        } else {
            e
        }
    })?;
    let list = users.as_array().cloned().unwrap_or_default();
    let found = if user.trim().is_empty() {
        list.iter()
            .find(|u| u["Policy"]["IsAdministrator"].as_bool() == Some(true))
            .or(list.first())
    } else {
        list.iter().find(|u| u["Name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(user.trim())))
    }
    .ok_or_else(|| tf!("Benutzer „{name}“ nicht gefunden", name = user.trim()))?;
    Ok(Account {
        kind: ServerKind::Emby,
        server: server.to_string(),
        user_id: found["Id"].as_str().ok_or(tr("keine Benutzerkennung"))?.to_string(),
        user_name: found["Name"].as_str().unwrap_or("").to_string(),
        token: key.to_string(),
        device_id: device_id.to_string(),
        api_key: true,
    })
}

impl Account {
    /// Kopfzeile für angemeldete Anfragen.
    pub fn header(&self) -> String {
        auth_header(&self.device_id, Some(&self.token))
    }

    /// Vollständige Kopfzeile „Name: Wert“ für Player und Downloads.
    pub fn header_line(&self) -> String {
        match self.kind {
            ServerKind::Jellyfin | ServerKind::Emby => format!("Authorization: {}", self.header()),
            ServerKind::Plex => format!("X-Plex-Token: {}", self.token),
        }
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

    /// Angemeldete GET-Anfrage an einen Pfad mit Abfrageteil (Jellyfin oder Plex), Antwort als
    /// JSON (zusätzliche Angaben, `crate::detail`).
    pub fn get_path(&self, path: &str) -> Result<serde_json::Value, String> {
        if self.kind == ServerKind::Plex {
            return crate::plex::get(self, path);
        }
        read_json(
            net::agent(Duration::from_secs(30))
                .get(format!("{}{path}", self.server))
                .header("Authorization", self.header())
                .call(),
        )
    }

    /// Angemeldete POST-Anfrage mit JSON (Antwort wird nicht ausgewertet).
    pub fn post_json(&self, path: &str, body: &serde_json::Value) -> Result<(), String> {
        net::agent(Duration::from_secs(10))
            .post(format!("{}{path}", self.server))
            .header("Authorization", self.header())
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Wie abspielen: Originaldatei, wenn keine Grenze gesetzt ist oder die Datei darunter
    /// liegt; sonst vom Server auf H.264/AAC mit der Grenze umgerechnet (HLS). Fragt dafür
    /// die Bitrate der Datei beim Server ab.
    pub fn playback(
        &self,
        item_id: &str,
        max_kbps: u32,
        session: &str,
    ) -> Result<Playback, String> {
        if self.kind == ServerKind::Plex {
            return crate::plex::playback(self, item_id, max_kbps, session);
        }
        let v = self.get_json(
            &format!("/Users/{}/Items/{item_id}", self.user_id),
            &[("Fields", "MediaSources")],
        )?;
        let source = &v["MediaSources"][0];
        let source_id = source["Id"].as_str().unwrap_or(item_id).to_string();
        let bitrate = source["Bitrate"].as_u64().unwrap_or(0);
        let limit = u64::from(max_kbps) * 1000;
        if limit == 0 || (bitrate > 0 && bitrate <= limit) {
            return Ok(Playback {
                url: format!(
                    "{}/Videos/{item_id}/stream?static=true&MediaSourceId={source_id}",
                    self.server
                ),
                source_id,
                transcoded: false,
                bitrate,
                duration_ms: 0,
            });
        }
        let audio = 128_000;
        Ok(Playback {
            url: format!(
                "{}/Videos/{item_id}/master.m3u8?MediaSourceId={source_id}&VideoCodec=h264\
                 &AudioCodec=aac&MaxStreamingBitrate={limit}&VideoBitrate={}&AudioBitrate={audio}\
                 &TranscodingMaxAudioChannels=2&SegmentContainer=ts&PlaySessionId={session}\
                 &DeviceId={}",
                self.server,
                limit.saturating_sub(audio),
                self.device_id
            ),
            source_id,
            transcoded: true,
            bitrate,
            duration_ms: 0,
        })
    }

    /// Originaldatei zum Herunterladen: Adresse (mit Anmeldekopf abzurufen), Containerformat
    /// als Dateiendung und Größe in Bytes (0 = unbekannt).
    pub fn download_source(&self, item_id: &str) -> Result<(String, String, u64), String> {
        if self.kind == ServerKind::Plex {
            return crate::plex::download_source(self, item_id);
        }
        let v = self.get_json(
            &format!("/Users/{}/Items/{item_id}", self.user_id),
            &[("Fields", "MediaSources")],
        )?;
        let source = &v["MediaSources"][0];
        let source_id = source["Id"].as_str().unwrap_or(item_id);
        // „mkv“, „mp4“ …; Jellyfin nennt manchmal mehrere („mov,mp4,m4a“): die erste nehmen.
        let container = source["Container"]
            .as_str()
            .and_then(|c| c.split(',').next())
            .filter(|c| !c.is_empty())
            .unwrap_or("mkv")
            .to_string();
        let size = source["Size"].as_u64().unwrap_or(0);
        Ok((
            format!(
                "{}/Videos/{item_id}/stream?static=true&MediaSourceId={source_id}",
                self.server
            ),
            container,
            size,
        ))
    }

    /// Wiedergabe an Jellyfin melden: Start, Fortschritt, Ende (Position in Sekunden).
    pub fn report(
        &self,
        what: Report,
        item_id: &str,
        pb: &Playback,
        session: &str,
        pos: f64,
        paused: bool,
    ) -> Result<(), String> {
        if self.kind == ServerKind::Plex {
            return crate::plex::report(self, what, item_id, pb, session, pos, paused);
        }
        let path = match what {
            Report::Start => "/Sessions/Playing",
            Report::Progress => "/Sessions/Playing/Progress",
            Report::Stop => "/Sessions/Playing/Stopped",
        };
        self.post_json(
            path,
            &serde_json::json!({
                "ItemId": item_id,
                "MediaSourceId": pb.source_id,
                "PlaySessionId": session,
                "PositionTicks": (pos.max(0.0) * 10_000_000.0) as i64,
                "IsPaused": paused,
                "CanSeek": true,
                "PlayMethod": if pb.transcoded { "Transcode" } else { "DirectPlay" },
            }),
        )
    }

    /// Umrechnung auf dem Server beenden (nach dem Abspielen mit Bandbreitengrenze).
    pub fn stop_transcoding(&self, session: &str) {
        if self.kind == ServerKind::Plex {
            return crate::plex::stop_transcoding(self, session);
        }
        let _ = net::agent(Duration::from_secs(10))
            .delete(format!("{}/Videos/ActiveEncodings", self.server))
            .query("DeviceId", &self.device_id)
            .query("PlaySessionId", session)
            .header("Authorization", self.header())
            .call();
    }

    /// Abmelden: Schlüssel beim Server ungültig machen (Fehler werden ignoriert).
    pub fn logout(&self) {
        // Plex: der Schlüssel gilt für das Konto; tvjoe vergisst ihn nur.
        if self.kind == ServerKind::Plex || self.api_key {
            return;
        }
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
    // Emby: <link rel="apple-touch-icon" href="images/icon-192x192.png">
    if !html.contains("touchicon") {
        let at = html.find("apple-touch-icon\" href=\"").ok_or(tr("kein Logo auf der Startseite"))?;
        let rest = &html[at + "apple-touch-icon\" href=\"".len()..];
        let end = rest.find('"').ok_or(tr("Logo-Adresse unvollständig"))?;
        return Ok(format!("{server}/web/{}", &rest[..end]));
    }
    let start = html
        .find("touchicon")
        .ok_or(tr("kein Logo auf der Startseite"))?;
    let end = start
        + html[start..]
            .find(['"', '\''])
            .ok_or(tr("Logo-Adresse unvollständig"))?;
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

    /// Gegen einen echten Emby-Server: `TVJOE_TEST_EMBY=http://host:8096`,
    /// `TVJOE_TEST_EMBY_KEY=<Datei mit API-Schlüssel>`; Anmeldung, Abgleich, Abspielen.
    #[test]
    #[ignore]
    fn emby_echter_server() {
        let server = std::env::var("TVJOE_TEST_EMBY").unwrap();
        let key = std::fs::read_to_string(std::env::var("TVJOE_TEST_EMBY_KEY").unwrap()).unwrap();
        let acc = emby_login_key(&server, "tvjoe-test", key.trim(), "").unwrap();
        eprintln!("angemeldet als {} ({:?})", acc.user_name, acc.kind);
        let path = std::env::temp_dir().join(format!("tvjoe-emby-{}.sqlite", std::process::id()));
        let mut lib = crate::jellyfin_lib::JellyfinLibrary::open(&path).unwrap();
        let st = lib.sync_full(&acc, 1).unwrap();
        eprintln!("Abgleich: {st:?}, Filme {}, Serien {}, Folgen {}", lib.count("Movie"), lib.count("Series"), lib.count("Episode"));
        assert!(lib.count("Movie") > 0);
        let first = lib.movies(&[], None, false).into_iter().next().unwrap();
        eprintln!("erster Film: {}", first.name);
        let direct = acc.playback(&first.id, 0, "t").unwrap();
        assert!(!direct.transcoded);
        let hls = acc.playback(&first.id, 1000, "t").unwrap();
        eprintln!("direkt {} kbit/s, umgerechnet: {}", direct.bitrate / 1000, hls.transcoded);
        let (_, ext, size) = acc.download_source(&first.id).unwrap();
        eprintln!("Download {ext}, {size} Bytes");
        // Liefert der Server die Adressen aus? HLS-Liste und Anfang der Originaldatei.
        let fetch = |url: &str| {
            let mut r = agent().get(url).header("Authorization", acc.header()).header("Range", "bytes=0-1023").call().unwrap();
            let status = r.status().as_u16();
            let body = r.body_mut().with_config().limit(1 << 20).read_to_vec().unwrap_or_default();
            (status, body)
        };
        let (st, body) = fetch(&hls.url);
        eprintln!("HLS: HTTP {st}, beginnt mit {:?}", String::from_utf8_lossy(&body[..body.len().min(7)]));
        assert!(body.starts_with(b"#EXTM3U"));
        let (st, body) = fetch(&direct.url);
        eprintln!("Original: HTTP {st}, {} Bytes gelesen", body.len());
        assert!(st == 200 || st == 206);
        acc.stop_transcoding("t");
        let _ = std::fs::remove_file(path);
    }

    /// Emby: Anmeldung mit Benutzer und Passwort; `TVJOE_TEST_EMBY=http://host:8096`,
    /// `TVJOE_TEST_EMBY_PASS=<Datei mit „Benutzer:Passwort“>`. Falsches Passwort: klare Meldung.
    #[test]
    #[ignore]
    fn emby_passwort() {
        let server = std::env::var("TVJOE_TEST_EMBY").unwrap();
        let line = std::fs::read_to_string(std::env::var("TVJOE_TEST_EMBY_PASS").unwrap()).unwrap();
        let (user, password) = line.trim().split_once(':').unwrap();
        let acc = emby_login(&server, "tvjoe-test", user, password).unwrap();
        assert!(!acc.api_key && !acc.token.is_empty() && !acc.user_id.is_empty());
        eprintln!("angemeldet als {} ({:?}), Zugangsschlüssel erhalten", acc.user_name, acc.kind);
        let v = acc.get_json(&format!("/Users/{}/Items", acc.user_id), &[("Limit", "1"), ("Recursive", "true")]).unwrap();
        eprintln!("Bibliothek lesbar: {} Einträge insgesamt", v["TotalRecordCount"]);
        let wrong = emby_login(&server, "tvjoe-test", user, "falsch-falsch").unwrap_err();
        eprintln!("falsches Passwort: {wrong}");
        // Sitzung wieder beenden (der Zugangsschlüssel gilt dann nicht mehr).
        acc.logout();
    }

    #[test]
    fn zugang_speichern_nur_fuer_nutzer() {
        let dir = std::env::temp_dir().join(format!("tvjoe-jf-{}", std::process::id()));
        let path = dir.join("jellyfin.toml");
        let acc = Account {
            kind: ServerKind::Jellyfin,
            server: "https://x.invalid".into(),
            user_id: "u".into(),
            user_name: "n".into(),
            token: "geheim".into(),
            device_id: "d".into(),
            api_key: false,
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
