//! MPEG-TS: Abschnitte (Sections) aus Transportpaketen zusammensetzen und die Tabellen lesen,
//! die der SAT>IP-Suchlauf braucht: PAT (Programme → PMT-PID), PMT (Spuren), SDT (Name,
//! Anbieter, Art, verschlüsselt), NIT (Empfangsdaten aller Transponder).

use std::collections::HashMap;

/// Sammelt Abschnitte je PID aus 188-Byte-Paketen.
#[derive(Default)]
pub struct SectionCollector {
    buf: HashMap<u16, Vec<u8>>,
    /// Fertige Abschnitte (PID, Bytes); gleiche Abschnitte nur einmal.
    pub sections: Vec<(u16, Vec<u8>)>,
    seen: std::collections::HashSet<(u16, u8, u16, u8)>,
}

impl SectionCollector {
    /// Nimmt ein Transportpaket an, wenn seine PID in `wanted` liegt (leer = alle).
    pub fn push_packet(&mut self, p: &[u8], wanted: &dyn Fn(u16) -> bool) {
        if p.len() < 188 || p[0] != 0x47 {
            return;
        }
        let pid = (u16::from(p[1] & 0x1f) << 8) | u16::from(p[2]);
        if !wanted(pid) {
            return;
        }
        let pusi = p[1] & 0x40 != 0;
        let afc = (p[3] >> 4) & 3;
        let payload: &[u8] = match afc {
            1 => &p[4..188],
            3 => {
                let n = p[4] as usize;
                if 5 + n >= 188 {
                    return;
                }
                &p[5 + n..188]
            }
            _ => return,
        };
        if pusi {
            let ptr = payload[0] as usize;
            if 1 + ptr > payload.len() {
                return;
            }
            if let Some(b) = self.buf.get_mut(&pid) {
                b.extend_from_slice(&payload[1..1 + ptr]);
            }
            self.drain(pid);
            self.buf.insert(pid, payload[1 + ptr..].to_vec());
        } else if let Some(b) = self.buf.get_mut(&pid) {
            b.extend_from_slice(payload);
        } else {
            return;
        }
        self.drain(pid);
    }

    fn drain(&mut self, pid: u16) {
        let Some(b) = self.buf.get_mut(&pid) else {
            return;
        };
        loop {
            if b.len() < 3 || b[0] == 0xff {
                if b.first() == Some(&0xff) {
                    b.clear();
                }
                return;
            }
            let len = (usize::from(b[1] & 0x0f) << 8) | usize::from(b[2]);
            if b.len() < 3 + len {
                return;
            }
            let sec: Vec<u8> = b.drain(..3 + len).collect();
            if sec.len() >= 8 {
                let key = (
                    pid,
                    sec[0],
                    (u16::from(sec[3]) << 8) | u16::from(sec[4]),
                    sec[6],
                );
                if self.seen.insert(key) {
                    self.sections.push((pid, sec));
                }
            }
        }
    }

    /// Abschnitte mit dieser Tabellenkennung.
    pub fn tables(&self, table_id: u8) -> impl Iterator<Item = &[u8]> {
        self.sections
            .iter()
            .filter(move |(_, s)| s[0] == table_id)
            .map(|(_, s)| s.as_slice())
    }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    (u16::from(b[i]) << 8) | u16::from(b[i + 1])
}

fn len12(b: &[u8], i: usize) -> usize {
    (usize::from(b[i] & 0x0f) << 8) | usize::from(b[i + 1])
}

/// PAT: Programmnummer → PMT-PID (ohne NIT-Eintrag 0).
pub fn parse_pat(sec: &[u8]) -> Vec<(u16, u16)> {
    let end = sec.len().saturating_sub(4);
    let mut out = Vec::new();
    let mut i = 8;
    while i + 4 <= end {
        let prog = u16_at(sec, i);
        let pid = u16_at(sec, i + 2) & 0x1fff;
        if prog != 0 {
            out.push((prog, pid));
        }
        i += 4;
    }
    out
}

