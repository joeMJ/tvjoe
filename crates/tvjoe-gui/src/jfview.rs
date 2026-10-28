//! Jellyfin-Ansicht, aufgebaut wie die ÖR-Mediathek: oben „Filme“, „Serien“, „Suche“, darunter
//! die Leiste der Bibliotheken, darunter Kacheln mit Filmplakat. Sammlungen (Filmreihen) und
//! Serien öffnen eine Seite mit ihren Teilen bzw. Staffeln und Folgen, Filme eine Filminfo.
//! Daten aus dem Zwischenspeicher (`jellyfin_lib`), Plakate vom Server (im Cache abgelegt).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tvjoe_core::jellyfin::{Account, ServerKind};
use tvjoe_core::jellyfin_lib::{Item, JellyfinLibrary};
use tvjoe_core::{net, paths, tf, tr, trn};

use crate::app::App;
use crate::{icons, trace};

const POSTER_W: i32 = 170;
const POSTER_H: i32 = 255;
const DETAIL_W: i32 = 300;
const DETAIL_H: i32 = 450;
/// Mindestfläche (Kopfzeile mit Reitern, zwei bis drei Plakatspalten).
const MIN_SIZE: (i32, i32) = (900, 600);
const IMAGE_THREADS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Movies,
    Collections,
    Series,
    Search,
}

thread_local! {
    /// Ansicht je Bibliothek (Kennung der Instanz).
    static VIEWS: RefCell<HashMap<String, Rc<JfView>>> = RefCell::new(HashMap::new());
}

// ---------------------------------------------------------------------------------------------
// Plakate

struct Images {
    requests: mpsc::Sender<(u64, String, String)>,
    generation: Arc<AtomicU64>,
}

impl Images {
    fn spawn(dir: PathBuf) -> (Self, async_channel::Receiver<(String, PathBuf)>) {
        let (req_tx, req_rx) = mpsc::channel::<(u64, String, String)>();
        let req_rx = Arc::new(Mutex::new(req_rx));
        let (res_tx, res_rx) = async_channel::unbounded();
        let generation = Arc::new(AtomicU64::new(0));
        for _ in 0..IMAGE_THREADS {
            let (req_rx, res_tx, generation, dir) = (
                req_rx.clone(),
                res_tx.clone(),
                generation.clone(),
                dir.clone(),
            );
            std::thread::spawn(move || {
                let agent = net::agent(Duration::from_secs(15));
                loop {
                    let next = req_rx.lock().ok().and_then(|r| r.recv().ok());
                    let Some((gen_, id, url)) = next else { break };
                    let path = dir.join(net::cache_name(&url, "jpg"));
                    if !path.exists() {
                        if gen_ != generation.load(Ordering::Relaxed) {
                            continue;
                        }
                        let saved = net::fetch(&agent, &url, 2 << 20).and_then(|b| {
                            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                            let tmp = path.with_extension("tmp");
                            std::fs::write(&tmp, b).map_err(|e| e.to_string())?;
                            std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
                        });
                        if let Err(e) = saved {
                            trace(&format!("Jellyfin-Plakat {id}: {e}"));
                            continue;
                        }
                    }
                    if res_tx.send_blocking((id, path)).is_err() {
                        break;
                    }
                }
            });
        }
        (
            Self {
                requests: req_tx,
                generation,
            },
            res_rx,
        )
    }

    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn request(&self, generation: u64, id: &str, url: String) {
        let _ = self.requests.send((generation, id.to_string(), url));
    }
}

/// Adresse des Plakats (Höhe in Pixeln); ohne Bild-Kennung kein Plakat.
fn poster_url(acc: Option<&Account>, it: &Item, height: i32) -> Option<String> {
    let acc = acc?;
    let tag = it.image_tag.as_deref()?;
    let server = &acc.server;
    Some(match acc.kind {
        ServerKind::Jellyfin => format!(
            "{server}/Items/{}/Images/Primary?maxHeight={height}&tag={tag}&quality=85",
            it.id
        ),
        // Plex: Pfad des Plakats, vom Server verkleinert (Seitenverhältnis 2:3).
        ServerKind::Plex => tvjoe_core::plex::poster_url(
            server,
            &acc.token,
            tag,
            (height * 2 / 3) as u32,
            height as u32,
        ),
    })
}

// ---------------------------------------------------------------------------------------------
// Hilfen

/// „1 h 33 min“ bzw. „45 min“.
fn runtime(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let m = (secs + 30) / 60;
    if m >= 60 {
        tf!("{h} h {min} min", h = m / 60, min = m % 60)
    } else {
        tf!("{min} min", min = m)
    }
}

/// „S01E04“.
fn episode_code(it: &Item) -> String {
    match (it.season_no, it.episode_no) {
        (Some(s), Some(e)) => format!("S{s:02}E{e:02}"),
        (None, Some(e)) => tf!("Folge {nr}", nr = e),
        _ => String::new(),
    }
}

/// Anteil gesehen (0–1), wenn angefangen und nicht fertig.
fn seen_fraction(it: &Item) -> Option<f64> {
    (!it.played && it.position_secs > 0.0 && it.runtime_secs > 0)
        .then(|| (it.position_secs / it.runtime_secs as f64).clamp(0.0, 1.0))
}

/// Weitersehen in einer geordneten Liste (Folgen, Teile einer Sammlung): zuletzt gespielter
/// Eintrag; angefangen → dort weiter, fertig → der nächste; sonst der erste.
fn next_in(list: &[Item]) -> Option<(usize, Option<f64>)> {
    if list.is_empty() {
        return None;
    }
    let last = list
        .iter()
        .enumerate()
        .filter(|(_, it)| it.played || it.position_secs > 0.0)
        .max_by(|a, b| a.1.last_played.cmp(&b.1.last_played));
    Some(match last {
        Some((i, it)) if !it.played && it.position_secs > 0.0 => (i, Some(it.position_secs)),
        Some((i, _)) if i + 1 < list.len() => (i + 1, None),
        Some((i, _)) => (i, None),
        None => (0, None),
    })
}

fn label(text: &str, class: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes([class])
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build()
}

