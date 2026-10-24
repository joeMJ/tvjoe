//! Fenster „Sender verwalten“ nach dem Vorbild der EyeTV-Senderliste: Tabelle mit Nummer, Logo,
//! Name, Gruppe, Quelle, Qualitäten, Status, Aktiv und Favorit; Kontextmenü (rechte Maustaste)
//! mit Umbenennen, eigenen Namen löschen, Nummer ändern, Verschieben, Aktivieren/Deaktivieren,
//! Favorit, Exportieren und Löschen. Aktionen gelten für die Auswahl (Strg/Umschalt-Klick).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use tvjoe_core::{Channel, NumberTaken, Quality, net, paths};

use crate::app::App;

pub struct Manager {
    pub app: Rc<App>,
    pub win: adw::Window,
    list: gtk::ListBox,
    toasts: adw::ToastOverlay,
    count: gtk::Label,
    rows: RefCell<Vec<(u64, gtk::ListBoxRow)>>,
}

thread_local! {
    static OPEN: RefCell<Option<glib::WeakRef<adw::Window>>> = const { RefCell::new(None) };
}

/// Öffnet das Fenster (oder holt ein offenes nach vorn).
pub fn open(app: &Rc<App>) {
    open_manager(app);
}

/// Wie [`open`], gibt aber den Verwalter zurück (Selbstprobe). Ein schon offenes Fenster wird
/// nach vorn geholt; dann gibt es `None`.
pub fn open_manager(app: &Rc<App>) -> Option<Rc<Manager>> {
    if let Some(w) = OPEN.with(|o| o.borrow().as_ref().and_then(|w| w.upgrade())) {
        w.present();
        return None;
    }
    let mgr = Manager::new(app.clone());
    OPEN.with(|o| *o.borrow_mut() = Some(mgr.win.downgrade()));
    mgr.win.present();
    Some(mgr)
}

fn cell(text: &str, width: i32, class: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .width_request(width)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes([class])
        .build()
}