/// Spur einer PMT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsInfo {
    pub stream_type: u8,
    pub pid: u16,
    /// Für die Wiedergabe nötig (Bild, Ton, Untertitel/Videotext).
    pub playable: bool,
}

/// PMT: Programmnummer, PCR-PID, Spuren.
pub fn parse_pmt(sec: &[u8]) -> Option<(u16, u16, Vec<EsInfo>)> {
    if sec.len() < 16 || sec[0] != 0x02 {
        return None;
    }
    let prog = u16_at(sec, 3);
    let pcr = u16_at(sec, 8) & 0x1fff;
    let info_len = len12(sec, 10);
    let end = sec.len() - 4;
    let mut i = 12 + info_len;
    let mut es = Vec::new();
    while i + 5 <= end {
        let st = sec[i];
        let pid = u16_at(sec, i + 1) & 0x1fff;
        let dl = len12(sec, i + 3);
        let descs = &sec[(i + 5).min(end)..(i + 5 + dl).min(end)];
        // Bild/Ton per Stream-Typ; private Daten (0x06) mit AC-3-, Untertitel- oder
        // Videotext-Beschreiber ebenfalls.
        let private_ok = descs_has(descs, &[0x6a, 0x7a, 0x59, 0x56]);
        let playable = matches!(
            st,
            0x01 | 0x02 | 0x03 | 0x04 | 0x0f | 0x11 | 0x1b | 0x24 | 0x81
        ) || (st == 0x06 && private_ok);
        es.push(EsInfo {
            stream_type: st,
            pid,
            playable,
        });
        i += 5 + dl;
    }
    Some((prog, pcr, es))
}

fn descs_has(d: &[u8], tags: &[u8]) -> bool {
    let mut i = 0;
    while i + 2 <= d.len() {
        if tags.contains(&d[i]) {
            return true;
        }
        i += 2 + d[i + 1] as usize;
    }
    false
}

/// Text nach DVB (EN 300 468 Anhang A): erstes Byte < 0x20 wählt die Zeichentabelle; ohne
/// Angabe die Standardtabelle (≈ ISO 6937). Hier: 0x15 = UTF-8, 0x05/0x10 0x00 0x09 =
/// ISO 8859-9 bzw. -x (als Latin-1 genähert), sonst Latin-1 mit den ISO-6937-Umlauten.
pub fn dvb_text(b: &[u8]) -> String {
    let (b, utf8) = match b.first() {
        Some(0x15) => (&b[1..], true),
        Some(0x10) if b.len() >= 3 => (&b[3..], false),
        Some(&c) if c < 0x20 => (&b[1..], false),
        _ => (b, false),
    };
    let s = if utf8 {
        String::from_utf8_lossy(b).to_string()
    } else {
        iso6937(b)
    };
    s.chars()
        .filter(|c| !matches!(*c, '\u{86}' | '\u{87}' | '\u{8a}') && !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

/// ISO 6937 grob: Akzent-Präfix 0xC1–0xCF kombiniert mit dem folgenden Buchstaben (nur die
/// in Deutschland üblichen Umlaute und Akzente), sonst Latin-1.
fn iso6937(b: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if (0xc1..=0xcf).contains(&c) && i + 1 < b.len() {
            let base = b[i + 1] as char;
            let comb = match (c, base) {
                (0xc8, 'a') => 'ä',
                (0xc8, 'o') => 'ö',
                (0xc8, 'u') => 'ü',
                (0xc8, 'A') => 'Ä',
                (0xc8, 'O') => 'Ö',
                (0xc8, 'U') => 'Ü',
                (0xc2, 'e') => 'é',
                (0xc1, 'e') => 'è',
                (0xc2, 'a') => 'á',
                (0xc1, 'a') => 'à',
                (0xcb, 'c') => 'ç',
                _ => base,
            };
            out.push(comb);
            i += 2;
            continue;
        }
        out.push(match c {
            0xfb => 'ß',
            _ => c as char,
        });
        i += 1;
    }
    out
}

/// Sendung aus der EIT (Programminformation im DVB-Strom).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EitEvent {
    /// Beginn und Ende (Sekunden seit 1970, UTC).
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub subtitle: Option<String>,
    pub desc: Option<String>,
}