/// Zusätzliche Angaben darstellen: Zeilen „Beschriftung – Wert“, Besetzung mit Fotos
/// (waagerecht blätterbar), Knöpfe zu externen Seiten.
fn fill_detail(area: &gtk::Box, info: &tvjoe_core::detail::Info) {
    if !info.fields.is_empty() {
        // Zeilen statt Tabelle: ein GtkGrid nahm der Seite das Umbrechen der Beschreibung.
        let rows = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        for (k, v) in &info.fields {
            let line = gtk::Box::builder().spacing(16).build();
            let key = label(k, "tvjoe-osd-sub");
            key.set_valign(gtk::Align::Start);
            key.set_width_request(96);
            let value = label(v, "tvjoe-mt-detail-desc");
            value.set_hexpand(true);
            line.append(&key);
            line.append(&value);
            rows.append(&line);
        }
        area.append(&rows);
    }
    for (title, people) in &info.people {
        area.append(&label(title, "tvjoe-osd-sub"));
        let row = gtk::Box::builder().spacing(12).build();
        for p in people {
            let tile = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .width_request(96)
                .build();
            let pic = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .width_request(96)
                .height_request(144)
                .css_classes(["tvjoe-mt-thumb"])
                .overflow(gtk::Overflow::Hidden)
                .build();
            tile.append(&pic);
            let name = label(&p.name, "tvjoe-mt-detail-desc");
            name.set_max_width_chars(12);
            tile.append(&name);
            if !p.role.is_empty() {
                let role = label(&p.role, "tvjoe-osd-sub");
                role.set_max_width_chars(12);
                tile.append(&role);
            }
            if let Some(url) = p.photo.clone() {
                let (tx, rx) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let _ = tx.send_blocking(net::fetch(
                        &net::agent(std::time::Duration::from_secs(15)),
                        &url,
                        2 << 20,
                    ));
                });
                let pic = pic.clone();
                glib::spawn_future_local(async move {
                    if let Ok(Ok(bytes)) = rx.recv().await {
                        if let Ok(t) = gtk::gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) {
                            pic.set_paintable(Some(&t));
                        }
                    }
                });
            }
            row.append(&tile);
        }
        // Die Reihe darf die Seite nicht verbreitern (sonst laufen die Texte aus dem Bild).
        area.append(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Automatic)
                .vscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_width(false)
                .min_content_width(1)
                .hexpand(true)
                .child(&row)
                .build(),
        );
    }
    if !info.links.is_empty() {
        let row = gtk::Box::builder().spacing(8).build();
        for (title, url) in &info.links {
            let b = gtk::Button::builder()
                .label(title)
                .css_classes(["pill"])
                .tooltip_text(url)
                .build();
            let url = url.clone();
            b.connect_clicked(move |b| {
                let parent = b.root().and_downcast::<gtk::Window>();
                gtk::UriLauncher::new(&url).launch(parent.as_ref(), gtk::gio::Cancellable::NONE, |_| {});
            });
            row.append(&b);
        }
        area.append(&row);
    }
}

