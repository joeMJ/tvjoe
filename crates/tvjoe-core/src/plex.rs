//! Plex Media Server (Nutzer, 2026-10-28): Anmeldung per PIN bei plex.tv, Abgleich der
//! Bibliothek in den gemeinsamen Zwischenspeicher (`jellyfin_lib`), Abspielen (Originaldatei
//! oder umgerechnet als HLS), Meldungen zu Fortschritt und „gesehen“.
//!
//! tvjoe fragt nie nach einem Passwort: Es holt bei plex.tv einen vierstelligen Code, den der
//! Nutzer unter plex.tv/link bestätigt; danach gilt ein Zugangsschlüssel. Er liegt wie bei
//! Jellyfin in einer nur für den Nutzer lesbaren Datei ([`Account`] mit `kind = plex`).

use std::time::Duration;

use serde_json::Value;

use crate::jellyfin::{Account, Playback, Report, ServerKind, read_json};
use crate::jellyfin_lib::{Item, JellyfinLibrary, LibraryInfo, SyncStats};
use crate::{net, tf, tr};

const PRODUCT: &str = "tvjoe";
const PLEX_TV: &str = "https://plex.tv/api/v2";
/// Plex rechnet nur für bekannte Plattformen um; als „Chrome“ gilt das Browser-Profil
/// (H.264/AAC in HLS), das libmpv abspielt.
const PLATFORM: &str = "Chrome";

/// Laufende PIN-Anfrage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub id: u64,
    /// Vierstelliger Code für plex.tv/link.
    pub code: String,
}

fn agent() -> net::Agent {
    net::agent(Duration::from_secs(30))
}

/// Anfrage an plex.tv mit den Kennungen des Clients.
fn tv(req: ureq::RequestBuilder<ureq::typestate::WithoutBody>, client_id: &str) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
    req.header("Accept", "application/json")
        .header("X-Plex-Product", PRODUCT)
        .header("X-Plex-Version", crate::VERSION)
        .header("X-Plex-Client-Identifier", client_id)
}

/// Neue PIN bei plex.tv anfordern.
pub fn pin_start(client_id: &str) -> Result<Pin, String> {
    let v = read_json(
        agent()
            .post(format!("{PLEX_TV}/pins"))
            .header("Accept", "application/json")
            .header("X-Plex-Product", PRODUCT)
            .header("X-Plex-Version", crate::VERSION)
            .header("X-Plex-Client-Identifier", client_id)
            .send_empty(),
    )?;
    Ok(Pin {
        id: v["id"].as_u64().ok_or(tr("plex.tv: keine PIN"))?,
        code: v["code"].as_str().ok_or(tr("plex.tv: kein Code"))?.to_string(),
    })
}

/// Zugangsschlüssel, sobald die PIN bestätigt ist (`None`: noch nicht).
pub fn pin_token(client_id: &str, pin: &Pin) -> Result<Option<String>, String> {
    let v = read_json(tv(agent().get(format!("{PLEX_TV}/pins/{}", pin.id)), client_id).call())?;
    Ok(v["authToken"].as_str().filter(|t| !t.is_empty()).map(str::to_string))
}

/// Zugang zu einem Server: Name des Kontos, Kennung des Servers und der passende Schlüssel
/// (eigener Server: der des Kontos; freigegebener Server: der aus der Ressourcenliste).
pub fn login(server: &str, client_id: &str, token: &str) -> Result<Account, String> {
    let user = read_json(
        tv(agent().get(format!("{PLEX_TV}/user")), client_id)
            .header("X-Plex-Token", token)
            .call(),
    )?;
    let identity = read_json(
        agent()
            .get(format!("{server}/identity"))
            .header("Accept", "application/json")
            .call(),
    )
    .map_err(|e| tf!("Plex-Server nicht erreichbar: {error}", error = e))?;
    let machine = identity["MediaContainer"]["machineIdentifier"]
        .as_str()
        .ok_or(tr("Plex-Server ohne Kennung"))?
        .to_string();
    let resources = read_json(
        tv(agent().get(format!("{PLEX_TV}/resources")), client_id)
            .header("X-Plex-Token", token)
            .call(),
    )?;
    let access = resources
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["clientIdentifier"].as_str() == Some(machine.as_str()))
        .and_then(|r| r["accessToken"].as_str())
        .unwrap_or(token)
        .to_string();
    let acc = Account {
        kind: ServerKind::Plex,
        server: server.to_string(),
        user_id: machine,
        user_name: user["username"]
            .as_str()
            .or(user["title"].as_str())
            .unwrap_or("")
            .to_string(),
        token: access,
        device_id: client_id.to_string(),
    };
    // Probe: Bibliotheken lesbar?
    get(&acc, "/library/sections")
        .map_err(|e| tf!("Kein Zugriff auf den Plex-Server: {error}", error = e))?;
    Ok(acc)
}

