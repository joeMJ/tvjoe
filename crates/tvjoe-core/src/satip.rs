//! SAT>IP (Netstream 4Sat u. a.): RTSP-Sitzungen zum Abstimmen, Tunerstatus aus der
//! Beschreibung (SDP) und Prüfen, ob der Datenstrom (RTP über UDP) ankommt.
//!
//! Die Netstream liefert nur RTP über UDP an die Absenderadresse (kein RTP über TCP) und
//! meldet sich nicht per SSDP; die Adresse wird eingetragen.

use std::io::{Read, Write};
use std::net::{TcpStream, UdpSocket};
use std::time::{Duration, Instant};

const PORT: u16 = 554;
const TIMEOUT: Duration = Duration::from_secs(5);

/// Ein Transponder auf Astra 19,2° Ost (Empfangsdaten für die SAT>IP-Adresse).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transponder {
    pub freq: u32,
    /// 'h' oder 'v'.
    pub pol: char,
    pub dvbs2: bool,
    pub sr: u32,
    /// Coderate ohne Schrägstrich („23“ = 2/3, „34“ = 3/4).
    pub fec: &'static str,
}

impl Transponder {
    /// Teil der SAT>IP-Adresse ab `src`.
    pub fn query(&self) -> String {
        let (msys, mtype) = if self.dvbs2 {
            ("dvbs2", "8psk")
        } else {
            ("dvbs", "qpsk")
        };
        format!(
            "src=1&freq={}&pol={}&ro=0.35&msys={msys}&mtype={mtype}&plts=on&sr={}&fec={}",
            self.freq, self.pol, self.sr, self.fec
        )
    }
}

/// Für „Prüfen“: vier verschiedene, sicher belegte Transponder (ARD/ZDF), damit jeder Tuner
/// einzeln abgestimmt wird.
pub const CHECK_TRANSPONDERS: [Transponder; 4] = [
    Transponder {
        freq: 11494,
        pol: 'h',
        dvbs2: true,
        sr: 22000,
        fec: "23",
    },
    Transponder {
        freq: 11362,
        pol: 'h',
        dvbs2: true,
        sr: 22000,
        fec: "23",
    },
    Transponder {
        freq: 11053,
        pol: 'h',
        dvbs2: true,
        sr: 22000,
        fec: "23",
    },
    Transponder {
        freq: 11836,
        pol: 'h',
        dvbs2: false,
        sr: 27500,
        fec: "34",
    },
];

/// Status eines Tuners laut SAT>IP-Beschreibung (`tuner=Nr,Pegel,Lock,Qualität,…`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TunerStatus {
    pub tuner: u32,
    /// 0–255.
    pub level: u32,
    pub lock: bool,
    /// 0–15.
    pub quality: u32,
    pub freq: u32,
}

impl TunerStatus {
    pub fn level_percent(&self) -> u32 {
        self.level * 100 / 255
    }
}

/// Ergebnis von „Prüfen“.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    /// Titel der Weboberfläche (z. B. „Elgato EyeTV Netstream“), falls vorhanden.
    pub name: Option<String>,
    /// Firmware laut Weboberfläche, falls erkennbar.
    pub firmware: Option<String>,
    /// Anzahl der Tuner laut Beschreibung (`SatIPServer:1 N`).
    pub tuners: u32,
    pub status: Vec<TunerStatus>,
    /// Pakete des Datenstroms in der Messzeit (0 = nichts angekommen, z. B. Firewall).
    pub packets: usize,
}

/// Auswertung einer SDP-Beschreibung: Tunerzahl und Status (falls ein Stream aktiv ist).
pub fn parse_sdp(sdp: &str) -> (Option<u32>, Option<TunerStatus>) {
    let tuners = sdp.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("s=SatIPServer:1")?;
        rest.split([' ', ',']).find_map(|x| x.trim().parse().ok())
    });
    let status = sdp.lines().find_map(|l| {
        let v = l.split("tuner=").nth(1)?;
        let f: Vec<&str> = v.split(',').collect();
        Some(TunerStatus {
            tuner: f.first()?.trim().parse().ok()?,
            level: f.get(1)?.trim().parse().ok()?,
            lock: f.get(2)?.trim() == "1",
            quality: f.get(3)?.trim().parse().ok()?,
            freq: f
                .get(4)
                .and_then(|x| x.trim().parse::<f64>().ok())
                .map(|x| x as u32)
                .unwrap_or(0),
        })
    });
    (tuners, status)
}

