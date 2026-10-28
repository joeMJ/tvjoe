//! Reiter „Empfang“ in den Einstellungen (Nutzer, 2026-10-27): die Empfangsquellen
//! (Internet-TV aus M3U-Listen, SAT>IP, DVB-Karten), je an- und abschaltbar und jede für sich
//! auf einer eigenen Seite eingerichtet, dazu die XMLTV-Quellen, die einmal angelegt und
//! betitelt werden und aus denen jede Empfangsquelle keine, eine oder mehrere nutzt.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::adapter::{self, Description, Driver};
use tvjoe_core::settings::{ADAPTER_TV, Adapter, SRC_SAT, XmltvSource};
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

/// Beschreibung einer Empfangsquelle.
fn description_of<'a>(a: &Adapter, descs: &'a [Description]) -> Option<&'a Description> {
    descs.iter().find(|d| d.id == a.kind)
}

/// Treiber einer Empfangsquelle.
fn driver_of(a: &Adapter, descs: &[Description]) -> Driver {
    description_of(a, descs).map_or(Driver::Iptv, |d| d.driver)
}

/// Anzeigename einer Empfangsquelle.
fn adapter_title(a: &Adapter, descs: &[Description]) -> String {
    if a.id == ADAPTER_TV {
        return tr("Internet-TV").to_string();
    }
    description_of(a, descs)
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
        let mut adapters = s.adapters.clone();
        adapters.sort_by_key(|a| a.num);
        Rc::new(RefCell::new(Edit {
            adapters,
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
    // Hinzufügen: jede Beschreibung außer der Internet-TV-Grundliste; DVB-Karten nur unter Linux.
    let addable: Vec<Description> = descs
        .iter()
        .filter(|d| d.id != "iptv-m3u")
        .filter(|d| d.driver != Driver::LinuxDvb || cfg!(target_os = "linux"))
        .cloned()
        .collect();
    // Eigene Unterseite statt Aufklappmenü: ein Popover im Dialog brachte GTK unter Windows zum
    // Absturz.
    if !addable.is_empty() {
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(tr("Empfangsquelle hinzufügen"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let (app, dialog, state, descs, group) = (
            app.clone(),
            dialog.clone(),
            state.clone(),
            descs.clone(),
            sources.clone(),
        );
        add.connect_clicked(move |_| {
            let page = add_page(&app, &dialog, &state, &descs, &group, &addable);
            dialog.push_subpage(&page);
        });
        sources.set_header_suffix(Some(&add));
    }
    let nums: Vec<u32> = state.borrow().adapters.iter().map(|a| a.num).collect();
    for num in nums {
        source_row(app, dialog, &state, &descs, &sources, num);
    }
    page.add(&sources);
    page.add(&xmltv_group(&state));

    let apply = move |s: &mut Settings| {
        let e = state.borrow();
        // Entfernte Empfangsquellen: Senderliste und Programmdaten aus dem Signal löschen.
        for old in &s.adapters {
            if old.num > SRC_SAT && !e.adapters.iter().any(|a| a.num == old.num) {
                if let Some(p) = paths::source_library_path(&old.id) {
                    let _ = std::fs::remove_file(p);
                }
                if let Some(d) = paths::cache_dir() {
                    let _ = std::fs::remove_file(tvjoe_core::epg::signal_cache_path(&d, &old.id));
                }
            }
        }
        s.adapters = e.adapters.clone();
        s.xmltv_sources = e.xmltv.clone();
    };
    (page, apply)
}

/// Unterseite „Empfangsquelle hinzufügen“: je Beschreibung eine Zeile; die Wahl legt die
/// Empfangsquelle an und öffnet ihre Seite.
fn add_page(
    app: &Rc<App>,
    dialog: &adw::PreferencesDialog,
    state: &State,
    descs: &Rc<Vec<Description>>,
    group: &adw::PreferencesGroup,
    addable: &[Description],
) -> adw::NavigationPage {
    let content = adw::PreferencesPage::new();
    let list = adw::PreferencesGroup::builder()
        .description(tr("Experimentell und ohne Gerät ungetestet sind alle außer der EyeTV Netstream 4Sat."))
        .build();
    for d in addable {
        let row = adw::ActionRow::builder()
            .title(&d.name)
            .subtitle(match d.driver {
                Driver::Satip => "SAT>IP",
                Driver::LinuxDvb => tr("DVB-Karte"),
                Driver::Hdhomerun => "HDHomeRun",
                Driver::Tvheadend => tr("Tvheadend-Server (experimentell)"),
                Driver::Iptv => tr("M3U-Listen"),
            })
            .activatable(true)
            .build();
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let (app, dialog, state, descs, group, d) = (
            app.clone(),
            dialog.clone(),
            state.clone(),
            descs.clone(),
            group.clone(),
            d.clone(),
        );
        row.connect_activated(move |_| {
            let num = {
                let mut e = state.borrow_mut();
                let num = e.adapters.iter().map(|a| a.num).max().unwrap_or(SRC_SAT).max(SRC_SAT) + 1;
                let prefix = match d.driver {
                    Driver::LinuxDvb => "dvb",
                    Driver::Satip => "sat",
                    Driver::Hdhomerun => "hdhr",
                    Driver::Tvheadend => "tvh",
                    Driver::Iptv => "iptv",
                };
                e.adapters.push(Adapter {
                    id: format!("{prefix}-{num}"),
                    num,
                    kind: d.id.clone(),
                    ..Adapter::default()
                });
                num
            };
            dialog.pop_subpage();
            let row = source_row(&app, &dialog, &state, &descs, &group, num);
            ActionRowExt::activate(&row);
        });
        list.add(&row);
    }
    content.add(&list);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content));
    adw::NavigationPage::builder()
        .title(tr("Empfangsquelle hinzufügen"))
        .child(&toolbar)
        .build()
}

/// Zeile einer Empfangsquelle in der Übersicht (Schalter, Pfeil zur Seite); gibt die Zeile zurück.
fn source_row(
    app: &Rc<App>,
    dialog: &adw::PreferencesDialog,
    state: &State,
    descs: &Rc<Vec<Description>>,
    group: &adw::PreferencesGroup,
    num: u32,
) -> adw::ActionRow {
    let a = state
        .borrow()
        .adapters
        .iter()
        .find(|a| a.num == num)
        .cloned()
        .unwrap_or_default();
    let row = adw::ActionRow::builder()
        .title(adapter_title(&a, descs))
        .subtitle(source_summary(app, &a, descs))
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
            if let Some(a) = state.borrow_mut().adapters.iter_mut().find(|a| a.num == num) {
                a.enabled = sw.is_active();
            }
        });
    }
    let (app, dialog, state, descs, group2) = (
        app.clone(),
        dialog.clone(),
        state.clone(),
        descs.clone(),
        group.clone(),
    );
    row.connect_activated(move |row| {
        let page = adapter_page(&app, &dialog, &state, &descs, num, row, &switch, &group2);
        dialog.push_subpage(&page);
    });
    group.add(&row);
    row
}

