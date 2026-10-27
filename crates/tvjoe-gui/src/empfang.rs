//! Reiter „Empfang“ in den Einstellungen (Nutzer, 2026-10-27): die Empfangsquellen (zunächst
//! Internet-TV aus M3U-Listen und SAT>IP), je an- und abschaltbar und jede für sich auf
//! einer eigenen Seite eingerichtet, dazu die XMLTV-Quellen, die einmal angelegt und
//! betitelt werden und aus denen jede Empfangsquelle keine, eine oder mehrere nutzt.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use tvjoe_core::adapter::{self, Description, Driver};
use tvjoe_core::settings::{ADAPTER_SAT, ADAPTER_TV, Adapter, XmltvSource};
use tvjoe_core::{Settings, paths, tf, tr, trn};

use crate::app::App;

/// Bearbeiteter Stand, beim Schließen der Einstellungen übernommen.
#[derive(Clone)]
struct Edit {
    adapters: Vec<Adapter>,
    xmltv: Vec<XmltvSource>,
}

type State = Rc<RefCell<Edit>>;

/// Beschreibungen der Empfangsquellen (eingebaute und eigene aus `adapter/`).
fn descriptions() -> Vec<Description> {
    let dir = paths::config_dir().map(|d| d.join("adapter"));
    let (all, notes) = adapter::all(dir.as_deref());
    for n in &notes {
        crate::trace(&format!("Adapterbeschreibung {n}"));
    }
    all
}

/// Anzeigename einer Empfangsquelle.
fn adapter_title(a: &Adapter, descs: &[Description]) -> String {
    if a.id == ADAPTER_TV {
        return tr("Internet-TV").to_string();
    }
    descs
        .iter()
        .find(|d| d.id == a.kind)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| a.kind.clone())
}

/// Reiter „Empfang“. Gibt die Seite und die Übernahme beim Schließen zurück.
pub fn prefs_page(
    app: &Rc<App>,
    dialog: &adw::PreferencesDialog,
) -> (adw::PreferencesPage, impl Fn(&mut Settings) + 'static) {
    let state: State = {
        let mut s = app.settings().clone();
        s.migrate();
        Rc::new(RefCell::new(Edit {
            adapters: s.adapters.clone(),
            xmltv: s.xmltv_sources.clone(),
        }))
    };
    let descs = Rc::new(descriptions());
    let page = adw::PreferencesPage::builder()
        .name("empfang")
        .title(tr("Empfang"))
        .icon_name("network-wireless-signal-excellent-symbolic")
        .build();

    let sources = adw::PreferencesGroup::builder()
        .title(tr("Empfangsquellen"))
        .description(tr("Jede Quelle hat ihre eigene Senderliste, Sortierung und ihr eigenes Programm. Ausgeschaltet verschwindet ihr Knopf in der Kopfleiste, und im Hintergrund läuft nichts für sie."))
        .build();
    for id in [ADAPTER_TV, ADAPTER_SAT] {
        let a = state.borrow().adapters.iter().find(|a| a.id == id).cloned();
        let Some(a) = a else { continue };
        let row = adw::ActionRow::builder()
            .title(adapter_title(&a, &descs))
            .subtitle(source_summary(app, &a))
            .activatable(true)
            .build();
        let switch = gtk::Switch::builder()
            .active(a.enabled)
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&switch);
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        {
            let state = state.clone();
            switch.connect_active_notify(move |sw| {
                if let Some(a) = state.borrow_mut().adapters.iter_mut().find(|a| a.id == id) {
                    a.enabled = sw.is_active();
                }
            });
        }
        let (app, dialog, state, descs) =
            (app.clone(), dialog.clone(), state.clone(), descs.clone());
        row.connect_activated(move |row| {
            let page = adapter_page(&app, &dialog, &state, &descs, id, row, &switch);
            dialog.push_subpage(&page);
        });
        sources.add(&row);
    }
    page.add(&sources);
    page.add(&xmltv_group(&state));

    let apply = move |s: &mut Settings| {
        let e = state.borrow();
        s.adapters = e.adapters.clone();
        s.xmltv_sources = e.xmltv.clone();
    };
    (page, apply)
}

