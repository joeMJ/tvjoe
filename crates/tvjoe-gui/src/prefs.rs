//! Einstellungen und Updateprüfung.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::{Fallback, Quality, Strategy, VERSION, epg, paths, update};

use crate::app::App;

pub fn open(app: &Rc<App>) {
    open_at(app, None, &app.window);
}

/// Ein Einstellungsfenster mit Reitern; `page` wählt den Reiter (z. B. „mediathek“ vom Zahnrad
/// der Mediathek), `parent` ist das Fenster, über dem es erscheint.
pub fn open_at(app: &Rc<App>, page: Option<&str>, parent: &impl IsA<gtk::Widget>) {
    let s = app.settings().clone();
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Einstellungen");
    let new_page = |name: &str, title: &str, icon: &str| {
        adw::PreferencesPage::builder()
            .name(name)
            .title(title)
            .icon_name(icon)
            .build()
    };

    let control = adw::PreferencesGroup::builder().title("Bedienung").build();
    let pause = adw::SpinRow::with_range(300.0, 5000.0, 100.0);
    pause.set_title("Ziffernblock-Pause");
    pause.set_subtitle("Millisekunden bis zum Umschalten nach der letzten Ziffer");
    pause.set_value(f64::from(s.numpad_pause_ms));
    let overlay = adw::SpinRow::with_range(1.0, 30.0, 1.0);
    overlay.set_title("Einblendung");
    overlay.set_subtitle("Sekunden, die Sendername und Programm sichtbar bleiben");
    overlay.set_value(f64::from(s.overlay_secs));
    let resume = adw::SwitchRow::builder()
        .title("Letzten Sender beim Start")
        .active(s.resume_last)
        .build();
    let inhibit = adw::SwitchRow::builder()
        .title("Bildschirmsperre auch im Fenster verhindern")
        .subtitle("Im Vollbild wird sie immer verhindert, solange tvjoe läuft")
        .active(s.inhibit_windowed)
        .build();
    control.add(&pause);
    control.add(&overlay);
    control.add(&resume);
    control.add(&inhibit);
    let sidebar = adw::SwitchRow::builder()
        .title("Senderliste beim Start zeigen")
        .subtitle("Sie liegt über dem Bild; L blendet sie ein und aus")
        .active(s.sidebar_on_start)
        .build();
    control.add(&sidebar);

    let osd = adw::PreferencesGroup::builder()
        .title("Senderanzeige")
        .description("Einblendung unten im Bild beim Umschalten, mit I oder Mausklick aufs Bild")
        .build();
    let switch = |title: &str, active: bool| {
        let row = adw::SwitchRow::builder()
            .title(title)
            .active(active)
            .build();
        osd.add(&row);
        row
    };
    let osd_number = switch("Kanalnummer", s.osd_number);
    let osd_origin = switch(
        "Quellsymbol vor der Qualität (Internet, Satellit)",
        s.osd_origin,
    );
    let osd_logo = switch("Senderlogo bei Internet-Sendern (schwarzweiß)", s.osd_logo);
    osd_logo.set_subtitle("Auch in der Info und in der Senderliste");
    let sat_logo = switch("Senderlogo bei SAT-Sendern (schwarzweiß)", s.sat_logo);
    sat_logo.set_subtitle("Auch in der Info und in der Senderliste");
    let osd_category = switch(
        "Kategorie (Vollprogramm, Spartenprogramm …)",
        s.osd_category,
    );
    let osd_quality = switch("Qualität", s.osd_quality);
    let osd_epg = switch("Jetzt und Danach", s.osd_epg);
    let osd_description = switch("Beschreibung der laufenden Sendung", s.osd_description);

    let quality = adw::PreferencesGroup::builder().title("Qualität").build();
    let levels = [
        None,
        Some(Quality::Uhd),
        Some(Quality::P1080),
        Some(Quality::P720),
        Some(Quality::Sd),
    ];
    let level = adw::ComboRow::builder()
        .title("Qualität")
        .subtitle("Gilt für alle Sender; Taste Q schaltet ebenfalls um")
        .model(&gtk::StringList::new(&[
            "Höchste", "UHD", "1080p", "720p", "SD",
        ]))
        .selected(levels.iter().position(|l| *l == s.quality).unwrap_or(0) as u32)
        .build();
    let cap = adw::SwitchRow::builder()
        .title("Nie höher als die gewählte Stufe")
        .subtitle("Fehlt sie, wird die nächstniedrigere genommen")
        .active(s.quality_cap)
        .build();
    let deinterlace = adw::SwitchRow::builder()
        .title("Zeilensprung entfernen")
        .subtitle("Gegen Kammstreifen bei Bewegung (z. B. SD-Sender über SAT); wirkt nur bei Zeilensprung-Bildern")
        .active(s.deinterlace)
        .build();
    quality.add(&level);
    quality.add(&cap);
    quality.add(&deinterlace);
    let strategy = adw::ComboRow::builder()
        .title("Beim Senderwechsel")
        .model(&gtk::StringList::new(&[
            "Qualität halten",
            "Immer die höchste",
        ]))
        .selected(match s.strategy {
            Strategy::Keep => 0,
            Strategy::Highest => 1,
        })
        .build();
    let fallback = adw::ComboRow::builder()
        .title("Fehlt die Stufe, ausweichen")
        .subtitle("Bei gleichem Abstand")
        .model(&gtk::StringList::new(&[
            "Nach unten (sparsamer)",
            "Nach oben",
        ]))
        .selected(match s.fallback {
            Fallback::Down => 0,
            Fallback::Up => 1,
        })
        .build();
    quality.add(&strategy);
    quality.add(&fallback);

    let guide = adw::PreferencesGroup::builder()
        .title("Programm (EPG)")
        .build();
    let hours = adw::SpinRow::with_range(0.0, 168.0, 1.0);
    hours.set_title("Neu laden nach");
    hours.set_subtitle("Stunden; 0 = nur über „EPG-Quellen…“ in der Verwaltung");
    hours.set_value(f64::from(s.epg_refresh_hours));
    guide.add(&hours);

    let updates = adw::PreferencesGroup::builder()
        .title("Updates")
        .description(format!(
            "Installiert: {VERSION}. Die Prüfung fragt höchstens einmal täglich bei GitHub ({}) nach und übermittelt dabei die IP-Adresse. Heruntergeladen wird nur nach Bestätigung.",
            update::REPO
        ))
        .build();
    let check = adw::SwitchRow::builder()
        .title("Nach neuen Versionen suchen")
        .active(s.update_check)
        .build();
    let now_row = adw::ActionRow::builder().title("Jetzt prüfen").build();
    let now_button = gtk::Button::builder()
        .label("Prüfen")
        .valign(gtk::Align::Center)
        .build();
    now_row.add_suffix(&now_button);
    updates.add(&check);
    updates.add(&now_row);

    let p_control = new_page("bedienung", "Bedienung", "input-keyboard-symbolic");
    p_control.add(&control);
    // Eine geladene Sicherung wird erst nach dem Schließen übernommen (nach den Werten
    // dieses Dialogs, damit sie diese ersetzt).
    let pending: Rc<std::cell::RefCell<Option<tvjoe_core::backup::Backup>>> = Rc::default();
    p_control.add(&crate::backup::prefs_group(app, &dialog, &pending));
    let p_osd = new_page("anzeige", "Senderanzeige", "video-display-symbolic");
    p_osd.add(&osd);
    let p_quality = new_page("qualitaet", "Qualität", "emblem-photos-symbolic");
    p_quality.add(&quality);
    let p_guide = new_page("programm", "Programm", "x-office-calendar-symbolic");
    p_guide.add(&guide);
    let (p_media, apply_media) = crate::mediathek::prefs_page(app);
    let (p_jellyfin, apply_jellyfin) = crate::jellyfin::prefs_page(app);
    let (p_satip, apply_satip) = crate::satip::prefs_page(app);
    let p_updates = new_page("updates", "Updates", "software-update-available-symbolic");
    p_updates.add(&updates);

    // Am Ende: Über tvjoe (Version, Lizenzen, Verweis auf das README bei GitHub).
    let about = adw::PreferencesGroup::new();
    let about_row = adw::ActionRow::builder()
        .title("Über tvjoe")
        .subtitle(format!("Version {VERSION}, Lizenzen"))
        .activatable(true)
        .build();
    about_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    about.add(&about_row);
    p_updates.add(&about);
    for p in [
        &p_control,
        &p_osd,
        &p_quality,
        &p_guide,
        &p_media,
        &p_jellyfin,
        &p_satip,
        &p_updates,
    ] {
        dialog.add(p);
    }
    if let Some(name) = page {
        dialog.set_visible_page_name(name);
    }
    let about_parent = dialog.clone();
    about_row.connect_activated(move |_| about_dialog().present(Some(&about_parent)));

    let weak = Rc::downgrade(app);
    now_button.connect_clicked(move |_| {
        if let Some(app) = weak.upgrade() {
            check_for_update(&app, true);
        }
    });

    let weak = Rc::downgrade(app);
    dialog.connect_closed(move |_| {
        let Some(app) = weak.upgrade() else { return };
        let logos_before = {
            let s = app.settings();
            (s.osd_logo, s.sat_logo)
        };
        {
            let mut s = app.settings_mut();
            s.numpad_pause_ms = pause.value() as u32;
            s.overlay_secs = overlay.value() as u32;
            s.resume_last = resume.is_active();
            s.inhibit_windowed = inhibit.is_active();
            s.sidebar_on_start = sidebar.is_active();
            s.strategy = if strategy.selected() == 1 {
                Strategy::Highest
            } else {
                Strategy::Keep
            };
            s.fallback = if fallback.selected() == 1 {
                Fallback::Up
            } else {
                Fallback::Down
            };
            s.epg_refresh_hours = hours.value() as u32;
            s.quality = levels.get(level.selected() as usize).copied().flatten();
            s.quality_cap = cap.is_active();
            s.deinterlace = deinterlace.is_active();
            s.osd_number = osd_number.is_active();
            s.osd_origin = osd_origin.is_active();
            s.osd_logo = osd_logo.is_active();
            s.sat_logo = sat_logo.is_active();
            s.osd_category = osd_category.is_active();
            s.osd_quality = osd_quality.is_active();
            s.osd_epg = osd_epg.is_active();
            s.osd_description = osd_description.is_active();
            s.update_check = check.is_active();
            apply_media(&mut s);
            apply_jellyfin(&mut s);
            apply_satip(&mut s);
        }
        app.save_settings();
        if let Some(p) = &app.player {
            p.set_deinterlace(app.settings().deinterlace);
        }
        app.reapply_quality();
        app.update_inhibit();
        crate::mediathek::settings_changed();
        app.update_media_buttons();
        crate::jfview::refresh();
        let logos_after = {
            let s = app.settings();
            (s.osd_logo, s.sat_logo)
        };
        if logos_after != logos_before {
            app.rebuild_list();
        }
        if let Some(b) = pending.take() {
            app.apply_backup(b);
        }
    });
    dialog.present(Some(parent));
}

