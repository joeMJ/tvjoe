//! Hauptfenster: Senderliste links, Bild rechts, Einblendung über dem Bild, Tastensteuerung.
//!
//! Tasten (Projektplan M4): Ziffern = Sender wählen (Pause aus den Einstellungen, Enter sofort),
//! Pfeil links/rechts bzw. Bild hoch/runter = Senderwechsel, F = Vollbild, Esc = Vollbild aus,
//! M = stumm, Q = Qualität eine Stufe höher (mit Umbruch), L = Senderliste, I = Info,
//! Pfeil hoch/runter, +/− und Mausrad = Lautstärke, Strg+Q = Beenden.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};
use tvjoe_core::epg::{self, Guide};
use tvjoe_core::{Channel, Library, Quality, Settings, SourceKind, Strategy, Stream, net, paths};

use crate::logos::Logos;
use crate::osd::{ChannelInfo, MessageBar, Origin, Osd};
use crate::player::{Player, PlayerEvent};
use crate::trace;

fn usable(c: &Channel) -> bool {
    c.active && !c.missing && !c.streams.is_empty()
}

fn quality_label(q: Option<Quality>) -> &'static str {
    q.map_or("Qualität unbekannt", Quality::label)
}

fn number_text(ch: &Channel) -> String {
    ch.number.map(|n| n.to_string()).unwrap_or_default()
}

struct Row {
    id: u64,
    row: gtk::ListBoxRow,
    logo: gtk::Picture,
    /// Laufende Sendung und nächste Sendung (EPG).
    now: gtk::Label,
    next: gtk::Label,
}

