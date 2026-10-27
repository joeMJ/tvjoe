//! HDHomeRun-Netzwerktuner (SiliconDust): Gerät und Senderliste über die HTTP-Schnittstelle
//! (`/discover.json`, `/lineup.json`), Streams als HTTP-Transportstrom. Ungetestet (Nutzer,
//! 2026-10-27: kein Gerät vorhanden).

use std::time::Duration;

use serde::Deserialize;

use crate::{tf, tr};

/// Angaben des Geräts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct Info {
    #[serde(rename = "FriendlyName")]
    pub name: String,
    #[serde(rename = "ModelNumber")]
    pub model: String,
    #[serde(rename = "TunerCount")]
    pub tuners: u32,
    #[serde(rename = "FirmwareVersion")]
    pub firmware: String,
}

#[derive(Deserialize)]
struct LineupEntry {
    #[serde(rename = "GuideNumber", default)]
    number: String,
    #[serde(rename = "GuideName", default)]
    name: String,
    #[serde(rename = "URL", default)]
    url: String,
    #[serde(rename = "HD", default)]
    hd: u8,
    #[serde(rename = "DRM", default)]
    drm: u8,
}

fn base(host: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    if h.starts_with("http://") || h.starts_with("https://") {
        h.to_string()
    } else {
        format!("http://{h}")
    }
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    let agent = crate::net::agent(Duration::from_secs(10));
    crate::net::fetch(&agent, url, 4 << 20)
}

/// Gerät abfragen.
pub fn discover(host: &str) -> Result<Info, String> {
    let url = format!("{}/discover.json", base(host));
    let bytes = get(&url)?;
    serde_json::from_slice(&bytes).map_err(|e| tf!("{url}: {error}", url = url, error = e))
}

/// Senderliste des Geräts (ohne verschlüsselte Sender).
pub fn lineup(host: &str) -> Result<Vec<crate::ScannedChannel>, String> {
    let url = format!("{}/lineup.json", base(host));
    let bytes = get(&url)?;
    let entries: Vec<LineupEntry> =
        serde_json::from_slice(&bytes).map_err(|e| tf!("{url}: {error}", url = url, error = e))?;
    let out: Vec<crate::ScannedChannel> = entries
        .into_iter()
        .filter(|e| e.drm == 0 && !e.url.is_empty() && !e.name.trim().is_empty())
        .map(|e| crate::ScannedChannel {
            key: format!("hdhr:{}", e.number),
            name: e.name.trim().to_string(),
            group: None,
            stream: crate::Stream {
                quality: Some(if e.hd == 1 {
                    crate::Quality::P720
                } else {
                    crate::Quality::Sd
                }),
                kind: None,
                url: e.url,
                max_bitrate: None,
            },
            radio: false,
            epg_id: None,
        })
        .collect();
    if out.is_empty() {
        return Err(tr("Das Gerät meldet keine frei empfangbaren Sender (Suchlauf am Gerät nötig?)").into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lineup_entries() {
        let json = br#"[{"GuideNumber":"1","GuideName":"Das Erste HD","VideoCodec":"H264","HD":1,"URL":"http://10.0.0.9:5004/auto/v1"},
                        {"GuideNumber":"2","GuideName":"Pay","DRM":1,"URL":"http://10.0.0.9:5004/auto/v2"}]"#;
        let entries: Vec<LineupEntry> = serde_json::from_slice(json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].hd, 1);
        assert_eq!(entries[1].drm, 1);
        assert_eq!(base("10.0.0.9"), "http://10.0.0.9");
        assert_eq!(base("http://hdhr.local/"), "http://hdhr.local");
    }
}
