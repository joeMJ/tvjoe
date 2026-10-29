//! Selbstprobe nach dem Handproben-Muster des Designstandards: fährt die Bedienschritte
//! nacheinander ab (nie überlappend) und legt nach jedem Schritt ein Bild des betroffenen Fensters ab.
//!
//! - `TVJOE_SELFTEST=/ordner tvjoe --mute`: Bedienung des Hauptfensters.
//! - zusätzlich `TVJOE_SELFTEST_IMPORT=/liste.m3u`: Verwaltung und Import mit Vorschau.
//! - zusätzlich `TVJOE_SELFTEST_MEDIATHEK=1`: Mediathek (lädt beim ersten Mal die Filmliste).
//! - `TVJOE_SELFTEST_BAUCHBINDE=1` (nur diese Schritte): Höhe der Bauchbinde in allen Lagen.
//! - `TVJOE_SELFTEST_MEDIATHEKEN=1` (nur diese Schritte): Reiter „Mediatheken“, Seite Jellyfin.
//! - `TVJOE_SELFTEST_WECHSEL=1` (nur diese Schritte): Quellenwechsel SAT ↔ Internet-TV, Bildzähler.
//! - `TVJOE_SELFTEST_UMZUG=/ziel` (nur diese Schritte, nur mit Testordnern): Ablage verschieben.
//! - `TVJOE_SELFTEST_ABO=1` (nur diese Schritte, nur mit Testordner): Nachladen und Abo.
//! - `TVJOE_SELFTEST_TVH=1` (nur diese Schritte): Tvheadend-Quelle, Kanäle holen, umschalten.
//! - `TVJOE_SELFTEST_UPDATE=1` (nur diese Schritte): Update-Dialog, Aufzeichnung der Bedienung.
//! - `TVJOE_SELFTEST_ABLAGE=1` (nur diese Schritte): Reiter „Ablage“, Gruppe, Prüfen, Seite „Über“.
//! - `TVJOE_SELFTEST_EMPFANG=1` (nur diese Schritte): Reiter „Empfang“ und Seiten der Quellen.
//! - `TVJOE_SELFTEST_EPGPRUEFUNG=1` (nur diese Schritte): Hinweis der EPG-Prüfung im Raster,
//!   wegklicken; mit eigener `TVJOE_CONFIG_DIR`, deren XMLTV-Quelle eine Attrappe ist.
//! - `TVJOE_SELFTEST_ZUGANG=1` (nur diese Schritte, mit `TVJOE_SELFTEST_IMPORT`): Vorauswahl
//!   der Vorschau, Dialog zu Zugangsdaten beim Übernehmen.
//! - `TVJOE_SELFTEST_README=1` (nur diese Schritte): Bilder für das README; mit neutralen Daten
//!   starten (eigene `TVJOE_CONFIG_DIR`/`TVJOE_CACHE_DIR`, ohne Jellyfin und private Sender).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
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

thread_local! {
    /// Offene Einstellungen der Empfangs-Schritte.
    static PREFS: RefCell<Option<adw::PreferencesDialog>> = const { RefCell::new(None) };
}

/// Zeile mit diesem Titel in den offenen Einstellungen auslösen (Unterseite öffnen).
fn activate_row(title: &str) {
    fn find(w: &gtk::Widget, title: &str) -> Option<adw::ActionRow> {
        if let Some(r) = w.downcast_ref::<adw::ActionRow>() {
            if r.title() == title {
                return Some(r.clone());
            }
        }
        let mut c = w.first_child();
        while let Some(child) = c {
            if let Some(r) = find(&child, title) {
                return Some(r);
            }
            c = child.next_sibling();
        }
        None
    }
    PREFS.with(|d| {
        if let Some(d) = d.borrow().as_ref() {
            d.pop_subpage();
            if let Some(r) = find(d.upcast_ref(), title) {
                ActionRowExt::activate(&r);
            }
        }
    });
}

/// Fenster der offenen Einstellungen (eigenes Fenster oder Hauptfenster).
fn prefs_window(p: &Probe) -> Option<gtk::Window> {
    PREFS
        .with(|d| d.borrow().as_ref().and_then(|d| d.root()))
        .and_then(|r| r.downcast::<gtk::Window>().ok())
        .or_else(|| main_window(p))
}

