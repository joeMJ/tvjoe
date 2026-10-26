//! Schwarze Balken im Bild erkennen und den Ausschnitt über `video-zoom`/`video-pan` einstellen.
//! Das geht schon mit libmpv 0.37 (Ubuntu 24.04); `video-crop` gibt es erst ab 0.38. Die
//! Balken bleiben dabei Teil des Bildes, liegen aber außerhalb des Fensters oder sehen aus wie
//! der leere Rand. Erkannt wird an Einzelbildern aus mpv (`screenshot-raw`), in einem eigenen
//! Thread, damit das Zurückholen aus der Grafikeinheit die Oberfläche nicht aufhält.

use std::ffi::{c_char, CStr, CString};

use libmpv2_sys as sys;

/// Bildteil ohne Balken als Anteile des ganzen Bildes (0.0–1.0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Abweichung, ab der zwei Messungen als verschieden gelten (Anteil des Bildes).
const TOLERANCE: f64 = 0.01;

impl Area {
    pub const FULL: Area = Area {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    fn right(&self) -> f64 {
        self.x + self.w
    }

    fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn close_to(&self, o: &Area) -> bool {
        (self.x - o.x).abs() <= TOLERANCE
            && (self.y - o.y).abs() <= TOLERANCE
            && (self.right() - o.right()).abs() <= TOLERANCE
            && (self.bottom() - o.bottom()).abs() <= TOLERANCE
    }

    /// Liegt `o` (bis auf die Toleranz) ganz in diesem Ausschnitt?
    fn contains(&self, o: &Area) -> bool {
        o.x >= self.x - TOLERANCE
            && o.y >= self.y - TOLERANCE
            && o.right() <= self.right() + TOLERANCE
            && o.bottom() <= self.bottom() + TOLERANCE
    }

    fn union(&self, o: &Area) -> Area {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Area {
            x,
            y,
            w: self.right().max(o.right()) - x,
            h: self.bottom().max(o.bottom()) - y,
        }
    }
}

/// Helligkeit, bis zu der ein Bildpunkt als schwarz gilt (größter Farbanteil, 0–255).
const DARK: u8 = 32;
/// Anteil heller Bildpunkte, den eine Zeile oder Spalte noch haben darf (Rauschen, Artefakte).
const BRIGHT_ALLOWED: f64 = 0.01;
/// Balken unter diesem Anteil zählen nicht (Kanten, Überscan).
const MIN_BAR: f64 = 0.01;
/// Der Bildteil muss mindestens so groß sein; sonst ist es eine dunkle Szene oder Abblende.
const MIN_CONTENT: f64 = 0.3;

/// Erkennt Balken in einem Bild mit 4 Byte je Bildpunkt (bgr0 o. ä.; die Reihenfolge der
/// Farben spielt keine Rolle). `None`: Bild unbrauchbar (fast ganz dunkel).
pub fn detect(data: &[u8], w: usize, h: usize, stride: usize) -> Option<Area> {
    if w < 16 || h < 16 || stride < w * 4 || data.len() < stride * (h - 1) + w * 4 {
        return None;
    }
    let px = |x: usize, y: usize| {
        let i = y * stride + x * 4;
        data[i].max(data[i + 1]).max(data[i + 2])
    };
    let dark_row = |y: usize| {
        let step = (w / 256).max(1);
        let (mut n, mut bright) = (0usize, 0usize);
        for x in (0..w).step_by(step) {
            n += 1;
            if px(x, y) > DARK {
                bright += 1;
            }
        }
        (bright as f64) <= n as f64 * BRIGHT_ALLOWED
    };
    let top = (0..h).take_while(|&y| dark_row(y)).count();
    if top == h {
        return None;
    }
    let bottom = (0..h).rev().take_while(|&y| dark_row(y)).count();
    let (y0, y1) = (top, h - bottom);
    let dark_col = |x: usize| {
        let step = ((y1 - y0) / 256).max(1);
        let (mut n, mut bright) = (0usize, 0usize);
        for y in (y0..y1).step_by(step) {
            n += 1;
            if px(x, y) > DARK {
                bright += 1;
            }
        }
        (bright as f64) <= n as f64 * BRIGHT_ALLOWED
    };
    let left = (0..w).take_while(|&x| dark_col(x)).count();
    let right = (0..w).rev().take_while(|&x| dark_col(x)).count();
    let frac = |n: usize, total: usize| {
        let f = n as f64 / total as f64;
        if f < MIN_BAR {
            0.0
        } else {
            f
        }
    };
    let (t, b) = (frac(top, h), frac(bottom, h));
    let (l, r) = (frac(left, w), frac(right, w));
    let area = Area {
        x: l,
        y: t,
        w: 1.0 - l - r,
        h: 1.0 - t - b,
    };
    (area.w >= MIN_CONTENT && area.h >= MIN_CONTENT).then_some(area)
}

/// Messungen hintereinander, die für einen engeren Ausschnitt übereinstimmen müssen.
const HITS: u32 = 2;

/// Hält den gezeigten Ausschnitt ruhig: enger erst nach übereinstimmenden Messungen
/// hintereinander (eine dunkle Szene schneidet nichts ab), weiter sofort, sobald Bild in
/// einen abgeschnittenen Rand kommt.
pub struct Tracker {
    shown: Area,
    candidate: Option<Area>,
    hits: u32,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            shown: Area::FULL,
            candidate: None,
            hits: 0,
        }
    }
}

