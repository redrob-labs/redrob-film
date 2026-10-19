//! Luma Wipe (Kdenlive's Wipe / Luma composition): a greyscale map decides where the incoming
//! clip appears — the darkest parts first — with a soft edge. The map is one of the built-in
//! wipe methods (spiral, matrix snake, waterfall, burst, cloud, curtain, double iris), every one
//! procedural maths written here, or a greyscale image file the user picks (PGM / PPM, PNG, JPEG,
//! TIFF, …), stretched to the frame.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::sync::{Arc, Mutex, OnceLock};

use super::{Tx, fbm, hash, mix, paint};
use crate::image::Image;

/// `method` choice indices (`redrob_film_project::vtransition::LUMA_METHODS`).
const SPIRAL: u32 = 0;
const MATRIX_SNAKE: u32 = 1;
const WATERFALL: u32 = 2;
const BURST: u32 = 3;
const CLOUD: u32 = 4;
const CURTAIN: u32 = 5;
const DOUBLE_IRIS: u32 = 6;
const IMAGE_FILE: u32 = 7;

pub(crate) fn apply(id: &str, t: &Tx) -> Option<Image> {
    if id != "luma_wipe" {
        return None;
    }
    let soft = t.frac("softness").clamp(0.0, 1.0).max(1e-3);
    let invert = t.flag("invert");
    let method = t.choice("method");
    let file = if method == IMAGE_FILE { load(&t.text("file")) } else { None };
    if method == IMAGE_FILE && file.is_none() {
        // no readable map: a plain dissolve, so the transition still completes
        return Some(paint(t.w, t.h, |x, y| mix(t.pa(x, y), t.pb(x, y), t.p)));
    }
    let (w, h) = (t.wf, t.hf);
    let level = |x: f32, y: f32| -> f32 {
        let (u, v) = (x / w, y / h);
        match method {
            SPIRAL => {
                // three turns from the centre outward: angle within a turn + the turn
                let (dx, dy) = ((u - 0.5) * w / h, v - 0.5);
                let r = (dx * dx + dy * dy).sqrt() / (0.25 * (w / h).powi(2) + 0.25).sqrt();
                let a = (dy.atan2(dx) + PI) / TAU;
                let turns = 3.0;
                let ring = (r * turns - a).floor();
                ((ring + a) / turns).clamp(0.0, 1.0)
            }
            MATRIX_SNAKE => {
                // cells filled row by row, every other row right to left
                let (cols, rows) = (8.0f32, 6.0f32);
                let (i, j) = ((u * cols).floor().min(cols - 1.0), (v * rows).floor().min(rows - 1.0));
                let i = if j as i32 % 2 == 1 { cols - 1.0 - i } else { i };
                (j * cols + i + 0.5) / (cols * rows)
            }
            WATERFALL => {
                // columns falling from the top, each with its own delay
                let cols = 24.0f32;
                let c = (u * cols).floor() as i32;
                hash(c, 7, 31) * 0.45 + v * 0.55
            }
            BURST => {
                // rays reaching out from the centre
                let (dx, dy) = ((u - 0.5) * w / h, v - 0.5);
                let r = (dx * dx + dy * dy).sqrt() / (0.25 * (w / h).powi(2) + 0.25).sqrt();
                let a = dy.atan2(dx);
                (r * (0.7 + 0.3 * (a * 8.0).cos())).clamp(0.0, 1.0)
            }
            CLOUD => {
                // soft value noise
                let s = fbm(u * 5.0 * w / h, v * 5.0, 17);
                ((s - 0.2) / 0.6).clamp(0.0, 1.0)
            }
            CURTAIN => {
                // opens from the middle, the bottom of the curtain a little later
                ((u - 0.5).abs() * 2.0 * 0.85 + v * 0.15).clamp(0.0, 1.0)
            }
            DOUBLE_IRIS => {
                let d = |cx: f32| (((u - cx) * w / h).powi(2) + (v - 0.5).powi(2)).sqrt();
                (d(1.0 / 3.0).min(d(2.0 / 3.0)) / 0.75).clamp(0.0, 1.0)
            }
            _ => file.as_ref().map_or(0.5, |m| m.sample(u, v)),
        }
    };
    let thr = t.p * (1.0 + soft);
    Some(paint(t.w, t.h, |x, y| {
        let l = level(x, y);
        let l = if invert { 1.0 - l } else { l };
        mix(t.pa(x, y), t.pb(x, y), ((thr - l) / soft).clamp(0.0, 1.0))
    }))
}

