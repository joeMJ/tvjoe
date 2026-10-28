//! Ablageordner verschieben (Nutzer, 2026-10-27): alle Dateien der Ablage samt Teildateien in
//! einen anderen Ordner bringen und das Verzeichnis nachtragen. Auf demselben Laufwerk wird nur
//! umbenannt, sonst kopiert, die Größe geprüft und erst dann das Original gelöscht. Nach jeder
//! Datei steht der neue Pfad im Verzeichnis; ein Abbruch lässt beides zueinander passend zurück.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adw::prelude::*;
use gtk::{gio, glib};
use tvjoe_core::ablage;
use tvjoe_core::{tf, tr};

use crate::ablageview::size_text;

/// Unterordner der Ablage (leer gewordene werden am alten Ort entfernt).
const SUB_DIRS: [&str; 4] = ["mediathek", "jellyfin", "records", "cache"];

/// Eine Datei des Verzeichnisses mit ihren Teildateien: (alt, neu).
struct Item {
    id: i64,
    files: Vec<(PathBuf, PathBuf)>,
    /// Neuer Pfad des Eintrags.
    target: PathBuf,
}

enum Msg {
    Progress(u64),
    Moved(i64, PathBuf),
    Failed(String),
    Finished { cancelled: bool },
}

/// Knopf „Verschieben nach …“: Hindernisse prüfen, Ziel wählen, bestätigen, verschieben.
/// `done` bekommt den neuen Ablageordner.
pub fn start(parent: &gtk::Button, done: impl Fn(&Path) + 'static) {
    let Some(app) = crate::ablage::app() else { return };
    if let Some(why) = crate::ablage::move_blocker() {
        alert(
            parent,
            tr("Verschieben nicht möglich"),
            &tf!("Gerade läuft etwas: {why}. Bitte danach erneut versuchen.", why = why),
        );
        return;
    }
    let old = crate::ablage::storage_dir(&app);
    let dialog = gtk::FileDialog::builder()
        .title(tr("Ablage verschieben nach"))
        .modal(true)
        .build();
    let window = parent.root().and_downcast::<gtk::Window>();
    let parent = parent.clone();
    let done: std::rc::Rc<dyn Fn(&Path)> = std::rc::Rc::new(done);
    dialog.select_folder(window.as_ref(), gio::Cancellable::NONE, move |r| {
        let Some(new) = r.ok().and_then(|f| f.path()) else { return };
        if let Err(e) = check_target(&old, &new) {
            alert(&parent, tr("Verschieben nicht möglich"), &e);
            return;
        }
        let items = plan(&old, &new);
        let bytes: u64 = items
            .iter()
            .flat_map(|i| &i.files)
            .map(|(src, _)| std::fs::metadata(src).map(|m| m.len()).unwrap_or(0))
            .sum();
        if !same_volume(&old, &new)
            && crate::ablage::free_bytes(&new).is_some_and(|f| f < bytes + (1 << 30))
        {
            alert(
                &parent,
                tr("Zu wenig Platz am Ziel"),
                &tf!(
                    "Zu verschieben sind {size}, am Ziel sind nur {free} frei.",
                    size = size_text(bytes),
                    free = size_text(crate::ablage::free_bytes(&new).unwrap_or(0))
                ),
            );
            return;
        }
        let files = items.iter().map(|i| i.files.len()).sum::<usize>();
        let ask = adw::AlertDialog::builder()
            .heading(tr("Ablage verschieben?"))
            .body(tf!(
                "{files} Dateien ({size}) von\n{old}\nnach\n{new}\nverschieben. Danach legt tvjoe Neues dort ab.",
                files = files,
                size = size_text(bytes),
                old = old.display(),
                new = new.display()
            ))
            .build();
        ask.add_responses(&[("cancel", tr("Abbrechen")), ("move", tr("Verschieben"))]);
        ask.set_response_appearance("move", adw::ResponseAppearance::Suggested);
        ask.set_default_response(Some("cancel"));
        let (p, old, done) = (parent.clone(), old.clone(), done.clone());
        ask.connect_response(None, move |_, r| {
            if r == "move" {
                run(p.upcast_ref(), &old, &new, bytes, done.clone());
            }
        });
        ask.present(Some(&parent));
    });
}

/// Selbstprobe: ohne Ordnerwahl und Rückfrage nach `new` verschieben.
pub fn probe(parent: &gtk::Widget, new: &Path) {
    let Some(app) = crate::ablage::app() else { return };
    let old = crate::ablage::storage_dir(&app);
    if let Err(e) = check_target(&old, new) {
        alert(parent, tr("Verschieben nicht möglich"), &e);
        return;
    }
    let bytes: u64 = plan(&old, new)
        .iter()
        .flat_map(|i| &i.files)
        .map(|(src, _)| std::fs::metadata(src).map(|m| m.len()).unwrap_or(0))
        .sum();
    run(parent, &old, new, bytes, std::rc::Rc::new(|_: &Path| {}));
}