impl Tracker {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn shown(&self) -> Area {
        self.shown
    }

    /// Neue Messung; gibt den neuen Ausschnitt zurück, wenn er sich ändert.
    pub fn feed(&mut self, m: Option<Area>) -> Option<Area> {
        let m = m?;
        if m.close_to(&self.shown) {
            self.candidate = None;
            return None;
        }
        if !self.shown.contains(&m) {
            self.candidate = None;
            self.shown = self.shown.union(&m);
            return Some(self.shown);
        }
        match self.candidate {
            Some(c) if c.close_to(&m) => self.hits += 1,
            _ => {
                self.candidate = Some(m);
                self.hits = 1;
            }
        }
        if self.hits >= HITS {
            self.candidate = None;
            self.shown = m;
            return Some(m);
        }
        None
    }
}

/// Einstellung für mpv: `video-zoom` (log2) und `video-pan-x/-y` (Anteile des skalierten Bildes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
}

/// Ausschnitt `area` eines Bildes der Anzeigegröße `video` ins Fenster `window`:
/// einpassen (ganz sichtbar) oder füllen (stößt an allen Seiten an, Überstand fällt weg).
pub fn view(area: Area, video: (f64, f64), window: (f64, f64), fill: bool) -> View {
    let (dw, dh) = video;
    let (ww, wh) = window;
    if dw <= 0.0 || dh <= 0.0 || ww <= 0.0 || wh <= 0.0 {
        return View {
            zoom: 0.0,
            pan_x: 0.0,
            pan_y: 0.0,
        };
    }
    // mpv passt das ganze Bild ein; der Zoom ist relativ dazu.
    let base = (ww / dw).min(wh / dh);
    let (cw, ch) = (area.w * dw, area.h * dh);
    let scale = if fill {
        (ww / cw).max(wh / ch)
    } else {
        (ww / cw).min(wh / ch)
    };
    View {
        zoom: (scale / base).log2(),
        pan_x: 0.5 - (area.x + area.w / 2.0),
        pan_y: 0.5 - (area.y + area.h / 2.0),
    }
}

/// Einzelbild aus mpv holen und auswerten (aus einem beliebigen Thread; die Client-API von
/// libmpv ist threadfest). `None`: kein Bild oder unbrauchbar.
pub fn grab(ctx: *mut sys::mpv_handle) -> Option<Area> {
    let args: Vec<CString> = ["screenshot-raw", "video"]
        .iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
    let mut argv: Vec<*const c_char> = args.iter().map(|s| s.as_ptr()).collect();
    argv.push(std::ptr::null());
    let mut node: sys::mpv_node = unsafe { std::mem::zeroed() };
    let rc = unsafe { sys::mpv_command_ret(ctx, argv.as_mut_ptr(), &mut node) };
    if rc < 0 {
        return None;
    }
    let result = unsafe { read_frame(&node) };
    unsafe { sys::mpv_free_node_contents(&mut node) };
    result
}

