//! Netzzugriffe (Logos, später M3U per URL, Erreichbarkeit, Updateprüfung). Blockierend; die
//! Oberfläche ruft sie nur aus Hintergrund-Threads auf.

use std::time::Duration;

pub type Agent = ureq::Agent;

pub fn agent(timeout: Duration) -> Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .into()
}

/// Lädt höchstens `limit` Bytes.
pub fn fetch(agent: &Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    resp.body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

/// Lädt den Anfang einer Seite (höchstens `limit` Bytes, ohne Fehler bei längeren Seiten).
pub fn fetch_prefix(agent: &Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Eine Stufe eines adaptiven HLS-Streams (aus `#EXT-X-STREAM-INF`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variant {
    pub height: u32,
    pub bandwidth: u64,
}

/// Lädt eine große Datei direkt auf die Platte (über `<datei>.part`, dann umbenannt).
pub fn download_to(
    agent: &Agent,
    url: &str,
    path: &std::path::Path,
    limit: u64,
) -> Result<(), String> {
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut reader = resp.body_mut().with_config().limit(limit).reader();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = path.with_extension("part");
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    std::io::copy(&mut reader, &mut file).map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&part, path).map_err(|e| e.to_string())
}

/// Ergebnis der Erreichbarkeitsprüfung.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// Antwort mit HTTP-Status; bei HLS-Master-Playlists die enthaltenen Stufen.
    Ok {
        status: u16,
        variants: Vec<Variant>,
    },
    Failed(String),
    /// Nicht prüfbar (z. B. `rtsp://`).
    Skipped,
}

/// Teilt eine Attributliste `A=1,B="x,y",C=2` an Kommas außerhalb von Anführungszeichen.
fn split_attrs(s: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let (mut start, mut quoted) = (0, false);
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'"' => quoted = !quoted,
            b',' if !quoted => {
                out.extend(s[start..i].split_once('='));
                start = i + 1;
            }
            _ => {}
        }
    }
    out.extend(s[start..].split_once('='));
    out
}

/// Stufen einer HLS-Master-Playlist (Auflösung und Bandbreite).
pub fn parse_variants(playlist: &str) -> Vec<Variant> {
    playlist
        .lines()
        .filter_map(|l| l.trim().strip_prefix("#EXT-X-STREAM-INF:"))
        .filter_map(|attrs| {
            let mut height = None;
            let mut bandwidth = None;
            for (k, v) in split_attrs(attrs) {
                match k.trim() {
                    "RESOLUTION" => {
                        height = v
                            .split_once(['x', 'X'])
                            .and_then(|(_, h)| h.trim().parse().ok())
                    }
                    "BANDWIDTH" => bandwidth = v.trim().parse().ok(),
                    _ => {}
                }
            }
            Some(Variant {
                height: height?,
                bandwidth: bandwidth?,
            })
        })
        .collect()
}

/// Erreichbarkeit, ohne abzuspielen: bei `.m3u8` die Playlist (höchstens 256 KiB) samt
/// enthaltener Stufen, sonst nur der Anfang (`Range`, höchstens 4 KiB).
pub fn probe(agent: &Agent, url: &str) -> Probe {
    match url
        .split_once("://")
        .map(|(s, _)| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("http" | "https") => {}
        Some(_) => return Probe::Skipped,
        None => return Probe::Failed("keine URL".into()),
    }
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_hls = path.ends_with(".m3u8");
    let req = agent.get(url);
    let req = if is_hls {
        req
    } else {
        req.header("Range", "bytes=0-2047")
    };
    match req.call() {
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            let limit = if is_hls { 256 * 1024 } else { 4096 };
            let body = resp
                .body_mut()
                .with_config()
                .limit(limit)
                .read_to_vec()
                .unwrap_or_default();
            let variants = if is_hls {
                parse_variants(&String::from_utf8_lossy(&body))
            } else {
                Vec::new()
            };
            Probe::Ok { status, variants }
        }
        Err(e) => Probe::Failed(e.to_string()),
    }
}

/// Dateiname im Cache für eine URL (FNV-1a, 64 Bit, hexadezimal).
pub fn cache_name(url: &str, ext: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stufen_aus_master_playlist() {
        let m3u8 = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=385000,AVERAGE-BANDWIDTH=300000,CODECS=\"avc1.4d401e,mp4a.40.2\",RESOLUTION=480x270\n\
            chunk_270.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=4180000,RESOLUTION=1920x1080,FRAME-RATE=50.000\n\
            chunk_1080.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=128000,CODECS=\"mp4a.40.2\"\n\
            audio.m3u8\n";
        assert_eq!(
            parse_variants(m3u8),
            [
                Variant {
                    height: 270,
                    bandwidth: 385_000
                },
                Variant {
                    height: 1080,
                    bandwidth: 4_180_000
                },
            ]
        );
        assert!(parse_variants("#EXTM3U\n#EXTINF:6,\nseg1.ts\n").is_empty());
    }

    #[test]
    fn cache_name_ist_stabil_und_verschieden() {
        let a = cache_name("https://x.invalid/a.png", "png");
        assert_eq!(a, cache_name("https://x.invalid/a.png", "png"));
        assert_ne!(a, cache_name("https://x.invalid/b.png", "png"));
        assert!(a.ends_with(".png") && a.len() == 20);
    }
}