const README_URL: &str = "https://github.com/joeMJ/tvjoe#readme";
const THIRD_PARTY_URL: &str = "https://github.com/joeMJ/tvjoe/blob/main/THIRD_PARTY.md";

/// „Über tvjoe“: Version, Entwickler, Lizenz, Verweis auf das README, Mediathek-Daten von
/// MediathekView, Bibliotheken mit Lizenzen (vollständige Liste in THIRD_PARTY.md).
fn about_dialog() -> adw::AboutDialog {
    let d = adw::AboutDialog::builder()
        .application_name("tvjoe")
        .application_icon("io.github.joeMJ.tvjoe")
        .version(VERSION)
        .developer_name("John Lose (joeMJ)")
        .developers(["John Lose (joeMJ)"])
        .copyright("© 2026 John Lose")
        .license_type(gtk::License::Apache20)
        .website(README_URL)
        .comments(
            "Streaming-Client für Live-TV und die Mediatheken der öffentlich-rechtlichen Sender.\n\n\
             Die Filmliste der Mediathek stammt von MediathekView (mediathekview.de); \
             die Videos kommen direkt von den Sendern.\n\n\
             Privates Hobbyprojekt – nicht gepflegt.",
        )
        .build();
    d.add_link("README bei GitHub", README_URL);
    d.add_link("MediathekView", "https://mediathekview.de/");
    d.add_link("Bibliotheken Dritter (THIRD_PARTY.md)", THIRD_PARTY_URL);
    d.add_acknowledgement_section(
        Some("Filmliste der Mediathek"),
        &["MediathekView https://mediathekview.de/"],
    );
    d.add_legal_section(
        "GTK 4 und libadwaita",
        Some("The GTK Team, GNOME"),
        gtk::License::Lgpl21,
        None,
    );
    d.add_legal_section(
        "mpv / libmpv",
        Some("mpv-Entwickler"),
        gtk::License::Custom,
        Some(
            "Quelltext GPL-2+ bzw. LGPL-2.1+. Die Ubuntu-Pakete von libmpv stehen laut deren \
             copyright-Datei unter GPL-3+ (gelinkt gegen libsmbclient); für die tvjoe-Pakete gelten \
             diese Bedingungen zusätzlich.",
        ),
    );
    d.add_legal_section(
        "SQLite (eingebaut)",
        None,
        gtk::License::Custom,
        Some("Public Domain, siehe https://sqlite.org/copyright.html"),
    );
    d.add_legal_section(
        "xz / liblzma (eingebaut)",
        None,
        gtk::License::Bsd,
        Some("0BSD"),
    );
    d.add_legal_section(
        "Rust-Bibliotheken",
        None,
        gtk::License::Custom,
        Some(&format!(
            "Rund 150 Pakete, überwiegend MIT bzw. Apache-2.0, dazu ISC, BSD-3-Clause, Zlib, \
             Unicode-3.0, CDLA-Permissive-2.0 und LGPL-2.1 (libmpv2). Vollständige Liste: {THIRD_PARTY_URL}"
        )),
    );
    d
}

