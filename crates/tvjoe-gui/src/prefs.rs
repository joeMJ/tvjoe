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
pub fn open_at(
    app: &Rc<App>,
    page: Option<&str>,
    parent: &impl IsA<gtk::Widget>,
) -> adw::PreferencesDialog {
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
    let stop_live = adw::SwitchRow::builder()
        .title(tr("Lineares Fernsehen stoppen, wenn Mediathek geöffnet"))
        .subtitle(tr("Gilt für Mediathek und Jellyfin; beim Schließen läuft der Sender wieder an. Aufnahmen laufen weiter."))
        .active(s.stop_live_for_media)
        .build();
    control.add(&stop_live);
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
    let (p_media, apply_media) = crate::mediatheken::prefs_page(app, &dialog);
    let (p_empfang, apply_empfang) = crate::empfang::prefs_page(app, &dialog);
    let (p_ablage, apply_ablage) = crate::ablage::prefs_page(app);
    let p_updates = new_page("updates", tr("Updates"), "software-update-available-symbolic");
    p_updates.add(&updates);

    // Eigene Seite „Über“ rechts von „Updates“ (Nutzer, 2026-10-27): Version und Lizenzen,
    // Anleitung (README bei GitHub), Fehler melden.
    let p_about = new_page("ueber", tr("Über"), "help-about-symbolic");
    let about = adw::PreferencesGroup::new();
    let about_row = adw::ActionRow::builder()
        .title(tr("Über tvjoe"))
        .subtitle(tf!("Version {version}, Lizenzen", version = VERSION))
        .activatable(true)
        .build();
    about_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    about.add(&about_row);
    let manual_row = adw::ActionRow::builder()
        .title(tr("Anleitung"))
        .subtitle(tr("Öffnet die Anleitung (README) bei GitHub im Browser"))
        .activatable(true)
        .build();
    manual_row.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
    about.add(&manual_row);
    let manual_parent = dialog.clone();
    manual_row.connect_activated(move |_| {
        let parent = manual_parent.root().and_downcast::<gtk::Window>();
        gtk::UriLauncher::new(manual_url()).launch(
            parent.as_ref(),
            gtk::gio::Cancellable::NONE,
            |_| {},
        );
    });
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
    p_about.add(&about);
    for p in [
        &p_control,
        &p_osd,
        &p_quality,
        &p_empfang,
        &p_media,
        &p_ablage,
        &p_updates,
        &p_about,
    ] {
        dialog.add(p);
    }
    if let Some(name) = page {
        dialog.set_visible_page_name(name);
    }
    let about_parent = dialog.clone();
    // Als Unterseite der Einstellungen (Nutzer, 2026-10-28: nicht als eigener Dialog darüber).
    about_row.connect_activated(move |_| about_parent.push_subpage(&about_page(&about_parent)));

    let weak = Rc::downgrade(app);
    now_button.connect_clicked(move |_| {
        if let Some(app) = weak.upgrade() {
            check_for_update(&app, true);
        }
    });

    let weak = Rc::downgrade(app);
    dialog.connect_closed(move |_| {
        let Some(app) = weak.upgrade() else { return };
        // Vorher: Logos und Programmquellen der Empfangsquellen (bei Änderung neu aufbauen bzw.
        // das Programm neu laden).
        let sources_of = |s: &tvjoe_core::Settings| {
            let a: Vec<_> = s
                .adapters
                .iter()
                .map(|a| (a.id.clone(), a.enabled, a.logos, a.xmltv.clone(), a.xmltv_hours))
                .collect();
            (a, s.xmltv_sources.clone())
        };
        let before = sources_of(&app.settings());
        {
            let mut s = app.settings_mut();
            s.numpad_pause_ms = pause.value() as u32;
            s.overlay_secs = overlay.value() as u32;
            s.resume_last = resume.is_active();
            s.inhibit_windowed = inhibit.is_active();
            s.sidebar_on_start = sidebar.is_active();
            s.stop_live_for_media = stop_live.is_active();
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
            s.quality = levels.get(level.selected() as usize).copied().flatten();
            s.quality_cap = cap.is_active();
            s.deinterlace = deinterlace.is_active();
            s.subtitles = subtitles.is_active();
            s.buffer_mb = buffer_mb.value() as u32;
            s.start_buffer_secs = start_buffer.value() as u32;
            s.loudness = loudness.is_active();
            s.osd_number = osd_number.is_active();
            s.osd_origin = osd_origin.is_active();
            s.osd_category = osd_category.is_active();
            s.osd_quality = osd_quality.is_active();
            s.osd_epg = osd_epg.is_active();
            s.osd_description = osd_description.is_active();
            s.update_check = check.is_active();
            apply_media(&mut s);
            apply_empfang(&mut s);
            apply_ablage(&mut s);
        }
        app.save_settings();
        // Suchläufe neuer Empfangsquellen liegen in Dateien: Listen neu lesen.
        app.forget_lists();
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
        let libraries: Vec<String> = app.settings().libraries.iter().map(|l| l.id.clone()).collect();
        for id in libraries {
            crate::jfview::refresh(&id);
        }
        let after = sources_of(&app.settings());
        if after != before {
            app.rebuild_list();
            app.load_epg(after.1 != before.1 || after.0.iter().zip(&before.0).any(|(a, b)| a.3 != b.3));
        }
        if let Some(b) = pending.take() {
            app.apply_backup(b);
        }
    });
    // Zu kleines Hauptfenster: als eigenes Fenster (wie Programm und Mediathek).
    let main: gtk::Widget = app.window.clone().upcast();
    let over_main = parent.as_ref().root().is_some_and(|r| r.upcast::<gtk::Widget>() == main);
    if over_main && (app.window.width() < PREFS_MIN.0 || app.window.height() < PREFS_MIN.1) {
        dialog.set_content_width(PREFS_MIN.0);
        dialog.set_content_height(PREFS_MIN.1);
        dialog.present(None::<&gtk::Widget>);
    } else {
        dialog.present(Some(parent));
    }
    dialog
}