/// Empfang (`TVJOE_SELFTEST_EMPFANG=1`, nur diese Schritte): Reiter „Empfang“, die Seiten der
/// beiden Empfangsquellen, dann die Einstellungen bei kleinem Hauptfenster (eigenes Fenster).
const EMPFANG_STEPS: &[Step] = &[
    ("80-start", main_window, 6000),
    (
        "81-empfang",
        |p| {
            let d = crate::prefs::open_at(&p.app, Some("empfang"), &p.app.window);
            PREFS.with(|s| *s.borrow_mut() = Some(d));
            main_window(p)
        },
        1500,
    ),
    (
        "82-internet-tv",
        |p| {
            activate_row(tvjoe_core::tr("Internet-TV"));
            main_window(p)
        },
        1500,
    ),
    (
        "83-sat",
        |p| {
            let name = p
                .app
                .settings()
                .adapter(tvjoe_core::settings::ADAPTER_SAT)
                .and_then(|a| tvjoe_core::adapter::find(&a.kind, None))
                .map(|d| d.name)
                .unwrap_or_default();
            activate_row(&name);
            main_window(p)
        },
        1500,
    ),
    (
        "84-klein",
        |p| {
            if let Some(d) = PREFS.with(|d| d.borrow_mut().take()) {
                d.close();
            }
            p.app.window.set_default_size(640, 420);
            main_window(p)
        },
        1500,
    ),
    (
        "85-einstellungen-eigenes-fenster",
        |p| {
            let d = crate::prefs::open_at(&p.app, Some("empfang"), &p.app.window);
            PREFS.with(|s| *s.borrow_mut() = Some(d));
            prefs_window(p)
        },
        2000,
    ),
];

/// Schalterzeile mit diesem Titel in den offenen Einstellungen setzen.
fn set_switch(title: &str, on: bool) {
    fn find(w: &gtk::Widget, title: &str) -> Option<adw::SwitchRow> {
        if let Some(r) = w.downcast_ref::<adw::SwitchRow>() {
            if r.title() == title {
                return Some(r.clone());
            }
        }
        let mut c = w.first_child();
        while let Some(child) = c {
            if let Some(r) = find(&child, title) {
                return Some(r);
            }
            c = child.next_sibling();
        }
        None
    }
    PREFS.with(|d| {
        if let Some(r) = d.borrow().as_ref().and_then(|d| find(d.upcast_ref(), title)) {
            r.set_active(on);
        }
    });
}

/// Knopf mit dieser Beschriftung in den offenen Einstellungen drücken.
fn click_button(label: &str) {
    fn find(w: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if let Some(b) = w.downcast_ref::<gtk::Button>() {
            if b.label().as_deref() == Some(label) && b.is_sensitive() {
                return Some(b.clone());
            }
        }
        let mut c = w.first_child();
        while let Some(child) = c {
            if let Some(b) = find(&child, label) {
                return Some(b);
            }
            c = child.next_sibling();
        }
        None
    }
    PREFS.with(|d| {
        if let Some(b) = d.borrow().as_ref().and_then(|d| find(d.upcast_ref(), label)) {
            b.emit_clicked();
        }
    });
}

/// DVB-Karte (`TVJOE_SELFTEST_DVB=1`, nur diese Schritte; braucht eine Empfangsquelle Nummer 2
/// mit Treiber linux-dvb in den Einstellungen und ein Gerät, z. B. dvb_vidtv): Seite der Quelle,
/// Suchlauf, Umschalten, Wiedergabe, Info, Aufnahme.
const DVB_STEPS: &[Step] = &[
    ("90-start", main_window, 5000),
    (
        "91-dvb-seite",
        |p| {
            let d = crate::prefs::open_at(&p.app, Some("empfang"), &p.app.window);
            PREFS.with(|s| *s.borrow_mut() = Some(d));
            activate_row("DVB-Karte (Linux)");
            main_window(p)
        },
        1500,
    ),
    (
        "92-suchlauf",
        |p| {
            set_switch(tvjoe_core::tr("Radiosender übernehmen"), true);
            click_button(tvjoe_core::tr("Suchlauf starten"));
            main_window(p)
        },
        40000,
    ),
    (
        "93-umschalten",
        |p| {
            if let Some(d) = PREFS.with(|d| d.borrow_mut().take()) {
                d.close();
            }
            p.app.switch_live(2);
            main_window(p)
        },
        8000,
    ),
    (
        "94-info",
        |p| {
            p.app.toggle_info();
            main_window(p)
        },
        2000,
    ),
    (
        "95-aufnahme",
        |p| {
            p.app.toggle_info();
            crate::ablage::toggle_record(&p.app);
            main_window(p)
        },
        10000,
    ),
    (
        "96-aufnahme-beendet",
        |p| {
            crate::ablage::toggle_record(&p.app);
            main_window(p)
        },
        4000,
    ),
];

