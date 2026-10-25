//! Gruppe „Sichern“ im Reiter Bedienung: Einstellungen und beide Senderlisten in eine Datei
//! exportieren bzw. daraus zurückholen (ohne Jellyfin-Anmeldung, siehe `tvjoe_core::backup`).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use tvjoe_core::backup::Backup;

use crate::app::App;

pub fn prefs_group(
    app: &Rc<App>,
    dialog: &adw::PreferencesDialog,
    pending: &Rc<RefCell<Option<Backup>>>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Sichern")
        .description("Alle Einstellungen, TV- und SAT-Senderliste in einer Datei. Nicht enthalten: Jellyfin-Anmeldung, Zwischenspeicher (Logos, Programm, Filmliste) und Weiterseh-Positionen.")
        .build();
    let export_row = adw::ActionRow::builder()
        .title("Exportieren")
        .subtitle("Stand beim Öffnen der Einstellungen")
        .build();
    let export = gtk::Button::builder()
        .label("Exportieren…")
        .valign(gtk::Align::Center)
        .build();
    export_row.add_suffix(&export);
    let import_row = adw::ActionRow::builder()
        .title("Importieren")
        .subtitle("Ersetzt Einstellungen und Senderlisten; die bisherigen Dateien bleiben als .bak")
        .build();
    let import = gtk::Button::builder()
        .label("Importieren…")
        .valign(gtk::Align::Center)
        .build();
    import_row.add_suffix(&import);
    group.add(&export_row);
    group.add(&import_row);

    let (a, d) = (Rc::downgrade(app), dialog.clone());
    export.connect_clicked(move |_| {
        if let Some(app) = a.upgrade() {
            export_file(&app, &d);
        }
    });
    let (a, d, p) = (Rc::downgrade(app), dialog.clone(), pending.clone());
    import.connect_clicked(move |_| {
        if let Some(app) = a.upgrade() {
            import_file(&app, &d, &p);
        }
    });
    group
}

fn toml_filter() -> (gtk::FileFilter, gio::ListStore) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("tvjoe-Sicherung"));
    filter.add_pattern("*.toml");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    (filter, filters)
}

fn parent_window(d: &adw::PreferencesDialog) -> Option<gtk::Window> {
    d.root().and_downcast::<gtk::Window>()
}

fn export_file(app: &Rc<App>, d: &adw::PreferencesDialog) {
    let text = match app.backup().to_toml() {
        Ok(t) => t,
        Err(e) => {
            d.add_toast(adw::Toast::new(&format!("Sicherung fehlgeschlagen: {e}")));
            return;
        }
    };
    let (filter, filters) = toml_filter();
    let fd = gtk::FileDialog::builder()
        .title("Sicherung exportieren")
        .modal(true)
        .initial_name("tvjoe-sicherung.toml")
        .filters(&filters)
        .default_filter(&filter)
        .build();
    let d2 = d.clone();
    fd.save(
        parent_window(d).as_ref(),
        gio::Cancellable::NONE,
        move |res| {
            let Ok(file) = res else { return };
            let Some(path) = file.path() else { return };
            let msg = match std::fs::write(&path, &text) {
                Ok(()) => format!("Gespeichert: {}", path.display()),
                Err(e) => format!("{}: {e}", path.display()),
            };
            d2.add_toast(adw::Toast::new(&msg));
        },
    );
}

fn import_file(app: &Rc<App>, d: &adw::PreferencesDialog, pending: &Rc<RefCell<Option<Backup>>>) {
    let (filter, filters) = toml_filter();
    let fd = gtk::FileDialog::builder()
        .title("Sicherung importieren")
        .modal(true)
        .filters(&filters)
        .default_filter(&filter)
        .build();
    let (a, d2, p) = (Rc::downgrade(app), d.clone(), pending.clone());
    fd.open(
        parent_window(d).as_ref(),
        gio::Cancellable::NONE,
        move |res| {
            let Ok(file) = res else { return };
            let Some(path) = file.path() else { return };
            let backup = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| Backup::from_toml(&t));
            let b = match backup {
                Ok(b) => b,
                Err(e) => {
                    d2.add_toast(adw::Toast::new(&format!("{}: {e}", path.display())));
                    return;
                }
            };
            if a.upgrade().is_none() {
                return;
            }
            confirm(&d2, b, &p);
        },
    );
}

fn confirm(d: &adw::PreferencesDialog, b: Backup, pending: &Rc<RefCell<Option<Backup>>>) {
    let active = |l: &tvjoe_core::Library| l.channels.iter().filter(|c| c.active).count();
    let alert = adw::AlertDialog::new(
        Some("Sicherung übernehmen?"),
        Some(&format!(
            "Ersetzt alle Einstellungen, die TV-Senderliste ({} Sender, {} aktiv) und die SAT-Senderliste ({} Sender, {} aktiv). Die bisherigen Dateien bleiben als .bak erhalten, die Jellyfin-Anmeldung bleibt unverändert.\n\nDie Einstellungen werden dazu geschlossen.",
            b.tv.channels.len(),
            active(&b.tv),
            b.sat.channels.len(),
            active(&b.sat),
        )),
    );
    alert.add_response("cancel", "Abbrechen");
    alert.add_response("ok", "Übernehmen");
    alert.set_response_appearance("ok", adw::ResponseAppearance::Destructive);
    alert.set_close_response("cancel");
    let (d2, p) = (d.clone(), pending.clone());
    let b = RefCell::new(Some(b));
    alert.connect_response(Some("ok"), move |_, _| {
        p.replace(b.borrow_mut().take());
        d2.close();
    });
    alert.present(Some(d));
}
