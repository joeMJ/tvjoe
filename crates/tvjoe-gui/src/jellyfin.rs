//! Jellyfin: Einstellungen (Server, Anmeldung per Quick Connect, Bandbreite, Bibliotheken),
//! Abgleich der Bibliothek im Hintergrund und das Logo des Servers für den Knopf in der
//! Kopfleiste. Die Ansicht folgt im nächsten Schritt.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::jellyfin::{self, Account};
use tvjoe_core::jellyfin_lib::JellyfinLibrary;
use tvjoe_core::{Settings, epg, paths};

use crate::app::App;
use crate::trace;

/// Voller Abgleich, wenn der letzte länger her ist (sonst nur Änderungen).
const FULL_EVERY_SECS: i64 = 24 * 3600;
/// Änderungen im Hintergrund abgleichen, solange tvjoe läuft.
const CHANGES_EVERY_SECS: u32 = 30 * 60;

thread_local! {
    static SYNCING: Cell<bool> = const { Cell::new(false) };
    static TIMER: Cell<bool> = const { Cell::new(false) };
}

/// Stufen der Bandbreitenbegrenzung (kbit/s, 0 = aus).
const LIMITS: [u32; 4] = [0, 2000, 4000, 8000];

/// Gespeicherter Zugang, falls angemeldet.
pub fn account() -> Option<Account> {
    paths::jellyfin_account_path().and_then(|p| Account::load(&p))
}

