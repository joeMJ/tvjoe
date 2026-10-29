//! Sicherheitsprüfung eingelesener Quellen (M3U, XMLTV): erlaubte Adressen, Adressklassen,
//! Textsäuberung. Netzzugriffe gibt es hier keine; Namen werden nicht aufgelöst.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Höchstlängen in Zeichen; längere Texte werden gekürzt.
pub const MAX_NAME: usize = 200;
pub const MAX_TITLE: usize = 300;
pub const MAX_DESC: usize = 16_000;

/// Schemata, die tvjoe als Stream annimmt (rtp/udp: Multicast-IPTV im Heimnetz; satip,
/// tvheadend, dvb: eigene Adressen).
pub const STREAM_SCHEMES: &[&str] = &[
    "http", "https", "rtsp", "rtsps", "rtp", "udp", "satip", "tvheadend", "dvb",
];

/// Schemata für Bilder (Senderlogos).
pub const IMAGE_SCHEMES: &[&str] = &["http", "https"];

/// Wohin eine Adresse zeigt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostClass {
    /// Internet (oder ein Name, der nicht als lokal erkennbar ist).
    Public,
    /// Eigenes Netz: private Adressbereiche, `.local`, `.lan`, `.box`, `.home.arpa` u. ä.
    Private,
    /// Dieser Rechner: `127.0.0.0/8`, `::1`, `localhost`.
    Loopback,
    /// Link-lokal: `169.254.0.0/16`, `fe80::/10` (u. a. Metadatendienste von Cloud-Rechnern).
    LinkLocal,
    /// `0.0.0.0`, `::`.
    Unspecified,
}

impl HostClass {
    /// Ziel auf diesem Rechner oder link-lokal: für Internet-Quellen nie sinnvoll.
    pub fn is_host_local(self) -> bool {
        matches!(self, Self::Loopback | Self::LinkLocal | Self::Unspecified)
    }
}

/// URL-Schema in Kleinschreibung; `None`, wenn `://` fehlt oder das Schema ungültig ist.
pub fn scheme(url: &str) -> Option<String> {
    let (s, _) = url.split_once("://")?;
    let ok = s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    ok.then(|| s.to_ascii_lowercase())
}

pub fn stream_scheme_allowed(url: &str) -> bool {
    scheme(url).is_some_and(|s| STREAM_SCHEMES.contains(&s.as_str()))
}

pub fn image_scheme_allowed(url: &str) -> bool {
    scheme(url).is_some_and(|s| IMAGE_SCHEMES.contains(&s.as_str()))
}

/// Steuerzeichen (auch Zeilenumbruch und Tabulator) oder Bidi-Steuerzeichen.
pub fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control() || is_bidi_control(c))
}

/// Richtungs-Überschreibungen und -Isolierungen, mit denen sich Text rückwärts tarnen lässt.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Teile einer Adresse: (Benutzerangaben, Host ohne Klammern und Port).
fn authority(url: &str) -> Option<(Option<&str>, &str)> {
    let (_, rest) = url.split_once("://")?;
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let (user, hostport) = match auth.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, auth),
    };
    let host = if let Some(v6) = hostport.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    Some((user, host))
}

/// Ob die Adresse Benutzer oder Passwort enthält (`schema://benutzer:passwort@host/…`).
pub fn has_credentials(url: &str) -> bool {
    authority(url).is_some_and(|(u, _)| u.is_some_and(|u| !u.is_empty()))
}

/// Host einer Adresse in Kleinschreibung.
pub fn host(url: &str) -> Option<String> {
    authority(url).map(|(_, h)| h.to_ascii_lowercase())
}

/// Klasse des Hosts. Zahlenformen wie `2130706433` oder `0x7f.1` (beides 127.0.0.1) werden
/// erkannt, weil der Resolver sie ebenso annimmt.
pub fn host_class(url: &str) -> HostClass {
    let Some(h) = host(url) else {
        return HostClass::Public;
    };
    let h = h.trim_end_matches('.');
    if h.is_empty() {
        // `file:///…` u. ä.: kein Host, also dieser Rechner.
        return HostClass::Loopback;
    }
    if let Some(ip) = parse_ip(h) {
        return ip_class(ip);
    }
    if h == "localhost" || h.ends_with(".localhost") {
        return HostClass::Loopback;
    }
    const LAN: &[&str] = &[".local", ".lan", ".home", ".box", ".internal", ".intranet", ".home.arpa"];
    if !h.contains('.') || LAN.iter().any(|s| h.ends_with(s)) {
        return HostClass::Private;
    }
    HostClass::Public
}

fn parse_ip(h: &str) -> Option<IpAddr> {
    if let Ok(v6) = h.parse::<Ipv6Addr>() {
        return Some(IpAddr::V6(v6));
    }
    inet_aton(h).map(IpAddr::V4)
}

/// IPv4 in allen Formen, die `inet_aton` annimmt: 1–4 Teile, dezimal, oktal (`0…`) oder
/// hexadezimal (`0x…`).
fn inet_aton(h: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = h.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut nums = Vec::with_capacity(parts.len());
    for p in &parts {
        let n = if let Some(x) = p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
            u32::from_str_radix(x, 16).ok()?
        } else if p.len() > 1 && p.starts_with('0') {
            u32::from_str_radix(&p[1..], 8).ok()?
        } else {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse().ok()?
        };
        nums.push(n);
    }
    let (last, head) = nums.split_last()?;
    let rest_bits = 8 * (4 - head.len() as u32);
    if head.iter().any(|&n| n > 255) || (rest_bits < 32 && *last >= 1 << rest_bits) {
        return None;
    }
    let mut v: u32 = 0;
    for (i, &n) in head.iter().enumerate() {
        v |= n << (24 - 8 * i as u32);
    }
    Some(Ipv4Addr::from(v | last))
}