/// Uhrzeit in Ortszeit, z. B. „18:10“.
pub fn hm(ts: i64) -> String {
    glib::DateTime::from_unix_local(ts)
        .ok()
        .and_then(|d| d.format("%H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

pub struct App {
    pub window: adw::ApplicationWindow,
    split: adw::OverlaySplitView,
    content_bar: adw::ToolbarView,
    list: gtk::ListBox,
    rows: RefCell<Vec<Row>>,
    osd: Rc<Osd>,
    /// Meldungsstreifen oben (Lautstärke, Ton, Qualität, Störungen).
    msg: Rc<MessageBar>,
    pub player: Option<Rc<Player>>,
    lib: RefCell<Library>,
    settings: RefCell<Settings>,
    settings_path: Option<PathBuf>,
    current: Cell<Option<u64>>,
    /// Laufender Stream: URL und ggf. Bitratengrenze (Stufe eines adaptiven Streams).
    current_stream: RefCell<Option<(String, Option<u64>)>>,
    /// Gewünschte Stufe für den laufenden Sender (Ausweichen bei Störungen).
    wanted: Cell<Option<Quality>>,
    /// Für den laufenden Sender schon versuchte URLs.
    tried: RefCell<Vec<String>>,
    digits: RefCell<String>,
    digit_timer: RefCell<Option<glib::SourceId>>,
    sidebar_before_fullscreen: Cell<bool>,
    /// Kennung der Meldung „Leerlauf verhindern“ an GNOME (keine Bildschirmsperre).
    inhibit_cookie: Cell<Option<u32>>,
    guide: RefCell<Option<Rc<Guide>>>,
    epg_timer: Cell<bool>,
}

impl App {
    pub fn new(
        application: &adw::Application,
        lib: Library,
        settings: Settings,
        settings_path: Option<PathBuf>,
        player: Option<Rc<Player>>,
    ) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(application)
            .default_width(1280)
            .default_height(760)
            .title("tvjoe")
            .build();

        let list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar", "tvjoe-list"])
            .selection_mode(gtk::SelectionMode::Single)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        let sidebar_bar = adw::ToolbarView::new();
        sidebar_bar.add_css_class("tvjoe-sidebar");
        sidebar_bar.add_top_bar(
            &adw::HeaderBar::builder()
                .show_end_title_buttons(false)
                .title_widget(&adw::WindowTitle::new("Sender", ""))
                .build(),
        );
        sidebar_bar.set_content(Some(&scroller));

        let osd = Osd::new();
        let msg = MessageBar::new();
        let overlay = gtk::Overlay::new();
        match &player {
            Some(p) => overlay.set_child(Some(&p.area)),
            None => overlay.set_child(Some(&status_page(
                "Wiedergabe nicht möglich",
                "libmpv ließ sich nicht starten.",
            ))),
        }
        overlay.add_overlay(&osd.root);
        overlay.add_overlay(&msg.root);

        let header = adw::HeaderBar::new();
        let sidebar_button = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Senderliste (L)")
            .active(true)
            .build();
        header.pack_start(&sidebar_button);
        let manage_button = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text("Sender verwalten (Strg+E)")
            .build();
        header.pack_start(&manage_button);
        let fullscreen_button = gtk::Button::builder()
            .icon_name("view-fullscreen-symbolic")
            .tooltip_text("Vollbild (F)")
            .build();
        header.pack_end(&fullscreen_button);
        let grid_button = gtk::Button::builder()
            .icon_name("x-office-calendar-symbolic")
            .tooltip_text("Programm (G)")
            .build();
        header.pack_end(&grid_button);
        let prefs_button = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text("Einstellungen (Strg+,)")
            .build();
        header.pack_end(&prefs_button);
        let content_bar = adw::ToolbarView::new();
        content_bar.add_top_bar(&header);
        content_bar.set_content(Some(&overlay));

        let split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar_bar)
            .content(&content_bar)
            .min_sidebar_width(280.0)
            .max_sidebar_width(380.0)
            .build();
        split
            .bind_property("show-sidebar", &sidebar_button, "active")
            .bidirectional()
            .sync_create()
            .build();
        window.set_content(Some(&split));

        let app = Rc::new(Self {
            window,
            split,
            content_bar,
            list,
            rows: RefCell::new(Vec::new()),
            osd,
            msg,
            player,
            lib: RefCell::new(lib),
            settings: RefCell::new(settings),
            settings_path,
            current: Cell::new(None),
            current_stream: RefCell::new(None),
            wanted: Cell::new(None),
            tried: RefCell::new(Vec::new()),
            digits: RefCell::new(String::new()),
            digit_timer: RefCell::new(None),
            sidebar_before_fullscreen: Cell::new(true),
            inhibit_cookie: Cell::new(None),
            guide: RefCell::new(None),
            epg_timer: Cell::new(false),
        });

        app.build_rows();
        app.connect_signals(&fullscreen_button, &overlay);
        let weak = Rc::downgrade(&app);
        prefs_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::prefs::open(&app);
            }
        });
        let weak = Rc::downgrade(&app);
        grid_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::grid::open(&app);
            }
        });
        let weak = Rc::downgrade(&app);
        manage_button.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::manage::open(&app);
            }
        });
        // Alle Signale halten nur schwache Referenzen. Das Fenster hält das Programmobjekt,
        // bis es zerstört wird; sonst wäre es nach den Start-Zeitgebern freigegeben und
        // Knöpfe und Tasten blieben stumm.
        let keep = app.clone();
        app.window.connect_destroy(move |_| {
            let _ = &keep;
        });
        app
    }

    pub fn guide(&self) -> Option<Rc<Guide>> {
        self.guide.borrow().clone()
    }

    pub fn settings(&self) -> std::cell::Ref<'_, Settings> {
        self.settings.borrow()
    }

    pub fn settings_mut(&self) -> std::cell::RefMut<'_, Settings> {
        self.settings.borrow_mut()
    }

    /// Lädt das EPG im Hintergrund (Cache, bei Bedarf aus dem Netz). `force` lädt neu.
    pub fn load_epg(self: &Rc<Self>, force: bool) {
        let (sources, hours) = {
            let s = self.settings.borrow();
            (s.epg_sources.clone(), s.epg_refresh_hours)
        };
        if sources.is_empty() {
            return;
        }
        let max_age = if force {
            Some(Duration::ZERO)
        } else {
            (hours > 0).then(|| Duration::from_secs(u64::from(hours) * 3600))
        };
        let cache = paths::cache_dir();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(epg::load_sources(&sources, cache.as_deref(), max_age));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if let Ok((guide, notes)) = rx.recv().await {
                for n in &notes {
                    trace(&format!("EPG {n}"));
                }
                if let Some(app) = weak.upgrade() {
                    app.set_guide(guide);
                }
            }
        });
    }

    fn set_guide(self: &Rc<Self>, guide: Guide) {
        *self.guide.borrow_mut() = Some(Rc::new(guide));
        self.refresh_epg();
        if !self.epg_timer.replace(true) {
            let weak = Rc::downgrade(self);
            glib::timeout_add_seconds_local(60, move || match weak.upgrade() {
                Some(app) => {
                    app.refresh_epg();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
        }
    }

    /// Aktualisiert die Zeile „laufende Sendung“ in der Senderliste.
    pub fn refresh_epg(&self) {
        let Some(guide) = self.guide() else { return };
        let lib = self.lib.borrow();
        let now = epg::now();
        for r in self.rows.borrow().iter() {
            let (current, next) = lib
                .channel(r.id)
                .and_then(|c| guide.match_channel(c))
                .map(|(id, _)| {
                    let (c, n) = guide.now_next(&id, now);
                    (c.cloned(), n.cloned())
                })
                .unwrap_or((None, None));
            match current {
                Some(p) => {
                    r.now
                        .set_label(&format!("{} · bis {}", p.title, hm(p.stop)));
                    r.now.set_tooltip_text(Some(&p.title));
                    r.now.set_visible(true);
                }
                None => r.now.set_visible(false),
            }
            match next {
                Some(p) => {
                    r.next
                        .set_label(&format!("Danach {}  {}", hm(p.start), p.title));
                    r.next.set_tooltip_text(Some(&p.title));
                    r.next.set_visible(true);
                }
                None => r.next.set_visible(false),
            }
        }
    }

    /// Der gerade laufende Stream eines Senders.
    fn current_stream_of<'a>(&self, ch: &'a Channel) -> Option<&'a Stream> {
        let cur = self.current_stream.borrow();
        ch.streams
            .iter()
            .find(|s| cur.as_ref() == Some(&(s.url.clone(), s.max_bitrate)))
    }

    /// Inhalt der Senderanzeige nach den Einstellungen (abgestimmtes Mockup).
    fn channel_info(
        &self,
        ch: &Channel,
        stream: Option<&Stream>,
        extra: Option<String>,
    ) -> ChannelInfo {
        let st = self.settings.borrow();
        let mut meta: Vec<String> = Vec::new();
        if st.osd_category {
            meta.extend(ch.display_group().map(String::from));
        }
        if st.osd_quality {
            meta.push(quality_label(stream.and_then(|s| s.quality)).to_string());
        }
        let origin = stream.map(|s| match s.kind {
            Some(SourceKind::SatIp) => Origin::Satellite,
            _ => Origin::Internet,
        });
        let logo = ch
            .logo
            .as_deref()
            .filter(|_| st.osd_logo)
            .and_then(|u| paths::logo_cache_dir().map(|d| d.join(net::cache_name(u, "png"))))
            .filter(|p| p.exists());
        let (mut now, mut desc, mut next) = (None, None, None);
        if let Some(guide) = self.guide() {
            if let Some((id, _)) = guide.match_channel(ch) {
                let (cur, nxt) = guide.now_next(&id, epg::now());
                if st.osd_epg {
                    now = cur.map(|p| format!("Jetzt {}–{}  {}", hm(p.start), hm(p.stop), p.title));
                    next = nxt.map(|p| format!("Danach {}  {}", hm(p.start), p.title));
                }
                if st.osd_description {
                    // Absätze zu einem Text zusammenziehen: die Anzeige kürzt dann auf
                    // insgesamt zwei Zeilen (die Zeilengrenze von GTK gilt je Absatz).
                    desc = cur
                        .and_then(|p| p.desc.as_deref().or(p.subtitle.as_deref()))
                        .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "));
                }
            }
        }
        ChannelInfo {
            number: if st.osd_number {
                number_text(ch)
            } else {
                String::new()
            },
            origin: origin.filter(|_| st.osd_origin),
            logo,
            name: ch.display_name().to_string(),
            meta: meta.join(" · "),
            now,
            desc,
            next,
            extra,
            full: true,
        }
    }

    /// Nach geänderter Qualitätseinstellung: laufenden Sender neu wählen, falls sich der
    /// passende Stream geändert hat.
    pub fn reapply_quality(self: &Rc<Self>) {
        let Some(id) = self.current.get() else { return };
        let target = {
            let lib = self.lib.borrow();
            let st = self.settings.borrow();
            let wanted = match st.strategy {
                Strategy::Keep => st.quality,
                Strategy::Highest => None,
            };
            lib.channel(id)
                .and_then(|ch| ch.pick_stream_capped(wanted, st.fallback, st.quality_cap))
                .map(|s| (s.url.clone(), s.max_bitrate))
        };
        if target.is_some() && target != *self.current_stream.borrow() {
            self.play_channel(id);
        }
    }

    /// Ziehen und Ablegen: Sender verschieben, Nummern nach Position neu vergeben.
    pub fn move_channel_relative(self: &Rc<Self>, id: u64, target: u64, after: bool) {
        let moved = {
            let mut lib = self.lib.borrow_mut();
            let moved = lib.move_relative(id, target, after);
            if moved {
                lib.renumber();
            }
            moved
        };
        if moved {
            self.library_changed();
            crate::manage::refresh_if_open();
        }
    }

    pub fn library(&self) -> std::cell::Ref<'_, Library> {
        self.lib.borrow()
    }

    pub fn library_mut(&self) -> std::cell::RefMut<'_, Library> {
        self.lib.borrow_mut()
    }

    /// Nach Änderungen an der Bibliothek: speichern und die Senderliste neu aufbauen.
    pub fn library_changed(self: &Rc<Self>) {
        if let Some(p) = tvjoe_core::paths::library_path() {
            if let Err(e) = self.lib.borrow().save(&p) {
                eprintln!("tvjoe: {e}");
            }
        }
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.rows.borrow_mut().clear();
        self.build_rows();
        if let Some(r) = self
            .rows
            .borrow()
            .iter()
            .find(|r| Some(r.id) == self.current.get())
        {
            self.list.select_row(Some(&r.row));
        }
        if let Some(dir) = tvjoe_core::paths::logo_cache_dir() {
            self.load_logos(dir);
        }
        self.refresh_epg();
        if self.current.get().is_none() {
            self.start(None);
        }
    }

    fn build_rows(self: &Rc<Self>) {
        let lib = self.lib.borrow();
        let mut rows = self.rows.borrow_mut();
        for ch in lib.channels.iter().filter(|c| usable(c)) {
            let num = gtk::Label::builder()
                .label(number_text(ch))
                .css_classes(["tvjoe-num"])
                .width_chars(3)
                .xalign(1.0)
                .build();
            let logo = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Contain)
                .can_shrink(true)
                .width_request(56)
                .height_request(36)
                .build();
            let name = gtk::Label::builder()
                .label(ch.display_name())
                .css_classes(["tvjoe-name"])
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            // Unter dem Namen: laufende Sendung, darunter „Danach …“ (aus dem EPG).
            let epg_line = || {
                gtk::Label::builder()
                    .css_classes(["tvjoe-sub"])
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .visible(false)
                    .build()
            };
            let now = epg_line();
            let next = epg_line();
            let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
            text.set_hexpand(true);
            text.set_valign(gtk::Align::Center);
            text.append(&name);
            text.append(&now);
            text.append(&next);
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            hbox.append(&num);
            hbox.append(&crate::logo_box(&logo, 56));
            hbox.append(&text);
            if ch.favorite {
                hbox.append(
                    &gtk::Label::builder()
                        .label("★")
                        .css_classes(["tvjoe-fav"])
                        .tooltip_text("Favorit")
                        .build(),
                );
            }
            let row = gtk::ListBoxRow::builder().child(&hbox).build();
            let weak = Rc::downgrade(self);
            crate::dnd::attach(&row, ch.id, move |src, target, after| {
                if let Some(app) = weak.upgrade() {
                    app.move_channel_relative(src, target, after);
                }
            });
            self.list.append(&row);
            rows.push(Row {
                id: ch.id,
                row,
                logo,
                now,
                next,
            });
        }
    }

    fn connect_signals(self: &Rc<Self>, fullscreen_button: &gtk::Button, overlay: &gtk::Overlay) {
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            let Some(app) = weak.upgrade() else { return };
            let id = app
                .rows
                .borrow()
                .iter()
                .find(|r| &r.row == row)
                .map(|r| r.id);
            if let Some(id) = id {
                app.play_channel(id);
            }
        });

        let w = self.window.clone();
        fullscreen_button.connect_clicked(move |_| w.set_fullscreened(!w.is_fullscreen()));

        let weak = Rc::downgrade(self);
        self.window.connect_fullscreened_notify(move |w| {
            let Some(app) = weak.upgrade() else { return };
            if w.is_fullscreen() {
                app.sidebar_before_fullscreen.set(app.split.shows_sidebar());
                app.split.set_show_sidebar(false);
                app.content_bar.set_reveal_top_bars(false);
            } else {
                app.split
                    .set_show_sidebar(app.sidebar_before_fullscreen.get());
                app.content_bar.set_reveal_top_bars(true);
            }
            app.update_inhibit();
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| match weak.upgrade() {
            Some(app) => app.on_key(key, state),
            None => glib::Propagation::Proceed,
        });
        self.window.add_controller(keys);

        let click = gtk::GestureClick::new();
        let w = self.window.clone();
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |_, n, _, _| match n {
            1 => {
                if let Some(app) = weak.upgrade() {
                    app.show_info();
                }
            }
            2 => w.set_fullscreened(!w.is_fullscreen()),
            _ => {}
        });
        overlay.add_controller(click);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let weak = Rc::downgrade(self);
        scroll.connect_scroll(move |_, _, dy| {
            if let Some(app) = weak.upgrade() {
                app.change_volume(if dy < 0.0 { 5 } else { -5 });
            }
            glib::Propagation::Stop
        });
        overlay.add_controller(scroll);

        if let Some(player) = &self.player {
            let events = player.spawn_events();
            let weak = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                while let Ok(ev) = events.recv().await {
                    let Some(app) = weak.upgrade() else { break };
                    match ev {
                        PlayerEvent::Failed | PlayerEvent::Ended => app.on_stream_failed(ev),
                        PlayerEvent::Loaded => trace("Stream geöffnet"),
                    }
                }
            });
        }

        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(app) = weak.upgrade() {
                app.save_settings();
            }
            glib::Propagation::Proceed
        });
    }

    /// Verbindet den Logo-Thread mit den Zeilen und fordert alle Logos an (nach dem Start).
    pub fn load_logos(self: &Rc<Self>, cache_dir: PathBuf) {
        let (logos, results) = Logos::spawn(cache_dir);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok((id, path)) = results.recv().await {
                let Some(app) = weak.upgrade() else { break };
                app.set_logo(id, &path);
            }
        });
        let lib = self.lib.borrow();
        for r in self.rows.borrow().iter() {
            if let Some(url) = lib.channel(r.id).and_then(|c| c.logo.as_deref()) {
                logos.request(r.id, url);
            }
        }
        // Der Thread endet, wenn der Sender fallen gelassen wird; Anfragen sind schon gestellt.
        drop(logos);
    }

    fn set_logo(&self, id: u64, path: &Path) {
        if let Some(r) = self.rows.borrow().iter().find(|r| r.id == id) {
            r.logo.set_filename(Some(path));
        }
    }

    pub fn save_settings(&self) {
        if let Some(p) = &self.settings_path {
            if let Err(e) = self.settings.borrow().save(p) {
                eprintln!("tvjoe: {e}");
            }
        }
    }

    fn wanted_for_switch(&self) -> Option<Quality> {
        let s = self.settings.borrow();
        match s.strategy {
            Strategy::Keep => s.quality,
            Strategy::Highest => None,
        }
    }

    fn overlay_secs(&self) -> u32 {
        self.settings.borrow().overlay_secs
    }

    /// Startet den ersten Sender: Kanalnummer aus der Kommandozeile, sonst der letzte, sonst der erste.
    pub fn start(self: &Rc<Self>, number: Option<u32>) {
        let id = {
            let lib = self.lib.borrow();
            let s = self.settings.borrow();
            number
                .and_then(|n| lib.by_number(n))
                .or_else(|| {
                    s.resume_last
                        .then_some(s.last_channel)
                        .flatten()
                        .and_then(|id| lib.channel(id))
                })
                .filter(|c| usable(c))
                .or_else(|| lib.channels.iter().find(|c| usable(c)))
                .map(|c| c.id)
        };
        match id {
            Some(id) => self.play_channel(id),
            None => {
                self.osd.show(
                    "",
                    "Keine Sender",
                    "Sender aus einer M3U übernehmen: tvjoe --import liste.m3u --mit-warnungen",
                    0,
                );
            }
        }
    }

    pub fn play_channel(self: &Rc<Self>, id: u64) {
        let Some(player) = self.player.clone() else {
            return;
        };
        let wanted = self.wanted_for_switch();
        let picked = {
            let lib = self.lib.borrow();
            let (fallback, cap) = {
                let st = self.settings.borrow();
                (st.fallback, st.quality_cap)
            };
            lib.channel(id).and_then(|ch| {
                ch.pick_stream_capped(wanted, fallback, cap).map(|s| {
                    (
                        s.url.clone(),
                        s.max_bitrate,
                        ch.display_name().to_string(),
                        self.channel_info(ch, Some(s), None),
                    )
                })
            })
        };
        let Some((url, bitrate, name, info)) = picked else {
            return;
        };
        player.load(&url, bitrate);
        trace(&format!("lade {name} ({url}, hls-bitrate {bitrate:?})"));
        self.tried.replace(vec![url.clone()]);
        self.current_stream.replace(Some((url, bitrate)));
        self.current.set(Some(id));
        self.wanted.set(wanted);
        self.settings.borrow_mut().last_channel = Some(id);
        self.window.set_title(Some(&format!("{name} – tvjoe")));
        if let Some(r) = self.rows.borrow().iter().find(|r| r.id == id) {
            self.list.select_row(Some(&r.row));
        }
        self.osd.show_channel(&info, self.overlay_secs());
        self.update_inhibit();
    }

    /// Bildschirmsperre verhindern: im Vollbild immer, im Fenster nur mit der Einstellung
    /// „Bildschirmsperre auch im Fenster verhindern“, solange ein Sender läuft.
    pub fn update_inhibit(&self) {
        let want = self.window.is_fullscreen()
            || (self.settings.borrow().inhibit_windowed && self.current.get().is_some());
        let Some(application) = self.window.application() else {
            return;
        };
        match (want, self.inhibit_cookie.get()) {
            (true, None) => {
                let cookie = application.inhibit(
                    Some(&self.window),
                    gtk::ApplicationInhibitFlags::IDLE,
                    Some("Fernsehen läuft"),
                );
                self.inhibit_cookie.set((cookie != 0).then_some(cookie));
                trace(&format!("Bildschirmsperre verhindert (Kennung {cookie})"));
            }
            (false, Some(cookie)) => {
                application.uninhibit(cookie);
                self.inhibit_cookie.set(None);
                trace("Bildschirmsperre wieder erlaubt");
            }
            _ => {}
        }
    }

    /// Stream gestört: nächsten noch nicht versuchten Stream des Senders nehmen.
    fn on_stream_failed(self: &Rc<Self>, ev: PlayerEvent) {
        let (Some(player), Some(id)) = (self.player.clone(), self.current.get()) else {
            return;
        };
        trace(&format!("Stream gestört: {ev:?}"));
        let found = {
            let lib = self.lib.borrow();
            let (fallback, cap) = {
                let st = self.settings.borrow();
                (st.fallback, st.quality_cap)
            };
            let tried = self.tried.borrow();
            lib.channel(id).map(|ch| {
                let next = ch
                    .streams_ordered(self.wanted.get(), fallback, cap)
                    .into_iter()
                    .find(|s| !tried.contains(&s.url))
                    .map(|s| (s.url.clone(), s.max_bitrate, s.quality));
                let text = match &next {
                    Some((_, _, q)) => {
                        format!("Stream gestört – weiche aus auf {}", quality_label(*q))
                    }
                    None => "Sender nicht erreichbar – alle Streams versucht".to_string(),
                };
                let stream = next.as_ref().and_then(|(u, b, _)| {
                    ch.streams
                        .iter()
                        .find(|s| &s.url == u && s.max_bitrate == *b)
                });
                (self.channel_info(ch, stream, None), text, next)
            })
        };
        let Some((info, text, next)) = found else {
            return;
        };
        match next {
            Some((url, bitrate, _)) => {
                player.load(&url, bitrate);
                self.tried.borrow_mut().push(url.clone());
                self.current_stream.replace(Some((url, bitrate)));
                self.osd.show_channel(&info, self.overlay_secs());
                self.msg.show(&text, self.overlay_secs());
            }
            None => {
                self.osd.show_channel(&info, 0);
                self.msg.show(&text, 0);
            }
        }
    }

    pub fn step(self: &Rc<Self>, delta: i64) {
        let next = {
            let lib = self.lib.borrow();
            let ids: Vec<u64> = lib
                .channels
                .iter()
                .filter(|c| usable(c))
                .map(|c| c.id)
                .collect();
            if ids.is_empty() {
                return;
            }
            let pos = self
                .current
                .get()
                .and_then(|id| ids.iter().position(|&x| x == id));
            let idx = match pos {
                Some(p) => (p as i64 + delta).rem_euclid(ids.len() as i64) as usize,
                None => 0,
            };
            ids[idx]
        };
        self.play_channel(next);
    }

    pub fn digit(self: &Rc<Self>, d: u32) {
        let typed = {
            let mut b = self.digits.borrow_mut();
            if b.len() >= 4 {
                b.clear();
            }
            b.extend(char::from_digit(d, 10));
            b.clone()
        };
        // Vorschau des Zielsenders schon in voller Bauchbinde (alle Daten liegen vor);
        // nach der Pause wird umgeschaltet, die Bauchbinde bleibt gleich.
        let preview = {
            let lib = self.lib.borrow();
            let (wanted, fallback, cap) = {
                let st = self.settings.borrow();
                let wanted = match st.strategy {
                    Strategy::Keep => st.quality,
                    Strategy::Highest => None,
                };
                (wanted, st.fallback, st.quality_cap)
            };
            typed
                .parse::<u32>()
                .ok()
                .and_then(|n| lib.by_number(n))
                .filter(|c| usable(c))
                .map(|ch| {
                    let stream = ch.pick_stream_capped(wanted, fallback, cap);
                    self.channel_info(ch, stream, None)
                })
        };
        match preview {
            Some(info) => self.osd.show_channel(&info, 0),
            None => self.osd.show(&typed, "…", "", 0),
        }
        if let Some(id) = self.digit_timer.take() {
            id.remove();
        }
        let pause = self.settings.borrow().numpad_pause_ms;
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_millis(u64::from(pause)), move || {
            if let Some(app) = weak.upgrade() {
                app.digit_timer.take();
                app.commit_digits();
            }
        });
        *self.digit_timer.borrow_mut() = Some(id);
    }

    fn commit_digits(self: &Rc<Self>) {
        if let Some(id) = self.digit_timer.take() {
            id.remove();
        }
        let typed = self.digits.take();
        let Ok(n) = typed.parse::<u32>() else { return };
        let target = self
            .lib
            .borrow()
            .by_number(n)
            .filter(|c| usable(c))
            .map(|c| c.id);
        match target {
            Some(id) => self.play_channel(id),
            None => {
                self.osd.hide();
                self.msg
                    .show("Kein Sender mit dieser Nummer", self.overlay_secs());
            }
        }
    }

    pub fn cycle_quality(self: &Rc<Self>) {
        let (Some(player), Some(id)) = (self.player.clone(), self.current.get()) else {
            return;
        };
        let current_q = {
            let lib = self.lib.borrow();
            let cur = self.current_stream.borrow();
            lib.channel(id)
                .and_then(|ch| {
                    ch.streams
                        .iter()
                        .find(|s| cur.as_ref() == Some(&(s.url.clone(), s.max_bitrate)))
                })
                .and_then(|s| s.quality)
        };
        let next = match self.settings.borrow().quality.or(current_q) {
            None | Some(Quality::Uhd) => Quality::Sd,
            Some(Quality::Sd) => Quality::P720,
            Some(Quality::P720) => Quality::P1080,
            Some(Quality::P1080) => Quality::Uhd,
        };
        self.settings.borrow_mut().quality = Some(next);
        self.wanted.set(Some(next));
        let picked = {
            let lib = self.lib.borrow();
            let (fallback, cap) = {
                let st = self.settings.borrow();
                (st.fallback, st.quality_cap)
            };
            lib.channel(id).and_then(|ch| {
                ch.pick_stream_capped(Some(next), fallback, cap).map(|s| {
                    let text = if s.quality == Some(next) {
                        format!("Qualität {}", next.label())
                    } else {
                        format!(
                            "Qualität {} nicht vorhanden – läuft {}",
                            next.label(),
                            quality_label(s.quality)
                        )
                    };
                    (
                        s.url.clone(),
                        s.max_bitrate,
                        self.channel_info(ch, Some(s), None),
                        text,
                    )
                })
            })
        };
        let Some((url, bitrate, info, text)) = picked else {
            return;
        };
        let target = (url.clone(), bitrate);
        if self.current_stream.borrow().as_ref() != Some(&target) {
            player.load(&url, bitrate);
            self.tried.replace(vec![url]);
            self.current_stream.replace(Some(target));
        }
        self.osd.show_channel(&info, self.overlay_secs());
        self.msg.show(&text, self.overlay_secs());
    }

    pub fn change_volume(&self, delta: i64) {
        let Some(v) = self.player.as_ref().and_then(|p| p.add_volume(delta)) else {
            return;
        };
        self.settings.borrow_mut().volume = v.clamp(0, 130) as u32;
        self.msg.show(&format!("Lautstärke {v} %"), 2);
    }

    fn toggle_mute(&self) {
        if let Some(muted) = self.player.as_ref().and_then(|p| p.toggle_mute()) {
            self.msg.show(if muted { "Ton aus" } else { "Ton an" }, 2);
        }
    }

    pub fn toggle_sidebar(&self) {
        self.split.set_show_sidebar(!self.split.shows_sidebar());
    }

    /// Senderanzeige erneut zeigen (Taste I, Mausklick aufs Bild).
    pub fn show_info(&self) {
        let Some(id) = self.current.get() else { return };
        let lib = self.lib.borrow();
        let Some(ch) = lib.channel(id) else { return };
        let info = self.channel_info(ch, self.current_stream_of(ch), None);
        self.osd.show_channel(&info, self.overlay_secs() * 2);
    }

    fn on_key(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        use gdk::Key;
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        if let Some(d) = key.to_unicode().and_then(|c| c.to_digit(10)) {
            if !ctrl {
                self.digit(d);
                return glib::Propagation::Stop;
            }
        }
        match key.to_lower() {
            Key::Return | Key::KP_Enter if !self.digits.borrow().is_empty() => self.commit_digits(),
            Key::Left | Key::KP_Left | Key::Page_Up => self.step(-1),
            Key::Right | Key::KP_Right | Key::Page_Down => self.step(1),
            Key::Down | Key::KP_Down | Key::KP_Subtract | Key::minus => self.change_volume(-5),
            Key::Up | Key::KP_Up | Key::KP_Add | Key::plus => self.change_volume(5),
            Key::q if ctrl => self.window.close(),
            Key::e if ctrl => crate::manage::open(self),
            Key::comma if ctrl => crate::prefs::open(self),
            Key::q => self.cycle_quality(),
            Key::f => self.window.set_fullscreened(!self.window.is_fullscreen()),
            Key::Escape => self.window.set_fullscreened(false),
            Key::m => self.toggle_mute(),
            Key::l => self.toggle_sidebar(),
            Key::i => self.show_info(),
            Key::g => crate::grid::open(self),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }
}

pub fn status_page(title: &str, text: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("video-display-symbolic")
        .title(title)
        .description(text)
        .build()
}
