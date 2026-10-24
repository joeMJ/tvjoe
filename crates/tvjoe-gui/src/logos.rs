//! Senderlogos: Laden im Hintergrund, verkleinert (≤ 128 px) als PNG im Cache abgelegt.
//! Der Start wartet nie auf Logos; fertige Logos kommen über einen Kanal zurück.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use gtk::gdk_pixbuf::{Pixbuf, PixbufRotation};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
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

thread_local! {
    static BRIGHT: RefCell<HashMap<PathBuf, Option<gdk::Texture>>> = RefCell::new(HashMap::new());
}

/// Logo für dunkle Flächen (Mediathek, Bauchbinde): die größere Fläche weiß, die kleinere
/// grau; hochkant stehende Logos (arte) werden quer gedreht. Je Datei zwischengespeichert.
pub fn bright(path: &Path) -> Option<gdk::Texture> {
    if let Some(t) = BRIGHT.with(|b| b.borrow().get(path).cloned()) {
        return t;
    }
    let texture = Pixbuf::from_file(path).ok().and_then(|pb| {
        let pb = if pb.height() * 2 > pb.width() * 3 {
            pb.rotate_simple(PixbufRotation::Clockwise)?
        } else {
            pb
        };
        let pb = pb.add_alpha(false, 0, 0, 0).ok()?;
        let (w, h, stride) = (pb.width(), pb.height(), pb.rowstride() as usize);
        let mut px = pb.read_pixel_bytes().to_vec();
        whiten(&mut px, w as usize, h as usize, stride);
        Some(
            gdk::MemoryTexture::new(
                w,
                h,
                gdk::MemoryFormat::R8g8b8a8,
                &glib::Bytes::from(&px[..]),
                stride,
            )
            .upcast(),
        )
    });
    BRIGHT.with(|b| b.borrow_mut().insert(path.to_path_buf(), texture.clone()));
    texture
}

/// Bild mit [`bright`], sonst die Datei unverändert.
pub fn bright_picture(path: &Path) -> gtk::Picture {
    let pic = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .can_shrink(true)
        .build();
    set_bright(&pic, path);
    pic
}

pub fn set_bright(pic: &gtk::Picture, path: &Path) {
    match bright(path) {
        Some(t) => pic.set_paintable(Some(&t)),
        None => pic.set_filename(Some(path)),
    }
}

/// Grau des kleineren Flächenanteils (der größere wird weiß).
const MINOR: u8 = 115;

/// RGBA-Pixel (nicht vormultipliziert): helle und dunkle Pixel (Grenze halbe Helligkeit)
/// nach Deckkraft gewichtet zählen; der größere Anteil wird weiß, der kleinere grau.
/// Die Deckkraft bleibt.
fn whiten(px: &mut [u8], w: usize, h: usize, stride: usize) {
    let bright = |p: &[u8]| {
        0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]) >= 127.5
    };
    let (mut light, mut dark) = (0u64, 0u64);
    for y in 0..h {
        for p in px[y * stride..y * stride + w * 4].chunks_exact(4) {
            if bright(p) {
                light += u64::from(p[3]);
            } else {
                dark += u64::from(p[3]);
            }
        }
    }
    let major_light = light >= dark;
    for y in 0..h {
        for p in px[y * stride..y * stride + w * 4].chunks_exact_mut(4) {
            let v = if bright(p) == major_light { 255 } else { MINOR };
            p[0] = v;
            p[1] = v;
            p[2] = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groessere_flaeche_weiss_kleinere_grau() {
        // 2×2: drei dunkle deckende Pixel, ein heller; Deckkraft bleibt.
        let mut px = [
            20, 20, 20, 255, 20, 20, 20, 255, 30, 0, 0, 200, 250, 250, 250, 255,
        ];
        whiten(&mut px, 2, 2, 8);
        assert_eq!(&px[0..4], &[255, 255, 255, 255]);
        assert_eq!(&px[8..12], &[255, 255, 255, 200]);
        assert_eq!(&px[12..16], &[MINOR, MINOR, MINOR, 255]);
    }
}