/// Angemeldete Anfrage an den Server, Antwort als JSON.
pub(crate) fn get(acc: &Account, path: &str) -> Result<Value, String> {
    read_json(
        net::agent(Duration::from_secs(60))
            .get(format!("{}{path}", acc.server))
            .header("Accept", "application/json")
            .header("X-Plex-Token", &acc.token)
            .header("X-Plex-Client-Identifier", &acc.device_id)
            .header("X-Plex-Product", PRODUCT)
            .call(),
    )
}

/// Angemeldete Anfrage ohne Auswertung der Antwort (Meldungen an den Server).
fn send(acc: &Account, path: &str) -> Result<(), String> {
    net::agent(Duration::from_secs(10))
        .get(format!("{}{path}", acc.server))
        .header("X-Plex-Token", &acc.token)
        .header("X-Plex-Client-Identifier", &acc.device_id)
        .header("X-Plex-Product", PRODUCT)
        .call()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn metadata(v: &Value) -> impl Iterator<Item = &Value> {
    v["MediaContainer"]["Metadata"].as_array().into_iter().flatten()
}

fn s(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().map(str::to_string)
}

fn n(v: &Value, k: &str) -> Option<i64> {
    v[k].as_i64().or_else(|| v[k].as_str().and_then(|x| x.parse().ok()))
}

/// Unix-Zeit als ISO-Text (sortiert wie die Datumsangaben von Jellyfin).
fn iso(ts: Option<i64>) -> Option<String> {
    let ts = ts?;
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    // Tage seit 1970 in Datum (Algorithmus nach Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    ))
}

/// Eintrag aus Plex-Metadaten (Film, Serie, Folge, Sammlung).
pub fn item_from_json(v: &Value) -> Option<Item> {
    let kind = match v["type"].as_str()? {
        "movie" => "Movie",
        "show" => "Series",
        "episode" => "Episode",
        "collection" => "BoxSet",
        _ => return None,
    };
    let name = s(v, "title")?;
    let genres: Vec<&str> = v["Genre"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| g["tag"].as_str())
        .collect();
    let episode = kind == "Episode";
    Some(Item {
        id: s(v, "ratingKey")?,
        kind: kind.to_string(),
        sort_name: s(v, "titleSort").unwrap_or_else(|| name.clone()),
        name,
        series_id: episode.then(|| s(v, "grandparentRatingKey")).flatten(),
        series_name: episode.then(|| s(v, "grandparentTitle")).flatten(),
        season_id: episode.then(|| s(v, "parentRatingKey")).flatten(),
        season_no: episode.then(|| n(v, "parentIndex")).flatten(),
        episode_no: episode.then(|| n(v, "index")).flatten(),
        year: n(v, "year"),
        premiere: s(v, "originallyAvailableAt"),
        runtime_secs: n(v, "duration").unwrap_or(0) / 1000,
        overview: s(v, "summary").unwrap_or_default(),
        genres: genres.join(", "),
        // Pfad des Plakats auf dem Server (Jellyfin: Kennung des Bildes).
        image_tag: s(v, "thumb"),
        date_created: iso(n(v, "addedAt")),
        played: n(v, "viewCount").unwrap_or(0) > 0,
        position_secs: n(v, "viewOffset").unwrap_or(0) as f64 / 1000.0,
        last_played: iso(n(v, "lastViewedAt")),
    })
}

/// Voller Abgleich: Film- und Serienbibliotheken, Folgen, Sammlungen mit ihren Teilen.
pub fn sync(lib: &mut JellyfinLibrary, acc: &Account, now: i64) -> Result<SyncStats, String> {
    let sections = get(acc, "/library/sections")?;
    let libs: Vec<LibraryInfo> = sections["MediaContainer"]["Directory"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let kind = match d["type"].as_str()? {
                "movie" => "movies",
                "show" => "tvshows",
                _ => return None,
            };
            Some(LibraryInfo {
                id: s(d, "key")?,
                name: s(d, "title")?,
                kind: kind.to_string(),
            })
        })
        .collect();
    let mut items: Vec<(String, Item)> = Vec::new();
    let mut collections: Vec<(String, String)> = Vec::new();
    for l in &libs {
        let mut paths = vec![format!("/library/sections/{}/all", l.id)];
        if l.kind == "tvshows" {
            // Typ 4: alle Folgen der Bibliothek in einer Abfrage.
            paths.push(format!("/library/sections/{}/all?type=4", l.id));
        }
        for path in paths {
            let v = get(acc, &path)?;
            items.extend(metadata(&v).filter_map(item_from_json).map(|it| (l.id.clone(), it)));
        }
        let cols = get(acc, &format!("/library/sections/{}/collections", l.id))?;
        for c in metadata(&cols) {
            let Some(mut it) = item_from_json(&serde_json::json!({
                "type": "collection",
                "ratingKey": c["ratingKey"],
                "title": c["title"],
                "titleSort": c["titleSort"],
                "summary": c["summary"],
                "thumb": c["thumb"],
                "addedAt": c["addedAt"],
            })) else {
                continue;
            };
            it.kind = "BoxSet".into();
            let children = get(acc, &format!("/library/collections/{}/children", it.id))?;
            for ch in metadata(&children) {
                if let Some(id) = s(ch, "ratingKey") {
                    collections.push((it.id.clone(), id));
                }
            }
            items.push((l.id.clone(), it));
        }
    }
    lib.store_full(&libs, &items, &collections, now)
}

