//! Abstimmung von Kanälen und Transpondern (DVB-T/T2, DVB-C, DVB-S/S2) und Frequenzpläne für
//! den Suchlauf, gemeinsam für DVB-Karten und SAT>IP-Geräte mit Antenne oder Kabel.

use std::collections::HashMap;

/// Empfangsart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum System {
    DvbT,
    DvbT2,
    DvbC,
    DvbS,
    DvbS2,
}

impl System {
    pub fn code(self) -> &'static str {
        match self {
            System::DvbT => "dvbt",
            System::DvbT2 => "dvbt2",
            System::DvbC => "dvbc",
            System::DvbS => "dvbs",
            System::DvbS2 => "dvbs2",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "dvbt" => System::DvbT,
            "dvbt2" => System::DvbT2,
            "dvbc" => System::DvbC,
            "dvbs" => System::DvbS,
            "dvbs2" => System::DvbS2,
            _ => return None,
        })
    }

    /// Anzeigename.
    pub fn label(self) -> &'static str {
        match self {
            System::DvbT => "DVB-T",
            System::DvbT2 => "DVB-T2",
            System::DvbC => "DVB-C",
            System::DvbS => "DVB-S",
            System::DvbS2 => "DVB-S2",
        }
    }

    pub fn satellite(self) -> bool {
        matches!(self, System::DvbS | System::DvbS2)
    }
}

/// Abstimmung eines Transponders bzw. Kanals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tuning {
    pub system: System,
    /// DVB-T/C: Hz; Satellit: kHz im Ku-Band (z. B. 11494000).
    pub freq: u32,
    /// DVB-T: Kanalbreite in Hz (meist 8 MHz).
    pub bandwidth: u32,
    /// DVB-C/S: Symbole je Sekunde.
    pub symbol_rate: u32,
    /// DVB-C: `qam64`, `qam256` …; DVB-S2: `8psk`; leer = automatisch.
    pub modulation: String,
    /// Satellit: `h` oder `v`.
    pub pol: char,
    /// DVB-T2: PLP (Datenstrom); `None` = automatisch.
    pub plp: Option<u32>,
}

impl Tuning {
    pub fn new(system: System, freq: u32) -> Self {
        Self {
            system,
            freq,
            bandwidth: 8_000_000,
            symbol_rate: 0,
            modulation: String::new(),
            pol: 'h',
            plp: None,
        }
    }

    /// Teil der Senderadresse (ohne `pids`).
    pub fn query(&self) -> String {
        let mut q = format!("sys={}&freq={}", self.system.code(), self.freq);
        match self.system {
            System::DvbT | System::DvbT2 => q += &format!("&bw={}", self.bandwidth),
            System::DvbC => q += &format!("&sr={}", self.symbol_rate),
            System::DvbS | System::DvbS2 => q += &format!("&sr={}&pol={}", self.symbol_rate, self.pol),
        }
        if !self.modulation.is_empty() {
            q += &format!("&mod={}", self.modulation);
        }
        if let Some(p) = self.plp {
            q += &format!("&plp={p}");
        }
        q
    }

    /// Aus dem Teil der Senderadresse lesen.
    pub fn parse(query: &str) -> Option<Self> {
        let kv: HashMap<&str, &str> = query.split('&').filter_map(|p| p.split_once('=')).collect();
        let mut t = Tuning::new(System::from_code(kv.get("sys")?)?, kv.get("freq")?.parse().ok()?);
        if let Some(v) = kv.get("bw").and_then(|v| v.parse().ok()) {
            t.bandwidth = v;
        }
        if let Some(v) = kv.get("sr").and_then(|v| v.parse().ok()) {
            t.symbol_rate = v;
        }
        if let Some(v) = kv.get("mod") {
            t.modulation = v.to_string();
        }
        if let Some(v) = kv.get("pol").and_then(|v| v.chars().next()) {
            t.pol = v;
        }
        t.plp = kv.get("plp").and_then(|v| v.parse().ok());
        Some(t)
    }

    /// Lesbare Angabe (Listen, Info).
    pub fn label(&self) -> String {
        if self.system.satellite() {
            format!(
                "{} {} MHz {}",
                self.system.label(),
                self.freq / 1000,
                self.pol.to_ascii_uppercase()
            )
        } else {
            format!("{} {:.1} MHz", self.system.label(), f64::from(self.freq) / 1e6)
        }
    }
}

/// Frequenzpläne für den Suchlauf (eingebaut: `data/dvb/*.toml`).
const PLANS: &[(&str, &str)] = &[
    ("dvbt-de", include_str!("../../../data/dvb/dvbt-de.toml")),
    ("dvbc-de", include_str!("../../../data/dvb/dvbc-de.toml")),
];

/// Ein Frequenzplan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub id: String,
    pub name: String,
    pub tunings: Vec<Tuning>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlan {
    name: String,
    system: String,
    #[serde(default)]
    bandwidth: u32,
    #[serde(default)]
    symbol_rate: u32,
    #[serde(default)]
    modulation: String,
    /// Frequenzen in kHz.
    frequencies: Vec<u32>,
}

/// Mitgelieferte Frequenzpläne.
pub fn plans() -> Vec<Plan> {
    PLANS
        .iter()
        .filter_map(|(id, text)| {
            let raw: RawPlan = toml::from_str(text).ok()?;
            let system = System::from_code(&raw.system)?;
            let tunings = raw
                .frequencies
                .iter()
                .map(|khz| {
                    let mut t = Tuning::new(system, khz * 1000);
                    if raw.bandwidth > 0 {
                        t.bandwidth = raw.bandwidth;
                    }
                    t.symbol_rate = raw.symbol_rate;
                    t.modulation = raw.modulation.clone();
                    t
                })
                .collect();
            Some(Plan {
                id: id.to_string(),
                name: raw.name,
                tunings,
            })
        })
        .collect()
}
