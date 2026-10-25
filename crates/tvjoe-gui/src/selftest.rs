//! Selbstprobe nach dem Handproben-Muster des Designstandards: fährt die Bedienschritte
//! nacheinander ab (nie überlappend) und legt nach jedem Schritt ein Bild des betroffenen Fensters ab.
//!
//! - `TVJOE_SELFTEST=/ordner tvjoe --mute`: Bedienung des Hauptfensters.
//! - zusätzlich `TVJOE_SELFTEST_IMPORT=/liste.m3u`: Verwaltung und Import mit Vorschau.
//! - zusätzlich `TVJOE_SELFTEST_MEDIATHEK=1`: Mediathek (lädt beim ersten Mal die Filmliste).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{glib, graphene};

use crate::app::App;
use crate::import::Preview;
use crate::manage::Manager;
use crate::trace;

/// Zustand der Probe: Hauptfenster, ggf. Verwaltung und Vorschau.
struct Probe {
    app: Rc<App>,
    mgr: RefCell<Option<Rc<Manager>>>,
    preview: RefCell<Option<Rc<Preview>>>,
    list: Option<PathBuf>,
}

/// Name des Bildes, Aktion (liefert das zu fotografierende Fenster), Wartezeit bis zum Bild.
type Step = (&'static str, fn(&Probe) -> Option<gtk::Window>, u64);

fn main_window(p: &Probe) -> Option<gtk::Window> {
    Some(p.app.window.clone().upcast())
}

const STEPS: &[Step] = &[
    ("01-start", main_window, 7000),
    (
        "02-naechster-sender",
        |p| {
            p.app.step(1);
            main_window(p)
        },
        5000,
    ),
    (
        "03-ziffer-eingabe",
        |p| {
            p.app.digit(1);
            main_window(p)
        },
        500,
    ),
    ("04-ziffer-gewaehlt", main_window, 3500),
    (
        "05-qualitaet",
        |p| {
            p.app.cycle_quality();
            main_window(p)
        },
        1500,
    ),
    (
        "06-info",
        |p| {
            p.app.show_info();
            main_window(p)
        },
        1500,
    ),
    (
        "07-lautstaerke",
        |p| {
            p.app.change_volume(-5);
            main_window(p)
        },
        800,
    ),
    (
        "08-ohne-liste",
        |p| {
            p.app.toggle_sidebar();
            main_window(p)
        },
        1500,
    ),
    (
        "09-vollbild",
        |p| {
            p.app.window.set_fullscreened(true);
            main_window(p)
        },
        3000,
    ),
    (
        "10-fenster",
        |p| {
            p.app.window.set_fullscreened(false);
            p.app.toggle_sidebar();
            main_window(p)
        },
        1500,
    ),
    ("11-programm", |p| crate::grid::open_window(&p.app), 2000),
    (
        "12-hauptfenster-epg",
        |p| {
            p.app.show_info();
            main_window(p)
        },
        1200,
    ),
    (
        "13-einstellungen",
        |p| {
            crate::prefs::open(&p.app);
            main_window(p)
        },
        1500,
    ),
];

/// Ebene (`TVJOE_SELFTEST_EBENE=1`, nur diese Schritte): Bild, Programm als Ebene (Bild nicht
/// gezeichnet, Ton läuft), Ebene wieder zu. Für Lastmessung von außen je 15 s.
const EBENE_STEPS: &[Step] = &[
    ("40-ebene-vorher", main_window, 15000),
    (
        "41-programm-ebene",
        |p| crate::grid::open_window(&p.app),
        15000,
    ),
    (
        "42-ebene-zu",
        |p| {
            crate::layer::close_layer(&p.app);
            main_window(p)
        },
        15000,
    ),
];

/// Bildbereich auf etwa `w`×`h` bringen (Kopfleiste des Hauptfensters kommt dazu).
fn resize(p: &Probe, w: i32, h: i32) {
    p.app.window.set_default_size(w, h + 47);
    let (min_w, _, _, _) = p.app.window.measure(gtk::Orientation::Horizontal, -1);
    let (min_h, _, _, _) = p.app.window.measure(gtk::Orientation::Vertical, -1);
    trace(&format!(
        "Größe {w}×{h} angefordert; Mindestgröße Hauptfenster {min_w}×{min_h}"
    ));
}

fn report_size(p: &Probe) -> Option<gtk::Window> {
    let (w, h) = p.app.video_area_size();
    trace(&format!("Bildbereich jetzt {w}×{h}"));
    main_window(p)
}

fn close_all(p: &Probe) {
    crate::layer::close_layer(&p.app);
    for w in gtk::Window::list_toplevels() {
        if let Ok(w) = w.downcast::<gtk::Window>() {
            if w != *p.app.window.upcast_ref::<gtk::Window>() {
                w.close();
            }
        }
    }
}

/// Mindestgrößen (`TVJOE_SELFTEST_GROESSE=1`, nur diese Schritte): jede Ansicht knapp über
/// ihrer Mindestgröße als Ebene, die Mediathek knapp darunter als Fenster, die Senderliste
/// bei sehr kleinem Fenster als eigenes Fenster.
const GROESSE_STEPS: &[Step] = &[
    (
        "50-start-920x600",
        |p| {
            resize(p, 920, 600);
            main_window(p)
        },
        6000,
    ),
    (
        "51-mediathek-ebene-920x600",
        |p| crate::mediathek::open_window(&p.app),
        5000,
    ),
    (
        "52-programm-ebene-920x600",
        |p| {
            close_all(p);
            crate::grid::open_window(&p.app)
        },
        3000,
    ),
    (
        "53-groesse-1020x580",
        |p| {
            close_all(p);
            resize(p, 1020, 580);
            main_window(p)
        },
        1500,
    ),
    (
        "54-verwaltung-ebene-1020x580",
        |p| {
            report_size(p);
            let m = crate::manage::open_manager(&p.app)?;
            crate::layer::window_of(&p.app, &(m as Rc<dyn crate::layer::View>))
        },
        4000,
    ),
    (
        "55-groesse-820x520",
        |p| {
            close_all(p);
            resize(p, 820, 520);
            main_window(p)
        },
        3000,
    ),
    (
        "56-mediathek-als-fenster-820x520",
        |p| {
            report_size(p);
            crate::mediathek::open_window(&p.app)
        },
        5000,
    ),
    (
        "57-groesse-560x340",
        |p| {
            close_all(p);
            resize(p, 560, 340);
            main_window(p)
        },
        3000,
    ),
    (
        "58-senderliste-als-fenster-560x340",
        |p| {
            report_size(p);
            p.app.toggle_sidebar();
            gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Window>().ok())
                .find(|w| w.title().is_some_and(|t| t.starts_with("Sender –")))
        },
        3000,
    ),
];