/// Liest w, h, stride und data aus dem Ergebnis von `screenshot-raw` und wertet aus.
unsafe fn read_frame(node: &sys::mpv_node) -> Option<Area> {
    if node.format != sys::mpv_format_MPV_FORMAT_NODE_MAP {
        return None;
    }
    let list = unsafe { &*node.u.list };
    let (mut w, mut h, mut stride) = (0i64, 0i64, 0i64);
    let mut bytes: Option<&[u8]> = None;
    for i in 0..list.num.max(0) as usize {
        let key = unsafe { CStr::from_ptr(*list.keys.add(i)) };
        let v = unsafe { &*list.values.add(i) };
        let int = || (v.format == sys::mpv_format_MPV_FORMAT_INT64).then(|| unsafe { v.u.int64 });
        match key.to_bytes() {
            b"w" => w = int().unwrap_or(0),
            b"h" => h = int().unwrap_or(0),
            b"stride" => stride = int().unwrap_or(0),
            b"data" if v.format == sys::mpv_format_MPV_FORMAT_BYTE_ARRAY => {
                let ba = unsafe { &*v.u.ba };
                if !ba.data.is_null() {
                    bytes = Some(unsafe { std::slice::from_raw_parts(ba.data as *const u8, ba.size) });
                }
            }
            _ => {}
        }
    }
    if w <= 0 || h <= 0 || stride <= 0 {
        return None;
    }
    detect(bytes?, w as usize, h as usize, stride as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bild w×h mit hellem Inhalt im Rechteck (x0..x1, y0..y1), sonst schwarz.
    fn frame(w: usize, h: usize, x0: usize, x1: usize, y0: usize, y1: usize) -> Vec<u8> {
        let mut d = vec![0u8; w * h * 4];
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * w + x) * 4;
                d[i] = 120;
                d[i + 1] = 90;
                d[i + 2] = 60;
            }
        }
        d
    }

    #[test]
    fn letterbox() {
        // 2,39:1 in 1920×1080: Balken oben und unten je 138 Zeilen.
        let d = frame(1920, 1080, 0, 1920, 138, 942);
        let a = detect(&d, 1920, 1080, 1920 * 4).unwrap();
        assert!(a.x == 0.0 && a.w == 1.0);
        assert!((a.y - 138.0 / 1080.0).abs() < 1e-9);
        assert!((a.h - 804.0 / 1080.0).abs() < 1e-9);
    }

    #[test]
    fn pillarbox_and_dark() {
        let d = frame(1920, 1080, 240, 1680, 0, 1080);
        let a = detect(&d, 1920, 1080, 1920 * 4).unwrap();
        assert!((a.x - 0.125).abs() < 1e-9 && (a.w - 0.75).abs() < 1e-9);
        assert_eq!(detect(&vec![0u8; 64 * 64 * 4], 64, 64, 256), None);
        // Nur ein kleiner heller Fleck: zu wenig Bild, keine Aussage.
        let d = frame(100, 100, 40, 60, 40, 60);
        assert_eq!(detect(&d, 100, 100, 400), None);
    }

    #[test]
    fn tracker_needs_two_hits_to_narrow_but_widens_at_once() {
        let narrow = Area {
            x: 0.0,
            y: 0.13,
            w: 1.0,
            h: 0.74,
        };
        let mut t = Tracker::default();
        assert_eq!(t.feed(Some(narrow)), None);
        assert_eq!(t.feed(Some(narrow)), Some(narrow));
        assert_eq!(t.feed(Some(narrow)), None);
        assert_eq!(t.feed(Some(Area::FULL)), Some(Area::FULL));
        assert_eq!(t.feed(None), None);
    }

    #[test]
    fn view_fit_and_fill() {
        let area = Area {
            x: 0.0,
            y: 0.125,
            w: 1.0,
            h: 0.75,
        };
        // 16:9-Rahmen mit 2,37:1-Bild im 32:9-Fenster: einpassen nach Höhe.
        let v = view(area, (1920.0, 1080.0), (5120.0, 1440.0), false);
        assert!((v.zoom - (1.0f64 / 0.75).log2()).abs() < 1e-9);
        assert_eq!(v.pan_y, 0.0);
        // Füllen: Breite stößt an.
        let v = view(area, (1920.0, 1080.0), (5120.0, 1440.0), true);
        let base: f64 = 1440.0 / 1080.0;
        assert!((v.zoom - ((5120.0 / 1920.0) / base).log2()).abs() < 1e-9);
        // Ganzes Bild, gleiches Seitenverhältnis: nichts zu tun.
        let v = view(Area::FULL, (1920.0, 1080.0), (960.0, 540.0), true);
        assert!(v.zoom.abs() < 1e-12);
    }
}