/// Mindestgröße des Hauptfensters für die Einstellungen darüber (sonst eigenes Fenster).
const PREFS_MIN: (i32, i32) = (720, 640);

const README_URL: &str = "https://github.com/joeMJ/tvjoe#readme";
const README_DE_URL: &str = "https://github.com/joeMJ/tvjoe/blob/main/README.de.md";

/// Anleitung in der Sprache der Oberfläche (deutsch: README.de.md, sonst die Startseite).
fn manual_url() -> &'static str {
    if tvjoe_core::i18n::lang() == "de" {
        README_DE_URL
    } else {
        README_URL
    }
}
const ISSUES_URL: &str = "https://github.com/joeMJ/tvjoe/issues";
const THIRD_PARTY_URL: &str = "https://github.com/joeMJ/tvjoe/blob/main/THIRD_PARTY.md";

/// „Über tvjoe“ als Unterseite: Version, Entwickler, Beschreibung, Verweise (README,
/// Bibliotheken Dritter) und Rechtshinweise (Herkunft der Mediathek-Daten steht im Adapter) (vollständige Liste in
/// THIRD_PARTY.md).
fn about_page(dialog: &adw::PreferencesDialog) -> adw::NavigationPage {
    let page = adw::PreferencesPage::new();

    // Kopf: Symbol, Name, Entwickler, Version, Beschreibung.
    let head = gtk::Box::new(gtk::Orientation::Vertical, 6);
    head.set_margin_top(12);
    head.append(&gtk::Image::builder().icon_name("io.github.joeMJ.tvjoe").pixel_size(96).build());
    let name = gtk::Label::new(Some("tvjoe"));
    name.add_css_class("title-1");
    head.append(&name);
    head.append(&gtk::Label::new(Some("John Lose (joeMJ)")));
    let version = gtk::Label::new(Some(VERSION));
    version.add_css_class("dim-label");
    head.append(&version);
    let comments = gtk::Label::new(Some(tr(
        "Streaming-Client für Live-TV und Medienserver.\n\n\
         Privates Hobbyprojekt – nicht gepflegt.",
    )));
    comments.set_wrap(true);
    comments.set_justify(gtk::Justification::Center);
    comments.set_margin_top(12);
    head.append(&comments);
    let head_group = adw::PreferencesGroup::new();
    head_group.add(&head);
    page.add(&head_group);

    // Verweise (im Browser).
    let links = adw::PreferencesGroup::new();
    for (title, url) in [
        (tr("README bei GitHub").to_string(), README_URL),
        (tr("Bibliotheken Dritter (THIRD_PARTY.md)").to_string(), THIRD_PARTY_URL),
    ] {
        let row = adw::ActionRow::builder().title(title).subtitle(url).activatable(true).build();
        row.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
        let parent = dialog.clone();
        row.connect_activated(move |_| {
            let window = parent.root().and_downcast::<gtk::Window>();
            gtk::UriLauncher::new(url).launch(window.as_ref(), gtk::gio::Cancellable::NONE, |_| {});
        });
        links.add(&row);
    }
    page.add(&links);

    let people = adw::PreferencesGroup::builder().title(tr("Mitwirkende")).build();
    people.add(&adw::ActionRow::builder().title("John Lose (joeMJ)").subtitle("© 2026 John Lose · Apache-2.0").build());
    page.add(&people);

    // Rechtshinweise: je Bestandteil eine aufklappbare Zeile.
    let legal = adw::PreferencesGroup::builder().title(tr("Rechtshinweis")).build();
    let rust_text = tf!(
        "Rund 150 Pakete, überwiegend MIT bzw. Apache-2.0, dazu ISC, BSD-3-Clause, Zlib, \
         Unicode-3.0, CDLA-Permissive-2.0 und LGPL-2.1 (libmpv2). Vollständige Liste: {url}",
        url = THIRD_PARTY_URL
    );
    let sections: [(String, String, String); 6] = [
        (
            tr("Symbole: Font Awesome Free").into(),
            "Fonticons, Inc. – https://fontawesome.com".into(),
            tr("Auswahl aus Font Awesome Free 6.7.2 für die Symbole der Quellen; Lizenz der Symbole: CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/).").into(),
        ),
        (tr("GTK 4 und libadwaita").into(), "The GTK Team, GNOME".into(), "LGPL-2.1".into()),
        (
            "mpv / libmpv".into(),
            tr("mpv-Entwickler").into(),
            tr(
                "Quelltext GPL-2+ bzw. LGPL-2.1+. Die Ubuntu-Pakete von libmpv stehen laut deren \
                 copyright-Datei unter GPL-3+ (gelinkt gegen libsmbclient); für die tvjoe-Pakete gelten \
                 diese Bedingungen zusätzlich.",
            )
            .into(),
        ),
        (tr("SQLite (eingebaut)").into(), String::new(), tr("Public Domain, siehe https://sqlite.org/copyright.html").into()),
        (tr("xz / liblzma (eingebaut)").into(), String::new(), "0BSD".into()),
        (tr("Rust-Bibliotheken").into(), String::new(), rust_text),
    ];
    for (title, holder, text) in sections {
        let row = adw::ExpanderRow::builder().title(glib::markup_escape_text(&title)).build();
        if !holder.is_empty() {
            row.set_subtitle(&glib::markup_escape_text(&holder));
        }
        let body = gtk::Label::builder()
            .label(text)
            .wrap(true)
            .xalign(0.0)
            .selectable(true)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        row.add_row(&body);
        legal.add(&row);
    }
    page.add(&legal);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&page));
    adw::NavigationPage::builder().title(tr("Über tvjoe")).child(&toolbar).build()
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