/// Seite einer Empfangsquelle (`TVJOE_SELFTEST_SEITE=<Titel der Zeile>`, nur diese Schritte):
/// Reiter „Empfang“ mit geöffnetem Hinzufügen-Menü, dann die Seite der genannten Quelle.
const SEITE_STEPS: &[Step] = &[
    ("97-start", main_window, 4000),
    (
        "98-hinzufuegen",
        |p| {
            let d = crate::prefs::open_at(&p.app, Some("empfang"), &p.app.window);
            PREFS.with(|s| *s.borrow_mut() = Some(d.clone()));
            fn find(w: &gtk::Widget, tip: &str) -> Option<gtk::Button> {
                if let Some(b) = w.downcast_ref::<gtk::Button>() {
                    if b.tooltip_text().as_deref() == Some(tip) {
                        return Some(b.clone());
                    }
                }
                let mut c = w.first_child();
                while let Some(child) = c {
                    if let Some(b) = find(&child, tip) {
                        return Some(b);
                    }
                    c = child.next_sibling();
                }
                None
            }
            if let Some(b) = find(d.upcast_ref(), tvjoe_core::tr("Empfangsquelle hinzufügen")) {
                b.emit_clicked();
            }
            main_window(p)
        },
        1500,
    ),
    (
        "99-seite",
        |p| {
            let title = std::env::var("TVJOE_SELFTEST_SEITE").unwrap_or_default();
            activate_row(&title);
            main_window(p)
        },
        1500,
    ),
];

/// Ersten Knopf mit der Beschriftung unterhalb von `w` drücken (Umschaltknöpfe einschalten).
fn click_in(w: &gtk::Widget, label: &str) {
    fn find(w: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if let Some(b) = w.downcast_ref::<gtk::Button>() {
            if b.label().as_deref() == Some(label) && b.is_sensitive() && b.is_mapped() {
                return Some(b.clone());
            }
        }
        let mut c = w.first_child();
        while let Some(child) = c {
            if let Some(b) = find(&child, label) {
                return Some(b);
            }
            c = child.next_sibling();
        }
        None
    }
    match find(w, label) {
        Some(b) => match b.downcast_ref::<gtk::ToggleButton>() {
            Some(t) => t.set_active(true),
            None => b.emit_clicked(),
        },
        None => eprintln!("tvjoe: Selbstprobe: Knopf „{label}“ nicht gefunden"),
    }
}

thread_local! {
    /// Serie und Staffel der Abo-Probe.
    static ABO: RefCell<Option<(String, Option<i64>)>> = const { RefCell::new(None) };
}

