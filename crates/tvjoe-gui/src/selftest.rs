//! Selbstprobe nach dem Handproben-Muster des Designstandards: fährt die Bedienschritte
//! nacheinander ab (nie überlappend) und legt nach jedem Schritt ein Bild des eigenen Fensters ab.
//! Aufruf: `TVJOE_SELFTEST=/ordner tvjoe --mute`.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{glib, graphene};

use crate::app::App;
use crate::trace;

/// Name des Bildes, Aktion, Wartezeit bis zum Bild.
type Step = (&'static str, fn(&Rc<App>), u64);

const STEPS: &[Step] = &[
    ("01-start", |_| {}, 7000),
    ("02-naechster-sender", |a| a.step(1), 5000),
    ("03-ziffer-eingabe", |a| a.digit(1), 500),
    ("04-ziffer-gewaehlt", |_| {}, 3500),
    ("05-qualitaet", |a| a.cycle_quality(), 4000),
    ("06-info", |a| a.show_info(), 1500),
    ("07-lautstaerke", |a| a.change_volume(-5), 800),
    ("08-ohne-liste", |a| a.toggle_sidebar(), 1500),
    ("09-vollbild", |a| a.window.set_fullscreened(true), 3000),
];

pub fn run(app: Rc<App>, dir: PathBuf) {
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("tvjoe: Selbstprobe: {e}");
        return;
    }
    next(app, dir, 0);
}

fn next(app: Rc<App>, dir: PathBuf, i: usize) {
    let Some((name, action, wait)) = STEPS.get(i).copied() else {
        app.window.set_fullscreened(false);
        trace("Selbstprobe fertig");
        app.window.close();
        return;
    };
    action(&app);
    glib::timeout_add_local_once(Duration::from_millis(wait), move || {
        let path = dir.join(format!("{name}.png"));
        match shot(&app, &path) {
            Ok(()) => trace(&format!("Bild {}", path.display())),
            Err(e) => eprintln!("tvjoe: Bild {name}: {e}"),
        }
        next(app, dir, i + 1);
    });
}

fn shot(app: &App, path: &Path) -> Result<(), String> {
    let (w, h) = (app.window.width(), app.window.height());
    let paintable = gtk::WidgetPaintable::new(Some(&app.window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let node = snapshot.to_node().ok_or("leeres Bild")?;
    let renderer = app.window.renderer().ok_or("kein Renderer")?;
    let texture = renderer.render_texture(
        &node,
        Some(&graphene::Rect::new(0.0, 0.0, w as f32, h as f32)),
    );
    texture.save_to_png(path).map_err(|e| e.to_string())
}