/// Abschnitt einer EIT (Tabellen 0x4E–0x6F): Netz, Transponder, Dienst und seine Sendungen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EitSection {
    pub onid: u16,
    pub tsid: u16,
    pub service_id: u16,
    pub events: Vec<EitEvent>,
}

/// BCD-Byte als Zahl.
fn bcd8(b: u8) -> i64 {
    i64::from(b >> 4) * 10 + i64::from(b & 0x0f)
}

/// EIT lesen: „Jetzt/Danach“ (0x4E/0x4F) und Programm (0x50–0x6F); kurze Beschreibung (0x4D)
/// als Titel/Untertitel, erweiterte (0x4E) als Beschreibung.
pub fn parse_eit(sec: &[u8]) -> Option<EitSection> {
    if sec.len() < 18 || !(0x4e..=0x6f).contains(&sec[0]) {
        return None;
    }
    let total = 3 + len12(sec, 1);
    let end = total.min(sec.len()).checked_sub(4)?;
    let service_id = u16_at(sec, 3);
    let tsid = u16_at(sec, 8);
    let onid = u16_at(sec, 10);
    let mut events = Vec::new();
    let mut i = 14;
    while i + 12 <= end {
        let mjd = i64::from(u16_at(sec, i + 2));
        let start = (mjd - 40_587) * 86_400
            + bcd8(sec[i + 4]) * 3600
            + bcd8(sec[i + 5]) * 60
            + bcd8(sec[i + 6]);
        let duration = bcd8(sec[i + 7]) * 3600 + bcd8(sec[i + 8]) * 60 + bcd8(sec[i + 9]);
        let dlen = len12(sec, i + 10);
        let mut j = i + 12;
        let dend = (j + dlen).min(end);
        let (mut title, mut subtitle, mut ext) = (String::new(), None, String::new());
        while j + 2 <= dend {
            let tag = sec[j];
            let l = sec[j + 1] as usize;
            let d = &sec[(j + 2).min(dend)..(j + 2 + l).min(dend)];
            match tag {
                // Kurze Beschreibung: Sprache (3), Name, Text.
                0x4d if d.len() >= 4 => {
                    let nl = d[3] as usize;
                    if 4 + nl < d.len() {
                        title = dvb_text(&d[4..4 + nl]);
                        let tl = d[4 + nl] as usize;
                        let t = &d[(5 + nl).min(d.len())..(5 + nl + tl).min(d.len())];
                        let t = dvb_text(t);
                        if !t.is_empty() {
                            subtitle = Some(t);
                        }
                    } else if 4 + nl == d.len() {
                        title = dvb_text(&d[4..]);
                    }
                }
                // Erweiterte Beschreibung (mehrteilig): Nummer, Sprache, Einträge, Text.
                0x4e if d.len() >= 6 => {
                    let items = d[4] as usize;
                    let k = 5 + items;
                    if k < d.len() {
                        let tl = d[k] as usize;
                        let t = &d[k + 1..(k + 1 + tl).min(d.len())];
                        ext.push_str(&dvb_text_raw(t));
                    }
                }
                _ => {}
            }
            j += 2 + l;
        }
        if !title.is_empty() && duration > 0 {
            // Zeilenumbruch in DVB-Texten ist 0x8A; Hervorhebung 0x86/0x87 entfällt.
            let desc: String = ext
                .replace('\u{8a}', "\n")
                .chars()
                .filter(|c| !matches!(*c, '\u{86}' | '\u{87}') && (!c.is_control() || *c == '\n'))
                .collect::<String>()
                .trim()
                .to_string();
            events.push(EitEvent {
                start,
                stop: start + duration,
                title,
                subtitle,
                desc: (!desc.is_empty()).then_some(desc),
            });
        }
        i += 12 + dlen;
    }
    Some(EitSection {
        onid,
        tsid,
        service_id,
        events,
    })
}

