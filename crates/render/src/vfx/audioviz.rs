//! Audio visualisations (Kdenlive's On Master effects): level meter, spectrum, waveform, and the
//! audio-reactive Dance (zoom / move / rotate by level) and Light Show (colour by level).
//!
//! The audio is the sequence Mix around the frame's time, read through [`super::FxEnv`]: a frame
//! shows the window of sound that plays while it is on screen. Clips and adjustment layers both
//! carry the environment, so an adjustment layer over the whole timeline applies them to the
//! master picture. Without an environment (isolated effect tests) there is no sound to show and
//! the picture is left unchanged.
//!
//! The three meters draw into their box on a transparent overlay that is rotated by Angle about
//! the box centre and laid over the picture; a Background Color with alpha fills the frame first.

use redrob_film_geom::Vec2;
use redrob_film_project::EffectInstance;

use super::{bv, chv, cv};
use crate::effects::{FxCtx, f};
use crate::image::Image;

/// The Mix for the frame at `cx`: `seconds` of stereo audio centred on the frame (sample rate).
fn audio(cx: &FxCtx, seconds: f64) -> Option<(Vec<Vec<f32>>, f64)> {
    let env = cx.env?;
    let b = env.sequence_audio(-seconds / 2.0, seconds)?;
    if b.channels.is_empty() || b.channels[0].is_empty() {
        return None;
    }
    Some((b.channels, b.sample_rate as f64))
}

fn frame_seconds(cx: &FxCtx) -> f64 {
    cx.env.map_or(1.0 / 25.0, |e| 1.0 / e.frame_rate().max(1.0))
}

fn peak(c: &[f32]) -> f32 {
    c.iter().fold(0.0f32, |m, x| m.max(x.abs()))
}