/// Abgleich im Hintergrund starten (30 s nach dem Start, dann alle 30 Minuten), wenn Jellyfin
/// eingeschaltet und angemeldet ist.
pub fn start_background(app: &Rc<App>) {
    // Später erneut (z. B. nach den Einstellungen): sofort abgleichen.
    if TIMER.with(|t| t.replace(true)) {
        sync(app);
        return;
    }
    // Beim Start erst nach 30 s, damit Fenster und erstes Bild nicht warten.
    let weak = Rc::downgrade(app);
    glib::timeout_add_seconds_local_once(30, move || {
        if let Some(app) = weak.upgrade() {
            sync(&app);
        }
    });
    let weak = Rc::downgrade(app);
    glib::timeout_add_seconds_local(CHANGES_EVERY_SECS, move || match weak.upgrade() {
        Some(app) => {
            sync(&app);
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
}

/// Ein Abgleich (voll, wenn älter als ein Tag oder leer; sonst nur Änderungen).
pub fn sync(app: &Rc<App>) {
    if !app.settings().jellyfin_enabled || SYNCING.with(|s| s.get()) {
        return;
    }
    let (Some(acc), Some(db)) = (account(), paths::jellyfin_db()) else {
        return;
    };
    SYNCING.with(|s| s.set(true));
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
        SYNCING.with(|s| s.set(false));
        match result {
            Ok(text) => {
                trace(&format!("Jellyfin-Abgleich {text}"));
                crate::jfview::refresh();
            }
            Err(e) => eprintln!("tvjoe: Jellyfin-Abgleich: {e}"),
        }
    });
}

/// Stand des Zwischenspeichers für die Einstellungen.
fn library_status() -> String {
    let Some(lib) = paths::jellyfin_db()
        .filter(|p| p.exists())
        .and_then(|p| JellyfinLibrary::open(&p).ok())
    else {
        return "Noch nicht abgeglichen".to_string();
    };
    let when = lib
        .last_sync()
        .and_then(|t| glib::DateTime::from_unix_local(t).ok())
        .and_then(|d| d.format("%d.%m. %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "–".into());
    format!(
        "Stand {when} · {} Filme, {} Sammlungen, {} Serien, {} Folgen",
        lib.count("Movie"),
        lib.count("BoxSet"),
        lib.count("Series"),
        lib.count("Episode")
    )
}

/// Serveradresse: aus den Einstellungen, sonst aus dem gespeicherten Zugang.
fn server_of(s: &Settings) -> String {
    let from_settings = jellyfin::normalize_server(&s.jellyfin_server);
    if from_settings.is_empty() {
        account().map(|a| a.server).unwrap_or_default()
    } else {
        from_settings
    }
}

/// Weiße Fassung des Logos (Bauchbinde, Info).
pub fn white_logo_path() -> Option<PathBuf> {
    paths::cache_dir().map(|d| d.join("jellyfin-logo-weiss.png"))
}

/// Position an Jellyfin melden (im Hintergrund) und im Zwischenspeicher nachführen.
/// `stop`: Wiedergabe endet (Jellyfin wertet dann „gesehen“ aus; Umrechnung beenden).
pub fn report_position(j: &crate::vod::JellyVod, pos: f64, dur: f64, paused: bool, stop: bool) {
    let played = dur > 0.0 && pos >= dur * 0.9;
    if let Some(lib) = paths::jellyfin_db()
        .filter(|p| p.exists())
        .and_then(|p| JellyfinLibrary::open(&p).ok())
    {
        lib.set_progress(
            &j.item.id,
            if played { 0.0 } else { pos },
            played || j.item.played,
        );
    }
    let (Some(acc), Some(pb)) = (account(), j.playback.clone()) else {
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

fn logo_path() -> Option<PathBuf> {
    paths::cache_dir().map(|d| d.join("jellyfin-logo.png"))
}

/// Logo des Servers für den Knopf: aus dem Cache, sonst im Hintergrund laden.
pub fn load_logo(app: &Rc<App>) {
    let Some(path) = logo_path() else { return };
    if path.exists() {
        app.set_jellyfin_logo(&path);
        make_white(&path);
        return;
    }
    let server = server_of(&app.settings());
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
    glib::spawn_future_local(async move {
        match rx.recv().await {
            Ok(Ok(())) => {
                if let Some(app) = weak.upgrade() {
                    app.set_jellyfin_logo(&path);
                }
                make_white(&path);
            }
            Ok(Err(e)) => trace(&format!("Jellyfin-Logo: {e}")),
            Err(_) => {}
        }
    });
}

fn make_white(path: &std::path::Path) {
    if let Some(dst) = white_logo_path().filter(|p| !p.exists()) {
        if let Err(e) = crate::logos::save_white_from_dark(path, &dst) {
            trace(&format!("Jellyfin-Logo weiß: {e}"));
        }
    }
}

/// Reiter „Jellyfin“ im Einstellungsfenster. Gibt die Seite und die Übernahme beim Schließen
/// zurück.
pub fn prefs_page(app: &Rc<App>) -> (adw::PreferencesPage, impl Fn(&mut Settings) + 'static) {
    let s = app.settings().clone();
    let page = adw::PreferencesPage::builder()
        .name("jellyfin")
        .title("Jellyfin")
        .icon_name("network-server-symbolic")
        .build();

    let server_group = adw::PreferencesGroup::builder()
        .title("Server")
        .description("Eigener Jellyfin-Server; die Anmeldung läuft über Quick Connect, tvjoe fragt nie nach einem Passwort")
        .build();
    let server = adw::EntryRow::builder()
        .title("Adresse (z. B. https://jellyfin.example.org)")
        .text(server_of(&s))
        .build();
    let login_row = adw::ActionRow::builder().title("Anmeldung").build();
    let login_hint = adw::ActionRow::builder()
        .title("So geht die Anmeldung")
        .subtitle("„Anmelden…“ zeigt einen Code. In Jellyfin oben rechts auf das Benutzersymbol klicken, „Quick Connect“ wählen und den Code bestätigen.")
        .build();
    let login = gtk::Button::builder()
        .label("Anmelden…")
        .valign(gtk::Align::Center)
        .build();
    let logout = gtk::Button::builder()
        .label("Abmelden")
        .valign(gtk::Align::Center)
        .build();
    login_row.add_suffix(&login);
    login_row.add_suffix(&logout);
    server_group.add(&server);
    server_group.add(&login_row);
    server_group.add(&login_hint);

    let show_state = {
        let (row, login, logout) = (login_row.clone(), login.clone(), logout.clone());
        move || match account() {
            Some(a) => {
                row.set_subtitle(&format!("Angemeldet als {} bei {}", a.user_name, a.server));
                login.set_visible(false);
                logout.set_visible(true);
            }
            None => {
                row.set_subtitle("Nicht angemeldet");
                login.set_visible(true);
                logout.set_visible(false);
            }
        }
    };
    show_state();
    let show_state = Rc::new(show_state);

    let st = show_state.clone();
    logout.connect_clicked(move |_| {
        if let (Some(a), Some(path)) = (account(), paths::jellyfin_account_path()) {
            std::thread::spawn(move || a.logout());
            let _ = std::fs::remove_file(path);
        }
        st();
    });
    let st = show_state.clone();
    let entry = server.clone();
    login.connect_clicked(move |b| {
        let url = jellyfin::normalize_server(&entry.text());
        if url.is_empty() {
            return;
        }
        entry.set_text(&url);
        quick_connect(b, url, st.clone());
    });

    let traffic = adw::PreferencesGroup::builder()
        .title("Bandbreite")
        .description("Mit Begrenzung rechnet der Server Filme auf H.264 in der gewählten Bitrate um (kostet Rechenleistung auf dem Server); ohne kommt die Originaldatei und wird hier per Grafikeinheit dekodiert")
        .build();
    let limit = adw::ComboRow::builder()
        .title("Bandbreite begrenzen")
        .model(&gtk::StringList::new(&[
            "Aus", "2 Mbit/s", "4 Mbit/s", "8 Mbit/s",
        ]))
        .selected(
            LIMITS
                .iter()
                .position(|l| *l == s.jellyfin_max_kbps)
                .unwrap_or(0) as u32,
        )
        .build();
    traffic.add(&limit);

    // Bibliotheken aus dem letzten Abgleich, je ein Schalter.
    let libs_group = adw::PreferencesGroup::builder()
        .title("Bibliotheken")
        .description(library_status())
        .build();
    let libs = paths::jellyfin_db()
        .filter(|p| p.exists())
        .and_then(|p| JellyfinLibrary::open(&p).ok())
        .map(|l| l.libraries())
        .unwrap_or_default();
    if libs.is_empty() {
        libs_group.add(
            &adw::ActionRow::builder()
                .title("Erscheinen nach dem ersten Abgleich")
                .build(),
        );
    }
    let lib_rows: Vec<(String, adw::SwitchRow)> = libs
        .into_iter()
        .map(|l| {
            let row = adw::SwitchRow::builder()
                .title(&l.name)
                .active(!s.jellyfin_hidden.contains(&l.name))
                .build();
            libs_group.add(&row);
            (l.name, row)
        })
        .collect();

    page.add(&server_group);
    page.add(&traffic);
    page.add(&libs_group);
    let apply = move |s: &mut Settings| {
        s.jellyfin_server = jellyfin::normalize_server(&server.text());
        s.jellyfin_max_kbps = LIMITS.get(limit.selected() as usize).copied().unwrap_or(0);
        if !lib_rows.is_empty() {
            s.jellyfin_hidden = lib_rows
                .iter()
                .filter(|(_, r)| !r.is_active())
                .map(|(n, _)| n.clone())
                .collect();
        }
    };
    (page, apply)
}

/// Quick Connect: Code anzeigen, im Hintergrund auf die Bestätigung warten (höchstens 5 min),
/// dann den Zugang speichern.
fn quick_connect(parent: &impl IsA<gtk::Widget>, server: String, done: Rc<dyn Fn()>) {
    let device_id = account()
        .map(|a| a.device_id)
        .unwrap_or_else(jellyfin::new_device_id);
    let dialog = adw::AlertDialog::new(Some("Bei Jellyfin anmelden"), Some("Verbinde …"));
    dialog.add_response("cancel", "Abbrechen");
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
                    "Quick Connect ist auf dem Server ausgeschaltet (Dashboard → Allgemein)."
                        .into(),
                );
            }
            let qc = jellyfin::quick_connect_start(&server, &device_id)?;
            let _ = tx.send_blocking(Step::Code(qc.code.clone()));
            for _ in 0..150 {
                if stop_thread.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err("abgebrochen".into());
                }
                std::thread::sleep(Duration::from_secs(2));
                if jellyfin::quick_connect_confirmed(&server, &device_id, &qc)? {
                    return jellyfin::quick_connect_finish(&server, &device_id, &qc);
                }
            }
            Err("Zeit abgelaufen – bitte erneut anmelden.".into())
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
                Step::Code(code) => dialog.set_body(&format!(
                    "Code: {code}\n\n\
                     So bestätigst du ihn:\n\
                     1. Jellyfin im Browser oder in der App öffnen, angemeldet als der Benutzer, \
                     mit dem tvjoe arbeiten soll.\n\
                     2. Oben rechts auf das runde Benutzersymbol klicken.\n\
                     3. Im aufklappenden Menü „Quick Connect“ wählen (zwischen „Metadaten-Manager“ \
                     und „Abmelden“), den Code eingeben und bestätigen.\n\n\
                     tvjoe wartet bis zu 5 Minuten und schließt diesen Dialog dann selbst."
                )),
                Step::Done(Ok(account)) => {
                    let saved = paths::jellyfin_account_path()
                        .ok_or_else(|| "kein Konfigurationsordner".to_string())
                        .and_then(|p| account.save(&p));
                    match saved {
                        Ok(()) => {
                            trace(&format!("Jellyfin: angemeldet als {}", account.user_name));
                            dialog.close();
                        }
                        Err(e) => dialog.set_body(&format!("Speichern fehlgeschlagen: {e}")),
                    }
                    done();
                    break;
                }
                Step::Done(Err(e)) => {
                    dialog.set_body(&format!("Anmeldung nicht möglich: {e}"));
                    break;
                }
            }
        }
    });
}