/// Abspielen im Hauptfenster; als Ebene schließt sich die Ansicht, als Fenster kommt das
/// Hauptfenster nach vorn.
fn play(app: &Rc<App>, library: &str, item: &Item, start: Option<f64>, rest: Vec<Item>) {
    app.play_jellyfin(library, item, start, rest);
    if let Some(v) = VIEWS.with(|v| v.borrow().get(library).cloned()) {
        let view: Rc<dyn crate::layer::View> = v;
        let in_layer = crate::layer::window_of(app, &view)
            .is_some_and(|w| w == *app.window.upcast_ref::<gtk::Window>());
        if in_layer {
            crate::layer::close_if_layer(app, &view);
        } else {
            app.window.present();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Ansicht

/// Heruntergeladenes aus der Ablage: Filme und Folgen direkt, Serien über ihre Folgen,
/// Sammlungen über ihre Teile.
struct Stored {
    keys: HashSet<String>,
    /// Serie → Zahl der heruntergeladenen Folgen.
    series: HashMap<String, i64>,
}

impl Stored {
    fn load(library: &str) -> Self {
        let keys = crate::ablage::stored_keys(library);
        let mut series: HashMap<String, i64> = HashMap::new();
        if let Some(d) = db(library) {
            for id in keys.iter().filter_map(|k| d.item(k)?.series_id) {
                *series.entry(id).or_default() += 1;
            }
        }
        Self { keys, series }
    }

    fn keeps(&self, db: &JellyfinLibrary, it: &Item) -> bool {
        match it.kind.as_str() {
            "Series" => self.series.contains_key(&it.id),
            "BoxSet" => self.parts(db, &it.id) > 0,
            _ => self.keys.contains(&it.id),
        }
    }

    /// Heruntergeladene Teile einer Sammlung.
    fn parts(&self, db: &JellyfinLibrary, id: &str) -> i64 {
        db.collection(id)
            .iter()
            .filter(|p| self.keys.contains(&p.id))
            .count() as i64
    }
}

struct JfView {
    app: Weak<App>,
    /// Bibliothek (Kennung der Instanz).
    source: String,
    body: gtk::Widget,
    header_start: gtk::Box,
    header_center: gtk::Widget,
    header_end: gtk::Box,
    tabs: [gtk::ToggleButton; 4],
    search: gtk::SearchEntry,
    search_bar: gtk::Box,
    libbar: gtk::Box,
    libbar_scroller: gtk::ScrolledWindow,
    status: gtk::Label,
    stack: gtk::Stack,
    empty: adw::StatusPage,
    flow: gtk::FlowBox,
    tab: Cell<Tab>,
    library: RefCell<Option<String>>,
    items: RefCell<Vec<Item>>,
    posters: RefCell<HashMap<String, gtk::Picture>>,
    images: Option<Images>,
    /// Zugang (Server, für Plex auch der Schlüssel der Plakate).
    account: Option<Account>,
    detail_open: Cell<bool>,
}

/// Jellyfin-Eintrag aus der Bibliothek (Ablage).
pub fn item(library: &str, id: &str) -> Option<Item> {
    db(library)?.item(id)
}

/// Folgen einer Staffel in der Bibliothek (Ablage: „Prüfen“).
pub fn season_count(library: &str, series_id: &str, season: Option<i64>) -> usize {
    db(library).map_or(0, |d| {
        d.episodes(series_id)
            .iter()
            .filter(|e| e.season_no == season)
            .count()
    })
}

fn db(library: &str) -> Option<JellyfinLibrary> {
    paths::library_db(library)
        .filter(|p| p.exists())
        .and_then(|p| JellyfinLibrary::open(&p).ok())
}

pub fn open(app: &Rc<App>, library: &str) {
    let existing = VIEWS.with(|v| v.borrow().get(library).cloned());
    let view = match existing {
        Some(v) => v,
        None => {
            let v = JfView::new(app, library);
            VIEWS.with(|slot| slot.borrow_mut().insert(library.to_string(), v.clone()));
            v
        }
    };
    view.rebuild();
    crate::layer::open(app, view);
}

/// Nach einem Abgleich oder geänderten Einstellungen: eine offene Ansicht neu aufbauen.
pub fn refresh(library: &str) {
    if let Some(v) = VIEWS.with(|v| v.borrow().get(library).cloned()) {
        if v.body.is_mapped() && !v.detail_open.get() {
            v.rebuild();
        }
    }
}

/// Selbstprobe: öffnen, Reiter wählen, erste Kachel öffnen.
/// Bibliothek der Selbstprobe (`TVJOE_SELFTEST_LIBRARY`, sonst die erste Jellyfin-Bibliothek).
fn probe_library() -> String {
    std::env::var("TVJOE_SELFTEST_LIBRARY")
        .unwrap_or_else(|_| tvjoe_core::settings::LIBRARY_JELLYFIN.to_string())
}

pub fn probe_open(app: &Rc<App>, tab: usize) -> Option<gtk::Window> {
    let library = probe_library();
    open(app, &library);
    let v = VIEWS.with(|v| v.borrow().get(&library).cloned())?;
    v.tabs.get(tab)?.set_active(true);
    let view: Rc<dyn crate::layer::View> = v;
    crate::layer::window_of(app, &view)
}

pub fn probe_first(app: &Rc<App>) -> Option<gtk::Window> {
    let v = VIEWS.with(|v| v.borrow().get(&probe_library()).cloned())?;
    let first = v.items.borrow().first().cloned()?;
    v.activate(&first);
    let view: Rc<dyn crate::layer::View> = v;
    crate::layer::window_of(app, &view)
}

impl crate::layer::View for JfView {
    fn title(&self) -> String {
        self.app
            .upgrade()
            .and_then(|a| a.settings().library(&self.source).map(|l| l.title.clone()))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Jellyfin".to_string())
    }
    fn min_size(&self) -> (i32, i32) {
        MIN_SIZE
    }
    fn body(&self) -> gtk::Widget {
        self.body.clone()
    }
    fn header_start(&self) -> Option<gtk::Widget> {
        Some(self.header_start.clone().upcast())
    }
    fn header_center(&self) -> Option<gtk::Widget> {
        Some(self.header_center.clone())
    }
    fn header_end(&self) -> Option<gtk::Widget> {
        Some(self.header_end.clone().upcast())
    }
    fn back(&self) -> bool {
        if self.detail_open.get() {
            self.close_detail();
            true
        } else {
            false
        }
    }
    fn can_go_back(&self) -> bool {
        self.detail_open.get()
    }
    fn media(&self) -> Option<crate::layer::Media> {
        Some(crate::layer::Media::Server(self.source.clone()))
    }
}

impl JfView {
    fn new(app: &Rc<App>, library: &str) -> Rc<Self> {
        let lib = app.settings().library(library).cloned().unwrap_or_default();
        let tabs = [
            icons::toggle(icons::FILM, tr("Filme")),
            icons::toggle(icons::COLLECTION, tr("Sammlungen")),
            icons::toggle(icons::SERIES, tr("Serien")),
            icons::toggle("system-search-symbolic", tr("Suche")),
        ];
        for t in &tabs[1..] {
            t.set_group(Some(&tabs[0]));
        }
        tabs[0].set_active(true);
        let tab_box = gtk::Box::builder().css_classes(["linked"]).build();
        for t in &tabs {
            tab_box.append(t);
        }
        let status = gtk::Label::builder().css_classes(["tvjoe-sub"]).build();
        let header_start = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header_start.append(&status);

        let newest = gtk::ToggleButton::builder().label(tr("Neueste")).build();
        let az = gtk::ToggleButton::builder()
            .label("A–Z")
            .group(&newest)
            .build();
        if lib.az {
            az.set_active(true);
        } else {
            newest.set_active(true);
        }
        let sort_box = gtk::Box::builder()
            .css_classes(["linked"])
            .valign(gtk::Align::Center)
            .build();
        sort_box.append(&newest);
        sort_box.append(&az);
        // Schalter „Geladen“: nur Heruntergeladenes, in allen Reitern wie sonst gruppiert (Nutzer, 2026-10-27).
        let stored_only = icons::toggle(icons::DOWNLOADED, tr("Geladen"));
        stored_only.set_valign(gtk::Align::Center);
        stored_only.add_css_class("tvjoe-filter");
        stored_only.set_active(lib.stored_only);
        let prefs_button = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text(tr("Einstellungen von Jellyfin"))
            .build();
        let header_end = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header_end.append(&stored_only);
        header_end.append(&sort_box);
        header_end.append(&prefs_button);

        let search = gtk::SearchEntry::builder()
            .placeholder_text(tr("Titel, Serie oder Beschreibung"))
            .hexpand(true)
            .build();
        let search_bar = gtk::Box::builder()
            .margin_start(16)
            .margin_end(16)
            .margin_top(8)
            .visible(false)
            .build();
        search_bar.append(&search);
        let libbar = gtk::Box::builder()
            .spacing(6)
            .margin_start(16)
            .margin_end(16)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        let libbar_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&libbar)
            .build();
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .activate_on_single_click(true)
            .homogeneous(true)
            .max_children_per_line(30)
            .column_spacing(14)
            .row_spacing(18)
            .margin_start(16)
            .margin_end(16)
            .margin_top(8)
            .margin_bottom(16)
            .valign(gtk::Align::Start)
            .build();
        let grid_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&flow)
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("network-server-symbolic")
            .vexpand(true)
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&grid_scroller, Some("grid"));
        stack.add_named(&empty, Some("empty"));
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&search_bar);
        body.append(&libbar_scroller);
        body.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        body.append(&stack);

        let (images, results) = match paths::thumb_cache_dir().map(Images::spawn) {
            Some((i, r)) => (Some(i), Some(r)),
            None => (None, None),
        };
        let account = crate::jellyfin::account(library);
        let v = Rc::new(Self {
            app: Rc::downgrade(app),
            source: library.to_string(),
            body: body.upcast(),
            header_start,
            header_center: tab_box.upcast(),
            header_end,
            tabs,
            search,
            search_bar,
            libbar,
            libbar_scroller,
            status,
            stack,
            empty,
            flow,
            tab: Cell::new(Tab::Movies),
            library: RefCell::new(None),
            items: RefCell::new(Vec::new()),
            posters: RefCell::new(HashMap::new()),
            images,
            account,
            detail_open: Cell::new(false),
        });
        if let Some(results) = results {
            let weak = Rc::downgrade(&v);
            glib::spawn_future_local(async move {
                while let Ok((id, path)) = results.recv().await {
                    let Some(v) = weak.upgrade() else { break };
                    if let Some(p) = v.posters.borrow().get(&id) {
                        p.set_filename(Some(&path));
                    }
                }
            });
        }
        for (i, t) in [Tab::Movies, Tab::Collections, Tab::Series, Tab::Search]
            .into_iter()
            .enumerate()
        {
            let weak = Rc::downgrade(&v);
            v.tabs[i].connect_toggled(move |b| {
                let Some(v) = weak.upgrade() else { return };
                if b.is_active() {
                    v.tab.set(t);
                    v.search_bar.set_visible(t == Tab::Search);
                    if t == Tab::Search {
                        v.search.grab_focus();
                    }
                    v.close_detail();
                    v.rebuild();
                }
            });
        }
        let weak = Rc::downgrade(&v);
        v.search.connect_search_changed(move |_| {
            if let Some(v) = weak.upgrade() {
                v.reload();
            }
        });
        let weak = Rc::downgrade(&v);
        v.flow.connect_child_activated(move |_, child| {
            let Some(v) = weak.upgrade() else { return };
            let it = usize::try_from(child.index())
                .ok()
                .and_then(|i| v.items.borrow().get(i).cloned());
            if let Some(it) = it {
                v.activate(&it);
            }
        });
        let weak = Rc::downgrade(&v);
        az.connect_toggled(move |b| {
            let Some(v) = weak.upgrade() else { return };
            if let Some(app) = v.app.upgrade() {
                if let Some(l) = app.settings_mut().library_mut(&v.source) {
                    l.az = b.is_active();
                }
                app.save_settings();
            }
            v.reload();
        });
        let weak = Rc::downgrade(&v);
        stored_only.connect_toggled(move |b| {
            let Some(v) = weak.upgrade() else { return };
            if let Some(app) = v.app.upgrade() {
                if let Some(l) = app.settings_mut().library_mut(&v.source) {
                    l.stored_only = b.is_active();
                }
                app.save_settings();
            }
            v.close_detail();
            v.reload();
        });
        let weak = Rc::downgrade(&v);
        prefs_button.connect_clicked(move |_| {
            let Some(v) = weak.upgrade() else { return };
            if let Some(app) = v.app.upgrade() {
                crate::mediatheken::open(&app, &v.source, &v.body);
            }
        });
        v
    }

    fn hidden(&self) -> Vec<String> {
        self.app
            .upgrade()
            .and_then(|a| a.settings().library(&self.source).map(|l| l.hidden.clone()))
            .unwrap_or_default()
    }

    fn az(&self) -> bool {
        self.app
            .upgrade()
            .and_then(|a| a.settings().library(&self.source).map(|l| l.az))
            .unwrap_or(false)
    }

    /// Schalter „Geladen“: Kennungen des Heruntergeladenen, sonst `None`.
    fn stored_only(&self) -> Option<Stored> {
        self.app
            .upgrade()
            .and_then(|a| a.settings().library(&self.source).map(|l| l.stored_only))
            .unwrap_or(false)
            .then(|| Stored::load(&self.source))
    }

    fn rebuild(self: &Rc<Self>) {
        self.build_libbar();
        self.reload();
    }

    /// Leiste der Film-Bibliotheken („Alle“ und je sichtbare Bibliothek).
    fn build_libbar(self: &Rc<Self>) {
        while let Some(c) = self.libbar.first_child() {
            self.libbar.remove(&c);
        }
        let hidden = self.hidden();
        let libs: Vec<String> = db(&self.source)
            .map(|d| d.libraries())
            .unwrap_or_default()
            .into_iter()
            .filter(|l| l.kind != "tvshows" && l.kind != "boxsets" && !hidden.contains(&l.name))
            .map(|l| l.name)
            .collect();
        // Nur bei mehreren Film-Bibliotheken; bei einer zeigten „Alle“ und sie dasselbe.
        let many = libs.len() > 1;
        self.libbar_scroller
            .set_visible(many && self.tab.get() == Tab::Movies);
        if self
            .library
            .borrow()
            .as_ref()
            .is_some_and(|l| !many || !libs.contains(l))
        {
            self.library.replace(None);
        }
        if !many {
            return;
        }
        let all = gtk::ToggleButton::builder()
            .label(tr("Alle"))
            .css_classes(["tvjoe-mt-sender"])
            .active(self.library.borrow().is_none())
            .build();
        self.libbar.append(&all);
        let weak = Rc::downgrade(self);
        all.connect_toggled(move |b| {
            if let (true, Some(v)) = (b.is_active(), weak.upgrade()) {
                v.library.replace(None);
                v.reload();
            }
        });
        for name in libs {
            let b = gtk::ToggleButton::builder()
                .label(&name)
                .css_classes(["tvjoe-mt-sender"])
                .group(&all)
                .active(self.library.borrow().as_deref() == Some(name.as_str()))
                .build();
            let weak = Rc::downgrade(self);
            b.connect_toggled(move |b| {
                if let (true, Some(v)) = (b.is_active(), weak.upgrade()) {
                    v.library.replace(Some(name.clone()));
                    v.reload();
                }
            });
            self.libbar.append(&b);
        }
    }

    fn show_status(&self, db: &JellyfinLibrary) {
        let when = db
            .last_sync()
            .and_then(|t| glib::DateTime::from_unix_local(t).ok())
            .and_then(|d| d.format("%d.%m. %H:%M").ok());
        let (movies, series) = (db.count("Movie"), db.count("Series"));
        self.status.set_label(&match when {
            Some(when) => tf!(
                "Stand {when} · {filme} Filme · {serien} Serien",
                when = when,
                filme = movies,
                serien = series
            ),
            None => tf!(
                "{filme} Filme · {serien} Serien",
                filme = movies,
                serien = series
            ),
        });
    }

    fn show_empty(&self, title: &str, text: &str) {
        self.empty.set_title(title);
        self.empty.set_description(Some(text));
        self.stack.set_visible_child_name("empty");
    }

    /// Kacheln des Reiters neu laden.
    fn reload(self: &Rc<Self>) {
        let Some(db) = db(&self.source) else {
            self.items.borrow_mut().clear();
            self.show_empty(
                tr("Bibliothek wird abgeglichen"),
                tr("Der erste Abgleich mit dem Jellyfin-Server dauert etwa eine halbe Minute."),
            );
            return;
        };
        self.show_status(&db);
        let hidden = self.hidden();
        let stored = self.stored_only();
        let items = match self.tab.get() {
            Tab::Movies => db.single_movies(&hidden, self.library.borrow().as_deref(), self.az()),
            Tab::Collections => db.collections(&hidden, self.az()),
            Tab::Series => db.series(&hidden, self.az()),
            Tab::Search => {
                let text = self.search.text();
                if text.trim().chars().count() < 2 {
                    self.items.borrow_mut().clear();
                    self.show_empty(tr("Suche"), tr("Mindestens zwei Zeichen eingeben."));
                    return;
                }
                db.search(text.trim(), &hidden, 300)
            }
        };
        let items: Vec<Item> = match &stored {
            Some(st) => items.into_iter().filter(|it| st.keeps(&db, it)).collect(),
            None => items,
        };
        while let Some(c) = self.flow.first_child() {
            self.flow.remove(&c);
        }
        self.posters.borrow_mut().clear();
        if items.is_empty() {
            self.items.borrow_mut().clear();
            self.show_empty(
                tr("Keine Einträge"),
                if stored.is_some() {
                    tr("Für diese Auswahl ist nichts heruntergeladen.")
                } else {
                    tr("Für diese Auswahl gibt es nichts.")
                },
            );
            return;
        }
        let generation = self.images.as_ref().map_or(0, Images::next_generation);
        for it in &items {
            let sub = match it.kind.as_str() {
                // Mit „Geladen“: nur die heruntergeladenen Teile bzw. Folgen.
                "BoxSet" => trn(
                    "Sammlung · {n} Film",
                    "Sammlung · {n} Filme",
                    match &stored {
                        Some(st) => st.parts(&db, &it.id),
                        None => db.collection_count(&it.id),
                    } as u64,
                ),
                "Series" => trn(
                    "Serie · {n} Folge",
                    "Serie · {n} Folgen",
                    match &stored {
                        Some(st) => st.series.get(&it.id).copied().unwrap_or(0),
                        None => db.episode_count(&it.id),
                    } as u64,
                ),
                "Episode" => format!(
                    "{} · {}",
                    it.series_name.clone().unwrap_or_default(),
                    episode_code(it)
                ),
                _ => [
                    it.year.map(|y| y.to_string()).unwrap_or_default(),
                    runtime(it.runtime_secs),
                ]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
            };
            let badge = (!matches!(it.kind.as_str(), "Series" | "BoxSet"))
                .then(|| crate::ablage::item_badge(&self.app, &self.source, it));
            let (tile, poster) = tile(it, &sub, badge.as_ref());
            match it.kind.as_str() {
                "Series" => series_menu(&self.app, &self.source, &tile, it.id.clone()),
                "BoxSet" => {}
                _ => item_menu(&self.app, &self.source, &tile, it.clone()),
            }
            self.flow.append(&tile);
            if let (Some(img), Some(url)) =
                (&self.images, poster_url(self.account.as_ref(), it, POSTER_H * 3 / 2))
            {
                self.posters.borrow_mut().insert(it.id.clone(), poster);
                img.request(generation, &it.id, url);
            }
        }
        self.items.replace(items);
        self.stack.set_visible_child_name("grid");
    }

    /// Kachel geklickt: Film → Filminfo, Sammlung → Teile, Serie/Folge → Serienseite.
    fn activate(self: &Rc<Self>, it: &Item) {
        match it.kind.as_str() {
            "BoxSet" => {
                let parts = db(&self.source).map(|d| d.collection(&it.id)).unwrap_or_default();
                self.show_list(it, self.only_stored(parts), false);
            }
            "Series" => {
                let eps = db(&self.source).map(|d| d.episodes(&it.id)).unwrap_or_default();
                self.show_list(it, self.only_stored(eps), true);
            }
            "Episode" => {
                let series = it
                    .series_id
                    .as_deref()
                    .and_then(|id| db(&self.source).and_then(|d| d.item(id)));
                match series {
                    Some(s) => self.activate(&s),
                    None => self.show_movie(it),
                }
            }
            _ => self.show_movie(it),
        }
    }

    /// Zusätzliche Angaben (Video, Ton, Besetzung, Links …) nach der Beschreibung `[detail]` des
    /// Adapters; im Hintergrund beim Server abgefragt, die Fläche füllt sich danach.
    fn detail_box(&self, it: &Item) -> gtk::Box {
        let area = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .margin_top(10)
            .build();
        let kind = self
            .app
            .upgrade()
            .and_then(|a| a.settings().library(&self.source).map(|l| l.kind.clone()))
            .unwrap_or_default();
        let dir = paths::config_dir().map(|d| d.join("adapter"));
        let detail = tvjoe_core::adapter::libraries(dir.as_deref())
            .0
            .into_iter()
            .find(|d| d.id == kind)
            .and_then(|d| d.detail);
        let (Some(detail), Some(acc)) = (detail, self.account.clone()) else {
            return area;
        };
        let id = it.id.clone();
        let english = tvjoe_core::i18n::lang() != "de";
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(tvjoe_core::detail::load(&detail, &acc, &id, english));
        });
        let a = area.clone();
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(info)) => fill_detail(&a, &info),
                Ok(Err(e)) => trace(&format!("Zusatzangaben: {e}")),
                Err(_) => {}
            }
        });
        area
    }

    /// Teile einer Sammlung bzw. Folgen einer Serie; mit dem Schalter nur Heruntergeladenes.
    fn only_stored(&self, list: Vec<Item>) -> Vec<Item> {
        match self.stored_only() {
            Some(st) => list.into_iter().filter(|it| st.keys.contains(&it.id)).collect(),
            None => list,
        }
    }

    fn set_detail(&self, open: bool) {
        self.detail_open.set(open);
        if let Some(app) = self.app.upgrade() {
            crate::layer::update_header(&app);
        }
    }

    fn close_detail(&self) {
        if !self.detail_open.get() {
            return;
        }
        self.set_detail(false);
        self.stack.set_visible_child_name("grid");
        if let Some(old) = self.stack.child_by_name("detail") {
            self.stack.remove(&old);
        }
    }

    fn present_detail(&self, page: &gtk::Widget) {
        if let Some(old) = self.stack.child_by_name("detail") {
            self.stack.remove(&old);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(page)
            .build();
        self.stack.add_named(&scroller, Some("detail"));
        self.stack.set_visible_child_name("detail");
        self.set_detail(true);
    }

    /// Großes Plakat (schon geladenes Kachelbild, sonst vom Server).
    fn big_poster(&self, it: &Item) -> gtk::Widget {
        let pic = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_shrink(true)
            .build();
        if let Some(url) = poster_url(self.account.as_ref(), it, DETAIL_H * 3 / 2) {
            if let Some(dir) = paths::thumb_cache_dir() {
                let path = dir.join(net::cache_name(&url, "jpg"));
                if path.exists() {
                    pic.set_filename(Some(&path));
                } else if let Some(t) = self
                    .posters
                    .borrow()
                    .get(&it.id)
                    .and_then(|p| p.paintable())
                {
                    pic.set_paintable(Some(&t));
                }
            }
        }
        let frame = gtk::Overlay::builder()
            .child(&pic)
            .css_classes(["tvjoe-mt-thumb"])
            .overflow(gtk::Overflow::Hidden)
            .width_request(DETAIL_W)
            .height_request(DETAIL_H)
            .valign(gtk::Align::Start)
            .build();
        adw::Clamp::builder()
            .orientation(gtk::Orientation::Vertical)
            .maximum_size(DETAIL_H)
            .tightening_threshold(DETAIL_H)
            .valign(gtk::Align::Start)
            .child(
                &adw::Clamp::builder()
                    .orientation(gtk::Orientation::Horizontal)
                    .maximum_size(DETAIL_W)
                    .tightening_threshold(DETAIL_W)
                    .child(&frame)
                    .build(),
            )
            .build()
            .upcast()
    }

    fn page(&self, poster: &gtk::Widget, right: &gtk::Box) -> gtk::Widget {
        let page = gtk::Box::builder()
            .spacing(32)
            .margin_start(32)
            .margin_end(32)
            .margin_top(32)
            .margin_bottom(32)
            .build();
        page.append(poster);
        page.append(right);
        page.upcast()
    }

    /// Filminfo: Plakat, Titel, Jahr · Laufzeit · Genres, Weitersehen/Von vorn, Beschreibung.
    fn show_movie(self: &Rc<Self>, it: &Item) {
        let right = gtk::Box::new(gtk::Orientation::Vertical, 10);
        right.set_hexpand(true);
        right.append(&label(&it.name, "tvjoe-mt-detail-title"));
        let meta: Vec<String> = [
            it.year.map(|y| y.to_string()).unwrap_or_default(),
            runtime(it.runtime_secs),
            it.genres.clone(),
            if it.played {
                tr("gesehen").to_string()
            } else {
                String::new()
            },
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
        right.append(&label(&meta.join(" · "), "tvjoe-osd-sub"));
        let buttons = gtk::Box::builder()
            .spacing(10)
            .margin_top(6)
            .halign(gtk::Align::Start)
            .build();
        let resume = seen_fraction(it).map(|_| it.position_secs);
        if let Some(pos) = resume {
            buttons.append(&self.play_button(
                &tf!("Weitersehen ab {zeit}", zeit = crate::osd::clock(pos)),
                true,
                it,
                Some(pos),
                Vec::new(),
            ));
        }
        buttons.append(&self.play_button(
            if resume.is_some() {
                tr("Von vorn")
            } else {
                tr("Abspielen")
            },
            resume.is_none(),
            it,
            None,
            Vec::new(),
        ));
        buttons.append(&crate::ablage::item_store_button(&self.app, &self.source, it));
        right.append(&buttons);
        let desc = label(it.overview.trim(), "tvjoe-mt-detail-desc");
        desc.set_margin_top(10);
        right.append(&desc);
        right.append(&self.detail_box(it));
        let poster = self.big_poster(it);
        self.present_detail(&self.page(&poster, &right));
    }

    fn play_button(
        self: &Rc<Self>,
        text: &str,
        suggested: bool,
        it: &Item,
        start: Option<f64>,
        rest: Vec<Item>,
    ) -> gtk::Button {
        let b = gtk::Button::builder()
            .css_classes(if suggested {
                vec!["suggested-action", "pill"]
            } else {
                vec!["pill"]
            })
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("media-playback-start-symbolic")
                    .label(text)
                    .build(),
            )
            .build();
        let weak = Rc::downgrade(self);
        let it = it.clone();
        b.connect_clicked(move |_| {
            let Some(v) = weak.upgrade() else { return };
            if let Some(app) = v.app.upgrade() {
                play(&app, &v.source, &it, start, rest.clone());
            }
        });
        b
    }

    /// Seite einer Sammlung (Teile nach Jahr) oder Serie (Staffeln, Folgen): Weitersehen,
    /// Liste; ein Klick spielt den Eintrag, danach laufen die folgenden automatisch.
    fn show_list(self: &Rc<Self>, head: &Item, list: Vec<Item>, series: bool) {
        let list = Rc::new(list);
        let right = gtk::Box::new(gtk::Orientation::Vertical, 10);
        right.set_hexpand(true);
        right.append(&label(&head.name, "tvjoe-mt-detail-title"));
        let count = if series {
            trn("{n} Folge", "{n} Folgen", list.len() as u64)
        } else {
            trn("{n} Film", "{n} Filme", list.len() as u64)
        };
        let mut meta = vec![count];
        if !head.genres.is_empty() {
            meta.push(head.genres.clone());
        }
        right.append(&label(&meta.join(" · "), "tvjoe-osd-sub"));
        if let Some((i, start)) = next_in(&list) {
            let target = &list[i];
            let what = if series {
                format!("{} {}", episode_code(target), target.name)
            } else {
                target.name.clone()
            };
            let text = match start {
                Some(p) => tf!(
                    "Weitersehen: {was} ab {zeit}",
                    was = what,
                    zeit = crate::osd::clock(p)
                ),
                None if list.iter().any(|x| x.played || x.position_secs > 0.0) => {
                    tf!("Weitersehen: {was}", was = what)
                }
                None => tf!("Ansehen ab {was}", was = what),
            };
            let b = self.play_button(&text, true, target, start, list[i + 1..].to_vec());
            b.set_halign(gtk::Align::Start);
            b.set_margin_top(6);
            right.append(&b);
        }
        if !head.overview.is_empty() {
            let d = label(head.overview.trim(), "tvjoe-osd-sub");
            d.set_lines(4);
            d.set_ellipsize(gtk::pango::EllipsizeMode::End);
            right.append(&d);
        }
        right.append(&self.detail_box(head));

        // Staffelwähler (nur Serien mit mehreren Staffeln) und Liste.
        let rows = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .margin_top(10)
            .build();
        // Serie: gewählte Staffel herunterladen (Beschriftung folgt der Staffelwahl).
        let current: Rc<Cell<Option<Option<i64>>>> = Rc::new(Cell::new(None));
        let download = gtk::Button::builder()
            .css_classes(["pill"])
            .halign(gtk::Align::Start)
            .margin_top(10)
            .visible(series)
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("folder-download-symbolic")
                    .build(),
            )
            .build();
        {
            let (app, list, current) = (self.app.clone(), list.clone(), current.clone());
            let library = self.source.clone();
            download.connect_clicked(move |_| {
                let Some(app) = app.upgrade() else { return };
                let season = current.get();
                for it in list.iter().filter(|it| season.is_none_or(|s| it.season_no == s)) {
                    crate::ablage::fetch_item(&app, &library, it, tvjoe_core::ablage::Art::Download);
                }
            });
        }
        let fill = {
            let (rows, list, app) = (rows.clone(), list.clone(), self.app.clone());
            let library = self.source.clone();
            let (current, download) = (current.clone(), download.clone());
            move |season: Option<Option<i64>>| {
                current.set(season);
                if let Some(c) = download.child().and_downcast::<adw::ButtonContent>() {
                    c.set_label(&jf_season_label(&list, season));
                }
                while let Some(c) = rows.first_child() {
                    rows.remove(&c);
                }
                for (i, it) in list.iter().enumerate() {
                    if season.is_none_or(|s| it.season_no == s) {
                        let row = list_row(i, it, series, crate::ablage::item_badge(&app, &library, it));
                        item_menu(&app, &library, &row, it.clone());
                        rows.append(&row);
                    }
                }
            }
        };
        let mut seasons: Vec<Option<i64>> = Vec::new();
        if series {
            for it in list.iter() {
                if !seasons.contains(&it.season_no) {
                    seasons.push(it.season_no);
                }
            }
        }
        let start_season = next_in(&list).map(|(i, _)| list[i].season_no);
        if seasons.len() > 1 {
            fill(Some(start_season.flatten()));
            let fill = Rc::new(fill);
            let row = gtk::Box::builder()
                .css_classes(["linked"])
                .halign(gtk::Align::Start)
                .margin_top(10)
                .build();
            let mut first: Option<gtk::ToggleButton> = None;
            for s in seasons {
                let b = gtk::ToggleButton::builder()
                    .label(match s {
                        Some(0) => tr("Extras").to_string(),
                        Some(n) => tf!("Staffel {nr}", nr = n),
                        None => tr("Weitere").to_string(),
                    })
                    .active(Some(s) == start_season)
                    .build();
                match &first {
                    Some(g) => b.set_group(Some(g)),
                    None => first = Some(b.clone()),
                }
                let fill = fill.clone();
                b.connect_toggled(move |b| {
                    if b.is_active() {
                        fill(Some(s));
                    }
                });
                // Rechtsklick auf den Staffelknopf: diese Staffel oder alle Folgen laden.
                let (weak, list, library) = (self.app.clone(), list.clone(), self.source.clone());
                crate::ablage::context_menu(&b, move || {
                    let Some(app) = weak.upgrade() else {
                        return Vec::new();
                    };
                    let season: Vec<Item> =
                        list.iter().filter(|e| e.season_no == s).cloned().collect();
                    let all: Vec<Item> = list.to_vec();
                    let fetch = |items: Vec<Item>| -> Box<dyn Fn()> {
                        let (app, library) = (app.clone(), library.clone());
                        Box::new(move || {
                            for it in &items {
                                crate::ablage::fetch_item(&app, &library, it, tvjoe_core::ablage::Art::Download);
                            }
                        })
                    };
                    vec![
                        (
                            tf!("Diese Staffel herunterladen ({n})", n = season.len()),
                            fetch(season),
                        ),
                        (
                            tf!("Alle Folgen herunterladen ({n})", n = all.len()),
                            fetch(all),
                        ),
                    ]
                });
                row.append(&b);
            }
            // Viele Staffeln: waagerecht blättern statt die Seite zu verbreitern.
            right.append(
                &gtk::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk::PolicyType::Automatic)
                    .vscrollbar_policy(gtk::PolicyType::Never)
                    .child(&row)
                    .build(),
            );
        } else {
            fill(None);
        }
        right.append(&download);
        right.append(&rows);
        let weak = Rc::downgrade(self);
        let l = list.clone();
        rows.connect_row_activated(move |_, row| {
            let Some(v) = weak.upgrade() else { return };
            let Ok(i) = row.widget_name().parse::<usize>() else {
                return;
            };
            let (Some(app), Some(it)) = (v.app.upgrade(), l.get(i)) else {
                return;
            };
            let start = seen_fraction(it).map(|_| it.position_secs);
            play(&app, &v.source, it, start, l[i + 1..].to_vec());
        });
        let poster = self.big_poster(head);
        self.present_detail(&self.page(&poster, &right));
    }
}

