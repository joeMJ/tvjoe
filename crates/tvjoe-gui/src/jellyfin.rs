//! Jellyfin: Einstellungen je Server-Bibliothek (Server, Anmeldung per Quick Connect,
//! Bandbreite, Bibliotheken des Servers), Abgleich im Hintergrund und das Logo des Servers für
//! den Knopf in der Kopfleiste. Mehrere Jellyfin-Server sind möglich; jede Bibliothek hat ihre
//! Kennung (`jellyfin`, `jellyfin-2`, …), danach Zugang, Zwischenspeicher und Logo.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::jellyfin::{self, Account};
use tvjoe_core::jellyfin_lib::JellyfinLibrary;
use tvjoe_core::settings::Library;
use tvjoe_core::{Settings, epg, paths, tf, tr};

use crate::app::App;
use crate::trace;

/// Voller Abgleich, wenn der letzte länger her ist (sonst nur Änderungen).
const FULL_EVERY_SECS: i64 = 24 * 3600;
/// Änderungen im Hintergrund abgleichen, solange tvjoe läuft.
const CHANGES_EVERY_SECS: u32 = 30 * 60;

thread_local! {
    /// Bibliotheken, deren Abgleich gerade läuft.
    static SYNCING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static TIMER: Cell<bool> = const { Cell::new(false) };
}

/// Stufen der Bandbreitenbegrenzung (kbit/s, 0 = aus).
const LIMITS: [u32; 4] = [0, 2000, 4000, 8000];

/// Gespeicherter Zugang einer Bibliothek, falls angemeldet.
pub fn account(library: &str) -> Option<Account> {
    paths::library_account_path(library).and_then(|p| Account::load(&p))
}

/// Art einer Bibliothek mit eigenem Server (Jellyfin, Plex) – teilen sich Zwischenspeicher,
/// Ansicht, Abgleich und Abspielen.
pub fn is_server_kind(kind: &str) -> bool {
    matches!(kind, "jellyfin" | "plex")
}

/// Eingeschaltete Server-Bibliotheken (Jellyfin, Plex).
fn jellyfin_libraries(app: &App) -> Vec<String> {
    app.settings()
        .enabled_libraries()
        .into_iter()
        .filter(|l| is_server_kind(&l.kind))
        .map(|l| l.id)
        .collect()
}

