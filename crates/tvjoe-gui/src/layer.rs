//! Ansichten als Ebene im Hauptfenster oder als eigenes Fenster (Projekt „Fenster“).
//!
//! Eine Ansicht (Programm, Mediathek, Verwaltung, Importvorschau) liefert ihren Inhalt und
//! ihre Mindestgröße. Ist der Bildbereich groß genug, erscheint sie als Ebene über dem Bild:
//! das Bild wird nicht gezeichnet (schwarz), der Ton läuft weiter, links oben schließt ein
//! Kreuz bzw. führt ein Pfeil zurück, Esc ebenso. Ist er zu klein, oder wird das Hauptfenster
//! bei offener Ebene unter die Mindestgröße verkleinert, lebt die Ansicht in einem eigenen
//! Fenster wie bisher (ohne Zurückspringen beim Vergrößern).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::app::App;

pub trait View {
    fn title(&self) -> String;
    /// Kleinste Fläche (logische Pixel), in der die Ansicht noch bedienbar ist.
    fn min_size(&self) -> (i32, i32);
    fn body(&self) -> gtk::Widget;
    /// Links in der Kopfzeile, nach Schließen/Titel (z. B. „Datei importieren…“).
    fn header_start(&self) -> Option<gtk::Widget> {
        None
    }
    /// Mitte der Kopfzeile (z. B. Reiter); sonst der Titel.
    fn header_center(&self) -> Option<gtk::Widget> {
        None
    }
    /// Rechts in der Kopfzeile (z. B. Sortierung, Zahnrad).
    fn header_end(&self) -> Option<gtk::Widget> {
        None
    }
    /// Interner Schritt zurück (Filminfo → Übersicht); `false` = Ansicht schließen.
    fn back(&self) -> bool {
        false
    }
    /// Zeigt der Knopf links oben „Zurück“ statt „Schließen“?
    fn can_go_back(&self) -> bool {
        false
    }
    /// Die Ansicht wurde geschlossen (Ebene oder Fenster).
    fn closed(&self) {}
}

/// Ebene über dem Bild; gehört zum Hauptfenster.
pub struct Layer {
    pub root: gtk::Box,
    button: gtk::Button,
    title: gtk::Label,
    start: gtk::Box,
    center: gtk::Box,
    end: gtk::Box,
    body: gtk::Box,
    current: RefCell<Option<Rc<dyn View>>>,
    /// Ansichten in eigenen Fenstern (bleiben bis zum Schließen dort).
    windows: RefCell<Vec<(Rc<dyn View>, adw::Window)>>,
}

impl Layer {
    pub fn new() -> Rc<Self> {
        let button = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Schließen (Esc)")
            .css_classes(["flat"])
            .build();
        let title = gtk::Label::builder().css_classes(["tvjoe-name"]).build();
        let start = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let left = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        // Zurück/Schließen sitzt als „<“ in der Kopfleiste des Hauptfensters.
        button.set_visible(false);
        left.append(&button);
        left.append(&title);
        left.append(&start);
        let center = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let end = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let header = gtk::CenterBox::builder()
            .start_widget(&left)
            .center_widget(&center)
            .end_widget(&end)
            .css_classes(["tvjoe-layer-header"])
            .build();
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["tvjoe-layer"])
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .visible(false)
            .build();
        root.append(&header);
        root.append(&body);
        Rc::new(Self {
            root,
            button,
            title,
            start,
            center,
            end,
            body,
            current: RefCell::new(None),
            windows: RefCell::new(Vec::new()),
        })
    }

    /// Ist eine Ebene offen?
    pub fn is_open(&self) -> bool {
        self.current.borrow().is_some()
    }
}

fn detach(w: &impl IsA<gtk::Widget>) {
    if w.parent().is_some() {
        w.unparent();
    }
}

/// Öffnet eine Ansicht: als Ebene, wenn der Bildbereich groß genug ist, sonst als Fenster.
pub fn open(app: &Rc<App>, view: Rc<dyn View>) {
    let layer = app.layer.clone();
    // Schon offen (als Ebene oder Fenster): nach vorn holen.
    if let Some(w) = layer
        .windows
        .borrow()
        .iter()
        .find(|(v, _)| Rc::ptr_eq(v, &view))
        .map(|(_, w)| w.clone())
    {
        w.present();
        return;
    }
    if layer
        .current
        .borrow()
        .as_ref()
        .is_some_and(|c| Rc::ptr_eq(c, &view))
    {
        return;
    }
    if fits(app, view.as_ref()) {
        show_in_layer(app, view);
    } else {
        show_in_window(app, view);
    }
}

/// Passt die Ansicht in den Bildbereich des Hauptfensters?
fn fits(app: &App, view: &dyn View) -> bool {
    let (w, h) = app.video_area_size();
    let (mw, mh) = view.min_size();
    w >= mw && h >= mh
}