/// Zeile einer Sammlung bzw. Serie; der Widget-Name trägt die Nummer in der Liste.
/// „Staffel 2 herunterladen (10)“, „Extras herunterladen (3)“; ohne Wahl alle Folgen.
fn jf_season_label(list: &[Item], season: Option<Option<i64>>) -> String {
    let n = list
        .iter()
        .filter(|it| season.is_none_or(|s| it.season_no == s))
        .count();
    match season {
        None => tf!("Alle Folgen herunterladen ({n})", n = n),
        Some(Some(0)) => tf!("Extras herunterladen ({n})", n = n),
        Some(Some(k)) => tf!("Staffel {k} herunterladen ({n})", k = k, n = n),
        Some(None) => tf!("Weitere herunterladen ({n})", n = n),
    }
}

/// Rechtsklick auf Film oder Folge: Herunterladen, Zwischenspeichern oder Abbrechen.
fn item_menu<W: IsA<gtk::Widget>>(app: &Weak<App>, library: &str, widget: &W, item: Item) {
    let (weak, library) = (app.clone(), library.to_string());
    crate::ablage::context_menu(widget, move || {
        let Some(app) = weak.upgrade() else {
            return Vec::new();
        };
        let (i1, i2, a2) = (item.clone(), item.clone(), app.clone());
        let (l1, l2) = (library.clone(), library.clone());
        crate::ablage::entries_for(
            &library,
            &item.id,
            move || crate::ablage::fetch_item(&app, &l1, &i1, tvjoe_core::ablage::Art::Download),
            move || crate::ablage::fetch_item(&a2, &l2, &i2, tvjoe_core::ablage::Art::Cache),
        )
    });
}

