//! Reiter „Mediatheken“ (Nutzer, 2026-10-28): Bibliotheken als Adapter wie die Empfangsquellen.
//! Je Bibliothek eine Zeile mit Schalter und eine eigene Seite (Name, Einstellungen des Typs);
//! „+“ legt eine weitere an (mehrere Jellyfin-Server möglich, die ÖR-Mediathek einmal).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use tvjoe_core::adapter::{self, LibraryDescription, LibraryDriver};
use tvjoe_core::settings::{LIBRARY_OER, Library};
use tvjoe_core::{Settings, paths, tf, tr};

use crate::app::App;

/// Bearbeitete Bibliotheken und die Übernahmen ihrer Seiten (beim Schließen ausgeführt).
struct Edit {
    libraries: Vec<Library>,
    applies: Vec<Box<dyn Fn(&mut Settings)>>,
}

type State = Rc<RefCell<Edit>>;

/// Beschreibungen der Klasse Bibliothek (eingebaute und eigene aus `adapter/library/`).
fn descriptions() -> Vec<LibraryDescription> {
    let dir = paths::config_dir().map(|d| d.join("adapter"));
    let (all, notes) = adapter::libraries(dir.as_deref());
    for n in &notes {
        crate::trace(&format!("Adapterbeschreibung {n}"));
    }
    all
}

/// Anzeigename einer Bibliothek: eigener Name, sonst der der Beschreibung.
pub fn title_of(l: &Library, descs: &[LibraryDescription]) -> String {
    if !l.title.trim().is_empty() {
        return l.title.clone();
    }
    descs
        .iter()
        .find(|d| d.id == l.kind)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| l.kind.clone())
}

fn driver_of(l: &Library, descs: &[LibraryDescription]) -> Option<LibraryDriver> {
    descs.iter().find(|d| d.id == l.kind).map(|d| d.driver)
}

/// Unterzeile: Art und Stand der Anmeldung.
fn summary(l: &Library, descs: &[LibraryDescription]) -> String {
    match driver_of(l, descs) {
        Some(LibraryDriver::Oer) => tr("Filmliste von MediathekView").to_string(),
        Some(LibraryDriver::Jellyfin) => match crate::jellyfin::account(&l.id) {
            Some(a) => tf!("Jellyfin · angemeldet bei {server}", server = a.server),
            None => tr("Jellyfin · nicht angemeldet").to_string(),
        },
        Some(LibraryDriver::Plex) => match crate::jellyfin::account(&l.id) {
            Some(a) => tf!("Plex · angemeldet bei {server}", server = a.server),
            None => tr("Plex · nicht angemeldet").to_string(),
        },
        None => tr("Beschreibung fehlt").to_string(),
    }
}