impl Manager {
    fn new(app: Rc<App>) -> Rc<Self> {
        let win = adw::Window::builder()
            .title("Sender verwalten – tvjoe")
            .default_width(1180)
            .default_height(760)
            .transient_for(&app.window)
            .build();

        let header = adw::HeaderBar::new();
        let import_file = gtk::Button::builder()
            .label("Datei importieren…")
            .tooltip_text("M3U-Datei ganz oder teilweise übernehmen")
            .build();
        import_file.add_css_class("suggested-action");
        let import_url = gtk::Button::builder()
            .label("Von URL…")
            .tooltip_text("M3U von einer Adresse laden")
            .build();
        let export = gtk::Button::builder()
            .label("Exportieren…")
            .tooltip_text("Alle Sender als M3U8 speichern")
            .build();
        header.pack_start(&import_file);
        header.pack_start(&import_url);
        header.pack_end(&export);
        let epg_sources = gtk::Button::builder()
            .label("EPG-Quellen…")
            .tooltip_text("XMLTV-Quellen eintragen und neu laden")
            .build();
        header.pack_end(&epg_sources);

        let titles = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        titles.set_margin_start(22);
        titles.set_margin_end(22);
        titles.set_margin_top(8);
        titles.set_margin_bottom(4);
        for (t, w) in [
            ("Nr.", 44),
            ("", 52),
            ("Name", 260),
            ("Gruppe", 150),
            ("Quelle", 130),
            ("Qualitäten", 150),
            ("Status", 110),
            ("EPG", 150),
            ("Aktiv", 50),
            ("Favorit", 50),
        ] {
            titles.append(&cell(t, w, "tvjoe-name"));
        }

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Multiple)
            .css_classes(["boxed-list", "tvjoe-table"])
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(12)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&list)
            .build();
        let count = gtk::Label::builder()
            .css_classes(["tvjoe-sub"])
            .xalign(0.0)
            .margin_start(22)
            .margin_bottom(8)
            .build();
        let hint = gtk::Label::builder()
            .label("Rechte Maustaste: Aktionen für die Auswahl · Strg/Umschalt-Klick: mehrere Sender wählen")
            .css_classes(["tvjoe-sub"])
            .xalign(0.0)
            .margin_start(22)
            .margin_top(4)
            .build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&hint);
        body.append(&titles);
        body.append(&scroller);
        body.append(&count);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&body));
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&toasts));
        win.set_content(Some(&view));

        let mgr = Rc::new(Self {
            app,
            win,
            list,
            toasts,
            count,
            rows: RefCell::new(Vec::new()),
        });
        mgr.install_actions();
        mgr.refill();

        let m = mgr.clone();
        import_file.connect_clicked(move |_| crate::import::from_file(&m));
        let m = mgr.clone();
        import_url.connect_clicked(move |_| crate::import::from_url(&m));
        let m = mgr.clone();
        export.connect_clicked(move |_| m.export(None));
        let m = mgr.clone();
        epg_sources.connect_clicked(move |_| m.edit_epg_sources());
        mgr
    }

    pub fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    /// Baut die Tabelle aus der Bibliothek neu auf.
    pub fn refill(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let mut rows = self.rows.borrow_mut();
        rows.clear();
        let lib = self.app.library();
        let logo_dir = paths::logo_cache_dir();
        for ch in &lib.channels {
            let source = lib
                .source(ch.source)
                .map(|s| s.name.as_str())
                .unwrap_or("–");
            let row = self.build_row(ch, source, logo_dir.as_deref());
            self.list.append(&row);
            rows.push((ch.id, row));
        }
        let active = lib.channels.iter().filter(|c| c.active).count();
        self.count.set_label(&format!(
            "{} Sender, davon {active} aktiv · {} Quellen",
            lib.channels.len(),
            lib.sources.len()
        ));
    }

    fn build_row(
        self: &Rc<Self>,
        ch: &Channel,
        source: &str,
        logo_dir: Option<&std::path::Path>,
    ) -> gtk::ListBoxRow {
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        hbox.set_margin_top(4);
        hbox.set_margin_bottom(4);
        hbox.set_margin_start(10);
        hbox.set_margin_end(10);
        hbox.append(&cell(
            &ch.number.map(|n| n.to_string()).unwrap_or_default(),
            44,
            "tvjoe-num",
        ));
        let logo = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .width_request(52)
            .height_request(30)
            .build();
        if let (Some(dir), Some(url)) = (logo_dir, ch.logo.as_deref()) {
            let path = dir.join(net::cache_name(url, "png"));
            if path.exists() {
                logo.set_filename(Some(&path));
            }
        }
        hbox.append(&crate::logo_box(&logo, 52));

        let name_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        name_box.set_width_request(260);
        let name = cell(ch.display_name(), 260, "tvjoe-name");
        name_box.append(&name);
        if ch.custom_name.is_some() {
            name_box.append(&cell(&format!("Quelle: {}", ch.name), 260, "tvjoe-sub"));
        }
        hbox.append(&name_box);
        hbox.append(&cell(ch.display_group().unwrap_or("–"), 150, "tvjoe-sub"));
        hbox.append(&cell(source, 130, "tvjoe-sub"));
        let qualities: Vec<&str> = ch
            .streams
            .iter()
            .map(|s| s.quality.map_or("?", Quality::label))
            .collect();
        hbox.append(&cell(&qualities.join(" "), 150, "tvjoe-sub"));
        let status = if ch.missing {
            "fehlt in Quelle"
        } else if !ch.active {
            "inaktiv"
        } else {
            "ok"
        };
        hbox.append(&cell(status, 110, "tvjoe-sub"));
        let epg_text = match self.app.guide() {
            Some(g) => match g.match_channel(ch) {
                Some((id, kind)) => {
                    let how = match kind {
                        tvjoe_core::epg::MatchKind::Manual => "manuell",
                        _ => "automatisch",
                    };
                    format!("{} ({how})", g.display_name(&id).unwrap_or(&id))
                }
                None => "keine Zuordnung".into(),
            },
            None => "–".into(),
        };
        let epg_cell = cell(&epg_text, 150, "tvjoe-sub");
        epg_cell.set_tooltip_text(Some(&epg_text));
        hbox.append(&epg_cell);

        let active = gtk::CheckButton::builder()
            .active(ch.active)
            .tooltip_text("Aktiv: erscheint in der Senderliste")
            .width_request(50)
            .build();
        let id = ch.id;
        let m = Rc::downgrade(self);
        active.connect_toggled(move |b| {
            if let Some(m) = m.upgrade() {
                m.set_flag(&[id], |c| c.active = b.is_active());
            }
        });
        hbox.append(&active);
        let fav = gtk::CheckButton::builder()
            .active(ch.favorite)
            .tooltip_text("Favorit")
            .width_request(50)
            .build();
        let m = Rc::downgrade(self);
        fav.connect_toggled(move |b| {
            if let Some(m) = m.upgrade() {
                m.set_flag(&[id], |c| c.favorite = b.is_active());
            }
        });
        hbox.append(&fav);

        let row = gtk::ListBoxRow::builder().child(&hbox).build();
        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .build();
        let m = Rc::downgrade(self);
        let r = row.clone();
        click.connect_pressed(move |_, _, x, y| {
            if let Some(m) = m.upgrade() {
                m.context_menu(&r, id, x, y);
            }
        });
        row.add_controller(click);
        row
    }

    /// Ändert Felder ohne Neuaufbau der Tabelle (Häkchen), speichert und aktualisiert das Hauptfenster.
    fn set_flag(&self, ids: &[u64], f: impl Fn(&mut Channel)) {
        {
            let mut lib = self.app.library_mut();
            for id in ids {
                if let Some(c) = lib.channel_mut(*id) {
                    f(c);
                }
            }
        }
        self.app.library_changed();
    }

    /// Ausgewählte Sender; gehört der angeklickte nicht zur Auswahl, nur dieser.
    fn targets(&self, id: u64) -> Vec<u64> {
        let rows = self.rows.borrow();
        let selected: Vec<u64> = rows
            .iter()
            .filter(|(_, r)| r.is_selected())
            .map(|(i, _)| *i)
            .collect();
        if selected.contains(&id) {
            selected
        } else {
            if let Some((_, r)) = rows.iter().find(|(i, _)| *i == id) {
                self.list.unselect_all();
                self.list.select_row(Some(r));
            }
            vec![id]
        }
    }

    fn context_menu(&self, row: &gtk::ListBoxRow, id: u64, x: f64, y: f64) {
        let n = self.targets(id).len();
        let item = |label: &str, action: &str| {
            let it = gio::MenuItem::new(Some(label), None);
            it.set_action_and_target_value(Some(&format!("mgr.{action}")), Some(&id.to_variant()));
            it
        };
        let menu = gio::Menu::new();
        let s1 = gio::Menu::new();
        if n == 1 {
            s1.append_item(&item("Umbenennen…", "rename"));
            s1.append_item(&item("Nummer ändern…", "number"));
            s1.append_item(&item("EPG-Zuordnung…", "epg"));
        }
        s1.append_item(&item("Eigenen Namen löschen", "reset-name"));
        menu.append_section(None, &s1);
        let s2 = gio::Menu::new();
        s2.append_item(&item("Nach oben", "up"));
        s2.append_item(&item("Nach unten", "down"));
        menu.append_section(None, &s2);
        let s3 = gio::Menu::new();
        s3.append_item(&item("Aktivieren", "activate"));
        s3.append_item(&item("Deaktivieren", "deactivate"));
        s3.append_item(&item("Favorit an/aus", "favorite"));
        menu.append_section(None, &s3);
        let s4 = gio::Menu::new();
        s4.append_item(&item(
            &if n > 1 {
                format!("{n} Sender exportieren…")
            } else {
                "Exportieren…".into()
            },
            "export",
        ));
        s4.append_item(&item(
            &if n > 1 {
                format!("{n} Sender löschen…")
            } else {
                "Löschen…".into()
            },
            "delete",
        ));
        menu.append_section(None, &s4);

        let pop = gtk::PopoverMenu::from_model(Some(&menu));
        pop.set_parent(row);
        pop.set_has_arrow(false);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    fn install_actions(self: &Rc<Self>) {
        let group = gio::SimpleActionGroup::new();
        let entry = |name: &str, f: fn(&Rc<Manager>, u64)| {
            let m = Rc::downgrade(self);
            gio::ActionEntry::builder(name)
                .parameter_type(Some(glib::VariantTy::UINT64))
                .activate(move |_: &gio::SimpleActionGroup, _, param| {
                    if let (Some(m), Some(id)) = (m.upgrade(), param.and_then(|p| p.get::<u64>())) {
                        f(&m, id);
                    }
                })
                .build()
        };
        group.add_action_entries([
            entry("rename", |m, id| m.rename(id)),
            entry("number", |m, id| m.change_number(id)),
            entry("epg", |m, id| m.assign_epg(id)),
            entry("reset-name", |m, id| {
                let ids = m.targets(id);
                m.set_flag(&ids, |c| c.custom_name = None);
                m.refill();
            }),
            entry("up", |m, id| m.shift(id, -1)),
            entry("down", |m, id| m.shift(id, 1)),
            entry("activate", |m, id| {
                let ids = m.targets(id);
                m.set_flag(&ids, |c| c.active = true);
                m.refill();
            }),
            entry("deactivate", |m, id| {
                let ids = m.targets(id);
                m.set_flag(&ids, |c| c.active = false);
                m.refill();
            }),
            entry("favorite", |m, id| {
                let ids = m.targets(id);
                m.set_flag(&ids, |c| c.favorite = !c.favorite);
                m.refill();
            }),
            entry("export", |m, id| {
                let ids = m.targets(id);
                m.export(Some(ids));
            }),
            entry("delete", |m, id| m.delete(id)),
        ]);
        self.win.insert_action_group("mgr", Some(&group));
    }

    fn ask(
        &self,
        heading: &str,
        body: &str,
        extra: Option<&gtk::Widget>,
        ok: &str,
        destructive: bool,
    ) -> adw::AlertDialog {
        let d = adw::AlertDialog::new(Some(heading), Some(body));
        d.add_response("cancel", "Abbrechen");
        d.add_response("ok", ok);
        d.set_response_appearance(
            "ok",
            if destructive {
                adw::ResponseAppearance::Destructive
            } else {
                adw::ResponseAppearance::Suggested
            },
        );
        d.set_default_response(Some(if destructive { "cancel" } else { "ok" }));
        d.set_close_response("cancel");
        if let Some(w) = extra {
            d.set_extra_child(Some(w));
        }
        d
    }

    fn rename(self: &Rc<Self>, id: u64) {
        let current = self
            .app
            .library()
            .channel(id)
            .map(|c| c.display_name().to_string())
            .unwrap_or_default();
        let entry = gtk::Entry::builder()
            .text(&current)
            .activates_default(true)
            .build();
        let d = self.ask(
            "Sender umbenennen",
            "Eigener Name (bleibt bei Aktualisierungen erhalten):",
            Some(entry.upcast_ref()),
            "Umbenennen",
            false,
        );
        let m = self.clone();
        d.connect_response(Some("ok"), move |_, _| {
            let text = entry.text().trim().to_string();
            m.set_flag(&[id], |c| {
                c.custom_name = (!text.is_empty() && text != c.name).then(|| text.clone());
            });
            m.refill();
        });
        d.present(Some(&self.win));
    }

    fn change_number(self: &Rc<Self>, id: u64) {
        let current = self
            .app
            .library()
            .channel(id)
            .and_then(|c| c.number)
            .map(|n| n.to_string())
            .unwrap_or_default();
        let entry = gtk::Entry::builder()
            .text(&current)
            .input_purpose(gtk::InputPurpose::Digits)
            .activates_default(true)
            .build();
        let d = self.ask(
            "Nummer ändern",
            "Ziffernblocknummer (muss frei sein):",
            Some(entry.upcast_ref()),
            "Übernehmen",
            false,
        );
        let m = self.clone();
        d.connect_response(Some("ok"), move |_, _| {
            let Ok(n) = entry.text().trim().parse::<u32>() else {
                m.toast("Bitte eine Zahl eingeben.");
                return;
            };
            let result = m.app.library_mut().set_number(id, n);
            match result {
                Ok(()) => {
                    m.app.library_changed();
                    m.refill();
                }
                Err(NumberTaken(other)) => {
                    let name = m
                        .app
                        .library()
                        .channel(other)
                        .map(|c| c.display_name().to_string())
                        .unwrap_or_default();
                    m.toast(&format!("Nummer {n} gehört schon zu „{name}“."));
                }
            }
        });
        d.present(Some(&self.win));
    }

    fn assign_epg(self: &Rc<Self>, id: u64) {
        let Some(guide) = self.app.guide() else {
            self.toast("Kein EPG geladen – zuerst eine EPG-Quelle eintragen.");
            return;
        };
        let Some(ch) = self.app.library().channel(id).cloned() else {
            return;
        };
        let suggestions = guide.suggestions(&ch);
        let mut labels: Vec<String> = vec!["Automatisch zuordnen".into()];
        labels.extend(
            suggestions
                .iter()
                .map(|(sid, label)| format!("{label} ({sid})")),
        );
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let drop = gtk::DropDown::from_strings(&refs);
        drop.set_enable_search(true);
        drop.set_expression(Some(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<gtk::Expression>,
            "string",
        )));
        if let Some(cur) = ch.epg_id.as_deref() {
            if let Some(pos) = suggestions.iter().position(|(sid, _)| sid == cur) {
                drop.set_selected(pos as u32 + 1);
            }
        }
        let d = self.ask(
            "EPG-Zuordnung",
            &format!(
                "Programmdaten für „{}“ (passende Kanäle zuerst):",
                ch.display_name()
            ),
            Some(drop.upcast_ref()),
            "Übernehmen",
            false,
        );
        let m = self.clone();
        d.connect_response(Some("ok"), move |_, _| {
            let sel = drop.selected() as usize;
            let epg_id = sel
                .checked_sub(1)
                .and_then(|i| suggestions.get(i))
                .map(|(sid, _)| sid.clone());
            m.set_flag(&[id], |c| c.epg_id = epg_id.clone());
            m.refill();
        });
        d.present(Some(&self.win));
    }

    fn edit_epg_sources(self: &Rc<Self>) {
        let text = self.app.settings().epg_sources.join("\n");
        let buffer = gtk::TextBuffer::new(None);
        buffer.set_text(&text);
        let view = gtk::TextView::builder()
            .buffer(&buffer)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::Char)
            .height_request(120)
            .width_request(520)
            .build();
        let d = self.ask(
            "EPG-Quellen",
            "Eine XMLTV-Quelle je Zeile (Datei oder URL, auch .xml.gz):",
            Some(view.upcast_ref()),
            "Speichern und neu laden",
            false,
        );
        let m = self.clone();
        d.connect_response(Some("ok"), move |_, _| {
            let (a, b) = buffer.bounds();
            let sources: Vec<String> = buffer
                .text(&a, &b, false)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            m.app.settings_mut().epg_sources = sources;
            m.app.save_settings();
            m.app.load_epg(true);
            m.toast("EPG wird neu geladen …");
        });
        d.present(Some(&self.win));
    }

    /// Verschiebt die Auswahl um eine Position (Reihenfolge, nicht Nummer).
    fn shift(self: &Rc<Self>, id: u64, delta: i64) {
        let ids = self.targets(id);
        {
            let mut lib = self.app.library_mut();
            let mut order: Vec<u64> = ids.clone();
            if delta > 0 {
                order.reverse();
            }
            for cid in order {
                if let Some(pos) = lib.channels.iter().position(|c| c.id == cid) {
                    let to = (pos as i64 + delta).clamp(0, lib.channels.len() as i64 - 1) as usize;
                    lib.move_channel(cid, to);
                }
            }
        }
        self.app.library_changed();
        self.refill();
        let rows = self.rows.borrow();
        for (i, r) in rows.iter() {
            if ids.contains(i) {
                self.list.select_row(Some(r));
            }
        }
    }

    fn delete(self: &Rc<Self>, id: u64) {
        let ids = self.targets(id);
        let body = if ids.len() == 1 {
            let name = self
                .app
                .library()
                .channel(id)
                .map(|c| c.display_name().to_string())
                .unwrap_or_default();
            format!(
                "„{name}“ wird aus der Bibliothek entfernt. Ein erneuter Import der Quelle fügt ihn wieder hinzu."
            )
        } else {
            format!("{} Sender werden aus der Bibliothek entfernt.", ids.len())
        };
        let d = self.ask("Sender löschen?", &body, None, "Löschen", true);
        let m = self.clone();
        d.connect_response(Some("ok"), move |_, _| {
            {
                let mut lib = m.app.library_mut();
                for id in &ids {
                    lib.remove_channel(*id);
                }
            }
            m.app.library_changed();
            m.refill();
            m.toast("Gelöscht.");
        });
        d.present(Some(&self.win));
    }

    fn export(self: &Rc<Self>, ids: Option<Vec<u64>>) {
        let dialog = gtk::FileDialog::builder()
            .title("Sender als M3U8 exportieren")
            .initial_name("tvjoe-sender.m3u8")
            .modal(true)
            .build();
        let m = self.clone();
        dialog.save(Some(&self.win), gio::Cancellable::NONE, move |res| {
            let Ok(file) = res else { return };
            let Some(path) = file.path() else { return };
            let text = m.app.library().export_m3u(ids.as_deref());
            match std::fs::write(&path, text) {
                Ok(()) => m.toast(&format!("Gespeichert: {}", path.display())),
                Err(e) => m.toast(&format!("Speichern fehlgeschlagen: {e}")),
            }
        });
    }
}
