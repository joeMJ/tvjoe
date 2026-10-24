//! Mediathek (abgestimmte Skizze v2): oben „Episoden“, „Filme“, „Suche“, darunter die Senderleiste
//! mit Schwarzweiß-Logos über die ganze Breite, darunter Kacheln mit Vorschaubild, Titel, Thema,
//! Datum · Uhrzeit und Dauer. Ein Klick spielt den Beitrag im Hauptfenster ab.
//!
//! Die Filmliste wird im Hintergrund aktualisiert (Einstellung „Filmliste neu laden nach“), sobald
//! die Mediathek einmal geöffnet wurde; Vorschaubilder kommen aus `og:image` der Sendungsseite.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use adw::prelude::*;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::{gio, glib};
use tvjoe_core::mediathek::{self, Film, Filter, Group, Kind, Mediathek, Version};
use tvjoe_core::{epg, net, normalize_id, paths};

use crate::app::{App, hm};
use crate::trace;

/// Höchstzahl Kacheln je Ansicht.
const LIMIT: usize = 200;
const THUMB_W: i32 = 256;
const THUMB_H: i32 = 144;
/// Vorschaubilder werden in dieser Größe abgelegt.
const THUMB_SAVE_W: i32 = 320;
const THUMB_SAVE_H: i32 = 180;
const THUMB_THREADS: usize = 4;
/// Abgefragte Einträge je Ansicht (mehrere Fassungen ergeben eine Kachel).
const FETCH: usize = 600;
/// Bild der Filminfo.
const DETAIL_W: i32 = 640;
const DETAIL_H: i32 = 360;
const PAGE_BYTES: u64 = 512 * 1024;
const IMAGE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Episodes,
    Films,
    Search,
}

thread_local! {
    static WINDOW: RefCell<Option<Rc<MtWindow>>> = const { RefCell::new(None) };
    static UPDATING: Cell<bool> = const { Cell::new(false) };
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
    static TIMER: Cell<bool> = const { Cell::new(false) };
}

fn filter_of(app: &App) -> Filter {
    let s = app.settings();
    Filter {
        hide_ad: s.mediathek_hide_ad,
        hidden: s.mediathek_hidden.clone(),
    }
}

// ---------------------------------------------------------------------------------------------
// Hintergrund-Aktualisierung der Filmliste