/// Selbstprobe: Update-Angebot mit den Notizen aus `TVJOE_SELFTEST_NOTES` (Datei) zeigen.
pub(crate) fn offer_probe(app: &Rc<App>) {
    let body = std::env::var_os("TVJOE_SELFTEST_NOTES")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| "- **Probe:** Text mit `Code`.".into());
    offer(
        app,
        update::Release {
            tag_name: "v9.9.9".into(),
            name: None,
            body: Some(body),
            html_url: String::new(),
            draft: false,
            prerelease: false,
            assets: Vec::new(),
        },
    );
}

fn offer(app: &Rc<App>, release: update::Release) {
    let d = alert(
        app,
        &tf!("tvjoe {version} ist verfügbar", version = release.tag_name),
        &tf!("Installiert ist {version}.", version = VERSION),
    );
    // Versionsnotizen linksbündig, nur in der Sprache der Oberfläche, Markdown als Markup.
    let body = release.body.clone().unwrap_or_default();
    let blocks = update::release_notes(&body, tvjoe_core::i18n::lang() != "de");
    if !blocks.is_empty() {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        for b in blocks {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            if b.bullet {
                let dot = gtk::Label::new(Some("•"));
                dot.set_valign(gtk::Align::Start);
                row.append(&dot);
            }
            let l = gtk::Label::new(None);
            l.set_markup(&b.markup);
            l.set_wrap(true);
            l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            l.set_xalign(0.0);
            l.set_hexpand(true);
            l.set_max_width_chars(60);
            row.append(&l);
            list.append(&row);
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(320);
        scroll.set_child(Some(&list));
        d.set_extra_child(Some(&scroll));
    }
    // Breiter ab libadwaita 1.6 (Ubuntu 24.04 hat 1.5: Eigenschaft fehlt, Dialog bleibt schmal).
    if d.find_property("prefer-wide-layout").is_some() {
        d.set_property("prefer-wide-layout", true);
    }
    d.add_response("close", tr("Später"));
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

/// Nach dem Update neu starten (Linux): dasselbe Programm mit denselben Angaben, sobald dieser
/// Prozess beendet ist (Tuner, Player und Einstellungen sind dann frei bzw. gespeichert).
fn restart(app: &Rc<App>) {
    let exe = std::env::current_exe()
        .map(|p| {
            // Nach dem Ersetzen durch das Paket meldet Linux „… (deleted)“.
            let s = p.to_string_lossy().to_string();
            std::path::PathBuf::from(s.strip_suffix(" (deleted)").unwrap_or(&s))
        })
        .unwrap_or_else(|_| "tvjoe-gui".into());
    let spawned = std::process::Command::new("sh")
        .arg("-c")
        .arg("while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; shift; exec \"$@\"")
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(&exe)
        .args(std::env::args_os().skip(1))
        .spawn();
    match spawned {
        Ok(_) => app.window.close(),
        Err(e) => {
            let d = alert(app, tr("Neustart nicht möglich"), &e.to_string());
            d.add_response("close", tr("OK"));
        }
    }
}

fn confirm_install(app: &Rc<App>, path: &std::path::Path) {
    let cmd = update::install_command(path);
    // Windows: Installer starten und tvjoe beenden, damit er die Dateien ersetzen kann.
    if cfg!(windows) {
        let d = alert(
            app,
            tr("Installer geprüft"),
            &tf!(
                "Die Prüfsumme (SHA-256) stimmt. tvjoe beendet sich und startet den Installer:\n\n{befehl}\n\nDanach tvjoe wieder öffnen.",
                befehl = cmd.join(" ")
            ),
        );
        d.add_response("close", tr("Abbrechen"));
        d.add_response("install", tr("Installieren"));
        d.set_response_appearance("install", adw::ResponseAppearance::Suggested);
        let weak = Rc::downgrade(app);
        d.connect_response(Some("install"), move |_, _| {
            let Some(app) = weak.upgrade() else { return };
            match std::process::Command::new(&cmd[0]).args(&cmd[1..]).spawn() {
                Ok(_) => app.window.close(),
                Err(e) => {
                    let d = alert(&app, tr("Installation nicht gestartet"), &e.to_string());
                    d.add_response("close", tr("OK"));
                }
            }
        });
        return;
    }
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
            if ok {
                let d = alert(&app, tr("Installiert"), tr("tvjoe jetzt neu starten, damit die neue Fassung läuft?"));
                d.add_response("close", tr("Später"));
                d.add_response("restart", tr("Neu starten"));
                d.set_response_appearance("restart", adw::ResponseAppearance::Suggested);
                let weak = Rc::downgrade(&app);
                d.connect_response(Some("restart"), move |_, _| {
                    if let Some(app) = weak.upgrade() {
                        restart(&app);
                    }
                });
            } else {
                let d = alert(
                    &app,
                    tr("Installation nicht abgeschlossen"),
                    tr("Abgebrochen oder fehlgeschlagen."),
                );
                d.add_response("close", tr("OK"));
            }
        });
    });
}