/// Rechtsklick auf eine Serie: alle Folgen oder eine Staffel herunterladen.
fn series_menu<W: IsA<gtk::Widget>>(app: &Weak<App>, library: &str, widget: &W, series_id: String) {
    let (weak, library) = (app.clone(), library.to_string());
    crate::ablage::context_menu(widget, move || {
        let mut out: Vec<(String, Box<dyn Fn()>)> = Vec::new();
        let Some(app) = weak.upgrade() else { return out };
        let episodes = db(&library).map(|d| d.episodes(&series_id)).unwrap_or_default();
        let fetch = |list: Vec<Item>| -> Box<dyn Fn()> {
            let (app, library) = (app.clone(), library.clone());
            Box::new(move || {
                for it in &list {
                    crate::ablage::fetch_item(&app, &library, it, tvjoe_core::ablage::Art::Download);
                }
            })
        };
        out.push((
            tf!("Alle Folgen herunterladen ({n})", n = episodes.len()),
            fetch(episodes.clone()),
        ));
        let mut seasons: Vec<i64> = episodes.iter().filter_map(|e| e.season_no).collect();
        seasons.sort_unstable();
        seasons.dedup();
        for season in seasons {
            let list: Vec<Item> = episodes
                .iter()
                .filter(|e| e.season_no == Some(season))
                .cloned()
                .collect();
            out.push((
                tf!(
                    "Staffel {k} herunterladen ({n})",
                    k = season,
                    n = list.len()
                ),
                fetch(list),
            ));
        }
        out
    });
}

