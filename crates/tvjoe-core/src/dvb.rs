//! DVB-Karten unter Linux (USB, PCI), Treiber `linux-dvb` (Nutzer, 2026-10-27; experimentell,
//! geprüft nur mit dem virtuellen Gerät `dvb_vidtv`).
//!
//! tvjoe stimmt das Frontend (`/dev/dvb/adapterN/frontendM`) über die DVB-Schnittstelle v5 des
//! Kernels ab und liest den Transportstrom aus dem Demux (`demuxM`, Ausgabe auf den Demux
//! selbst). Senderadressen haben die Form
//! `dvb://N.M/?sys=dvbt2&freq=474000000&bw=8000000&pids=0,256,…` (Frequenz bei DVB-T/C in Hz,
//! bei Satellit in kHz im Ku-Band).
//!
//! Zugriff auf `/dev/dvb` braucht in der Regel die Gruppe `video`.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use crate::plan::{Plan, System, Tuning, plans};
use crate::{tf, tr};

// ---------------------------------------------------------------------------------------------
// Schnittstelle des Kernels (linux/dvb/frontend.h, dmx.h)

const fn ioc(dir: u64, nr: u64, size: u64) -> u64 {
    (dir << 30) | (size << 16) | ((b'o' as u64) << 8) | nr
}
const IOC_NONE: u64 = 0;
const IOC_WRITE: u64 = 1;
const IOC_READ: u64 = 2;

const FE_GET_INFO: u64 = ioc(IOC_READ, 61, 168);
const FE_READ_STATUS: u64 = ioc(IOC_READ, 69, 4);
const FE_READ_SIGNAL_STRENGTH: u64 = ioc(IOC_READ, 71, 2);
const FE_READ_SNR: u64 = ioc(IOC_READ, 72, 2);
const FE_SET_PROPERTY: u64 = ioc(IOC_WRITE, 82, std::mem::size_of::<DtvProperties>() as u64);
const FE_GET_PROPERTY: u64 = ioc(IOC_READ, 83, std::mem::size_of::<DtvProperties>() as u64);
const DMX_STOP: u64 = ioc(IOC_NONE, 42, 0);
const DMX_SET_PES_FILTER: u64 = ioc(IOC_WRITE, 44, std::mem::size_of::<DmxPesFilter>() as u64);
const DMX_SET_BUFFER_SIZE: u64 = ioc(IOC_NONE, 45, 0);
const DMX_ADD_PID: u64 = ioc(IOC_WRITE, 51, 2);

const DTV_TUNE: u32 = 1;
const DTV_CLEAR: u32 = 2;
const DTV_FREQUENCY: u32 = 3;
const DTV_MODULATION: u32 = 4;
const DTV_BANDWIDTH_HZ: u32 = 5;
const DTV_INVERSION: u32 = 6;
const DTV_SYMBOL_RATE: u32 = 8;
const DTV_INNER_FEC: u32 = 9;
const DTV_VOLTAGE: u32 = 10;
const DTV_TONE: u32 = 11;
const DTV_PILOT: u32 = 12;
const DTV_ROLLOFF: u32 = 13;
const DTV_DELIVERY_SYSTEM: u32 = 17;
const DTV_STREAM_ID: u32 = 42;
const DTV_ENUM_DELSYS: u32 = 44;

const SYS_DVBC_ANNEX_A: u32 = 1;
const SYS_DVBT: u32 = 3;
const SYS_DVBS: u32 = 5;
const SYS_DVBS2: u32 = 6;
const SYS_DVBT2: u32 = 16;

const QPSK: u32 = 0;
const QAM_64: u32 = 3;
const QAM_128: u32 = 4;
const QAM_256: u32 = 5;
const QAM_AUTO: u32 = 6;
const PSK_8: u32 = 9;
const INVERSION_AUTO: u32 = 2;
const FEC_AUTO: u32 = 9;
const ROLLOFF_AUTO: u32 = 3;
const PILOT_AUTO: u32 = 2;
const SEC_VOLTAGE_13: u32 = 0;
const SEC_VOLTAGE_18: u32 = 1;
const SEC_TONE_ON: u32 = 0;
const SEC_TONE_OFF: u32 = 1;

const FE_HAS_SIGNAL: u32 = 0x01;
const FE_HAS_CARRIER: u32 = 0x02;
const FE_HAS_LOCK: u32 = 0x10;

const DMX_IN_FRONTEND: u32 = 0;
const DMX_OUT_TSDEMUX_TAP: u32 = 3;
const DMX_PES_OTHER: u32 = 20;
const DMX_IMMEDIATE_START: u32 = 4;
/// Alle PIDs.
const PID_ALL: u16 = 0x2000;

/// `struct dtv_property` (gepackt, 76 Bytes): Befehl, Reserve, Wert bzw. Puffer, Ergebnis.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct DtvProperty {
    cmd: u32,
    reserved: [u32; 3],
    data: [u8; 56],
    result: i32,
}