/// Datei eines Eintrags: Pfad des Teils, Container, Größe, Bitrate (bit/s), Laufzeit (ms).
fn part(acc: &Account, item_id: &str) -> Result<(String, String, u64, u64, u64), String> {
    let v = get(acc, &format!("/library/metadata/{item_id}"))?;
    let m = metadata(&v).next().ok_or(tr("Eintrag nicht gefunden"))?;
    let media = &m["Media"][0];
    let part = &media["Part"][0];
    let key = s(part, "key").ok_or(tr("Eintrag ohne Datei"))?;
    let container = s(part, "container")
        .or_else(|| s(media, "container"))
        .unwrap_or_else(|| "mkv".into());
    let size = part["size"].as_u64().unwrap_or(0);
    // Plex nennt die Bitrate in kbit/s.
    let bitrate = media["bitrate"].as_u64().unwrap_or(0) * 1000;
    let duration = m["duration"].as_u64().unwrap_or(0);
    Ok((key, container, size, bitrate, duration))
}

/// Wie abspielen: Originaldatei, wenn keine Grenze gesetzt ist oder die Datei darunter liegt;
/// sonst vom Server auf H.264/AAC umgerechnet (HLS). Der Schlüssel geht als Kopfzeile mit.
pub fn playback(acc: &Account, item_id: &str, max_kbps: u32, session: &str) -> Result<Playback, String> {
    let (key, _, _, bitrate, duration) = part(acc, item_id)?;
    let limit = u64::from(max_kbps) * 1000;
    if limit == 0 || (bitrate > 0 && bitrate <= limit) {
        return Ok(Playback {
            url: format!("{}{key}", acc.server),
            source_id: key,
            transcoded: false,
            bitrate,
            duration_ms: duration,
        });
    }
    let path = format!("/library/metadata/{item_id}");
    let url = format!(
        "{}/video/:/transcode/universal/start.m3u8?path={}&mediaIndex=0&partIndex=0&protocol=hls\
         &fastSeek=1&directPlay=0&directStream=1&location=lan&maxVideoBitrate={max_kbps}\
         &session={session}&X-Plex-Client-Identifier={}&X-Plex-Product={PRODUCT}\
         &X-Plex-Platform={PLATFORM}",
        acc.server,
        urlencode(&path),
        acc.device_id
    );
    Ok(Playback {
        url,
        source_id: key,
        transcoded: true,
        bitrate,
        duration_ms: duration,
    })
}

/// Originaldatei zum Herunterladen: Adresse (mit Schlüssel als Kopfzeile), Endung, Größe.
pub fn download_source(acc: &Account, item_id: &str) -> Result<(String, String, u64), String> {
    let (key, container, size, _, _) = part(acc, item_id)?;
    Ok((format!("{}{key}", acc.server), container, size))
}

