//! SAT>IP (Netstream 4Sat u. a.): RTSP-Sitzungen zum Abstimmen, Tunerstatus aus der
//! Beschreibung (SDP) und Prüfen, ob der Datenstrom (RTP über UDP) ankommt.
//!
//! Die Netstream liefert nur RTP über UDP an die Absenderadresse (kein RTP über TCP) und
//! meldet sich nicht per SSDP; die Adresse wird eingetragen.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use crate::{tf, tr};

const PORT: u16 = 554;
const TIMEOUT: Duration = Duration::from_secs(5);

/// Ein Transponder auf Astra 19,2° Ost (Empfangsdaten für die SAT>IP-Adresse).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transponder {
    pub freq: u32,
    /// 'h' oder 'v'.
    pub pol: char,
    pub dvbs2: bool,
    /// „qpsk“, „8psk“, „16apsk“.
    pub mtype: &'static str,
    pub sr: u32,
    /// Coderate ohne Schrägstrich („23“ = 2/3, „34“ = 3/4).
    pub fec: &'static str,
}

impl Transponder {
    /// Teil der SAT>IP-Adresse ab `src`.
    pub fn query(&self) -> String {
        let msys = if self.dvbs2 { "dvbs2" } else { "dvbs" };
        format!(
            "src=1&freq={}&pol={}&ro=0.35&msys={msys}&mtype={}&plts=on&sr={}&fec={}",
            self.freq, self.pol, self.mtype, self.sr, self.fec
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
        mtype: "8psk",
        sr: 22000,
        fec: "23",
    },
    Transponder {
        freq: 11362,
        pol: 'h',
        dvbs2: true,
        mtype: "8psk",
        sr: 22000,
        fec: "23",
    },
    Transponder {
        freq: 11053,
        pol: 'h',
        dvbs2: true,
        mtype: "8psk",
        sr: 22000,
        fec: "23",
    },
    Transponder {
        freq: 11836,
        pol: 'h',
        dvbs2: false,
        mtype: "qpsk",
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
    let addr = std::net::ToSocketAddrs::to_socket_addrs(&(host, PORT))
        .map_err(|e| e.to_string())?
        .next()
        .ok_or(tr("Adresse nicht auflösbar"))?;
    let t0 = std::time::Instant::now();
    crate::diag::log("rtsp", &format!("→ {method} {url} {headers:?}"));
    let mut s = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| {
        crate::diag::log("rtsp", &format!("✖ Verbindung zu {addr}: {e}"));
        e.to_string()
    })?;
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
    crate::diag::log(
        "rtsp",
        &format!(
            "← {method} nach {} ms, {} Bytes: {}",
            t0.elapsed().as_millis(),
            data.len(),
            text.replace("\r\n", " | ")
        ),
    );
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| tr("keine RTSP-Antwort").to_string())?;
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
            .ok_or(tr("keine Sitzung"))?;
        let stream = r
            .header("com.ses.streamID")
            .ok_or(tr("keine Stream-Nummer"))?
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
        return Err(tf!("RTSP antwortet mit {status}", status = opt.status));
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
    Err(tr("kein freier UDP-Port").into())
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

// ---------------------------------------------------------------------------------------------
// Wiedergabe

/// Gerät und Abstimmteil (ab `src=`) einer gespeicherten SAT>IP-Adresse
/// (`rtsp://gerät:554/?src=1&freq=…`), sonst `None`.
pub fn split_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("rtsp://")?;
    let (hostport, query) = rest.split_once("/?")?;
    let host = hostport.split(':').next()?;
    (query.contains("src=") && query.contains("freq="))
        .then(|| (host.to_string(), query.to_string()))
}

/// Angeforderte PIDs einer SAT>IP-Adresse (`None` bei `pids=all` oder ohne Angabe).
pub fn requested_pids(query: &str) -> Option<Vec<u16>> {
    let list = query.split('&').find_map(|p| p.strip_prefix("pids="))?;
    if list == "all" {
        return None;
    }
    let pids: Vec<u16> = list.split(',').filter_map(|p| p.parse().ok()).collect();
    (!pids.is_empty()).then_some(pids)
}

/// SAT>IP-Adresse mit anderer PID-Liste.
pub fn with_pids(url: &str, pids: &[u16]) -> String {
    let list: Vec<String> = pids.iter().map(u16::to_string).collect();
    let list = list.join(",");
    match url.split_once("pids=") {
        Some((a, rest)) => match rest.split_once('&') {
            Some((_, tail)) => format!("{a}pids={list}&{tail}"),
            None => format!("{a}pids={list}"),
        },
        None => format!("{url}&pids={list}"),
    }
}

