//! Starter `tvjoe`: prüft die Systembibliotheken (ohne GTK zu laden), bietet die Installation an
//! und startet dann die Oberfläche `tvjoe-gui`. Außerdem Kommandozeilen-Import einer M3U.

use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use tvjoe_core::epg::{self, MatchKind};
use tvjoe_core::{Library, Playlist, Settings, Severity, VERSION, paths, syscheck};

fn main() -> ExitCode {
    let t0 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("tvjoe {VERSION}");
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("--check") => check(args.iter().any(|a| a == "--dev")),
        Some("--import") => import(&args[1..]),
        Some("--epg-add") => epg_add(&args[1..]),
        Some("--epg-check") => epg_check(&args[1..]),
        Some("--update-check") => update_check(),
        _ => start_gui(t0, &args),
    }
}

fn print_help() {
    println!(
        "tvjoe {VERSION}

Aufruf:
  tvjoe [--channel NUMMER] [--mute] [--quit-after SEKUNDEN]
  tvjoe --import DATEI.m3u [--name NAME] [--mit-warnungen]
  tvjoe --check [--dev]
  tvjoe --epg-add QUELLE          XMLTV-Datei oder -URL in die Einstellungen aufnehmen
  tvjoe --epg-check [QUELLE …]    Zuordnung der Sender zum EPG prüfen
  tvjoe --update-check            nach neuer Version auf GitHub fragen
  tvjoe --version

--import übernimmt Einträge ohne Befund; mit --mit-warnungen auch Einträge mit Warnungen.
Umgebung: TVJOE_CONFIG_DIR, TVJOE_CACHE_DIR (andere Ablage), TVJOE_TRACE=1 (Zeitmessung)."
    );
}

fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Zeigt den Installationsbefehl und führt ihn nach Bestätigung aus. Gibt zurück, ob installiert wurde.
fn offer_install(packages: &[&str]) -> bool {
    let cmd = syscheck::install_command(packages);
    println!("Installation: {}", cmd.join(" "));
    if !interactive() {
        let _ = Command::new("notify-send")
            .args([
                "tvjoe: Systempakete fehlen",
                &format!(
                    "{}\nIm Terminal ausführen: tvjoe --check",
                    packages.join(", ")
                ),
            ])
            .status();
        return false;
    }
    print!("Jetzt installieren? [j/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().lock().read_line(&mut answer);
    if !matches!(answer.trim(), "j" | "J" | "ja" | "Ja") {
        return false;
    }
    Command::new(&cmd[0])
        .args(&cmd[1..])
        .status()
        .is_ok_and(|s| s.success())
}

fn check(dev: bool) -> ExitCode {
    let missing = syscheck::missing_runtime();
    for r in syscheck::RUNTIME {
        let ok = !missing.contains(&r);
        println!(
            "{} {:<11} {} ({})",
            if ok { "✔" } else { "✖" },
            r.what,
            r.soname,
            r.package
        );
    }
    let mut packages: Vec<&str> = missing.iter().map(|r| r.package).collect();
    if dev {
        let dev_missing = syscheck::missing_dev_packages();
        for p in syscheck::DEV_PACKAGES {
            println!(
                "{} {p} (Entwicklung)",
                if dev_missing.contains(p) {
                    "✖"
                } else {
                    "✔"
                }
            );
        }
        packages.extend(dev_missing);
    }
    if packages.is_empty() {
        println!("Alles vorhanden.");
        return ExitCode::SUCCESS;
    }
    if offer_install(&packages) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn start_gui(t0: u128, args: &[String]) -> ExitCode {
    let missing = syscheck::missing_runtime();
    if !missing.is_empty() {
        eprintln!("tvjoe: Es fehlen Systembibliotheken:");
        for r in &missing {
            eprintln!("  {} ({}, Paket {})", r.what, r.soname, r.package);
        }
        let packages: Vec<&str> = missing.iter().map(|r| r.package).collect();
        if !offer_install(&packages) || !syscheck::missing_runtime().is_empty() {
            return ExitCode::FAILURE;
        }
    }

    let gui = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .join(if cfg!(windows) {
            "tvjoe-gui.exe"
        } else {
            "tvjoe-gui"
        });
    let mut cmd = Command::new(&gui);
    cmd.args(args).env("TVJOE_T0", t0.to_string());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        eprintln!("tvjoe: {} lässt sich nicht starten: {err}", gui.display());
        ExitCode::FAILURE
    }
    #[cfg(not(unix))]
    match cmd.status() {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("tvjoe: {} lässt sich nicht starten: {err}", gui.display());
            ExitCode::FAILURE
        }
    }
}