/// 0 … 1 from a linear level on a 60 dB scale.
fn level01(x: f32) -> f32 {
    ((20.0 * x.max(1e-6).log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Magnitude of `freq` in `x` (Goertzel, Hann-windowed), normalised to a full-scale sine ≈ 1.
fn goertzel(x: &[f32], freq: f64, sr: f64) -> f32 {
    let n = x.len();
    if n < 2 {
        return 0.0;
    }
    let w = 2.0 * std::f64::consts::PI * freq / sr;
    let k = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for (i, v) in x.iter().enumerate() {
        let win = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
        let s0 = *v as f64 * win + k * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let p = s1 * s1 + s2 * s2 - k * s1 * s2;
    (p.max(0.0).sqrt() * 4.0 / n as f64) as f32
}

fn mono(ch: &[Vec<f32>]) -> Vec<f32> {
    let n = ch[0].len();
    (0..n).map(|i| ch.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / ch.len() as f32).collect()
}

/// Level in dBFS of a frequency band of the mono sum: 0 Full (peak), 1 Low (< 250 Hz),
/// 2 Mid (250 Hz – 4 kHz), 3 High (> 4 kHz), 4 the custom `lo` … `hi` range.
fn band_db(ch: &[Vec<f32>], sr: f64, band: u32, lo_hi: (f64, f64)) -> f32 {
    let m = mono(ch);
    if band == 0 {
        return 20.0 * peak(&m).max(1e-6).log10();
    }
    let (lo, hi) = match band {
        1 => (40.0, 250.0),
        2 => (250.0, 4000.0),
        3 => (4000.0, 16000.0f64.min(sr * 0.45)),
        _ => (lo_hi.0.max(10.0), lo_hi.1.clamp(lo_hi.0.max(10.0) * 1.01, sr * 0.49)),
    };
    let steps = 8;
    let mut mx = 0.0f32;
    for k in 0..steps {
        let fq = lo * (hi / lo).powf(k as f64 / (steps - 1) as f64);
        mx = mx.max(goertzel(&m, fq, sr));
    }
    20.0 * mx.max(1e-6).log10()
}

/// The 0 … 1 drive of Dance / Light Show: band level above Level Threshold (dB), scaled by
/// sensitivity, modulated by Oscillation (Hz, 0 = none). `None` without sound.
fn drive(e: &EffectInstance, cx: &FxCtx) -> Option<f32> {
    let fs = frame_seconds(cx).max(0.04);
    let (probe, sr) = audio(cx, fs)?;
    let window = (f(e, "window", cx).clamp(256.0, 16384.0) as f64 / sr).max(fs);
    let (ch, sr) = if window > fs + 1e-6 { audio(cx, window)? } else { (probe, sr) };
    let thr = f(e, "threshold", cx).min(-1.0);
    let db = band_db(&ch, sr, chv(e, "band"), (f(e, "low_freq", cx) as f64, f(e, "high_freq", cx) as f64));
    let l = ((db - thr) / -thr).clamp(0.0, 1.0) * f(e, "sensitivity", cx) / 100.0;
    let osc = f(e, "oscillation", cx);
    let m = if osc > 0.0 { (2.0 * std::f64::consts::PI * osc as f64 * cx.seconds).sin() as f32 } else { 1.0 };
    Some(l.clamp(0.0, 1.0) * m)
}

/// Blend `c` (straight RGB) over premultiplied pixel `p` with coverage `a`.
#[inline]
fn over(p: &mut [f32], c: [f32; 4], a: f32) {
    let a = (a * c[3]).clamp(0.0, 1.0);
    for k in 0..3 {
        p[k] = p[k] * (1.0 - a) + c[k] * a;
    }
    p[3] = p[3] * (1.0 - a) + a;
}

fn fill_rect(img: &mut Image, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 4], a: f32) {
    fill_rect_with(img, x0, y0, x1, y1, a, |_, _| c);
}

/// Fill a rectangle with a per-pixel colour `c(x, y)`.
fn fill_rect_with(img: &mut Image, x0: f32, y0: f32, x1: f32, y1: f32, a: f32, c: impl Fn(f32, f32) -> [f32; 4]) {
    let (w, h) = (img.w as f32, img.h as f32);
    let (xa, xb) = (x0.min(x1).max(0.0).min(w) as usize, x0.max(x1).max(0.0).min(w).ceil() as usize);
    let (ya, yb) = (y0.min(y1).max(0.0).min(h) as usize, y0.max(y1).max(0.0).min(h).ceil() as usize);
    for y in ya..yb {
        for x in xa..xb {
            let i = (y * img.w + x) * 4;
            over(&mut img.px[i..i + 4], c(x as f32 + 0.5, y as f32 + 0.5), a);
        }
    }
}

/// The effect's box in image pixels: (left, top, width, height) from centre X/Y and size in %.
fn frame_box(img: &Image, e: &EffectInstance, cx: &FxCtx) -> (f32, f32, f32, f32) {
    let (w, h) = (img.w as f32, img.h as f32);
    let bw = f(e, "width", cx) / 100.0 * w;
    let bh = f(e, "height", cx) / 100.0 * h;
    (f(e, "x", cx) / 100.0 * w - bw / 2.0, f(e, "y", cx) / 100.0 * h - bh / 2.0, bw, bh)
}

/// Background, then the overlay drawn by `draw` (rotated by Angle about the box centre) over
/// the picture.
fn meter(img: &mut Image, e: &EffectInstance, cx: &FxCtx, draw: impl FnOnce(&mut Image, (f32, f32, f32, f32))) {
    let bg = cv(e, "bg_color", cx);
    if bg[3] > 0.0 {
        let (w, h) = (img.w as f32, img.h as f32);
        fill_rect(img, 0.0, 0.0, w, h, bg, 1.0);
    }
    let bx = frame_box(img, e, cx);
    let mut layer = Image::new(img.w, img.h);
    draw(&mut layer, bx);
    let angle = f(e, "angle", cx);
    if angle.abs() > 1e-3 {
        let c = Vec2::new((bx.0 + bx.2 / 2.0) as f64, (bx.1 + bx.3 / 2.0) as f64);
        super::place(&mut layer, c, Vec2::ZERO, 1.0, angle as f64);
    }
    for (p, s) in img.px.chunks_mut(4).zip(layer.px.chunks(4)) {
        let k = 1.0 - s[3];
        for i in 0..4 {
            p[i] = s[i] + p[i] * k;
        }
    }
}

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * t)
}

/// Audio Level Visualization: one bar (or a column of segments) per channel at its peak level
/// on a 60 dB scale, coloured by the three-colour gradient (Color 1 top, 2 middle, 3 bottom,
/// vertical or horizontal); Mirror grows the bars both ways from the centre line, Reverse starts
/// with the last channel.
pub(super) fn levels(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some((ch, _)) = audio(cx, frame_seconds(cx)) else { return };
    let (c1, c2, c3) = (cv(e, "peak_color", cx), cv(e, "mid_color", cx), cv(e, "color", cx));
    let op = f(e, "opacity", cx) / 100.0;
    let n = (f(e, "channels", cx).round() as usize).clamp(1, 8).min(ch.len().max(1));
    let vertical = chv(e, "orientation") == 0;
    let grad_vertical = chv(e, "gradient") == 0;
    let segments = chv(e, "graph_type") == 1;
    let (mirror, reverse) = (bv(e, "mirror"), bv(e, "reverse"));
    let thick = f(e, "thickness", cx) * cx.px_scale;
    let gap = (f(e, "segment_gap", cx) * cx.px_scale).max(0.0);
    meter(img, e, cx, |layer, (x, y, bw, bh)| {
        // gradient position 0 (bottom / left) … 1 (top / right) for an image point
        let grad = |px: f32, py: f32| {
            let t = if grad_vertical { 1.0 - (py - y) / bh.max(1.0) } else { (px - x) / bw.max(1.0) };
            let t = t.clamp(0.0, 1.0);
            if t < 0.5 { lerp4(c3, c2, t * 2.0) } else { lerp4(c2, c1, (t - 0.5) * 2.0) }
        };
        for k in 0..n {
            let src = if reverse { n - 1 - k } else { k };
            let l = ch.get(src).map_or(0.0, |c| level01(peak(c)));
            let lane = (if vertical { bw } else { bh }) / n as f32;
            let size = if thick > 0.5 { thick.min(lane) } else { lane * 0.8 };
            let a0 = (if vertical { x } else { y }) + k as f32 * lane + (lane - size) / 2.0;
            let (len_total, base) = if vertical { (bh, y + bh) } else { (bw, x) };
            // the extent along the level axis: [from, to] measured from the base
            let (from, to) = if mirror { ((1.0 - l) * len_total / 2.0, (1.0 + l) * len_total / 2.0) } else { (0.0, l * len_total) };
            let seg = if segments { size.max(2.0) } else { len_total };
            let mut s = from;
            while s < to {
                let end = if segments { (s + seg - gap).min(to) } else { to };
                if vertical {
                    fill_rect_with(layer, a0, base - end, a0 + size, base - s, op, &grad);
                } else {
                    fill_rect_with(layer, base + s, a0, base + end, a0 + size, op, &grad);
                }
                s += seg;
                if !segments {
                    break;
                }
            }
        }
    });
}

/// Audio Spectrum: `Points` log-spaced bands between the low and high frequency, height on a dB
/// scale from Level Threshold to 0 dBFS, drawn as bars or as a line (Catmull-Rom with Line
/// Tension, optionally filled underneath); Mirror reflects about the centre line, Reverse puts
/// the highest frequency first.
pub(super) fn spectrum(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some((ch, sr)) = audio(cx, frame_seconds(cx).max(0.04)) else { return };
    let bands = (f(e, "bands", cx).round() as usize).clamp(4, 128);
    let lo = f(e, "low_freq", cx).max(10.0) as f64;
    let hi = (f(e, "high_freq", cx) as f64).clamp(lo * 2.0, sr * 0.49);
    let floor = f(e, "floor", cx).min(-1.0);
    let c = cv(e, "color", cx);
    let op = f(e, "opacity", cx) / 100.0;
    let m = mono(&ch);
    let mut lv: Vec<f32> = (0..bands)
        .map(|k| {
            let fq = lo * (hi / lo).powf((k as f64 + 0.5) / bands as f64);
            let db = 20.0 * goertzel(&m, fq, sr).max(1e-9).log10();
            ((db - floor) / -floor).clamp(0.0, 1.0)
        })
        .collect();
    if bv(e, "reverse") {
        lv.reverse();
    }
    let line = chv(e, "graph_type") == 0;
    let (fill, mirror) = (bv(e, "fill"), bv(e, "mirror"));
    let thick = (f(e, "thickness", cx) * cx.px_scale).max(0.0);
    let tension = f(e, "tension", cx) / 100.0;
    meter(img, e, cx, |layer, (x, y, bw, bh)| {
        // vertical span of a value: from the base (bottom, or the centre when mirrored)
        let span = |v: f32| if mirror { (y + bh / 2.0 - v * bh / 2.0, y + bh / 2.0 + v * bh / 2.0) } else { (y + bh * (1.0 - v), y + bh) };
        if !line {
            let lw = bw / bands as f32;
            for (k, v) in lv.iter().enumerate() {
                let size = if thick > 0.5 { thick.min(lw) } else { lw * 0.76 };
                let bx0 = x + k as f32 * lw + (lw - size) / 2.0;
                let (t, b) = span(*v);
                fill_rect(layer, bx0, t, bx0 + size, b, c, op);
            }
            return;
        }
        // Catmull-Rom through the band centres; tension scales the tangents
        let pt = |i: isize| lv[i.clamp(0, bands as isize - 1) as usize];
        let value_at = |u: f32| {
            let s = (u * bands as f32 - 0.5).clamp(0.0, bands as f32 - 1.0);
            let i = s.floor() as isize;
            let t = s - i as f32;
            let (p0, p1, p2, p3) = (pt(i - 1), pt(i), pt(i + 1), pt(i + 2));
            let (m1, m2) = ((p2 - p0) * tension, (p3 - p1) * tension);
            let (t2, t3) = (t * t, t * t * t);
            ((2.0 * t3 - 3.0 * t2 + 1.0) * p1 + (t3 - 2.0 * t2 + t) * m1 + (-2.0 * t3 + 3.0 * t2) * p2 + (t3 - t2) * m2).clamp(0.0, 1.0)
        };
        let th = thick.max(1.0);
        let cols = bw.max(1.0) as usize;
        let mut prev: Option<(f32, f32)> = None;
        for col in 0..cols {
            let px = x + col as f32;
            let v = value_at((col as f32 + 0.5) / cols as f32);
            let (top, bottom) = span(v);
            if fill {
                fill_rect(layer, px, top, px + 1.0, bottom, c, op);
            }
            let (pt_, pb) = prev.unwrap_or((top, bottom));
            fill_rect(layer, px, pt_.min(top) - th / 2.0, px + 1.0, pt_.max(top) + th / 2.0, c, op);
            if mirror {
                fill_rect(layer, px, pb.min(bottom) - th / 2.0, px + 1.0, pb.max(bottom) + th / 2.0, c, op);
            }
            prev = Some((top, bottom));
        }
    });
}

/// Audio Waveform: the Mix's waveform over the last `window` ms, as a line, filled from the
/// centre, or mirrored (rectified, both ways from the centre). Channel to Draw: All (one lane per
/// channel), Merge (the mono sum) or a single channel.
pub(super) fn waveform(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let window = (f(e, "window", cx) as f64 / 1000.0).clamp(0.005, 0.5);
    let Some((ch, _)) = audio(cx, window) else { return };
    let c = cv(e, "color", cx);
    let op = f(e, "opacity", cx) / 100.0;
    let th = (f(e, "thickness", cx) * cx.px_scale.max(0.05)).max(1.0);
    let style = chv(e, "style");
    let lanes: Vec<Vec<f32>> = match chv(e, "channel") {
        0 => ch.clone(),
        1 => vec![mono(&ch)],
        k => vec![ch.get(k as usize - 2).cloned().unwrap_or_else(|| vec![0.0; ch[0].len()])],
    };
    meter(img, e, cx, |layer, (x, y, bw, bh)| {
        let lh = bh / lanes.len().max(1) as f32;
        for (li, s_all) in lanes.iter().enumerate() {
            let n = s_all.len();
            let cols = bw.max(1.0) as usize;
            let mid = y + lh * (li as f32 + 0.5);
            let mut prev: Option<f32> = None;
            for col in 0..cols {
                let a = col * n / cols;
                let b = ((col + 1) * n / cols).max(a + 1).min(n);
                if a >= b {
                    continue;
                }
                let s = &s_all[a..b];
                let px = x + col as f32;
                match style {
                    1 => {
                        let (lo, hi) = s.iter().fold((0.0f32, 0.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
                        fill_rect(layer, px, mid - hi.clamp(-1.0, 1.0) * lh / 2.0, px + 1.0, mid - lo.clamp(-1.0, 1.0) * lh / 2.0 + 1.0, c, op);
                    }
                    2 => {
                        let p = peak(s).min(1.0);
                        fill_rect(layer, px, mid - p * lh / 2.0, px + 1.0, mid + p * lh / 2.0 + 1.0, c, op);
                    }
                    _ => {
                        let v = s.iter().sum::<f32>() / s.len().max(1) as f32;
                        let yy = mid - v.clamp(-1.0, 1.0) * lh / 2.0;
                        let (ya, yb) = match prev {
                            Some(p) => (p.min(yy), p.max(yy)),
                            None => (yy, yy),
                        };
                        fill_rect(layer, px, ya - th / 2.0, px + 1.0, yb + th / 2.0, c, op);
                        prev = Some(yy);
                    }
                }
            }
        }
    });
}

/// Dance: zooms (Initial Zoom, plus Zoom × level; negative zooms out), moves (Right − Left,
/// Down − Up, % of the frame) and rotates (Clockwise − Counter Clockwise) the picture in
/// proportion to the level of the chosen frequency range.
pub(super) fn dance(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some(l) = drive(e, cx) else { return };
    let s = (f(e, "initial_zoom", cx) / 100.0 + f(e, "scale", cx) / 100.0 * l).max(0.01);
    let (w, h) = (img.w as f32, img.h as f32);
    let dx = (f(e, "move_x", cx) - f(e, "move_left", cx)) / 100.0 * w * l;
    let dy = (f(e, "move_down", cx) - f(e, "move_y", cx)) / 100.0 * h * l;
    let rot = (f(e, "rotate_cw", cx) - f(e, "rotate_ccw", cx)) * l;
    if (s - 1.0).abs() < 1e-5 && dx.abs() < 1e-3 && dy.abs() < 1e-3 && rot.abs() < 1e-3 {
        return;
    }
    super::place(img, Vec2::new(w as f64 / 2.0, h as f64 / 2.0), Vec2::new(dx as f64, dy as f64), s as f64, rot as f64);
}

/// Light Show: the picture is lit (Add) or tinted (Tint) with the gradient from the 1st colour
/// (top) to the 2nd (bottom), in proportion to the level of the chosen frequency range.
pub(super) fn light_show(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some(l) = drive(e, cx) else { return };
    let k = l.abs() * f(e, "intensity", cx) / 100.0;
    if k <= 0.0 {
        return;
    }
    let (c1, c2) = (cv(e, "color", cx), cv(e, "color2", cx));
    let add = chv(e, "mode") == 0;
    let (w, h) = (img.w, img.h.max(1));
    for (i, p) in img.px.chunks_mut(4).enumerate() {
        let a = p[3];
        if a <= 0.0 {
            continue;
        }
        let t = ((i / w.max(1)) as f32 + 0.5) / h as f32;
        let c = lerp4(c1, c2, t);
        for j in 0..3 {
            p[j] = if add { p[j] + c[j] * k * a } else { p[j] * (1.0 - k) + c[j] * k * a };
        }
    }
}