/// Mediathek (`TVJOE_SELFTEST_MEDIATHEK=1`): beim ersten Öffnen wird die Filmliste geladen.
const MEDIATHEK_STEPS: &[Step] = &[
    (
        "30-mediathek-oeffnen",
        |p| crate::mediathek::open_window(&p.app),
        2000,
    ),
    (
        "31-mediathek-episoden",
        |_| crate::mediathek::probe_tab(0, None),
        90000,
    ),
    (
        "32-mediathek-filme",
        |_| crate::mediathek::probe_tab(1, None),
        15000,
    ),
    (
        "33-mediathek-suche",
        |_| crate::mediathek::probe_tab(2, Some("tatort")),
        15000,
    ),
    (
        "34-mediathek-abspielen",
        |p| {
            crate::mediathek::probe_play_first(&p.app);
            main_window(p)
        },
        6000,
    ),
];

const IMPORT_STEPS: &[Step] = &[
    (
        "20-verwaltung",
        |p| {
            let m = crate::manage::open_manager(&p.app)?;
            let w = crate::layer::window_of(&p.app, &(m.clone() as Rc<dyn crate::layer::View>))?;
            *p.mgr.borrow_mut() = Some(m);
            Some(w)
        },
        2000,
    ),
    (
        "21-vorschau-pruefung",
        |p| {
            let m = p.mgr.borrow().clone()?;
            let pr = crate::import::preview_path(&m, p.list.as_deref()?)?;
            let w = crate::layer::window_of(&p.app, &(pr.clone() as Rc<dyn crate::layer::View>))?;
            *p.preview.borrow_mut() = Some(pr);
            Some(w)
        },
        15000,
    ),
    (
        "22-auswahl-gruppen",
        |p| {
            let pr = p.preview.borrow().clone()?;
            pr.set_where(|_| true, false);
            pr.set_where(
                |e| crate::import::groups_of(e).any(|g| g == "Vollprogramm" || g == "Regional"),
                true,
            );
            crate::layer::window_of(&p.app, &(pr as Rc<dyn crate::layer::View>))
        },
        1000,
    ),
    (
        "23-verwaltung-nach-import",
        |p| {
            let pr = p.preview.borrow_mut().take()?;
            pr.apply();
            let m = p.mgr.borrow().clone()?;
            crate::layer::window_of(&p.app, &(m as Rc<dyn crate::layer::View>))
        },
        3000,
    ),
    (
        "24-hauptfenster-nach-import",
        |p| {
            if let Some(m) = p.mgr.borrow_mut().take() {
                crate::layer::close_body(&p.app, &m.body);
            }
            main_window(p)
        },
        2500,
    ),
];

