//! Sicherheitsprüfung mit schädlichen Attrappen aus `tests/fixtures/attrappen/` (synthetisch,
//! alle Adressen erfunden oder lokal).

use std::path::PathBuf;

use tvjoe_core::epg::parse_xmltv;
use tvjoe_core::pruefung::HostClass;
use tvjoe_core::{FindingKind, Playlist, Severity};

fn fixture(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/attrappen")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn kinds(p: &Playlist, url_part: &str) -> Vec<FindingKind> {
    let i = p
        .entries
        .iter()
        .position(|e| e.url.contains(url_part))
        .unwrap_or_else(|| panic!("Eintrag mit {url_part}"));
    p.findings
        .iter()
        .filter(|f| f.entry == Some(i))
        .map(|f| f.kind.clone())
        .collect()
}

fn rejected(p: &Playlist, url_part: &str) -> bool {
    let i = p.entries.iter().position(|e| e.url.contains(url_part)).unwrap();
    p.has_errors_for(i)
}

#[test]
fn m3u_schemata_werden_abgelehnt() {
    let p = Playlist::parse(&fixture("schaedlich.m3u")).unwrap();
    assert!(!rejected(&p, "stream.example/gut"));
    for u in [
        "file:///etc/passwd",
        "av://lavfi",
        "edl://",
        "memory://",
        "javascript://",
        "data:video",
        "/home/nutzer/.ssh",
        "C:\\Windows",
        "\u{1b}[2J",
    ] {
        assert!(rejected(&p, u), "nicht abgelehnt: {u}");
    }
    assert!(kinds(&p, "\u{1b}[2J").contains(&FindingKind::UnsafeUrl));
    // Abgelehnte Einträge ohne zusätzliche Adresswarnung.
    for u in ["av://lavfi", "file:///etc/passwd", "memory://"] {
        assert!(!kinds(&p, u).iter().any(|k| matches!(k, FindingKind::LocalAddress(_))), "{u}");
    }
}

#[test]
fn m3u_lokale_adressen_und_zugangsdaten_nur_warnung() {
    let p = Playlist::parse(&fixture("schaedlich.m3u")).unwrap();
    for (u, class) in [
        ("127.0.0.1:631", HostClass::Loopback),
        ("2130706433", HostClass::Loopback),
        ("169.254.169.254", HostClass::LinkLocal),
        ("10.0.0.254", HostClass::Private),
    ] {
        assert!(!rejected(&p, u), "{u}");
        assert!(kinds(&p, u).contains(&FindingKind::LocalAddress(class)), "{u}");
    }
    let k = kinds(&p, "nutzer:geheim@");
    assert!(k.contains(&FindingKind::Credentials));
    assert!(k.iter().all(|f| f.severity() == Severity::Warning));
    assert!(FindingKind::Credentials.to_string().contains("rechtlich selbst verantwortlich"));
    // Heimnetz-Adressen sind nur ein Hinweis (Eintrag bleibt beim Import angewählt),
    // Zugangsdaten nicht.
    assert!(FindingKind::LocalAddress(HostClass::Private).is_notice());
    assert!(FindingKind::LocalLogo.is_notice());
    assert!(!FindingKind::Credentials.is_notice());
    assert!(!FindingKind::Http.is_notice());
}

#[test]
fn m3u_texte_und_logos_gesaeubert() {
    let p = Playlist::parse(&fixture("schaedlich.m3u")).unwrap();
    for u in ["ansi.m3u8", "bidi.m3u8", "lang.m3u8"] {
        assert!(kinds(&p, u).contains(&FindingKind::TextCleaned), "{u}");
    }
    let name = |u: &str| p.entries.iter().find(|e| e.url.contains(u)).unwrap().name.clone();
    assert_eq!(name("ansi.m3u8"), "Ansi[31mRot");
    assert_eq!(name("bidi.m3u8"), "Sportgpj.exe");
    assert_eq!(name("lang.m3u8").chars().count(), 201);

    let logo = |u: &str| {
        p.entries
            .iter()
            .find(|e| e.url.contains(u))
            .unwrap()
            .tvg_logo()
            .map(str::to_string)
    };
    assert!(kinds(&p, "l1.m3u8").contains(&FindingKind::LogoRemoved));
    assert_eq!(logo("l1.m3u8"), None);
    assert!(kinds(&p, "l2.m3u8").contains(&FindingKind::LogoRemoved));
    assert_eq!(logo("l2.m3u8"), None);
    assert!(kinds(&p, "l3.m3u8").contains(&FindingKind::LocalLogo));
    assert_eq!(logo("l3.m3u8").as_deref(), Some("http://10.0.0.5/logo.png"));
    // #EXTVLCOPT/#KODIPROP bleiben ohne Wirkung.
    assert!(p.entries.iter().find(|e| e.url.contains("opt.m3u8")).unwrap().attrs.iter().all(|(k, _)| !k.contains("license")));
}