/// Antwort einer RTSP-Anfrage.
#[derive(Debug, Clone, Default)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Eine RTSP-Anfrage (eigene Verbindung je Anfrage, wie es die Netstream verträgt).
pub fn request(
    host: &str,
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
) -> Result<Response, String> {
    let mut s = TcpStream::connect((host, PORT)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(TIMEOUT)).ok();
    s.set_write_timeout(Some(TIMEOUT)).ok();
    let mut req = format!("{method} {url} RTSP/1.0\r\nCSeq: 1\r\nUser-Agent: tvjoe\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        data.extend_from_slice(&buf[..n]);
        if let Some(end) = find(&data, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..end]).to_string();
            let len = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse().ok())?
                })
                .unwrap_or(0usize);
            if data.len() >= end + 4 + len {
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&data).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| "keine RTSP-Antwort".to_string())?;
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Ok(Response {
        status,
        headers,
        body: body.to_string(),
    })
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// Eine abgestimmte Sitzung (SETUP); `teardown` gibt den Tuner frei.
#[derive(Debug, Clone)]
pub struct Session {
    pub host: String,
    pub id: String,
    pub stream: String,
}

impl Session {
    /// Abstimmen auf `query` (ab `src=`), Daten an `client_port` dieses Rechners.
    pub fn setup(host: &str, query: &str, client_port: u16) -> Result<Self, String> {
        let r = request(
            host,
            "SETUP",
            &format!("rtsp://{host}:{PORT}/?{query}"),
            &[(
                "Transport",
                &format!(
                    "RTP/AVP;unicast;client_port={client_port}-{}",
                    client_port + 1
                ),
            )],
        )?;
        if r.status != 200 {
            return Err(format!("SETUP: RTSP {}", r.status));
        }
        let id = r
            .header("Session")
            .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
            .ok_or("keine Sitzung")?;
        let stream = r
            .header("com.ses.streamID")
            .ok_or("keine Stream-Nummer")?
            .to_string();
        Ok(Self {
            host: host.to_string(),
            id,
            stream,
        })
    }

    fn url(&self) -> String {
        format!("rtsp://{}:{PORT}/stream={}", self.host, self.stream)
    }

    pub fn play(&self) -> Result<(), String> {
        let r = request(&self.host, "PLAY", &self.url(), &[("Session", &self.id)])?;
        (r.status == 200)
            .then_some(())
            .ok_or_else(|| format!("PLAY: RTSP {}", r.status))
    }

    /// Beschreibung dieser Sitzung: Tunerzahl und Status.
    pub fn describe(&self) -> Result<(Option<u32>, Option<TunerStatus>), String> {
        let r = request(
            &self.host,
            "DESCRIBE",
            &self.url(),
            &[("Session", &self.id), ("Accept", "application/sdp")],
        )?;
        Ok(parse_sdp(&r.body))
    }

    pub fn teardown(&self) {
        let _ = request(
            &self.host,
            "TEARDOWN",
            &self.url(),
            &[("Session", &self.id)],
        );
    }
}

/// Name und Firmware aus der Weboberfläche (Netstream: `<title>` und „Version: …“).
fn web_info(host: &str) -> (Option<String>, Option<String>) {
    let agent = crate::net::agent(Duration::from_secs(4));
    let Ok(html) = crate::net::fetch_prefix(&agent, &format!("http://{host}/"), 256 * 1024) else {
        return (None, None);
    };
    let html = String::from_utf8_lossy(&html);
    let title = html
        .split("<title>")
        .nth(1)
        .and_then(|t| t.split("</title>").next())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let firmware = html.split("Version:").nth(1).map(|v| {
        v.trim_start()
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '<')
            .collect::<String>()
    });
    (title, firmware.filter(|f| !f.is_empty()))
}