/// Unterzeile einer Empfangsquelle: Art, ggf. Gerät, Zahl der Sender.
fn source_summary(app: &App, a: &Adapter, descs: &[Description]) -> String {
    let n = app.list_len(a.num) as u64;
    let channels = trn("{n} Sender", "{n} Sender", n);
    match driver_of(a, descs) {
        Driver::Satip if a.host.trim().is_empty() => {
            tf!("SAT>IP · noch keine Adresse · {channels}", channels = channels)
        }
        Driver::Satip => {
            tf!("SAT>IP · {host} · {channels}", host = a.host.trim(), channels = channels)
        }
        Driver::LinuxDvb if a.host.trim().is_empty() => {
            tf!("DVB-Karte · noch kein Gerät · {channels}", channels = channels)
        }
        Driver::LinuxDvb => tf!(
            "DVB-Karte · adapter{device} · {channels}",
            device = a.host.trim(),
            channels = channels
        ),
        Driver::Hdhomerun if a.host.trim().is_empty() => {
            tf!("HDHomeRun · noch keine Adresse · {channels}", channels = channels)
        }
        Driver::Hdhomerun => {
            tf!("HDHomeRun · {host} · {channels}", host = a.host.trim(), channels = channels)
        }
        Driver::Tvheadend if a.host.trim().is_empty() => {
            tf!("Tvheadend · noch keine Adresse · {channels}", channels = channels)
        }
        Driver::Tvheadend => {
            tf!("Tvheadend · {host} · {channels}", host = a.host.trim(), channels = channels)
        }
        Driver::Iptv => tf!("M3U-Listen · {channels}", channels = channels),
    }
}