/// Abo und Nachladen (`TVJOE_SELFTEST_ABO=1`, nur mit Testordner als Ablage): eine Folge als
/// vorhanden eintragen, Ablage öffnen (Nachladen (n)), Abo „automatisch“ setzen und den Abgleich
/// nachstellen (lädt die fehlenden Folgen), dann alle Downloads wieder abbrechen.
const ABO_STEPS: &[Step] = &[
    ("88-start", main_window, 4000),
    (
        "89-folge-vorhanden",
        |p| {
            let lib = crate::jfview::probe_library_id();
            match crate::ablage::probe_fake_episode(&p.app, &lib) {
                Some((series, season, missing)) => {
                    eprintln!("tvjoe: Selbstprobe Abo: Staffel {season:?}, {missing} Folgen fehlen");
                    ABO.with(|a| *a.borrow_mut() = Some((series, season)));
                }
                None => eprintln!("tvjoe: Selbstprobe Abo: keine passende Staffel"),
            }
            crate::ablageview::open(&p.app);
            main_window(p)
        },
        4000,
    ),
    (
        "89a-abo-automatisch",
        |p| {
            let lib = crate::jfview::probe_library_id();
            if let Some((series, season)) = ABO.with(|a| a.borrow().clone()) {
                crate::ablage::set_subscription(&lib, &series, season, Some(tvjoe_core::ablage::Subscription::Auto));
                eprintln!("tvjoe: Selbstprobe Abo: gespeichert {:?}", crate::ablage::subscription(&lib, &series, season));
                crate::ablage::after_sync(&p.app, &lib);
            }
            main_window(p)
        },
        6000,
    ),
    (
        "89b-abgebrochen",
        |p| {
            let n = crate::ablage::probe_cancel_downloads();
            eprintln!("tvjoe: Selbstprobe Abo: {n} Downloads gestartet und abgebrochen");
            main_window(p)
        },
        2000,
    ),
];

/// Tvheadend (`TVJOE_SELFTEST_TVH=1`, nur diese Schritte): Seite der Quelle, „Kanäle holen“,
/// dann auf die Quelle umschalten und den Bildzähler prüfen.
fn tvh_source(p: &Probe) -> Option<u32> {
    p.app.settings().adapters.iter().find(|a| a.kind == "tvheadend").map(|a| a.num)
}

const TVH_STEPS: &[Step] = &[
    ("95-start", main_window, 4000),
    (
        "96-empfang",
        |p| {
            let d = crate::prefs::open_at(&p.app, Some("empfang"), &p.app.window);
            PREFS.with(|s| *s.borrow_mut() = Some(d));
            main_window(p)
        },
        2000,
    ),
    ("97-tvh-seite", |p| { activate_row("Tvheadend"); main_window(p) }, 2000),
    (
        "98-kanaele-holen",
        |p| {
            PREFS.with(|d| {
                if let Some(d) = d.borrow().as_ref() {
                    click_in(d.upcast_ref(), tvjoe_core::tr("Kanäle holen"));
                }
            });
            main_window(p)
        },
        6000,
    ),
    (
        "99-umschalten",
        |p| {
            PREFS.with(|d| {
                if let Some(d) = d.borrow_mut().take() {
                    d.close();
                }
            });
            if let Some(num) = tvh_source(p) {
                p.app.switch_live(num);
            }
            main_window(p)
        },
        12000,
    ),
    ("99a-zaehler", |p| { frames_trace(p, "Tvheadend a"); main_window(p) }, 3000),
    ("99b-zaehler", |p| { frames_trace(p, "Tvheadend b"); main_window(p) }, 500),
    ("99c-epg-holen", |p| { crate::satepg::run(&p.app); main_window(p) }, 10000),
    ("99d-info", |p| { p.app.toggle_info(); main_window(p) }, 2000),
];

/// Update-Dialog und Aufzeichnung der Bedienung (`TVJOE_SELFTEST_UPDATE=1`, nur diese
/// Schritte; mit `TVJOE_DIAGNOSE=1`): Angebot mit Notizen aus `TVJOE_SELFTEST_NOTES`, danach
/// ein Testfeld für Passwörter im Fokus. Klicks und Tasten kommen von außen (XTest).
const UPDATE_STEPS: &[Step] = &[
    ("90-start", main_window, 3000),
    ("91-update", |p| { crate::prefs::offer_probe(&p.app); main_window(p) }, 10000),
    (
        "92-eingabe",
        |p| {
            let entry = gtk::PasswordEntry::new();
            let d = adw::Dialog::builder().title("Testfeld").child(&entry).content_width(300).build();
            d.present(Some(&p.app.window));
            entry.grab_focus();
            main_window(p)
        },
        10000,
    ),
    ("93-ende", main_window, 500),
];