/// Unterzeile einer Empfangsquelle: Art, ggf. Gerät, Zahl der Sender.
fn source_summary(app: &App, a: &Adapter) -> String {
    let sat = a.id == ADAPTER_SAT;
    let n = app.list_len(sat) as u64;
    let channels = trn("{n} Sender", "{n} Sender", n);
    if sat {
        if a.host.trim().is_empty() {
            tf!("SAT>IP · noch keine Adresse · {channels}", channels = channels)
        } else {
            tf!("SAT>IP · {host} · {channels}", host = a.host.trim(), channels = channels)
        }
    } else {
        tf!("M3U-Listen · {channels}", channels = channels)
    }
}

/// Seite einer Empfangsquelle.
fn adapter_page(
    app: &Rc<App>,
    dialog: &adw::PreferencesDialog,
    state: &State,
    descs: &Rc<Vec<Description>>,
    id: &'static str,
    list_row: &adw::ActionRow,
    list_switch: &gtk::Switch,
) -> adw::NavigationPage {
    let a = state
        .borrow()
        .adapters
        .iter()
        .find(|a| a.id == id)
        .cloned()
        .unwrap_or_default();
    let sat = id == ADAPTER_SAT;
    let content = adw::PreferencesPage::new();
    // Änderung an der Empfangsquelle in den bearbeiteten Stand schreiben.
    let edit = {
        let state = state.clone();
        move |f: &dyn Fn(&mut Adapter)| {
            if let Some(a) = state.borrow_mut().adapters.iter_mut().find(|a| a.id == id) {
                f(a);
            }
        }
    };
    let edit = Rc::new(edit);

    let general = adw::PreferencesGroup::new();
    let enabled = adw::SwitchRow::builder()
        .title(tr("Eingeschaltet"))
        .active(list_switch.is_active())
        .build();
    // Schalter hier und in der Übersicht gleich halten.
    enabled
        .bind_property("active", list_switch, "active")
        .bidirectional()
        .build();
    general.add(&enabled);
    if sat {
        let choices: Vec<&Description> =
            descs.iter().filter(|d| d.driver == Driver::Satip).collect();
        let names: Vec<String> = choices
            .iter()
            .map(|d| {
                if d.builtin {
                    d.name.clone()
                } else {
                    tf!("{name} (eigene Beschreibung)", name = d.name)
                }
            })
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let device = adw::ComboRow::builder()
            .title(tr("Gerätebeschreibung"))
            .model(&gtk::StringList::new(&names))
            .selected(choices.iter().position(|d| d.id == a.kind).unwrap_or(0) as u32)
            .build();
        let kinds: Vec<String> = choices.iter().map(|d| d.id.clone()).collect();
        let e = edit.clone();
        device.connect_selected_notify(move |row| {
            if let Some(k) = kinds.get(row.selected() as usize) {
                let k = k.clone();
                e(&|a| a.kind = k.clone());
            }
        });
        general.add(&device);
    }
    content.add(&general);

    if sat {
        let satellite = descs
            .iter()
            .find(|d| d.id == a.kind)
            .and_then(|d| d.satellite.clone());
        let (groups, host) = crate::satip::device_groups(app, &a.host, satellite);
        let e = edit.clone();
        host.connect_changed(move |h| {
            let text = h.text().trim().to_string();
            e(&|a| a.host = text.clone());
        });
        for g in groups {
            content.add(&g);
        }
    } else {
        content.add(&lists_group(app, dialog, a.enabled));
    }

    // Programm: aus dem Signal (SAT) und/oder aus XMLTV, beides freiwillig.
    let guide = adw::PreferencesGroup::builder()
        .title(tr("Programm (EPG)"))
        .description(if sat {
            tr("Aus dem Signal (je Sender in „Sender verwalten“ → EPG „Aus dem SAT-Empfang“) und/oder aus XMLTV-Quellen")
        } else {
            tr("Aus XMLTV-Quellen; ohne Quelle laufen die Sender ohne Programm")
        })
        .build();
    if sat {
        let signal = adw::SpinRow::with_range(1.0, 24.0, 1.0);
        signal.set_title(tr("Aus dem Signal sammeln alle"));
        signal.set_subtitle(tr("Stunden; belegt je Transponder etwa 40 Sekunden einen Tuner"));
        signal.set_value(f64::from(a.signal_epg_hours.max(1)));
        let e = edit.clone();
        signal.connect_value_notify(move |r| {
            let v = r.value() as u32;
            e(&|a| a.signal_epg_hours = v);
        });
        guide.add(&signal);
    }
    let xmltv = state.borrow().xmltv.clone();
    if xmltv.is_empty() {
        guide.add(
            &adw::ActionRow::builder()
                .title(tr("Noch keine XMLTV-Quelle angelegt"))
                .subtitle(tr("Anlegen unter Empfang → XMLTV-Quellen"))
                .build(),
        );
    }
    for x in xmltv {
        let row = adw::SwitchRow::builder()
            .title(&x.title)
            .subtitle(&x.location)
            .active(a.xmltv.contains(&x.id))
            .build();
        let e = edit.clone();
        row.connect_active_notify(move |r| {
            let on = r.is_active();
            e(&|a| {
                a.xmltv.retain(|i| *i != x.id);
                if on {
                    a.xmltv.push(x.id);
                }
            });
        });
        guide.add(&row);
    }
    let hours = adw::SpinRow::with_range(0.0, 168.0, 1.0);
    hours.set_title(tr("XMLTV neu laden nach"));
    hours.set_subtitle(tr("Stunden; 0 = nur nach Änderungen an den XMLTV-Quellen"));
    hours.set_value(f64::from(a.xmltv_hours));
    let e = edit.clone();
    hours.connect_value_notify(move |r| {
        let v = r.value() as u32;
        e(&|a| a.xmltv_hours = v);
    });
    guide.add(&hours);
    content.add(&guide);

    let view = adw::PreferencesGroup::builder().title(tr("Anzeige")).build();
    let logos = adw::SwitchRow::builder()
        .title(tr("Senderlogos (schwarzweiß)"))
        .subtitle(tr("In der Senderanzeige, der Info und der Senderliste"))
        .active(a.logos)
        .build();
    let e = edit.clone();
    logos.connect_active_notify(move |r| {
        let on = r.is_active();
        e(&|a| a.logos = on);
    });
    view.add(&logos);
    content.add(&view);

    let record = adw::PreferencesGroup::builder().title(tr("Aufnahme")).build();
    let max = adw::SpinRow::with_range(0.0, if sat { 8.0 } else { 20.0 }, 1.0);
    max.set_title(tr("Gleichzeitig höchstens"));
    max.set_subtitle(if sat {
        tr("Aufnahmen; jede belegt einen Tuner (bei gleichem Transponder geteilt); 0 = ohne Grenze")
    } else {
        tr("Aufnahmen; 0 = ohne Grenze")
    });
    max.set_value(f64::from(a.record_max));
    let e = edit.clone();
    max.connect_value_notify(move |r| {
        let v = r.value() as u32;
        e(&|a| a.record_max = v);
    });
    record.add(&max);
    content.add(&record);

    // Unterzeile in der Übersicht nach der Rückkehr auffrischen (Adresse).
    let title = list_row.title().to_string();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content));
    let nav = adw::NavigationPage::builder()
        .title(&title)
        .child(&toolbar)
        .build();
    let (app, state, row) = (app.clone(), state.clone(), list_row.clone());
    nav.connect_hidden(move |_| {
        if let Some(a) = state.borrow().adapters.iter().find(|a| a.id == id) {
            row.set_subtitle(&source_summary(&app, a));
        }
    });
    nav
}

