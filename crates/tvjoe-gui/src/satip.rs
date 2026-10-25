//! SAT-Quelle (SAT>IP): Reiter in den Einstellungen mit Adresse, „Prüfen“ (Gerät, Tuner,
//! Empfang, Datenstrom), Hilfe für die Firewall und Suchlauf in die SAT-Senderliste.
//! Wiedergabe folgt.

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::satip::{self, CheckResult, ScanProgress};
use tvjoe_core::{Library, Settings, paths};

use crate::app::App;

/// Reiter „SAT>IP“. Gibt die Seite und die Übernahme beim Schließen zurück.
pub fn prefs_page(app: &Rc<App>) -> (adw::PreferencesPage, impl Fn(&mut Settings) + 'static) {
    let s = app.settings().clone();
    let page = adw::PreferencesPage::builder()
        .name("satip")
        .title("SAT>IP")
        .icon_name("network-wireless-signal-excellent-symbolic")
        .build();

    let device = adw::PreferencesGroup::builder()
        .title("Gerät")
        .description(
            "SAT>IP-Empfänger im Netz (z. B. EyeTV Netstream 4Sat); Satellit Astra 19,2° Ost",
        )
        .build();
    let enabled = adw::SwitchRow::builder()
        .title("SAT-Quelle verwenden")
        .active(s.satip_enabled)
        .build();
    let host = adw::EntryRow::builder()
        .title("Adresse des SAT>IP-Geräts (IP-Adresse)")
        .text(&s.satip_host)
        .build();
    let check_row = adw::ActionRow::builder()
        .title("Prüfen")
        .subtitle("Stimmt je Tuner einen Transponder ab, liest den Empfang und prüft, ob der Datenstrom ankommt")
        .build();
    let check = gtk::Button::builder()
        .label("Prüfen")
        .valign(gtk::Align::Center)
        .build();
    check_row.add_suffix(&check);
    device.add(&enabled);
    device.add(&host);
    device.add(&check_row);

    let result = adw::PreferencesGroup::builder()
        .title("Ergebnis")
        .visible(false)
        .build();
    page.add(&device);
    page.add(&result);

    let list = adw::PreferencesGroup::builder()
        .title("Senderliste")
        .description("Frei empfangbare TV-Sender aller Transponder von Astra 19,2° Ost; verschlüsselte Sender und Radio werden ausgelassen. Ein erneuter Suchlauf behält Nummern und eigene Namen.")
        .build();
    let scan_row = adw::ActionRow::builder()
        .title("Suchlauf")
        .subtitle(list_summary())
        .build();
    let scan = gtk::Button::builder()
        .label("Suchlauf starten")
        .valign(gtk::Align::Center)
        .build();
    scan_row.add_suffix(&scan);
    let bar = gtk::ProgressBar::builder()
        .show_text(true)
        .visible(false)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    list.add(&scan_row);
    list.add(&bar);
    page.add(&list);
    {
        let (host, row, bar) = (host.clone(), scan_row.clone(), bar.clone());
        let weak = Rc::downgrade(app);
        scan.connect_clicked(move |b| run_scan(&weak, host.text().trim(), b, &row, &bar));
    }

    let rows: Rc<std::cell::RefCell<Vec<gtk::Widget>>> = Rc::default();
    let run = Rc::new({
        let (result, rows, host, check) =
            (result.clone(), rows.clone(), host.clone(), check.clone());
        move || run_check(&result, &rows, host.text().trim(), &check)
    });
    let r = run.clone();
    check.connect_clicked(move |_| r());

    let apply = move |s: &mut Settings| {
        s.satip_enabled = enabled.is_active();
        s.satip_host = host.text().trim().to_string();
    };
    (page, apply)
}

fn run_check(
    group: &adw::PreferencesGroup,
    rows: &Rc<std::cell::RefCell<Vec<gtk::Widget>>>,
    host: &str,
    button: &gtk::Button,
) {
    if host.is_empty() {
        return;
    }
    for w in rows.borrow_mut().drain(..) {
        group.remove(&w);
    }
    let waiting = adw::ActionRow::builder().title("Prüfe …").build();
    group.add(&waiting);
    rows.borrow_mut().push(waiting.upcast());
    group.set_visible(true);
    button.set_sensitive(false);

    let (tx, rx) = async_channel::bounded(1);
    let h = host.to_string();
    std::thread::spawn(move || {
        let _ = tx.send_blocking(satip::check(&h, Duration::from_secs(3)));
    });
    let (group, rows, button, host) = (
        group.clone(),
        rows.clone(),
        button.clone(),
        host.to_string(),
    );
    glib::spawn_future_local(async move {
        let Ok(result) = rx.recv().await else { return };
        button.set_sensitive(true);
        for w in rows.borrow_mut().drain(..) {
            group.remove(&w);
        }
        let add = |w: gtk::Widget| {
            group.add(&w);
            rows.borrow_mut().push(w);
        };
        match result {
            Ok(r) => show_result(&r, &host, &add, &group, &rows, &button),
            Err(e) => add(adw::ActionRow::builder()
                .title("Gerät nicht erreichbar")
                .subtitle(e)
                .build()
                .upcast()),
        }
    });
}

