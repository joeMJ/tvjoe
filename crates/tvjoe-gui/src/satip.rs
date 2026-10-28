//! SAT-Quelle (SAT>IP): Einrichtung auf der Seite der Empfangsquelle mit Adresse, „Prüfen“
//! (Gerät, Tuner, Empfang, Datenstrom), Hilfe für die Firewall und Suchlauf in die
//! SAT-Senderliste.

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::satip::{self, CheckResult, ScanProgress};
use tvjoe_core::adapter::{Description, Satellite};
use tvjoe_core::plan::Plan;
use tvjoe_core::settings::Adapter;
use tvjoe_core::{Library, paths, tf, tr, trn};

use crate::app::App;

/// Gerät nur für Kabel bzw. Antenne (keine Satelliten-Empfangsart in der Beschreibung)?
fn terrestrial_or_cable(desc: Option<&Description>) -> bool {
    desc.is_some_and(|d| !d.systems.is_empty() && !d.systems.iter().any(|s| s.starts_with("dvbs")))
}

/// Einrichtung des SAT>IP-Geräts auf der Seite seiner Empfangsquelle: Gruppen „Gerät“
/// (Adresse, Prüfen), „Ergebnis“ und „Senderliste“ (Suchlauf über den Satelliten der
/// Beschreibung). Gibt die Gruppen und das Adressfeld zurück.
pub fn device_groups(
    app: &Rc<App>,
    adapter: &Adapter,
    desc: Option<&Description>,
) -> (Vec<adw::PreferencesGroup>, adw::EntryRow) {
    let satellite = desc.and_then(|d| d.satellite.clone());
    let choices = scan_choices(desc);
    let sat_name = satellite.as_ref().map(|s| s.name.clone()).unwrap_or_default();
    let device = adw::PreferencesGroup::builder()
        .title(tr("Gerät"))
        .description(if satellite.is_some() {
            tf!("SAT>IP-Empfänger im Netz; Satellit {satellite}. Eigene Gerätebeschreibungen liegen im Ordner „adapter“ der Konfiguration.", satellite = sat_name)
        } else if terrestrial_or_cable(desc) {
            // Kabel bzw. Antenne über das SAT>IP-Protokoll (z. B. FRITZ!Box Cable).
            tr("Kabel- bzw. Antennen-Empfänger im Netz (SAT>IP-Protokoll). Eigene Gerätebeschreibungen liegen im Ordner „adapter“ der Konfiguration.").to_string()
        } else {
            tr("SAT>IP-Empfänger im Netz. Eigene Gerätebeschreibungen liegen im Ordner „adapter“ der Konfiguration.").to_string()
        })
        .build();
    let host = adw::EntryRow::builder()
        .title(if terrestrial_or_cable(desc) {
            tr("Adresse des Geräts (IP-Adresse)")
        } else {
            tr("Adresse des SAT>IP-Geräts (IP-Adresse)")
        })
        .text(adapter.host.as_str())
        .build();
    let check_row = adw::ActionRow::builder()
        .title(tr("Prüfen"))
        .subtitle(tr("Stimmt je Tuner einen Transponder ab, liest den Empfang und prüft, ob der Datenstrom ankommt"))
        .build();
    let check = gtk::Button::builder()
        .label(tr("Prüfen"))
        .valign(gtk::Align::Center)
        .build();
    check_row.add_suffix(&check);
    device.add(&host);
    // Prüfen stimmt Transponder von Astra 19,2° Ost ab; nur bei Geräten am Satelliten.
    if satellite.is_some() {
        device.add(&check_row);
    }

    let result = adw::PreferencesGroup::builder()
        .title(tr("Ergebnis"))
        .visible(false)
        .build();

    let from_device = matches!(choices.first(), Some(ScanChoice::Device(_)));
    let list = adw::PreferencesGroup::builder()
        .title(tr("Senderliste"))
        .description(if from_device {
            tr("Die Senderliste kommt vom Gerät; ein erneutes Holen behält Nummern und eigene Namen.").to_string()
        } else if choices.len() == 1 && satellite.is_some() {
            tf!("Frei empfangbare TV-Sender aller Transponder von {satellite}; verschlüsselte Sender und Radio werden ausgelassen. Ein erneuter Suchlauf behält Nummern und eigene Namen.", satellite = sat_name)
        } else {
            tr("Frei empfangbare TV-Sender; verschlüsselte Sender und Radio werden ausgelassen. Ein erneuter Suchlauf behält Nummern und eigene Namen.").to_string()
        })
        .build();
    let choice_row = adw::ComboRow::builder()
        .title(tr("Suchlauf über"))
        .model(&gtk::StringList::new(
            &choices.iter().map(ScanChoice::label).collect::<Vec<_>>().iter().map(String::as_str).collect::<Vec<_>>(),
        ))
        .build();
    if choices.len() > 1 {
        list.add(&choice_row);
    }
    let scan_row = adw::ActionRow::builder()
        .title(if from_device { tr("Senderliste") } else { tr("Suchlauf") })
        .subtitle(list_summary(&adapter.id))
        .build();
    let scan = gtk::Button::builder()
        .label(if from_device {
            tr("Vom Gerät holen")
        } else {
            tr("Suchlauf starten")
        })
        .valign(gtk::Align::Center)
        .sensitive(!choices.is_empty())
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
    {
        let (host, row, bar, choice_row) =
            (host.clone(), scan_row.clone(), bar.clone(), choice_row.clone());
        let weak = Rc::downgrade(app);
        let (id, num) = (adapter.id.clone(), adapter.num);
        scan.connect_clicked(move |b| {
            let Some(choice) = choices.get(choice_row.selected() as usize).cloned() else {
                return;
            };
            run_scan(&weak, &id, num, host.text().trim(), choice, b, &row, &bar)
        });
    }

    let rows: Rc<std::cell::RefCell<Vec<gtk::Widget>>> = Rc::default();
    let run = Rc::new({
        let (result, rows, host, check) =
            (result.clone(), rows.clone(), host.clone(), check.clone());
        move || run_check(&result, &rows, host.text().trim(), &check)
    });
    let r = run.clone();
    check.connect_clicked(move |_| r());
    (vec![device, result, list], host)
}