fn show_in_layer(app: &Rc<App>, view: Rc<dyn View>) {
    close_layer(app);
    let layer = &app.layer;
    layer.title.set_label(&view.title());
    if let Some(c) = view.header_center() {
        detach(&c);
        layer.center.append(&c);
        layer.title.set_visible(false);
    } else {
        layer.title.set_visible(true);
    }
    if let Some(e) = view.header_end() {
        detach(&e);
        layer.end.append(&e);
    }
    if let Some(s) = view.header_start() {
        detach(&s);
        layer.start.append(&s);
    }
    let body = view.body();
    detach(&body);
    layer.body.append(&body);
    *layer.current.borrow_mut() = Some(view);
    update_header(app);
    layer.root.set_visible(true);
    app.set_layer_active(true);
    let (mw, _, _, _) = layer.root.measure(gtk::Orientation::Horizontal, -1);
    let (mh, _, _, _) = layer.root.measure(gtk::Orientation::Vertical, mw);
    crate::trace(&format!(
        "Ebene „{}“: GTK-Mindestmaß {mw}×{mh}, eingestellt {:?}",
        layer.title.label(),
        layer
            .current
            .borrow()
            .as_ref()
            .map(|v| v.min_size())
            .unwrap_or_default()
    ));
}

fn show_in_window(app: &Rc<App>, view: Rc<dyn View>) {
    let header = adw::HeaderBar::new();
    match view.header_center() {
        Some(c) => {
            detach(&c);
            header.set_title_widget(Some(&c));
        }
        None => header.set_title_widget(Some(&adw::WindowTitle::new(&view.title(), ""))),
    }
    if let Some(e) = view.header_end() {
        detach(&e);
        header.pack_end(&e);
    }
    // „<“ immer da: zurück, auf der obersten Seite schließt er das Fenster.
    let back = gtk::Button::builder()
        .icon_name("go-previous-symbolic")
        .tooltip_text(if view.can_go_back() {
            "Zurück (Esc)"
        } else {
            "Schließen (Esc)"
        })
        .build();
    header.pack_start(&back);
    if let Some(s) = view.header_start() {
        detach(&s);
        header.pack_start(&s);
    }
    let body = view.body();
    detach(&body);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&header);
    tv.set_content(Some(&body));
    let (mw, mh) = view.min_size();
    let win = adw::Window::builder()
        .title(format!("{} – tvjoe", view.title()))
        .default_width((mw * 5 / 4).max(800))
        .default_height((mh * 5 / 4).max(560))
        .content(&tv)
        .build();
    let (v, b, w) = (view.clone(), back.clone(), win.clone());
    back.connect_clicked(move |_| {
        if !v.back() {
            w.close();
        }
        b.set_tooltip_text(Some(if v.can_go_back() {
            "Zurück (Esc)"
        } else {
            "Schließen (Esc)"
        }));
    });
    let keys = gtk::EventControllerKey::new();
    let (v, b, w) = (view.clone(), back.clone(), win.clone());
    keys.connect_key_pressed(move |_, key, _, _| {
        if key != gtk::gdk::Key::Escape {
            return glib::Propagation::Proceed;
        }
        if !v.back() {
            w.close();
        }
        b.set_tooltip_text(Some(if v.can_go_back() {
            "Zurück (Esc)"
        } else {
            "Schließen (Esc)"
        }));
        glib::Propagation::Stop
    });
    win.add_controller(keys);
    let weak = Rc::downgrade(app);
    let v = view.clone();
    win.connect_close_request(move |w| {
        if let Some(app) = weak.upgrade() {
            app.layer.windows.borrow_mut().retain(|(_, x)| x != w);
        }
        v.closed();
        glib::Propagation::Proceed
    });
    let (mw, _, _, _) = tv.measure(gtk::Orientation::Horizontal, -1);
    let (mh, _, _, _) = tv.measure(gtk::Orientation::Vertical, mw);
    crate::trace(&format!(
        "Fenster „{}“: GTK-Mindestmaß {mw}×{mh}, eingestellt {:?}",
        view.title(),
        view.min_size()
    ));
    app.layer.windows.borrow_mut().push((view, win.clone()));
    win.present();
}

/// Kopfzeile nach dem Zustand der Ansicht (Kreuz oder Pfeil zurück).
pub fn update_header(app: &App) {
    let layer = &app.layer;
    match layer.current.borrow().as_ref() {
        Some(cur) => {
            app.layer_back.set_tooltip_text(Some(if cur.can_go_back() {
                "Zurück (Esc)"
            } else {
                "Schließen (Esc)"
            }));
            app.layer_back.set_visible(true);
        }
        None => app.layer_back.set_visible(false),
    }
    // Fenster: Zurück-Knopf in deren Kopfleiste nachziehen.
    for (v, w) in layer.windows.borrow().iter() {
        if let Some(b) = w
            .content()
            .and_downcast::<adw::ToolbarView>()
            .and_then(|tv| find_back_button(&tv))
        {
            b.set_tooltip_text(Some(if v.can_go_back() {
                "Zurück (Esc)"
            } else {
                "Schließen (Esc)"
            }));
        }
    }
}