/// Nach dem Start: prüft regelmäßig, ob die Filmliste fällig ist. Nur, wenn die Mediathek schon
/// einmal geöffnet wurde (Datenbank vorhanden); sonst lädt tvjoe nichts.
pub fn start_background(app: &Rc<App>) {
    if TIMER.with(|t| t.replace(true)) {
        return;
    }
    maybe_update(app);
    let weak = Rc::downgrade(app);
    glib::timeout_add_seconds_local(600, move || match weak.upgrade() {
        Some(app) => {
            maybe_update(&app);
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
}

fn maybe_update(app: &Rc<App>) {
    let Some(db) = paths::mediathek_db().filter(|p| p.exists()) else {
        return;
    };
    let hours = app.settings().mediathek_refresh_hours.max(1);
    let due = Mediathek::open(&db)
        .map(|mt| {
            mt.last_update()
                .is_none_or(|t| epg::now() - t >= i64::from(hours) * 3600)
        })
        .unwrap_or(false);
    if due {
        run_update(app);
    }
}

/// Lädt die Filmliste im Hintergrund (volle Liste oder Änderungsliste) und baut danach ein
/// offenes Fenster neu auf.
fn run_update(app: &Rc<App>) {
    if UPDATING.with(|u| u.replace(true)) {
        return;
    }
    let (Some(db), Some(cache)) = (paths::mediathek_db(), paths::cache_dir()) else {
        UPDATING.with(|u| u.set(false));
        return;
    };
    let days = app.settings().mediathek_days.max(1);
    trace("Mediathek: Filmliste wird geladen");
    if let Some(w) = current() {
        w.show_status();
    }
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(mediathek::update_from_net(&db, &cache, days, epg::now()));
    });
    glib::spawn_future_local(async move {
        let Ok(result) = rx.recv().await else { return };
        UPDATING.with(|u| u.set(false));
        match result {
            Ok(st) => {
                trace(&format!(
                    "Mediathek: {} gelesen, {} neu, {} entfernt, {} im Bestand",
                    st.seen, st.added, st.purged, st.total
                ));
                LAST_ERROR.with(|e| e.borrow_mut().take());
            }
            Err(e) => {
                eprintln!("tvjoe: Filmliste: {e}");
                LAST_ERROR.with(|l| *l.borrow_mut() = Some(e));
            }
        }
        if let Some(w) = current() {
            w.rebuild();
        }
    });
}

thread_local! {
    /// Eigene Verbindung für Spielpositionen (Hauptfenster, auch ohne offene Mediathek).
    static PROGRESS_DB: RefCell<Option<Mediathek>> = const { RefCell::new(None) };
}

fn with_progress_db<T>(f: impl FnOnce(&Mediathek) -> Option<T>) -> Option<T> {
    PROGRESS_DB.with(|db| {
        let mut db = db.borrow_mut();
        if db.is_none() {
            *db = paths::mediathek_db()
                .filter(|p| p.exists())
                .and_then(|p| Mediathek::open(&p).ok());
        }
        db.as_ref().and_then(f)
    })
}

/// Spielposition eines Beitrags merken (Regeln im Kern: ab 1 Minute, ab 95 % gesehen).
pub fn save_progress(film: &Film, position: f64, duration: f64) {
    with_progress_db(|db| {
        db.save_progress(&film.url, position, duration, epg::now());
        Some(())
    });
}

/// Gemerkte Position und Dauer.
pub fn progress_of(film: &Film) -> Option<(f64, f64)> {
    with_progress_db(|db| db.progress(&film.url))
}

fn current() -> Option<Rc<MtWindow>> {
    WINDOW.with(|w| w.borrow().clone())
}

// ---------------------------------------------------------------------------------------------
// Vorschaubilder

struct Thumbs {
    requests: mpsc::Sender<(u64, i64, String)>,
    generation: Arc<AtomicU64>,
}

impl Thumbs {
    /// Startet die Hintergrund-Threads; `results` liefert (Beitrags-ID, Pfad des Bildes).
    fn spawn(dir: PathBuf) -> (Self, async_channel::Receiver<(i64, PathBuf)>) {
        let (req_tx, req_rx) = mpsc::channel::<(u64, i64, String)>();
        let req_rx = Arc::new(Mutex::new(req_rx));
        let (res_tx, res_rx) = async_channel::unbounded();
        let generation = Arc::new(AtomicU64::new(0));
        for _ in 0..THUMB_THREADS {
            let (req_rx, res_tx, generation, dir) = (
                req_rx.clone(),
                res_tx.clone(),
                generation.clone(),
                dir.clone(),
            );
            std::thread::spawn(move || {
                let agent = net::agent(Duration::from_secs(10));
                loop {
                    let next = req_rx.lock().ok().and_then(|r| r.recv().ok());
                    let Some((gen_, id, website)) = next else {
                        break;
                    };
                    let path = dir.join(net::cache_name(&website, "jpg"));
                    let none = path.with_extension("none");
                    if !path.exists() {
                        // Nach einem Ansichtswechsel nur noch Bilder aus dem Cache.
                        if none.exists() || gen_ != generation.load(Ordering::Relaxed) {
                            continue;
                        }
                        if let Err((e, permanent)) = load_thumb(&agent, &website, &path) {
                            trace(&format!("Vorschaubild {website}: {e}"));
                            // Netzfehler beim nächsten Anzeigen erneut versuchen, „kein Bild“ nicht.
                            if permanent {
                                let _ = std::fs::write(&none, b"");
                            }
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

    /// Neue Ansicht: ältere, noch nicht geladene Anfragen verfallen.
    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn request(&self, generation: u64, id: i64, website: &str) {
        let _ = self.requests.send((generation, id, website.to_string()));
    }
}

/// Fehler mit `true`, wenn ein neuer Versuch nichts ändert (Seite ohne Bild, Bild unlesbar).
fn load_thumb(agent: &net::Agent, website: &str, path: &Path) -> Result<(), (String, bool)> {
    let page = net::fetch_prefix(agent, website, PAGE_BYTES).map_err(|e| (e, false))?;
    let url = mediathek::og_image(&String::from_utf8_lossy(&page))
        .ok_or(("kein og:image".to_string(), true))?;
    let bytes = net::fetch(agent, &url, IMAGE_BYTES).map_err(|e| (e, false))?;
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(bytes));
    let pixbuf = Pixbuf::from_stream_at_scale(
        &stream,
        THUMB_SAVE_W,
        THUMB_SAVE_H,
        true,
        gio::Cancellable::NONE,
    )
    .map_err(|e| (e.to_string(), true))?;
    let io = |e: String| (e, false);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io(e.to_string()))?;
    }
    let tmp = path.with_extension("tmp");
    pixbuf
        .savev(&tmp, "jpeg", &[("quality", "80")])
        .map_err(|e| io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| io(e.to_string()))
}

// ---------------------------------------------------------------------------------------------
// Senderlogos

/// Kurzname der Mediathek → Anfang des Sendernamens in der Senderliste (normalisiert).
const ALIASES: &[(&str, &str)] = &[("ard", "daserste"), ("arte.de", "arte")];

/// Logo eines Mediathek-Senders aus der Senderliste (schon geladene Logodatei), sonst `None`.
pub fn sender_logo(app: &App, sender: &str) -> Option<PathBuf> {
    let key = ALIASES
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(sender))
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| normalize_id(sender));
    let dir = paths::logo_cache_dir()?;
    let lib = app.library();
    lib.channels.iter().find_map(|ch| {
        let name = ch.display_name();
        let full = normalize_id(name.trim_end_matches(" HD").trim_end_matches(" hd"));
        let first = normalize_id(name.split_whitespace().next().unwrap_or(""));
        if full != key && first != key {
            return None;
        }
        let path = dir.join(net::cache_name(ch.logo.as_deref()?, "png"));
        path.exists().then_some(path)
    })
}

// ---------------------------------------------------------------------------------------------
// Fenster

struct MtWindow {
    app: std::rc::Weak<App>,
    win: adw::Window,
    tabs: [gtk::ToggleButton; 3],
    search: gtk::SearchEntry,
    search_bar: gtk::Box,
    senders: gtk::Box,
    stand: gtk::Label,
    stack: gtk::Stack,
    empty: adw::StatusPage,
    flow: gtk::FlowBox,
    back: gtk::Button,
    mode: Cell<Mode>,
    sender: RefCell<Option<String>>,
    /// Sendungen der Ansicht (Fassungen zusammengefasst).
    films: RefCell<Vec<Group>>,
    thumbs_of: RefCell<HashMap<i64, gtk::Picture>>,
    thumbs: Option<Thumbs>,
    db: RefCell<Option<Mediathek>>,
}

pub fn open(app: &Rc<App>) {
    if let Some(w) = current() {
        w.win.present();
        return;
    }
    let w = MtWindow::new(app);
    WINDOW.with(|slot| *slot.borrow_mut() = Some(w.clone()));
    w.rebuild();
    w.win.present();
    start_background(app);
    if w.db.borrow().as_ref().is_none_or(|db| db.count() == 0) {
        run_update(app);
    }
}

/// Selbstprobe: Fenster öffnen und zurückgeben.
pub fn open_window(app: &Rc<App>) -> Option<gtk::Window> {
    open(app);
    current().map(|w| w.win.clone().upcast())
}

/// Selbstprobe: Reiter wählen (0 Episoden, 1 Filme, 2 Suche), optional Suchtext.
pub fn probe_tab(index: usize, search: Option<&str>) -> Option<gtk::Window> {
    let w = current()?;
    w.tabs.get(index)?.set_active(true);
    if let Some(text) = search {
        w.search.set_text(text);
        w.reload();
    }
    Some(w.win.clone().upcast())
}

/// Selbstprobe: erste Kachel abspielen.
pub fn probe_play_first(app: &Rc<App>) {
    let film = current().and_then(|w| w.films.borrow().first().map(|g| g.main().clone()));
    if let Some(film) = film {
        app.play_film(&film, None);
        app.window.present();
    }
}

pub fn day_time(ts: i64) -> String {
    glib::DateTime::from_unix_local(ts)
        .ok()
        .and_then(|d| d.format("%a %d.%m. · %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn minutes(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    format!("{} Min.", (secs + 30) / 60)
}

impl MtWindow {
    fn new(app: &Rc<App>) -> Rc<Self> {
        let tab = |label: &str| gtk::ToggleButton::builder().label(label).build();
        let tabs = [tab("Episoden"), tab("Filme"), tab("Suche")];
        tabs[1].set_group(Some(&tabs[0]));
        tabs[2].set_group(Some(&tabs[0]));
        tabs[0].set_active(true);
        let tab_box = gtk::Box::builder().css_classes(["linked"]).build();
        for t in &tabs {
            tab_box.append(t);
        }
        let stand = gtk::Label::builder().css_classes(["tvjoe-sub"]).build();
        let header = adw::HeaderBar::builder().title_widget(&tab_box).build();
        let back = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Zurück zur Übersicht (Esc)")
            .visible(false)
            .build();
        header.pack_start(&back);
        header.pack_start(&stand);
        let prefs_button = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text("Einstellungen der Mediathek")
            .build();
        header.pack_end(&prefs_button);

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Titel, Thema oder Beschreibung")
            .hexpand(true)
            .build();
        let search_bar = gtk::Box::builder()
            .margin_start(16)
            .margin_end(16)
            .margin_top(8)
            .visible(false)
            .build();
        search_bar.append(&search);

        let senders = gtk::Box::builder()
            .spacing(6)
            .margin_start(16)
            .margin_end(16)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        let sender_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&senders)
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
            .icon_name("folder-videos-symbolic")
            .vexpand(true)
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&grid_scroller, Some("grid"));
        stack.add_named(&empty, Some("empty"));

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&search_bar);
        body.append(&sender_scroller);
        body.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        body.append(&stack);

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&body));
        let win = adw::Window::builder()
            .title("Mediathek – tvjoe")
            .default_width(1400)
            .default_height(860)
            .hide_on_close(true)
            .content(&view)
            .build();

        let thumbs = paths::thumb_cache_dir().map(Thumbs::spawn);
        let (thumbs, results) = match thumbs {
            Some((t, r)) => (Some(t), Some(r)),
            None => (None, None),
        };
        let db = paths::mediathek_db().and_then(|p| {
            Mediathek::open(&p)
                .map_err(|e| eprintln!("tvjoe: {e}"))
                .ok()
        });

        let w = Rc::new(Self {
            app: Rc::downgrade(app),
            win,
            tabs,
            search,
            search_bar,
            senders,
            stand,
            stack,
            empty,
            flow,
            back,
            mode: Cell::new(Mode::Episodes),
            sender: RefCell::new(None),
            films: RefCell::new(Vec::new()),
            thumbs_of: RefCell::new(HashMap::new()),
            thumbs,
            db: RefCell::new(db),
        });

        if let Some(results) = results {
            let weak = Rc::downgrade(&w);
            glib::spawn_future_local(async move {
                while let Ok((id, path)) = results.recv().await {
                    let Some(w) = weak.upgrade() else { break };
                    if let Some(p) = w.thumbs_of.borrow().get(&id) {
                        p.set_filename(Some(&path));
                    }
                }
            });
        }

        for (i, mode) in [Mode::Episodes, Mode::Films, Mode::Search]
            .into_iter()
            .enumerate()
        {
            let weak = Rc::downgrade(&w);
            w.tabs[i].connect_toggled(move |b| {
                let Some(w) = weak.upgrade() else { return };
                if b.is_active() {
                    w.mode.set(mode);
                    w.search_bar.set_visible(mode == Mode::Search);
                    if mode == Mode::Search {
                        w.search.grab_focus();
                    }
                    w.reload();
                }
            });
        }
        let weak = Rc::downgrade(&w);
        prefs_button.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.open_prefs();
            }
        });
        let weak = Rc::downgrade(&w);
        w.search.connect_search_changed(move |_| {
            if let Some(w) = weak.upgrade() {
                w.reload();
            }
        });
        let weak = Rc::downgrade(&w);
        w.flow.connect_child_activated(move |_, child| {
            let Some(w) = weak.upgrade() else { return };
            let group = usize::try_from(child.index())
                .ok()
                .and_then(|i| w.films.borrow().get(i).cloned());
            if let Some(group) = group {
                w.show_detail(&group);
            }
        });
        let weak = Rc::downgrade(&w);
        w.back.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.close_detail();
            }
        });
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&w);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(w) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if key == gtk::gdk::Key::Escape && w.back.is_visible() {
                w.close_detail();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        w.win.add_controller(keys);
        w
    }

    /// Ganzflächige Filminfo (Plex/Jellyfin-Stil): großes Bild, Titel, Sender · Thema,
    /// Datum · Dauer, Fassungen, „Abspielen“, Beschreibung. Esc bzw. Pfeil führt zurück.
    fn show_detail(self: &Rc<Self>, group: &Group) {
        if let Some(old) = self.stack.child_by_name("detail") {
            self.stack.remove(&old);
        }
        let main = group.main();
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_shrink(true)
            .build();
        let thumb = self
            .thumbs_of
            .borrow()
            .get(&main.id)
            .and_then(|p| p.paintable());
        let image: gtk::Widget = match thumb {
            Some(t) => {
                picture.set_paintable(Some(&t));
                picture.upcast()
            }
            None => match self
                .app
                .upgrade()
                .and_then(|a| sender_logo(&a, &main.sender))
            {
                Some(path) => {
                    let pic = crate::logos::bright_picture(&path);
                    pic.set_margin_start(160);
                    pic.set_margin_end(160);
                    pic.set_margin_top(110);
                    pic.set_margin_bottom(110);
                    pic.upcast()
                }
                None => gtk::Label::builder()
                    .label(&main.sender)
                    .css_classes(["tvjoe-osd-name"])
                    .build()
                    .upcast(),
            },
        };
        let frame = gtk::Overlay::builder()
            .child(&image)
            .css_classes(["tvjoe-mt-thumb"])
            .overflow(gtk::Overflow::Hidden)
            .width_request(DETAIL_W)
            .height_request(DETAIL_H)
            .valign(gtk::Align::Start)
            .build();
        let frame = adw::Clamp::builder()
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
            .build();

        let text = |t: &str, class: &str| {
            gtk::Label::builder()
                .label(t)
                .css_classes([class])
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .build()
        };
        let right = gtk::Box::new(gtk::Orientation::Vertical, 10);
        right.set_hexpand(true);
        right.append(&text(&main.base_title(), "tvjoe-mt-detail-title"));
        right.append(&text(
            &format!("{} · {}", main.sender, main.topic),
            "tvjoe-osd-sub",
        ));
        let meta: Vec<String> = [day_time(main.timestamp), minutes(main.duration)]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        right.append(&text(&meta.join(" · "), "tvjoe-osd-sub"));

        // Fassungen als Knöpfe; „Weitersehen ab …“ bzw. „Abspielen“ starten die gewählte.
        let chosen = Rc::new(Cell::new(0usize));
        // Hervorhebung setzt `update`.
        let button = |icon: &str, label: &str| {
            gtk::Button::builder()
                .css_classes(["pill"])
                .child(
                    &adw::ButtonContent::builder()
                        .icon_name(icon)
                        .label(label)
                        .build(),
                )
                .build()
        };
        let resume = button("media-playback-start-symbolic", "Weitersehen");
        let play = button("media-playback-start-symbolic", "Abspielen");
        let films = Rc::new(group.films.clone());
        // Knöpfe zur gewählten Fassung: mit gemerkter Position „Weitersehen ab 23:10“ und
        // „Von vorn“, sonst nur „Abspielen“.
        let update = {
            let (films, resume, play) = (films.clone(), resume.clone(), play.clone());
            move |i: usize| {
                let pos = films.get(i).and_then(progress_of);
                let set = |b: &gtk::Button, text: &str, suggested: bool| {
                    if let Some(c) = b.child().and_downcast::<adw::ButtonContent>() {
                        c.set_label(text);
                    }
                    if suggested {
                        b.add_css_class("suggested-action");
                    } else {
                        b.remove_css_class("suggested-action");
                    }
                };
                match pos {
                    Some((p, _)) => {
                        set(
                            &resume,
                            &format!("Weitersehen ab {}", crate::osd::clock(p)),
                            true,
                        );
                        resume.set_visible(true);
                        set(&play, "Von vorn", false);
                    }
                    None => {
                        resume.set_visible(false);
                        set(&play, "Abspielen", true);
                    }
                }
            }
        };
        update(0);
        let update = Rc::new(update);
        if group.films.len() > 1 {
            let row = gtk::Box::builder()
                .css_classes(["linked"])
                .margin_top(6)
                .build();
            let mut first: Option<gtk::ToggleButton> = None;
            for (i, f) in group.films.iter().enumerate() {
                let b = gtk::ToggleButton::builder()
                    .label(f.version().label())
                    .active(i == 0)
                    .build();
                if let Some(g) = &first {
                    b.set_group(Some(g));
                } else {
                    first = Some(b.clone());
                }
                let (chosen, update) = (chosen.clone(), update.clone());
                b.connect_toggled(move |b| {
                    if b.is_active() {
                        chosen.set(i);
                        update(i);
                    }
                });
                row.append(&b);
            }
            row.set_halign(gtk::Align::Start);
            right.append(&row);
        }
        let buttons = gtk::Box::builder()
            .spacing(10)
            .margin_top(6)
            .halign(gtk::Align::Start)
            .build();
        buttons.append(&resume);
        buttons.append(&play);
        right.append(&buttons);
        for (b, from_start) in [(&resume, false), (&play, true)] {
            let (films, chosen) = (films.clone(), chosen.clone());
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                let Some(w) = weak.upgrade() else { return };
                let (Some(app), Some(film)) = (w.app.upgrade(), films.get(chosen.get())) else {
                    return;
                };
                let start = if from_start {
                    None
                } else {
                    progress_of(film).map(|(p, _)| p)
                };
                app.play_film(film, start);
                app.window.present();
            });
        }
        let desc = text(main.description.trim(), "tvjoe-mt-detail-desc");
        desc.set_margin_top(10);
        desc.set_valign(gtk::Align::Start);
        right.append(&desc);

        let page = gtk::Box::builder()
            .spacing(32)
            .margin_start(32)
            .margin_end(32)
            .margin_top(32)
            .margin_bottom(32)
            .build();
        page.append(&frame);
        page.append(&right);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&page)
            .build();
        self.stack.add_named(&scroller, Some("detail"));
        self.stack.set_visible_child_name("detail");
        self.back.set_visible(true);
        if resume.is_visible() {
            resume.grab_focus();
        } else {
            play.grab_focus();
        }
    }

    /// Zurück zur Übersicht (an dieselbe Stelle).
    fn close_detail(&self) {
        self.back.set_visible(false);
        self.stack.set_visible_child_name("grid");
        if let Some(old) = self.stack.child_by_name("detail") {
            self.stack.remove(&old);
        }
    }

    /// Einstellungen der Mediathek (Zahnrad): Filmliste, Audiodeskription, Sender einzeln.
    fn open_prefs(self: &Rc<Self>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let s = app.settings().clone();
        let dialog = adw::PreferencesDialog::new();
        dialog.set_title("Mediathek");
        let page = adw::PreferencesPage::new();

        let list = adw::PreferencesGroup::builder()
            .title("Filmliste")
            .description("Von MediathekView, im Hintergrund geladen, sobald die Mediathek einmal geöffnet wurde")
            .build();
        let hours = adw::SpinRow::with_range(1.0, 168.0, 1.0);
        hours.set_title("Neu laden nach");
        hours.set_subtitle("Stunden");
        hours.set_value(f64::from(s.mediathek_refresh_hours.max(1)));
        let days = adw::SpinRow::with_range(1.0, 90.0, 1.0);
        days.set_title("Beiträge behalten");
        days.set_subtitle("Tage; gilt ab der nächsten Aktualisierung");
        days.set_value(f64::from(s.mediathek_days.max(1)));
        list.add(&hours);
        list.add(&days);

        let view = adw::PreferencesGroup::builder().title("Anzeige").build();
        let hide_ad = adw::SwitchRow::builder()
            .title("Fassungen mit Audiodeskription ausblenden")
            .active(s.mediathek_hide_ad)
            .build();
        view.add(&hide_ad);

        let senders = adw::PreferencesGroup::builder()
            .title("Sender")
            .description("Ausgeschaltete Sender erscheinen weder in der Senderleiste noch in Listen und Suche")
            .build();
        let mut names: Vec<String> = self
            .db
            .borrow()
            .as_ref()
            .map(|db| db.senders().into_iter().map(|(n, _)| n).collect())
            .unwrap_or_default();
        // Ausgeblendete, die gerade nicht in der Filmliste stehen, trotzdem anbieten.
        for h in &s.mediathek_hidden {
            if !names.contains(h) {
                names.push(h.clone());
            }
        }
        names.sort_by_key(|n| n.to_lowercase());
        let rows: Vec<(String, adw::SwitchRow)> = names
            .into_iter()
            .map(|n| {
                let row = adw::SwitchRow::builder()
                    .title(&n)
                    .active(!s.mediathek_hidden.contains(&n))
                    .build();
                senders.add(&row);
                (n, row)
            })
            .collect();

        page.add(&list);
        page.add(&view);
        page.add(&senders);
        dialog.add(&page);

        let weak = Rc::downgrade(self);
        dialog.connect_closed(move |_| {
            let Some(w) = weak.upgrade() else { return };
            let Some(app) = w.app.upgrade() else { return };
            {
                let mut s = app.settings_mut();
                s.mediathek_refresh_hours = hours.value() as u32;
                s.mediathek_days = days.value() as u32;
                s.mediathek_hide_ad = hide_ad.is_active();
                s.mediathek_hidden = rows
                    .iter()
                    .filter(|(_, r)| !r.is_active())
                    .map(|(n, _)| n.clone())
                    .collect();
            }
            app.save_settings();
            w.rebuild();
        });
        dialog.present(Some(&self.win));
    }

    /// Senderleiste, Stand und Kacheln neu aufbauen (nach Aktualisierung oder Einstellungen).
    fn rebuild(self: &Rc<Self>) {
        self.build_senders();
        self.reload();
    }

    fn build_senders(self: &Rc<Self>) {
        while let Some(c) = self.senders.first_child() {
            self.senders.remove(&c);
        }
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let list = self
            .db
            .borrow()
            .as_ref()
            .map(|db| db.senders_filtered(&filter_of(&app)))
            .unwrap_or_default();
        // Gewählter Sender nicht mehr vorhanden (z. B. ausgeblendet) → „Alle“.
        if self
            .sender
            .borrow()
            .as_ref()
            .is_some_and(|s| !list.iter().any(|(n, _)| n == s))
        {
            self.sender.replace(None);
        }
        let all = gtk::ToggleButton::builder()
            .label("Alle")
            .css_classes(["tvjoe-mt-sender"])
            .active(self.sender.borrow().is_none())
            .build();
        self.senders.append(&all);
        let weak = Rc::downgrade(self);
        all.connect_toggled(move |b| {
            if let (true, Some(w)) = (b.is_active(), weak.upgrade()) {
                w.sender.replace(None);
                w.reload();
            }
        });
        for (name, count) in list {
            let button = gtk::ToggleButton::builder()
                .css_classes(["tvjoe-mt-sender"])
                .tooltip_text(format!("{name} ({count})"))
                .group(&all)
                .active(self.sender.borrow().as_deref() == Some(name.as_str()))
                .build();
            match sender_logo(&app, &name) {
                Some(path) => {
                    let pic = crate::logos::bright_picture(&path);
                    pic.set_height_request(28);
                    button.set_child(Some(
                        &adw::Clamp::builder()
                            .orientation(gtk::Orientation::Vertical)
                            .maximum_size(28)
                            .tightening_threshold(28)
                            .child(&crate::logo_box(&pic, 56))
                            .build(),
                    ));
                }
                None => button.set_label(&name),
            }
            let weak = Rc::downgrade(self);
            button.connect_toggled(move |b| {
                if let (true, Some(w)) = (b.is_active(), weak.upgrade()) {
                    w.sender.replace(Some(name.clone()));
                    w.reload();
                }
            });
            self.senders.append(&button);
        }
    }

    /// Stand der Filmliste oben links.
    fn show_status(&self) {
        let updating = UPDATING.with(Cell::get);
        let db = self.db.borrow();
        let last = db.as_ref().and_then(Mediathek::last_update);
        let mut text = match last {
            Some(t) => format!(
                "Stand {} {} · {} Beiträge",
                glib::DateTime::from_unix_local(t)
                    .ok()
                    .and_then(|d| d.format("%d.%m.").ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                hm(t),
                db.as_ref().map_or(0, Mediathek::count)
            ),
            None => String::new(),
        };
        if updating {
            if !text.is_empty() {
                text.push_str(" · ");
            }
            text.push_str("Filmliste wird aktualisiert …");
        }
        self.stand.set_label(&text);
    }

    fn show_empty(&self, title: &str, text: &str) {
        self.empty.set_title(title);
        self.empty.set_description(Some(text));
        self.stack.set_visible_child_name("empty");
    }

    /// Kacheln der aktuellen Ansicht neu laden.
    fn reload(self: &Rc<Self>) {
        self.show_status();
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let filter = filter_of(&app);
        let sender = self.sender.borrow().clone();
        let films = {
            let db = self.db.borrow();
            let Some(db) = db.as_ref() else {
                self.show_empty(
                    "Keine Mediathek",
                    "Die Datenbank ließ sich nicht öffnen (Cacheordner).",
                );
                return;
            };
            if db.count() == 0 {
                let (title, text) = match LAST_ERROR.with(|e| e.borrow().clone()) {
                    Some(e) if !UPDATING.with(Cell::get) => {
                        ("Filmliste nicht geladen", format!("Fehler: {e}"))
                    }
                    _ => (
                        "Filmliste wird geladen",
                        "Beim ersten Mal wird die vollständige Filmliste von MediathekView geladen (rund 80 MB). Danach genügt meist die kleine Änderungsliste.".to_string(),
                    ),
                };
                self.show_empty(title, &text);
                return;
            }
            match self.mode.get() {
                Mode::Episodes => db.latest_filtered(
                    Some(Kind::Episode),
                    sender.as_deref(),
                    &filter,
                    epg::now(),
                    FETCH,
                ),
                Mode::Films => db.latest_filtered(
                    Some(Kind::Film),
                    sender.as_deref(),
                    &filter,
                    epg::now(),
                    FETCH,
                ),
                Mode::Search => {
                    let text = self.search.text();
                    if text.trim().chars().count() < 2 {
                        self.films.borrow_mut().clear();
                        self.show_empty("Suche", "Mindestens zwei Zeichen eingeben.");
                        return;
                    }
                    db.search_filtered(text.trim(), sender.as_deref(), &filter, FETCH)
                }
            }
        };
        let mut films = mediathek::group(films);
        films.truncate(LIMIT);
        self.back.set_visible(false);
        while let Some(c) = self.flow.first_child() {
            self.flow.remove(&c);
        }
        self.thumbs_of.borrow_mut().clear();
        if films.is_empty() {
            self.films.borrow_mut().clear();
            self.show_empty(
                "Keine Beiträge",
                "Für diese Auswahl gibt es keine Beiträge.",
            );
            return;
        }
        let generation = self.thumbs.as_ref().map_or(0, Thumbs::next_generation);
        let mut logos: HashMap<String, Option<PathBuf>> = HashMap::new();
        for group in &films {
            let film = group.main();
            let logo = logos
                .entry(film.sender.clone())
                .or_insert_with(|| sender_logo(&app, &film.sender))
                .clone();
            let seen = group
                .films
                .iter()
                .find_map(progress_of)
                .filter(|(_, d)| *d > 0.0)
                .map(|(p, d)| p / d);
            let (tile, thumb) = tile(group, logo.as_deref(), seen);
            self.flow.append(&tile);
            if let (Some(t), Some(site)) = (&self.thumbs, film.website.as_deref()) {
                self.thumbs_of.borrow_mut().insert(film.id, thumb);
                t.request(generation, film.id, site);
            }
        }
        self.films.replace(films);
        self.stack.set_visible_child_name("grid");
        if let Some(adj) = self
            .stack
            .child_by_name("grid")
            .and_downcast::<gtk::ScrolledWindow>()
            .map(|s| s.vadjustment())
        {
            adj.set_value(0.0);
        }
    }
}