fn ip_class(ip: IpAddr) -> HostClass {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            if v4.is_loopback() {
                HostClass::Loopback
            } else if v4.is_link_local() {
                HostClass::LinkLocal
            } else if v4.is_unspecified() || o[0] == 0 {
                HostClass::Unspecified
            } else if v4.is_private() || (o[0] == 100 && (o[1] & 0xc0) == 64) {
                // RFC 1918 und Carrier-NAT 100.64.0.0/10
                HostClass::Private
            } else {
                HostClass::Public
            }
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ip_class(IpAddr::V4(v4));
            }
            let s = v6.segments();
            if v6.is_loopback() {
                HostClass::Loopback
            } else if v6.is_unspecified() {
                HostClass::Unspecified
            } else if (s[0] & 0xffc0) == 0xfe80 {
                HostClass::LinkLocal
            } else if (s[0] & 0xfe00) == 0xfc00 {
                HostClass::Private
            } else {
                HostClass::Public
            }
        }
    }
}

/// Entfernt Steuer- und Bidi-Steuerzeichen (Zeilenumbrüche nur, wenn `multiline` nicht gesetzt
/// ist; Tabulatoren werden zu Leerzeichen) und kürzt auf `max` Zeichen. Zweiter Wert: ob sich
/// etwas geändert hat.
pub fn clean_text(s: &str, max: usize, multiline: bool) -> (String, bool) {
    let mut out = String::with_capacity(s.len().min(max * 4));
    let mut changed = false;
    let mut count = 0;
    for c in s.chars() {
        let c = match c {
            '\n' if multiline => c,
            '\t' => ' ',
            c if c.is_control() || is_bidi_control(c) => {
                changed = true;
                continue;
            }
            c => c,
        };
        if count == max {
            out.push('…');
            return (out, true);
        }
        out.push(c);
        count += 1;
    }
    (out, changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemata() {
        assert!(stream_scheme_allowed("https://a.example/x.m3u8"));
        assert!(stream_scheme_allowed("RTSP://192.0.2.1/?src=1"));
        assert!(stream_scheme_allowed("tvheadend://192.0.2.5:9981/abc"));
        for u in [
            "file:///etc/passwd",
            "av://lavfi:sine",
            "edl://a;b",
            "memory://x",
            "javascript://alert(1)",
            "data:text/plain,x",
            "/home/x/film.ts",
            "C:\\x.ts",
            "ftp://a.example/x.ts",
        ] {
            assert!(!stream_scheme_allowed(u), "{u}");
        }
        assert!(image_scheme_allowed("https://a.example/l.png"));
        assert!(!image_scheme_allowed("file:///x.png"));
    }

    #[test]
    fn adressklassen() {
        use HostClass::*;
        for (u, c) in [
            ("https://example.org/a", Public),
            ("http://10.0.0.1/admin", Private),
            ("http://10.0.0.5/", Private),
            ("http://172.20.0.1/", Private),
            ("http://100.64.1.1/", Private),
            ("http://fritz.box/", Private),
            ("http://nas.local:8096/", Private),
            ("http://nas:8096/", Private),
            ("http://127.0.0.1:9981/", Loopback),
            ("http://localhost/", Loopback),
            ("http://2130706433/", Loopback),
            ("http://0x7f.1/", Loopback),
            ("http://0177.0.0.1/", Loopback),
            ("http://[::1]/", Loopback),
            ("http://[::ffff:127.0.0.1]/", Loopback),
            ("http://169.254.169.254/latest/meta-data/", LinkLocal),
            ("http://[fe80::1]/", LinkLocal),
            ("http://0.0.0.0/", Unspecified),
            ("http://[fd00::1]/", Private),
            ("http://user:pw@127.0.0.1@example.org/", Public),
            ("file:///etc/passwd", Loopback),
        ] {
            assert_eq!(host_class(u), c, "{u}");
        }
    }

    #[test]
    fn zugangsdaten() {
        assert!(has_credentials("http://nutzer:geheim@iptv.example/live/1.ts"));
        assert!(has_credentials("rtsp://nutzer@cam.example/"));
        assert!(!has_credentials("https://iptv.example/a@b"));
        assert!(!has_credentials("https://iptv.example/x?mail=a@b"));
    }

    #[test]
    fn text_saeubern() {
        assert_eq!(clean_text("Das Erste", 200, false), ("Das Erste".into(), false));
        assert_eq!(clean_text("A\u{1b}[31mB", 200, false), ("A[31mB".into(), true));
        assert_eq!(clean_text("gpj.\u{202e}exe", 200, false), ("gpj.exe".into(), true));
        assert_eq!(clean_text("a\nb", 200, false), ("ab".into(), true));
        assert_eq!(clean_text("a\nb", 200, true), ("a\nb".into(), false));
        assert_eq!(clean_text("abcdef", 3, false), ("abc…".into(), true));
        assert_eq!(clean_text("abc", 3, false), ("abc".into(), false));
    }
}
