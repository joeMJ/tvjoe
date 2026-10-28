//! Ablage in der Oberfläche: Warteschlange der Downloads (höchstens zwei gleichzeitig),
//! Zwischenspeicher mit Grenze, Aufnahmen (sofort und nach Plan aus dem Programm), Kennzeichen
//! auf den Kacheln. Das Verzeichnis liegt in `ablage.sqlite` im Konfigurationsordner.
//!
//! Geladen und aufgenommen wird in Hintergrund-Threads; Ergebnisse kommen über einen Kanal in
//! den Hauptthread, der allein das Verzeichnis schreibt.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use adw::prelude::*;
use gtk::{gio, glib};
use tvjoe_core::ablage::{self, Art, Index, Progress, State, Timer};
use tvjoe_core::jellyfin_lib::Item;
use tvjoe_core::mediathek::Film;
use tvjoe_core::paths;
use tvjoe_core::settings::{SRC_SAT, SRC_TV};
use tvjoe_core::{tf, tr, trn};

use crate::app::App;
use crate::capture;
use crate::trace;

/// Gleichzeitige Downloads.
const PARALLEL: usize = 2;

thread_local! {
    static ABLAGE: RefCell<Option<Rc<Ablage>>> = const { RefCell::new(None) };
}

pub fn get() -> Option<Rc<Ablage>> {
    ABLAGE.with(|a| a.borrow().clone())
}

/// Was geladen wird.
#[derive(Clone)]
enum Fetch {
    Http { url: String, header: Option<String> },
    Hls { url: String, header: Option<String> },
    /// Jellyfin: Quelle erst im Ladethread beim Server erfragen.
    Jelly {
        account: tvjoe_core::jellyfin::Account,
        item_id: String,
        original: bool,
        kbps: u32,
    },
}

struct Pending {
    id: i64,
    fetch: Fetch,
}

struct Job {
    id: i64,
    progress: Arc<Progress>,
    /// Bei HLS: gelesene Position (ms); `progress.total` ist dann die Dauer in ms.
    position_ms: Arc<AtomicU64>,
    hls: bool,
}

enum Recorder {
    /// Transportstrom direkt vom Gerät (SAT>IP, DVB-Karte).
    Ts(Box<dyn TsRecorder>),
    Stream(Arc<Progress>),
}

/// Aufnahme eines Transportstroms (eigene Sitzung bzw. eigenes Frontend).
trait TsRecorder: Send {
    fn error(&self) -> Option<String>;
    fn finish(self: Box<Self>) -> Result<PathBuf, String>;
}

impl TsRecorder for tvjoe_core::satip::Recorder {
    fn error(&self) -> Option<String> {
        tvjoe_core::satip::Recorder::error(self)
    }
    fn finish(self: Box<Self>) -> Result<PathBuf, String> {
        tvjoe_core::satip::Recorder::finish(*self)
    }
}

#[cfg(target_os = "linux")]
impl TsRecorder for tvjoe_core::dvb::Recorder {
    fn error(&self) -> Option<String> {
        tvjoe_core::dvb::Recorder::error(self)
    }
    fn finish(self: Box<Self>) -> Result<PathBuf, String> {
        tvjoe_core::dvb::Recorder::finish(*self)
    }
}

struct Rec {
    id: i64,
    /// Empfangsquelle (Nummer).
    source: u32,
    channel_id: u64,
    /// Ende (Sekunden seit 1970) mit Nachlauf.
    stop_at: i64,
    timer_id: Option<i64>,
    rec: Option<Recorder>,
}

enum Done {
    Job(i64, Result<PathBuf, String>),
    Rec(i64, Result<(), String>),
}

pub struct Ablage {
    app: Weak<App>,
    index: Index,
    queue: RefCell<VecDeque<Pending>>,
    jobs: RefCell<Vec<Job>>,
    recs: RefCell<Vec<Rec>>,
    badges: RefCell<Vec<(String, String, glib::WeakRef<gtk::Button>)>>,
    /// Knöpfe „Herunterladen“ auf Infoseiten (Beschriftung je Zustand).
    store_buttons: RefCell<Vec<(String, String, glib::WeakRef<gtk::Button>)>>,
    done_tx: async_channel::Sender<Done>,
    suspend_cookie: Cell<Option<u32>>,
    ticks: Cell<u32>,
}

/// Beim Start: Verzeichnis öffnen, Abgebrochenes als „teilweise“ markieren, Takt starten.
pub fn init(app: &Rc<App>) {
    let Some(path) = paths::config_dir().map(|d| d.join("ablage.sqlite")) else {
        return;
    };
    let index = match Index::open(&path) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("tvjoe: Ablage: {e}");
            return;
        }
    };
    index.mark_interrupted();
    move_into_subdirs(app, &index);
    // Einstellung an den tatsächlichen Zustand des Aufnahmedienstes angleichen.
    let installed = crate::dienst::installed();
    if app.settings().record_service != installed {
        app.settings_mut().record_service = installed;
        app.save_settings();
    }
    let (tx, rx) = async_channel::unbounded();
    let a = Rc::new(Ablage {
        app: Rc::downgrade(app),
        index,
        queue: RefCell::new(VecDeque::new()),
        jobs: RefCell::new(Vec::new()),
        recs: RefCell::new(Vec::new()),
        badges: RefCell::new(Vec::new()),
        store_buttons: RefCell::new(Vec::new()),
        done_tx: tx,
        suspend_cookie: Cell::new(None),
        ticks: Cell::new(0),
    });
    ABLAGE.with(|s| *s.borrow_mut() = Some(a));
    glib::spawn_future_local(async move {
        while let Ok(d) = rx.recv().await {
            if let Some(a) = get() {
                a.finished(d);
            }
        }
    });
    glib::timeout_add_seconds_local(1, || {
        if let Some(a) = get() {
            a.tick();
        }
        glib::ControlFlow::Continue
    });
}

/// Ablageordner für Heruntergeladenes und Aufnahmen (Einstellung, sonst `Videos/tvjoe`).
pub fn storage_dir(app: &App) -> PathBuf {
    storage_dir_for(&app.settings())
}

/// Ablageordner nach den Einstellungen (auch ohne Fenster, Aufnahmedienst).
pub fn storage_dir_for(s: &tvjoe_core::Settings) -> PathBuf {
    if let Some(d) = s.storage_dir.as_deref().filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    glib::user_special_dir(glib::UserDirectory::Videos)
        .unwrap_or_else(|| glib::home_dir().join("Videos"))
        .join("tvjoe")
}

/// Unterordner der Ablage je Art und Herkunft (Nutzer, 2026-10-26): `cache` (Zwischenspeicher),
/// `records` (Aufnahmen), `mediathek`, `jellyfin` (Downloads).
fn sub_dir(app: &App, art: Art, source: &str) -> PathBuf {
    storage_dir(app).join(match (art, source) {
        (Art::Cache, _) => "cache",
        (Art::Recording, _) => "records",
        (_, "oer") => "mediathek",
        (_, s) if is_server_source(s) => s,
        _ => "mediathek",
    })
}

/// Unterordner anlegen; was noch direkt im Ablageordner liegt (vor den Unterordnern),
/// vollständig oder teilweise geladen, in seinen Unterordner verschieben und das Verzeichnis
/// anpassen.
fn move_into_subdirs(app: &App, index: &Index) {
    for (art, source) in [
        (Art::Cache, ""),
        (Art::Recording, ""),
        (Art::Download, "oer"),
        (Art::Download, "jellyfin"),
    ] {
        let _ = std::fs::create_dir_all(sub_dir(app, art, source));
    }
    let base = storage_dir(app);
    for e in index.all() {
        if !matches!(e.state, State::Done | State::Partial)
            || e.path.parent() != Some(base.as_path())
        {
            continue;
        }
        let Some(name) = e.path.file_name() else { continue };
        let target = ablage::unique_path(&sub_dir(app, e.art, &e.source).join(name));
        // Teilweise Geladenes liegt als `<datei>.part` da.
        let (from, to) = if e.state == State::Done {
            (e.path.clone(), target.clone())
        } else {
            (ablage::part_path(&e.path), ablage::part_path(&target))
        };
        let moved = if from.exists() {
            std::fs::rename(&from, &to)
        } else {
            Ok(())
        };
        match moved {
            Ok(()) => index.set_path(e.id, &target),
            Err(err) => eprintln!("tvjoe: Ablage: {} nicht verschoben: {err}", from.display()),
        }
    }
}

