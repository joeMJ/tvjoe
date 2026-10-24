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
