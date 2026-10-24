//! Oberfläche von tvjoe (wird vom Starter `tvjoe` aufgerufen).

mod gl;
mod player;

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use adw::prelude::*;
use gtk::{gdk, glib};
use tvjoe_core::{Channel, Library, Settings, paths};

const APP_ID: &str = "io.github.joeMJ.tvjoe";

struct TraceClock {
    start: Instant,
    /// Zeit, die vor dem Start dieses Prozesses im Starter verging.
    offset_ms: f64,
}

static TRACE: OnceLock<Option<TraceClock>> = OnceLock::new();

fn init_trace() {
    TRACE.get_or_init(|| {
        std::env::var_os("TVJOE_TRACE")?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let offset_ms = std::env::var("TVJOE_T0")
            .ok()
            .and_then(|t| t.parse::<u128>().ok())
            .map(|t0| now.saturating_sub(t0) as f64 / 1e6)
            .unwrap_or(0.0);
        Some(TraceClock {
            start: Instant::now(),
            offset_ms,
        })
    });
}

/// Zeitmarke seit dem Start des Starters (nur mit `TVJOE_TRACE=1`).
pub fn trace(msg: &str) {
    if let Some(Some(c)) = TRACE.get() {
        let ms = c.offset_ms + c.start.elapsed().as_secs_f64() * 1e3;
        eprintln!("[tvjoe {ms:8.1} ms] {msg}");
    }
}

#[derive(Default, Clone)]
struct Options {
    mute: bool,
    quit_after: Option<u32>,
    channel: Option<u32>,
}

fn parse_options() -> Options {
    let mut o = Options::default();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--mute" => o.mute = true,
            "--quit-after" => o.quit_after = it.next().and_then(|v| v.parse().ok()),
            "--channel" => o.channel = it.next().and_then(|v| v.parse().ok()),
            _ => eprintln!("tvjoe: unbekannte Option {a}"),
        }
    }
    o
}

fn choose_channel<'a>(
    lib: &'a Library,
    settings: &Settings,
    opts: &Options,
) -> Option<&'a Channel> {
    let usable = |c: &&Channel| c.active && !c.missing && !c.streams.is_empty();
    opts.channel
        .and_then(|n| lib.by_number(n))
        .or_else(|| {
            settings
                .resume_last
                .then_some(settings.last_channel)
                .flatten()
                .and_then(|id| lib.channel(id))
        })
        .filter(usable)
        .or_else(|| lib.channels.iter().find(usable))
}

fn build_window(app: &adw::Application, opts: &Options) {
    trace("GTK bereit");
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    let settings_path = paths::settings_path();
    let mut settings = settings_path
        .as_deref()
        .and_then(|p| Settings::load(p).map_err(|e| eprintln!("tvjoe: {e}")).ok())
        .unwrap_or_default();
    let lib = paths::library_path()
        .as_deref()
        .and_then(|p| Library::load(p).map_err(|e| eprintln!("tvjoe: {e}")).ok())
        .unwrap_or_default();
    trace("Bibliothek geladen");

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(1280)
        .default_height(720)
        .title("tvjoe")
        .build();

    let channel = choose_channel(&lib, &settings, opts);
    let player = match channel {
        Some(ch) => match player::Player::new(opts.mute) {
            Ok(p) => {
                window.set_title(Some(&format!("{} – tvjoe", ch.display_name())));
                window.set_content(Some(&p.area));
                if let Some(stream) = ch.pick_stream(settings.quality, settings.fallback) {
                    p.load(&stream.url);
                    trace(&format!("lade {} ({})", ch.display_name(), stream.url));
                }
                settings.last_channel = Some(ch.id);
                Some(p)
            }
            Err(e) => {
                eprintln!("tvjoe: libmpv ließ sich nicht starten: {e:?}");
                window.set_content(Some(&status_page(
                    "Wiedergabe nicht möglich",
                    &format!("libmpv ließ sich nicht starten: {e:?}"),
                )));
                None
            }
        },
        None => {
            window.set_content(Some(&status_page(
                "Keine Sender",
                "Sender aus einer M3U übernehmen: tvjoe --import liste.m3u --mit-warnungen",
            )));
            None
        }
    };

    let keys = gtk::EventControllerKey::new();
    let w = window.clone();
    keys.connect_key_pressed(move |_, key, _, state| {
        match key {
            gdk::Key::f | gdk::Key::F => w.set_fullscreened(!w.is_fullscreen()),
            gdk::Key::Escape => w.set_fullscreened(false),
            gdk::Key::q if state.contains(gdk::ModifierType::CONTROL_MASK) => w.close(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);

    window.connect_map(|_| trace("Fenster angezeigt"));
    window.connect_close_request(move |_| {
        if let Some(p) = settings_path.as_deref() {
            if let Err(e) = settings.save(p) {
                eprintln!("tvjoe: {e}");
            }
        }
        glib::Propagation::Proceed
    });

    if let Some(p) = player.clone() {
        glib::timeout_add_seconds_local_once(8, move || trace(&p.report()));
    }
    if let Some(secs) = opts.quit_after {
        let w = window.clone();
        glib::timeout_add_seconds_local_once(secs, move || {
            if let Some(p) = &player {
                trace(&p.report());
            }
            w.close();
        });
    }
    window.present();
}

fn status_page(title: &str, text: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("video-display-symbolic")
        .title(title)
        .description(text)
        .build()
}

fn main() -> glib::ExitCode {
    init_trace();
    trace("Oberfläche gestartet");
    let opts = parse_options();
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| build_window(app, &opts));
    app.run_with_args(&["tvjoe"])
}
