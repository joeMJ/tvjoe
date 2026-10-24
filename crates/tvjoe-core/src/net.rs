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