/// Wege zur Senderliste eines SAT>IP-Geräts.
#[derive(Clone)]
enum ScanChoice {
    /// Suchlauf über die Netzwerktabelle eines Satelliten.
    Satellite(Satellite),
    /// Suchlauf über einen Frequenzplan (Antenne, Kabel).
    Plan(Plan),
    /// Senderliste vom Gerät (M3U-Adressen mit `{host}`).
    Device(Vec<String>),
}

impl ScanChoice {
    fn label(&self) -> String {
        match self {
            ScanChoice::Satellite(s) => tf!("Satellit {satellite}", satellite = s.name),
            ScanChoice::Plan(p) => p.name.clone(),
            ScanChoice::Device(_) => tr("Senderliste vom Gerät").to_string(),
        }
    }
}

fn scan_choices(desc: Option<&Description>) -> Vec<ScanChoice> {
    let Some(d) = desc else { return Vec::new() };
    if !d.channel_list.is_empty() {
        return vec![ScanChoice::Device(d.channel_list.clone())];
    }
    let mut out = Vec::new();
    if let Some(s) = &d.satellite {
        out.push(ScanChoice::Satellite(s.clone()));
    }
    let systems = d.plan_systems();
    for p in tvjoe_core::plan::plans() {
        if p.tunings.first().is_some_and(|t| systems.contains(&t.system)) {
            out.push(ScanChoice::Plan(p));
        }
    }
    out
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
    let waiting = adw::ActionRow::builder().title(tr("Prüfe …")).build();
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
                .title(tr("Gerät nicht erreichbar"))
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
    let mut name = r.name.clone().unwrap_or_else(|| tr("SAT>IP-Gerät").into());
    if let Some(f) = &r.firmware {
        name = tf!("{name} · Firmware {firmware}", name = name, firmware = f);
    }
    add(adw::ActionRow::builder()
        .title(&name)
        .subtitle(trn("{n} Tuner", "{n} Tuner", u64::from(r.tuners)))
        .build()
        .upcast());
    for t in &r.status {
        let text = if t.lock {
            tf!(
                "Signal {percent} % · Qualität {quality}/15",
                percent = t.level_percent(),
                quality = t.quality
            )
        } else {
            tr("kein Signal (Kabel angeschlossen?)").to_string()
        };
        let row = adw::ActionRow::builder()
            .title(tf!("Tuner {tuner} · {freq} MHz", tuner = t.tuner, freq = t.freq))
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
            .title(tr("Datenstrom kommt an"))
            .subtitle(tf!("{n} Pakete in 3 s", n = r.packets))
            .build()
            .upcast());
        return;
    }
    // Kein Datenstrom: meist die Firewall (ufw bzw. Windows-Firewall verwirft das eingehende UDP).
    let row = adw::ActionRow::builder()
        .title(tr("Kein Datenstrom angekommen"))
        .subtitle(if cfg!(windows) {
            tr("Die Windows-Firewall verwirft vermutlich die UDP-Pakete des Geräts.")
        } else if satip::ufw_active() {
            tr("Die Firewall (ufw) ist aktiv und verwirft vermutlich die UDP-Pakete des Geräts.")
        } else {
            tr("Eine Firewall oder das Netz verhindert vermutlich die UDP-Pakete des Geräts.")
        })
        .build();
    if satip::firewall_fix_available() {
        let fix = gtk::Button::builder()
            .label(tr("Freigabe eintragen…"))
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
        tr("Windows fragt über die Benutzerkontensteuerung nach der Bestätigung.")
    } else {
        tr("Das System fragt nach dem Passwort.")
    };
    let d = adw::AlertDialog::new(
        Some(tr("Firewall-Freigabe")),
        Some(&tf!(
            "Erlaubt wird nur UDP vom SAT>IP-Gerät {host}, sonst nichts:\n\n{rule}\n\n{ask}",
            host = host,
            rule = satip::firewall_rule_text(host),
            ask = ask
        )),
    );
    d.add_response("cancel", tr("Abbrechen"));
    d.add_response("ok", tr("Eintragen"));
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
                .title(tr("Freigabe nicht eingetragen"))
                .subtitle(tr("Abgebrochen oder vom System abgelehnt. Die Regel lässt sich auch von Hand eintragen (siehe docs/SAT-IP.md)."))
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
            group.add(&row);
            rows.borrow_mut().push(row.upcast());
        });
    });
    d.present(Some(parent));
}