/// Seite einer Empfangsquelle.
#[allow(clippy::too_many_arguments)]
fn adapter_page(
    app: &Rc<App>,
    dialog: &adw::PreferencesDialog,
    state: &State,
    descs: &Rc<Vec<Description>>,
    num: u32,
    list_row: &adw::ActionRow,
    list_switch: &gtk::Switch,
    group: &adw::PreferencesGroup,
) -> adw::NavigationPage {
    let a = state
        .borrow()
        .adapters
        .iter()
        .find(|a| a.num == num)
        .cloned()
        .unwrap_or_default();
    let driver = driver_of(&a, descs);
    let sat = driver == Driver::Satip;
    let signal_epg = description_of(&a, descs).is_some_and(|d| d.signal_epg);
    let tvh = description_of(&a, descs).is_some_and(|d| d.driver == Driver::Tvheadend);
    let content = adw::PreferencesPage::new();
    // Änderung an der Empfangsquelle in den bearbeiteten Stand schreiben.
    let edit = {
        let state = state.clone();
        move |f: &dyn Fn(&mut Adapter)| {
            if let Some(a) = state.borrow_mut().adapters.iter_mut().find(|a| a.num == num) {
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
    {
        let edit = edit.clone();
        general.add(&crate::icons::chooser_row(
            dialog,
            &a.icon,
            &crate::app::default_source_icon(driver),
            Rc::new(move |name: String| edit(&|a: &mut Adapter| a.icon = name.clone())),
        ));
    }
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

    match driver {
        Driver::Satip => {
            let (groups, host) = crate::satip::device_groups(app, &a, description_of(&a, descs));
            let e = edit.clone();
            host.connect_changed(move |h| {
                let text = h.text().trim().to_string();
                e(&|a| a.host = text.clone());
            });
            for g in groups {
                content.add(&g);
            }
        }
        #[cfg(target_os = "linux")]
        Driver::LinuxDvb => {
            let e = edit.clone();
            let set_device: Rc<dyn Fn(String)> = Rc::new(move |dev: String| {
                e(&|a| a.host = dev.clone());
            });
            for g in crate::dvbcard::device_groups(app, &a, set_device) {
                content.add(&g);
            }
        }
        Driver::Hdhomerun => {
            let e = edit.clone();
            let set_host: Rc<dyn Fn(String)> = Rc::new(move |h: String| {
                e(&|a| a.host = h.clone());
            });
            for g in hdhomerun_groups(app, &a, set_host) {
                content.add(&g);
            }
        }
        Driver::Tvheadend => {
            let e = edit.clone();
            let set_host: Rc<dyn Fn(String)> = Rc::new(move |h: String| {
                e(&|a| a.host = h.clone());
            });
            for g in tvheadend_groups(app, &a, set_host) {
                content.add(&g);
            }
        }
        _ => content.add(&lists_group(app, dialog, &a)),
    }

    // Programm: aus dem Signal und/oder aus XMLTV, beides freiwillig.
    let guide = adw::PreferencesGroup::builder()
        .title(tr("Programm (EPG)"))
        .description(if sat {
            tr("Aus dem Signal (je Sender in „Sender verwalten“ → EPG „Aus dem SAT-Empfang“) und/oder aus XMLTV-Quellen")
        } else if tvh {
            tr("Von Tvheadend (je Sender in „Sender verwalten“ → EPG „Von Tvheadend“) und/oder aus XMLTV-Quellen")
        } else if signal_epg {
            tr("Aus dem Signal (neue Sender nach dem Suchlauf) und/oder aus XMLTV-Quellen")
        } else {
            tr("Aus XMLTV-Quellen; ohne Quelle laufen die Sender ohne Programm")
        })
        .build();
    if signal_epg {
        let signal = adw::SpinRow::with_range(1.0, 24.0, 1.0);
        if tvh {
            signal.set_title(tr("Von Tvheadend holen alle"));
            signal.set_subtitle(tr("Stunden; eine Anfrage an den Server, kein Tuner nötig"));
        } else {
            signal.set_title(tr("Aus dem Signal sammeln alle"));
            signal.set_subtitle(tr("Stunden; belegt je Transponder etwa 40 Sekunden einen Tuner"));
        }
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
    let tuners = driver != Driver::Iptv;
    let max = adw::SpinRow::with_range(0.0, if tuners { 8.0 } else { 20.0 }, 1.0);
    max.set_title(tr("Gleichzeitig höchstens"));
    max.set_subtitle(if tuners {
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

    // Weitere Empfangsquellen lassen sich wieder entfernen (samt Senderliste).
    if num > SRC_SAT {
        let remove_group = adw::PreferencesGroup::new();
        let remove_row = adw::ActionRow::builder()
            .title(tr("Empfangsquelle entfernen"))
            .subtitle(tr("Mit ihrer Senderliste und ihren Programmdaten"))
            .build();
        let remove = gtk::Button::builder()
            .label(tr("Entfernen"))
            .valign(gtk::Align::Center)
            .css_classes(["destructive-action"])
            .build();
        remove_row.add_suffix(&remove);
        let (state, dialog2, list_row2, group2) =
            (state.clone(), dialog.clone(), list_row.clone(), group.clone());
        let name = list_row.title().to_string();
        remove.connect_clicked(move |b| {
            let d = adw::AlertDialog::new(
                Some(&tf!("„{name}“ entfernen?", name = name)),
                Some(tr("Ihre Senderliste und ihre Programmdaten werden beim Schließen der Einstellungen gelöscht.")),
            );
            d.add_response("cancel", tr("Abbrechen"));
            d.add_response("remove", tr("Entfernen"));
            d.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
            d.set_close_response("cancel");
            let (state, dialog, row, group) =
                (state.clone(), dialog2.clone(), list_row2.clone(), group2.clone());
            d.connect_response(Some("remove"), move |_, _| {
                state.borrow_mut().adapters.retain(|a| a.num != num);
                group.remove(&row);
                dialog.pop_subpage();
            });
            d.present(Some(b));
        });
        remove_group.add(&remove_row);
        content.add(&remove_group);
    }

    // Unterzeile in der Übersicht nach der Rückkehr auffrischen (Adresse, Gerät, Sender).
    let title = list_row.title().to_string();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content));
    let nav = adw::NavigationPage::builder()
        .title(&title)
        .child(&toolbar)
        .build();
    let (app, state, row, descs) = (app.clone(), state.clone(), list_row.clone(), descs.clone());
    nav.connect_hidden(move |_| {
        if let Some(a) = state.borrow().adapters.iter().find(|a| a.num == num) {
            row.set_subtitle(&source_summary(&app, a, &descs));
        }
    });
    nav
}

/// Internet-TV: die M3U-Listen der Senderliste, Übernahme aus Datei oder URL.
fn lists_group(app: &Rc<App>, dialog: &adw::PreferencesDialog, a: &Adapter) -> adw::PreferencesGroup {
    let (enabled, num) = (a.enabled, a.num);
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
    fill_lists(app, &group, &rows, num);
    for (button, from_url) in [(file, false), (url, true)] {
        let (app, dialog) = (app.clone(), dialog.clone());
        button.connect_clicked(move |_| {
            // Erst die Einstellungen schließen (übernimmt sie), dann die Übernahme öffnen.
            dialog.close();
            crate::manage::import_into(&app, num, from_url);
        });
    }
    group
}

fn fill_lists(
    app: &Rc<App>,
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    num: u32,
) {
    for r in rows.borrow_mut().drain(..) {
        group.remove(&r);
    }
    let sources = app.with_list(num, |lib| {
        lib.sources
            .iter()
            .map(|s| {
                let n = lib.channels.iter().filter(|c| c.source == s.id).count();
                (s.id, s.name.clone(), paths::plain_location(&s.location), n)
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
                app.remove_list_source(num, id);
                fill_lists(&app, &group, &rows, num);
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
        let users: Vec<String> = state
            .borrow()
            .adapters
            .iter()
            .filter(|a| a.xmltv.contains(&x.id))
            .map(|a| match a.id.as_str() {
                ADAPTER_TV => "TV".to_string(),
                tvjoe_core::settings::ADAPTER_SAT => "SAT".to_string(),
                id => id.to_string(),
            })
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

/// Tvheadend: Adresse und Zugang (Datei nur für den Nutzer lesbar), Prüfen, Kanäle holen.
fn tvheadend_groups(app: &Rc<App>, a: &Adapter, set_host: Rc<dyn Fn(String)>) -> Vec<adw::PreferencesGroup> {
    use tvjoe_core::tvheadend::{self as tvh, Access};
    let stored = (!a.host.trim().is_empty())
        .then(|| tvh::access_path(&tvh::normalize(&a.host)).and_then(|p| Access::load(&p)))
        .flatten()
        .unwrap_or_default();
    let server = adw::PreferencesGroup::builder()
        .title(tr("Server"))
        .description(tr("Eigener Tvheadend-Server im Netz (experimentell). Benutzer mit Streaming-Recht in Tvheadend anlegen; das Passwort liegt nur für dich lesbar in den Einstellungen, in Stream-Adressen steht nur ein zeitlich begrenztes Ticket."))
        .build();
    let host = adw::EntryRow::builder()
        .title(tr("Adresse (z. B. http://tvheadend.example.org:9981)"))
        .text(a.host.as_str())
        .build();
    let user = adw::EntryRow::builder().title(tr("Benutzer")).text(stored.user.as_str()).build();
    let password = adw::PasswordEntryRow::builder().title(tr("Passwort")).text(stored.password.as_str()).build();
    let check_row = adw::ActionRow::builder()
        .title(tr("Prüfen"))
        .subtitle(tr("Meldet sich an und fragt die Fassung ab"))
        .build();
    let check = gtk::Button::builder().label(tr("Prüfen")).valign(gtk::Align::Center).build();
    check_row.add_suffix(&check);
    server.add(&host);
    server.add(&user);
    server.add(&password);
    server.add(&check_row);
    host.connect_changed(move |h| set_host(h.text().trim().to_string()));

    let list = adw::PreferencesGroup::builder()
        .title(tr("Senderliste"))
        .description(tr("Die Kanäle kommen von Tvheadend (dort vorher Suchlauf und Kanalzuordnung); ein erneutes Holen behält Nummern und eigene Namen. Programm von Tvheadend; je Sender in „Sender verwalten“ auf XMLTV umstellbar."))
        .build();
    let list_row = adw::ActionRow::builder().title(tr("Senderliste")).subtitle(list_summary(&a.id)).build();
    let fetch = gtk::Button::builder().label(tr("Kanäle holen")).valign(gtk::Align::Center).build();
    list_row.add_suffix(&fetch);
    list.add(&list_row);

    // Zugang aus den Feldern; gespeichert bei Prüfen und Holen.
    let access = {
        let (host, user, password) = (host.clone(), user.clone(), password.clone());
        move || -> Option<Access> {
            let h = host.text().trim().to_string();
            (!h.is_empty()).then(|| Access {
                server: tvh::normalize(&h),
                user: user.text().trim().to_string(),
                password: password.text().to_string(),
            })
        }
    };
    let save = |a: &Access| -> Result<(), String> {
        let path = tvh::access_path(&a.server).ok_or_else(|| tr("Kein Konfigurationsordner gefunden").to_string())?;
        a.save(&path)
    };
    {
        let (row, access) = (check_row.clone(), access.clone());
        check.connect_clicked(move |b| {
            let Some(a) = access() else {
                row.set_subtitle(tr("Bitte zuerst die Adresse eintragen"));
                return;
            };
            if let Err(e) = save(&a) {
                row.set_subtitle(&e);
                return;
            }
            b.set_sensitive(false);
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(a.server_info());
            });
            let (b, row) = (b.clone(), row.clone());
            glib::spawn_future_local(async move {
                let Ok(r) = rx.recv().await else { return };
                b.set_sensitive(true);
                row.set_subtitle(&match r {
                    Ok(info) => tf!("Verbunden: {info}", info = info),
                    Err(e) => tf!("Nicht verbunden: {error}", error = e),
                });
            });
        });
    }
    {
        let (row, app, id, num) = (list_row.clone(), Rc::downgrade(app), a.id.clone(), a.num);
        fetch.connect_clicked(move |b| {
            let Some(a) = access() else {
                row.set_subtitle(tr("Bitte zuerst die Adresse eintragen"));
                return;
            };
            if let Err(e) = save(&a) {
                row.set_subtitle(&e);
                return;
            }
            b.set_sensitive(false);
            let location = format!("tvheadend://{}", a.server.split_once("://").map_or(a.server.as_str(), |(_, r)| r));
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(a.channels());
            });
            let (b, row, app, id) = (b.clone(), row.clone(), app.clone(), id.clone());
            glib::spawn_future_local(async move {
                let Ok(r) = rx.recv().await else { return };
                b.set_sensitive(true);
                row.set_subtitle(&match r {
                    Ok(items) => {
                        let text = save_list(&id, "Tvheadend", &location, &items);
                        if let Some(app) = app.upgrade() {
                            app.reload_list(num);
                        }
                        text
                    }
                    Err(e) => tf!("Senderliste nicht geholt: {error}", error = e),
                });
            });
        });
    }
    vec![server, list]
}

/// HDHomeRun: Adresse, Prüfen, Senderliste vom Gerät (`lineup.json`).
fn hdhomerun_groups(app: &Rc<App>, a: &Adapter, set_host: Rc<dyn Fn(String)>) -> Vec<adw::PreferencesGroup> {
    let device = adw::PreferencesGroup::builder()
        .title(tr("Gerät"))
        .description(tr("Netzwerktuner von SiliconDust; Senderliste und Streams über HTTP. Ungetestet."))
        .build();
    let host = adw::EntryRow::builder()
        .title(tr("Adresse des HDHomeRun (IP-Adresse oder Name)"))
        .text(a.host.as_str())
        .build();
    let check_row = adw::ActionRow::builder()
        .title(tr("Prüfen"))
        .subtitle(tr("Fragt Modell, Firmware und Zahl der Tuner ab"))
        .build();
    let check = gtk::Button::builder()
        .label(tr("Prüfen"))
        .valign(gtk::Align::Center)
        .build();
    check_row.add_suffix(&check);
    device.add(&host);
    device.add(&check_row);
    host.connect_changed(move |h| set_host(h.text().trim().to_string()));

    let list = adw::PreferencesGroup::builder()
        .title(tr("Senderliste"))
        .description(tr("Die Senderliste kommt vom Gerät (dort vorher den Suchlauf ausführen); ein erneutes Holen behält Nummern und eigene Namen."))
        .build();
    let list_row = adw::ActionRow::builder()
        .title(tr("Senderliste"))
        .subtitle(list_summary(&a.id))
        .build();
    let fetch = gtk::Button::builder()
        .label(tr("Vom Gerät holen"))
        .valign(gtk::Align::Center)
        .build();
    list_row.add_suffix(&fetch);
    list.add(&list_row);

    {
        let (host, row) = (host.clone(), check_row.clone());
        check.connect_clicked(move |b| {
            let h = host.text().trim().to_string();
            if h.is_empty() {
                return;
            }
            b.set_sensitive(false);
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(tvjoe_core::hdhomerun::discover(&h));
            });
            let (b, row) = (b.clone(), row.clone());
            glib::spawn_future_local(async move {
                let Ok(r) = rx.recv().await else { return };
                b.set_sensitive(true);
                row.set_subtitle(&match r {
                    Ok(i) => tf!(
                        "{name} ({model}) · Firmware {firmware} · {tuners}",
                        name = i.name,
                        model = i.model,
                        firmware = i.firmware,
                        tuners = trn("{n} Tuner", "{n} Tuner", u64::from(i.tuners))
                    ),
                    Err(e) => tf!("Gerät nicht erreichbar: {error}", error = e),
                });
            });
        });
    }
    {
        let (host, row, app, id, num) = (
            host.clone(),
            list_row.clone(),
            Rc::downgrade(app),
            a.id.clone(),
            a.num,
        );
        fetch.connect_clicked(move |b| {
            let h = host.text().trim().to_string();
            if h.is_empty() {
                row.set_subtitle(tr("Bitte zuerst die Adresse des Geräts eintragen"));
                return;
            }
            b.set_sensitive(false);
            let (tx, rx) = async_channel::bounded(1);
            let hh = h.clone();
            std::thread::spawn(move || {
                let _ = tx.send_blocking(tvjoe_core::hdhomerun::lineup(&hh));
            });
            let (b, row, app, id) = (b.clone(), row.clone(), app.clone(), id.clone());
            glib::spawn_future_local(async move {
                let Ok(r) = rx.recv().await else { return };
                b.set_sensitive(true);
                row.set_subtitle(&match r {
                    Ok(items) => {
                        let text = save_list(&id, "HDHomeRun", &format!("hdhomerun://{h}"), &items);
                        if let Some(app) = app.upgrade() {
                            app.reload_list(num);
                        }
                        text
                    }
                    Err(e) => tf!("Senderliste nicht geholt: {error}", error = e),
                });
            });
        });
    }
    vec![device, list]
}

/// Stand der gespeicherten Senderliste einer Empfangsquelle.
fn list_summary(adapter_id: &str) -> String {
    let lib = paths::source_library_path(adapter_id)
        .and_then(|p| tvjoe_core::Library::load(&p).ok())
        .unwrap_or_default();
    match lib.channels.iter().filter(|c| !c.missing).count() {
        0 => tr("Noch keine Sender gespeichert").to_string(),
        n => trn("{n} Sender gespeichert", "{n} Sender gespeichert", n as u64),
    }
}

/// Sender in die Senderliste einer Empfangsquelle übernehmen und zusammenfassen.
fn save_list(adapter_id: &str, name: &str, location: &str, items: &[tvjoe_core::ScannedChannel]) -> String {
    let Some(path) = paths::source_library_path(adapter_id) else {
        return tr("Kein Konfigurationsordner gefunden").to_string();
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut lib = match tvjoe_core::Library::load(&path) {
        Ok(l) => l,
        Err(e) => return tf!("Senderliste nicht lesbar: {error}", error = e),
    };
    let out = lib.sync_scanned(name, location, items);
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