#[test]
fn xmltv_entities_abgelehnt() {
    for f in ["billion-laughs.xml", "xxe.xml"] {
        let e = parse_xmltv(&fixture(f)).unwrap_err();
        assert!(e.contains("ENTITY"), "{f}: {e}");
    }
}

#[test]
fn xmltv_gepackte_bombe_abgelehnt() {
    let e = parse_xmltv(&fixture("bombe.xml.gz")).unwrap_err();
    assert!(e.contains("abgelehnt"), "{e}");
}

#[test]
fn xmltv_markup_bleibt_text_und_wird_gesaeubert() {
    let g = parse_xmltv(&fixture("markup.xml")).unwrap();
    let list = &g.programmes["A.test"];
    // Markup bleibt wörtlicher Text; die Oberfläche zeigt es nur als Text an.
    assert_eq!(list[0].title, "<span foreground=\"red\" size=\"xx-large\">Rot</span>");
    let desc = list[0].desc.as_deref().unwrap();
    assert!(!desc.contains('\u{1b}') && !desc.contains('\u{202e}'), "{desc:?}");
    assert_eq!(list[1].desc.as_deref().unwrap().chars().count(), 16_001);
    assert_eq!(g.cleaned, 2, "{:?}", g.warnings);
    assert!(g.warnings.is_empty(), "{:?}", g.warnings);
}

#[test]
fn xmltv_unmoegliche_zeiten() {
    let g = parse_xmltv(&fixture("zeiten.xml")).unwrap();
    let titles: Vec<&str> = g.programmes["A.test"].iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, ["Normal", "Achtundvierzig"]);
    assert!(g.warnings.iter().any(|w| w.contains("3 Sendungen")), "{:?}", g.warnings);
    assert!(g.warnings.iter().any(|w| w.contains("24 Stunden")), "{:?}", g.warnings);
}

#[test]
fn xmltv_logos() {
    let g = parse_xmltv(&fixture("logos.xml")).unwrap();
    let mut kept: Vec<&str> = g.icons.values().map(String::as_str).collect();
    kept.sort();
    assert_eq!(kept, ["http://10.0.0.5/lan.png", "https://logos.example/ok.png"]);
    assert!(g.warnings.iter().any(|w| w.contains("4 Logo")), "{:?}", g.warnings);
}

/// Probe gegen echte Quellen: `TVJOE_PROBE=pfad1,pfad2 cargo test -p tvjoe-core --test attrappen
/// -- --ignored --nocapture` (M3U an der Endung `.m3u`/`.m3u8` erkannt, sonst XMLTV).
#[test]
#[ignore]
fn echte_quellen() {
    let paths = std::env::var("TVJOE_PROBE").expect("TVJOE_PROBE");
    for path in paths.split(',') {
        let bytes = std::fs::read(path).unwrap();
        println!("== {path}");
        if path.ends_with(".m3u") || path.ends_with(".m3u8") {
            let p = Playlist::parse(&bytes).unwrap();
            let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
            for f in &p.findings {
                let key = match &f.kind {
                    FindingKind::UnknownScheme(_) => "UnknownScheme".to_string(),
                    k => format!("{k:?}"),
                };
                *counts.entry(key).or_default() += 1;
            }
            let errors = (0..p.entries.len()).filter(|&i| p.has_errors_for(i)).count();
            println!("Einträge {}, abgelehnt {errors}", p.entries.len());
            for (k, n) in counts {
                println!("  {k}: {n}");
            }
            for f in p.findings.iter().filter(|f| f.kind.severity() == Severity::Error) {
                println!("  Zeile {}: {}", f.line, f.kind);
            }
        } else {
            match parse_xmltv(&bytes) {
                Ok(g) => {
                    println!("Kanäle {}, Sendungen {}", g.channels.len(), g.programme_count());
                    println!("  gesäubert: {}", g.cleaned);
                    for w in &g.warnings {
                        println!("  {w}");
                    }
                }
                Err(e) => println!("  abgelehnt: {e}"),
            }
        }
    }
}