fn show_result(
    r: &CheckResult,
    host: &str,
    add: &dyn Fn(gtk::Widget),
    group: &adw::PreferencesGroup,
    rows: &Rc<std::cell::RefCell<Vec<gtk::Widget>>>,
    button: &gtk::Button,
) {
    let mut name = r.name.clone().unwrap_or_else(|| "SAT>IP-Gerät".into());
    if let Some(f) = &r.firmware {
        name.push_str(&format!(" · Firmware {f}"));
    }
    add(adw::ActionRow::builder()
        .title(&name)
        .subtitle(format!("{} Tuner", r.tuners))
        .build()
        .upcast());
    for t in &r.status {
        let text = if t.lock {
            format!("Signal {} % · Qualität {}/15", t.level_percent(), t.quality)
        } else {
            "kein Signal (Kabel angeschlossen?)".to_string()
        };
        let row = adw::ActionRow::builder()
            .title(format!("Tuner {} · {} MHz", t.tuner, t.freq))
            .subtitle(text)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name(if t.lock {
            "object-select-symbolic"
        } else {
            "dialog-warning-symbolic"
        }));
        add(row.upcast());
    }
    if r.packets > 0 {
        add(adw::ActionRow::builder()
            .title("Datenstrom kommt an")
            .subtitle(format!("{} Pakete in 3 s", r.packets))
            .build()
            .upcast());
        return;
    }
    // Kein Datenstrom: meist die Firewall (ufw bzw. Windows-Firewall verwirft das eingehende UDP).
    let row = adw::ActionRow::builder()
        .title("Kein Datenstrom angekommen")
        .subtitle(if cfg!(windows) {
            "Die Windows-Firewall verwirft vermutlich die UDP-Pakete des Geräts."
        } else if satip::ufw_active() {
            "Die Firewall (ufw) ist aktiv und verwirft vermutlich die UDP-Pakete des Geräts."
        } else {
            "Eine Firewall oder das Netz verhindert vermutlich die UDP-Pakete des Geräts."
        })
        .build();
    if satip::firewall_fix_available() {
        let fix = gtk::Button::builder()
            .label("Freigabe eintragen…")
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&fix);
        let (host, group, rows, button) = (
            host.to_string(),
            group.clone(),
            rows.clone(),
            button.clone(),
        );
        fix.connect_clicked(move |b| confirm_firewall(b, &host, &group, &rows, &button));
    }
    add(row.upcast());
}

/// Firewall-Regel nach Bestätigung eintragen (pkexec fragt nach dem Passwort), dann erneut prüfen.
fn confirm_firewall(
    parent: &gtk::Button,
    host: &str,
    group: &adw::PreferencesGroup,
    rows: &Rc<std::cell::RefCell<Vec<gtk::Widget>>>,
    button: &gtk::Button,
) {
    let Some(cmd) = satip::firewall_rule_command(host) else {
        return;
    };
    let ask = if cfg!(windows) {
        "Windows fragt über die Benutzerkontensteuerung nach der Bestätigung."
    } else {
        "Das System fragt nach dem Passwort."
    };
    let d = adw::AlertDialog::new(
        Some("Firewall-Freigabe"),
        Some(&format!(
            "Erlaubt wird nur UDP vom SAT>IP-Gerät {host}, sonst nichts:\n\n{}\n\n{ask}",
            satip::firewall_rule_text(host)
        )),
    );
    d.add_response("cancel", "Abbrechen");
    d.add_response("ok", "Eintragen");
    d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    d.set_close_response("cancel");
    let (host, group, rows, button) = (
        host.to_string(),
        group.clone(),
        rows.clone(),
        button.clone(),
    );
    d.connect_response(Some("ok"), move |_, _| {
        let cmd = cmd.clone();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let mut c = std::process::Command::new(&cmd[0]);
            c.args(&cmd[1..]);
            // Windows: die PowerShell im Hintergrund ohne eigenes Konsolenfenster (CREATE_NO_WINDOW).
            #[cfg(windows)]
            std::os::windows::process::CommandExt::creation_flags(&mut c, 0x0800_0000);
            let ok = c.status().is_ok_and(|s| s.success());
            let _ = tx.send_blocking(ok);
        });
        let (host, group, rows, button) =
            (host.clone(), group.clone(), rows.clone(), button.clone());
        glib::spawn_future_local(async move {
            if rx.recv().await == Ok(true) {
                run_check(&group, &rows, &host, &button);
                return;
            }
            let row = adw::ActionRow::builder()
                .title("Freigabe nicht eingetragen")
                .subtitle("Abgebrochen oder vom System abgelehnt. Die Regel lässt sich auch von Hand eintragen (siehe docs/SAT-IP.md).")
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
            group.add(&row);
            rows.borrow_mut().push(row.upcast());
        });
    });
    d.present(Some(parent));
}