fn alert(app: &App, heading: &str, body: &str) -> adw::AlertDialog {
    let d = adw::AlertDialog::new(Some(heading), Some(body));
    d.set_close_response("close");
    d.present(Some(&app.window));
    d
}

/// Automatische Prüfung beim Start: nur wenn eingeschaltet und die letzte länger als einen Tag her ist.
pub fn maybe_check_on_start(app: &Rc<App>) {
    let due = {
        let s = app.settings();
        s.update_check
            && s.update_last_check
                .is_none_or(|t| epg::now() - t as i64 > 86_400)
    };
    if due {
        check_for_update(app, false);
    }
}

/// Prüft im Hintergrund. `manual`: auch „keine neue Version“ und Fehler melden.
pub fn check_for_update(app: &Rc<App>, manual: bool) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(update::check());
    });
    let weak = Rc::downgrade(app);
    glib::spawn_future_local(async move {
        let Ok(result) = rx.recv().await else { return };
        let Some(app) = weak.upgrade() else { return };
        app.settings_mut().update_last_check = Some(epg::now() as u64);
        app.save_settings();
        match result {
            Ok(Some(release)) => offer(&app, release),
            Ok(None) if manual => {
                let d = alert(
                    &app,
                    "Keine neuere Version",
                    &format!("Installiert ist {VERSION}."),
                );
                d.add_response("close", "OK");
            }
            Err(e) if manual => {
                let d = alert(&app, "Prüfung fehlgeschlagen", &e);
                d.add_response("close", "OK");
            }
            _ => {}
        }
    });
}