/// Ziel prüfen: nicht derselbe Ordner, nicht innerhalb der Ablage, beschreibbar.
fn check_target(old: &Path, new: &Path) -> Result<(), String> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (o, n) = (canon(old), canon(new));
    if o == n {
        return Err(tr("Das ist schon der Ablageordner.").into());
    }
    if n.starts_with(&o) {
        return Err(tr("Das Ziel liegt innerhalb der bisherigen Ablage.").into());
    }
    // Fehlenden Zielordner anlegen (sonst schlägt die Schreibprobe fehl, unter Windows mit
    // „os error 3“).
    std::fs::create_dir_all(new).map_err(|e| tf!("Zielordner lässt sich nicht anlegen: {error}", error = e))?;
    let probe = new.join(".tvjoe-schreibprobe");
    std::fs::write(&probe, b"")
        .map_err(|e| tf!("Am Ziel lässt sich nicht schreiben: {error}", error = e))?;
    let _ = std::fs::remove_file(probe);
    Ok(())
}

/// Liegen beide Ordner auf demselben Laufwerk (Umbenennen statt Kopieren)?
fn same_volume(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let root = |p: &Path| p.components().next().map(|c| c.as_os_str().to_ascii_lowercase());
        root(a).is_some() && root(a) == root(b)
    }
}

/// Was zu verschieben ist: jeder Eintrag unterhalb der alten Ablage mit Datei und Teildateien.
fn plan(old: &Path, new: &Path) -> Vec<Item> {
    let mut out = Vec::new();
    for e in crate::ablage::all_entries() {
        let Ok(rel) = e.path.strip_prefix(old) else { continue };
        let mut target = ablage::unique_path(&new.join(rel));
        // Kein Ziel zweimal vergeben (unique_path sieht nur, was schon auf der Platte liegt).
        while out.iter().any(|i: &Item| i.target == target) {
            target = ablage::unique_path(&target.with_file_name(format!(
                "_{}",
                target.file_name().unwrap_or_default().to_string_lossy()
            )));
        }
        let pairs = [
            (e.path.clone(), target.clone()),
            (ablage::part_path(&e.path), ablage::part_path(&target)),
            (crate::capture::part_path(&e.path), crate::capture::part_path(&target)),
        ];
        let files: Vec<(PathBuf, PathBuf)> = pairs.into_iter().filter(|(s, _)| s.exists()).collect();
        out.push(Item { id: e.id, files, target });
    }
    out
}

/// Verschieben im Hintergrund mit Fortschritt; „Abbrechen“ hält nach der laufenden Datei an.
/// Der Plan entsteht erst hier neu (zwischen Rückfrage und Start kann etwas fertig werden).
fn run(
    parent: &gtk::Widget,
    old: &Path,
    new: &Path,
    bytes: u64,
    done: std::rc::Rc<dyn Fn(&Path)>,
) {
    let items = plan(old, new);
    let bar = gtk::ProgressBar::builder()
        .show_text(true)
        .text(tf!("0 von {size}", size = size_text(bytes)))
        .build();
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Ablage wird verschoben"))
        .body(tr("Bitte tvjoe offen lassen."))
        .extra_child(&bar)
        .can_close(false)
        .build();
    dialog.add_response("cancel", tr("Abbrechen"));
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let cancel = cancel.clone();
        dialog.connect_response(Some("cancel"), move |d, _| {
            cancel.store(true, Ordering::Relaxed);
            d.set_body(tr("Wird nach der laufenden Datei angehalten …"));
        });
    }
    dialog.present(Some(parent));
    let (tx, rx) = async_channel::unbounded();
    {
        let cancel = cancel.clone();
        std::thread::spawn(move || work(items, &cancel, &tx));
    }
    let (parent, old, new) = (parent.clone(), old.to_path_buf(), new.to_path_buf());
    glib::spawn_future_local(async move {
        let mut error = None;
        let mut moved = 0usize;
        while let Ok(msg) = rx.recv().await {
            match msg {
                Msg::Progress(n) => {
                    bar.set_fraction(n as f64 / bytes.max(1) as f64);
                    bar.set_text(Some(&tf!(
                        "{done} von {size}",
                        done = size_text(n),
                        size = size_text(bytes)
                    )));
                }
                Msg::Moved(id, path) => {
                    crate::ablage::set_entry_path(id, &path);
                    moved += 1;
                }
                Msg::Failed(e) => error = Some(e),
                Msg::Finished { cancelled } => {
                    dialog.set_can_close(true);
                    dialog.force_close();
                    finish(&parent, &old, &new, moved, cancelled, error.take(), &done);
                    break;
                }
            }
        }
    });
}