/// Teil eines mehrteiligen Textes: dekodiert, aber ohne Kürzen (die Teile werden aneinander-
/// gehängt, Wortgrenzen liegen oft mitten im Teil).
fn dvb_text_raw(b: &[u8]) -> String {
    let (b, utf8) = match b.first() {
        Some(0x15) => (&b[1..], true),
        Some(0x10) if b.len() >= 3 => (&b[3..], false),
        Some(&c) if c < 0x20 => (&b[1..], false),
        _ => (b, false),
    };
    if utf8 {
        String::from_utf8_lossy(b).to_string()
    } else {
        iso6937(b)
    }
}

/// Eintrag der SDT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdtService {
    pub tsid: u16,
    pub onid: u16,
    pub service_id: u16,
    pub name: String,
    pub provider: String,
    /// 0x01 SD-TV, 0x16 SD (H.264), 0x19/0x1F HD, 0x02/0x0A Radio.
    pub service_type: u8,
    /// Verschlüsselt (free_CA_mode).
    pub scrambled: bool,
}

/// SDT (tatsächlicher Transponder, Tabelle 0x42).
pub fn parse_sdt(sec: &[u8]) -> Vec<SdtService> {
    if sec.len() < 15 || sec[0] != 0x42 {
        return Vec::new();
    }
    let tsid = u16_at(sec, 3);
    let onid = u16_at(sec, 8);
    let end = sec.len() - 4;
    let mut out = Vec::new();
    let mut i = 11;
    while i + 5 <= end {
        let sid = u16_at(sec, i);
        let scrambled = sec[i + 3] & 0x10 != 0;
        let dl = len12(sec, i + 3);
        let d = &sec[(i + 5).min(end)..(i + 5 + dl).min(end)];
        let mut j = 0;
        while j + 2 <= d.len() {
            let (tag, l) = (d[j], d[j + 1] as usize);
            let body = &d[(j + 2).min(d.len())..(j + 2 + l).min(d.len())];
            if tag == 0x48 && body.len() >= 2 {
                let pl = body[1] as usize;
                let provider = dvb_text(&body[2..(2 + pl).min(body.len())]);
                let rest = &body[(2 + pl).min(body.len())..];
                let name = rest
                    .first()
                    .map(|&nl| dvb_text(&rest[1..(1 + nl as usize).min(rest.len())]))
                    .unwrap_or_default();
                out.push(SdtService {
                    tsid,
                    onid,
                    service_id: sid,
                    name,
                    provider,
                    service_type: body[0],
                    scrambled,
                });
            }
            j += 2 + l;
        }
        i += 5 + dl;
    }
    out
}

/// Satelliten-Empfangsdaten aus der NIT (Beschreiber 0x43).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SatDelivery {
    pub tsid: u16,
    /// MHz.
    pub freq: u32,
    /// Orbitposition in Zehntelgrad (192 = 19,2°), Osten.
    pub orbital: u16,
    pub east: bool,
    pub pol: char,
    pub dvbs2: bool,
    /// „qpsk“, „8psk“, „16apsk“.
    pub mtype: &'static str,
    /// kSym/s.
    pub sr: u32,
    /// „12“, „23“, „34“, „56“, „78“, „89“, „35“, „45“, „910“.
    pub fec: &'static str,
}

fn bcd(b: &[u8]) -> u32 {
    b.iter().fold(0, |acc, &x| {
        acc * 100 + u32::from(x >> 4) * 10 + u32::from(x & 0x0f)
    })
}