/// Prüfen: Weboberfläche lesen, je Tuner einen anderen Transponder abstimmen, Status lesen,
/// beim ersten den Datenstrom `listen` lang zählen, alles wieder freigeben.
pub fn check(host: &str, listen: Duration) -> Result<CheckResult, String> {
    let (name, firmware) = web_info(host);
    let opt = request(host, "OPTIONS", &format!("rtsp://{host}:{PORT}/"), &[])?;
    if opt.status != 200 {
        return Err(format!("RTSP antwortet mit {}", opt.status));
    }
    // Empfangsport: freien UDP-Port vom System nehmen (gerade Nummer für RTP).
    let sock = bind_rtp()?;
    let port = sock.local_addr().map_err(|e| e.to_string())?.port();
    let first = Session::setup(
        host,
        &format!("{}&pids=0", CHECK_TRANSPONDERS[0].query()),
        port,
    )?;
    let mut sessions = vec![first];
    let (tuners, _) = sessions[0].describe().unwrap_or((None, None));
    let tuners = tuners.unwrap_or(1);
    for tp in CHECK_TRANSPONDERS.iter().take(tuners as usize).skip(1) {
        // Weitere Sitzungen ohne Datenempfang (Port 0 wäre ungültig; ein zweiter freier Port).
        let extra = bind_rtp()?;
        let p = extra.local_addr().map_err(|e| e.to_string())?.port();
        match Session::setup(host, &format!("{}&pids=0", tp.query()), p) {
            Ok(s) => sessions.push(s),
            Err(_) => break,
        }
    }
    std::thread::sleep(Duration::from_millis(1500));
    let status: Vec<TunerStatus> = sessions
        .iter()
        .filter_map(|s| s.describe().ok().and_then(|(_, st)| st))
        .collect();
    let packets = sessions[0]
        .play()
        .map(|()| count_packets(&sock, listen))
        .unwrap_or(0);
    for s in &sessions {
        s.teardown();
    }
    Ok(CheckResult {
        name,
        firmware,
        tuners,
        status,
        packets,
    })
}

fn bind_rtp() -> Result<UdpSocket, String> {
    // Gerade Portnummer suchen (RTP), RTCP nimmt die nächste.
    for _ in 0..20 {
        let s = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
        if s.local_addr().map_err(|e| e.to_string())?.port() % 2 == 0 {
            return Ok(s);
        }
    }
    Err("kein freier UDP-Port".into())
}

fn count_packets(sock: &UdpSocket, listen: Duration) -> usize {
    sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let mut buf = [0u8; 2048];
    let mut n = 0;
    let t = Instant::now();
    while t.elapsed() < listen {
        if sock.recv(&mut buf).is_ok() {
            n += 1;
        }
    }
    n
}

/// Ist unter Ubuntu die Firewall ufw eingeschaltet?
pub fn ufw_active() -> bool {
    std::fs::read_to_string("/etc/ufw/ufw.conf")
        .is_ok_and(|c| c.lines().any(|l| l.trim() == "ENABLED=yes"))
}

/// Befehl für die Freigabe: nur UDP vom SAT>IP-Gerät (über pkexec, mit Passwortabfrage).
pub fn ufw_rule_command(host: &str) -> Vec<String> {
    vec![
        "pkexec".into(),
        "ufw".into(),
        "allow".into(),
        "from".into(),
        host.into(),
        "proto".into(),
        "udp".into(),
        "comment".into(),
        "tvjoe SAT>IP".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beschreibung_auswerten() {
        let sdp = "v=0\r\ns=SatIPServer:1 4\r\nm=video 0 RTP/AVP 33\r\n\
            a=fmtp:33 ver=1.0;src=1;tuner=2,255,1,15,11362.00,h,dvbs2,8psk,on,0.35,22000,23\r\n";
        let (n, st) = parse_sdp(sdp);
        assert_eq!(n, Some(4));
        let st = st.unwrap();
        assert_eq!(
            (st.tuner, st.level, st.lock, st.quality, st.freq),
            (2, 255, true, 15, 11362)
        );
        assert_eq!(st.level_percent(), 100);
        let (_, none) =
            parse_sdp("s=SatIPServer:1 4\r\na=fmtp:33 ver=1.0;src=1;tuner=4,0,0,0,11836.00,h");
        assert!(!none.unwrap().lock);
    }

    #[test]
    fn adresse_eines_transponders() {
        assert_eq!(
            CHECK_TRANSPONDERS[0].query(),
            "src=1&freq=11494&pol=h&ro=0.35&msys=dvbs2&mtype=8psk&plts=on&sr=22000&fec=23"
        );
        assert!(
            CHECK_TRANSPONDERS[3]
                .query()
                .contains("msys=dvbs&mtype=qpsk")
        );
    }

    /// Echtes Gerät: `TVJOE_TEST_SATIP=<Adresse> cargo test -p tvjoe-core -- --ignored
    /// --nocapture echtes_geraet` (belegt kurz die Tuner).
    #[test]
    #[ignore]
    fn echtes_geraet() {
        let Ok(host) = std::env::var("TVJOE_TEST_SATIP") else {
            return;
        };
        let r = check(&host, Duration::from_secs(3)).unwrap();
        println!("{r:#?}");
    }
}