impl DtvProperty {
    fn new(cmd: u32, value: u32) -> Self {
        let mut data = [0u8; 56];
        data[..4].copy_from_slice(&value.to_ne_bytes());
        Self {
            cmd,
            reserved: [0; 3],
            data,
            result: 0,
        }
    }
}

#[repr(C)]
struct DtvProperties {
    num: u32,
    props: *mut DtvProperty,
}

#[repr(C)]
struct DmxPesFilter {
    pid: u16,
    input: u32,
    output: u32,
    pes_type: u32,
    flags: u32,
}

fn ioctl<T>(fd: &OwnedFd, req: u64, arg: *mut T) -> std::io::Result<()> {
    // SAFETY: `arg` zeigt auf einen Puffer in der Größe, die `req` angibt.
    let r = unsafe { libc::ioctl(fd.as_raw_fd(), req as _, arg) };
    if r < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn ioctl_value(fd: &OwnedFd, req: u64, value: libc::c_ulong) -> std::io::Result<()> {
    // SAFETY: Befehl mit Zahlenwert statt Zeiger.
    let r = unsafe { libc::ioctl(fd.as_raw_fd(), req as _, value) };
    if r < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn open(path: &Path, write: bool) -> std::io::Result<OwnedFd> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags_nonblock()
        .open(path)?;
    Ok(f.into())
}

trait NonBlock {
    fn custom_flags_nonblock(&mut self) -> &mut Self;
}

impl NonBlock for std::fs::OpenOptions {
    fn custom_flags_nonblock(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.custom_flags(libc::O_NONBLOCK)
    }
}

fn os_error(path: &Path, e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        tf!(
            "{path}: kein Zugriff (Benutzer in die Gruppe „video“ aufnehmen und neu anmelden)",
            path = path.display()
        )
    } else if e.raw_os_error() == Some(libc::EBUSY) {
        tf!("{path}: Tuner belegt", path = path.display())
    } else {
        format!("{}: {e}", path.display())
    }
}

// ---------------------------------------------------------------------------------------------
// Geräte

fn kernel_system(s: System) -> u32 {
    match s {
        System::DvbT => SYS_DVBT,
        System::DvbT2 => SYS_DVBT2,
        System::DvbC => SYS_DVBC_ANNEX_A,
        System::DvbS => SYS_DVBS,
        System::DvbS2 => SYS_DVBS2,
    }
}

fn system_from_kernel(v: u8) -> Option<System> {
    Some(match u32::from(v) {
        SYS_DVBT => System::DvbT,
        SYS_DVBT2 => System::DvbT2,
        SYS_DVBC_ANNEX_A => System::DvbC,
        SYS_DVBS => System::DvbS,
        SYS_DVBS2 => System::DvbS2,
        _ => return None,
    })
}

/// Ein Frontend einer DVB-Karte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub adapter: u32,
    pub frontend: u32,
    /// Name laut Treiber.
    pub name: String,
    pub systems: Vec<System>,
}

impl Device {
    /// Kennung `N.M` (Adapter, Frontend).
    pub fn id(&self) -> String {
        format!("{}.{}", self.adapter, self.frontend)
    }
}

fn frontend_path(adapter: u32, frontend: u32) -> PathBuf {
    PathBuf::from(format!("/dev/dvb/adapter{adapter}/frontend{frontend}"))
}

fn demux_path(adapter: u32, frontend: u32) -> PathBuf {
    PathBuf::from(format!("/dev/dvb/adapter{adapter}/demux{frontend}"))
}

/// Vorhandene DVB-Frontends (leer ohne `/dev/dvb`).
pub fn devices() -> Vec<Device> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/dev/dvb") else {
        return out;
    };
    let mut adapters: Vec<u32> = dir
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_prefix("adapter")?.parse().ok())
        .collect();
    adapters.sort_unstable();
    for a in adapters {
        let mut fronts: Vec<u32> = std::fs::read_dir(format!("/dev/dvb/adapter{a}"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_prefix("frontend")?.parse().ok())
            .collect();
        fronts.sort_unstable();
        for f in fronts {
            if let Ok(d) = probe(a, f) {
                out.push(d);
            }
        }
    }
    out
}

/// Name und Empfangsarten eines Frontends.
pub fn probe(adapter: u32, frontend: u32) -> Result<Device, String> {
    let path = frontend_path(adapter, frontend);
    let fd = open(&path, false).map_err(|e| os_error(&path, e))?;
    let mut info = [0u8; 168];
    ioctl(&fd, FE_GET_INFO, info.as_mut_ptr()).map_err(|e| os_error(&path, e))?;
    let name = String::from_utf8_lossy(info[..128].split(|b| *b == 0).next().unwrap_or(&[]))
        .trim()
        .to_string();
    let mut prop = DtvProperty::new(DTV_ENUM_DELSYS, 0);
    let mut props = DtvProperties {
        num: 1,
        props: &mut prop,
    };
    ioctl(&fd, FE_GET_PROPERTY, &mut props).map_err(|e| os_error(&path, e))?;
    let data = prop.data;
    let len = u32::from_ne_bytes([data[32], data[33], data[34], data[35]]).min(32) as usize;
    let mut systems: Vec<System> = data[..len].iter().filter_map(|v| system_from_kernel(*v)).collect();
    systems.dedup();
    Ok(Device {
        adapter,
        frontend,
        name,
        systems,
    })
}

