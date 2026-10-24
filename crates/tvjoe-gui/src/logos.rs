//! Senderlogos: Laden im Hintergrund, verkleinert (≤ 128 px) als PNG im Cache abgelegt.
//! Der Start wartet nie auf Logos; fertige Logos kommen über einen Kanal zurück.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use gtk::gdk_pixbuf::Pixbuf;
use gtk::{gio, glib};
use tvjoe_core::net;

const MAX_SIZE: i32 = 128;
const MAX_BYTES: u64 = 4 * 1024 * 1024;

pub struct Logos {
    requests: mpsc::Sender<(u64, String)>,
}

impl Logos {
    /// Startet den Hintergrund-Thread; `results` liefert (Sender-ID, Pfad der Logodatei).
    pub fn spawn(cache_dir: PathBuf) -> (Self, async_channel::Receiver<(u64, PathBuf)>) {
        let (req_tx, req_rx) = mpsc::channel::<(u64, String)>();
        let (res_tx, res_rx) = async_channel::unbounded();
        std::thread::spawn(move || {
            let agent = net::agent(Duration::from_secs(10));
            for (id, url) in req_rx {
                let path = cache_dir.join(net::cache_name(&url, "png"));
                if !path.exists() {
                    if let Err(e) = download(&agent, &url, &path) {
                        crate::trace(&format!("Logo {url}: {e}"));
                        continue;
                    }
                }
                if res_tx.send_blocking((id, path)).is_err() {
                    break;
                }
            }
        });
        (Self { requests: req_tx }, res_rx)
    }

    pub fn request(&self, id: u64, url: &str) {
        let _ = self.requests.send((id, url.to_string()));
    }
}

fn download(agent: &net::Agent, url: &str, path: &Path) -> Result<(), String> {
    let bytes = net::fetch(agent, url, MAX_BYTES)?;
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(bytes));
    let pixbuf =
        Pixbuf::from_stream_at_scale(&stream, MAX_SIZE, MAX_SIZE, true, gio::Cancellable::NONE)
            .map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    pixbuf.savev(&tmp, "png", &[]).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}