/// A greyscale map, 0..1 per pixel.
pub(crate) struct Luma {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Luma {
    /// Bilinear sample at (u, v) in 0..1 across and down.
    fn sample(&self, u: f32, v: f32) -> f32 {
        let x = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |x: usize, y: usize| self.v[y * self.w + x];
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
        let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
        top + (bot - top) * fy
    }
}

/// The map in `path`, decoded once per path (and again when the file changes).
fn load(path: &str) -> Option<Arc<Luma>> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    type Cache = Mutex<HashMap<String, (Option<std::time::SystemTime>, Option<Arc<Luma>>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    if let Some((m, l)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(path)
        && *m == modified
    {
        return l.clone();
    }
    let luma = std::fs::read(path).ok().and_then(|bytes| decode(path, &bytes)).map(Arc::new);
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_string(), (modified, luma.clone()));
    luma
}

/// Decode a greyscale map: Netpbm (P2 / P5 grey, P3 / P6 colour) here, any other still image
/// through the media crate. Colour pictures are reduced to their luma.
pub(crate) fn decode(name: &str, bytes: &[u8]) -> Option<Luma> {
    if let Some(l) = netpbm(bytes) {
        return Some(l);
    }
    let r = redrob_film_media::still::decode_raster(name, bytes).ok()?;
    let (w, h) = (r.width as usize, r.height as usize);
    if w == 0 || h == 0 || r.rgba.len() < w * h * 4 {
        return None;
    }
    let v = r.rgba.as_chunks::<4>().0.iter().map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0).collect();
    Some(Luma { w, h, v })
}

/// Netpbm grey / colour maps (`P2`, `P3` text; `P5`, `P6` binary, 8 or 16 bits).
fn netpbm(b: &[u8]) -> Option<Luma> {
    if b.len() < 3 || b[0] != b'P' || !matches!(b[1], b'2' | b'3' | b'5' | b'6') {
        return None;
    }
    let kind = b[1];
    // header: magic, width, height, maxval, with # comments
    let mut i = 2;
    let mut nums = Vec::new();
    while nums.len() < 3 {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'#') {
            if b[i] == b'#' {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        nums.push(std::str::from_utf8(&b[start..i]).ok()?.parse::<usize>().ok()?);
    }
    let (w, h, max) = (nums[0], nums[1], nums[2].max(1));
    if w == 0 || h == 0 || w.checked_mul(h)? > 1 << 28 {
        return None;
    }
    let channels = if matches!(kind, b'3' | b'6') { 3 } else { 1 };
    let n = w * h * channels;
    let samples: Vec<f32> = if matches!(kind, b'5' | b'6') {
        // one whitespace byte, then the raster
        let data = b.get(i + 1..)?;
        if max < 256 {
            data.get(..n)?.iter().map(|&x| x as f32 / max as f32).collect()
        } else {
            data.get(..n * 2)?.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]]) as f32 / max as f32).collect()
        }
    } else {
        let text = std::str::from_utf8(&b[i..]).ok()?;
        let v: Vec<f32> = text.split_ascii_whitespace().take(n).filter_map(|s| s.parse::<f32>().ok()).map(|x| x / max as f32).collect();
        if v.len() < n {
            return None;
        }
        v
    };
    let v = if channels == 3 { samples.as_chunks::<3>().0.iter().map(|c| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]).collect() } else { samples };
    Some(Luma { w, h, v })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netpbm_maps_decode() {
        let p2 = b"P2\n# a ramp\n3 1\n255\n0 128 255\n";
        let l = netpbm(p2).unwrap();
        assert_eq!((l.w, l.h), (3, 1));
        assert!((l.v[1] - 128.0 / 255.0).abs() < 1e-6);
        let mut p5 = b"P5 2 2 255\n".to_vec();
        p5.extend([0u8, 255, 255, 0]);
        let l = netpbm(&p5).unwrap();
        assert_eq!(l.v, vec![0.0, 1.0, 1.0, 0.0]);
        assert!((l.sample(0.0, 0.0)).abs() < 1e-6);
        assert!(netpbm(b"P5 2 2 255\n\x00").is_none(), "short raster");
    }
}
