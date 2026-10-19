//! Audio visualisations (Kdenlive's On Master effects): level bars, spectrum, waveform, and the
//! audio-reactive Dance (scale / move by level) and Light Show (colour flash by level).
//!
//! The audio is the sequence Mix around the frame's time, read through [`super::FxEnv`]: a frame
//! shows the window of sound that plays while it is on screen. Without an environment (isolated
//! effect tests, adjustment layers) there is no sound to show and the picture is left unchanged.

use redrob_film_project::EffectInstance;

use crate::effects::{FxCtx, choice, color, f};
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

/// Level (0 … 1) of a frequency band of the mono sum: 0 Full (peak), 1 Low (< 250 Hz),
/// 2 Mid (250 Hz – 4 kHz), 3 High (> 4 kHz).
fn band_level(ch: &[Vec<f32>], sr: f64, band: u32) -> f32 {
    let n = ch[0].len();
    let mono: Vec<f32> = (0..n).map(|i| ch.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / ch.len() as f32).collect();
    if band == 0 {
        return level01(peak(&mono));
    }
    let (lo, hi) = match band {
        1 => (40.0, 250.0),
        2 => (250.0, 4000.0),
        _ => (4000.0, 16000.0f64.min(sr * 0.45)),
    };
    let steps = 6;
    let mut m = 0.0f32;
    for k in 0..steps {
        let fq = lo * (hi / lo).powf(k as f64 / (steps - 1) as f64);
        m = m.max(goertzel(&mono, fq, sr));
    }
    level01(m)
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
    let (w, h) = (img.w as f32, img.h as f32);
    let (xa, xb) = (x0.max(0.0).min(w) as usize, x1.max(0.0).min(w).ceil() as usize);
    let (ya, yb) = (y0.max(0.0).min(h) as usize, y1.max(0.0).min(h).ceil() as usize);
    for y in ya..yb {
        for x in xa..xb {
            let i = (y * img.w + x) * 4;
            over(&mut img.px[i..i + 4], c, a);
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

/// Audio Level Visualization: one bar per channel (peak level on a 60 dB scale), with the part
/// above −6 dBFS in the peak colour.
pub(super) fn levels(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some((ch, _)) = audio(cx, frame_seconds(cx)) else { return };
    let (x, y, bw, bh) = frame_box(img, e, cx);
    let (c, pc) = (color(e, "color", cx), color(e, "peak_color", cx));
    let op = f(e, "opacity", cx) / 100.0;
    let n = ch.len().min(8);
    let vertical = choice(e, "orientation") == 0;
    let knee = level01(0.5);
    for (k, c_data) in ch.iter().take(n).enumerate() {
        let l = level01(peak(c_data));
        if vertical {
            let lw = bw / n as f32;
            let (lx0, lx1) = (x + k as f32 * lw + lw * 0.1, x + (k + 1) as f32 * lw - lw * 0.1);
            let top = y + bh * (1.0 - l);
            let split = y + bh * (1.0 - knee);
            fill_rect(img, lx0, top.max(split), lx1, y + bh, c, op);
            if top < split {
                fill_rect(img, lx0, top, lx1, split, pc, op);
            }
        } else {
            let lh = bh / n as f32;
            let (ly0, ly1) = (y + k as f32 * lh + lh * 0.1, y + (k + 1) as f32 * lh - lh * 0.1);
            let right = x + bw * l;
            let split = x + bw * knee;
            fill_rect(img, x, ly0, right.min(split), ly1, c, op);
            if right > split {
                fill_rect(img, split, ly0, right, ly1, pc, op);
            }
        }
    }
}

/// Audio Spectrum: `bands` log-spaced bars between the lowest and highest frequency, height on
/// a dB scale from the floor to 0 dBFS.
pub(super) fn spectrum(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some((ch, sr)) = audio(cx, frame_seconds(cx).max(0.04)) else { return };
    let (x, y, bw, bh) = frame_box(img, e, cx);
    let bands = (f(e, "bands", cx).round() as usize).clamp(4, 128);
    let lo = f(e, "low_freq", cx).max(10.0) as f64;
    let hi = (f(e, "high_freq", cx) as f64).clamp(lo * 2.0, sr * 0.49);
    let floor = f(e, "floor", cx).min(-1.0);
    let c = color(e, "color", cx);
    let op = f(e, "opacity", cx) / 100.0;
    let n = ch[0].len();
    let mono: Vec<f32> = (0..n).map(|i| ch.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / ch.len() as f32).collect();
    let lw = bw / bands as f32;
    for k in 0..bands {
        let fq = lo * (hi / lo).powf((k as f64 + 0.5) / bands as f64);
        let m = goertzel(&mono, fq, sr);
        let db = 20.0 * m.max(1e-9).log10();
        let l = ((db - floor) / -floor).clamp(0.0, 1.0);
        let (bx0, bx1) = (x + k as f32 * lw + lw * 0.12, x + (k + 1) as f32 * lw - lw * 0.12);
        fill_rect(img, bx0, y + bh * (1.0 - l), bx1, y + bh, c, op);
    }
}

/// Audio Waveform: the Mix's waveform over the last `window` ms, as a line, filled from the
/// centre, or mirrored (rectified, both ways from the centre).
pub(super) fn waveform(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let window = (f(e, "window", cx) as f64 / 1000.0).clamp(0.005, 0.5);
    let Some((ch, _)) = audio(cx, window) else { return };
    let (x, y, bw, bh) = frame_box(img, e, cx);
    let c = color(e, "color", cx);
    let op = f(e, "opacity", cx) / 100.0;
    let th = (f(e, "thickness", cx) * cx.px_scale.max(0.05)).max(1.0);
    let style = choice(e, "style");
    let n = ch[0].len();
    let cols = bw.max(1.0) as usize;
    let mid = y + bh / 2.0;
    let mut prev: Option<f32> = None;
    for col in 0..cols {
        let a = col * n / cols;
        let b = ((col + 1) * n / cols).max(a + 1).min(n);
        let s: Vec<f32> = (a..b).map(|i| ch.iter().map(|c| c[i]).sum::<f32>() / ch.len() as f32).collect();
        let px = x + col as f32;
        match style {
            1 => {
                let (lo, hi) = s.iter().fold((0.0f32, 0.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
                fill_rect(img, px, mid - hi.clamp(-1.0, 1.0) * bh / 2.0, px + 1.0, mid - lo.clamp(-1.0, 1.0) * bh / 2.0 + 1.0, c, op);
            }
            2 => {
                let p = peak(&s).min(1.0);
                fill_rect(img, px, mid - p * bh / 2.0, px + 1.0, mid + p * bh / 2.0 + 1.0, c, op);
            }
            _ => {
                let v = s.iter().sum::<f32>() / s.len().max(1) as f32;
                let yy = mid - v.clamp(-1.0, 1.0) * bh / 2.0;
                let (ya, yb) = match prev {
                    Some(p) => (p.min(yy), p.max(yy)),
                    None => (yy, yy),
                };
                fill_rect(img, px, ya - th / 2.0, px + 1.0, yb + th / 2.0, c, op);
                prev = Some(yy);
            }
        }
    }
}

/// Dance: the picture grows by up to `scale` % and moves by up to `move_x` / `move_y` % with the
/// level of the chosen band (the move alternates direction with the beat of the level).
pub(super) fn dance(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some((ch, sr)) = audio(cx, frame_seconds(cx).max(0.04)) else { return };
    let l = (band_level(&ch, sr, choice(e, "band")) * f(e, "sensitivity", cx) / 100.0).clamp(0.0, 1.0);
    if l <= 0.0 {
        return;
    }
    let s = 1.0 + f(e, "scale", cx) / 100.0 * l;
    let side = if (cx.seconds * 2.0).floor() as i64 % 2 == 0 { 1.0 } else { -1.0 };
    let (w, h) = (img.w as f32, img.h as f32);
    let dx = f(e, "move_x", cx) / 100.0 * w * l * side;
    let dy = -f(e, "move_y", cx) / 100.0 * h * l;
    let src = img.clone();
    let (cx0, cy0) = (w / 2.0, h / 2.0);
    for y in 0..img.h {
        for x in 0..img.w {
            let sx = (x as f32 + 0.5 - cx0 - dx) / s + cx0 - 0.5;
            let sy = (y as f32 + 0.5 - cy0 - dy) / s + cy0 - 0.5;
            let p = if sx < -0.5 || sy < -0.5 || sx > w - 0.5 || sy > h - 0.5 { [0.0; 4] } else { src.sample_bilinear_clamped(sx, sy) };
            let i = (y * img.w + x) * 4;
            img.px[i..i + 4].copy_from_slice(&p);
        }
    }
}

/// Light Show: the picture flashes with `color` (added light, or a tint) in proportion to the
/// level of the chosen band.
pub(super) fn light_show(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some((ch, sr)) = audio(cx, frame_seconds(cx).max(0.04)) else { return };
    let l = (band_level(&ch, sr, choice(e, "band")) * f(e, "sensitivity", cx) / 100.0).clamp(0.0, 1.0);
    let k = l * f(e, "intensity", cx) / 100.0;
    if k <= 0.0 {
        return;
    }
    let c = color(e, "color", cx);
    let add = choice(e, "mode") == 0;
    for p in img.px.chunks_mut(4) {
        let a = p[3];
        if a <= 0.0 {
            continue;
        }
        for i in 0..3 {
            p[i] = if add { p[i] + c[i] * k * a } else { p[i] * (1.0 - k) + c[i] * k * a };
        }
    }
}