/// PIDs zum Abspielen eines Dienstes: PAT, PMT, PCR und alle abspielbaren Spuren.
pub(crate) fn service_pids(pmt: u16, pcr: u16, es: &[crate::ts::EsInfo]) -> Vec<u16> {
    let mut pids = vec![0, pmt];
    if pcr != 0x1fff {
        pids.push(pcr);
    }
    pids.extend(es.iter().filter(|e| e.playable).map(|e| e.pid));
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// Neue PID-Liste, wenn die PMT des angeforderten Dienstes auf Spuren zeigt, die nicht
/// angefordert sind (Regionalfenster wie die WDR-Lokalzeit wechseln die Spuren je Sendung).
/// Der Dienst ist der einzige Eintrag der PAT, dessen PMT-PID in der Liste steht.
pub fn changed_pids(
    pat: &[(u16, u16)],
    pmts: &[(u16, u16, Vec<crate::ts::EsInfo>)],
    requested: &[u16],
) -> Option<Vec<u16>> {
    let mut own = pat.iter().filter(|(prog, pid)| *prog != 0 && requested.contains(pid));
    let (prog, pmt) = *own.next()?;
    if own.next().is_some() {
        return None;
    }
    let (_, pcr, es) = pmts.iter().find(|(p, _, _)| *p == prog)?;
    let wanted = service_pids(pmt, *pcr, es);
    wanted.iter().any(|p| !requested.contains(p)).then_some(wanted)
}

/// Nutzdaten (Transportstrom) eines RTP-Pakets.
fn rtp_payload(p: &[u8]) -> Option<&[u8]> {
    if p.len() < 12 || p[0] >> 6 != 2 {
        return None;
    }
    let mut start = 12 + 4 * (p[0] & 0x0f) as usize;
    if p[0] & 0x10 != 0 {
        let ext = p.get(start + 2..start + 4)?;
        start += 4 + 4 * u16::from_be_bytes([ext[0], ext[1]]) as usize;
    }
    p.get(start..)
}

/// Laufende Wiedergabe eines SAT-Senders: tvjoe stimmt den Tuner selbst ab (SETUP/PLAY,
/// libmpv kann SAT>IP nicht), empfängt RTP über UDP und gibt den Transportstrom unter
/// `local_url` (nur 127.0.0.1) an den Player weiter. Status alle 5 s (hält zugleich die
/// Sitzung offen). Beim Freigeben (Drop, ohne zu warten) oder wenn der Player trennt:
/// TEARDOWN, der Tuner ist wieder frei. Beim Beenden des Programms [`Relay::close`].
pub struct Relay {
    pub local_url: String,
    status: std::sync::Arc<std::sync::Mutex<Option<TunerStatus>>>,
    /// Empfangene RTP-Pakete (0 nach einigen Sekunden: Firewall oder Tunnel).
    packets: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Programmdaten (EIT, PID 18), die beim Ansehen mitgelesen wurden; abholen mit
    /// [`Relay::take_eit`].
    eit: std::sync::Arc<std::sync::Mutex<Vec<crate::ts::EitSection>>>,
    /// Adresse mit den Spuren laut aktueller PMT, wenn sie von den angeforderten abweichen;
    /// abholen mit [`Relay::retune`].
    retune: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Relay {
    pub fn start(url: &str) -> Result<Self, String> {
        let requested = requested_pids(url);
        let original = url.to_string();
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};
        let (host, query) = split_url(url).ok_or(tr("keine SAT>IP-Adresse"))?;
        // PID 18 (EIT) mitnehmen: „Jetzt/Danach“ beim Ansehen; der Player übergeht sie.
        let query = match query.split_once("pids=") {
            Some((a, rest)) => {
                let (list, tail) = rest.split_once('&').map_or((rest, ""), |(l, t)| (l, t));
                if list.split(',').any(|p| p == "18") || list == "all" {
                    query.clone()
                } else if tail.is_empty() {
                    format!("{a}pids={list},18")
                } else {
                    format!("{a}pids={list},18&{tail}")
                }
            }
            None => query,
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let status = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let packets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let eit: Arc<Mutex<Vec<crate::ts::EitSection>>> = Arc::default();
        let retune: Arc<Mutex<Option<String>>> = Arc::default();
        let (st, sp, pk, ei, rt) =
            (status.clone(), stop.clone(), packets.clone(), eit.clone(), retune.clone());
        let thread = std::thread::Builder::new().name("satip-weitergabe".into()).spawn(move || {
            let stopped = || sp.load(Ordering::Relaxed);
            let Ok(sock) = bind_rtp() else { return };
            let Ok(port) = sock.local_addr().map(|a| a.port()) else {
                return;
            };
            let session = match Session::setup(&host, &query, port) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("tvjoe: SAT>IP {e}");
                    return;
                }
            };
            if let Err(e) = session.play() {
                eprintln!("tvjoe: SAT>IP {e}");
                session.teardown();
                return;
            }
            sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
            let mut next_status = Instant::now() + Duration::from_secs(2);
            let mut next_eit = Instant::now() + Duration::from_secs(20);
            let mut col = crate::ts::SectionCollector::default();
            // PAT und PMT des Senders, alle 2 s frisch (der Sammler übergeht neue Versionen).
            let mut tables = crate::ts::SectionCollector::default();
            let mut next_tables = Instant::now() + Duration::from_secs(2);
            let watched = |pid: u16| pid == 0 || requested.as_ref().is_some_and(|r| r.contains(&pid));
            let mut buf = [0u8; 2048];
            // Erste Verbindung des Players abwarten; trennt er (z. B. erneutes Öffnen beim
            // Erkennen des Formats), kurz auf eine neue warten.
            let mut wait = Duration::from_secs(15);
            crate::diag::log("satip-weitergabe", &format!("bereit, RTP-Port {port}, Sitzung {}", session.id));
            let mut sent: u64 = 0;
            'clients: while !stopped() {
                let t = Instant::now();
                let mut client = loop {
                    if stopped() || t.elapsed() > wait {
                        crate::diag::log(
                            "satip-weitergabe",
                            if stopped() { "Ende (gestoppt) beim Warten auf den Player" } else { "Ende: kein Player verbunden" },
                        );
                        break 'clients;
                    }
                    match listener.accept() {
                        Ok((c, _)) => break c,
                        Err(_) => {
                            // Daten verwerfen, solange niemand liest.
                            if sock.recv(&mut buf).is_ok() {
                                pk.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                };
                wait = Duration::from_secs(3);
                crate::diag::log("satip-weitergabe", &format!("Player verbunden: {:?}", client.peer_addr()));
                client.set_nonblocking(false).ok();
                client.set_read_timeout(Some(Duration::from_secs(2))).ok();
                client.set_write_timeout(Some(Duration::from_secs(5))).ok();
                let mut req = Vec::new();
                let mut b = [0u8; 1024];
                while find(&req, b"\r\n\r\n").is_none() && req.len() < 8192 {
                    match client.read(&mut b) {
                        Ok(n) if n > 0 => req.extend_from_slice(&b[..n]),
                        _ => continue 'clients,
                    }
                }
                let head =
                    "HTTP/1.0 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n";
                if client.write_all(head.as_bytes()).is_err() {
                    continue;
                }
                while !stopped() {
                    if let Ok(n) = sock.recv(&mut buf) {
                        pk.fetch_add(1, Ordering::Relaxed);
                        if let Some(ts) = rtp_payload(&buf[..n]) {
                            for p in ts.chunks(188) {
                                col.push_packet(p, &|pid| pid == 18);
                                tables.push_packet(p, &watched);
                            }
                            if let Err(e) = client.write_all(ts) {
                                crate::diag::log(
                                    "satip-weitergabe",
                                    &format!("Player getrennt nach {sent} Bytes: {e}"),
                                );
                                continue 'clients;
                            }
                            sent += ts.len() as u64;
                        }
                    }
                    if Instant::now() >= next_status {
                        next_status = Instant::now() + Duration::from_secs(5);
                        if let Ok((_, s)) = session.describe() {
                            *st.lock().unwrap_or_else(|e| e.into_inner()) = s;
                        }
                    }
                    if Instant::now() >= next_tables {
                        next_tables = Instant::now() + Duration::from_secs(2);
                        let done = std::mem::take(&mut tables);
                        if let Some(req) = &requested {
                            let pat: Vec<(u16, u16)> =
                                done.tables(0x00).flat_map(crate::ts::parse_pat).collect();
                            let pmts: Vec<_> =
                                done.tables(0x02).filter_map(crate::ts::parse_pmt).collect();
                            if let Some(pids) = changed_pids(&pat, &pmts, req) {
                                let new = with_pids(&original, &pids);
                                crate::diag::log(
                                    "satip-weitergabe",
                                    &format!("Spuren laut PMT geändert: {pids:?}"),
                                );
                                *rt.lock().unwrap_or_else(|e| e.into_inner()) = Some(new);
                            }
                        }
                    }
                    if Instant::now() >= next_eit {
                        next_eit = Instant::now() + Duration::from_secs(20);
                        let done = std::mem::take(&mut col);
                        let sections: Vec<_> = done
                            .sections
                            .iter()
                            .filter_map(|(_, sec)| crate::ts::parse_eit(sec))
                            .collect();
                        if !sections.is_empty() {
                            ei.lock().unwrap_or_else(|e| e.into_inner()).extend(sections);
                        }
                    }
                }
            }
            crate::diag::log(
                "satip-weitergabe",
                &format!("Schleife beendet (gestoppt: {}), {sent} Bytes weitergegeben; TEARDOWN", stopped()),
            );
            session.teardown();
            crate::diag::log("satip-weitergabe", "TEARDOWN fertig, Thread endet");
        });
        Ok(Self {
            local_url: format!("http://127.0.0.1:{port}/satip.ts"),
            status,
            packets,
            eit,
            retune,
            stop,
            thread: thread.ok(),
        })
    }

    /// Adresse mit den Spuren laut aktueller PMT, falls sie sich geändert haben (einmalig):
    /// der Sender ist damit neu zu starten.
    pub fn retune(&self) -> Option<String> {
        self.retune.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Seit dem letzten Abholen mitgelesene Programmdaten.
    pub fn take_eit(&self) -> Vec<crate::ts::EitSection> {
        std::mem::take(&mut *self.eit.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Bisher vom Gerät empfangene RTP-Pakete.
    pub fn packets(&self) -> usize {
        self.packets.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Letzter gemeldeter Empfang des Tuners (nach etwa 2 s verfügbar).
    pub fn status(&self) -> Option<TunerStatus> {
        *self.status.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Relay {
    /// Beenden und auf die Freigabe des Tuners warten.
    pub fn close(mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        crate::diag::log("satip-weitergabe", &format!("Stopp angefordert ({})", self.local_url));
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Aufnahme eines SAT-Senders in eine Datei: eigene Sitzung am Gerät (teilt sich bei gleichem
/// Transponder den Tuner mit der Wiedergabe), Empfang in einem Thread, Schreiben in einem
/// zweiten über eine Warteschlange im Arbeitsspeicher. Kommt die Platte kurz nicht nach, füllt
/// sich die Warteschlange bis `queue_bytes`; erst darüber gehen Daten verloren (gezählt).
/// Geschrieben wird nach `<datei>.part`, beim Beenden umbenannt.
pub struct Recorder {
    pub path: std::path::PathBuf,
    bytes: std::sync::Arc<std::sync::atomic::AtomicU64>,
    dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,
    error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// Größe der Stücke in der Warteschlange.
const CHUNK: usize = 64 * 1024;

impl Recorder {
    pub fn start(url: &str, path: &std::path::Path, queue_bytes: u64) -> Result<Self, String> {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::{Arc, Mutex, mpsc};
        let (host, query) = split_url(url).ok_or(tr("keine SAT>IP-Adresse"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let part = crate::ablage::part_path(path);
        let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let bytes = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let slots = (queue_bytes as usize / CHUNK).max(4);
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(slots);
        let set_error = {
            let error = error.clone();
            move |e: String| {
                let mut g = error.lock().unwrap_or_else(|e| e.into_inner());
                g.get_or_insert(e);
            }
        };

        let (b, err, sp) = (bytes.clone(), set_error.clone(), stop.clone());
        let writer = std::thread::spawn(move || {
            for chunk in rx {
                if let Err(e) = file.write_all(&chunk) {
                    err(tf!("Schreiben fehlgeschlagen: {error}", error = e));
                    sp.store(true, Ordering::Relaxed);
                    break;
                }
                b.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
            let _ = file.sync_all();
        });

        let (dr, err, sp) = (dropped.clone(), set_error, stop.clone());
        let receiver = std::thread::spawn(move || {
            let stopped = || sp.load(Ordering::Relaxed);
            let sock = match bind_rtp() {
                Ok(s) => s,
                Err(e) => return err(e),
            };
            let Ok(port) = sock.local_addr().map(|a| a.port()) else {
                return err(tr("kein UDP-Anschluss").into());
            };
            let session = match Session::setup(&host, &query, port) {
                Ok(s) => s,
                Err(e) => return err(e),
            };
            if let Err(e) = session.play() {
                session.teardown();
                return err(e);
            }
            sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
            let mut next_status = Instant::now() + Duration::from_secs(5);
            let mut last_data = Instant::now();
            let mut buf = [0u8; 2048];
            let mut chunk = Vec::with_capacity(CHUNK);
            while !stopped() {
                if let Ok(n) = sock.recv(&mut buf) {
                    if let Some(ts) = rtp_payload(&buf[..n]) {
                        last_data = Instant::now();
                        chunk.extend_from_slice(ts);
                        if chunk.len() >= CHUNK {
                            let full = std::mem::replace(&mut chunk, Vec::with_capacity(CHUNK));
                            if let Err(mpsc::TrySendError::Full(lost)) = tx.try_send(full) {
                                dr.fetch_add(lost.len() as u64, Ordering::Relaxed);
                            }
                        }
                    }
                } else if last_data.elapsed() > Duration::from_secs(15) {
                    err(tr("kein Datenstrom vom SAT>IP-Gerät").into());
                    break;
                }
                if Instant::now() >= next_status {
                    next_status = Instant::now() + Duration::from_secs(5);
                    let _ = session.describe();
                }
            }
            if !chunk.is_empty() {
                let _ = tx.send(chunk);
            }
            session.teardown();
        });
        Ok(Self {
            path: path.to_path_buf(),
            bytes,
            dropped,
            error,
            stop,
            threads: vec![receiver, writer],
        })
    }

    /// Geschriebene Bytes.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Verlorene Bytes (Warteschlange voll).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Erster Fehler (Gerät, Netz, Platte); die Aufnahme läuft dann nicht mehr.
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Beenden, Warteschlange leeren, Tuner freigeben, Datei umbenennen.
    pub fn finish(mut self) -> Result<std::path::PathBuf, String> {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        let part = crate::ablage::part_path(&self.path);
        std::fs::rename(&part, &self.path).map_err(|e| e.to_string())?;
        Ok(self.path.clone())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------------------------
// Suchlauf

/// Ein gefundener Sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedService {
    pub name: String,
    pub provider: String,
    pub onid: u16,
    pub tsid: u16,
    pub service_id: u16,
    pub hd: bool,
    pub radio: bool,
    pub scrambled: bool,
    /// Abstimmung (SAT>IP-Adresse ab `src` bzw. `freq`, ohne `pids`).
    pub query: String,
    /// PAT, PMT, PCR und alle abspielbaren Spuren.
    pub pids: Vec<u16>,
}

impl ScannedService {
    /// SAT>IP-Adresse dieses Senders am Gerät `host`.
    pub fn url(&self, host: &str) -> String {
        let pids: Vec<String> = self.pids.iter().map(u16::to_string).collect();
        format!(
            "rtsp://{host}:{PORT}/?{}&pids={}",
            self.query,
            pids.join(",")
        )
    }

    /// Kennung zum Wiedererkennen bei einem neuen Suchlauf.
    pub fn key(&self) -> String {
        format!("satip:{}:{}:{}", self.onid, self.tsid, self.service_id)
    }
}

/// Ein Dienst aus den Tabellen eines Transponders (PAT, PMT, SDT), gleich über welchen Weg
/// empfangen (SAT>IP, DVB-Karte).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    pub name: String,
    pub provider: String,
    pub onid: u16,
    pub tsid: u16,
    pub service_id: u16,
    pub hd: bool,
    pub radio: bool,
    pub scrambled: bool,
    /// PAT, PMT, PCR und alle abspielbaren Spuren.
    pub pids: Vec<u16>,
}

/// TV- und Radiodienste aus gesammelten Tabellen (`pat`: Programmnummer, PMT-PID).
pub(crate) fn found_services(col: &crate::ts::SectionCollector, pat: &[(u16, u16)]) -> Vec<Found> {
    let pmts: HashMap<u16, (u16, Vec<crate::ts::EsInfo>)> = col
        .tables(0x02)
        .filter_map(crate::ts::parse_pmt)
        .map(|(prog, pcr, es)| (prog, (pcr, es)))
        .collect();
    let pmt_of: HashMap<u16, u16> = pat.iter().copied().collect();
    let mut out = Vec::new();
    for svc in col.tables(0x42).flat_map(crate::ts::parse_sdt) {
        let radio = matches!(svc.service_type, 0x02 | 0x0a);
        let tv = matches!(svc.service_type, 0x01 | 0x11 | 0x16 | 0x19 | 0x1f);
        if !(tv || radio) || svc.name.is_empty() {
            continue;
        }
        let Some(&pmt) = pmt_of.get(&svc.service_id) else {
            continue;
        };
        let (pcr, es) = pmts
            .get(&svc.service_id)
            .cloned()
            .unwrap_or((0x1fff, Vec::new()));
        let pids = service_pids(pmt, pcr, &es);
        let hd = matches!(svc.service_type, 0x19 | 0x1f)
            || es.iter().any(|e| matches!(e.stream_type, 0x1b | 0x24));
        out.push(Found {
            name: svc.name,
            provider: svc.provider,
            onid: svc.onid,
            tsid: svc.tsid,
            service_id: svc.service_id,
            hd,
            radio,
            scrambled: svc.scrambled,
            pids,
        });
    }
    out
}

/// Fortschritt des Suchlaufs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanProgress {
    pub done: usize,
    pub total: usize,
    pub services: usize,
}


/// Pakete einer Sitzung empfangen und an den Sammler geben, bis `done` erfüllt oder die Zeit
/// um ist.
fn collect(
    sock: &UdpSocket,
    limit: Duration,
    wanted: &dyn Fn(u16) -> bool,
    done: &dyn Fn(&crate::ts::SectionCollector) -> bool,
) -> crate::ts::SectionCollector {
    let mut col = crate::ts::SectionCollector::default();
    sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let mut buf = [0u8; 2048];
    let t = Instant::now();
    let mut checked = Instant::now();
    while t.elapsed() < limit {
        if let Ok(n) = sock.recv(&mut buf) {
            if n > 12 {
                for p in buf[12..n].chunks(188) {
                    col.push_packet(p, wanted);
                }
            }
        }
        if checked.elapsed() > Duration::from_millis(250) {
            checked = Instant::now();
            if done(&col) {
                break;
            }
        }
    }
    col
}

/// Programmdaten (EIT, PID 18) eines Transponders sammeln: `query` ist die Abstimmung ohne
/// `pids`; gelesen wird `secs` Sekunden lang (40 s reichen für mehrere Tage).
pub fn collect_eit(host: &str, query: &str, secs: u64) -> Result<Vec<crate::ts::EitSection>, String> {
    let sock = bind_rtp()?;
    let port = sock.local_addr().map_err(|e| e.to_string())?.port();
    let s = Session::setup(host, &format!("{query}&pids=18"), port)?;
    if let Err(e) = s.play() {
        s.teardown();
        return Err(e);
    }
    let col = collect(&sock, Duration::from_secs(secs), &|pid| pid == 18, &|_| false);
    s.teardown();
    if col.sections.is_empty() {
        return Err(tr("keine Programmdaten empfangen").into());
    }
    Ok(col
        .sections
        .iter()
        .filter_map(|(_, sec)| crate::ts::parse_eit(sec))
        .collect())
}

/// Abstimmung eines Senders ohne Spurliste (`src=…&freq=…`), zum Gruppieren nach Transponder.
pub fn tuning_of(url: &str) -> Option<(String, String)> {
    let (host, query) = split_url(url)?;
    let q: Vec<&str> = query.split('&').filter(|p| !p.starts_with("pids=")).collect();
    Some((host, q.join("&")))
}

/// Transponder des Satelliten aus der Netzwerktabelle seines Starttransponders (laut
/// Beschreibung der Empfangsquelle).
pub fn transponders(host: &str, sat: &crate::adapter::Satellite) -> Result<Vec<Transponder>, String> {
    let sock = bind_rtp()?;
    let port = sock.local_addr().map_err(|e| e.to_string())?.port();
    let s = Session::setup(host, &format!("{}&pids=0,16", sat.seed.query()), port)?;
    s.play()?;
    let col = collect(&sock, Duration::from_secs(15), &|pid| pid == 16, &|c| {
        // Alle Abschnitte der eigenen NIT da? (letzte Abschnittsnummer erreicht)
        let mut it = c.tables(0x40).peekable();
        let Some(first) = it.peek() else { return false };
        let last = first[7];
        c.tables(0x40).count() > last as usize
    });
    s.teardown();
    let mut out: Vec<Transponder> = Vec::new();
    for sec in col.tables(0x40).chain(col.tables(0x41)) {
        for d in crate::ts::parse_nit(sec) {
            if d.orbital != sat.orbital || d.east != sat.east || d.freq == 0 {
                continue;
            }
            let tp = Transponder {
                freq: d.freq,
                pol: d.pol,
                dvbs2: d.dvbs2,
                mtype: d.mtype,
                sr: d.sr,
                fec: d.fec,
            };
            if !out.iter().any(|x| x.freq == tp.freq && x.pol == tp.pol) {
                out.push(tp);
            }
        }
    }
    out.sort_by_key(|t| (t.freq, t.pol));
    if out.is_empty() {
        return Err(tr("Keine Transponder in der Netzwerktabelle gefunden.").into());
    }
    Ok(out)
}

/// Einen Transponder durchsuchen: PAT, SDT und alle PMTs lesen. `None`, wenn kein Empfang.
fn scan_transponder(host: &str, query: &str) -> Result<Option<Vec<ScannedService>>, String> {
    let sock = bind_rtp()?;
    let port = sock.local_addr().map_err(|e| e.to_string())?.port();
    let s = Session::setup(host, &format!("{query}&pids=all"), port)?;
    let result = (|| {
        s.play()?;
        let col = collect(&sock, Duration::from_millis(1500), &|pid| pid == 0, &|c| {
            c.tables(0x00).next().is_some()
        });
        // Ohne PAT nach 1,5 s: Empfang prüfen.
        let mut col = if col.tables(0x00).next().is_none() {
            let locked = s
                .describe()
                .ok()
                .and_then(|(_, st)| st)
                .is_some_and(|st| st.lock);
            if !locked {
                return Ok(None);
            }
            col
        } else {
            col
        };
        let pat: Vec<(u16, u16)> = col.tables(0x00).flat_map(crate::ts::parse_pat).collect();
        let pmt_pids: Vec<u16> = pat.iter().map(|(_, p)| *p).collect();
        let more = collect(
            &sock,
            Duration::from_secs(6),
            &|pid| pid == 0 || pid == 17 || pmt_pids.contains(&pid),
            &|c| {
                let pmts = c.tables(0x02).count();
                c.tables(0x42).next().is_some() && pmts >= pmt_pids.len()
            },
        );
        col.sections.extend(more.sections);
        let out = found_services(&col, &pat)
            .into_iter()
            .map(|f| ScannedService {
                name: f.name,
                provider: f.provider,
                onid: f.onid,
                tsid: f.tsid,
                service_id: f.service_id,
                hd: f.hd,
                radio: f.radio,
                scrambled: f.scrambled,
                query: query.to_string(),
                pids: f.pids,
            })
            .collect();
        Ok(Some(out))
    })();
    s.teardown();
    result
}

/// Suchlauf über alle Transponder mit `workers` Tunern parallel. Transponder ohne Empfang
/// werden am Ende einmal wiederholt (anderer Tuner).
pub fn scan(
    host: &str,
    sat: &crate::adapter::Satellite,
    workers: usize,
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<Vec<ScannedService>, String> {
    let list: Vec<String> = transponders(host, sat)?.iter().map(Transponder::query).collect();
    scan_queries(host, list, workers, progress)
}

/// Suchlauf über einen Frequenzplan (Antenne, Kabel) mit `workers` Tunern parallel.
pub fn scan_plan(
    host: &str,
    tunings: &[crate::plan::Tuning],
    workers: usize,
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<Vec<ScannedService>, String> {
    scan_queries(host, tunings.iter().map(query_for).collect(), workers, progress)
}

/// Abstimmung eines Kanals als SAT>IP-Adresse (ohne `pids`), nach SAT>IP 1.2: DVB-T/T2
/// `freq` (MHz), `bw`, `msys`, ggf. `plp`; DVB-C `freq`, `msys=dvbc`, `sr` (kSym/s), `mtype`.
pub fn query_for(t: &crate::plan::Tuning) -> String {
    use crate::plan::System;
    let mhz = |hz: u32| {
        let v = f64::from(hz) / 1e6;
        if v.fract() == 0.0 {
            format!("{v:.0}")
        } else {
            format!("{v}")
        }
    };
    match t.system {
        System::DvbT | System::DvbT2 => {
            let mut q = format!(
                "freq={}&bw={}&msys={}",
                mhz(t.freq),
                t.bandwidth / 1_000_000,
                t.system.code()
            );
            if let Some(p) = t.plp {
                q += &format!("&plp={p}");
            }
            q
        }
        System::DvbC => format!(
            "freq={}&msys=dvbc&sr={}&mtype={}",
            mhz(t.freq),
            t.symbol_rate / 1000,
            match t.modulation.as_str() {
                "qam64" => "64qam",
                "qam128" => "128qam",
                _ => "256qam",
            }
        ),
        System::DvbS | System::DvbS2 => format!(
            "src=1&freq={}&pol={}&msys={}&sr={}",
            t.freq / 1000,
            t.pol,
            t.system.code(),
            t.symbol_rate / 1000
        ),
    }
}

/// Senderliste vom Gerät (M3U-Listen mit RTSP-Adressen, z. B. FRITZ!Box Cable); `urls` mit
/// `{host}`. Gibt die Sender für die Senderliste zurück.
pub fn channel_list(host: &str, urls: &[String]) -> Result<Vec<crate::ScannedChannel>, String> {
    let agent = crate::net::agent(Duration::from_secs(20));
    let mut out: Vec<crate::ScannedChannel> = Vec::new();
    let mut errors = Vec::new();
    for tpl in urls {
        let url = tpl.replace("{host}", host);
        let bytes = match crate::net::fetch(&agent, &url, 4 << 20) {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("{url}: {e}"));
                continue;
            }
        };
        let list = crate::Playlist::parse(&bytes).map_err(|e| format!("{url}: {e}"))?;
        // HD-Liste vor SD-Liste: gleiche Sender nur einmal.
        let hd = url.contains("hd");
        for e in list.entries {
            if split_url(&e.url).is_none() || e.name.trim().is_empty() {
                continue;
            }
            let key = format!("satip-list:{}", crate::normalize_id(&e.name));
            if out.iter().any(|c| c.key == key) {
                continue;
            }
            out.push(crate::ScannedChannel {
                key,
                name: e.name.trim().to_string(),
                group: None,
                stream: crate::Stream {
                    quality: Some(if hd { crate::Quality::P720 } else { crate::Quality::Sd }),
                    kind: Some(crate::SourceKind::SatIp),
                    url: e.url.clone(),
                    max_bitrate: None,
                },
                radio: false,
                epg_id: None,
            });
        }
    }
    if out.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(out)
}

/// Suchlauf über eine Liste von Abstimmungen; Transponder ohne Empfang werden am Ende einmal
/// wiederholt (anderer Tuner).
fn scan_queries(
    host: &str,
    list: Vec<String>,
    workers: usize,
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<Vec<ScannedService>, String> {
    let total = list.len();
    let queue = std::sync::Mutex::new(list.into_iter().map(|t| (t, 0u8)).collect::<Vec<_>>());
    let found = std::sync::Mutex::new(Vec::<ScannedService>::new());
    let done = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..workers.max(1) {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().ok().and_then(|mut q| q.pop());
                    let Some((tp, tries)) = next else { break };
                    match scan_transponder(host, &tp) {
                        Ok(Some(svcs)) => {
                            found.lock().unwrap().extend(svcs);
                        }
                        Ok(None) | Err(_) if tries == 0 => {
                            queue.lock().unwrap().insert(0, (tp, 1));
                            continue;
                        }
                        _ => {}
                    }
                    let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    let n = found.lock().map(|f| f.len()).unwrap_or(0);
                    progress(ScanProgress {
                        done: d,
                        total,
                        services: n,
                    });
                }
            });
        }
    });
    let mut out = found.into_inner().unwrap_or_default();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.key() == b.key());
    Ok(out)
}

/// Frei empfangbare TV-Sender eines Suchlaufs für die SAT-Senderliste (ohne Radio, ohne
/// verschlüsselte, ohne Platzhalternamen ohne Buchstaben wie „.“ oder „.4.“ und ohne
/// Testdienste „TEST_…“).
pub fn to_channels(host: &str, services: &[ScannedService]) -> Vec<crate::ScannedChannel> {
    services
        .iter()
        .filter(|s| {
            !s.radio
                && !s.scrambled
                && s.name.chars().any(char::is_alphabetic)
                && !s.name.starts_with("TEST_")
        })
        .map(|s| crate::ScannedChannel {
            key: s.key(),
            // Ersatzdienste von SES heißen „ALT_Name“; angezeigt wird nur der Name.
            name: s.name.strip_prefix("ALT_").unwrap_or(&s.name).to_string(),
            group: (!s.provider.is_empty()).then(|| s.provider.clone()),
            stream: crate::Stream {
                quality: Some(if s.hd {
                    crate::Quality::P720
                } else {
                    crate::Quality::Sd
                }),
                kind: Some(crate::SourceKind::SatIp),
                url: s.url(host),
                max_bitrate: None,
            },
            radio: false,
            epg_id: None,
        })
        .collect()
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

/// Kann tvjoe eine Firewall-Freigabe für das Gerät anbieten? Ubuntu: wenn ufw aktiv ist;
/// Windows: immer (Windows-Firewall, Eintrag über die Benutzerkontensteuerung).
pub fn firewall_fix_available() -> bool {
    cfg!(windows) || ufw_active()
}

/// Nur Adressen und Namen aus Ziffern, Buchstaben, Punkt, Bindestrich und Doppelpunkt (die
/// Adresse landet in einem Befehl).
fn plain_host(host: &str) -> bool {
    !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
}

/// Die Freigabe in lesbarer Form (für die Rückfrage): nur UDP vom SAT>IP-Gerät.
pub fn firewall_rule_text(host: &str) -> String {
    if cfg!(windows) {
        format!(
            "New-NetFirewallRule -DisplayName \"tvjoe SAT>IP {host}\" -Direction Inbound -Protocol UDP -RemoteAddress {host} -Action Allow"
        )
    } else {
        ufw_rule_command(host)[1..].join(" ")
    }
}

/// Befehl für die Freigabe (Ubuntu: pkexec ufw, fragt nach dem Passwort; Windows: PowerShell mit
/// erhöhten Rechten, fragt über die Benutzerkontensteuerung). `None` bei ungewöhnlicher Adresse.
pub fn firewall_rule_command(host: &str) -> Option<Vec<String>> {
    if !plain_host(host) {
        return None;
    }
    if !cfg!(windows) {
        return Some(ufw_rule_command(host));
    }
    // Die erhöhte PowerShell bekommt die Regel als -EncodedCommand (Base64 aus UTF-16LE): so muss
    // nichts maskiert werden (Start-Process fügt -ArgumentList ohne Anführungszeichen zusammen).
    // Rückgabewert 0 nur, wenn die Regel angelegt wurde; Abbruch der Benutzerkontensteuerung oder
    // ein Fehler ergeben ≠ 0.
    let script = format!(
        "try {{ New-NetFirewallRule -DisplayName 'tvjoe SAT>IP {host}' -Direction Inbound -Protocol UDP -RemoteAddress {host} -Action Allow -ErrorAction Stop | Out-Null; exit 0 }} catch {{ exit 1 }}"
    );
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let inner = format!(
        "try {{ $p = Start-Process powershell.exe -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile','-EncodedCommand','{}'; exit $p.ExitCode }} catch {{ exit 1 }}",
        base64(&utf16)
    );
    Some(vec![
        "powershell.exe".into(),
        "-NoProfile".into(),
        "-Command".into(),
        inner,
    ])
}

/// Base64 (Standardalphabet, mit Auffüllung) für -EncodedCommand.
fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// Messung am Gerät: `TVJOE_EIT_HOST=<gerät> TVJOE_EIT_TP="<abstimmung>;…"
    /// cargo test -p tvjoe-core eit_probe -- --ignored --nocapture`. Liest je Transponder 40 s
    /// PID 18 und zeigt je Dienst Zahl der Sendungen und wie weit das Programm reicht.
    #[test]
    #[ignore]
    fn eit_probe() {
        let (Ok(host), Ok(tps)) = (std::env::var("TVJOE_EIT_HOST"), std::env::var("TVJOE_EIT_TP"))
        else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        for q in tps.split(';').filter(|q| !q.is_empty()) {
            let sock = bind_rtp().unwrap();
            let port = sock.local_addr().unwrap().port();
            let s = Session::setup(&host, &format!("{q}&pids=0,17,18"), port).unwrap();
            s.play().unwrap();
            let col = collect(&sock, Duration::from_secs(40), &|pid| pid == 18 || pid == 17, &|_| false);
            s.teardown();
            let names: std::collections::HashMap<u16, String> = col
                .tables(0x42)
                .flat_map(crate::ts::parse_sdt)
                .map(|x| (x.service_id, x.name))
                .collect();
            let mut per: std::collections::BTreeMap<u16, (usize, i64, String)> = Default::default();
            let mut tables = std::collections::BTreeSet::new();
            for (_, sec) in &col.sections {
                if let Some(e) = crate::ts::parse_eit(sec) {
                    tables.insert(sec[0]);
                    let ent = per.entry(e.service_id).or_insert((0, 0, String::new()));
                    for ev in e.events {
                        ent.0 += 1;
                        ent.1 = ent.1.max(ev.stop);
                        if ent.2.is_empty() {
                            ent.2 = format!("{} / {}", ev.title, ev.desc.as_deref().unwrap_or("").chars().take(60).collect::<String>());
                        }
                    }
                }
            }
            eprintln!("== {q}: {} Abschnitte, Tabellen {:02x?}", col.sections.len(), tables);
            for (sid, (n, max, sample)) in per {
                eprintln!(
                    "  {sid:5} {:28} {n:5} Sendungen, bis +{:.1} Tage | {sample}",
                    names.get(&sid).cloned().unwrap_or_default(),
                    (max - now) as f64 / 86400.0
                );
            }
        }
    }

    use super::*;

    #[test]
    fn base64_wie_ueblich() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn freigabe_nur_fuer_schlichte_adressen() {
        assert!(firewall_rule_command("192.0.2.10").is_some());
        assert!(firewall_rule_command("netstream.local").is_some());
        assert!(firewall_rule_command("x; rm -rf /").is_none());
        assert!(firewall_rule_command("").is_none());
    }

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

    /// Echter Suchlauf: `TVJOE_TEST_SATIP=<Adresse> TVJOE_TEST_SATIP_TUNER=3 cargo test
    /// -p tvjoe-core --release -- --ignored --nocapture echter_suchlauf` (belegt die Tuner).
    #[test]
    #[ignore]
    fn echter_suchlauf() {
        let Ok(host) = std::env::var("TVJOE_TEST_SATIP") else {
            return;
        };
        let workers = std::env::var("TVJOE_TEST_SATIP_TUNER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let sat = crate::adapter::find("netstream-4sat", None)
            .and_then(|d| d.satellite)
            .unwrap();
        let t = Instant::now();
        let tps = transponders(&host, &sat).unwrap();
        println!(
            "{} Transponder in {:.1} s",
            tps.len(),
            t.elapsed().as_secs_f64()
        );
        let t = Instant::now();
        let all = scan(&host, &sat, workers, &|p| {
            if p.done % 10 == 0 || p.done == p.total {
                println!(
                    "  {}/{} Transponder, {} Sender",
                    p.done, p.total, p.services
                );
            }
        })
        .unwrap();
        let tv: Vec<_> = all.iter().filter(|s| !s.radio).collect();
        let free: Vec<_> = tv.iter().filter(|s| !s.scrambled).collect();
        println!(
            "Suchlauf {:.0} s: {} Dienste, {} TV, davon {} frei, {} HD frei",
            t.elapsed().as_secs_f64(),
            all.len(),
            tv.len(),
            free.len(),
            free.iter().filter(|s| s.hd).count()
        );
        for s in free.iter().filter(|s| s.hd).take(15) {
            println!("  {} | {} | {}", s.name, s.provider, s.url(&host));
        }
        // Abgleich mit der Senderliste: ein zweiter Abgleich ändert keine Nummer.
        let items = to_channels(&host, &all);
        let mut lib = crate::Library::default();
        let first = lib.sync_scanned("SAT>IP", "satip://test", &items);
        let numbers: Vec<_> = lib
            .channels
            .iter()
            .map(|c| (c.key.clone(), c.number))
            .collect();
        let second = lib.sync_scanned("SAT>IP", "satip://test", &items[1..]);
        println!(
            "Senderliste: {} übernommen, erneut: {} aktualisiert, {} fehlt",
            first.added, second.updated, second.missing
        );
        assert_eq!(first.added, items.len());
        assert_eq!(second.missing, 1);
        let again: Vec<_> = lib
            .channels
            .iter()
            .map(|c| (c.key.clone(), c.number))
            .collect();
        assert_eq!(numbers, again);
        for c in lib.channels.iter().filter(|c| {
            !c.name
                .chars()
                .all(|ch| ch.is_alphanumeric() || " .-+&'!()/".contains(ch))
        }) {
            println!("  ungewöhnlicher Name: {:?}", c.name);
        }
    }

    /// Echtes Gerät: `TVJOE_TEST_SATIP=<Adresse> cargo test -p tvjoe-core -- --ignored
    /// --nocapture echtes_geraet` (belegt kurz die Tuner).
    #[test]
    fn adresse_zerlegen() {
        let (h, q) = split_url("rtsp://192.0.2.5:554/?src=1&freq=11347&pol=v&pids=0,100").unwrap();
        assert_eq!(h, "192.0.2.5");
        assert_eq!(q, "src=1&freq=11347&pol=v&pids=0,100");
        assert!(split_url("rtsp://example.org/live").is_none());
        assert!(split_url("http://example.org/?src=1&freq=1").is_none());
    }

    #[test]
    fn rtp_nutzdaten() {
        let mut p = vec![0x80, 33, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        p.extend_from_slice(&[0x47; 188]);
        assert_eq!(rtp_payload(&p).unwrap().len(), 188);
        // Mit Erweiterung (1 Wort) und einer CSRC.
        let mut q = vec![
            0x91, 33, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 0, 0, 0, 1, 9, 9, 9, 9,
        ];
        q.extend_from_slice(&[0x47; 188]);
        assert_eq!(rtp_payload(&q).unwrap()[0], 0x47);
        assert!(rtp_payload(&[0u8; 20]).is_none());
    }

    /// Echtes Gerät: Weitergabe eines Senders an 127.0.0.1 (belegt kurz einen Tuner).
    #[test]
    #[ignore]
    fn echte_weitergabe() {
        let Ok(url) = std::env::var("TVJOE_TEST_SATIP_URL") else {
            return;
        };
        let relay = Relay::start(&url).unwrap();
        let mut c = TcpStream::connect(
            relay
                .local_url
                .trim_start_matches("http://")
                .split('/')
                .next()
                .unwrap(),
        )
        .unwrap();
        c.write_all(b"GET /satip.ts HTTP/1.1\r\n\r\n").unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut total = 0;
        let mut buf = [0u8; 65536];
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(4) {
            total += c.read(&mut buf).unwrap_or(0);
        }
        println!(
            "{} kB in 4 s, {} Pakete, Status {:?}",
            total / 1024,
            relay.packets(),
            relay.status()
        );
        assert!(total > 100_000);
        drop(c);
        let t = Instant::now();
        relay.close();
        println!("freigegeben in {} ms", t.elapsed().as_millis());
    }

    #[test]
    #[ignore]
    fn echtes_geraet() {
        let Ok(host) = std::env::var("TVJOE_TEST_SATIP") else {
            return;
        };
        let r = check(&host, Duration::from_secs(3)).unwrap();
        println!("{r:#?}");
    }

    /// WDR HD Düsseldorf (PMT 160) am 2026-10-29 gemessen: zur Tagesschau Kölner Spuren,
    /// zur Lokalzeit eigene.
    #[test]
    fn regionalfenster_wechselt_spuren() {
        use crate::ts::EsInfo;
        let es = |list: &[(u8, u16)]| -> Vec<EsInfo> {
            list.iter().map(|&(t, pid)| EsInfo { stream_type: t, pid, playable: t != 0x05 && t != 0x0b && t != 0x0c }).collect()
        };
        let pat = [(0, 16), (28332, 100), (28338, 160), (28339, 170)];
        let tagesschau = es(&[(0x1b, 101), (0x03, 102), (0x03, 103), (0x03, 107), (0x06, 104), (0x06, 106), (0x06, 105), (0x0b, 2171), (0x05, 2371)]);
        let lokalzeit = es(&[(0x1b, 161), (0x03, 162), (0x06, 104), (0x06, 105), (0x0b, 2171), (0x05, 2371)]);
        let gespeichert = requested_pids("src=1&freq=11523&pol=h&pids=0,101,102,103,104,105,106,107,160").unwrap();
        // Tagesschau: Liste passt.
        assert_eq!(changed_pids(&pat, &[(28338, 101, tagesschau.clone())], &gespeichert), None);
        // Lokalzeit: eigene Spuren fehlen, neue Liste.
        let neu = changed_pids(&pat, &[(28338, 161, lokalzeit)], &gespeichert).unwrap();
        assert_eq!(neu, vec![0, 104, 105, 160, 161, 162]);
        assert_eq!(
            with_pids("rtsp://192.0.2.5:554/?src=1&freq=11523&pids=0,101,160", &neu),
            "rtsp://192.0.2.5:554/?src=1&freq=11523&pids=0,104,105,160,161,162"
        );
        // Zurück zur Tagesschau: wieder Kölner Spuren.
        let zurueck = changed_pids(&pat, &[(28338, 101, tagesschau)], &neu).unwrap();
        assert!(zurueck.contains(&101) && zurueck.contains(&160));
        // Ohne eindeutige PMT in der Liste oder bei pids=all nichts.
        assert_eq!(changed_pids(&pat, &[], &[0, 100, 160]), None);
        assert_eq!(requested_pids("src=1&pids=all"), None);
    }
}