fn import(args: &[String]) -> ExitCode {
    let mut file: Option<&str> = None;
    let mut name: Option<&str> = None;
    let mut with_warnings = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--name" => name = it.next().map(String::as_str),
            "--mit-warnungen" => with_warnings = true,
            _ => file = Some(a),
        }
    }
    let Some(file) = file else {
        eprintln!("tvjoe --import: Datei fehlt (tvjoe --help)");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{file}: {e}");
            return ExitCode::from(2);
        }
    };
    let playlist = match Playlist::parse(&bytes) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{file}: {e}");
            return ExitCode::from(2);
        }
    };

    let errors = playlist
        .findings
        .iter()
        .filter(|f| f.kind.severity() == Severity::Error)
        .count();
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for f in &playlist.findings {
        *by_kind.entry(f.kind.to_string()).or_default() += 1;
    }
    println!(
        "{file}: {} Einträge, {} Befunde, davon {errors} Fehler",
        playlist.entries.len(),
        playlist.findings.len()
    );
    for (kind, n) in &by_kind {
        println!("  {n:>4} × {kind}");
    }

    let selected: Vec<usize> = (0..playlist.entries.len())
        .filter(|&i| {
            let mut own = playlist.findings.iter().filter(|f| f.entry == Some(i));
            if with_warnings {
                !own.any(|f| f.kind.severity() == Severity::Error)
            } else {
                own.next().is_none()
            }
        })
        .collect();
    if selected.is_empty() {
        println!(
            "Keine Einträge ohne Befund. Mit --mit-warnungen auch Einträge mit Warnungen übernehmen."
        );
        return ExitCode::FAILURE;
    }

    let Some(lib_path) = paths::library_path() else {
        eprintln!("Kein Konfigurationsordner gefunden (HOME bzw. APPDATA fehlt).");
        return ExitCode::FAILURE;
    };
    let mut lib = match Library::load(&lib_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}: {e}", lib_path.display());
            return ExitCode::FAILURE;
        }
    };
    let location = std::fs::canonicalize(file)
        .unwrap_or_else(|_| PathBuf::from(file))
        .to_string_lossy()
        .into_owned();
    let source = match lib.sources.iter().find(|s| s.location == location) {
        Some(s) => s.id,
        None => {
            let stem = Path::new(file)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.to_string());
            lib.add_source(name.unwrap_or(&stem), &location)
        }
    };
    let all = selected.len() == playlist.entries.len();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let Some(out) = lib.import(source, &playlist, (!all).then_some(&selected[..]), now) else {
        return ExitCode::FAILURE;
    };
    if let Err(e) = lib.save(&lib_path) {
        eprintln!("{}: {e}", lib_path.display());
        return ExitCode::FAILURE;
    }
    println!(
        "Übernommen: {} Einträge → {} neu, {} aktualisiert, {} fehlen in der Quelle. Bibliothek: {}",
        selected.len(),
        out.added,
        out.updated,
        out.missing,
        lib_path.display()
    );
    ExitCode::SUCCESS
}

fn load_settings() -> (Option<PathBuf>, Settings) {
    let path = paths::settings_path();
    let settings = path
        .as_deref()
        .and_then(|p| {
            Settings::load(p)
                .map_err(|e| eprintln!("{}: {e}", p.display()))
                .ok()
        })
        .unwrap_or_default();
    (path, settings)
}

fn epg_add(args: &[String]) -> ExitCode {
    let Some(src) = args.first() else {
        eprintln!("tvjoe --epg-add: Quelle fehlt");
        return ExitCode::from(2);
    };
    let (path, mut settings) = load_settings();
    let Some(path) = path else {
        return ExitCode::FAILURE;
    };
    if !settings.epg_sources.contains(src) {
        settings.epg_sources.push(src.clone());
    }
    match settings.save(&path) {
        Ok(()) => {
            println!("EPG-Quellen: {}", settings.epg_sources.join(", "));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}

fn epg_check(args: &[String]) -> ExitCode {
    let (_, settings) = load_settings();
    let sources: Vec<String> = if args.is_empty() {
        settings.epg_sources.clone()
    } else {
        args.to_vec()
    };
    if sources.is_empty() {
        eprintln!("Keine EPG-Quelle: tvjoe --epg-add URL oder tvjoe --epg-check URL");
        return ExitCode::from(2);
    }
    let lib = paths::library_path()
        .and_then(|p| Library::load(&p).ok())
        .unwrap_or_default();
    let cache = paths::cache_dir();
    let max_age = (settings.epg_refresh_hours > 0)
        .then(|| std::time::Duration::from_secs(u64::from(settings.epg_refresh_hours) * 3600));
    let started = std::time::Instant::now();
    let (guide, notes) = epg::load_sources(&sources, cache.as_deref(), max_age);
    for n in &notes {
        println!("{n}");
    }
    println!("geladen in {:.1} s", started.elapsed().as_secs_f64());
    let now = epg::now();
    let mut matched = 0;
    for ch in &lib.channels {
        let num = ch.number.map(|n| n.to_string()).unwrap_or_default();
        match guide.match_channel(ch) {
            Some((id, kind)) => {
                matched += 1;
                let how = match kind {
                    MatchKind::Manual => "manuell",
                    MatchKind::TvgId => "tvg-id",
                    MatchKind::Name => "Name",
                    MatchKind::Fuzzy => "unscharf",
                };
                let current = guide
                    .now_next(&id, now)
                    .0
                    .map(|p| p.title.as_str())
                    .unwrap_or("–");
                println!(
                    "✔ {num:>3} {:<28} → {id} ({how}) · jetzt: {current}",
                    ch.display_name()
                );
            }
            None => println!(
                "✖ {num:>3} {:<28} → keine Zuordnung (tvg-id {})",
                ch.display_name(),
                ch.tvg_id.as_deref().unwrap_or("–")
            ),
        }
    }
    println!("{matched} von {} Sendern zugeordnet", lib.channels.len());
    ExitCode::SUCCESS
}

fn update_check() -> ExitCode {
    use tvjoe_core::update;
    println!(
        "Installiert: {VERSION} · Distribution: {} · Architektur: {}",
        update::distro_tag().unwrap_or_else(|| "?".into()),
        update::deb_arch()
    );
    match update::check() {
        Ok(Some(r)) => {
            println!("Neue Version: {} ({})", r.tag_name, r.html_url);
            ExitCode::SUCCESS
        }
        Ok(None) => {
            println!(
                "Keine neuere Version (oder noch keine Veröffentlichung auf github.com/{}).",
                update::REPO
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Prüfung fehlgeschlagen: {e}");
            ExitCode::FAILURE
        }
    }
}