fn list_row(index: usize, it: &Item, series: bool, badge: gtk::Button) -> gtk::ListBoxRow {
    let code = gtk::Label::builder()
        .label(if series {
            episode_code(it)
        } else {
            it.year.map(|y| y.to_string()).unwrap_or_default()
        })
        .css_classes(["tvjoe-num"])
        .width_chars(9)
        .xalign(0.0)
        .build();
    let title = gtk::Label::builder()
        .label(&it.name)
        .css_classes(["tvjoe-name"])
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let mut meta = vec![runtime(it.runtime_secs)];
    if it.played {
        meta.push(tr("gesehen").into());
    }
    let sub = gtk::Label::builder()
        .label(
            meta.into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
        )
        .css_classes(["tvjoe-sub"])
        .xalign(0.0)
        .build();
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.append(&title);
    text.append(&sub);
    if let Some(f) = seen_fraction(it) {
        text.append(
            &gtk::ProgressBar::builder()
                .fraction(f)
                .css_classes(["tvjoe-mt-seen"])
                .margin_top(4)
                .build(),
        );
    }
    let hbox = gtk::Box::builder()
        .spacing(16)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    hbox.append(&code);
    hbox.append(&text);
    badge.set_valign(gtk::Align::Center);
    badge.set_margin_bottom(0);
    hbox.append(&badge);
    hbox.append(&gtk::Image::from_icon_name(if it.played {
        "object-select-symbolic"
    } else {
        "media-playback-start-symbolic"
    }));
    gtk::ListBoxRow::builder()
        .child(&hbox)
        .activatable(true)
        .name(index.to_string())
        .build()
}

