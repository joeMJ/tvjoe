//! SAT-Quelle (SAT>IP): Reiter in den Einstellungen mit Adresse, „Prüfen“ (Gerät, Tuner,
//! Empfang, Datenstrom) und Hilfe für die Firewall. Suchlauf und Wiedergabe folgen.

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::Settings;
use tvjoe_core::satip::{self, CheckResult};

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
    // Kein Datenstrom: unter Ubuntu meist die Firewall (ufw verwirft das eingehende UDP).
    let row = adw::ActionRow::builder()
        .title("Kein Datenstrom angekommen")
        .subtitle(if satip::ufw_active() {
            "Die Firewall (ufw) ist aktiv und verwirft vermutlich die UDP-Pakete des Geräts."
        } else {
            "Eine Firewall oder das Netz verhindert vermutlich die UDP-Pakete des Geräts."
        })
        .build();
    if satip::ufw_active() {
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
    let cmd = satip::ufw_rule_command(host);
    let d = adw::AlertDialog::new(
        Some("Firewall-Freigabe"),
        Some(&format!(
            "Erlaubt wird nur UDP vom SAT>IP-Gerät {host}, sonst nichts:\n\n{}\n\nDas System fragt nach dem Passwort.",
            cmd[1..].join(" ")
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
            let ok = std::process::Command::new(&cmd[0])
                .args(&cmd[1..])
                .status()
                .is_ok_and(|s| s.success());
            let _ = tx.send_blocking(ok);
        });
        let (host, group, rows, button) =
            (host.clone(), group.clone(), rows.clone(), button.clone());
        glib::spawn_future_local(async move {
            if rx.recv().await == Ok(true) {
                run_check(&group, &rows, &host, &button);
            }
        });
    });
    d.present(Some(parent));
}
