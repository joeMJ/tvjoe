//! Tests mit synthetischen Dateien aus `tests/fixtures/` (keine echten Sender-URLs im Repo).

use std::path::PathBuf;

use tvjoe_core::{FindingKind, Playlist, Quality, Severity, SourceKind};

fn fixture(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn kinds(p: &Playlist, name: &str) -> Vec<FindingKind> {
    let i = p
        .entries
        .iter()
        .position(|e| e.name == name)
        .unwrap_or_else(|| panic!("Eintrag {name}"));
    p.findings
        .iter()
        .filter(|f| f.entry == Some(i))
        .map(|f| f.kind.clone())
        .collect()
}

#[test]
fn ok_datei_ohne_befunde() {
    let p = Playlist::parse(&fixture("ok.m3u")).unwrap();
    assert_eq!(p.entries.len(), 5);
    assert!(p.findings.is_empty(), "{:?}", p.findings);
    let q: Vec<_> = p
        .entries
        .iter()
        .take(4)
        .map(|e| e.quality().unwrap())
        .collect();
    assert_eq!(
        q,
        [Quality::Sd, Quality::P720, Quality::P1080, Quality::Uhd]
    );
    assert_eq!(p.entries[0].kind(), Some(SourceKind::Hls));
    assert_eq!(
        p.entries[0].tvg_logo(),
        Some("https://example.invalid/logos/eins,a.png")
    );
    assert_eq!(p.entries[0].group(), Some("ÖR"));
    assert_eq!(p.entries[0].chno(), Some(1));
}

#[test]
fn warnungen_und_fehler() {
    let p = Playlist::parse(&fixture("warnungen.m3u")).unwrap();

    assert!(kinds(&p, "Eins SD").is_empty());

    assert_eq!(
        kinds(&p, "Ohne Alles"),
        [
            FindingKind::MissingTvgId,
            FindingKind::MissingLogo,
            FindingKind::MissingGroup,
            FindingKind::Http,
            FindingKind::UnknownType,
            FindingKind::MissingQuality,
        ]
    );

    assert_eq!(
        kinds(&p, "Drei Schema und Qualität"),
        [
            FindingKind::UnknownScheme("ftp".into()),
            FindingKind::UnknownQuality("hd".into())
        ]
    );

    assert_eq!(
        kinds(&p, "Eins SD doppelt"),
        [FindingKind::DuplicateQuality(Quality::Sd)]
    );
    assert_eq!(kinds(&p, "Eins URL doppelt"), [FindingKind::DuplicateUrl]);

    // Gleiche tvg-id ohne unterscheidende Qualität: nur der zweite Eintrag wird bemängelt.
    assert_eq!(
        kinds(&p, "Vier A")
            .iter()
            .filter(|k| **k == FindingKind::DuplicateTvgId)
            .count(),
        0
    );
    assert!(kinds(&p, "Vier B").contains(&FindingKind::DuplicateTvgId));

    // satip wird erkannt, ohne Befund.
    let fuenf = p.entries.iter().find(|e| e.name == "Fünf").unwrap();
    assert_eq!(fuenf.kind(), Some(SourceKind::SatIp));
    assert!(kinds(&p, "Fünf").is_empty());
}

#[test]
fn doppelte_kanalnummer_wird_neu_vergeben() {
    let p = Playlist::parse(&fixture("warnungen.m3u")).unwrap();
    // „Vier A“ wollte 1 (gehört „Eins“), höchste vorhandene Nummer ist 5: neu 6.
    let vier = p.entries.iter().find(|e| e.name == "Vier A").unwrap();
    assert_eq!(vier.chno(), Some(6));
    assert!(kinds(&p, "Vier A").contains(&FindingKind::DuplicateChno { old: 1, new: 6 }));
    // Gleicher Sender in mehreren Qualitäten darf dieselbe Nummer behalten.
    let ok = Playlist::parse(&fixture("ok.m3u")).unwrap();
    assert!(ok.entries.iter().take(4).all(|e| e.chno() == Some(1)));
}

#[test]
fn dateiebene_befunde() {
    let p = Playlist::parse(&fixture("warnungen.m3u")).unwrap();
    let file_level: Vec<_> = p
        .findings
        .iter()
        .filter(|f| f.entry.is_none())
        .map(|f| f.kind.clone())
        .collect();
    assert_eq!(
        file_level,
        [
            FindingKind::ExtinfWithoutUrl,
            FindingKind::UrlWithoutExtinf,
            FindingKind::ExtinfWithoutUrl
        ]
    );
    let first = p.findings.iter().find(|f| f.entry.is_none()).unwrap();
    assert_eq!(first.kind.severity(), Severity::Error);
    // Nach Zeilennummer sortiert.
    assert!(p.findings.windows(2).all(|w| w[0].line <= w[1].line));
}

/// Optional: echte Liste prüfen. `TVJOE_TEST_M3U=/pfad/zur/liste.m3u cargo test -- --nocapture`.
#[test]
fn echte_liste_optional() {
    let Ok(path) = std::env::var("TVJOE_TEST_M3U") else {
        return;
    };
    let p = Playlist::parse(&std::fs::read(&path).unwrap()).unwrap();
    let errors = p
        .findings
        .iter()
        .filter(|f| f.kind.severity() == Severity::Error)
        .count();
    println!(
        "{path}: {} Einträge, {} Befunde, davon {errors} Fehler",
        p.entries.len(),
        p.findings.len()
    );
    for f in &p.findings {
        println!("  Zeile {}: {}", f.line, f.kind);
    }
}