/// Kachel mit Plakat (bis es da ist: Titel), Titel und Zeile darunter; gesehen = Haken,
/// angefangen = Strich am unteren Rand.
fn tile(it: &Item, sub: &str, badge: Option<&gtk::Button>) -> (gtk::Box, gtk::Picture) {
    let fallback = gtk::Label::builder()
        .label(&it.name)
        .css_classes(["tvjoe-name"])
        .wrap(true)
        .justify(gtk::Justification::Center)
        .margin_start(10)
        .margin_end(10)
        .build();
    let poster = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .build();
    let frame = gtk::Overlay::builder()
        .child(&fallback)
        .css_classes(["tvjoe-mt-thumb"])
        .overflow(gtk::Overflow::Hidden)
        .width_request(POSTER_W)
        .height_request(POSTER_H)
        .build();
    frame.add_overlay(&poster);
    if let Some(b) = badge {
        frame.add_overlay(b);
    }
    if it.played {
        frame.add_overlay(
            &gtk::Image::builder()
                .icon_name("object-select-symbolic")
                .css_classes(["tvjoe-jf-played"])
                .halign(gtk::Align::End)
                .valign(gtk::Align::Start)
                .margin_top(6)
                .margin_end(6)
                .build(),
        );
    }
    if let Some(f) = seen_fraction(it) {
        frame.add_overlay(
            &gtk::ProgressBar::builder()
                .fraction(f)
                .valign(gtk::Align::End)
                .css_classes(["tvjoe-mt-seen"])
                .build(),
        );
    }
    let frame = adw::Clamp::builder()
        .orientation(gtk::Orientation::Vertical)
        .maximum_size(POSTER_H)
        .tightening_threshold(POSTER_H)
        .child(
            &adw::Clamp::builder()
                .orientation(gtk::Orientation::Horizontal)
                .maximum_size(POSTER_W)
                .tightening_threshold(POSTER_W)
                .child(&frame)
                .build(),
        )
        .build();
    let title = gtk::Label::builder()
        .label(&it.name)
        .css_classes(["tvjoe-name"])
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .lines(2)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(1)
        .build();
    let sub = gtk::Label::builder()
        .label(sub)
        .css_classes(["tvjoe-sub"])
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(1)
        .build();
    let tile = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .width_request(POSTER_W)
        .css_classes(["tvjoe-mt-tile"])
        .build();
    tile.append(&frame);
    tile.append(&title);
    tile.append(&sub);
    let desc: String = it.overview.chars().take(400).collect();
    tile.set_tooltip_text(Some(&format!("{}\n\n{desc}", it.name)));
    (tile, poster)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, played: bool, pos: f64, last: Option<&str>) -> Item {
        Item {
            id: id.into(),
            kind: "Episode".into(),
            name: id.into(),
            sort_name: id.into(),
            series_id: None,
            series_name: None,
            season_id: None,
            season_no: Some(1),
            episode_no: None,
            year: None,
            premiere: None,
            runtime_secs: 3000,
            overview: String::new(),
            genres: String::new(),
            image_tag: None,
            date_created: None,
            played,
            position_secs: pos,
            last_played: last.map(str::to_string),
        }
    }

    #[test]
    fn weitersehen_in_liste() {
        let none = [item("a", false, 0.0, None), item("b", false, 0.0, None)];
        assert_eq!(next_in(&none), Some((0, None)));
        let done = [
            item("a", true, 0.0, Some("2026-01-01")),
            item("b", false, 0.0, None),
        ];
        assert_eq!(next_in(&done), Some((1, None)));
        let half = [
            item("a", true, 0.0, Some("2026-01-01")),
            item("b", false, 600.0, Some("2026-01-02")),
            item("c", false, 0.0, None),
        ];
        assert_eq!(next_in(&half), Some((1, Some(600.0))));
        assert_eq!(runtime(5580), "1 h 33 min");
        assert_eq!(runtime(2700), "45 min");
    }
}