/// Ablage (`TVJOE_SELFTEST_ABLAGE=1`, nur diese Schritte): Ebene „Ablage“ (Taste D), erste
/// Gruppe aufgeklappt, „Alles prüfen“, dann die Seite „Über“ der Einstellungen.
const ABLAGE_STEPS: &[Step] = &[
    ("80-start", main_window, 4000),
    (
        "81-ablage",
        |p| {
            crate::ablageview::open(&p.app);
            main_window(p)
        },
        3000,
    ),
    (
        "82-ablage-gruppe",
        |p| {
            // Erste Gruppe, oder die mit `TVJOE_SELFTEST_ZEILE` im Titel.
            fn find(w: &gtk::Widget, want: &str) -> Option<adw::ExpanderRow> {
                if let Some(r) = w.downcast_ref::<adw::ExpanderRow>() {
                    if want.is_empty() || r.title().contains(want) {
                        return Some(r.clone());
                    }
                }
                let mut c = w.first_child();
                while let Some(child) = c {
                    if let Some(r) = find(&child, want) {
                        return Some(r);
                    }
                    c = child.next_sibling();
                }
                None
            }
            let want = std::env::var("TVJOE_SELFTEST_ZEILE").unwrap_or_default();
            if let Some(r) = find(p.app.window.upcast_ref(), &want) {
                r.set_expanded(true);
                // Bis zur Gruppe herunterrollen (sonst liegt sie unter dem Fensterrand).
                if let Some(sw) = r.ancestor(gtk::ScrolledWindow::static_type()).and_downcast::<gtk::ScrolledWindow>() {
                    let r2 = r.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                        if let Some(child) = sw.child() {
                            if let Some(pt) = r2.compute_point(&child, &gtk::graphene::Point::new(0.0, 0.0)) {
                                sw.vadjustment().set_value(f64::from(pt.y()) - 20.0);
                            }
                        }
                    });
                }
            }
            main_window(p)
        },
        2000,
    ),
    (
        "83-ablage-pruefen",
        |p| {
            click_in(p.app.window.upcast_ref(), tvjoe_core::tr("Alles prüfen"));
            main_window(p)
        },
        2000,
    ),
    (
        "84-ueber",
        |p| {
            let d = crate::prefs::open_at(&p.app, Some("ueber"), &p.app.window);
            PREFS.with(|s| *s.borrow_mut() = Some(d));
            main_window(p)
        },
        2000,
    ),
];

/// Ablage verschieben (`TVJOE_SELFTEST_UMZUG=<Zielordner>`, nur diese Schritte; nur mit
/// eigenem Testordner für Konfiguration und Ablage starten).
const UMZUG_STEPS: &[Step] = &[
    ("85-start", main_window, 3000),
    (
        "86-umzug",
        |p| {
            let target = std::env::var("TVJOE_SELFTEST_UMZUG").unwrap_or_default();
            crate::umzug::probe(p.app.window.upcast_ref(), std::path::Path::new(&target));
            main_window(p)
        },
        4000,
    ),
];

/// Quellenwechsel (`TVJOE_SELFTEST_WECHSEL=1`, nur diese Schritte): Start auf der laufenden
/// Quelle, dann Internet-TV, zurück und noch einmal; je Wechsel der Bildzähler zweimal im
/// Abstand von 3 s (bewegt sich das Bild?).
fn frames_trace(p: &Probe, what: &str) {
    let f = p.app.player.as_ref().and_then(|pl| pl.frames());
    let r = p.app.player.as_ref().map(|pl| pl.report()).unwrap_or_default();
    eprintln!("tvjoe: Selbstprobe {what}: Bild {f:?} · {r}");
}