fn find_back_button(tv: &adw::ToolbarView) -> Option<gtk::Button> {
    // Erste Kopfleiste, erster Knopf links mit dem Zurück-Symbol.
    let mut w = tv.first_child();
    while let Some(c) = w {
        if let Some(b) = find_in(&c) {
            return Some(b);
        }
        w = c.next_sibling();
    }
    None
}

fn find_in(w: &gtk::Widget) -> Option<gtk::Button> {
    if let Some(b) = w.downcast_ref::<gtk::Button>() {
        if b.icon_name().as_deref() == Some("go-previous-symbolic") {
            return Some(b.clone());
        }
    }
    let mut c = w.first_child();
    while let Some(x) = c {
        if let Some(b) = find_in(&x) {
            return Some(b);
        }
        c = x.next_sibling();
    }
    None
}

/// Knopf links oben bzw. Esc: erst intern zurück, sonst Ebene schließen.
pub fn back_or_close(app: &Rc<App>) {
    let view = app.layer.current.borrow().clone();
    if let Some(v) = view {
        if v.back() {
            update_header(app);
        } else {
            close_layer(app);
        }
    }
}

/// Ebene schließen (falls offen) und das Bild wieder zeichnen.
pub fn close_layer(app: &Rc<App>) {
    let layer = &app.layer;
    let Some(cur) = layer.current.borrow_mut().take() else {
        return;
    };
    for slot in [&layer.start, &layer.center, &layer.end, &layer.body] {
        while let Some(c) = slot.first_child() {
            slot.remove(&c);
        }
    }
    layer.root.set_visible(false);
    app.layer_back.set_visible(false);
    app.set_layer_active(false);
    cur.closed();
}

/// Nach dem Abspielen aus einer Ansicht: als Ebene schließt sie sich, ein Fenster bleibt.
pub fn close_if_layer(app: &Rc<App>, view: &Rc<dyn View>) {
    let is_layer = app
        .layer
        .current
        .borrow()
        .as_ref()
        .is_some_and(|c| Rc::ptr_eq(c, view));
    if is_layer {
        close_layer(app);
    }
}

/// Ansicht schließen, egal ob Ebene oder Fenster (erkannt an ihrem Inhalt).
pub fn close_body(app: &Rc<App>, body: &gtk::Widget) {
    let in_layer = app
        .layer
        .current
        .borrow()
        .as_ref()
        .is_some_and(|c| &c.body() == body);
    if in_layer {
        close_layer(app);
        return;
    }
    let win = app
        .layer
        .windows
        .borrow()
        .iter()
        .find(|(v, _)| &v.body() == body)
        .map(|(_, w)| w.clone());
    if let Some(w) = win {
        w.close();
    }
}

/// Fenster, in dem die Ansicht gerade lebt (Hauptfenster bei einer Ebene; für die Selbstprobe).
pub fn window_of(app: &App, view: &Rc<dyn View>) -> Option<gtk::Window> {
    if app
        .layer
        .current
        .borrow()
        .as_ref()
        .is_some_and(|c| Rc::ptr_eq(c, view))
    {
        return Some(app.window.clone().upcast());
    }
    app.layer
        .windows
        .borrow()
        .iter()
        .find(|(v, _)| Rc::ptr_eq(v, view))
        .map(|(_, w)| w.clone().upcast())
}

/// Hauptfenster wurde verkleinert: eine offene Ebene, die nicht mehr passt, wandert in ein
/// eigenes Fenster.
pub fn check_size(app: &Rc<App>) {
    let view = app.layer.current.borrow().clone();
    let Some(view) = view else { return };
    if fits(app, view.as_ref()) {
        return;
    }
    // Ohne `closed()`: die Ansicht lebt im Fenster weiter (gleicher Zustand).
    let layer = &app.layer;
    layer.current.borrow_mut().take();
    for slot in [&layer.start, &layer.center, &layer.end, &layer.body] {
        while let Some(c) = slot.first_child() {
            slot.remove(&c);
        }
    }
    layer.root.set_visible(false);
    app.set_layer_active(false);
    show_in_window(app, view);
}

/// Knopf links oben mit dem Hauptfenster verbinden (einmal beim Start).
pub fn connect(app: &Rc<App>) {
    let weak = Rc::downgrade(app);
    app.layer.button.connect_clicked(move |_| {
        if let Some(app) = weak.upgrade() {
            back_or_close(&app);
        }
    });
}