pub fn run(app: Rc<App>, dir: PathBuf) {
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("tvjoe: Selbstprobe: {e}");
        return;
    }
    let list = std::env::var_os("TVJOE_SELFTEST_IMPORT").map(PathBuf::from);
    let mut steps: Vec<Step> = if std::env::var_os("TVJOE_SELFTEST_EBENE").is_some() {
        EBENE_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_GROESSE").is_some() {
        GROESSE_STEPS.to_vec()
    } else {
        STEPS.to_vec()
    };
    if list.is_some() {
        steps.extend_from_slice(IMPORT_STEPS);
    }
    if std::env::var_os("TVJOE_SELFTEST_MEDIATHEK").is_some() {
        steps.extend_from_slice(MEDIATHEK_STEPS);
    }
    let probe = Rc::new(Probe {
        app,
        mgr: RefCell::new(None),
        preview: RefCell::new(None),
        list,
    });
    next(probe, Rc::new(steps), dir, 0);
}

fn next(probe: Rc<Probe>, steps: Rc<Vec<Step>>, dir: PathBuf, i: usize) {
    let Some((name, action, wait)) = steps.get(i).copied() else {
        trace("Selbstprobe fertig");
        probe.app.window.close();
        return;
    };
    let window = action(&probe);
    glib::timeout_add_local_once(Duration::from_millis(wait), move || {
        let path = dir.join(format!("{name}.png"));
        match window.as_ref().map(|w| shot(w, &path)) {
            Some(Ok(())) => trace(&format!("Bild {}", path.display())),
            Some(Err(e)) => eprintln!("tvjoe: Bild {name}: {e}"),
            None => eprintln!("tvjoe: Schritt {name}: kein Fenster"),
        }
        next(probe, steps, dir, i + 1);
    });
}

fn shot(window: &gtk::Window, path: &Path) -> Result<(), String> {
    let (w, h) = (window.width(), window.height());
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let node = snapshot.to_node().ok_or("leeres Bild")?;
    let renderer = window.renderer().ok_or("kein Renderer")?;
    let texture = renderer.render_texture(
        &node,
        Some(&graphene::Rect::new(0.0, 0.0, w as f32, h as f32)),
    );
    texture.save_to_png(path).map_err(|e| e.to_string())
}