// ---------------------------------------------------------------------------------------------
// Abstimmen

fn properties(t: &Tuning) -> Vec<DtvProperty> {
    let p = DtvProperty::new;
    let mut v = vec![p(DTV_CLEAR, 0), p(DTV_DELIVERY_SYSTEM, kernel_system(t.system))];
    match t.system {
        System::DvbT | System::DvbT2 => {
            v.push(p(DTV_FREQUENCY, t.freq));
            v.push(p(DTV_BANDWIDTH_HZ, t.bandwidth));
            v.push(p(DTV_MODULATION, QAM_AUTO));
            v.push(p(DTV_INVERSION, INVERSION_AUTO));
            if t.system == System::DvbT2 {
                // Ohne Angabe: erster Datenstrom (u32::MAX = keiner gewählt).
                v.push(p(DTV_STREAM_ID, t.plp.unwrap_or(u32::MAX)));
            }
        }
        System::DvbC => {
            v.push(p(DTV_FREQUENCY, t.freq));
            v.push(p(DTV_SYMBOL_RATE, t.symbol_rate));
            v.push(p(
                DTV_MODULATION,
                match t.modulation.as_str() {
                    "qam64" => QAM_64,
                    "qam128" => QAM_128,
                    "qam256" => QAM_256,
                    _ => QAM_AUTO,
                },
            ));
            v.push(p(DTV_INNER_FEC, FEC_AUTO));
            v.push(p(DTV_INVERSION, INVERSION_AUTO));
        }
        System::DvbS | System::DvbS2 => {
            // Universal-LNB: unteres Band 9,75 GHz, oberes 10,6 GHz ab 11,7 GHz (22 kHz an).
            let high = t.freq >= 11_700_000;
            let lof = if high { 10_600_000 } else { 9_750_000 };
            v.push(p(DTV_FREQUENCY, t.freq.saturating_sub(lof)));
            v.push(p(
                DTV_VOLTAGE,
                if t.pol == 'v' { SEC_VOLTAGE_13 } else { SEC_VOLTAGE_18 },
            ));
            v.push(p(DTV_TONE, if high { SEC_TONE_ON } else { SEC_TONE_OFF }));
            v.push(p(DTV_SYMBOL_RATE, t.symbol_rate));
            v.push(p(DTV_INNER_FEC, FEC_AUTO));
            v.push(p(DTV_INVERSION, INVERSION_AUTO));
            if t.system == System::DvbS2 {
                v.push(p(
                    DTV_MODULATION,
                    if t.modulation == "8psk" { PSK_8 } else { QPSK },
                ));
                v.push(p(DTV_ROLLOFF, ROLLOFF_AUTO));
                v.push(p(DTV_PILOT, PILOT_AUTO));
            }
        }
    }
    v.push(p(DTV_TUNE, 0));
    v
}

/// Empfangswerte eines Frontends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Signal {
    pub lock: bool,
    /// 0–100 (laut Treiber; nicht jeder liefert Werte).
    pub level: u32,
    pub quality: u32,
}

/// Ein abgestimmtes Frontend (exklusiv; freigegeben beim Fallenlassen).
pub struct Frontend {
    fd: OwnedFd,
    path: PathBuf,
}

impl Frontend {
    pub fn open(adapter: u32, frontend: u32) -> Result<Self, String> {
        let path = frontend_path(adapter, frontend);
        let fd = open(&path, true).map_err(|e| os_error(&path, e))?;
        Ok(Self { fd, path })
    }