/// Internet-TV: die M3U-Listen der Senderliste, Übernahme aus Datei oder URL.
fn lists_group(app: &Rc<App>, dialog: &adw::PreferencesDialog, enabled: bool) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(tr("M3U-Listen"))
        .description(tr("Übernahme mit Vorschau und Prüfung in der Senderverwaltung; „Entfernen“ nimmt die Liste samt ihren Sendern aus der Senderliste"))
        .build();
    let file = gtk::Button::builder()
        .label(tr("Datei…"))
        .tooltip_text(tr("Liste aus einer Datei übernehmen"))
        .sensitive(enabled)
        .build();
    let url = gtk::Button::builder()
        .label(tr("URL…"))
        .tooltip_text(tr("Liste von einer Adresse übernehmen"))
        .sensitive(enabled)
        .build();
    let buttons = gtk::Box::builder()
        .spacing(6)
        .valign(gtk::Align::Center)
        .build();
    buttons.append(&file);
    buttons.append(&url);
    group.set_header_suffix(Some(&buttons));
    if !enabled {
        group.set_description(Some(tr("Zum Übernehmen erst die Empfangsquelle einschalten")));
    }
    let rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
    fill_lists(app, &group, &rows);
    for (button, from_url) in [(file, false), (url, true)] {
        let (app, dialog) = (app.clone(), dialog.clone());
        button.connect_clicked(move |_| {
            // Erst die Einstellungen schließen (übernimmt sie), dann die Übernahme öffnen.
            dialog.close();
            crate::manage::import_tv(&app, from_url);
        });
    }
    group
}