/// Freier Platz auf dem Datenträger von `dir` (der Ordner muss noch nicht existieren); `None`,
/// wenn das System keine Angabe liefert (dann wird nicht gebremst). Windows fragt direkt
/// (`GetDiskFreeSpaceExW`): unter dem Dienstkonto liefert gio die Angabe nicht.
pub fn free_bytes(dir: &Path) -> Option<u64> {
    let mut p = dir;
    while !p.exists() {
        p = p.parent()?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        unsafe extern "system" {
            fn GetDiskFreeSpaceExW(
                dir: *const u16,
                free_to_caller: *mut u64,
                total: *mut u64,
                total_free: *mut u64,
            ) -> i32;
        }
        let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free = 0u64;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        return (ok != 0).then_some(free);
    }
    #[cfg(not(windows))]
    {
        let info = gio::File::for_path(p)
            .query_filesystem_info("filesystem::free", gio::Cancellable::NONE)
            .ok()?;
        info.has_attribute("filesystem::free")
            .then(|| info.attribute_uint64("filesystem::free"))
    }
}

/// Ortszeit für Dateinamen.
pub fn stamp(ts: i64) -> String {
    glib::DateTime::from_unix_local(ts)
        .ok()
        .and_then(|d| d.format("%Y-%m-%d %H-%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn gb(n: u32) -> u64 {
    u64::from(n) << 30
}

// ---------------------------------------------------------------------------------------------
// Herunterladen und Zwischenspeichern

/// Beitrag der ÖR-Mediathek herunterladen bzw. zwischenspeichern.
pub fn fetch_film(app: &Rc<App>, film: &Film, art: Art) {
    let Some(a) = get() else { return };
    let quality = {
        let s = app.settings();
        if art == Art::Download {
            s.download_quality
        } else {
            s.quality
        }
    };
    let url = film.url_for(quality).to_string();
    let hls = url.contains(".m3u8");
    let ext = if hls {
        "ts".to_string()
    } else {
        ablage::extension_of(&url, "mp4")
    };
    let name = ablage::show_name(&film.sender, &film.display_title(), &stamp(film.timestamp), &ext);
    let fetch = if hls {
        Fetch::Hls { url, header: None }
    } else {
        Fetch::Http { url, header: None }
    };
    a.enqueue(
        app,
        art,
        "oer",
        &film.url,
        &film.display_title(),
        &format!("{} · {}", film.sender, film.topic),
        &name,
        fetch,
    );
}

/// Jellyfin-Eintrag (Film oder Folge) herunterladen bzw. zwischenspeichern.
pub fn fetch_item(app: &Rc<App>, library: &str, item: &Item, art: Art) {
    let Some(a) = get() else { return };
    let Some(account) = crate::jellyfin::account(library) else {
        app.message(tr("Jellyfin: nicht angemeldet"));
        return;
    };
    let (original, kbps) = {
        let s = app.settings();
        (s.jellyfin_download_original, s.jellyfin_download_kbps)
    };
    // Endung der Originaldatei kennt erst der Server; der Ladethread korrigiert sie.
    let ext = if original { "mkv" } else { "ts" };
    let title = match (&item.series_name, item.season_no, item.episode_no) {
        (Some(s), Some(se), Some(ep)) => format!("{s} S{se:02}E{ep:02} {}", item.name),
        _ => item.name.clone(),
    };
    let name = match item.kind.as_str() {
        "Episode" => format!("{}.{ext}", ablage::clean_name(&title)),
        _ => ablage::film_name(&item.name, item.year, ext),
    };
    a.enqueue(
        app,
        art,
        library,
        &item.id,
        &title,
        "Jellyfin",
        &name,
        Fetch::Jelly {
            account,
            item_id: item.id.clone(),
            original,
            kbps,
        },
    );
}

/// Laufenden oder wartenden Download abbrechen.
pub fn cancel(source: &str, key: &str) {
    let Some(a) = get() else { return };
    for e in a.index.find(source, key) {
        a.queue.borrow_mut().retain(|p| p.id != e.id);
        if let Some(j) = a.jobs.borrow().iter().find(|j| j.id == e.id) {
            j.progress.cancel.store(true, Ordering::Relaxed);
        }
        if e.state == State::Queued {
            a.index.set_state(e.id, State::Partial, Some(tr("abgebrochen")));
        }
    }
    a.refresh_badges();
}

/// Lokale Datei einer Sendung, falls vollständig in der Ablage (Download vor Zwischenspeicher).
pub fn local_file(source: &str, key: &str) -> Option<PathBuf> {
    let a = get()?;
    let mut entries = a.index.find(source, key);
    entries.sort_by_key(|e| e.art != Art::Download);
    let e = entries
        .into_iter()
        .find(|e| e.state == State::Done && e.path.exists())?;
    a.index.touch(e.id);
    Some(e.path)
}

/// Heruntergeladenes einer Herkunft (laufend, wartend oder fertig mit vorhandener Datei; ohne
/// Zwischengespeichertes), neueste zuerst, je Kennung einmal.
pub fn stored(source: &str) -> Vec<ablage::Entry> {
    let Some(a) = get() else { return Vec::new() };
    let mut seen = std::collections::HashSet::new();
    let mut all: Vec<ablage::Entry> = a
        .index
        .all()
        .into_iter()
        .filter(|e| {
            e.source == source
                && e.art == Art::Download
                && match e.state {
                    State::Failed => false,
                    State::Done => e.path.exists(),
                    _ => true,
                }
        })
        .collect();
    all.sort_by_key(|e| -e.created);
    all.retain(|e| seen.insert(e.key.clone()));
    all
}

/// Kennungen des Heruntergeladenen einer Herkunft (Schalter „Geladen“).
pub fn stored_keys(source: &str) -> std::collections::HashSet<String> {
    stored(source).into_iter().map(|e| e.key).collect()
}

/// Zustand einer Sendung für Kontextmenü und Kennzeichen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    None,
    Loading,
    Done,
    Partial,
}

pub fn status(source: &str, key: &str) -> Status {
    let Some(a) = get() else {
        return Status::None;
    };
    let entries = a.index.find(source, key);
    if entries
        .iter()
        .any(|e| matches!(e.state, State::Loading | State::Queued))
    {
        Status::Loading
    } else if entries.iter().any(|e| e.state == State::Done && e.path.exists()) {
        Status::Done
    } else if entries.iter().any(|e| e.state == State::Partial) {
        Status::Partial
    } else {
        Status::None
    }
}

/// Kennzeichen für eine Kachel (unten rechts): vollständig, lädt (mit Anteil), teilweise.
/// Ohne Eintrag unsichtbar; wird jede Sekunde nachgeführt. Teilweise geladen: ein Klick setzt
/// fort (`resume` mit der Art des Abgebrochenen); lädt: ein Klick bricht nach Rückfrage ab;
/// sonst geht der Klick an die Kachel.
pub fn badge(source: &str, key: &str, resume: impl Fn(Art) + 'static) -> gtk::Button {
    let button = gtk::Button::builder()
        .child(&gtk::Image::builder().pixel_size(16).build())
        .halign(gtk::Align::End)
        .valign(gtk::Align::End)
        .margin_end(6)
        .margin_bottom(8)
        .css_classes(["flat", "circular", "tvjoe-store-badge"])
        .visible(false)
        .build();
    let (s, k) = (source.to_string(), key.to_string());
    button.connect_clicked(move |b| {
        let Some(a) = get() else { return };
        let entries = a.index.find(&s, &k);
        match status(&s, &k) {
            Status::Partial => {
                let art = entries
                    .iter()
                    .find(|e| e.state == State::Partial)
                    .map_or(Art::Download, |e| e.art);
                resume(art);
            }
            Status::Loading => confirm_cancel(b, &s, &k),
            _ => {}
        }
    });
    if let Some(a) = get() {
        a.badges
            .borrow_mut()
            .push((source.to_string(), key.to_string(), button.downgrade()));
        a.update_badge(source, key, &button);
    }
    button
}

/// Knopf „Herunterladen“ für Infoseiten: lädt herunter, setzt fort, bricht nach Rückfrage ab
/// oder zeigt „In der Ablage“; Beschriftung und Anteil werden jede Sekunde nachgeführt.
pub fn store_button(source: &str, key: &str, fetch: impl Fn(Art) + 'static) -> gtk::Button {
    let button = gtk::Button::builder()
        .css_classes(["pill"])
        .child(&adw::ButtonContent::builder().icon_name("folder-download-symbolic").build())
        .build();
    let (s, k) = (source.to_string(), key.to_string());
    button.connect_clicked(move |b| {
        let Some(a) = get() else { return };
        let entries = a.index.find(&s, &k);
        match status(&s, &k) {
            Status::None => fetch(Art::Download),
            Status::Partial => fetch(
                entries
                    .iter()
                    .find(|e| e.state == State::Partial)
                    .map_or(Art::Download, |e| e.art),
            ),
            Status::Loading => confirm_cancel(b, &s, &k),
            Status::Done => {}
        }
        a.refresh_badges();
    });
    if let Some(a) = get() {
        a.store_buttons
            .borrow_mut()
            .push((source.to_string(), key.to_string(), button.downgrade()));
        a.update_store_button(source, key, &button);
    }
    button
}

/// Rückfrage vor dem Abbrechen eines Downloads.
fn confirm_cancel(parent: &impl IsA<gtk::Widget>, source: &str, key: &str) {
    let title = get()
        .and_then(|a| a.index.find(source, key).first().map(|e| e.title.clone()))
        .unwrap_or_default();
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Laden abbrechen?"))
        .body(tf!(
            "„{title}“ wird nicht weiter geladen. Was schon da ist, bleibt; „Fortsetzen“ lädt später den Rest.",
            title = title
        ))
        .build();
    dialog.add_responses(&[("keep", tr("Weiter laden")), ("cancel", tr("Abbrechen"))]);
    dialog.set_response_appearance("cancel", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("keep"));
    let (s, k) = (source.to_string(), key.to_string());
    dialog.connect_response(None, move |_, r| {
        if r == "cancel" {
            cancel(&s, &k);
        }
    });
    dialog.present(Some(parent));
}

/// Knopf „Herunterladen“ für einen ÖR-Beitrag.
pub fn film_store_button(app: &Rc<App>, film: &Film) -> gtk::Button {
    let (weak, f) = (Rc::downgrade(app), film.clone());
    store_button("oer", &film.url, move |art| {
        if let Some(app) = weak.upgrade() {
            fetch_film(&app, &f, art);
        }
    })
}

/// Knopf „Herunterladen“ für einen Jellyfin-Eintrag.
pub fn item_store_button(app: &Weak<App>, library: &str, item: &Item) -> gtk::Button {
    let (weak, it, lib) = (app.clone(), item.clone(), library.to_string());
    store_button(library, &item.id, move |art| {
        if let Some(app) = weak.upgrade() {
            fetch_item(&app, &lib, &it, art);
        }
    })
}

/// Kennzeichen eines ÖR-Beitrags (setzt mit diesem Beitrag fort).
pub fn film_badge(app: &Rc<App>, film: &Film) -> gtk::Button {
    let (weak, f) = (Rc::downgrade(app), film.clone());
    badge("oer", &film.url, move |art| {
        if let Some(app) = weak.upgrade() {
            fetch_film(&app, &f, art);
        }
    })
}

/// Kennzeichen eines Jellyfin-Eintrags.
pub fn item_badge(app: &Weak<App>, library: &str, item: &Item) -> gtk::Button {
    let (weak, it, lib) = (app.clone(), item.clone(), library.to_string());
    badge(library, &item.id, move |art| {
        if let Some(app) = weak.upgrade() {
            fetch_item(&app, &lib, &it, art);
        }
    })
}

impl Ablage {
    #[allow(clippy::too_many_arguments)]
    fn enqueue(
        &self,
        app: &Rc<App>,
        art: Art,
        source: &str,
        key: &str,
        title: &str,
        subtitle: &str,
        name: &str,
        fetch: Fetch,
    ) {
        let existing = self.index.find(source, key);
        if existing
            .iter()
            .any(|e| e.art == art && e.state == State::Done && e.path.exists())
        {
            app.message(&tf!("Liegt schon in der Ablage: {title}", title = title));
            return;
        }
        if existing
            .iter()
            .any(|e| matches!(e.state, State::Loading | State::Queued))
        {
            app.message(&tf!("Wird schon geladen: {title}", title = title));
            return;
        }
        let dir = sub_dir(app, art, source);
        let min_free = gb(app.settings().min_free_gb);
        if free_bytes(&dir).is_some_and(|f| f < min_free) {
            app.message(&tf!(
                "Zu wenig Platz in {dir} (unter {gb} GB frei)",
                dir = dir.display(),
                gb = app.settings().min_free_gb
            ));
            return;
        }
        // Abgebrochenen Download derselben Art an derselben Stelle fortsetzen.
        let path = existing
            .iter()
            .find(|e| e.art == art && e.state == State::Partial)
            .map(|e| e.path.clone())
            .unwrap_or_else(|| ablage::unique_path(&dir.join(name)));
        let id = match self.index.add(art, source, key, title, subtitle, &path) {
            Ok(id) => id,
            Err(e) => {
                app.message(&tf!("Ablage: {e}", e = e));
                return;
            }
        };
        self.queue.borrow_mut().push_back(Pending { id, fetch });
        app.message(&if art == Art::Cache {
            tf!("Wird zwischengespeichert: {title}", title = title)
        } else {
            tf!("Wird heruntergeladen: {title}", title = title)
        });
        self.pump();
        self.refresh_badges();
    }

    /// Wartende starten, solange Plätze frei sind.
    fn pump(&self) {
        while self.jobs.borrow().len() < PARALLEL {
            let Some(p) = self.queue.borrow_mut().pop_front() else {
                break;
            };
            self.start(p);
        }
    }

    fn start(&self, p: Pending) {
        let Some(entry) = self.index.get(p.id) else {
            return;
        };
        self.index.set_state(p.id, State::Loading, None);
        let progress = Arc::new(Progress::default());
        let position_ms = Arc::new(AtomicU64::new(0));
        let hls = matches!(
            p.fetch,
            Fetch::Hls { .. } | Fetch::Jelly { original: false, .. }
        );
        self.jobs.borrow_mut().push(Job {
            id: p.id,
            progress: progress.clone(),
            position_ms: position_ms.clone(),
            hls,
        });
        let tx = self.done_tx.clone();
        let id = p.id;
        std::thread::spawn(move || {
            let r = run_fetch(p.fetch, entry.path, &progress, &position_ms);
            let _ = tx.send_blocking(Done::Job(id, r));
        });
    }

    fn finished(&self, d: Done) {
        let app = self.app.upgrade();
        match d {
            Done::Job(id, r) => {
                let cancelled = self
                    .jobs
                    .borrow()
                    .iter()
                    .any(|j| j.id == id && j.progress.cancelled());
                self.jobs.borrow_mut().retain(|j| j.id != id);
                let Some(e) = self.index.get(id) else { return };
                match r {
                    Ok(path) => {
                        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        self.index.set_path(id, &path);
                        self.index.set_size(id, size, size);
                        self.index.set_state(id, State::Done, None);
                        if let Some(app) = &app {
                            app.message(&if e.art == Art::Cache {
                                tf!("Zwischengespeichert: {title}", title = e.title)
                            } else {
                                tf!("Heruntergeladen: {title}", title = e.title)
                            });
                        }
                        if e.art == Art::Cache {
                            self.enforce_cache_limit();
                        }
                    }
                    Err(err) => {
                        let size = std::fs::metadata(ablage::part_path(&e.path))
                            .map(|m| m.len())
                            .unwrap_or(0);
                        self.index.set_size(id, size, e.total);
                        self.index.set_state(
                            id,
                            if cancelled || size > 0 {
                                State::Partial
                            } else {
                                State::Failed
                            },
                            Some(&err),
                        );
                        if let (Some(app), false) = (&app, cancelled) {
                            app.message(&tf!("Laden fehlgeschlagen: {title} ({err})", title = e.title, err = err));
                        }
                    }
                }
                self.pump();
            }
            Done::Rec(id, r) => {
                let Some(e) = self.index.get(id) else { return };
                let size = std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0);
                self.index.set_size(id, size, size);
                match r {
                    Ok(()) => {
                        self.index.set_state(id, State::Done, None);
                        if let Some(app) = &app {
                            app.message(&tf!("Aufnahme beendet: {title}", title = e.title));
                        }
                    }
                    Err(err) => {
                        self.index.set_state(
                            id,
                            if size > 0 { State::Partial } else { State::Failed },
                            Some(&err),
                        );
                        if let Some(app) = &app {
                            app.message(&tf!("Aufnahme abgebrochen: {title} ({err})", title = e.title, err = err));
                        }
                    }
                }
            }
        }
        self.refresh_badges();
    }

    /// Zwischenspeicher auf die Grenze bringen: am längsten nicht Benutztes löschen.
    fn enforce_cache_limit(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let limit = gb(app.settings().cache_limit_gb);
        for e in self.index.evictable(limit, 0) {
            let _ = std::fs::remove_file(&e.path);
            let _ = std::fs::remove_file(ablage::part_path(&e.path));
            self.index.remove(e.id);
        }
    }

    /// Jede Sekunde: Kennzeichen; alle 5 s Größen ins Verzeichnis; alle 15 s Aufnahmen und Plan.
    fn tick(&self) {
        let t = self.ticks.get().wrapping_add(1);
        self.ticks.set(t);
        if !self.jobs.borrow().is_empty() || !self.recs.borrow().is_empty() {
            self.refresh_badges();
        }
        if t % 5 == 0 {
            for j in self.jobs.borrow().iter() {
                self.index.set_size(
                    j.id,
                    j.progress.done.load(Ordering::Relaxed),
                    if j.hls {
                        0
                    } else {
                        j.progress.total.load(Ordering::Relaxed)
                    },
                );
            }
        }
        if t % 15 == 1 {
            self.check_recordings();
        }
        // Windows-Dienst: seinen Stand übernehmen.
        #[cfg(windows)]
        if t % 5 == 0 {
            if let Some(app) = self.app.upgrade().filter(|a| a.settings().record_service) {
                crate::dienst_win::import_status(&app);
            }
        }
    }

    fn refresh_badges(&self) {
        let list: Vec<(String, String, gtk::Button)> = {
            let mut b = self.badges.borrow_mut();
            b.retain(|(_, _, w)| w.upgrade().is_some());
            b.iter()
                .filter_map(|(s, k, w)| w.upgrade().map(|i| (s.clone(), k.clone(), i)))
                .collect()
        };
        for (s, k, img) in list {
            self.update_badge(&s, &k, &img);
        }
        let buttons: Vec<(String, String, gtk::Button)> = {
            let mut b = self.store_buttons.borrow_mut();
            b.retain(|(_, _, w)| w.upgrade().is_some());
            b.iter()
                .filter_map(|(s, k, w)| w.upgrade().map(|i| (s.clone(), k.clone(), i)))
                .collect()
        };
        for (s, k, b) in buttons {
            self.update_store_button(&s, &k, &b);
        }
    }

    fn update_store_button(&self, source: &str, key: &str, button: &gtk::Button) {
        let Some(content) = button.child().and_downcast::<adw::ButtonContent>() else {
            return;
        };
        let entries = self.index.find(source, key);
        let running = entries.iter().find_map(|e| {
            self.jobs
                .borrow()
                .iter()
                .find(|j| j.id == e.id)
                .map(|j| fraction(j))
        });
        let (icon, text, active) = match status(source, key) {
            Status::Loading => (
                "process-stop-symbolic",
                match running.flatten() {
                    Some(f) => tf!(
                        "Wird geladen {percent} % – abbrechen",
                        percent = format!("{:.0}", f * 100.0)
                    ),
                    None => tr("Wird geladen – abbrechen").to_string(),
                },
                true,
            ),
            Status::Partial => ("folder-download-symbolic", tr("Herunterladen fortsetzen").into(), true),
            Status::Done => ("drive-harddisk-symbolic", tr("In der Ablage").into(), false),
            Status::None => ("folder-download-symbolic", tr("Herunterladen").into(), true),
        };
        content.set_icon_name(icon);
        content.set_label(&text);
        button.set_sensitive(active);
    }

    fn update_badge(&self, source: &str, key: &str, button: &gtk::Button) {
        let Some(img) = button.child().and_downcast::<gtk::Image>() else {
            return;
        };
        let entries = self.index.find(source, key);
        let running = entries.iter().find_map(|e| {
            self.jobs
                .borrow()
                .iter()
                .find(|j| j.id == e.id)
                .map(|j| fraction(j))
        });
        let (icon, tip) = if let Some(f) = running {
            (
                "folder-download-symbolic",
                match f {
                    Some(f) => tf!("Wird geladen: {percent} %", percent = format!("{:.0}", f * 100.0)),
                    None => tr("Wird geladen").to_string(),
                },
            )
        } else if entries.iter().any(|e| e.state == State::Queued) {
            ("folder-download-symbolic", tr("Wartet auf das Laden").to_string())
        } else if let Some(e) = entries
            .iter()
            .find(|e| e.state == State::Done && e.path.exists())
        {
            (
                "drive-harddisk-symbolic",
                if e.art == Art::Cache {
                    tr("Zwischengespeichert").to_string()
                } else {
                    tr("Heruntergeladen").to_string()
                },
            )
        } else if entries.iter().any(|e| e.state == State::Partial) {
            (
                "media-playback-pause-symbolic",
                tr("Teilweise geladen (Herunterladen setzt fort)").to_string(),
            )
        } else {
            button.set_visible(false);
            return;
        };
        img.set_icon_name(Some(icon));
        // Nur Laden und Teilweise nehmen Klicks an (Abbrechen, Fortsetzen); sonst die Kachel.
        let clickable = icon != "drive-harddisk-symbolic";
        button.set_can_target(clickable);
        button.set_tooltip_text(Some(&if clickable {
            if icon == "media-playback-pause-symbolic" {
                tf!("{tip} – Klick: fortsetzen", tip = tip)
            } else {
                tf!("{tip} – Klick: abbrechen", tip = tip)
            }
        } else {
            tip
        }));
        button.set_visible(true);
    }
}