/// Abgleich im Hintergrund starten (30 s nach dem Start, dann alle 30 Minuten) für jede
/// eingeschaltete und angemeldete Jellyfin-Bibliothek.
pub fn start_background(app: &Rc<App>) {
    let all = |app: &Rc<App>| {
        for id in jellyfin_libraries(app) {
            sync(app, &id);
        }
    };
    // Später erneut (z. B. nach den Einstellungen): sofort abgleichen.
    if TIMER.with(|t| t.replace(true)) {
        all(app);
        return;
    }
    // Beim Start erst nach 30 s, damit Fenster und erstes Bild nicht warten.
    let weak = Rc::downgrade(app);
    glib::timeout_add_seconds_local_once(30, move || {
        if let Some(app) = weak.upgrade() {
            all(&app);
        }
    });
    let weak = Rc::downgrade(app);
    glib::timeout_add_seconds_local(CHANGES_EVERY_SECS, move || match weak.upgrade() {
        Some(app) => {
            all(&app);
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
}

/// Ein Abgleich einer Bibliothek (voll, wenn älter als ein Tag oder leer; sonst nur Änderungen).
pub fn sync(app: &Rc<App>, library: &str) {
    let enabled = app.settings().library(library).is_some_and(|l| l.enabled);
    if !enabled || SYNCING.with(|s| s.borrow().contains(library)) {
        return;
    }
    let (Some(acc), Some(db)) = (account(library), paths::library_db(library)) else {
        return;
    };
    SYNCING.with(|s| s.borrow_mut().insert(library.to_string()));
    let library = library.to_string();
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let run = || -> Result<String, String> {
            let mut lib = JellyfinLibrary::open(&db)?;
            let now = epg::now();
            let full = lib.last_full().is_none_or(|t| now - t >= FULL_EVERY_SECS);
            if full {
                let st = lib.sync_full(&acc, now)?;
                Ok(format!(
                    "voll: {} Einträge, {} Sammlungen, {} entfernt",
                    st.items, st.collections, st.removed
                ))
            } else {
                Ok(format!("Änderungen: {}", lib.sync_changes(&acc, now)?))
            }
        };
        let _ = tx.send_blocking(run());
    });
    glib::spawn_future_local(async move {
        let Ok(result) = rx.recv().await else { return };
        SYNCING.with(|s| s.borrow_mut().remove(&library));
        match result {
            Ok(text) => {
                trace(&format!("Jellyfin-Abgleich {library}: {text}"));
                crate::jfview::refresh(&library);
            }
            Err(e) => eprintln!("tvjoe: Jellyfin-Abgleich {library}: {e}"),
        }
    });
}

/// Stand des Zwischenspeichers für die Einstellungen.
fn library_status(library: &str) -> String {
    let Some(lib) = paths::library_db(library)
        .filter(|p| p.exists())
        .and_then(|p| JellyfinLibrary::open(&p).ok())
    else {
        return tr("Noch nicht abgeglichen").to_string();
    };
    let when = lib
        .last_sync()
        .and_then(|t| glib::DateTime::from_unix_local(t).ok())
        .and_then(|d| d.format("%d.%m. %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "–".into());
    tf!(
        "Stand {when} · {filme} Filme, {sammlungen} Sammlungen, {serien} Serien, {folgen} Folgen",
        when = when,
        filme = lib.count("Movie"),
        sammlungen = lib.count("BoxSet"),
        serien = lib.count("Series"),
        folgen = lib.count("Episode")
    )
}

/// Serveradresse: aus den Einstellungen, sonst aus dem gespeicherten Zugang.
fn server_of(l: &Library) -> String {
    let from_settings = jellyfin::normalize_server(&l.server);
    if from_settings.is_empty() {
        account(&l.id).map(|a| a.server).unwrap_or_default()
    } else {
        from_settings
    }
}

/// Dateiname im Zwischenspeicher je Bibliothek (die erste Jellyfin-Bibliothek wie bis 0.1.4).
fn cache_name(library: &str, suffix: &str) -> Option<PathBuf> {
    let name = if library == tvjoe_core::settings::LIBRARY_JELLYFIN {
        format!("jellyfin-{suffix}")
    } else {
        format!("{library}-{suffix}")
    };
    paths::cache_dir().map(|d| d.join(name))
}

/// Weiße Fassung des Logos (Bauchbinde, Info).
pub fn white_logo_path(library: &str) -> Option<PathBuf> {
    cache_name(library, "logo-weiss.png")
}

/// Position an Jellyfin melden (im Hintergrund) und im Zwischenspeicher nachführen.
/// `stop`: Wiedergabe endet (Jellyfin wertet dann „gesehen“ aus; Umrechnung beenden).
pub fn report_position(j: &crate::vod::JellyVod, pos: f64, dur: f64, paused: bool, stop: bool) {
    let played = dur > 0.0 && pos >= dur * 0.9;
    if let Some(lib) = paths::library_db(&j.library)
        .filter(|p| p.exists())
        .and_then(|p| JellyfinLibrary::open(&p).ok())
    {
        lib.set_progress(
            &j.item.id,
            if played { 0.0 } else { pos },
            played || j.item.played,
        );
    }
    let (Some(acc), Some(pb)) = (account(&j.library), j.playback.clone()) else {
        return;
    };
    let (id, session) = (j.item.id.clone(), j.session.clone());
    let what = if stop {
        jellyfin::Report::Stop
    } else {
        jellyfin::Report::Progress
    };
    let work = move || {
        if let Err(e) = acc.report(what, &id, &pb, &session, pos, paused) {
            eprintln!("tvjoe: Jellyfin melden: {e}");
        }
        if stop && pb.transcoded {
            acc.stop_transcoding(&session);
        }
    };
    if stop {
        // Beim Beenden von tvjoe muss die Meldung noch rausgehen (kurzer Zeitrahmen im Agenten).
        work();
    } else {
        std::thread::spawn(work);
    }
}

fn logo_path(library: &str) -> Option<PathBuf> {
    cache_name(library, "logo.png")
}

/// Logo des Servers für den Knopf: aus dem Cache, sonst im Hintergrund laden.
pub fn load_logo(app: &Rc<App>, library: &str) {
    let Some(path) = logo_path(library) else { return };
    if path.exists() {
        app.set_library_logo(library, &path);
        make_white(library, &path);
        return;
    }
    let Some(l) = app.settings().library(library).cloned() else { return };
    let server = server_of(&l);
    if server.is_empty() {
        return;
    }
    let (tx, rx) = async_channel::bounded(1);
    let target = path.clone();
    std::thread::spawn(move || {
        let result = jellyfin::logo_url(&server).and_then(|url| {
            let bytes = tvjoe_core::net::fetch(
                &tvjoe_core::net::agent(Duration::from_secs(15)),
                &url,
                1 << 20,
            )?;
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&target, bytes).map_err(|e| e.to_string())
        });
        let _ = tx.send_blocking(result);
    });
    let weak = Rc::downgrade(app);
    let library = library.to_string();
    glib::spawn_future_local(async move {
        match rx.recv().await {
            Ok(Ok(())) => {
                if let Some(app) = weak.upgrade() {
                    app.set_library_logo(&library, &path);
                }
                make_white(&library, &path);
            }
            Ok(Err(e)) => trace(&format!("Jellyfin-Logo: {e}")),
            Err(_) => {}
        }
    });
}

fn make_white(library: &str, path: &std::path::Path) {
    if let Some(dst) = white_logo_path(library).filter(|p| !p.exists()) {
        if let Err(e) = crate::logos::save_white_from_dark(path, &dst) {
            trace(&format!("Jellyfin-Logo weiß: {e}"));
        }
    }
}

/// Gruppen einer Jellyfin-Bibliothek für ihre Seite unter „Mediatheken“. Gibt die Gruppen und
/// die Übernahme beim Schließen zurück.
pub fn prefs_groups(
    app: &Rc<App>,
    library: &str,
    kind: &str,
) -> (Vec<adw::PreferencesGroup>, impl Fn(&mut Settings) + 'static) {
    let l = app.settings().library(library).cloned().unwrap_or_default();
    let id = library.to_string();
    let plex = kind == "plex";
    // Plex im Heimnetz meist ohne TLS unter der IP-Adresse (Port 32400).
    let normalize = move |url: &str| -> String {
        let url = url.trim().trim_end_matches('/');
        if plex && !url.is_empty() && !url.contains("://") {
            format!("http://{url}")
        } else if plex {
            url.to_string()
        } else {
            jellyfin::normalize_server(url)
        }
    };

    let server_group = adw::PreferencesGroup::builder()
        .title(tr("Server"))
        .description(if plex {
            tr("Eigener Plex Media Server (experimentell); die Anmeldung läuft über einen Code bei plex.tv, tvjoe fragt nie nach einem Passwort. Dabei gehen Konto und IP-Adresse an plex.tv.")
        } else {
            tr("Eigener Jellyfin-Server; die Anmeldung läuft über Quick Connect, tvjoe fragt nie nach einem Passwort")
        })
        .build();
    let server = adw::EntryRow::builder()
        .title(if plex {
            tr("Adresse (z. B. http://<IP-Adresse>:32400)")
        } else {
            tr("Adresse (z. B. https://jellyfin.example.org)")
        })
        .text(server_of(&l))
        .build();
    let login_row = adw::ActionRow::builder().title(tr("Anmeldung")).build();
    let login_hint = adw::ActionRow::builder()
        .title(tr("So geht die Anmeldung"))
        .subtitle(if plex {
            tr("„Anmelden…“ zeigt einen vierstelligen Code. Im Browser plex.tv/link öffnen, mit dem Plex-Konto anmelden und den Code eingeben.")
        } else {
            tr("„Anmelden…“ zeigt einen Code. In Jellyfin oben rechts auf das Benutzersymbol klicken, „Quick Connect“ wählen und den Code bestätigen.")
        })
        .build();
    let login = gtk::Button::builder()
        .label(tr("Anmelden…"))
        .valign(gtk::Align::Center)
        .build();
    let logout = gtk::Button::builder()
        .label(tr("Abmelden"))
        .valign(gtk::Align::Center)
        .build();
    login_row.add_suffix(&login);
    login_row.add_suffix(&logout);
    server_group.add(&server);
    server_group.add(&login_row);
    server_group.add(&login_hint);

    let show_state = {
        let (row, login, logout, id) = (login_row.clone(), login.clone(), logout.clone(), id.clone());
        move || match account(&id) {
            Some(a) => {
                row.set_subtitle(&tf!(
                    "Angemeldet als {user} bei {server}",
                    user = a.user_name,
                    server = a.server
                ));
                login.set_visible(false);
                logout.set_visible(true);
            }
            None => {
                row.set_subtitle(tr("Nicht angemeldet"));
                login.set_visible(true);
                logout.set_visible(false);
            }
        }
    };
    show_state();
    let show_state = Rc::new(show_state);

    let st = show_state.clone();
    let lid = id.clone();
    logout.connect_clicked(move |_| {
        if let (Some(a), Some(path)) = (account(&lid), paths::library_account_path(&lid)) {
            std::thread::spawn(move || a.logout());
            let _ = std::fs::remove_file(path);
        }
        st();
    });
    let st = show_state.clone();
    let entry = server.clone();
    let lid = id.clone();
    login.connect_clicked(move |b| {
        let url = normalize(&entry.text());
        if url.is_empty() {
            return;
        }
        entry.set_text(&url);
        if plex {
            plex_pin(b, &lid, url, st.clone());
        } else {
            quick_connect(b, &lid, url, st.clone());
        }
    });

    let traffic = adw::PreferencesGroup::builder()
        .title(tr("Bandbreite"))
        .description(tr("Mit Begrenzung rechnet der Server Filme auf H.264 in der gewählten Bitrate um (kostet Rechenleistung auf dem Server); ohne kommt die Originaldatei und wird hier per Grafikeinheit dekodiert"))
        .build();
    let limit = adw::ComboRow::builder()
        .title(tr("Bandbreite begrenzen"))
        .model(&gtk::StringList::new(&[
            tr("Aus"),
            "2 Mbit/s",
            "4 Mbit/s",
            "8 Mbit/s",
        ]))
        .selected(
            LIMITS
                .iter()
                .position(|k| *k == l.max_kbps)
                .unwrap_or(0) as u32,
        )
        .build();
    traffic.add(&limit);

    // Bibliotheken aus dem letzten Abgleich, je ein Schalter.
    let libs_group = adw::PreferencesGroup::builder()
        .title(tr("Bibliotheken"))
        .description(library_status(library))
        .build();
    let libs = paths::library_db(library)
        .filter(|p| p.exists())
        .and_then(|p| JellyfinLibrary::open(&p).ok())
        .map(|l| l.libraries())
        .unwrap_or_default();
    if libs.is_empty() {
        libs_group.add(
            &adw::ActionRow::builder()
                .title(tr("Erscheinen nach dem ersten Abgleich"))
                .build(),
        );
    }
    let lib_rows: Vec<(String, adw::SwitchRow)> = libs
        .into_iter()
        .map(|sl| {
            let row = adw::SwitchRow::builder()
                .title(&sl.name)
                .active(!l.hidden.contains(&sl.name))
                .build();
            libs_group.add(&row);
            (sl.name, row)
        })
        .collect();

    let apply = move |s: &mut Settings| {
        let Some(l) = s.library_mut(&id) else { return };
        l.server = normalize(&server.text());
        l.max_kbps = LIMITS.get(limit.selected() as usize).copied().unwrap_or(0);
        if !lib_rows.is_empty() {
            l.hidden = lib_rows
                .iter()
                .filter(|(_, r)| !r.is_active())
                .map(|(n, _)| n.clone())
                .collect();
        }
    };
    (vec![server_group, traffic, libs_group], apply)
}

/// Quick Connect: Code anzeigen, im Hintergrund auf die Bestätigung warten (höchstens 5 min),
/// dann den Zugang speichern.
fn quick_connect(parent: &impl IsA<gtk::Widget>, library: &str, server: String, done: Rc<dyn Fn()>) {
    let library = library.to_string();
    let device_id = account(&library)
        .map(|a| a.device_id)
        .unwrap_or_else(jellyfin::new_device_id);
    let dialog = adw::AlertDialog::new(Some(tr("Bei Jellyfin anmelden")), Some(tr("Verbinde …")));
    dialog.add_response("cancel", tr("Abbrechen"));
    dialog.set_close_response("cancel");
    dialog.present(Some(parent));

    let cancel = Rc::new(std::sync::atomic::AtomicBool::new(false));
    let c = cancel.clone();
    dialog.connect_response(None, move |_, _| {
        c.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    enum Step {
        Code(String),
        Done(Result<Account, String>),
    }
    let (tx, rx) = async_channel::unbounded();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_thread = stop.clone();
    std::thread::spawn(move || {
        let run = || -> Result<Account, String> {
            if !jellyfin::quick_connect_enabled(&server)? {
                return Err(
                    tr("Quick Connect ist auf dem Server ausgeschaltet (Dashboard → Allgemein).")
                        .into(),
                );
            }
            let qc = jellyfin::quick_connect_start(&server, &device_id)?;
            let _ = tx.send_blocking(Step::Code(qc.code.clone()));
            for _ in 0..150 {
                if stop_thread.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(tr("abgebrochen").into());
                }
                std::thread::sleep(Duration::from_secs(2));
                if jellyfin::quick_connect_confirmed(&server, &device_id, &qc)? {
                    return jellyfin::quick_connect_finish(&server, &device_id, &qc);
                }
            }
            Err(tr("Zeit abgelaufen – bitte erneut anmelden.").into())
        };
        let _ = tx.send_blocking(Step::Done(run()));
    });
    glib::spawn_future_local(async move {
        while let Ok(step) = rx.recv().await {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                break;
            }
            match step {
                Step::Code(code) => dialog.set_body(&tf!(
                    "Code: {code}\n\nSo bestätigst du ihn:\n1. Jellyfin im Browser oder in der App öffnen, angemeldet als der Benutzer, mit dem tvjoe arbeiten soll.\n2. Oben rechts auf das runde Benutzersymbol klicken.\n3. Im aufklappenden Menü „Quick Connect“ wählen (zwischen „Metadaten-Manager“ und „Abmelden“), den Code eingeben und bestätigen.\n\ntvjoe wartet bis zu 5 Minuten und schließt diesen Dialog dann selbst.",
                    code = code
                )),
                Step::Done(Ok(account)) => {
                    let saved = paths::library_account_path(&library)
                        .ok_or_else(|| tr("kein Konfigurationsordner").to_string())
                        .and_then(|p| account.save(&p));
                    match saved {
                        Ok(()) => {
                            trace(&format!("Jellyfin: angemeldet als {}", account.user_name));
                            dialog.close();
                        }
                        Err(e) => dialog.set_body(&tf!("Speichern fehlgeschlagen: {fehler}", fehler = e)),
                    }
                    done();
                    break;
                }
                Step::Done(Err(e)) => {
                    dialog.set_body(&tf!("Anmeldung nicht möglich: {fehler}", fehler = e));
                    break;
                }
            }
        }
    });
}

/// Plex: Code bei plex.tv holen und anzeigen, im Hintergrund auf die Bestätigung unter
/// plex.tv/link warten (höchstens 15 min), dann den Zugang zum Server speichern.
fn plex_pin(parent: &impl IsA<gtk::Widget>, library: &str, server: String, done: Rc<dyn Fn()>) {
    let library = library.to_string();
    let client_id = account(&library)
        .map(|a| a.device_id)
        .unwrap_or_else(jellyfin::new_device_id);
    let dialog = adw::AlertDialog::new(Some(tr("Bei Plex anmelden")), Some(tr("Verbinde mit plex.tv …")));
    dialog.add_response("cancel", tr("Abbrechen"));
    dialog.set_close_response("cancel");
    dialog.present(Some(parent));
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let c = cancel.clone();
    dialog.connect_response(None, move |_, _| {
        c.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    enum Step {
        Code(String),
        Done(Result<Account, String>),
    }
    let (tx, rx) = async_channel::unbounded();
    let stop = cancel.clone();
    std::thread::spawn(move || {
        let run = || -> Result<Account, String> {
            let pin = tvjoe_core::plex::pin_start(&client_id)?;
            let _ = tx.send_blocking(Step::Code(pin.code.clone()));
            for _ in 0..450 {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(tr("abgebrochen").into());
                }
                std::thread::sleep(Duration::from_secs(2));
                if let Some(token) = tvjoe_core::plex::pin_token(&client_id, &pin)? {
                    return tvjoe_core::plex::login(&server, &client_id, &token);
                }
            }
            Err(tr("Zeit abgelaufen – bitte erneut anmelden.").into())
        };
        let _ = tx.send_blocking(Step::Done(run()));
    });
    glib::spawn_future_local(async move {
        while let Ok(step) = rx.recv().await {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            match step {
                Step::Code(code) => dialog.set_body(&tf!(
                    "Code: {code}\n\nSo bestätigst du ihn:\n1. Im Browser plex.tv/link öffnen.\n2. Mit dem Plex-Konto anmelden, dem der Server gehört oder das ihn nutzen darf.\n3. Den Code eingeben.\n\ntvjoe wartet bis zu 15 Minuten und schließt diesen Dialog dann selbst.",
                    code = code
                )),
                Step::Done(Ok(account)) => {
                    let saved = paths::library_account_path(&library)
                        .ok_or_else(|| tr("kein Konfigurationsordner").to_string())
                        .and_then(|p| account.save(&p));
                    match saved {
                        Ok(()) => {
                            trace(&format!("Plex: angemeldet als {}", account.user_name));
                            dialog.close();
                        }
                        Err(e) => dialog.set_body(&tf!("Speichern fehlgeschlagen: {fehler}", fehler = e)),
                    }
                    done();
                    break;
                }
                Step::Done(Err(e)) => {
                    dialog.set_body(&tf!("Anmeldung nicht möglich: {fehler}", fehler = e));
                    break;
                }
            }
        }
    });
}