fn offer(app: &Rc<App>, release: update::Release) {
    let notes: String = release
        .body
        .clone()
        .unwrap_or_default()
        .chars()
        .take(800)
        .collect();
    let d = alert(
        app,
        &format!("tvjoe {} ist verfügbar", release.tag_name),
        &format!("Installiert ist {VERSION}.\n\n{notes}"),
    );
    d.add_response("close", "Später");
    d.add_response("download", "Herunterladen…");
    d.set_response_appearance("download", adw::ResponseAppearance::Suggested);
    let weak = Rc::downgrade(app);
    d.connect_response(Some("download"), move |_, _| {
        let Some(app) = weak.upgrade() else { return };
        let Some(dir) = paths::cache_dir().map(|d| d.join("updates")) else {
            return;
        };
        let (tx, rx) = async_channel::bounded(1);
        let rel = release.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(update::download(&rel, &dir));
        });
        let weak = Rc::downgrade(&app);
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else { return };
            let Some(app) = weak.upgrade() else { return };
            match result {
                Ok(path) => confirm_install(&app, &path),
                Err(e) => {
                    let d = alert(&app, "Download fehlgeschlagen", &e);
                    d.add_response("close", "OK");
                }
            }
        });
    });
}

fn confirm_install(app: &Rc<App>, path: &std::path::Path) {
    let cmd = update::install_command(path);
    let d = alert(
        app,
        "Paket geprüft",
        &format!(
            "Die Prüfsumme (SHA-256) stimmt. Installation mit:\n\n{}\n\nDanach tvjoe neu starten.",
            cmd.join(" ")
        ),
    );
    d.add_response("close", "Abbrechen");
    d.add_response("install", "Installieren");
    d.set_response_appearance("install", adw::ResponseAppearance::Suggested);
    let weak = Rc::downgrade(app);
    d.connect_response(Some("install"), move |_, _| {
        let cmd = cmd.clone();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let ok = std::process::Command::new(&cmd[0])
                .args(&cmd[1..])
                .status()
                .is_ok_and(|s| s.success());
            let _ = tx.send_blocking(ok);
        });
        let weak = weak.clone();
        glib::spawn_future_local(async move {
            let Ok(ok) = rx.recv().await else { return };
            let Some(app) = weak.upgrade() else { return };
            let d = if ok {
                alert(&app, "Installiert", "Bitte tvjoe neu starten.")
            } else {
                alert(
                    &app,
                    "Installation nicht abgeschlossen",
                    "Abgebrochen oder fehlgeschlagen.",
                )
            };
            d.add_response("close", "OK");
        });
    });
}