/// Fortschritt an Plex melden (Zeitleiste); am Ende ab 90 % als gesehen markieren.
pub fn report(
    acc: &Account,
    what: Report,
    item_id: &str,
    pb: &Playback,
    session: &str,
    pos: f64,
    paused: bool,
) -> Result<(), String> {
    let state = match (what, paused) {
        (Report::Stop, _) => "stopped",
        (_, true) => "paused",
        _ => "playing",
    };
    let ms = (pos.max(0.0) * 1000.0) as u64;
    send(
        acc,
        &format!(
            "/:/timeline?ratingKey={item_id}&key={}&state={state}&time={ms}&duration={}\
             &X-Plex-Session-Identifier={session}",
            urlencode(&format!("/library/metadata/{item_id}")),
            pb.duration_ms
        ),
    )?;
    if what == Report::Stop && pb.duration_ms > 0 && ms * 10 >= pb.duration_ms * 9 {
        let _ = send(
            acc,
            &format!("/:/scrobble?identifier=com.plexapp.plugins.library&key={item_id}"),
        );
    }
    Ok(())
}

/// Umrechnung auf dem Server beenden.
pub fn stop_transcoding(acc: &Account, session: &str) {
    let _ = send(acc, &format!("/video/:/transcode/universal/stop?session={session}"));
}

/// Plakat in passender Größe (vom Server verkleinert); der Schlüssel steht in der Adresse, weil
/// der Bildlader keine Kopfzeilen setzt.
pub fn poster_url(acc_server: &str, token: &str, thumb: &str, w: u32, h: u32) -> String {
    format!(
        "{acc_server}/photo/:/transcode?width={w}&height={h}&minSize=1&upscale=1&url={}&X-Plex-Token={token}",
        urlencode(thumb)
    )
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gegen einen echten Server: `TVJOE_TEST_PLEX=http://host:32400`,
    /// `TVJOE_TEST_PLEX_TOKEN=<Datei mit Schlüssel>`, `TVJOE_TEST_PLEX_CLIENT=<Kennung>`.
    #[test]
    #[ignore]
    fn echter_server() {
        let server = std::env::var("TVJOE_TEST_PLEX").unwrap();
        let token = std::fs::read_to_string(std::env::var("TVJOE_TEST_PLEX_TOKEN").unwrap()).unwrap();
        let client = std::env::var("TVJOE_TEST_PLEX_CLIENT").unwrap();
        let acc = login(&server, client.trim(), token.trim()).unwrap();
        eprintln!("angemeldet als {} bei {}", acc.user_name, acc.server);
        let path = std::env::temp_dir().join(format!("tvjoe-plex-{}.sqlite", std::process::id()));
        let mut lib = JellyfinLibrary::open(&path).unwrap();
        let st = sync(&mut lib, &acc, 1).unwrap();
        eprintln!("Abgleich: {st:?}, Filme {}", lib.count("Movie"));
        assert!(lib.count("Movie") > 0);
        let first = lib.movies(&[], None, false).into_iter().next().unwrap();
        let direct = playback(&acc, &first.id, 0, "t").unwrap();
        assert!(!direct.transcoded && direct.duration_ms > 0);
        let hls = playback(&acc, &first.id, 1000, "t").unwrap();
        eprintln!("direkt {} kbit/s, umgerechnet: {}", direct.bitrate / 1000, hls.transcoded);
        let (_, ext, size) = download_source(&acc, &first.id).unwrap();
        eprintln!("Download {ext}, {size} Bytes");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn datum_iso() {
        assert_eq!(iso(Some(0)).as_deref(), Some("1970-01-01T00:00:00Z"));
        assert_eq!(iso(Some(1_331_780_555)).as_deref(), Some("2012-03-15T03:02:35Z"));
    }

    #[test]
    fn film_und_folge() {
        let film = serde_json::json!({
            "type": "movie", "ratingKey": "2457", "title": "2 Fast 2 Furious", "year": 2003,
            "duration": 6455872, "addedAt": 1331780555, "viewOffset": 60000,
            "thumb": "/library/metadata/2457/thumb/1", "Genre": [{"tag": "Action"}, {"tag": "Krimi"}]
        });
        let it = item_from_json(&film).unwrap();
        assert_eq!((it.kind.as_str(), it.runtime_secs, it.position_secs), ("Movie", 6455, 60.0));
        assert_eq!(it.genres, "Action, Krimi");
        assert!(!it.played);
        let ep = serde_json::json!({
            "type": "episode", "ratingKey": "9", "title": "Pilot", "grandparentRatingKey": "7",
            "grandparentTitle": "Serie", "parentRatingKey": "8", "parentIndex": 1, "index": 2,
            "viewCount": 1
        });
        let e = item_from_json(&ep).unwrap();
        assert_eq!((e.series_id.as_deref(), e.season_no, e.episode_no), (Some("7"), Some(1), Some(2)));
        assert!(e.played);
        assert_eq!(urlencode("/library/metadata/9"), "%2Flibrary%2Fmetadata%2F9");
    }
}