/// Stand der gespeicherten SAT-Senderliste für die Unterzeile.
fn list_summary(adapter_id: &str) -> String {
    let lib = paths::source_library_path(adapter_id)
        .and_then(|p| Library::load(&p).ok())
        .unwrap_or_default();
    match lib.channels.iter().filter(|c| !c.missing).count() {
        0 => tr("Noch keine Sender gespeichert").to_string(),
        n => trn("{n} Sender gespeichert", "{n} Sender gespeichert", n as u64),
    }
}

/// Ergebnis eines Suchlaufs: gefundene Dienste bzw. fertige Sender (Liste vom Gerät).
enum Found {
    Services(Vec<satip::ScannedService>),
    Channels(Vec<tvjoe_core::ScannedChannel>),
}

enum ScanMsg {
    Step(&'static str),
    Progress(ScanProgress),
    Done(Result<Found, String>),
}

/// Suchlauf im Hintergrund: so viele Tuner parallel, wie beim Prüfen Empfang haben; danach
/// Abgleich mit der gespeicherten Senderliste der Empfangsquelle.
#[allow(clippy::too_many_arguments)]
fn run_scan(
    app: &std::rc::Weak<App>,
    adapter_id: &str,
    num: u32,
    host: &str,
    choice: ScanChoice,
    button: &gtk::Button,
    row: &adw::ActionRow,
    bar: &gtk::ProgressBar,
) {
    if host.is_empty() {
        row.set_subtitle(tr("Bitte zuerst die Adresse des Geräts eintragen"));
        return;
    }
    button.set_sensitive(false);
    bar.set_visible(true);
    bar.set_fraction(0.0);
    bar.set_text(Some(tr("Prüfe Tuner …")));
    let (tx, rx) = async_channel::unbounded();
    let h = host.to_string();
    std::thread::spawn(move || {
        let progress = |p: ScanProgress| {
            let _ = tx.send_blocking(ScanMsg::Progress(p));
        };
        let result = match choice {
            ScanChoice::Device(urls) => satip::channel_list(&h, &urls).map(Found::Channels),
            ScanChoice::Satellite(sat) => {
                let workers = satip::check(&h, Duration::from_secs(1))
                    .map(|r| r.status.iter().filter(|t| t.lock).count())
                    .unwrap_or(1)
                    .max(1);
                let _ = tx.send_blocking(ScanMsg::Step(tr("Lese die Transponderliste …")));
                satip::scan(&h, &sat, workers, &progress).map(Found::Services)
            }
            ScanChoice::Plan(plan) => {
                satip::scan_plan(&h, &plan.tunings, 1, &progress).map(Found::Services)
            }
        };
        let _ = tx.send_blocking(ScanMsg::Done(result));
    });
    let (app, button, row, bar, host, id) = (
        app.clone(),
        button.clone(),
        row.clone(),
        bar.clone(),
        host.to_string(),
        adapter_id.to_string(),
    );
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                ScanMsg::Step(t) => bar.set_text(Some(t)),
                ScanMsg::Progress(p) => {
                    bar.set_fraction(p.done as f64 / p.total.max(1) as f64);
                    bar.set_text(Some(&tf!(
                        "Transponder {done} von {total} · {services} Dienste gefunden",
                        done = p.done,
                        total = p.total,
                        services = p.services
                    )));
                }
                ScanMsg::Done(result) => {
                    button.set_sensitive(true);
                    bar.set_visible(false);
                    row.set_subtitle(&match result {
                        Ok(found) => {
                            let text = save_scan(&id, &host, found);
                            if let Some(app) = app.upgrade() {
                                app.reload_list(num);
                            }
                            text
                        }
                        Err(e) => tf!("Suchlauf fehlgeschlagen: {error}", error = e),
                    });
                    break;
                }
            }
        }
    });
}