/// Ladethread: Dateien verschieben, nach jedem Eintrag melden.
fn work(items: Vec<Item>, cancel: &AtomicBool, tx: &async_channel::Sender<Msg>) {
    let mut done = 0u64;
    for item in items {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send_blocking(Msg::Finished { cancelled: true });
            return;
        }
        for (src, dst) in &item.files {
            if let Err(e) = move_file(src, dst, &mut done, tx) {
                let _ = tx.send_blocking(Msg::Failed(tf!(
                    "{file}: {error}",
                    file = src.display(),
                    error = e
                )));
                let _ = tx.send_blocking(Msg::Finished { cancelled: false });
                return;
            }
        }
        let _ = tx.send_blocking(Msg::Moved(item.id, item.target));
    }
    let _ = tx.send_blocking(Msg::Finished { cancelled: false });
}

/// Eine Datei: umbenennen, sonst kopieren (mit Fortschritt), Größe prüfen, Original löschen.
fn move_file(
    src: &Path,
    dst: &Path,
    done: &mut u64,
    tx: &async_channel::Sender<Msg>,
) -> std::io::Result<()> {
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let len = std::fs::metadata(src)?.len();
    if std::fs::rename(src, dst).is_ok() {
        *done += len;
        let _ = tx.send_blocking(Msg::Progress(*done));
        return Ok(());
    }
    let result = (|| {
        let mut input = std::fs::File::open(src)?;
        let mut output = std::fs::File::create(dst)?;
        let mut buf = vec![0u8; 8 << 20];
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n])?;
            *done += n as u64;
            let _ = tx.send_blocking(Msg::Progress(*done));
        }
        output.sync_all()?;
        if std::fs::metadata(dst)?.len() != len {
            return Err(std::io::Error::other(tr("Größe nach dem Kopieren abweichend")));
        }
        Ok(())
    })();
    match result {
        Ok(()) => std::fs::remove_file(src),
        Err(e) => {
            let _ = std::fs::remove_file(dst);
            Err(e)
        }
    }
}

/// Abschluss: bei Erfolg Ablageordner umstellen und leere alte Ordner entfernen; Meldung.
fn finish(
    parent: &gtk::Widget,
    old: &Path,
    new: &Path,
    moved: usize,
    cancelled: bool,
    error: Option<String>,
    done: &std::rc::Rc<dyn Fn(&Path)>,
) {
    let complete = !cancelled && error.is_none();
    if complete {
        if let Some(app) = crate::ablage::app() {
            app.settings_mut().storage_dir = Some(new.to_string_lossy().to_string());
            app.save_settings();
        }
        for sub in SUB_DIRS {
            let _ = std::fs::remove_dir(old.join(sub));
        }
        let _ = std::fs::remove_dir(old);
        done(new);
    }
    let (heading, body) = match (complete, error) {
        (true, _) => (
            tr("Ablage verschoben"),
            tf!(
                "{n} Einträge liegen jetzt in\n{new}",
                n = moved,
                new = new.display()
            ),
        ),
        (false, Some(e)) => (
            tr("Verschieben fehlgeschlagen"),
            tf!(
                "{n} Einträge wurden verschoben, dann trat ein Fehler auf:\n{error}\n\nDas Verzeichnis passt zu den Dateien; der Ablageordner bleibt der alte.",
                n = moved,
                error = e
            ),
        ),
        (false, None) => (
            tr("Verschieben angehalten"),
            tf!(
                "{n} Einträge wurden verschoben, der Rest liegt noch am alten Ort. Das Verzeichnis passt zu den Dateien; der Ablageordner bleibt der alte.",
                n = moved
            ),
        ),
    };
    alert(parent, heading, &body);
}

fn alert(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str) {
    let d = adw::AlertDialog::builder().heading(heading).body(body).build();
    d.add_response("close", tr("OK"));
    d.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fehlender Zielordner wird angelegt (früher „os error 3“ unter Windows).
    #[test]
    fn zielordner_wird_angelegt() {
        let base = std::env::temp_dir().join(format!("tvjoe-umzug-{}", std::process::id()));
        let old = base.join("alt");
        let new = base.join("neu").join("tiefer");
        std::fs::create_dir_all(&old).unwrap();
        assert!(check_target(&old, &new).is_ok());
        assert!(new.is_dir());
        assert!(check_target(&old, &old.join("innen")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
