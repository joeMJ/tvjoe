//! DVB-Karte unter Linux: Einrichtung auf der Seite ihrer Empfangsquelle (Gerät, Suchlauf in
//! die eigene Senderliste). Experimentell, geprüft nur mit dem virtuellen Gerät `dvb_vidtv`.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::dvb::{self, Device, Plan};
use tvjoe_core::settings::Adapter;
use tvjoe_core::{Library, paths, tf, tr, trn};

use crate::app::App;

/// Gruppen „Gerät“ und „Senderliste“. `set_device` übernimmt das gewählte Gerät (`N.M`).
pub fn device_groups(
    app: &Rc<App>,
    a: &Adapter,
    set_device: Rc<dyn Fn(String)>,
) -> Vec<adw::PreferencesGroup> {
    let devices = Rc::new(dvb::devices());
    let device = adw::PreferencesGroup::builder()
        .title(tr("Gerät"))
        .description(tr("Experimentell: geprüft nur mit dem virtuellen Gerät dvb_vidtv. Die Karte braucht einen Treiber des Kernels, oft auch Firmware; der Benutzer braucht Zugriff auf /dev/dvb (Gruppe „video“)."))
        .build();
    let current = Rc::new(std::cell::RefCell::new(a.host.trim().to_string()));
    if devices.is_empty() {
        device.add(
            &adw::ActionRow::builder()
                .title(tr("Keine DVB-Karte gefunden"))
                .subtitle(tr("/dev/dvb fehlt: Karte angeschlossen, Treiber und Firmware geladen?"))
                .build(),
        );
    } else {
        let names: Vec<String> = devices
            .iter()
            .map(|d| {
                let systems: Vec<&str> = d.systems.iter().map(|s| s.label()).collect();
                tf!(
                    "adapter{id}: {name} ({systems})",
                    id = d.id(),
                    name = d.name,
                    systems = systems.join(", ")
                )
            })
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let pos = devices.iter().position(|d| d.id() == *current.borrow());
        let row = adw::ComboRow::builder()
            .title(tr("Gerät"))
            .model(&gtk::StringList::new(&names))
            .selected(pos.unwrap_or(0) as u32)
            .build();
        if pos.is_none() {
            let id = devices[0].id();
            *current.borrow_mut() = id.clone();
            set_device(id);
        }
        let (devs, cur, set) = (devices.clone(), current.clone(), set_device.clone());
        row.connect_selected_notify(move |r| {
            if let Some(d) = devs.get(r.selected() as usize) {
                *cur.borrow_mut() = d.id();
                set(d.id());
            }
        });
        device.add(&row);
    }

    let list = adw::PreferencesGroup::builder()
        .title(tr("Senderliste"))
        .description(tr("Suchlauf über alle Kanäle des gewählten Plans; verschlüsselte Sender werden ausgelassen. Ein erneuter Suchlauf behält Nummern und eigene Namen."))
        .build();
    // Frequenzpläne (Antenne, Kabel) und Satellit Astra 19,2° Ost über die Netzwerktabelle.
    let mut choices: Vec<Choice> = dvb::plans().into_iter().map(Choice::Plan).collect();
    if let Some(sat) = tvjoe_core::adapter::find("netstream-4sat", None).and_then(|d| d.satellite) {
        choices.push(Choice::Satellite(sat));
    }
    let plans = Rc::new(choices);
    let plan_labels: Vec<String> = plans.iter().map(Choice::label).collect();
    let plan_names: Vec<&str> = plan_labels.iter().map(String::as_str).collect();
    let plan_row = adw::ComboRow::builder()
        .title(tr("Suchlauf über"))
        .model(&gtk::StringList::new(&plan_names))
        .build();
    let radio = adw::SwitchRow::builder()
        .title(tr("Radiosender übernehmen"))
        .build();
    let scan_row = adw::ActionRow::builder()
        .title(tr("Suchlauf"))
        .subtitle(list_summary(&a.id))
        .build();
    let scan = gtk::Button::builder()
        .label(tr("Suchlauf starten"))
        .valign(gtk::Align::Center)
        .sensitive(!devices.is_empty())
        .build();
    scan_row.add_suffix(&scan);
    let bar = gtk::ProgressBar::builder()
        .show_text(true)
        .visible(false)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    list.add(&plan_row);
    list.add(&radio);
    list.add(&scan_row);
    list.add(&bar);
    {
        let (app, id, num) = (Rc::downgrade(app), a.id.clone(), a.num);
        let (row, bar, plan_row, radio) = (scan_row.clone(), bar.clone(), plan_row.clone(), radio.clone());
        scan.connect_clicked(move |b| {
            let Some(dev) = devices.iter().find(|d| d.id() == *current.borrow()).cloned() else {
                return;
            };
            let Some(plan) = plans.get(plan_row.selected() as usize).cloned() else {
                return;
            };
            run_scan(&app, &id, num, dev, plan, radio.is_active(), b, &row, &bar);
        });
    }
    vec![device, list]
}