/// NIT (0x40 eigenes, 0x41 anderes Netz): Empfangsdaten aller Transponder.
pub fn parse_nit(sec: &[u8]) -> Vec<SatDelivery> {
    if sec.len() < 16 || !matches!(sec[0], 0x40 | 0x41) {
        return Vec::new();
    }
    let end = sec.len() - 4;
    let mut i = 10 + len12(sec, 8);
    if i + 2 > end {
        return Vec::new();
    }
    let loop_end = (i + 2 + len12(sec, i)).min(end);
    i += 2;
    let mut out = Vec::new();
    while i + 6 <= loop_end {
        let tsid = u16_at(sec, i);
        let dl = len12(sec, i + 4);
        let d = &sec[i + 6..(i + 6 + dl).min(loop_end)];
        let mut j = 0;
        while j + 2 <= d.len() {
            let (tag, l) = (d[j], d[j + 1] as usize);
            let b = &d[(j + 2).min(d.len())..(j + 2 + l).min(d.len())];
            if tag == 0x43 && b.len() >= 11 {
                let flags = b[6];
                out.push(SatDelivery {
                    tsid,
                    freq: bcd(&b[0..4]) / 100,
                    orbital: bcd(&b[4..6]) as u16,
                    east: flags & 0x80 != 0,
                    pol: ['h', 'v', 'l', 'r'][((flags >> 5) & 3) as usize],
                    dvbs2: flags & 0x04 != 0,
                    mtype: match flags & 3 {
                        2 => "8psk",
                        3 => "16apsk",
                        _ => "qpsk",
                    },
                    sr: bcd(&b[7..11]) / 100,
                    fec: match b[10] & 0x0f {
                        1 => "12",
                        2 => "23",
                        3 => "34",
                        4 => "56",
                        5 => "78",
                        6 => "89",
                        7 => "35",
                        8 => "45",
                        9 => "910",
                        _ => "auto",
                    },
                });
            }
            j += 2 + l;
        }
        i += 6 + dl;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_und_bcd() {
        assert_eq!(dvb_text(b"\x05Das Erste HD"), "Das Erste HD");
        assert_eq!(dvb_text(b"M\xc8unchen"), "München");
        assert_eq!(dvb_text("\u{15}Zürich".as_bytes()), "Zürich");
        assert_eq!(bcd(&[0x01, 0x14, 0x94, 0x00]), 1_149_400);
    }

    #[test]
    fn pat_und_sdt() {
        // PAT: Programm 28106 → PMT 5100 (CRC am Ende nur Platzhalter).
        let pat = [
            0x00, 0xb0, 0x11, 0x04, 0x51, 0xc1, 0x00, 0x00, 0x00, 0x00, 0xe0, 0x10, 0x6d, 0xca,
            0xf3, 0xec, 0, 0, 0, 0,
        ];
        assert_eq!(parse_pat(&pat), [(0x6dca, 5100)]);
        // SDT mit einem Dienst „Das Erste HD“ (Anbieter „ARD“, HD, frei).
        let name = b"Das Erste HD";
        let prov = b"ARD";
        let mut d = vec![
            0x48,
            (3 + prov.len() + name.len()) as u8,
            0x19,
            prov.len() as u8,
        ];
        d.extend_from_slice(prov);
        d.push(name.len() as u8);
        d.extend_from_slice(name);
        let mut s = vec![
            0x42, 0, 0, 0x04, 0x51, 0xc1, 0, 0, 0x00, 0x01, 0xff, 0x6d, 0xca, 0xfc,
        ];
        s.push(0x80 | ((d.len() >> 8) as u8 & 0x0f));
        s.push(d.len() as u8);
        s.extend_from_slice(&d);
        s.extend_from_slice(&[0, 0, 0, 0]);
        let len = s.len() - 3;
        s[1] = 0xf0 | ((len >> 8) as u8);
        s[2] = len as u8;
        let svc = parse_sdt(&s);
        assert_eq!(svc.len(), 1);
        assert_eq!(
            (svc[0].name.as_str(), svc[0].provider.as_str()),
            ("Das Erste HD", "ARD")
        );
        assert_eq!(
            (svc[0].service_id, svc[0].service_type, svc[0].scrambled),
            (0x6dca, 0x19, false)
        );
    }
}