/// Eine Kachel: Vorschaubild (bis es da ist: Senderlogo oder Sendername), Titel, Thema,
/// Datum · Uhrzeit · Dauer. Gibt die Kachel und das Bild für das spätere Vorschaubild zurück.
fn tile(group: &Group, logo: Option<&Path>, seen: Option<f64>) -> (gtk::Box, gtk::Picture) {
    let film = group.main();
    let fallback: gtk::Widget = match logo {
        Some(path) => {
            let pic = crate::logos::bright_picture(path);
            pic.set_margin_start(70);
            pic.set_margin_end(70);
            pic.set_margin_top(40);
            pic.set_margin_bottom(40);
            pic.upcast()
        }
        None => gtk::Label::builder()
            .label(&film.sender)
            .css_classes(["tvjoe-osd-name"])
            .build()
            .upcast(),
    };
    let thumb = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .build();
    let frame = gtk::Overlay::builder()
        .child(&fallback)
        .css_classes(["tvjoe-mt-thumb"])
        .overflow(gtk::Overflow::Hidden)
        .width_request(THUMB_W)
        .height_request(THUMB_H)
        .build();
    frame.add_overlay(&thumb);
    // Gemerkte Position als Strich am unteren Bildrand.
    if let Some(f) = seen {
        frame.add_overlay(
            &gtk::ProgressBar::builder()
                .fraction(f.clamp(0.0, 1.0))
                .valign(gtk::Align::End)
                .css_classes(["tvjoe-mt-seen"])
                .build(),
        );
    }
    let frame = adw::Clamp::builder()
        .orientation(gtk::Orientation::Vertical)
        .maximum_size(THUMB_H)
        .tightening_threshold(THUMB_H)
        .child(
            &adw::Clamp::builder()
                .orientation(gtk::Orientation::Horizontal)
                .maximum_size(THUMB_W)
                .tightening_threshold(THUMB_W)
                .child(&frame)
                .build(),
        )
        .build();

    let title = gtk::Label::builder()
        .label(film.base_title())
        .css_classes(["tvjoe-name"])
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .lines(2)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(1)
        .build();
    let sub = |text: &str| {
        gtk::Label::builder()
            .label(text)
            .css_classes(["tvjoe-sub"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(1)
            .build()
    };
    let topic = sub(&format!("{} · {}", film.sender, film.topic));
    let meta: Vec<String> = [day_time(film.timestamp), minutes(film.duration)]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    let meta = sub(&meta.join(" · "));

    let tile = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .width_request(THUMB_W)
        .css_classes(["tvjoe-mt-tile"])
        .build();
    tile.append(&frame);
    tile.append(&title);
    tile.append(&topic);
    tile.append(&meta);
    // Fassungen als Kennzeichen („DE · UT · OmU“), eine einzelne nur, wenn sie nicht Deutsch ist.
    let badges: Vec<&str> = group.films.iter().map(|f| f.version().badge()).collect();
    if group.films.len() > 1 || film.version() != Version::Normal {
        tile.append(
            &gtk::Label::builder()
                .label(badges.join(" · "))
                .css_classes(["tvjoe-mt-badge"])
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(1)
                .build(),
        );
    }
    let desc: String = film.description.chars().take(400).collect();
    tile.set_tooltip_text(Some(&format!("{}\n\n{desc}", film.display_title())));
    (tile, thumb)
}
