//! Hauptfenster: Senderliste links, Bild rechts, Einblendung über dem Bild, Tastensteuerung.
//!
//! Tasten (Projektplan M4): Ziffern = Sender wählen (Pause aus den Einstellungen, Enter sofort),
//! Pfeil hoch/runter bzw. Bild hoch/runter = Senderwechsel, F = Vollbild, Esc = Vollbild aus,
//! M = stumm, Q = Qualität eine Stufe höher (mit Umbruch), L = Senderliste, I = Info,
//! Pfeil links/rechts und Mausrad = Lautstärke, Strg+Q = Beenden.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};
use tvjoe_core::{Channel, Library, Quality, Settings, Strategy};

use crate::logos::Logos;
use crate::osd::Osd;
use crate::player::{Player, PlayerEvent};
use crate::trace;

fn usable(c: &Channel) -> bool {
    c.active && !c.missing && !c.streams.is_empty()
}

fn quality_label(q: Option<Quality>) -> &'static str {
    q.map_or("Qualität unbekannt", Quality::label)
}

/// Zusatzzeile: Gruppe · Qualität.
fn sub_line(ch: &Channel, q: Option<Quality>) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(g) = ch.display_group() {
        parts.push(g);
    }
    parts.push(quality_label(q));
    parts.join(" · ")
}

fn number_text(ch: &Channel) -> String {
    ch.number.map(|n| n.to_string()).unwrap_or_default()
}

struct Row {
    id: u64,
    row: gtk::ListBoxRow,
    logo: gtk::Picture,
}

pub struct App {
    pub window: adw::ApplicationWindow,
    split: adw::OverlaySplitView,
    content_bar: adw::ToolbarView,
    list: gtk::ListBox,
    rows: RefCell<Vec<Row>>,
    osd: Rc<Osd>,
    pub player: Option<Rc<Player>>,
    lib: RefCell<Library>,
    settings: RefCell<Settings>,
    settings_path: Option<PathBuf>,
    current: Cell<Option<u64>>,
    current_url: RefCell<Option<String>>,
    /// Gewünschte Stufe für den laufenden Sender (Ausweichen bei Störungen).
    wanted: Cell<Option<Quality>>,
    /// Für den laufenden Sender schon versuchte URLs.
    tried: RefCell<Vec<String>>,
    digits: RefCell<String>,
    digit_timer: RefCell<Option<glib::SourceId>>,
    sidebar_before_fullscreen: Cell<bool>,
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
        let overlay = gtk::Overlay::new();
        match &player {
            Some(p) => overlay.set_child(Some(&p.area)),
            None => overlay.set_child(Some(&status_page(
                "Wiedergabe nicht möglich",
                "libmpv ließ sich nicht starten.",
            ))),
        }
        overlay.add_overlay(&osd.root);