const WECHSEL_STEPS: &[Step] = &[
    ("70-start", |p| { frames_trace(p, "Start"); main_window(p) }, 12000),
    ("71-zaehler", |p| { frames_trace(p, "Start a"); main_window(p) }, 3000),
    ("72-zaehler", |p| { frames_trace(p, "Start b"); main_window(p) }, 500),
    (
        "73-internet",
        |p| {
            p.app.switch_live(tvjoe_core::settings::SRC_TV);
            main_window(p)
        },
        10000,
    ),
    ("74-zaehler", |p| { frames_trace(p, "Internet a"); main_window(p) }, 3000),
    ("75-zaehler", |p| { frames_trace(p, "Internet b"); main_window(p) }, 500),
    (
        "76-sat",
        |p| {
            p.app.switch_live(tvjoe_core::settings::SRC_SAT);
            main_window(p)
        },
        10000,
    ),
    ("77-zaehler", |p| { frames_trace(p, "SAT a"); main_window(p) }, 3000),
    ("78-zaehler", |p| { frames_trace(p, "SAT b"); main_window(p) }, 500),
    (
        "79-internet",
        |p| {
            p.app.switch_live(tvjoe_core::settings::SRC_TV);
            main_window(p)
        },
        10000,
    ),
    ("79a-zaehler", |p| { frames_trace(p, "Internet2 a"); main_window(p) }, 3000),
    ("79b-zaehler", |p| { frames_trace(p, "Internet2 b"); main_window(p) }, 500),
];

/// Mediatheken (`TVJOE_SELFTEST_MEDIATHEKEN=1`, nur diese Schritte): Reiter „Mediatheken“,
/// Seite der Jellyfin-Bibliothek, Unterseite „Mediathek hinzufügen“.
/// EPG-Prüfung (`TVJOE_SELFTEST_EPGPRUEFUNG=1`, nur diese Schritte).
const EPGPRUEFUNG_STEPS: &[Step] = &[
    ("70-start", main_window, 8000),
    ("71-epg-hinweis", |p| crate::grid::open_window(&p.app), 2000),
    (
        "72-epg-weggeklickt",
        |p| {
            trace(&format!("EPG-Prüfung: {:?}", p.app.epg_warnings()));
            p.app.dismiss_epg_warnings();
            close_all(p);
            crate::grid::open_window(&p.app)
        },
        2000,
    ),
];

/// Vorauswahl und Zugangsdaten-Dialog (`TVJOE_SELFTEST_ZUGANG=1`, nur diese Schritte).
const ZUGANG_STEPS: &[Step] = &[
    ("80-start", main_window, 6000),
    (
        "81-vorschau-vorauswahl",
        |p| {
            let m = crate::manage::open_manager(&p.app)?;
            *p.mgr.borrow_mut() = Some(m.clone());
            let pr = crate::import::preview_path(&m, p.list.as_deref()?)?;
            trace(&format!("Vorauswahl: {:?}", pr.selected_names()));
            let w = crate::layer::window_of(&p.app, &(pr.clone() as Rc<dyn crate::layer::View>))?;
            *p.preview.borrow_mut() = Some(pr);
            Some(w)
        },
        4000,
    ),
    (
        "82-zugang-dialog",
        |p| {
            let pr = p.preview.borrow().clone()?;
            pr.set_where(|e| e.url.contains('@'), true);
            trace(&format!("Gewählt: {:?}", pr.selected_names()));
            pr.confirm_credentials();
            Some(p.app.window.clone().upcast())
        },
        2000,
    ),
];