/// Weg des Suchlaufs.
#[derive(Clone)]
enum Choice {
    Plan(Plan),
    Satellite(tvjoe_core::adapter::Satellite),
}

impl Choice {
    fn label(&self) -> String {
        match self {
            Choice::Plan(p) => p.name.clone(),
            Choice::Satellite(s) => tf!("Satellit {satellite}", satellite = s.name),
        }
    }
}

/// Stand der gespeicherten Senderliste für die Unterzeile.
fn list_summary(adapter_id: &str) -> String {
    let lib = paths::source_library_path(adapter_id)
        .and_then(|p| Library::load(&p).ok())
        .unwrap_or_default();
    match lib.channels.iter().filter(|c| !c.missing).count() {
        0 => tr("Noch keine Sender gespeichert").to_string(),
        n => trn("{n} Sender gespeichert", "{n} Sender gespeichert", n as u64),
    }
}

enum ScanMsg {
    Progress(dvb::Progress),
    Done(Result<Vec<dvb::DvbService>, String>),
}

/// Suchlauf im Hintergrund, danach Abgleich mit der gespeicherten Senderliste.
#[allow(clippy::too_many_arguments)]
fn run_scan(
    app: &std::rc::Weak<App>,
    adapter_id: &str,
    num: u32,
    dev: Device,
    plan: Choice,
    radio: bool,
    button: &gtk::Button,
    row: &adw::ActionRow,
    bar: &gtk::ProgressBar,
) {
    button.set_sensitive(false);
    bar.set_visible(true);
    bar.set_fraction(0.0);
    bar.set_text(Some(tr("Stimme ab …")));
    let (tx, rx) = async_channel::unbounded();
    let scan_dev = dev.clone();
    std::thread::spawn(move || {
        let progress = |p: dvb::Progress| {
            let _ = tx.send_blocking(ScanMsg::Progress(p));
        };
        let result = match plan {
            Choice::Plan(p) => dvb::scan(&scan_dev, &p.tunings, &progress, &|| false),
            Choice::Satellite(sat) => dvb::scan_satellite(&scan_dev, &sat, &progress, &|| false),
        };
        let _ = tx.send_blocking(ScanMsg::Done(result));
    });
    let (app, button, row, bar, id) = (
        app.clone(),
        button.clone(),
        row.clone(),
        bar.clone(),
        adapter_id.to_string(),
    );
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                ScanMsg::Progress((done, total, found)) => {
                    bar.set_fraction(done as f64 / total.max(1) as f64);
                    bar.set_text(Some(&tf!(
                        "Kanal {done} von {total} · {found} Dienste gefunden",
                        done = done,
                        total = total,
                        found = found
                    )));
                }
                ScanMsg::Done(result) => {
                    button.set_sensitive(true);
                    bar.set_visible(false);
                    row.set_subtitle(&match result {
                        Ok(services) => {
                            let text = save_scan(&id, &dev, &services, radio);
                            if let Some(app) = app.upgrade() {
                                if app.active_source() == num {
                                    app.reload_list(num);
                                } else {
                                    app.forget_lists();
                                }
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

/// Ergebnis in die Senderliste der Empfangsquelle übernehmen und zusammenfassen.
fn save_scan(adapter_id: &str, dev: &Device, services: &[dvb::DvbService], radio: bool) -> String {
    let Some(path) = paths::source_library_path(adapter_id) else {
        return tr("Kein Konfigurationsordner gefunden").to_string();
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut lib = match Library::load(&path) {
        Ok(l) => l,
        Err(e) => return tf!("Senderliste nicht lesbar: {error}", error = e),
    };
    let items = dvb::to_channels(&dev.id(), services, radio);
    let out = lib.sync_scanned(&dev.name, &format!("dvb://{}", dev.id()), &items);
    if let Err(e) = lib.save(&path) {
        return tf!("Senderliste nicht gespeichert: {error}", error = e);
    }
    tf!(
        "{total} Sender ({added} neu, {updated} aktualisiert, {missing} nicht mehr gefunden)",
        total = items.len(),
        added = out.added,
        updated = out.updated,
        missing = out.missing
    )
}
