//! Starter `tvjoe`: prüft die Systembibliotheken (ohne GTK zu laden), bietet die Installation an
//! und startet dann die Oberfläche `tvjoe-gui`. Außerdem Kommandozeilen-Import einer M3U.
//! Unter Windows ein Fensterprogramm ohne eigenes Konsolenfenster (siehe `tvjoe_core::console`).
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use tvjoe_core::epg::{self, MatchKind};
use tvjoe_core::{Library, Playlist, Settings, Severity, VERSION, paths, syscheck, tf, tr};

fn main() -> ExitCode {
    tvjoe_core::console::attach_parent();
    init_language();
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

/// Sprache wählen (Einstellung `language`, sonst Systemsprache; eigene Kataloge in `sprachen/`).
/// Die Oberfläche läuft als eigener Prozess und wählt ihre Sprache selbst.
fn init_language() {
    let choice = paths::settings_path()
        .and_then(|p| Settings::load(&p).ok())
        .map(|s| s.language)
        .unwrap_or_default();
    let dir = paths::config_dir().map(|d| d.join("sprachen"));
    tvjoe_core::i18n::init(&choice, &tvjoe_core::i18n::env_languages(), dir.as_deref());
}

fn print_help() {
    println!("tvjoe {VERSION}\n");
    println!(
        "{}\n",
        tr("Aufruf:
  tvjoe [--channel NUMMER] [--mute] [--quit-after SEKUNDEN] [--protokoll]
  tvjoe --import DATEI.m3u [--name NAME] [--mit-warnungen]
  tvjoe --check [--dev]
  tvjoe --epg-add QUELLE          XMLTV-Datei oder -URL in die Einstellungen aufnehmen
  tvjoe --epg-check [QUELLE …]    Zuordnung der Sender zum EPG prüfen
  tvjoe --update-check            nach neuer Version auf GitHub fragen
  tvjoe --version")
    );
    println!(
        "{}",
        tr("--protokoll schreibt nach jeder Pause oder Fortsetzung sechs Sekunden lang Bildnummer,
Position und jedes Neuzeichnen in die Ausgabe (Fehlersuche).
--import übernimmt Einträge ohne Befund; mit --mit-warnungen auch Einträge mit Warnungen.
Umgebung: TVJOE_CONFIG_DIR, TVJOE_CACHE_DIR (andere Ablage), TVJOE_TRACE=1 (Zeitmessung).")
    );
}

fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Zeigt den Installationsbefehl und führt ihn nach Bestätigung aus. Gibt zurück, ob installiert wurde.
fn offer_install(packages: &[&str]) -> bool {
    if cfg!(windows) {
        // Unter Windows liegen die DLLs dem Programm bei; beim Bauen kommen sie aus MSYS2.
        let msys = packages
            .iter()
            .map(|p| match *p {
                p if p.starts_with("mingw-") => p.to_string(),
                // pkg-config-Modul → MSYS2-Paket
                "libadwaita-1" => "mingw-w64-ucrt-x86_64-libadwaita".to_string(),
                p => format!("mingw-w64-ucrt-x86_64-{p}"),
            })
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "{}",
            tf!(
                "Fehlt: {packages}. Zum Bauen in MSYS2 installieren: pacman -S {msys}",
                packages = packages.join(", "),
                msys = msys
            )
        );
        return false;
    }
    let cmd = syscheck::install_command(packages);
    println!("{}", tf!("Installation: {command}", command = cmd.join(" ")));
    if !interactive() {
        let _ = Command::new("notify-send")
            .args([
                tr("tvjoe: Systempakete fehlen"),
                &tf!(
                    "{packages}\nIm Terminal ausführen: tvjoe --check",
                    packages = packages.join(", ")
                ),
            ])
            .status();
        return false;
    }
    print!("{} ", tr("Jetzt installieren? [j/N]"));
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().lock().read_line(&mut answer);
    if !matches!(
        answer.trim(),
        "j" | "J" | "ja" | "Ja" | "y" | "Y" | "yes" | "Yes"
    ) {
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
            let what = if cfg!(windows) {
                tf!("{package} (Entwicklung, pkg-config)", package = p)
            } else {
                tf!("{package} (Entwicklung)", package = p)
            };
            println!(
                "{} {what}",
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
        println!("{}", tr("Alles vorhanden."));
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
        eprintln!("{}", tr("tvjoe: Es fehlen Systembibliotheken:"));
        for r in &missing {
            eprintln!(
                "{}",
                tf!(
                    "  {what} ({soname}, Paket {package})",
                    what = r.what,
                    soname = r.soname,
                    package = r.package
                )
            );
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
        eprintln!(
            "{}",
            tf!(
                "tvjoe: {program} lässt sich nicht starten: {error}",
                program = gui.display(),
                error = err
            )
        );
        ExitCode::FAILURE
    }
    #[cfg(not(unix))]
    match cmd.status() {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!(
                "{}",
                tf!(
                    "tvjoe: {program} lässt sich nicht starten: {error}",
                    program = gui.display(),
                    error = err
                )
            );
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
        eprintln!("{}", tr("tvjoe --import: Datei fehlt (tvjoe --help)"));
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
        "{}",
        tf!(
            "{file}: {entries} Einträge, {findings} Befunde, davon {errors} Fehler",
            file = file,
            entries = playlist.entries.len(),
            findings = playlist.findings.len(),
            errors = errors
        )
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
            "{}",
            tr("Keine Einträge ohne Befund. Mit --mit-warnungen auch Einträge mit Warnungen übernehmen.")
        );
        return ExitCode::FAILURE;
    }

    let Some(lib_path) = paths::library_path() else {
        eprintln!(
            "{}",
            tr("Kein Konfigurationsordner gefunden (HOME bzw. APPDATA fehlt).")
        );
        return ExitCode::FAILURE;
    };
    let mut lib = match Library::load(&lib_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}: {e}", lib_path.display());
            return ExitCode::FAILURE;
        }
    };
    let location = paths::file_location(std::path::Path::new(file));
    let source = match lib
        .sources
        .iter()
        .find(|s| paths::plain_location(&s.location) == location)
    {
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
        "{}",
        tf!(
            "Übernommen: {taken} Einträge → {added} neu, {updated} aktualisiert, {missing} fehlen in der Quelle. Bibliothek: {library}",
            taken = selected.len(),
            added = out.added,
            updated = out.updated,
            missing = out.missing,
            library = lib_path.display()
        )
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
        eprintln!("{}", tr("tvjoe --epg-add: Quelle fehlt"));
        return ExitCode::from(2);
    };
    let (path, mut settings) = load_settings();
    let Some(path) = path else {
        return ExitCode::FAILURE;
    };
    // Als XMLTV-Quelle anlegen und der Empfangsquelle Internet-TV zuordnen.
    let id = settings.add_xmltv("", src);
    let tv = settings.adapter_mut(tvjoe_core::settings::ADAPTER_TV);
    if !tv.xmltv.contains(&id) {
        tv.xmltv.push(id);
    }
    match settings.save(&path) {
        Ok(()) => {
            println!(
                "{}",
                tf!(
                    "EPG-Quellen: {sources}",
                    sources = settings
                        .xmltv_locations(tvjoe_core::settings::ADAPTER_TV)
                        .join(", ")
                )
            );
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
        // Alle Quellen, die eine Empfangsquelle nutzt.
        let mut all = Vec::new();
        for a in &settings.adapters {
            for loc in settings.xmltv_locations(&a.id) {
                if !all.contains(&loc) {
                    all.push(loc);
                }
            }
        }
        all
    } else {
        args.to_vec()
    };
    if sources.is_empty() {
        eprintln!(
            "{}",
            tr("Keine EPG-Quelle: tvjoe --epg-add URL oder tvjoe --epg-check URL")
        );
        return ExitCode::from(2);
    }
    let lib = paths::library_path()
        .and_then(|p| Library::load(&p).ok())
        .unwrap_or_default();
    let cache = paths::cache_dir();
    let hours = settings
        .adapter(tvjoe_core::settings::ADAPTER_TV)
        .map_or(settings.epg_refresh_hours, |a| a.xmltv_hours);
    let max_age = (hours > 0).then(|| std::time::Duration::from_secs(u64::from(hours) * 3600));
    let started = std::time::Instant::now();
    let (guide, notes) = epg::load_sources(&sources, cache.as_deref(), max_age);
    for n in &notes {
        println!("{n}");
    }
    println!(
        "{}",
        tf!(
            "geladen in {seconds} s",
            seconds = format!("{:.1}", started.elapsed().as_secs_f64())
        )
    );
    let now = epg::now();
    let mut matched = 0;
    for ch in &lib.channels {
        let num = ch.number.map(|n| n.to_string()).unwrap_or_default();
        match guide.match_channel(ch) {
            Some((id, kind)) => {
                matched += 1;
                let how = match kind {
                    MatchKind::Manual => tr("manuell"),
                    MatchKind::TvgId => "tvg-id",
                    MatchKind::Name => tr("Name"),
                    MatchKind::Fuzzy => tr("unscharf"),
                };
                let current = guide
                    .now_next(&id, now)
                    .0
                    .map(|p| p.title.as_str())
                    .unwrap_or("–");
                println!(
                    "{}",
                    tf!(
                        "✔ {num} {name} → {id} ({how}) · jetzt: {current}",
                        num = format!("{num:>3}"),
                        name = format!("{:<28}", ch.display_name()),
                        id = id,
                        how = how,
                        current = current
                    )
                );
            }
            None => println!(
                "{}",
                tf!(
                    "✖ {num} {name} → keine Zuordnung (tvg-id {tvg_id})",
                    num = format!("{num:>3}"),
                    name = format!("{:<28}", ch.display_name()),
                    tvg_id = ch.tvg_id.as_deref().unwrap_or("–")
                )
            ),
        }
    }
    println!(
        "{}",
        tf!(
            "{matched} von {total} Sendern zugeordnet",
            matched = matched,
            total = lib.channels.len()
        )
    );
    ExitCode::SUCCESS
}

fn update_check() -> ExitCode {
    use tvjoe_core::update;
    println!(
        "{}",
        tf!(
            "Installiert: {version} · Distribution: {distro} · Architektur: {arch}",
            version = VERSION,
            distro = update::distro_tag().unwrap_or_else(|| "?".into()),
            arch = update::deb_arch()
        )
    );
    match update::check() {
        Ok(Some(r)) => {
            println!(
                "{}",
                tf!(
                    "Neue Version: {tag} ({url})",
                    tag = r.tag_name,
                    url = r.html_url
                )
            );
            ExitCode::SUCCESS
        }
        Ok(None) => {
            println!(
                "{}",
                tf!(
                    "Keine neuere Version (oder noch keine Veröffentlichung auf github.com/{repo}).",
                    repo = update::REPO
                )
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}", tf!("Prüfung fehlgeschlagen: {error}", error = e));
            ExitCode::FAILURE
        }
    }
}