/// Reiter „Mediatheken“. Gibt die Seite und die Übernahme beim Schließen zurück.
pub fn prefs_page(
    app: &Rc<App>,
    dialog: &adw::PreferencesDialog,
) -> (adw::PreferencesPage, impl Fn(&mut Settings) + 'static) {
    let state: State = {
        let mut s = app.settings().clone();
        s.migrate();
        let mut libraries = s.libraries.clone();
        libraries.sort_by_key(|l| l.num);
        Rc::new(RefCell::new(Edit {
            libraries,
            applies: Vec::new(),
        }))
    };
    let descs = Rc::new(descriptions());
    let page = adw::PreferencesPage::builder()
        .name("mediatheken")
        .title(tr("Mediatheken"))
        .icon_name("folder-videos-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(tr("Mediatheken"))
        .description(tr("Jede Mediathek hat ihren eigenen Knopf in der Kopfleiste. Ausgeschaltet verschwindet er, und im Hintergrund wird nichts für sie geladen."))
        .build();
    // Eigene Unterseite statt Aufklappmenü (ein Popover im Dialog brachte GTK unter Windows
    // zum Absturz).
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text(tr("Mediathek hinzufügen"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    {
        let (app, dialog, state, descs, group) = (
            app.clone(),
            dialog.clone(),
            state.clone(),
            descs.clone(),
            group.clone(),
        );
        add.connect_clicked(move |_| {
            let page = add_page(&app, &dialog, &state, &descs, &group);
            dialog.push_subpage(&page);
        });
    }
    group.set_header_suffix(Some(&add));
    let ids: Vec<String> = state.borrow().libraries.iter().map(|l| l.id.clone()).collect();
    for id in ids {
        library_row(app, dialog, &state, &descs, &group, &id);
    }
    page.add(&group);

    let apply = move |s: &mut Settings| {
        let e = state.borrow();
        // Entfernte Server-Bibliotheken: Zugang und Zwischenspeicher löschen.
        for old in &s.libraries {
            if old.id != LIBRARY_OER && !e.libraries.iter().any(|l| l.id == old.id) {
                for p in [paths::library_account_path(&old.id), paths::library_db(&old.id)]
                    .into_iter()
                    .flatten()
                {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        s.libraries = e.libraries.clone();
        for apply in &e.applies {
            apply(s);
        }
    };
    (page, apply)
}

/// Unterseite „Mediathek hinzufügen“: je Beschreibung eine Zeile (die ÖR-Mediathek nur, wenn
/// sie fehlt); die Wahl legt die Bibliothek an und öffnet ihre Seite.
fn add_page(
    app: &Rc<App>,
    dialog: &adw::PreferencesDialog,
    state: &State,
    descs: &Rc<Vec<LibraryDescription>>,
    group: &adw::PreferencesGroup,
) -> adw::NavigationPage {
    let content = adw::PreferencesPage::new();
    let list = adw::PreferencesGroup::new();
    let have_oer = state
        .borrow()
        .libraries
        .iter()
        .any(|l| driver_of(l, descs) == Some(LibraryDriver::Oer));
    for d in descs.iter() {
        if d.driver == LibraryDriver::Oer && have_oer {
            continue;
        }
        let row = adw::ActionRow::builder()
            .title(&d.name)
            .subtitle(match d.driver {
                LibraryDriver::Oer => tr("Öffentlich-rechtliche Sender"),
                LibraryDriver::Jellyfin => tr("Eigener Server"),
                LibraryDriver::Plex => tr("Eigener Server (experimentell)"),
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
            let id = {
                let mut e = state.borrow_mut();
                let mut probe = Settings {
                    libraries: e.libraries.clone(),
                    ..Settings::default()
                };
                let id = probe.add_library(&d.id);
                e.libraries = probe.libraries;
                id
            };
            dialog.pop_subpage();
            let row = library_row(&app, &dialog, &state, &descs, &group, &id);
            ActionRowExt::activate(&row);
        });
        list.add(&row);
    }
    content.add(&list);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content));
    adw::NavigationPage::builder()
        .title(tr("Mediathek hinzufügen"))
        .child(&toolbar)
        .build()
}

/// Zeile einer Bibliothek in der Übersicht (Schalter, Pfeil zur Seite); gibt die Zeile zurück.
fn library_row(
    app: &Rc<App>,
    dialog: &adw::PreferencesDialog,
    state: &State,
    descs: &Rc<Vec<LibraryDescription>>,
    group: &adw::PreferencesGroup,
    id: &str,
) -> adw::ActionRow {
    let l = state
        .borrow()
        .libraries
        .iter()
        .find(|l| l.id == id)
        .cloned()
        .unwrap_or_default();
    let row = adw::ActionRow::builder()
        .title(title_of(&l, descs))
        .subtitle(summary(&l, descs))
        .activatable(true)
        .build();
    let switch = gtk::Switch::builder()
        .active(l.enabled)
        .valign(gtk::Align::Center)
        .build();
    row.add_suffix(&switch);
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    {
        let (state, id) = (state.clone(), id.to_string());
        switch.connect_active_notify(move |sw| {
            if let Some(l) = state.borrow_mut().libraries.iter_mut().find(|l| l.id == id) {
                l.enabled = sw.is_active();
            }
        });
    }
    let (app, dialog, state, descs, group2, id) = (
        app.clone(),
        dialog.clone(),
        state.clone(),
        descs.clone(),
        group.clone(),
        id.to_string(),
    );
    row.connect_activated(move |row| {
        let page = library_page(&app, &dialog, &state, &descs, &id, row, &group2);
        dialog.push_subpage(&page);
    });
    group.add(&row);
    row
}

/// Seite einer Bibliothek: Name, Entfernen, darunter die Einstellungen ihres Typs.
fn library_page(
    app: &Rc<App>,
    dialog: &adw::PreferencesDialog,
    state: &State,
    descs: &Rc<Vec<LibraryDescription>>,
    id: &str,
    row: &adw::ActionRow,
    group: &adw::PreferencesGroup,
) -> adw::NavigationPage {
    let l = state
        .borrow()
        .libraries
        .iter()
        .find(|l| l.id == id)
        .cloned()
        .unwrap_or_default();
    let content = adw::PreferencesPage::new();
    let general = adw::PreferencesGroup::builder().title(tr("Allgemein")).build();
    let name = adw::EntryRow::builder()
        .title(tr("Name in der Kopfleiste"))
        .text(title_of(&l, descs))
        .build();
    {
        let (state, id, row, descs) = (state.clone(), id.to_string(), row.clone(), descs.clone());
        name.connect_changed(move |e| {
            let mut st = state.borrow_mut();
            if let Some(l) = st.libraries.iter_mut().find(|l| l.id == id) {
                l.title = e.text().trim().to_string();
                row.set_title(&title_of(l, &descs));
            }
        });
    }
    general.add(&name);
    // Die ÖR-Mediathek gibt es genau einmal; Server-Bibliotheken lassen sich entfernen.
    if driver_of(&l, descs) != Some(LibraryDriver::Oer) {
        let remove_row = adw::ActionRow::builder()
            .title(tr("Mediathek entfernen"))
            .subtitle(tr("Löscht auch die Anmeldung; Heruntergeladenes bleibt in der Ablage"))
            .build();
        let remove = gtk::Button::builder()
            .label(tr("Entfernen"))
            .valign(gtk::Align::Center)
            .css_classes(["destructive-action"])
            .build();
        remove_row.add_suffix(&remove);
        let (state, id, row, group, dialog) = (
            state.clone(),
            id.to_string(),
            row.clone(),
            group.clone(),
            dialog.clone(),
        );
        remove.connect_clicked(move |_| {
            state.borrow_mut().libraries.retain(|l| l.id != id);
            group.remove(&row);
            dialog.pop_subpage();
        });
        general.add(&remove_row);
    }
    content.add(&general);
    match driver_of(&l, descs) {
        Some(LibraryDriver::Oer) => {
            let (groups, apply) = crate::mediathek::prefs_groups(app);
            for g in groups {
                content.add(&g);
            }
            state.borrow_mut().applies.push(Box::new(apply));
        }
        Some(LibraryDriver::Jellyfin | LibraryDriver::Plex) => {
            let (groups, apply) = crate::jellyfin::prefs_groups(app, id, &l.kind);
            for g in groups {
                content.add(&g);
            }
            state.borrow_mut().applies.push(Box::new(apply));
        }
        None => {}
    }
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content));
    adw::NavigationPage::builder()
        .title(title_of(&l, descs))
        .child(&toolbar)
        .build()
}

/// Einstellungen öffnen, gleich auf der Seite der Bibliothek `id` (Zahnrad in der Ansicht,
/// Knopf ohne Anmeldung).
pub fn open(app: &Rc<App>, id: &str, parent: &impl IsA<gtk::Widget>) {
    let dialog = crate::prefs::open_at(app, Some("mediatheken"), parent);
    let Some(l) = app.settings().library(id).cloned() else { return };
    let title = title_of(&l, &descriptions());
    fn find(w: &gtk::Widget, title: &str) -> Option<adw::ActionRow> {
        if let Some(r) = w.downcast_ref::<adw::ActionRow>() {
            if r.is_activatable() && r.title() == title {
                return Some(r.clone());
            }
        }
        let mut c = w.first_child();
        while let Some(child) = c {
            if let Some(r) = find(&child, title) {
                return Some(r);
            }
            c = child.next_sibling();
        }
        None
    }
    if let Some(row) = find(dialog.upcast_ref(), &title) {
        ActionRowExt::activate(&row);
    }
}
