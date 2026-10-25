//! Ziehen und Ablegen von Senderzeilen (Senderliste und Verwaltung).

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

/// Macht eine Zeile ziehbar und zum Ablageziel. `on_drop(gezogener Sender, Ziel, dahinter)`
/// läuft erst nach Ende des Ziehens, weil die Listen danach neu aufgebaut werden.
pub fn attach(row: &gtk::ListBoxRow, id: u64, on_drop: impl Fn(u64, u64, bool) + 'static) {
    let source = gtk::DragSource::builder()
        .actions(gdk::DragAction::MOVE)
        .build();
    source.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&id.to_value())));
    let weak = row.downgrade();
    source.connect_drag_begin(move |src, _| {
        if let Some(r) = weak.upgrade() {
            src.set_icon(Some(&gtk::WidgetPaintable::new(Some(&r))), 0, 0);
        }
    });
    row.add_controller(source);

    let on_drop = Rc::new(on_drop);
    let target = gtk::DropTarget::new(u64::static_type(), gdk::DragAction::MOVE);
    let weak = row.downgrade();
    target.connect_drop(move |_, value, _, y| {
        let (Ok(src), Some(r)) = (value.get::<u64>(), weak.upgrade()) else {
            return false;
        };
        let after = y > f64::from(r.height()) / 2.0;
        let f = on_drop.clone();
        glib::idle_add_local_once(move || f(src, id, after));
        true
    });
    row.add_controller(target);
}

/// Beim Ziehen am oberen bzw. unteren Rand mitlaufen (je näher am Rand, desto schneller).
pub fn autoscroll(scroller: &gtk::ScrolledWindow) {
    const EDGE: f64 = 48.0;
    let motion = gtk::DropControllerMotion::new();
    let y = Rc::new(std::cell::Cell::new(None::<f64>));
    let timer: Rc<std::cell::RefCell<Option<glib::SourceId>>> = Rc::default();
    let yy = y.clone();
    motion.connect_motion(move |_, _, py| yy.set(Some(py)));
    let (yy, t, sc) = (y.clone(), timer.clone(), scroller.downgrade());
    motion.connect_enter(move |_, _, py| {
        yy.set(Some(py));
        if t.borrow().is_some() {
            return;
        }
        let (yy, sc, t2) = (yy.clone(), sc.clone(), t.clone());
        let id = glib::timeout_add_local(std::time::Duration::from_millis(30), move || {
            let (Some(py), Some(sc)) = (yy.get(), sc.upgrade()) else {
                t2.borrow_mut().take();
                return glib::ControlFlow::Break;
            };
            let h = f64::from(sc.height());
            let step = if py < EDGE {
                -(EDGE - py) / 2.0
            } else if py > h - EDGE {
                (py - (h - EDGE)) / 2.0
            } else {
                0.0
            };
            if step != 0.0 {
                let adj = sc.vadjustment();
                adj.set_value(
                    (adj.value() + step).clamp(adj.lower(), adj.upper() - adj.page_size()),
                );
            }
            glib::ControlFlow::Continue
        });
        *t.borrow_mut() = Some(id);
    });
    let (yy, t) = (y, timer);
    motion.connect_leave(move |_| {
        yy.set(None);
        if let Some(id) = t.borrow_mut().take() {
            id.remove();
        }
    });
    scroller.add_controller(motion);
}