/// Anteil eines laufenden Downloads (0–1), falls bekannt.
fn fraction(j: &Job) -> Option<f64> {
    let total = j.progress.total.load(Ordering::Relaxed);
    if total == 0 {
        return None;
    }
    let done = if j.hls {
        j.position_ms.load(Ordering::Relaxed)
    } else {
        j.progress.done.load(Ordering::Relaxed)
    };
    Some((done as f64 / total as f64).clamp(0.0, 1.0))
}

/// Im Ladethread: holen und die endgültige Datei zurückgeben.
fn run_fetch(
    fetch: Fetch,
    path: PathBuf,
    progress: &Progress,
    position_ms: &AtomicU64,
) -> Result<PathBuf, String> {
    match fetch {
        Fetch::Http { url, header } => {
            ablage::download(&url, header.as_deref(), &path, progress).map(|_| path)
        }
        Fetch::Hls { url, header } => {
            let src = capture::Source {
                url: &url,
                header: header.as_deref(),
                max_bitrate: None,
                live: false,
            };
            capture::run(&src, &path, progress, position_ms).map(|_| path)
        }
        Fetch::Jelly {
            account,
            item_id,
            original,
            kbps,
        } => {
            let header = account.header_line();
            if original {
                let (url, container, size) = account.download_source(&item_id)?;
                let path = path.with_extension(&container);
                progress.total.store(size, Ordering::Relaxed);
                ablage::download(&url, Some(&header), &path, progress).map(|_| path)
            } else {
                let session = tvjoe_core::jellyfin::new_device_id();
                let pb = account.playback(&item_id, kbps, &session)?;
                let src = capture::Source {
                    url: &pb.url,
                    header: Some(&header),
                    max_bitrate: None,
                    live: false,
                };
                let r = capture::run(&src, &path, progress, position_ms).map(|_| path);
                account.stop_transcoding(&session);
                r
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Aufnahmen

/// Taste R / Aufnahmeknopf: laufenden Sender bis zum Sendungsende (mit Nachlauf) aufnehmen bzw.
/// die laufende Aufnahme dieses Senders beenden.
pub fn toggle_record(app: &Rc<App>) {
    let Some(a) = get() else { return };
    let Some((cid, src)) = app.current_live() else {
        app.message(tr("Aufnehmen geht beim Fernsehen (nicht bei Abrufvideos)"));
        return;
    };
    // Aufnahmedienst: Sofortaufnahme als Planung ab jetzt; Beenden löscht sie aus dem Plan.
    if app.settings().record_service {
        let now = ablage::now();
        let post = i64::from(app.settings().record_post_min) * 60;
        let current = a
            .index
            .timers()
            .into_iter()
            .find(|t| t.channel_id == cid && t.source == src && t.start <= now + 60 && now < t.stop + post);
        if let Some(t) = current {
            a.index.remove_timer(t.id);
            app.message(&tf!("Aufnahme wird beendet: {title}", title = t.title));
            return;
        }
        let (title, stop) = app
            .programme_now(cid, src)
            .map(|(t, _, stop)| (t, stop))
            .unwrap_or_else(|| (String::new(), now + 4 * 3600));
        let name = app
            .record_source(cid, src)
            .map(|(n, _, _)| n)
            .unwrap_or_default();
        let timer = Timer {
            id: 0,
            channel_id: cid,
            source: src,
            channel: name.clone(),
            title: if title.is_empty() { name } else { title },
            // Vorlauf zählt der Dienst ab: so beginnt die Aufnahme sofort.
            start: now,
            stop,
        };
        match a.index.add_timer(&timer) {
            Ok(id) => {
                crate::dienst::start_now(id);
                crate::dienst::sync_tasks(app);
                app.message(&tf!(
                    "Aufnahme an den Aufnahmedienst übergeben: {title} (bis {end})",
                    title = timer.title,
                    end = crate::app::hm(stop + post)
                ));
            }
            Err(e) => app.message(&tf!("Aufnahme nicht übergeben: {e}", e = e)),
        }
        return;
    }
    let running = a
        .recs
        .borrow()
        .iter()
        .position(|r| r.channel_id == cid && r.source == src);
    if let Some(i) = running {
        a.stop(i);
        return;
    }
    let now = ablage::now();
    let post = i64::from(app.settings().record_post_min) * 60;
    let (title, stop_at) = match app.programme_now(cid, src) {
        Some((title, _, stop)) => (title, stop + post),
        // Ohne Programm: höchstens vier Stunden, vorher mit R beenden.
        None => (String::new(), now + 4 * 3600),
    };
    a.start_rec(app, cid, src, &title, stop_at, None);
}

/// Kennung einer Empfangsquelle in der Ablage und im Aufnahmeordner (`tv`, `sat`, `dvb-2` …).
pub fn source_key(app: &App, src: u32) -> String {
    app.settings()
        .source(src)
        .map(|a| a.id.clone())
        .unwrap_or_else(|| if src == SRC_SAT { "sat".into() } else { "tv".into() })
}

/// Läuft eine Aufnahme dieses Senders?
pub fn is_recording(channel_id: u64, src: u32) -> bool {
    get().is_some_and(|a| {
        let source = a
            .app
            .upgrade()
            .map(|app| source_key(&app, src))
            .unwrap_or_default();
        let prefix = format!("{channel_id}@");
        a.recs
            .borrow()
            .iter()
            .any(|r| r.channel_id == channel_id && r.source == src)
            // Aufnahmen des Dienstes (eigener Vorgang, im Verzeichnis „lädt“).
            || a.index
                .running_recordings()
                .iter()
                .any(|e| e.source == source && e.key.starts_with(&prefix))
    })
}

/// Geplante Aufnahme einer Sendung.
pub fn timer_for(channel_id: u64, src: u32, start: i64) -> Option<Timer> {
    get()?.index.timer_for(channel_id, src, start)
}

/// Aufnahme einer Sendung planen.
pub fn plan(app: &Rc<App>, timer: &Timer) {
    let Some(a) = get() else { return };
    match a.index.add_timer(timer) {
        Ok(_) => app.message(&tf!("Aufnahme geplant: {title}", title = timer.title)),
        Err(e) => app.message(&tf!("Aufnahme nicht geplant: {e}", e = e)),
    }
    a.check_recordings();
    crate::dienst::sync_tasks(app);
}

/// Planung löschen (eine schon laufende Aufnahme läuft weiter).
pub fn unplan(app: &Rc<App>, timer: &Timer) {
    let Some(a) = get() else { return };
    a.index.remove_timer(timer.id);
    for r in a.recs.borrow_mut().iter_mut() {
        if r.timer_id == Some(timer.id) {
            r.timer_id = None;
        }
    }
    app.message(&tf!("Planung gelöscht: {title}", title = timer.title));
    crate::dienst::sync_tasks(app);
}

/// Für die Rückfrage beim Beenden: laufende und geplante Aufnahmen, laufende Downloads.
pub fn busy_text() -> Option<String> {
    let a = get()?;
    let now = ablage::now();
    let service = a.app.upgrade().is_some_and(|app| app.settings().record_service);
    let recs = a.recs.borrow().len();
    // Mit Aufnahmedienst laufen Aufnahmen ohne das Fenster weiter.
    let planned = if service {
        0
    } else {
        a.index.timers().iter().filter(|t| t.stop > now).count()
    };
    let loads = a.jobs.borrow().len() + a.queue.borrow().len();
    let mut parts = Vec::new();
    if recs > 0 {
        parts.push(trn("{n} Aufnahme läuft", "{n} Aufnahmen laufen", recs as u64));
    }
    if planned > 0 {
        parts.push(trn("{n} Aufnahme geplant", "{n} Aufnahmen geplant", planned as u64));
    }
    if loads > 0 {
        parts.push(trn("{n} Download läuft", "{n} Downloads laufen", loads as u64));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

thread_local! {
    static CLOSE_CONFIRMED: Cell<bool> = const { Cell::new(false) };
}

/// Beenden trotz laufender Aufnahmen bestätigt.
pub fn confirm_close() {
    CLOSE_CONFIRMED.with(|c| c.set(true));
}

pub fn close_confirmed() -> bool {
    CLOSE_CONFIRMED.with(Cell::get)
}

/// Beim Beenden: Aufnahmen sauber abschließen (Datei umbenennen), Downloads anhalten (sie
/// lassen sich später fortsetzen).
pub fn shutdown() {
    let Some(a) = get() else { return };
    for j in a.jobs.borrow().iter() {
        j.progress.cancel.store(true, Ordering::Relaxed);
    }
    let recs: Vec<Rec> = a.recs.borrow_mut().drain(..).collect();
    for mut r in recs {
        let ok = match r.rec.take() {
            Some(Recorder::Ts(rec)) => rec.finish().is_ok(),
            Some(Recorder::Stream(p)) => {
                p.cancel.store(true, Ordering::Relaxed);
                false
            }
            None => false,
        };
        if ok {
            a.index.set_state(r.id, State::Done, None);
        }
    }
}

impl Ablage {
    fn start_rec(
        &self,
        app: &Rc<App>,
        cid: u64,
        src: u32,
        title: &str,
        stop_at: i64,
        timer_id: Option<i64>,
    ) {
        let (max, min_free, queue_bytes) = {
            let s = app.settings();
            (
                s.source(src).map_or(0, |a| a.record_max),
                gb(s.min_free_gb),
                u64::from(s.buffer_mb) << 19,
            )
        };
        let running = self.recs.borrow().iter().filter(|r| r.source == src).count();
        if max > 0 && running >= max as usize {
            app.message(&match src {
                SRC_SAT => tf!(
                    "Keine weitere SAT-Aufnahme: höchstens {max} gleichzeitig (Einstellungen → Empfang)",
                    max = max
                ),
                SRC_TV => tf!(
                    "Keine weitere Internet-Aufnahme: höchstens {max} gleichzeitig (Einstellungen → Empfang)",
                    max = max
                ),
                _ => tf!(
                    "Keine weitere Aufnahme über „{source}“: höchstens {max} gleichzeitig (Einstellungen → Empfang)",
                    source = app.source_title(src),
                    max = max
                ),
            });
            return;
        }
        let key = source_key(app, src);
        let dir = sub_dir(app, Art::Recording, &key);
        if free_bytes(&dir).is_some_and(|f| f < min_free) {
            app.message(&tf!(
                "Aufnahme nicht gestartet: zu wenig Platz in {dir}",
                dir = dir.display()
            ));
            return;
        }
        let Some((name, url, bitrate)) = app.record_source(cid, src) else {
            app.message(tr("Aufnahme nicht gestartet: Sender nicht gefunden"));
            return;
        };
        let title = if title.is_empty() { name.clone() } else { title.to_string() };
        let now = ablage::now();
        let path = ablage::unique_path(&dir.join(ablage::show_name(&name, &title, &stamp(now), "ts")));
        let id = match self.index.add(
            Art::Recording,
            &key,
            &format!("{cid}@{now}"),
            &title,
            &name,
            &path,
        ) {
            Ok(id) => id,
            Err(e) => {
                app.message(&tf!("Ablage: {e}", e = e));
                return;
            }
        };
        let driver = app.driver_of(src);
        let ts: Option<Result<Box<dyn TsRecorder>, String>> = match driver {
            tvjoe_core::adapter::Driver::Satip => Some(
                tvjoe_core::satip::Recorder::start(&url, &path, queue_bytes)
                    .map(|r| Box::new(r) as Box<dyn TsRecorder>),
            ),
            #[cfg(target_os = "linux")]
            tvjoe_core::adapter::Driver::LinuxDvb => Some(
                tvjoe_core::dvb::Recorder::start(&url, &path, queue_bytes)
                    .map(|r| Box::new(r) as Box<dyn TsRecorder>),
            ),
            _ => None,
        };
        let rec = if let Some(ts) = ts {
            match ts {
                Ok(r) => Recorder::Ts(r),
                Err(e) => {
                    self.index.set_state(id, State::Failed, Some(&e));
                    app.message(&tf!("Aufnahme nicht gestartet: {e}", e = e));
                    return;
                }
            }
        } else {
            let progress = Arc::new(Progress::default());
            let (p, tx) = (progress.clone(), self.done_tx.clone());
            let path = path.clone();
            std::thread::spawn(move || {
                let src = capture::Source {
                    url: &url,
                    header: None,
                    max_bitrate: bitrate,
                    live: true,
                };
                let r = capture::run(&src, &path, &p, &AtomicU64::new(0));
                let _ = tx.send_blocking(Done::Rec(id, r));
            });
            Recorder::Stream(progress)
        };
        self.index.set_state(id, State::Loading, None);
        self.recs.borrow_mut().push(Rec {
            id,
            source: src,
            channel_id: cid,
            stop_at,
            timer_id,
            rec: Some(rec),
        });
        app.message(&tf!(
            "Aufnahme läuft: {title} (bis {end})",
            title = title,
            end = crate::app::hm(stop_at)
        ));
        self.update_suspend();
    }

    /// Aufnahme `i` beenden (SAT: im Hintergrund, der Tuner wird frei).
    fn stop(&self, i: usize) {
        let mut r = self.recs.borrow_mut().remove(i);
        if let Some(t) = r.timer_id {
            self.index.remove_timer(t);
        }
        match r.rec.take() {
            Some(Recorder::Ts(rec)) => {
                let (tx, id) = (self.done_tx.clone(), r.id);
                let err = rec.error();
                std::thread::spawn(move || {
                    let res = rec.finish().map(|_| ());
                    let res = match (res, err) {
                        (Ok(()), Some(e)) => Err(e),
                        (r, _) => r,
                    };
                    let _ = tx.send_blocking(Done::Rec(id, res));
                });
            }
            Some(Recorder::Stream(p)) => p.cancel.store(true, Ordering::Relaxed),
            None => {}
        }
        self.update_suspend();
    }

    /// Alle 15 s: Enden, Fehler und Platz der laufenden Aufnahmen prüfen, Geplantes starten.
    fn check_recordings(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let now = ablage::now();
        let (pre, post, min_free) = {
            let s = app.settings();
            (
                i64::from(s.record_pre_min) * 60,
                i64::from(s.record_post_min) * 60,
                gb(s.min_free_gb),
            )
        };
        let low = free_bytes(&storage_dir(&app)).is_some_and(|f| f < min_free);
        loop {
            let due = self.recs.borrow().iter().position(|r| {
                r.stop_at <= now
                    || low
                    || matches!(&r.rec, Some(Recorder::Ts(s)) if s.error().is_some())
            });
            let Some(i) = due else { break };
            if low {
                app.message(tr("Aufnahme beendet: zu wenig Platz auf der Platte"));
            }
            self.stop(i);
        }
        // Mit Aufnahmedienst nimmt nur der Dienst auf.
        if app.settings().record_service {
            return;
        }
        for t in self.index.timers() {
            let (begin, end) = (t.start - pre, t.stop + post);
            let running = self.recs.borrow().iter().any(|r| r.timer_id == Some(t.id));
            if now >= end {
                if !running {
                    self.index.remove_timer(t.id);
                }
            } else if now >= begin && !running && !low {
                self.start_rec(&app, t.channel_id, t.source, &t.title, end, Some(t.id));
            }
        }
    }

    /// Ruhezustand verhindern, solange aufgenommen wird.
    fn update_suspend(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let Some(application) = app.window.application() else {
            return;
        };
        let want = !self.recs.borrow().is_empty();
        match (want, self.suspend_cookie.get()) {
            (true, None) => {
                let cookie = application.inhibit(
                    Some(&app.window),
                    gtk::ApplicationInhibitFlags::SUSPEND,
                    Some(tr("Aufnahme läuft")),
                );
                self.suspend_cookie.set((cookie != 0).then_some(cookie));
            }
            (false, Some(cookie)) => {
                application.uninhibit(cookie);
                self.suspend_cookie.set(None);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Kontextmenü

/// Rechtsklick auf `widget` öffnet ein Menü mit den Einträgen von `items` (erst beim Klick
/// berechnet, damit es den aktuellen Zustand zeigt).
pub fn context_menu<W: IsA<gtk::Widget>>(
    widget: &W,
    items: impl Fn() -> Vec<(String, Box<dyn Fn()>)> + 'static,
) {
    let click = gtk::GestureClick::builder().button(3).build();
    let w = widget.as_ref().clone();
    click.connect_pressed(move |g, _, x, y| {
        g.set_state(gtk::EventSequenceState::Claimed);
        let entries = items();
        if entries.is_empty() {
            return;
        }
        let popover = gtk::Popover::builder().has_arrow(true).build();
        popover.set_parent(&w);
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for (label, action) in entries {
            let b = gtk::Button::builder()
                .label(label)
                .css_classes(["flat"])
                .build();
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
            let pop = popover.clone();
            b.connect_clicked(move |_| {
                pop.popdown();
                action();
            });
            list.append(&b);
        }
        popover.set_child(Some(&list));
        popover.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        popover.popup();
    });
    widget.as_ref().add_controller(click);
}

/// Menüeinträge für eine Sendung (Herunterladen, Zwischenspeichern, Abbrechen) je Zustand.
pub fn entries_for(
    source: &str,
    key: &str,
    download: impl Fn() + Clone + 'static,
    cache: impl Fn() + 'static,
) -> Vec<(String, Box<dyn Fn()>)> {
    let mut out: Vec<(String, Box<dyn Fn()>)> = Vec::new();
    match status(source, key) {
        Status::Loading => {
            let (s, k) = (source.to_string(), key.to_string());
            out.push((tr("Laden abbrechen").into(), Box::new(move || cancel(&s, &k))));
        }
        Status::Done => {}
        Status::Partial => {
            out.push((tr("Herunterladen fortsetzen").into(), Box::new(download)));
        }
        Status::None => {
            out.push((tr("Herunterladen").into(), Box::new(download)));
            out.push((tr("Zwischenspeichern").into(), Box::new(cache)));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Einstellungen

/// Seite „Ablage“: Ordner und Grenzen, Herunterladen, Aufnahme.
pub fn prefs_page(
    app: &App,
) -> (
    adw::PreferencesPage,
    impl Fn(&mut tvjoe_core::Settings) + 'static,
) {
    use adw::prelude::*;
    use tvjoe_core::Quality;
    let s = app.settings().clone();
    let page = adw::PreferencesPage::builder()
        .name("ablage")
        .title(tr("Ablage"))
        .icon_name("drive-harddisk-symbolic")
        .build();

    let place = adw::PreferencesGroup::builder()
        .title(tr("Ablage"))
        .description(tr("Im Ordner entstehen Unterordner: mediathek und jellyfin (Heruntergeladenes), records (Aufnahmen), cache (Zwischenspeicher). Heruntergeladenes und Aufnahmen bleiben, bis Sie sie löschen; Zwischengespeichertes löscht tvjoe wieder, sobald die Grenze erreicht ist (am längsten nicht Gesehenes zuerst)."))
        .build();
    let dir: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(s.storage_dir.clone()));
    let dir_row = adw::ActionRow::builder()
        .title(tr("Ablageordner"))
        .subtitle(storage_dir(app).display().to_string())
        .build();
    let choose = gtk::Button::builder()
        .label(tr("Ändern …"))
        .valign(gtk::Align::Center)
        .build();
    dir_row.add_suffix(&choose);
    // Vorhandenes mitnehmen: Dateien verschieben, Verzeichnis nachtragen (Nutzer, 2026-10-27).
    let relocate = gtk::Button::builder()
        .label(tr("Verschieben nach …"))
        .valign(gtk::Align::Center)
        .build();
    dir_row.add_suffix(&relocate);
    {
        let (dir, row) = (dir.clone(), dir_row.clone());
        relocate.connect_clicked(move |b| {
            let (dir, row) = (dir.clone(), row.clone());
            crate::umzug::start(b, move |new| {
                row.set_subtitle(&new.display().to_string());
                *dir.borrow_mut() = Some(new.to_string_lossy().to_string());
            });
        });
    }
    {
        let (dir, row) = (dir.clone(), dir_row.clone());
        choose.connect_clicked(move |b| {
            let dialog = gtk::FileDialog::builder()
                .title(tr("Ordner für die Ablage"))
                .modal(true)
                .build();
            let parent = b.root().and_downcast::<gtk::Window>();
            let (dir, row) = (dir.clone(), row.clone());
            dialog.select_folder(parent.as_ref(), gio::Cancellable::NONE, move |r| {
                if let Some(path) = r.ok().and_then(|f| f.path()) {
                    row.set_subtitle(&path.display().to_string());
                    *dir.borrow_mut() = Some(path.to_string_lossy().to_string());
                }
            });
        });
    }
    let cache_limit = adw::SpinRow::with_range(1.0, 2000.0, 1.0);
    cache_limit.set_title(tr("Zwischenspeicher höchstens"));
    cache_limit.set_subtitle(tr("GB"));
    cache_limit.set_value(f64::from(s.cache_limit_gb));
    let min_free = adw::SpinRow::with_range(1.0, 500.0, 1.0);
    min_free.set_title(tr("Mindestens frei lassen"));
    min_free.set_subtitle(tr("GB; darunter beginnt kein Download und keine Aufnahme, eine laufende Aufnahme endet"));
    min_free.set_value(f64::from(s.min_free_gb));
    place.add(&dir_row);
    place.add(&cache_limit);
    place.add(&min_free);

    let download = adw::PreferencesGroup::builder()
        .title(tr("Herunterladen"))
        .description(tr("Rechtsklick auf eine Kachel: Herunterladen, Zwischenspeichern, bei Serien eine Staffel oder alle Folgen"))
        .build();
    let levels = [None, Some(Quality::P1080), Some(Quality::P720), Some(Quality::Sd)];
    let oer_quality = adw::ComboRow::builder()
        .title(tr("ÖR-Mediathek: Qualität"))
        .subtitle(tr("Zwischenspeichern nimmt die Qualität der Wiedergabe"))
        .model(&gtk::StringList::new(&[tr("Höchste"), "1080p", "720p", "SD"]))
        .selected(
            levels
                .iter()
                .position(|l| *l == s.download_quality)
                .unwrap_or(0) as u32,
        )
        .build();
    let jf_original = adw::SwitchRow::builder()
        .title(tr("Jellyfin: Originaldatei"))
        .subtitle(tr("Aus: vom Server umgerechnet mit der Bitrate darunter (kleinere Datei)"))
        .active(s.jellyfin_download_original)
        .build();
    let jf_kbps = adw::SpinRow::with_range(500.0, 40000.0, 500.0);
    jf_kbps.set_title(tr("Jellyfin: Bitrate beim Umrechnen"));
    jf_kbps.set_subtitle(tr("kbit/s"));
    jf_kbps.set_value(f64::from(s.jellyfin_download_kbps));
    download.add(&oer_quality);
    download.add(&jf_original);
    download.add(&jf_kbps);

    let record = adw::PreferencesGroup::builder()
        .title(tr("Aufnahme"))
        .description(tr("Taste R oder der rote Knopf in der Senderanzeige nimmt die laufende Sendung auf; im Programm per Rechtsklick planen. Aufgenommen wird nur, solange tvjoe läuft."))
        .build();
    let pre = adw::SpinRow::with_range(0.0, 30.0, 1.0);
    pre.set_title(tr("Vorlauf"));
    pre.set_subtitle(tr("Minuten vor Beginn laut Programm"));
    pre.set_value(f64::from(s.record_pre_min));
    let post = adw::SpinRow::with_range(0.0, 60.0, 1.0);
    post.set_title(tr("Nachlauf"));
    post.set_subtitle(tr("Minuten nach Ende laut Programm"));
    post.set_value(f64::from(s.record_post_min));
    // Höchstzahl gleichzeitiger Aufnahmen: je Empfangsquelle (Einstellungen → Empfang).
    for r in [&pre, &post] {
        record.add(r);
    }
    let service = adw::SwitchRow::builder()
        .title(tr("Aufnahmen auch ohne geöffnetes tvjoe"))
        .subtitle(if cfg!(windows) {
            tr("Richtet den Windows-Dienst „tvjoe Aufnahmedienst“ ein, der auch ohne Anmeldung aufnimmt und den Rechner dafür weckt. Wirkt beim Schließen der Einstellungen: Dann fragt Windows nach Administratorrechten, die Einrichtung dauert einige Sekunden; eine Meldung oben sagt, wann sie fertig ist")
        } else {
            tr("Richtet einen Hintergrunddienst ein (systemd-Benutzerdienst), der auch ohne Anmeldung aufnimmt; wirkt beim Schließen der Einstellungen")
        })
        .active(crate::dienst::installed())
        .build();
    record.add(&service);

    page.add(&place);
    page.add(&download);
    page.add(&record);
    let apply = move |s: &mut tvjoe_core::Settings| {
        s.storage_dir = dir.borrow().clone();
        s.cache_limit_gb = cache_limit.value() as u32;
        s.min_free_gb = min_free.value() as u32;
        s.download_quality = levels
            .get(oer_quality.selected() as usize)
            .copied()
            .flatten();
        s.jellyfin_download_original = jf_original.is_active();
        s.jellyfin_download_kbps = jf_kbps.value() as u32;
        s.record_pre_min = pre.value() as u32;
        s.record_post_min = post.value() as u32;
        if service.is_active() != s.record_service {
            crate::dienst::set_enabled(s, service.is_active());
        }
    };
    (page, apply)
}

// ---------------------------------------------------------------------------------------------
// Für die Reiter „Geplant“ und „Ablage“ im Programm

/// Alle Einträge, neueste zuerst.
pub fn all_entries() -> Vec<ablage::Entry> {
    get().map(|a| a.index.all()).unwrap_or_default()
}

/// Anteil eines laufenden Downloads: `Some(None)` = lädt, Anteil unbekannt; `None` = läuft nicht.
pub fn running_fraction(id: i64) -> Option<Option<f64>> {
    let a = get()?;
    let jobs = a.jobs.borrow();
    jobs.iter().find(|j| j.id == id).map(fraction)
}

/// Laufende Aufnahmen: Eintrag und Ende (mit Nachlauf).
pub fn running_recordings() -> Vec<(i64, i64)> {
    get()
        .map(|a| a.recs.borrow().iter().map(|r| (r.id, r.stop_at)).collect())
        .unwrap_or_default()
}

/// Laufende Aufnahme `id` beenden.
pub fn stop_recording(id: i64) {
    let Some(a) = get() else { return };
    let i = a.recs.borrow().iter().position(|r| r.id == id);
    if let Some(i) = i {
        a.stop(i);
    }
}

/// Geplante Aufnahmen, früheste zuerst.
pub fn timers() -> Vec<Timer> {
    get().map(|a| a.index.timers()).unwrap_or_default()
}

/// Hindernis für das Verschieben der Ablage: laufende Downloads und Aufnahmen; mit
/// Aufnahmedienst auch eine Aufnahme, die läuft oder in den nächsten 15 Minuten beginnt.
pub fn move_blocker() -> Option<String> {
    let a = get()?;
    let mut parts = Vec::new();
    let recs = a.recs.borrow().len();
    if recs > 0 {
        parts.push(trn("{n} Aufnahme läuft", "{n} Aufnahmen laufen", recs as u64));
    }
    let loads = a.jobs.borrow().len() + a.queue.borrow().len();
    if loads > 0 {
        parts.push(trn("{n} Download läuft", "{n} Downloads laufen", loads as u64));
    }
    let service = a.app.upgrade().is_some_and(|app| app.settings().record_service);
    let now = ablage::now();
    if service
        && a
            .index
            .timers()
            .iter()
            .any(|t| t.start <= now + 15 * 60 && t.stop > now)
    {
        parts.push(tr("der Aufnahmedienst nimmt gleich auf").to_string());
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// Neuer Ort einer Datei nach dem Verschieben der Ablage.
pub fn set_entry_path(id: i64, path: &Path) {
    if let Some(a) = get() {
        a.index.set_path(id, path);
    }
}

/// Freier Platz am Ablageordner.
pub fn free_space(app: &App) -> Option<u64> {
    free_bytes(&storage_dir(app))
}

/// Eintrag löschen: laufendes Laden abbrechen, Datei und Teildatei entfernen, aus dem
/// Verzeichnis nehmen. Laufende Aufnahmen werden vorher beendet.
pub fn delete_entry(id: i64) {
    let Some(a) = get() else { return };
    let Some(e) = a.index.get(id) else { return };
    if a.recs.borrow().iter().any(|r| r.id == id) {
        stop_recording(id);
    }
    a.queue.borrow_mut().retain(|p| p.id != id);
    if let Some(j) = a.jobs.borrow().iter().find(|j| j.id == id) {
        j.progress.cancel.store(true, Ordering::Relaxed);
    }
    let _ = std::fs::remove_file(&e.path);
    let _ = std::fs::remove_file(ablage::part_path(&e.path));
    let _ = std::fs::remove_file(capture::part_path(&e.path));
    a.index.remove(id);
    a.refresh_badges();
}

/// Beitrag der ÖR-Mediathek zu einem Eintrag: aus der Filmliste, sonst aus den Angaben der
/// Ablage (die Kennung ist die Video-URL).
fn film_of(e: &ablage::Entry) -> Film {
    crate::mediathek::film_by_url(&e.key).unwrap_or_else(|| {
        let (sender, topic) = e
            .subtitle
            .split_once(" · ")
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .unwrap_or_default();
        Film {
            id: -e.id,
            sender,
            topic,
            title: e.title.clone(),
            timestamp: e.created,
            duration: 0,
            description: String::new(),
            url: e.key.clone(),
            url_low: None,
            url_hd: None,
            website: None,
            subtitle: None,
            kind: tvjoe_core::mediathek::Kind::Other,
        }
    })
}

/// Staffel als Zahl für das Abo (`-1`: ohne Staffel).
fn season_key(season: Option<i64>) -> i64 {
    season.unwrap_or(-1)
}

/// Folgen einer Staffel auf dem Server, die nicht (vollständig oder gerade) in der Ablage sind.
pub fn missing_episodes(library: &str, series_id: &str, season: Option<i64>) -> Vec<Item> {
    let Some(a) = get() else { return Vec::new() };
    let entries = a.index.all();
    crate::jfview::season_episodes(library, series_id, season)
        .into_iter()
        .filter(|it| {
            !entries.iter().any(|e| {
                e.source == library
                    && e.key == it.id
                    && e.art == Art::Download
                    && match e.state {
                        State::Done => e.path.exists(),
                        State::Queued | State::Loading => true,
                        _ => running_fraction(e.id).is_some(),
                    }
            })
        })
        .collect()
}

/// Fehlende Folgen einer Staffel laden.
pub fn reload_season(app: &Rc<App>, library: &str, series_id: &str, season: Option<i64>) -> usize {
    let missing = missing_episodes(library, series_id, season);
    for it in &missing {
        fetch_item(app, library, it, Art::Download);
    }
    missing.len()
}

pub fn subscription(library: &str, series_id: &str, season: Option<i64>) -> Option<ablage::Subscription> {
    get().and_then(|a| a.index.subscription(library, series_id, season_key(season)))
}

pub fn set_subscription(library: &str, series_id: &str, season: Option<i64>, mode: Option<ablage::Subscription>) {
    if let Some(a) = get() {
        a.index.set_subscription(library, series_id, season_key(season), mode);
    }
}

thread_local! {
    /// Zuletzt gemeldete Zahl neuer Folgen je Abo (manuell): nur bei Änderung erneut melden.
    static REPORTED: RefCell<std::collections::HashMap<(String, String, i64), usize>> = RefCell::default();
}

/// Nach dem Abgleich einer Bibliothek: Abos ausführen – automatisch laden oder neue Folgen
/// melden (Knopf „Nachladen“ in der Ablage).
pub fn after_sync(app: &Rc<App>, library: &str) {
    let Some(a) = get() else { return };
    for (series_id, season, mode) in a.index.subscriptions(library) {
        let season_opt = (season >= 0).then_some(season);
        let missing = missing_episodes(library, &series_id, season_opt);
        match mode {
            ablage::Subscription::Auto => {
                for it in &missing {
                    fetch_item(app, library, it, Art::Download);
                }
                if !missing.is_empty() {
                    trace(&format!("Abo: {} neue Folgen geladen ({library}, {series_id}, Staffel {season})", missing.len()));
                }
            }
            ablage::Subscription::Manual => {
                let key = (library.to_string(), series_id.clone(), season);
                let before = REPORTED.with(|r| r.borrow().get(&key).copied().unwrap_or(0));
                if missing.len() > before {
                    let name = missing
                        .first()
                        .and_then(|it| it.series_name.clone())
                        .unwrap_or_default();
                    app.message(&tf!(
                        "{name}: {n} neue Folgen – Nachladen in der Ablage (Taste D)",
                        name = name,
                        n = missing.len()
                    ));
                }
                REPORTED.with(|r| r.borrow_mut().insert(key, missing.len()));
            }
        }
    }
}

/// Befund von „Alles prüfen“ (Nutzer, 2026-10-28).
#[derive(Debug, Clone)]
pub struct Finding {
    /// Eintrag im Verzeichnis (`None`: Datei ohne Eintrag).
    pub id: Option<i64>,
    pub title: String,
    pub path: PathBuf,
    pub problem: String,
    /// Nachladen möglich (Download aus Mediathek oder Server, keine Aufnahme).
    pub reload: bool,
}

/// Konsistenz aller lokalen Videodaten: Einträge ohne Datei, unvollständige Dateien,
/// abgebrochene Downloads (nicht laufend) und Dateien in den Ordnern der Ablage ohne Eintrag.
/// Gibt die Befunde und die Zahl geprüfter Einträge und Dateien zurück.
pub fn check_all(app: &App) -> (Vec<Finding>, usize, usize) {
    let Some(a) = get() else { return (Vec::new(), 0, 0) };
    let entries = a.index.all();
    let mut found = Vec::new();
    let mut known = std::collections::HashSet::new();
    for e in &entries {
        known.insert(e.path.clone());
        known.insert(ablage::part_path(&e.path));
        known.insert(capture::part_path(&e.path));
        if running_fraction(e.id).is_some() || a.queue.borrow().iter().any(|p| p.id == e.id) {
            continue;
        }
        let reload = e.art == Art::Download;
        let len = std::fs::metadata(&e.path).map(|m| m.len()).ok();
        let problem = match (e.state, len) {
            (State::Done, None) => Some(tr("Datei fehlt").to_string()),
            (State::Done, Some(l)) if e.total > 0 && l < e.total => Some(tf!(
                "unvollständig: {have} von {all}",
                have = crate::ablageview::size_text(l),
                all = crate::ablageview::size_text(e.total)
            )),
            (State::Partial, _) if e.art != Art::Recording => Some(tr("abgebrochen, nicht vollständig geladen").to_string()),
            (State::Failed, _) => Some(tr("fehlgeschlagen").to_string()),
            _ => None,
        };
        if let Some(problem) = problem {
            found.push(Finding { id: Some(e.id), title: e.title.clone(), path: e.path.clone(), problem, reload });
        }
    }
    // Dateien in den Unterordnern der Ablage, zu denen kein Eintrag gehört.
    let mut dirs = vec![
        sub_dir(app, Art::Cache, ""),
        sub_dir(app, Art::Recording, ""),
        sub_dir(app, Art::Download, "oer"),
    ];
    for l in &app.settings().libraries {
        if is_server_source(&l.id) {
            dirs.push(sub_dir(app, Art::Download, &l.id));
        }
    }
    let mut files = 0;
    for dir in dirs {
        let Ok(list) = std::fs::read_dir(&dir) else { continue };
        for f in list.flatten() {
            let path = f.path();
            if !path.is_file() || f.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            files += 1;
            if !known.contains(&path) {
                let size = f.metadata().map(|m| m.len()).unwrap_or(0);
                found.push(Finding {
                    id: None,
                    title: path.strip_prefix(storage_dir(app)).unwrap_or(&path).display().to_string(),
                    path,
                    problem: tf!("Datei ohne Eintrag, {size}", size = crate::ablageview::size_text(size)),
                    reload: false,
                });
            }
        }
    }
    (found, entries.len(), files)
}

/// Befund beheben: nachladen (wenn möglich) oder löschen (Eintrag samt Datei bzw. die Datei).
pub fn fix_finding(app: &Rc<App>, f: &Finding, reload: bool) {
    match (f.id, reload && f.reload) {
        (Some(id), true) => resume_entry(app, id),
        (Some(id), false) => delete_entry(id),
        (None, _) => {
            let _ = std::fs::remove_file(&f.path);
        }
    }
}

/// Abgebrochenen Download fortsetzen.
pub fn resume_entry(app: &Rc<App>, id: i64) {
    let Some(e) = get().and_then(|a| a.index.get(id)) else {
        return;
    };
    match e.source.as_str() {
        "oer" => fetch_film(app, &film_of(&e), e.art),
        s if is_server_source(s) => match crate::jfview::item(s, &e.key) {
            Some(it) => fetch_item(app, s, &it, e.art),
            None => app.message(tr("Jellyfin: Eintrag nicht mehr in der Bibliothek")),
        },
        _ => {}
    }
}

/// Eintrag abspielen (aus der lokalen Datei).
pub fn play_entry(app: &Rc<App>, id: i64) {
    let Some(e) = get().and_then(|a| a.index.get(id)) else {
        return;
    };
    match e.source.as_str() {
        "oer" => app.play_film(&film_of(&e), None),
        s if is_server_source(s) => match crate::jfview::item(s, &e.key) {
            Some(it) => app.play_jellyfin(s, &it, None, Vec::new()),
            None => app.play_file(&e.title, "Jellyfin", &e.path),
        },
        _ => app.play_file(&e.title, &e.subtitle, &e.path),
    }
}

/// Herkunft aus einer Server-Bibliothek (Jellyfin; Kennung der Instanz): nicht ÖR-Mediathek
/// und keine Aufnahme (Fernsehquellen `tv`, `sat`, weitere mit Nummer).
pub fn is_server_source(source: &str) -> bool {
    source == tvjoe_core::settings::LIBRARY_JELLYFIN
        || app().is_some_and(|app| {
            app.settings()
                .library(source)
                .is_some_and(|l| l.kind != "oer-mediathek")
        })
}

/// Hauptfenster (für Abläufe ohne eigenen Zugriff darauf, z. B. Einrichtung des Windows-Dienstes,
/// Verschieben der Ablage).
pub fn app() -> Option<Rc<App>> {
    get().and_then(|a| a.app.upgrade())
}

/// Erst im nächsten Durchlauf der Hauptschleife: der Aufrufer hält oft die Einstellungen zum
/// Schreiben (Einstellungsdialog), die Meldung liest sie.
pub fn report(text: &str) {
    let text = text.to_string();
    glib::idle_add_local_once(move || {
        if let Some(app) = get().and_then(|a| a.app.upgrade()) {
            app.message(&text);
        }
    });
}