const MEDIATHEKEN_STEPS: &[Step] = &[
    ("50-start", main_window, 4000),
    (
        "51-mediatheken",
        |p| {
            let d = crate::prefs::open_at(&p.app, Some("mediatheken"), &p.app.window);
            PREFS.with(|s| *s.borrow_mut() = Some(d));
            main_window(p)
        },
        2000,
    ),
    (
        "52-jellyfin-seite",
        |p| {
            activate_row(&std::env::var("TVJOE_SELFTEST_ZEILE").unwrap_or_else(|_| "Jellyfin".into()));
            main_window(p)
        },
        2000,
    ),
    (
        "53-symbolwahl",
        |p| {
            // Nur sichtbare Knöpfe: „Ändern …“ gibt es auch auf der Seite „Ablage“.
            PREFS.with(|d| {
                if let Some(d) = d.borrow().as_ref() {
                    click_in(d.upcast_ref(), tvjoe_core::tr("Ändern …"));
                }
            });
            main_window(p)
        },
        2000,
    ),
    (
        "54-symbol-gewaehlt",
        |p| {
            // Erstes Symbol mit dem Namen aus TVJOE_SELFTEST_SYMBOL (z. B. solid/server) wählen.
            let want = std::env::var("TVJOE_SELFTEST_SYMBOL").unwrap_or_else(|_| "solid/server".into());
            fn find(w: &gtk::Widget, name: &str) -> Option<gtk::Button> {
                if let Some(b) = w.downcast_ref::<gtk::Button>() {
                    if b.widget_name() == name {
                        return Some(b.clone());
                    }
                }
                let mut c = w.first_child();
                while let Some(child) = c {
                    if let Some(b) = find(&child, name) {
                        return Some(b);
                    }
                    c = child.next_sibling();
                }
                None
            }
            PREFS.with(|d| {
                if let Some(b) = d.borrow().as_ref().and_then(|d| find(d.upcast_ref(), &want)) {
                    b.emit_clicked();
                }
            });
            main_window(p)
        },
        2000,
    ),
    (
        "55-kopfleiste",
        |p| {
            PREFS.with(|d| {
                if let Some(d) = d.borrow_mut().take() {
                    d.close();
                }
            });
            main_window(p)
        },
        2500,
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

/// Bauchbinde (`TVJOE_SELFTEST_BAUCHBINDE=1`, nur diese Schritte): live, angehalten, gespult,
/// fortgesetzt, zurück auf live, Senderwechsel, Info. Zu jedem Bild steht die Höhe der
/// Bauchbinde in der Ausgabe (mit `TVJOE_TRACE=1`); sie muss überall gleich sein.
const BAUCHBINDE_STEPS: &[Step] = &[
    ("70-start", main_window, 8000),
    (
        "71-live",
        |p| {
            p.app.show_info();
            main_window(p)
        },
        1500,
    ),
    (
        "72-angehalten",
        |p| {
            p.app.toggle_live_pause();
            main_window(p)
        },
        1500,
    ),
    (
        "73-zurueckgespult",
        |p| {
            p.app.live_seek(-10.0);
            main_window(p)
        },
        1500,
    ),
    (
        "74-fortgesetzt",
        |p| {
            p.app.toggle_live_pause();
            main_window(p)
        },
        1500,
    ),
    (
        "75-wieder-live",
        |p| {
            p.app.go_live();
            main_window(p)
        },
        1500,
    ),
    (
        "76-senderwechsel",
        |p| {
            p.app.step(1);
            main_window(p)
        },
        2000,
    ),
    (
        "77-info",
        |p| {
            p.app.show_info();
            main_window(p)
        },
        1500,
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

/// Jellyfin (`TVJOE_SELFTEST_JELLYFIN=1`, nur diese Schritte; braucht Zugang und Einstellungen
/// im Testordner): erster Abgleich nach 30 s, dann Filme, Serien, erste Kachel.
const JELLYFIN_STEPS: &[Step] = &[
    ("60-warten-auf-abgleich", main_window, 65000),
    (
        "61-jellyfin-filme",
        |p| crate::jfview::probe_open(&p.app, 0),
        6000,
    ),
    (
        "62-jellyfin-erste-kachel",
        |p| crate::jfview::probe_first(&p.app),
        4000,
    ),
    (
        "63-jellyfin-serien",
        |p| {
            crate::layer::close_layer(&p.app);
            crate::jfview::probe_open(&p.app, 2)
        },
        6000,
    ),
    (
        "64-jellyfin-serienseite",
        |p| crate::jfview::probe_first(&p.app),
        4000,
    ),
    (
        "65-jellyfin-sammlungen",
        |p| {
            crate::layer::close_layer(&p.app);
            crate::jfview::probe_open(&p.app, 1)
        },
        6000,
    ),
    (
        "66-jellyfin-sammlung",
        |p| crate::jfview::probe_first(&p.app),
        4000,
    ),
];

/// Jellyfin-Wiedergabe (`TVJOE_SELFTEST_JFPLAY=<Eintrags-ID>`, nur diese Schritte): Eintrag
/// abspielen, nach 10 s Videodaten mitschreiben, nach 40 s Bild; Last von außen messen.
const JFPLAY_STEPS: &[Step] = &[
    ("70-vorher", main_window, 8000),
    (
        "71-jellyfin-wiedergabe",
        |p| {
            let id = std::env::var("TVJOE_SELFTEST_JFPLAY").ok()?;
            let library = std::env::var("TVJOE_SELFTEST_LIBRARY")
                .unwrap_or_else(|_| tvjoe_core::settings::LIBRARY_JELLYFIN.to_string());
            let item = crate::jfview::item(&library, &id)?;
            p.app.play_jellyfin(&library, &item, None, Vec::new());
            main_window(p)
        },
        12000,
    ),
    (
        "72-videodaten",
        |p| {
            if let Some(pl) = &p.app.player {
                trace(&pl.report());
            }
            main_window(p)
        },
        30000,
    ),
    (
        "73-videodaten-ende",
        |p| {
            if let Some(pl) = &p.app.player {
                trace(&pl.report());
            }
            main_window(p)
        },
        500,
    ),
];

/// README-Bilder (`TVJOE_SELFTEST_README=1`): Senderliste, Einblendung, Info, SAT mit
/// Empfangsanzeige, Einstellungen (SAT>IP), dann die Ebenen Programm, Verwaltung, Mediathek.
/// SAT und Einstellungen vor den Ebenen: direkt nach dem Wiedereinblenden des Videobilds
/// liefert das Fensterbild nichts.
const README_STEPS: &[Step] = &[
    ("readme-01-senderliste", main_window, 14000),
    (
        "readme-02-einblendung",
        |p| {
            p.app.toggle_sidebar();
            p.app.show_info();
            main_window(p)
        },
        1500,
    ),
    (
        "readme-03-info",
        |p| {
            p.app.toggle_info();
            main_window(p)
        },
        2000,
    ),
    (
        "readme-04-sat",
        |p| {
            p.app.toggle_info();
            p.app.switch_live(tvjoe_core::settings::SRC_SAT);
            main_window(p)
        },
        6000,
    ),
    (
        "readme-05-einstellungen-sat",
        |p| {
            crate::prefs::open_at(&p.app, Some("empfang"), &p.app.window);
            main_window(p)
        },
        2000,
    ),
    (
        "readme-06-programm",
        |p| {
            if let Some(d) = adw::prelude::AdwApplicationWindowExt::visible_dialog(&p.app.window) {
                adw::prelude::AdwDialogExt::close(&d);
            }
            p.app.switch_live(tvjoe_core::settings::SRC_TV);
            crate::grid::open(&p.app);
            main_window(p)
        },
        4000,
    ),
    (
        "readme-07-verwaltung",
        |p| {
            crate::layer::close_layer(&p.app);
            crate::manage::open(&p.app);
            main_window(p)
        },
        3000,
    ),
    (
        "readme-08-mediathek",
        |p| {
            crate::layer::close_layer(&p.app);
            crate::mediathek::open(&p.app);
            main_window(p)
        },
        8000,
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
    } else if std::env::var_os("TVJOE_SELFTEST_EMPFANG").is_some() {
        EMPFANG_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_DVB").is_some() {
        DVB_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_SEITE").is_some() {
        SEITE_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_GROESSE").is_some() {
        GROESSE_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_MEDIATHEKEN").is_some() {
        MEDIATHEKEN_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_WECHSEL").is_some() {
        WECHSEL_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_UMZUG").is_some() {
        UMZUG_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_ABO").is_some() {
        ABO_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_TVH").is_some() {
        TVH_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_UPDATE").is_some() {
        UPDATE_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_ABLAGE").is_some() {
        ABLAGE_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_JELLYFIN").is_some() {
        JELLYFIN_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_JFPLAY").is_some() {
        JFPLAY_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_BAUCHBINDE").is_some() {
        BAUCHBINDE_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_EPGPRUEFUNG").is_some() {
        EPGPRUEFUNG_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_ZUGANG").is_some() {
        ZUGANG_STEPS.to_vec()
    } else if std::env::var_os("TVJOE_SELFTEST_README").is_some() {
        README_STEPS.to_vec()
    } else {
        STEPS.to_vec()
    };
    if list.is_some() && std::env::var_os("TVJOE_SELFTEST_ZUGANG").is_none() {
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
        if std::env::var_os("TVJOE_SELFTEST_BAUCHBINDE").is_some() {
            trace(&format!("Bauchbinde {name}: {} px hoch", probe.app.osd_height()));
        }
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