/// Ergebnis in die SAT-Senderliste übernehmen und als Unterzeile zusammenfassen.
fn save_scan(adapter_id: &str, host: &str, found: Found) -> String {
    let Some(path) = paths::source_library_path(adapter_id) else {
        return tr("Kein Konfigurationsordner gefunden").to_string();
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut lib = match Library::load(&path) {
        Ok(l) => l,
        Err(e) => return tf!("SAT-Senderliste nicht lesbar: {error}", error = e),
    };
    let items = match found {
        Found::Services(services) => satip::to_channels(host, &services),
        Found::Channels(channels) => channels,
    };
    let out = lib.sync_scanned("SAT>IP", &format!("satip://{host}"), &items);
    if let Err(e) = lib.save(&path) {
        return tf!("SAT-Senderliste nicht gespeichert: {error}", error = e);
    }
    if out.missing > 0 {
        tf!(
            "{total} frei empfangbare TV-Sender ({added} neu, {updated} aktualisiert, {missing} nicht mehr gefunden)",
            total = items.len(),
            added = out.added,
            updated = out.updated,
            missing = out.missing
        )
    } else {
        tf!(
            "{total} frei empfangbare TV-Sender ({added} neu, {updated} aktualisiert)",
            total = items.len(),
            added = out.added,
            updated = out.updated
        )
    }
}
