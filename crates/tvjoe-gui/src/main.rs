//! Oberfläche von tvjoe (wird vom Starter `tvjoe` aufgerufen). Unter Windows ein Fensterprogramm
//! ohne eigenes Konsolenfenster (siehe `tvjoe_core::console`).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod backup;
mod crop;
mod dnd;
mod gl;
mod grid;
mod icons;
mod import;
mod jellyfin;
mod jfview;
mod layer;
mod logos;
mod manage;
mod mediathek;
mod osd;
mod player;
mod prefs;
mod satip;
mod selftest;
mod vod;

use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use adw::prelude::*;
use gtk::{gdk, glib};
use tvjoe_core::{Library, Settings, paths};

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
    /// Ablauf einer Pause protokollieren (Bildnummer, Position, Neuzeichnen).
    protocol: bool,
}

fn parse_options() -> Options {
    let mut o = Options::default();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--mute" => o.mute = true,
            "--protokoll" => o.protocol = true,
            "--quit-after" => o.quit_after = it.next().and_then(|v| v.parse().ok()),
            "--channel" => o.channel = it.next().and_then(|v| v.parse().ok()),
            _ => eprintln!("tvjoe: unbekannte Option {a}"),
        }
    }
    o
}

/// Logo mit fester Höchstgröße, damit breite Logos die Spalten nicht verschieben.
pub fn logo_box(picture: &gtk::Picture, width: i32) -> adw::Clamp {
    picture.set_halign(gtk::Align::Center);
    adw::Clamp::builder()
        .orientation(gtk::Orientation::Horizontal)
        .maximum_size(width)
        .tightening_threshold(width)
        .width_request(width)
        .child(picture)
        .build()
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn activate(application: &adw::Application, opts: &Options) {
    trace("GTK bereit");
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    load_css();
    let settings_path = paths::settings_path();
    let settings = settings_path
        .as_deref()
        .and_then(|p| Settings::load(p).map_err(|e| eprintln!("tvjoe: {e}")).ok())
        .unwrap_or_default();
    let lib = paths::library_path()
        .as_deref()
        .and_then(|p| Library::load(p).map_err(|e| eprintln!("tvjoe: {e}")).ok())
        .unwrap_or_default();
    trace("Bibliothek geladen");

    let player = match player::Player::new(opts.mute, settings.volume) {
        Ok(p) => {
            p.set_deinterlace(settings.deinterlace);
            p.set_subtitles(settings.subtitles);
            p.set_loudness(settings.loudness);
            p.set_view_mode(settings.crop_bars, settings.fill);
            p.set_buffer(settings.buffer_mb, settings.start_buffer_secs);
            p.set_protocol(opts.protocol);
            Some(p)
        }
        Err(e) => {
            eprintln!("tvjoe: libmpv ließ sich nicht starten: {e:?}");
            None
        }
    };
    let app = app::App::new(application, lib, settings, settings_path, player);
    app.start(opts.channel);
    app.window.connect_map(|_| trace("Fenster angezeigt"));
    app.window.present();

    // Logos erst nach dem Start anfordern, damit Fenster und erstes Bild nicht warten.
    if let Some(dir) = paths::logo_cache_dir() {
        let a = app.clone();
        glib::timeout_add_local_once(Duration::from_millis(300), move || a.load_logos(dir));
    }
    // EPG ebenfalls im Hintergrund (Cache, bei Bedarf Netz).
    let a = app.clone();
    glib::timeout_add_local_once(Duration::from_millis(500), move || a.load_epg(false));
    // Filmliste der Mediathek im Hintergrund (nur, wenn die Mediathek schon benutzt wurde).
    let a = app.clone();
    glib::timeout_add_seconds_local_once(60, move || mediathek::start_background(&a));
    // Updateprüfung nur, wenn eingeschaltet (Standard: aus), frühestens nach 10 s.
    let a = app.clone();
    glib::timeout_add_seconds_local_once(10, move || prefs::maybe_check_on_start(&a));
    if let Some(p) = app.player.clone() {
        glib::timeout_add_seconds_local_once(8, move || trace(&p.report()));
    }
    if let Some(dir) = std::env::var_os("TVJOE_SELFTEST") {
        selftest::run(app.clone(), dir.into());
    }
    if let Some(secs) = opts.quit_after {
        let a = app.clone();
        glib::timeout_add_seconds_local_once(secs, move || {
            if let Some(p) = &a.player {
                trace(&p.report());
            }
            a.window.close();
        });
    }
}

fn main() -> glib::ExitCode {
    tvjoe_core::console::attach_parent();
    init_trace();
    trace("Oberfläche gestartet");
    let opts = parse_options();
    let application = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application.connect_activate(move |a| activate(a, &opts));
    application.run_with_args(&["tvjoe"])
}
