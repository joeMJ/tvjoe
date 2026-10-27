//! Einstellungen und Updateprüfung.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::{Fallback, Quality, Strategy, VERSION, epg, i18n, paths, tf, tr, update};

use crate::app::App;

pub fn open(app: &Rc<App>) {
    open_at(app, None, &app.window);
}

/// Ein Einstellungsfenster mit Reitern; `page` wählt den Reiter (z. B. „mediathek“ vom Zahnrad
/// der Mediathek), `parent` ist das Fenster, über dem es erscheint.
pub fn open_at(app: &Rc<App>, page: Option<&str>, parent: &impl IsA<gtk::Widget>) {
    let s = app.settings().clone();
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(tr("Einstellungen"));
    let new_page = |name: &str, title: &str, icon: &str| {
        adw::PreferencesPage::builder()
            .name(name)
            .title(title)
            .icon_name(icon)
            .build()
    };

    let control = adw::PreferencesGroup::builder().title(tr("Bedienung")).build();
    let pause = adw::SpinRow::with_range(300.0, 5000.0, 100.0);
    pause.set_title(tr("Ziffernblock-Pause"));
    pause.set_subtitle(tr("Millisekunden bis zum Umschalten nach der letzten Ziffer"));
    pause.set_value(f64::from(s.numpad_pause_ms));
    let overlay = adw::SpinRow::with_range(1.0, 30.0, 1.0);
    overlay.set_title(tr("Einblendung"));
    overlay.set_subtitle(tr("Sekunden, die Sendername und Programm sichtbar bleiben"));
    overlay.set_value(f64::from(s.overlay_secs));
    let resume = adw::SwitchRow::builder()
        .title(tr("Letzten Sender beim Start"))
        .active(s.resume_last)
        .build();
    let inhibit = adw::SwitchRow::builder()
        .title(tr("Bildschirmsperre auch im Fenster verhindern"))
        .subtitle(tr("Im Vollbild wird sie immer verhindert, solange tvjoe läuft"))
        .active(s.inhibit_windowed)
        .build();
    control.add(&pause);
    control.add(&overlay);
    control.add(&resume);
    control.add(&inhibit);
    let sidebar = adw::SwitchRow::builder()
        .title(tr("Senderliste beim Start zeigen"))
        .subtitle(tr("Sie liegt über dem Bild; L blendet sie ein und aus"))
        .active(s.sidebar_on_start)
        .build();
    control.add(&sidebar);
    // Sprache: leer = Systemsprache; wirkt nach dem Neustart.
    let mut lang_names = vec![tr("Systemsprache")];
    lang_names.extend(i18n::LANGUAGES.iter().map(|(_, name)| *name));
    let language = adw::ComboRow::builder()
        .title(tr("Sprache"))
        .subtitle(tr("Wirkt nach dem Neustart von tvjoe"))
        .model(&gtk::StringList::new(&lang_names))
        .selected(
            i18n::LANGUAGES
                .iter()
                .position(|(code, _)| *code == s.language)
                .map_or(0, |i| i as u32 + 1),
        )
        .build();
    control.add(&language);

    let osd = adw::PreferencesGroup::builder()
        .title(tr("Senderanzeige"))
        .description(tr("Einblendung unten im Bild beim Umschalten, mit I oder Mausklick aufs Bild"))
        .build();
    let switch = |title: &str, active: bool| {
        let row = adw::SwitchRow::builder()
            .title(title)
            .active(active)
            .build();
        osd.add(&row);
        row
    };
    let osd_number = switch(tr("Kanalnummer"), s.osd_number);
    let osd_origin = switch(
        tr("Quellsymbol vor der Qualität (Internet, Satellit)"),
        s.osd_origin,
    );
    let osd_logo = switch(tr("Senderlogo bei Internet-Sendern (schwarzweiß)"), s.osd_logo);
    osd_logo.set_subtitle(tr("Auch in der Info und in der Senderliste"));
    let sat_logo = switch(tr("Senderlogo bei SAT-Sendern (schwarzweiß)"), s.sat_logo);
    sat_logo.set_subtitle(tr("Auch in der Info und in der Senderliste"));
    let osd_category = switch(
        tr("Kategorie (Vollprogramm, Spartenprogramm …)"),
        s.osd_category,
    );
    let osd_quality = switch(tr("Qualität"), s.osd_quality);
    let osd_epg = switch(tr("Jetzt und Danach"), s.osd_epg);
    let osd_description = switch(tr("Beschreibung der laufenden Sendung"), s.osd_description);

    let quality = adw::PreferencesGroup::builder().title(tr("Qualität")).build();
    let levels = [
        None,
        Some(Quality::Uhd),
        Some(Quality::P1080),
        Some(Quality::P720),
        Some(Quality::Sd),
    ];
    let level = adw::ComboRow::builder()
        .title(tr("Qualität"))
        .subtitle(tr("Gilt für alle Sender; Taste Q schaltet ebenfalls um"))
        .model(&gtk::StringList::new(&[
            tr("Höchste"), "UHD", "1080p", "720p", "SD",
        ]))
        .selected(levels.iter().position(|l| *l == s.quality).unwrap_or(0) as u32)
        .build();
    let cap = adw::SwitchRow::builder()
        .title(tr("Nie höher als die gewählte Stufe"))
        .subtitle(tr("Fehlt sie, wird die nächstniedrigere genommen"))
        .active(s.quality_cap)
        .build();
    let deinterlace = adw::SwitchRow::builder()
        .title(tr("Zeilensprung entfernen"))
        .subtitle(tr("Gegen Kammstreifen bei Bewegung (z. B. SD-Sender über SAT); wirkt nur bei Zeilensprung-Bildern"))
        .active(s.deinterlace)
        .build();
    quality.add(&level);
    quality.add(&cap);
    let subtitles = adw::SwitchRow::builder()
        .title(tr("Untertitel"))
        .subtitle(tr("Sofern der Sender welche mitschickt; Taste U wählt die Spur für die laufende Sendung"))
        .active(s.subtitles)
        .build();
    quality.add(&deinterlace);
    let loudness = adw::SwitchRow::builder()
        .title(tr("Lautstärke angleichen"))
        .subtitle(tr("Hebt den Ton um 7 dB an (Fernsehnorm −23 LUFS auf etwa −16 LUFS wie bei Musik und Videoportalen); ein Begrenzer verhindert Übersteuern"))
        .active(s.loudness)
        .build();
    quality.add(&subtitles);
    quality.add(&loudness);
    let crop_bars = adw::SwitchRow::builder()
        .title(tr("Schwarze Balken abschneiden"))
        .subtitle(tr("Erkennt mitgesendete Balken im Bild; das Fenster lässt sich dann kleiner ziehen, ohne dass das Bild schrumpft. Knopf neben Vollbild"))
        .active(s.crop_bars)
        .build();
    let fill = adw::SwitchRow::builder()
        .title(tr("Bild füllen"))
        .subtitle(tr("Vergrößert, bis das Bild an allen Seiten anstößt; Überstand fällt weg (z. B. Breitbildmonitor im Vollbild). Taste Z"))
        .active(s.fill)
        .build();
    quality.add(&crop_bars);
    quality.add(&fill);
    let strategy = adw::ComboRow::builder()
        .title(tr("Beim Senderwechsel"))
        .model(&gtk::StringList::new(&[
            tr("Qualität halten"),
            tr("Immer die höchste"),
        ]))
        .selected(match s.strategy {
            Strategy::Keep => 0,
            Strategy::Highest => 1,
        })
        .build();
    let fallback = adw::ComboRow::builder()
        .title(tr("Fehlt die Stufe, ausweichen"))
        .subtitle(tr("Bei gleichem Abstand"))
        .model(&gtk::StringList::new(&[
            tr("Nach unten (sparsamer)"),
            tr("Nach oben"),
        ]))
        .selected(match s.fallback {
            Fallback::Down => 0,
            Fallback::Up => 1,
        })
        .build();
    quality.add(&strategy);
    quality.add(&fallback);

    let storage = adw::PreferencesGroup::builder()
        .title(tr("Pufferspeicher"))
        .description(tr("Im Arbeitsspeicher: hält beim Fernsehen eine Pause (Leertaste) und gleicht ein schwaches Netz aus. Gilt ab dem nächsten Senderwechsel."))
        .build();
    let buffer_mb = adw::SpinRow::with_range(64.0, 8192.0, 64.0);
    buffer_mb.set_title(tr("Größe"));
    buffer_mb.set_subtitle(tr("MB; drei Viertel davon halten die Pause, bei 1080p (etwa 5 Mbit/s) reichen 512 MB für rund zehn Minuten"));
    buffer_mb.set_value(f64::from(s.buffer_mb));
    let start_buffer = adw::SpinRow::with_range(1.0, 10.0, 1.0);
    start_buffer.set_title(tr("Startpuffer"));
    start_buffer.set_subtitle(tr("Sekunden Vorrat vor dem Abspielen und nach einem Abriss; mehr ist robuster, der Senderwechsel dauert entsprechend länger"));
    start_buffer.set_value(f64::from(s.start_buffer_secs));
    storage.add(&buffer_mb);
    storage.add(&start_buffer);

    let guide = adw::PreferencesGroup::builder()
        .title(tr("Programm (EPG)"))
        .build();
    let hours = adw::SpinRow::with_range(0.0, 168.0, 1.0);
    hours.set_title(tr("Neu laden nach"));
    hours.set_subtitle(tr("Stunden; 0 = nur über „EPG-Quellen…“ in der Verwaltung"));
    hours.set_value(f64::from(s.epg_refresh_hours));
    guide.add(&hours);
    let sat_hours = adw::SpinRow::with_range(1.0, 24.0, 1.0);
    sat_hours.set_title(tr("SAT-Programmdaten sammeln alle"));
    sat_hours.set_subtitle(tr("Stunden; nur für Sender mit EPG-Quelle „SAT“ (Sender verwalten → EPG). Belegt je Transponder etwa 40 Sekunden einen Tuner"));
    sat_hours.set_value(f64::from(s.sat_epg_hours));
    guide.add(&sat_hours);

    let updates = adw::PreferencesGroup::builder()
        .title(tr("Updates"))
        .description(tf!(
            "Installiert: {version}. Die Prüfung fragt höchstens einmal täglich bei GitHub ({repo}) nach und übermittelt dabei die IP-Adresse. Heruntergeladen wird nur nach Bestätigung.",
            version = VERSION,
            repo = update::REPO
        ))
        .build();
    let check = adw::SwitchRow::builder()
        .title(tr("Nach neuen Versionen suchen"))
        .active(s.update_check)
        .build();
    let now_row = adw::ActionRow::builder().title(tr("Jetzt prüfen")).build();
    let now_button = gtk::Button::builder()
        .label(tr("Prüfen"))
        .valign(gtk::Align::Center)
        .build();
    now_row.add_suffix(&now_button);
    updates.add(&check);
    updates.add(&now_row);

    let p_control = new_page("bedienung", tr("Bedienung"), "input-keyboard-symbolic");
    p_control.add(&control);
    // Eine geladene Sicherung wird erst nach dem Schließen übernommen (nach den Werten
    // dieses Dialogs, damit sie diese ersetzt).
    let pending: Rc<std::cell::RefCell<Option<tvjoe_core::backup::Backup>>> = Rc::default();
    p_control.add(&crate::backup::prefs_group(app, &dialog, &pending));
    let p_osd = new_page("anzeige", tr("Senderanzeige"), "video-display-symbolic");
    p_osd.add(&osd);
    let p_quality = new_page("qualitaet", tr("Qualität"), "emblem-photos-symbolic");
    p_quality.add(&quality);
    p_quality.add(&storage);
    let p_guide = new_page("programm", tr("Programm"), "x-office-calendar-symbolic");
    p_guide.add(&guide);
    let (p_media, apply_media) = crate::mediathek::prefs_page(app);
    let (p_jellyfin, apply_jellyfin) = crate::jellyfin::prefs_page(app);
    let (p_satip, apply_satip) = crate::satip::prefs_page(app);
    let (p_ablage, apply_ablage) = crate::ablage::prefs_page(app);
    let p_updates = new_page("updates", tr("Updates"), "software-update-available-symbolic");
    p_updates.add(&updates);

    // Am Ende: Über tvjoe (Version, Lizenzen, Verweis auf das README bei GitHub).
    let about = adw::PreferencesGroup::new();
    let about_row = adw::ActionRow::builder()
        .title(tr("Über tvjoe"))
        .subtitle(tf!("Version {version}, Lizenzen", version = VERSION))
        .activatable(true)
        .build();
    about_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    about.add(&about_row);
    // Fehler und Wünsche: öffnet den Issue-Tracker bei GitHub im Browser.
    let issue_row = adw::ActionRow::builder()
        .title(tr("Fehler melden / Wunsch"))
        .subtitle(tr("Öffnet die Issues von tvjoe bei GitHub im Browser (GitHub-Konto nötig)"))
        .activatable(true)
        .build();
    issue_row.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
    about.add(&issue_row);
    let issue_parent = dialog.clone();
    issue_row.connect_activated(move |_| {
        let parent = issue_parent.root().and_downcast::<gtk::Window>();
        gtk::UriLauncher::new(ISSUES_URL).launch(
            parent.as_ref(),
            gtk::gio::Cancellable::NONE,
            |_| {},
        );
    });
    p_updates.add(&about);
    for p in [
        &p_control,
        &p_osd,
        &p_quality,
        &p_guide,
        &p_media,
        &p_jellyfin,
        &p_satip,
        &p_ablage,
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
            s.language = match language.selected() {
                0 => String::new(),
                i => i18n::LANGUAGES
                    .get(i as usize - 1)
                    .map(|(code, _)| code.to_string())
                    .unwrap_or_default(),
            };
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
            s.sat_epg_hours = sat_hours.value() as u32;
            s.quality = levels.get(level.selected() as usize).copied().flatten();
            s.quality_cap = cap.is_active();
            s.deinterlace = deinterlace.is_active();
            s.subtitles = subtitles.is_active();
            s.buffer_mb = buffer_mb.value() as u32;
            s.start_buffer_secs = start_buffer.value() as u32;
            s.loudness = loudness.is_active();
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
            apply_ablage(&mut s);
        }
        app.save_settings();
        app.set_view(crop_bars.is_active(), fill.is_active());
        if let Some(p) = &app.player {
            p.set_deinterlace(app.settings().deinterlace);
            p.set_subtitles(app.settings().subtitles);
            p.set_loudness(app.settings().loudness);
            p.set_buffer(app.settings().buffer_mb, app.settings().start_buffer_secs);
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
const ISSUES_URL: &str = "https://github.com/joeMJ/tvjoe/issues";
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
        .comments(tr(
            "Streaming-Client für Live-TV und die Mediatheken der öffentlich-rechtlichen Sender.\n\n\
             Die Filmliste der Mediathek stammt von MediathekView (mediathekview.de); \
             die Videos kommen direkt von den Sendern.\n\n\
             Privates Hobbyprojekt – nicht gepflegt.",
        ))
        .build();
    d.add_link(tr("README bei GitHub"), README_URL);
    d.add_link("MediathekView", "https://mediathekview.de/");
    d.add_link(tr("Bibliotheken Dritter (THIRD_PARTY.md)"), THIRD_PARTY_URL);
    d.add_acknowledgement_section(
        Some(tr("Filmliste der Mediathek")),
        &["MediathekView https://mediathekview.de/"],
    );
    d.add_legal_section(
        tr("GTK 4 und libadwaita"),
        Some("The GTK Team, GNOME"),
        gtk::License::Lgpl21,
        None,
    );
    d.add_legal_section(
        "mpv / libmpv",
        Some(tr("mpv-Entwickler")),
        gtk::License::Custom,
        Some(tr(
            "Quelltext GPL-2+ bzw. LGPL-2.1+. Die Ubuntu-Pakete von libmpv stehen laut deren \
             copyright-Datei unter GPL-3+ (gelinkt gegen libsmbclient); für die tvjoe-Pakete gelten \
             diese Bedingungen zusätzlich.",
        )),
    );
    d.add_legal_section(
        tr("SQLite (eingebaut)"),
        None,
        gtk::License::Custom,
        Some(tr("Public Domain, siehe https://sqlite.org/copyright.html")),
    );
    d.add_legal_section(
        tr("xz / liblzma (eingebaut)"),
        None,
        gtk::License::Bsd,
        Some("0BSD"),
    );
    d.add_legal_section(
        tr("Rust-Bibliotheken"),
        None,
        gtk::License::Custom,
        Some(&tf!(
            "Rund 150 Pakete, überwiegend MIT bzw. Apache-2.0, dazu ISC, BSD-3-Clause, Zlib, \
             Unicode-3.0, CDLA-Permissive-2.0 und LGPL-2.1 (libmpv2). Vollständige Liste: {url}",
            url = THIRD_PARTY_URL
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
                    tr("Keine neuere Version"),
                    &tf!("Installiert ist {version}.", version = VERSION),
                );
                d.add_response("close", tr("OK"));
            }
            Err(e) if manual => {
                let d = alert(&app, tr("Prüfung fehlgeschlagen"), &e);
                d.add_response("close", tr("OK"));
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
        &tf!("tvjoe {version} ist verfügbar", version = release.tag_name),
        &(tf!("Installiert ist {version}.", version = VERSION) + "\n\n" + &notes),
    );
    d.add_response("close", tr("Später"));
    // Windows: keine .deb-Pakete; die neue MSI lädt man von der Release-Seite und installiert sie
    // über die alte.
    if cfg!(windows) {
        d.add_response("open", tr("Zur Release-Seite"));
        d.set_response_appearance("open", adw::ResponseAppearance::Suggested);
        let url = release.html_url.clone();
        let parent = app.window.clone();
        d.connect_response(Some("open"), move |_, _| {
            gtk::UriLauncher::new(&url).launch(Some(&parent), gtk::gio::Cancellable::NONE, |_| {});
        });
        return;
    }
    d.add_response("download", tr("Herunterladen…"));
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
                    let d = alert(&app, tr("Download fehlgeschlagen"), &e);
                    d.add_response("close", tr("OK"));
                }
            }
        });
    });
}

fn confirm_install(app: &Rc<App>, path: &std::path::Path) {
    let cmd = update::install_command(path);
    let d = alert(
        app,
        tr("Paket geprüft"),
        &tf!(
            "Die Prüfsumme (SHA-256) stimmt. Installation mit:\n\n{befehl}\n\nDanach tvjoe neu starten.",
            befehl = cmd.join(" ")
        ),
    );
    d.add_response("close", tr("Abbrechen"));
    d.add_response("install", tr("Installieren"));
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
                alert(&app, tr("Installiert"), tr("Bitte tvjoe neu starten."))
            } else {
                alert(
                    &app,
                    tr("Installation nicht abgeschlossen"),
                    tr("Abgebrochen oder fehlgeschlagen."),
                )
            };
            d.add_response("close", tr("OK"));
        });
    });
}