        let header = adw::HeaderBar::new();
        let sidebar_button = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Senderliste (L)")
            .active(true)
            .build();
        header.pack_start(&sidebar_button);
        let fullscreen_button = gtk::Button::builder()
            .icon_name("view-fullscreen-symbolic")
            .tooltip_text("Vollbild (F)")
            .build();
        header.pack_end(&fullscreen_button);
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
            player,
            lib: RefCell::new(lib),
            settings: RefCell::new(settings),
            settings_path,
            current: Cell::new(None),
            current_url: RefCell::new(None),
            wanted: Cell::new(None),
            tried: RefCell::new(Vec::new()),
            digits: RefCell::new(String::new()),
            digit_timer: RefCell::new(None),
            sidebar_before_fullscreen: Cell::new(true),
        });

        app.build_rows();
        app.connect_signals(&fullscreen_button, &overlay);
        app
    }

    fn build_rows(&self) {
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
            let qualities: Vec<&str> = ch
                .streams
                .iter()
                .filter_map(|s| s.quality.map(Quality::label))
                .collect();
            let mut sub_parts: Vec<String> =
                ch.display_group().map(String::from).into_iter().collect();
            if !qualities.is_empty() {
                sub_parts.push(qualities.join(" "));
            }
            let sub = gtk::Label::builder()
                .label(sub_parts.join(" · "))
                .css_classes(["tvjoe-sub"])
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .visible(!sub_parts.is_empty())
                .build();
            let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
            text.set_hexpand(true);
            text.set_valign(gtk::Align::Center);
            text.append(&name);
            text.append(&sub);
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            hbox.append(&num);
            hbox.append(&logo);
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
            self.list.append(&row);
            rows.push(Row {
                id: ch.id,
                row,
                logo,
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
        click.connect_pressed(move |_, n, _, _| {
            if n == 2 {
                w.set_fullscreened(!w.is_fullscreen());
            }
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
            let fallback = self.settings.borrow().fallback;
            lib.channel(id).and_then(|ch| {
                ch.pick_stream(wanted, fallback).map(|s| {
                    (
                        s.url.clone(),
                        number_text(ch),
                        ch.display_name().to_string(),
                        sub_line(ch, s.quality),
                    )
                })
            })
        };
        let Some((url, number, name, sub)) = picked else {
            return;
        };
        player.load(&url);
        trace(&format!("lade {name} ({url})"));
        self.tried.replace(vec![url.clone()]);
        self.current_url.replace(Some(url));
        self.current.set(Some(id));
        self.wanted.set(wanted);
        self.settings.borrow_mut().last_channel = Some(id);
        self.window.set_title(Some(&format!("{name} – tvjoe")));
        if let Some(r) = self.rows.borrow().iter().find(|r| r.id == id) {
            self.list.select_row(Some(&r.row));
        }
        self.osd.show(&number, &name, &sub, self.overlay_secs());
    }

    /// Stream gestört: nächsten noch nicht versuchten Stream des Senders nehmen.
    fn on_stream_failed(self: &Rc<Self>, ev: PlayerEvent) {
        let (Some(player), Some(id)) = (self.player.clone(), self.current.get()) else {
            return;
        };
        trace(&format!("Stream gestört: {ev:?}"));
        let found = {
            let lib = self.lib.borrow();
            let fallback = self.settings.borrow().fallback;
            let tried = self.tried.borrow();
            lib.channel(id).map(|ch| {
                let next = ch
                    .streams_by_preference(self.wanted.get(), fallback)
                    .into_iter()
                    .find(|s| !tried.contains(&s.url))
                    .map(|s| (s.url.clone(), s.quality));
                (number_text(ch), ch.display_name().to_string(), next)
            })
        };
        let Some((number, name, next)) = found else {
            return;
        };
        match next {
            Some((url, q)) => {
                player.load(&url);
                self.tried.borrow_mut().push(url.clone());
                self.current_url.replace(Some(url));
                let text = format!("Stream gestört – weiche aus auf {}", quality_label(q));
                self.osd.show(&number, &name, &text, self.overlay_secs());
            }
            None => {
                let text = "Sender nicht erreichbar – alle Streams versucht";
                self.osd.show(&number, &name, text, 0);
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
        let preview = typed
            .parse::<u32>()
            .ok()
            .and_then(|n| {
                self.lib
                    .borrow()
                    .by_number(n)
                    .map(|c| c.display_name().to_string())
            })
            .unwrap_or_else(|| "…".into());
        self.osd.show(&typed, &preview, "", 0);
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
            None => self.osd.show(
                &typed,
                "Kein Sender mit dieser Nummer",
                "",
                self.overlay_secs(),
            ),
        }
    }

    pub fn cycle_quality(self: &Rc<Self>) {
        let (Some(player), Some(id)) = (self.player.clone(), self.current.get()) else {
            return;
        };
        let current_q = {
            let lib = self.lib.borrow();
            let url = self.current_url.borrow();
            lib.channel(id)
                .and_then(|ch| ch.streams.iter().find(|s| Some(&s.url) == url.as_ref()))
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
            let fallback = self.settings.borrow().fallback;
            lib.channel(id).and_then(|ch| {
                ch.pick_stream(Some(next), fallback).map(|s| {
                    (
                        s.url.clone(),
                        s.quality,
                        number_text(ch),
                        ch.display_name().to_string(),
                    )
                })
            })
        };
        let Some((url, q, number, name)) = picked else {
            return;
        };
        if self.current_url.borrow().as_deref() != Some(url.as_str()) {
            player.load(&url);
            self.tried.replace(vec![url.clone()]);
            self.current_url.replace(Some(url));
        }
        let text = if q == Some(next) {
            format!("Qualität {}", next.label())
        } else {
            format!(
                "Qualität {} nicht vorhanden – läuft {}",
                next.label(),
                quality_label(q)
            )
        };
        self.osd.show(&number, &name, &text, self.overlay_secs());
    }

    pub fn change_volume(&self, delta: i64) {
        let Some(v) = self.player.as_ref().and_then(|p| p.add_volume(delta)) else {
            return;
        };
        self.settings.borrow_mut().volume = v.clamp(0, 130) as u32;
        self.osd.show("", &format!("Lautstärke {v} %"), "", 2);
    }

    fn toggle_mute(&self) {
        if let Some(muted) = self.player.as_ref().and_then(|p| p.toggle_mute()) {
            self.osd
                .show("", if muted { "Ton aus" } else { "Ton an" }, "", 2);
        }
    }

    pub fn toggle_sidebar(&self) {
        self.split.set_show_sidebar(!self.split.shows_sidebar());
    }

    pub fn show_info(&self) {
        let Some(id) = self.current.get() else { return };
        let lib = self.lib.borrow();
        let Some(ch) = lib.channel(id) else { return };
        let url = self.current_url.borrow();
        let q = ch
            .streams
            .iter()
            .find(|s| Some(&s.url) == url.as_ref())
            .and_then(|s| s.quality);
        let mut sub = sub_line(ch, q);
        if let Some(info) = self.player.as_ref().and_then(|p| p.video_info()) {
            sub.push('\n');
            sub.push_str(&info);
        }
        self.osd.show(
            &number_text(ch),
            ch.display_name(),
            &sub,
            self.overlay_secs() * 2,
        );
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
            Key::Up | Key::KP_Up | Key::Page_Up => self.step(-1),
            Key::Down | Key::KP_Down | Key::Page_Down => self.step(1),
            Key::Left | Key::KP_Left | Key::KP_Subtract | Key::minus => self.change_volume(-5),
            Key::Right | Key::KP_Right | Key::KP_Add | Key::plus => self.change_volume(5),
            Key::q if ctrl => self.window.close(),
            Key::q => self.cycle_quality(),
            Key::f => self.window.set_fullscreened(!self.window.is_fullscreen()),
            Key::Escape => self.window.set_fullscreened(false),
            Key::m => self.toggle_mute(),
            Key::l => self.toggle_sidebar(),
            Key::i => self.show_info(),
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
