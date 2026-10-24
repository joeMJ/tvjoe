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

/// Ergebnis der Erreichbarkeitsprüfung.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// Antwort mit HTTP-Status (2xx).
    Ok(u16),
    Failed(String),
    /// Nicht prüfbar (z. B. `rtsp://`).
    Skipped,
}

/// Erreichbarkeit: fragt nur den Anfang ab (`Range`, höchstens 4 KiB), spielt nichts ab.
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
    match agent.get(url).header("Range", "bytes=0-2047").call() {
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            let _ = resp.body_mut().with_config().limit(4096).read_to_vec();
            Probe::Ok(status)
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
    fn cache_name_ist_stabil_und_verschieden() {
        let a = cache_name("https://x.invalid/a.png", "png");
        assert_eq!(a, cache_name("https://x.invalid/a.png", "png"));
        assert_ne!(a, cache_name("https://x.invalid/b.png", "png"));
        assert!(a.ends_with(".png") && a.len() == 20);
    }
}