    /// Abstimmen und bis `limit` auf Synchronisation warten; `Ok(false)` = kein Empfang.
    pub fn tune(&self, t: &Tuning, limit: Duration) -> Result<bool, String> {
        let mut props = properties(t);
        let mut list = DtvProperties {
            num: props.len() as u32,
            props: props.as_mut_ptr(),
        };
        ioctl(&self.fd, FE_SET_PROPERTY, &mut list).map_err(|e| os_error(&self.path, e))?;
        let start = Instant::now();
        while start.elapsed() < limit {
            let st = self.status();
            if st & FE_HAS_LOCK != 0 {
                return Ok(true);
            }
            // Nach einer Sekunde ohne jedes Signal: hier sendet nichts (spart beim Suchlauf Zeit).
            if start.elapsed() > Duration::from_secs(1) && st & (FE_HAS_SIGNAL | FE_HAS_CARRIER) == 0 {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(false)
    }

    fn status(&self) -> u32 {
        let mut st: u32 = 0;
        let _ = ioctl(&self.fd, FE_READ_STATUS, &mut st);
        st
    }

    /// Empfang und Qualität (Prozent).
    pub fn signal(&self) -> Signal {
        let st = self.status();
        let mut level: u16 = 0;
        let mut snr: u16 = 0;
        let _ = ioctl(&self.fd, FE_READ_SIGNAL_STRENGTH, &mut level);
        let _ = ioctl(&self.fd, FE_READ_SNR, &mut snr);
        Signal {
            lock: st & FE_HAS_LOCK != 0,
            level: if st & FE_HAS_SIGNAL != 0 {
                u32::from(level) * 100 / 65535
            } else {
                0
            },
            quality: u32::from(snr) * 100 / 65535,
        }
    }
}

/// Transportstrom aus dem Demux (gefiltert auf PIDs bzw. alle).
pub struct Demux {
    fd: OwnedFd,
    path: PathBuf,
}

impl Demux {
    /// `pids` leer = alle PIDs.
    pub fn open(adapter: u32, frontend: u32, pids: &[u16]) -> Result<Self, String> {
        let path = demux_path(adapter, frontend);
        let fd = open(&path, true).map_err(|e| os_error(&path, e))?;
        // Großer Puffer im Kernel, damit beim kurzen Stocken des Lesers nichts verloren geht.
        let _ = ioctl_value(&fd, DMX_SET_BUFFER_SIZE, 8 << 20);
        let first = pids.first().copied().unwrap_or(PID_ALL);
        let mut f = DmxPesFilter {
            pid: first,
            input: DMX_IN_FRONTEND,
            output: DMX_OUT_TSDEMUX_TAP,
            pes_type: DMX_PES_OTHER,
            flags: DMX_IMMEDIATE_START,
        };
        ioctl(&fd, DMX_SET_PES_FILTER, &mut f).map_err(|e| os_error(&path, e))?;
        for pid in pids.iter().skip(1) {
            let mut p = *pid;
            ioctl(&fd, DMX_ADD_PID, &mut p).map_err(|e| os_error(&path, e))?;
        }
        Ok(Self { fd, path })
    }

    /// Daten lesen; wartet höchstens `wait` (dann `Ok(0)`).
    pub fn read(&mut self, buf: &mut [u8], wait: Duration) -> Result<usize, String> {
        let mut pfd = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: ein gültiger pollfd.
        let r = unsafe { libc::poll(&mut pfd, 1, wait.as_millis() as i32) };
        if r <= 0 {
            return Ok(0);
        }
        // SAFETY: Puffer mit seiner Länge.
        let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            // Nichts da bzw. Überlauf im Kernelpuffer: weiterlesen.
            Some(libc::EAGAIN) | Some(libc::EOVERFLOW) => Ok(0),
            _ => Err(os_error(&self.path, e)),
        }
    }
}

impl Drop for Demux {
    fn drop(&mut self) {
        let _ = ioctl_value(&self.fd, DMX_STOP, 0);
    }
}

// ---------------------------------------------------------------------------------------------
// Senderadressen

/// Senderadresse zerlegen: Adapter, Frontend, Abstimmung, PIDs.
pub fn split_url(url: &str) -> Option<(u32, u32, Tuning, Vec<u16>)> {
    let rest = url.strip_prefix("dvb://")?;
    let (dev, query) = rest.split_once("/?")?;
    let (a, f) = dev.split_once('.')?;
    let pids = query
        .split('&')
        .find_map(|p| p.strip_prefix("pids="))
        .map(|l| l.split(',').filter_map(|p| p.parse().ok()).collect())
        .unwrap_or_default();
    Some((a.parse().ok()?, f.parse().ok()?, Tuning::parse(query)?, pids))
}

/// Abstimmung ohne PIDs (zum Gruppieren nach Transponder): Gerät und Abstimmung.
pub fn tuning_of(url: &str) -> Option<(String, String)> {
    let (a, f, t, _) = split_url(url)?;
    Some((format!("{a}.{f}"), t.query()))
}

// ---------------------------------------------------------------------------------------------
// Suchlauf

/// Ein gefundener Sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DvbService {
    pub name: String,
    pub provider: String,
    pub onid: u16,
    pub tsid: u16,
    pub service_id: u16,
    pub hd: bool,
    pub radio: bool,
    pub scrambled: bool,
    pub tuning: Tuning,
    pub pids: Vec<u16>,
}

impl DvbService {
    /// Senderadresse am Gerät `dev` (`N.M`).
    pub fn url(&self, dev: &str) -> String {
        let pids: Vec<String> = self.pids.iter().map(u16::to_string).collect();
        format!("dvb://{dev}/?{}&pids={}", self.tuning.query(), pids.join(","))
    }