fn fill_lists(app: &Rc<App>, group: &adw::PreferencesGroup, rows: &Rc<RefCell<Vec<adw::ActionRow>>>) {
    for r in rows.borrow_mut().drain(..) {
        group.remove(&r);
    }
    let sources = app.with_list(false, |lib| {
        lib.sources
            .iter()
            .map(|s| {
                let n = lib.channels.iter().filter(|c| c.source == s.id).count();
                (s.id, s.name.clone(), s.location.clone(), n)
            })
            .collect::<Vec<_>>()
    });
    if sources.is_empty() {
        let row = adw::ActionRow::builder()
            .title(tr("Noch keine Liste übernommen"))
            .build();
        group.add(&row);
        rows.borrow_mut().push(row);
    }
    for (id, name, location, n) in sources {
        let row = adw::ActionRow::builder()
            .title(&name)
            .subtitle(tf!(
                "{location} · {channels}",
                location = location,
                channels = trn("{n} Sender", "{n} Sender", n as u64)
            ))
            .build();
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text(tr("Entfernen"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        row.add_suffix(&remove);
        let (app, group2, rows2) = (app.clone(), group.clone(), rows.clone());
        remove.connect_clicked(move |b| {
            let d = adw::AlertDialog::new(
                Some(&tf!("„{name}“ entfernen?", name = name)),
                Some(&trn(
                    "Die Liste und ihr Sender verschwinden aus der Senderliste. Eine erneute Übernahme fügt sie wieder hinzu.",
                    "Die Liste und ihre {n} Sender verschwinden aus der Senderliste. Eine erneute Übernahme fügt sie wieder hinzu.",
                    n as u64,
                )),
            );
            d.add_response("cancel", tr("Abbrechen"));
            d.add_response("remove", tr("Entfernen"));
            d.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
            d.set_close_response("cancel");
            let (app, group, rows) = (app.clone(), group2.clone(), rows2.clone());
            d.connect_response(Some("remove"), move |_, _| {
                app.remove_tv_source(id);
                fill_lists(&app, &group, &rows);
            });
            d.present(Some(b));
        });
        group.add(&row);
        rows.borrow_mut().push(row);
    }
}

/// XMLTV-Quellen: anlegen, umbenennen, entfernen.
fn xmltv_group(state: &State) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(tr("XMLTV-Quellen"))
        .description(tr("Einmal anlegen und betiteln; jede Empfangsquelle wählt auf ihrer Seite, welche sie nutzt"))
        .build();
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text(tr("XMLTV-Quelle anlegen"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    group.set_header_suffix(Some(&add));
    let rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
    fill_xmltv(state, &group, &rows);
    let (state, g, rows2) = (state.clone(), group.clone(), rows.clone());
    add.connect_clicked(move |b| edit_xmltv(b, &state, None, &g, &rows2));
    group
}

fn fill_xmltv(state: &State, group: &adw::PreferencesGroup, rows: &Rc<RefCell<Vec<adw::ActionRow>>>) {
    for r in rows.borrow_mut().drain(..) {
        group.remove(&r);
    }
    let list = state.borrow().xmltv.clone();
    if list.is_empty() {
        let row = adw::ActionRow::builder()
            .title(tr("Noch keine XMLTV-Quelle"))
            .subtitle(tr("Datei oder URL einer XMLTV-Liste (auch .xml.gz); „+“ legt eine an"))
            .build();
        group.add(&row);
        rows.borrow_mut().push(row);
    }
    for x in list {
        let users: Vec<&str> = state
            .borrow()
            .adapters
            .iter()
            .filter(|a| a.xmltv.contains(&x.id))
            .map(|a| if a.id == ADAPTER_SAT { "SAT" } else { "TV" })
            .collect();
        let subtitle = if users.is_empty() {
            tf!("{location} · nicht genutzt", location = x.location)
        } else {
            tf!("{location} · genutzt von {sources}", location = x.location, sources = users.join(", "))
        };
        let row = adw::ActionRow::builder()
            .title(&x.title)
            .subtitle(subtitle)
            .build();
        let edit = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text(tr("Bearbeiten"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text(tr("Entfernen"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        row.add_suffix(&edit);
        row.add_suffix(&remove);
        {
            let (state, group, rows, x) = (state.clone(), group.clone(), rows.clone(), x.clone());
            edit.connect_clicked(move |b| edit_xmltv(b, &state, Some(&x), &group, &rows));
        }
        {
            let (state, group, rows) = (state.clone(), group.clone(), rows.clone());
            remove.connect_clicked(move |_| {
                {
                    let mut e = state.borrow_mut();
                    e.xmltv.retain(|s| s.id != x.id);
                    for a in &mut e.adapters {
                        a.xmltv.retain(|i| *i != x.id);
                    }
                }
                fill_xmltv(&state, &group, &rows);
            });
        }
        group.add(&row);
        rows.borrow_mut().push(row);
    }
}

/// XMLTV-Quelle anlegen (`old` = None) oder bearbeiten.
fn edit_xmltv(
    parent: &impl IsA<gtk::Widget>,
    state: &State,
    old: Option<&XmltvSource>,
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
) {
    let title = gtk::Entry::builder()
        .placeholder_text(tr("Titel (frei wählbar)"))
        .text(old.map(|x| x.title.as_str()).unwrap_or(""))
        .build();
    let location = gtk::Entry::builder()
        .placeholder_text(tr("Datei oder URL (https://…/epg.xml.gz)"))
        .text(old.map(|x| x.location.as_str()).unwrap_or(""))
        .build();
    let fields = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    fields.append(&title);
    fields.append(&location);
    let d = adw::AlertDialog::new(
        Some(if old.is_some() {
            tr("XMLTV-Quelle bearbeiten")
        } else {
            tr("XMLTV-Quelle anlegen")
        }),
        None,
    );
    d.set_extra_child(Some(&fields));
    d.add_response("cancel", tr("Abbrechen"));
    d.add_response("ok", tr("Übernehmen"));
    d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    d.set_close_response("cancel");
    let old_id = old.map(|x| x.id);
    let (state, group, rows) = (state.clone(), group.clone(), rows.clone());
    d.connect_response(Some("ok"), move |_, _| {
        let loc = location.text().trim().to_string();
        if loc.is_empty() {
            return;
        }
        let t = title.text().trim().to_string();
        let t = if t.is_empty() {
            tvjoe_core::settings::xmltv_title(&loc)
        } else {
            t
        };
        {
            let mut e = state.borrow_mut();
            match old_id.and_then(|id| e.xmltv.iter_mut().find(|x| x.id == id)) {
                Some(x) => {
                    x.title = t;
                    x.location = loc;
                }
                None => {
                    let id = e.xmltv.iter().map(|x| x.id).max().unwrap_or(0) + 1;
                    e.xmltv.push(XmltvSource {
                        id,
                        title: t,
                        location: loc,
                    });
                }
            }
        }
        fill_xmltv(&state, &group, &rows);
    });
    d.present(Some(parent));
}