/// Stand der gespeicherten SAT-Senderliste für die Unterzeile.
fn list_summary() -> String {
    let lib = paths::satip_library_path()
        .and_then(|p| Library::load(&p).ok())
        .unwrap_or_default();
    match lib.channels.iter().filter(|c| !c.missing).count() {
        0 => "Noch keine Sender gespeichert".to_string(),
        n => format!("{n} Sender gespeichert"),
    }
}

enum ScanMsg {
    Step(&'static str),
    Progress(ScanProgress),
    Done(Result<Vec<satip::ScannedService>, String>),
}

/// Suchlauf im Hintergrund: so viele Tuner parallel, wie beim Prüfen Empfang haben; danach
/// Abgleich mit der gespeicherten SAT-Senderliste.
fn run_scan(
    app: &std::rc::Weak<App>,
    host: &str,
    button: &gtk::Button,
    row: &adw::ActionRow,
    bar: &gtk::ProgressBar,
) {
    if host.is_empty() {
        row.set_subtitle("Bitte zuerst die Adresse des Geräts eintragen");
        return;
    }
    button.set_sensitive(false);
    bar.set_visible(true);
    bar.set_fraction(0.0);
    bar.set_text(Some("Prüfe Tuner …"));
    let (tx, rx) = async_channel::unbounded();
    let h = host.to_string();
    std::thread::spawn(move || {
        let workers = satip::check(&h, Duration::from_secs(1))
            .map(|r| r.status.iter().filter(|t| t.lock).count())
            .unwrap_or(1)
            .max(1);
        let _ = tx.send_blocking(ScanMsg::Step("Lese die Transponderliste …"));
        let progress = |p: ScanProgress| {
            let _ = tx.send_blocking(ScanMsg::Progress(p));
        };
        let result = satip::scan(&h, workers, &progress);
        let _ = tx.send_blocking(ScanMsg::Done(result));
    });
    let (app, button, row, bar, host) = (
        app.clone(),
        button.clone(),
        row.clone(),
        bar.clone(),
        host.to_string(),
    );
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                ScanMsg::Step(t) => bar.set_text(Some(t)),
                ScanMsg::Progress(p) => {
                    bar.set_fraction(p.done as f64 / p.total.max(1) as f64);
                    bar.set_text(Some(&format!(
                        "Transponder {} von {} · {} Dienste gefunden",
                        p.done, p.total, p.services
                    )));
                }
                ScanMsg::Done(result) => {
                    button.set_sensitive(true);
                    bar.set_visible(false);
                    row.set_subtitle(&match result {
                        Ok(services) => {
                            let text = save_scan(&host, &services);
                            if let Some(app) = app.upgrade() {
                                app.reload_sat_list();
                            }
                            text
                        }
                        Err(e) => format!("Suchlauf fehlgeschlagen: {e}"),
                    });
                    break;
                }
            }
        }
    });
}

/// Ergebnis in die SAT-Senderliste übernehmen und als Unterzeile zusammenfassen.
fn save_scan(host: &str, services: &[satip::ScannedService]) -> String {
    let Some(path) = paths::satip_library_path() else {
        return "Kein Konfigurationsordner gefunden".to_string();
    };
    let mut lib = match Library::load(&path) {
        Ok(l) => l,
        Err(e) => return format!("SAT-Senderliste nicht lesbar: {e}"),
    };
    let items = satip::to_channels(host, services);
    let out = lib.sync_scanned("SAT>IP", &format!("satip://{host}"), &items);
    if let Err(e) = lib.save(&path) {
        return format!("SAT-Senderliste nicht gespeichert: {e}");
    }
    let mut text = format!(
        "{} frei empfangbare TV-Sender ({} neu, {} aktualisiert",
        items.len(),
        out.added,
        out.updated
    );
    if out.missing > 0 {
        text.push_str(&format!(", {} nicht mehr gefunden", out.missing));
    }
    text.push(')');
    text
}