    /// Kennung zum Wiedererkennen (Senderschlüssel, zugleich EPG-Kennung aus dem Signal).
    pub fn key(&self) -> String {
        format!("{SIGNAL_PREFIX}{}:{}:{}", self.onid, self.tsid, self.service_id)
    }
}

/// Vorsilbe der Senderschlüssel und EPG-Kennungen aus dem Signal einer DVB-Karte.
pub const SIGNAL_PREFIX: &str = "dvb:";

/// Pakete aus dem Demux sammeln, bis `done` erfüllt oder die Zeit um ist.
fn collect(
    dmx: &mut Demux,
    limit: Duration,
    wanted: &dyn Fn(u16) -> bool,
    done: &dyn Fn(&crate::ts::SectionCollector) -> bool,
) -> crate::ts::SectionCollector {
    let mut col = crate::ts::SectionCollector::default();
    let mut buf = vec![0u8; 188 * 512];
    let mut rest: Vec<u8> = Vec::new();
    let t = Instant::now();
    let mut checked = Instant::now();
    while t.elapsed() < limit {
        if let Ok(n) = dmx.read(&mut buf, Duration::from_millis(200)) {
            rest.extend_from_slice(&buf[..n]);
            let whole = rest.len() / 188 * 188;
            for p in rest[..whole].chunks(188) {
                col.push_packet(p, wanted);
            }
            rest.drain(..whole);
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

/// Einen Kanal absuchen: `Ok(None)` = kein Empfang.
fn scan_tuning(dev: &Device, fe: &Frontend, t: &Tuning) -> Result<Option<Vec<DvbService>>, String> {
    if !fe.tune(t, Duration::from_millis(2500))? {
        return Ok(None);
    }
    let mut dmx = Demux::open(dev.adapter, dev.frontend, &[])?;
    let col = collect(&mut dmx, Duration::from_secs(3), &|pid| pid == 0, &|c| {
        c.tables(0x00).next().is_some()
    });
    let pat: Vec<(u16, u16)> = col.tables(0x00).flat_map(crate::ts::parse_pat).collect();
    if pat.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let pmt_pids: Vec<u16> = pat.iter().map(|(_, p)| *p).collect();
    let mut more = collect(
        &mut dmx,
        Duration::from_secs(8),
        &|pid| pid == 0 || pid == 17 || pmt_pids.contains(&pid),
        &|c| c.tables(0x42).next().is_some() && c.tables(0x02).count() >= pmt_pids.len(),
    );
    more.sections.extend(col.sections);
    Ok(Some(
        crate::satip::found_services(&more, &pat)
            .into_iter()
            .map(|f| DvbService {
                name: f.name,
                provider: f.provider,
                onid: f.onid,
                tsid: f.tsid,
                service_id: f.service_id,
                hd: f.hd,
                radio: f.radio,
                scrambled: f.scrambled,
                tuning: t.clone(),
                pids: f.pids,
            })
            .collect(),
    ))
}

/// Fortschritt des Suchlaufs: erledigt, gesamt, gefundene Sender.
pub type Progress = (usize, usize, usize);

/// Suchlauf über alle Kanäle eines Plans (nur die Empfangsarten, die das Gerät kann).
pub fn scan(
    dev: &Device,
    tunings: &[Tuning],
    progress: &dyn Fn(Progress),
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<DvbService>, String> {
    let fe = Frontend::open(dev.adapter, dev.frontend)?;
    let list: Vec<&Tuning> = tunings
        .iter()
        .filter(|t| dev.systems.contains(&t.system))
        .collect();
    if list.is_empty() {
        return Err(tr("Das Gerät kann diese Empfangsart nicht").into());
    }
    let mut out: Vec<DvbService> = Vec::new();
    for (i, t) in list.iter().enumerate() {
        if cancel() {
            break;
        }
        // DVB-T2 nach DVB-T: auf demselben Kanal kann beides senden.
        if let Some(found) = scan_tuning(dev, &fe, t)? {
            out.extend(found);
        }
        progress((i + 1, list.len(), out.len()));
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.key() == b.key());
    Ok(out)
}

/// Transponder eines Satelliten aus der Netzwerktabelle (NIT) seines Starttransponders.
fn satellite_tunings(dev: &Device, fe: &Frontend, sat: &crate::adapter::Satellite) -> Result<Vec<Tuning>, String> {
    let seed = &sat.seed;
    let mut t = Tuning::new(
        if seed.dvbs2 { System::DvbS2 } else { System::DvbS },
        seed.freq * 1000,
    );
    t.symbol_rate = seed.sr * 1000;
    t.pol = seed.pol;
    t.modulation = seed.mtype.to_string();
    if !fe.tune(&t, Duration::from_secs(3))? {
        return Err(tr("kein Empfang auf dem Starttransponder (Antenne, LNB, Kabel?)").into());
    }
    let mut dmx = Demux::open(dev.adapter, dev.frontend, &[16])?;
    let col = collect(&mut dmx, Duration::from_secs(15), &|pid| pid == 16, &|c| {
        let mut it = c.tables(0x40).peekable();
        let Some(first) = it.peek() else { return false };
        let last = first[7];
        c.tables(0x40).count() > last as usize
    });
    let mut out: Vec<Tuning> = Vec::new();
    for sec in col.tables(0x40).chain(col.tables(0x41)) {
        for d in crate::ts::parse_nit(sec) {
            if d.orbital != sat.orbital || d.east != sat.east || d.freq == 0 {
                continue;
            }
            let mut t = Tuning::new(if d.dvbs2 { System::DvbS2 } else { System::DvbS }, d.freq * 1000);
            t.symbol_rate = d.sr * 1000;
            t.pol = d.pol;
            t.modulation = d.mtype.to_string();
            if !out.iter().any(|o| o.freq == t.freq && o.pol == t.pol) {
                out.push(t);
            }
        }
    }
    if out.is_empty() {
        out.push(t);
    }
    Ok(out)
}

/// Suchlauf über einen Satelliten: Transponder aus der Netzwerktabelle, dann jeden absuchen.
pub fn scan_satellite(
    dev: &Device,
    sat: &crate::adapter::Satellite,
    progress: &dyn Fn(Progress),
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<DvbService>, String> {
    let tunings = {
        let fe = Frontend::open(dev.adapter, dev.frontend)?;
        satellite_tunings(dev, &fe, sat)?
    };
    scan(dev, &tunings, progress, cancel)
}

/// Gefundene Sender für die Senderliste (`radio`: Radiosender übernehmen; verschlüsselte
/// und Platzhalter ohne Buchstaben bleiben weg).
pub fn to_channels(dev: &str, services: &[DvbService], radio: bool) -> Vec<crate::ScannedChannel> {
    services
        .iter()
        .filter(|s| !s.scrambled && (radio || !s.radio))
        .filter(|s| s.name.chars().any(char::is_alphabetic))
        .map(|s| crate::ScannedChannel {
            key: s.key(),
            name: s.name.clone(),
            group: (!s.provider.is_empty()).then(|| s.provider.clone()),
            stream: crate::Stream {
                quality: Some(if s.hd {
                    crate::Quality::P720
                } else {
                    crate::Quality::Sd
                }),
                kind: Some(crate::SourceKind::Dvb),
                url: s.url(dev),
                max_bitrate: None,
            },
            radio: s.radio,
            // Neue Sender: Programm aus dem Signal (Nutzer, 2026-10-27).
            epg_id: Some(s.key()),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Wiedergabe und Aufnahme

/// Laufende Wiedergabe eines DVB-Senders: Frontend abstimmen, Demux auf die PIDs des Senders
/// (dazu PID 18 für die Programmdaten), Transportstrom unter `local_url` (nur 127.0.0.1) an
/// den Player. Beim Fallenlassen endet der Strom, das Frontend ist wieder frei.
pub struct Relay {
    pub local_url: String,
    signal: std::sync::Arc<std::sync::Mutex<Option<Signal>>>,
    bytes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    eit: std::sync::Arc<std::sync::Mutex<Vec<crate::ts::EitSection>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Relay {
    pub fn start(url: &str) -> Result<Self, String> {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        let (a, f, t, mut pids) = split_url(url).ok_or(tr("keine DVB-Adresse"))?;
        if !pids.is_empty() && !pids.contains(&18) {
            pids.push(18);
        }
        // Abstimmen vorab, damit Fehler (belegt, kein Empfang) sofort gemeldet werden.
        let fe = Frontend::open(a, f)?;
        if !fe.tune(&t, Duration::from_secs(3))? {
            return Err(tr("kein Empfang auf diesem Kanal").into());
        }
        let mut dmx = Demux::open(a, f, &pids)?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let signal = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(AtomicUsize::new(0));
        let eit: Arc<Mutex<Vec<crate::ts::EitSection>>> = Arc::default();
        let (sg, sp, by, ei) = (signal.clone(), stop.clone(), bytes.clone(), eit.clone());
        let thread = std::thread::Builder::new().name("dvb-weitergabe".into()).spawn(move || {
            let stopped = || sp.load(Ordering::Relaxed);
            let mut buf = vec![0u8; 188 * 512];
            let mut col = crate::ts::SectionCollector::default();
            let mut next_status = Instant::now();
            let mut next_eit = Instant::now() + Duration::from_secs(20);
            let mut wait = Duration::from_secs(15);
            'clients: while !stopped() {
                let t0 = Instant::now();
                let mut client = loop {
                    if stopped() || t0.elapsed() > wait {
                        crate::diag::log("dvb-weitergabe", if stopped() { "Ende (gestoppt) beim Warten auf den Player" } else { "Ende: kein Player verbunden" });
                        break 'clients;
                    }
                    match listener.accept() {
                        Ok((c, _)) => break c,
                        Err(_) => {
                            // Daten verwerfen, solange niemand liest.
                            let _ = dmx.read(&mut buf, Duration::from_millis(50));
                        }
                    }
                };
                wait = Duration::from_secs(3);
                crate::diag::log("dvb-weitergabe", &format!("Player verbunden: {:?}", client.peer_addr()));
                client.set_nonblocking(false).ok();
                client.set_read_timeout(Some(Duration::from_secs(2))).ok();
                client.set_write_timeout(Some(Duration::from_secs(5))).ok();
                let mut req = Vec::new();
                let mut b = [0u8; 1024];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") && req.len() < 8192 {
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
                let mut rest: Vec<u8> = Vec::new();
                while !stopped() {
                    match dmx.read(&mut buf, Duration::from_millis(200)) {
                        Ok(n) if n > 0 => {
                            by.fetch_add(n, Ordering::Relaxed);
                            rest.extend_from_slice(&buf[..n]);
                            let whole = rest.len() / 188 * 188;
                            for p in rest[..whole].chunks(188) {
                                col.push_packet(p, &|pid| pid == 18);
                            }
                            if let Err(e) = client.write_all(&rest[..whole]) {
                                crate::diag::log("dvb-weitergabe", &format!("Player getrennt: {e}"));
                                continue 'clients;
                            }
                            rest.drain(..whole);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            crate::diag::log("dvb-weitergabe", &format!("Lesefehler Demux: {e}"));
                            break 'clients;
                        }
                    }
                    if Instant::now() >= next_status {
                        next_status = Instant::now() + Duration::from_secs(2);
                        *sg.lock().unwrap_or_else(|e| e.into_inner()) = Some(fe.signal());
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
            crate::diag::log("dvb-weitergabe", "Schleife beendet, Thread endet");
            drop(dmx);
            drop(fe);
        });
        Ok(Self {
            local_url: format!("http://127.0.0.1:{port}/dvb.ts"),
            signal,
            bytes,
            eit,
            stop,
            thread: thread.ok(),
        })
    }

    /// Seit dem letzten Abholen mitgelesene Programmdaten.
    pub fn take_eit(&self) -> Vec<crate::ts::EitSection> {
        std::mem::take(&mut *self.eit.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Bisher gelesene Bytes.
    pub fn bytes(&self) -> usize {
        self.bytes.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Letzter Empfang des Frontends.
    pub fn signal(&self) -> Option<Signal> {
        *self.signal.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Beenden und auf die Freigabe des Frontends warten.
    pub fn close(mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        crate::diag::log("dvb-weitergabe", &format!("Stopp angefordert ({})", self.local_url));
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Aufnahme eines DVB-Senders in eine Datei (`<datei>.part`, beim Beenden umbenannt): eigenes
/// Frontend; Lesen in einem Thread, Schreiben in einem zweiten über eine Warteschlange im
/// Arbeitsspeicher bis `queue_bytes`.
pub struct Recorder {
    pub path: PathBuf,
    bytes: std::sync::Arc<std::sync::atomic::AtomicU64>,
    dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,
    error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Recorder {
    pub fn start(url: &str, path: &Path, queue_bytes: u64) -> Result<Self, String> {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::{Arc, Mutex};
        let (a, f, t, pids) = split_url(url).ok_or(tr("keine DVB-Adresse"))?;
        let fe = Frontend::open(a, f)?;
        if !fe.tune(&t, Duration::from_secs(3))? {
            return Err(tr("kein Empfang auf diesem Kanal").into());
        }
        let mut dmx = Demux::open(a, f, &pids)?;
        let part = crate::ablage::part_path(path);
        let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let bytes = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let error: Arc<Mutex<Option<String>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let chunks = (queue_bytes / (188 * 512) as u64).max(16) as usize;
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(chunks);
        let (sp, dr, er) = (stop.clone(), dropped.clone(), error.clone());
        let reader = std::thread::spawn(move || {
            let mut buf = vec![0u8; 188 * 512];
            while !sp.load(Ordering::Relaxed) {
                match dmx.read(&mut buf, Duration::from_millis(200)) {
                    Ok(n) if n > 0 => {
                        if tx.try_send(buf[..n].to_vec()).is_err() {
                            dr.fetch_add(n as u64, Ordering::Relaxed);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        *er.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                        break;
                    }
                }
            }
            drop(dmx);
            drop(fe);
        });
        let (by, er) = (bytes.clone(), error.clone());
        let writer = std::thread::spawn(move || {
            for chunk in rx {
                if let Err(e) = file.write_all(&chunk) {
                    *er.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.to_string());
                    break;
                }
                by.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
            let _ = file.flush();
        });
        Ok(Self {
            path: path.to_path_buf(),
            bytes,
            dropped,
            error,
            stop,
            threads: vec![reader, writer],
        })
    }

    pub fn bytes(&self) -> u64 {
        self.bytes.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Beenden, Datei schließen und umbenennen.
    pub fn finish(mut self) -> Result<PathBuf, String> {
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

/// Programmdaten (EIT, PID 18) eines Kanals sammeln (`secs` Sekunden).
pub fn collect_eit(dev: &str, query: &str, secs: u64) -> Result<Vec<crate::ts::EitSection>, String> {
    let (a, f) = dev
        .split_once('.')
        .and_then(|(a, f)| Some((a.parse().ok()?, f.parse().ok()?)))
        .ok_or(tr("keine DVB-Adresse"))?;
    let t = Tuning::parse(query).ok_or(tr("keine DVB-Adresse"))?;
    let fe = Frontend::open(a, f)?;
    if !fe.tune(&t, Duration::from_secs(3))? {
        return Err(tr("kein Empfang auf diesem Kanal").into());
    }
    let mut dmx = Demux::open(a, f, &[18])?;
    let col = collect(&mut dmx, Duration::from_secs(secs), &|pid| pid == 18, &|_| false);
    if col.sections.is_empty() {
        return Err(tr("keine Programmdaten empfangen").into());
    }
    Ok(col
        .sections
        .iter()
        .filter_map(|(_, sec)| crate::ts::parse_eit(sec))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_structs_have_kernel_sizes() {
        assert_eq!(std::mem::size_of::<DtvProperty>(), 76);
        assert_eq!(std::mem::size_of::<DtvProperties>(), 16);
        assert_eq!(std::mem::size_of::<DmxPesFilter>(), 20);
        // Werte wie in linux/dvb/frontend.h bzw. dmx.h.
        assert_eq!(FE_GET_INFO, 0x80a8_6f3d);
        assert_eq!(FE_SET_PROPERTY, 0x4010_6f52);
        assert_eq!(FE_GET_PROPERTY, 0x8010_6f53);
        assert_eq!(DMX_SET_PES_FILTER, 0x4014_6f2c);
    }

    #[test]
    fn urls_round_trip() {
        let mut t = Tuning::new(System::DvbT2, 474_000_000);
        t.plp = Some(0);
        let s = DvbService {
            name: "Das Erste HD".into(),
            provider: String::new(),
            onid: 0x2114,
            tsid: 0x3301,
            service_id: 0x2a81,
            hd: true,
            radio: false,
            scrambled: false,
            tuning: t.clone(),
            pids: vec![0, 256, 257],
        };
        let url = s.url("0.0");
        assert_eq!(url, "dvb://0.0/?sys=dvbt2&freq=474000000&bw=8000000&plp=0&pids=0,256,257");
        let (a, f, back, pids) = split_url(&url).unwrap();
        assert_eq!((a, f, back, pids), (0, 0, t, vec![0, 256, 257]));
        assert_eq!(tuning_of(&url).unwrap().1, "sys=dvbt2&freq=474000000&bw=8000000&plp=0");
        assert_eq!(s.key(), "dvb:8468:13057:10881");
    }

    #[test]
    fn plans_parse() {
        let p = plans();
        let t = p.iter().find(|p| p.id == "dvbt-de").unwrap();
        // UHF-Kanäle 21–48.
        assert_eq!(t.tunings.first().unwrap().freq, 474_000_000);
        assert_eq!(t.tunings.len(), 28);
        assert!(p.iter().any(|p| p.id == "dvbc-de"));
    }

    /// Mit geladenem virtuellen Gerät (`dvb_vidtv`): `cargo test -p tvjoe-core dvb -- --ignored`.
    #[test]
    #[ignore]
    fn virtuelles_geraet() {
        let devs = devices();
        let dev = devs.first().expect("kein /dev/dvb");
        println!("{dev:?}");
        let t = Tuning::new(System::DvbT, 474_000_000);
        let found = scan(dev, &[t], &|p| println!("{p:?}"), &|| false).unwrap();
        println!("{found:#?}");
        assert!(!found.is_empty());
        let url = found[0].url(&dev.id());
        let eit = collect_eit(&dev.id(), &tuning_of(&url).unwrap().1, 5).unwrap_or_default();
        println!("EIT: {} Abschnitte", eit.len());
        // Aufnahme: fünf Sekunden in eine Datei.
        let path = std::env::temp_dir().join(format!("tvjoe-dvb-{}.ts", std::process::id()));
        let rec = Recorder::start(&url, &path, 1 << 20).unwrap();
        std::thread::sleep(Duration::from_secs(5));
        assert!(rec.error().is_none(), "{:?}", rec.error());
        let out = rec.finish().unwrap();
        let size = std::fs::metadata(&out).unwrap().len();
        println!("Aufnahme: {size} Bytes");
        assert!(size > 188 * 100 && size % 188 == 0);
        let _ = std::fs::remove_file(out);
    }
}
