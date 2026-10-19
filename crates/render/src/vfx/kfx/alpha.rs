//! Alpha, mask and keying effects of the bin (`redrob_film_project::effect::kdx`): gradient and
//! morphological alpha operations, strobing, background / HSV keys, key-spill clean-up,
//! premultiplication, file mattes, the selections that start a mask chain, Spot Remover and the
//! Motion Tracker's obscuring shapes.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use redrob_film_geom::Vec2;
use redrob_film_media::MediaSource;
use redrob_film_project::MaskMode;

use super::*;
use crate::vfx::{over, smoothstep};

// ------------------------------------------------------------------ alpha plane helpers

/// Set a premultiplied pixel's alpha to `a`, keeping its straight colour (black where it had none).
#[inline]
pub(crate) fn set_alpha(p: &mut [f32], a: f32) {
    let a = a.clamp(0.0, 1.0);
    if p[3] > 1e-6 {
        let k = a / p[3];
        p[0] *= k;
        p[1] *= k;
        p[2] *= k;
    } else {
        p[0] = 0.0;
        p[1] = 0.0;
        p[2] = 0.0;
    }
    p[3] = a;
}

/// Replace the picture's alpha by `plane`.
pub(crate) fn write_alpha(img: &mut Image, plane: &[f32]) {
    img.px.par_chunks_mut(4).zip(plane.par_iter()).for_each(|(p, &a)| set_alpha(p, a));
}

/// Separable square min (`max = false`) or max filter of radius `r`.
pub(crate) fn minmax_plane(p: &[f32], w: usize, h: usize, r: usize, max: bool) -> Vec<f32> {
    if r == 0 || w == 0 || h == 0 {
        return p.to_vec();
    }
    let pick = |a: f32, b: f32| if max { a.max(b) } else { a.min(b) };
    let ri = r as isize;
    let mut tmp = vec![0.0f32; p.len()];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &p[y * w..(y + 1) * w];
        for x in 0..w {
            let mut v = s[x];
            for k in -ri..=ri {
                v = pick(v, s[(x as isize + k).clamp(0, w as isize - 1) as usize]);
            }
            row[x] = v;
        }
    });
    let mut out = vec![0.0f32; p.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut v = tmp[y * w + x];
            for k in -ri..=ri {
                v = pick(v, tmp[(y as isize + k).clamp(0, h as isize - 1) as usize * w + x]);
            }
            row[x] = v;
        }
    });
    out
}

/// Straight display RGB → (hue 0..1, saturation, value).
#[inline]
pub(crate) fn hsv(c: [f32; 3]) -> (f32, f32, f32) {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    let d = mx - mn;
    let h = if d <= 1e-6 {
        0.0
    } else if mx == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0) / 6.0
    } else if mx == c[1] {
        ((c[2] - c[0]) / d + 2.0) / 6.0
    } else {
        ((c[0] - c[1]) / d + 4.0) / 6.0
    };
    (h, if mx <= 1e-6 { 0.0 } else { d / mx }, mx)
}

/// Shortest distance between two hues (turns), 0..0.5.
#[inline]
fn hue_dist(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(1.0);
    d.min(1.0 - d)
}

#[inline]
fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

// ------------------------------------------------------------------ Alpha Gradient

/// Alpha Gradient: a linear opacity ramp (tilted, positioned, with a transition width) combined
/// with the picture's alpha by the chosen operation.
pub fn alpha_gradient(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (iw, ih) = (img.w, img.h);
    let (w, h) = (iw as f32, ih as f32);
    let op = pick(e, "operation");
    let pos = fv(e, "position", cx) / 100.0;
    let width = (fv(e, "width", cx) / 100.0).max(0.0);
    let (s, c) = fv(e, "tilt", cx).to_radians().sin_cos();
    let a0 = (1.0 - fv(e, "transparency_top", cx) / 100.0).clamp(0.0, 1.0);
    let a1 = (1.0 - fv(e, "transparency_bottom", cx) / 100.0).clamp(0.0, 1.0);
    // the ramp runs down the frame at tilt 0, turned clockwise by the tilt
    let (dx, dy) = (-s, c);
    let extent = (w * s.abs() + h * c.abs()).max(1.0);
    img.px.par_chunks_mut(iw * 4).enumerate().for_each(|(y, row)| {
        for x in 0..iw {
            let d = ((x as f32 + 0.5 - w / 2.0) * dx + (y as f32 + 0.5 - h / 2.0) * dy) / extent + 0.5;
            let t = if width <= 1e-4 { if d < pos { 0.0 } else { 1.0 } } else { ((d - pos) / width + 0.5).clamp(0.0, 1.0) };
            let g = a0 + (a1 - a0) * t;
            let p = &mut row[x * 4..x * 4 + 4];
            let a = p[3];
            let na = match op {
                1 => a + g,
                2 => a.min(g),
                3 => a.max(g),
                4 => a - g,
                _ => g,
            };
            set_alpha(p, na);
        }
    });
}

// ------------------------------------------------------------------ Alpha Operations

/// Alpha Operations: invert, shave / shrink / grow (hard or soft), threshold or blur the alpha
/// channel, and optionally show it for checking.
pub fn alpha_ops(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (w, h) = (img.w, img.h);
    let input = alpha_of_img(img);
    let mut a = input.clone();
    if flag(e, "invert") {
        a.iter_mut().for_each(|v| *v = 1.0 - *v);
    }
    let r = (fv(e, "amount", cx) * cx.px_scale).round().max(0.0) as usize;
    let r1 = r.max(1);
    a = match pick(e, "operation") {
        1 => {
            // shave: pull every pixel down to its neighbourhood mean, once per step
            let mut p = a;
            for _ in 0..r1.min(16) {
                let m = blur_plane_box(&p, w, h, 1);
                p = p.iter().zip(&m).map(|(v, m)| v.min(*m)).collect();
            }
            p
        }
        2 => minmax_plane(&a, w, h, r, false),
        3 => {
            let b = blur_plane_box(&a, w, h, r);
            a.iter().zip(&b).map(|(v, b)| v.min(*b)).collect()
        }
        4 => minmax_plane(&a, w, h, r, true),
        5 => {
            let b = blur_plane_box(&a, w, h, r);
            a.iter().zip(&b).map(|(v, b)| v.max(*b)).collect()
        }
        6 => {
            let th = fv(e, "threshold", cx) / 100.0;
            a.iter().map(|v| if *v >= th { 1.0 } else { 0.0 }).collect()
        }
        7 => blur_plane_box(&a, w, h, r),
        _ => a,
    };
    let display = pick(e, "display");
    if display == 0 {
        write_alpha(img, &a);
        return;
    }
    let shown = if flag(e, "display_input") { &input } else { &a };
    let mut d = Disp::of(img);
    d.px.par_iter_mut().enumerate().for_each(|(i, p)| {
        let al = shown[i].clamp(0.0, 1.0);
        let (x, y) = (i % w, i / w);
        let o = match display {
            1 => [al, al, al],
            2 => [al + (1.0 - al) * 0.8, al, al],
            _ => {
                let bg = match display {
                    3 => 0.0,
                    4 => 0.5,
                    5 => 1.0,
                    _ => {
                        if ((x / 8) + (y / 8)) % 2 == 0 {
                            0.4
                        } else {
                            0.6
                        }
                    }
                };
                [0, 1, 2].map(|k| p[k] * al + bg * (1.0 - al))
            }
        };
        *p = [o[0], o[1], o[2], 1.0];
    });
    d.store(img);
}

fn alpha_of_img(img: &Image) -> Vec<f32> {
    img.px.par_chunks(4).map(|p| p[3]).collect()
}

// ------------------------------------------------------------------ Alpha Strobing

/// Alpha Strobing: the picture's alpha drops to 0 on alternate stretches of frames (an interval of
/// 1 alternates every frame, 100 every 50 frames).
pub fn alpha_strobe(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let half = (fv(e, "interval", cx).max(1.0) / 2.0).round().max(1.0) as i64;
    let mut off = frame_no(cx).div_euclid(half) % 2 == 1;
    if flag(e, "invert") {
        off = !off;
    }
    if off {
        img.px.fill(0.0);
    }
}

// ------------------------------------------------------------------ Backgroundkey

/// Backgroundkey: pixels similar to the clip's first frame (the static background) become
/// transparent; a frame that differs from it as a whole by more than Threshold (a scene change)
/// is left alone.
pub fn backgroundkey(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some(env) = cx.env else { return };
    let back = -(env.clip_offset() * fps(cx));
    let Some(bg) = neighbour(img, e, cx, back) else { return };
    let (cur, bgd) = (Disp::of(img), Disp::of(&bg));
    let diffs: Vec<f32> =
        cur.px.par_iter().zip(bgd.px.par_iter()).map(|(a, b)| ((a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()) / 3.0).collect();
    let mean = diffs.iter().sum::<f32>() / diffs.len().max(1) as f32;
    if mean > fv(e, "threshold", cx) {
        return;
    }
    let sim = fv(e, "similarity", cx).max(0.0);
    let blend = fv(e, "blend", cx).max(0.0);
    img.px.par_chunks_mut(4).zip(diffs.par_iter()).for_each(|(p, &d)| {
        let k = if d < sim {
            0.0
        } else if blend > 1e-6 {
            ((d - sim) / blend).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let a = p[3] * k;
        set_alpha(p, a);
    });
}

// ------------------------------------------------------------------ HSV Keying

/// HSV Keying: transparency where the pixel's colour (hue and saturation as a vector, plus value)
/// is within Similarity of the key; Blend softens the edge.
pub fn hsv_key(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let kh = fv(e, "hue", cx).to_radians();
    let ks = fv(e, "saturation", cx);
    let kv = fv(e, "value", cx);
    let key = [ks * kh.cos(), ks * kh.sin(), kv];
    let sim = fv(e, "similarity", cx) / 100.0;
    let blend = fv(e, "blend", cx) / 100.0;
    img.px.par_chunks_mut(4).for_each(|p| {
        let a = p[3];
        if a <= 1e-6 {
            return;
        }
        let c = crate::effects::enc([p[0] / a, p[1] / a, p[2] / a]);
        let (hh, s, v) = hsv([c[0].min(1.0), c[1].min(1.0), c[2].min(1.0)]);
        let ang = hh * std::f32::consts::TAU;
        let q = [s * ang.cos(), s * ang.sin(), v];
        let d = dist3(q, key) / 2.0;
        let k = if d < sim {
            0.0
        } else if blend > 1e-6 {
            ((d - sim) / blend).clamp(0.0, 1.0)
        } else {
            1.0
        };
        set_alpha(p, a * k);
    });
}

// ------------------------------------------------------------------ Key Spill Mop Up

/// Key Spill Mop Up: a mask (colour distance from the key, partial transparency, or the inner /
/// outer edge of the opaque part) pruned by a hue gate and a saturation threshold, through which
/// up to two operations pull the key colour out of the picture.
pub fn spill_mopup(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (w, h) = (img.w, img.h);
    let mut d = Disp::of(img);
    let kc = color(e, "key", cx);
    let key = [kc[0], kc[1], kc[2]];
    let tc = color(e, "target", cx);
    let target = [tc[0], tc[1], tc[2]];
    let tol = fv(e, "tolerance", cx) / 100.0;
    let slope = fv(e, "slope", cx) / 100.0;
    let gate = fv(e, "hue_gate", cx) / 100.0;
    let sat_th = fv(e, "sat_threshold", cx) / 100.0;
    let amounts = [fv(e, "amount1", cx) / 100.0, fv(e, "amount2", cx) / 100.0];
    let ops = [pick(e, "op1"), pick(e, "op2")];
    let mask_type = pick(e, "mask_type");
    // edge masks: distance to the opaque part's border, as a blurred opaque plane
    let edge = if mask_type >= 2 {
        let o: Vec<f32> = d.px.iter().map(|p| if p[3] > 0.999 { 1.0 } else { 0.0 }).collect();
        let r = (fv(e, "tolerance", cx) * cx.px_scale).round().max(1.0) as usize;
        Some(blur_plane_box(&o, w, h, r))
    } else {
        None
    };
    let (key_h, _, _) = hsv(key);
    // the key's dominant channel (the one De-Key pulls down)
    let ki = if key[1] >= key[0] && key[1] >= key[2] {
        1
    } else if key[2] >= key[0] {
        2
    } else {
        0
    };
    let show = flag(e, "show_mask");
    let to_alpha = flag(e, "mask_to_alpha");
    d.px.par_iter_mut().enumerate().for_each(|(i, p)| {
        let a = p[3];
        if a <= 0.0 {
            return;
        }
        let c = [p[0], p[1], p[2]];
        let mut m = match mask_type {
            1 => {
                if a >= 0.999 {
                    0.0
                } else {
                    ((1.0 - a) * (1.0 + tol * 4.0)).min(1.0)
                }
            }
            2 => edge.as_ref().map_or(0.0, |b| if a > 0.999 { ((1.0 - b[i]) * 2.0).clamp(0.0, 1.0) } else { 0.0 }),
            3 => edge.as_ref().map_or(0.0, |b| if a <= 0.999 { (b[i] * 2.0).clamp(0.0, 1.0) } else { 0.0 }),
            _ => {
                let dd = dist3(c, key) / 3f32.sqrt();
                if dd <= tol {
                    1.0
                } else if slope > 1e-6 {
                    (1.0 - (dd - tol) / slope).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            }
        };
        let (ph, ps, _) = hsv(c);
        if gate > 0.0 {
            m *= (1.0 - hue_dist(ph, key_h) * 2.0 * gate * 4.0).clamp(0.0, 1.0);
        }
        if sat_th > 0.0 {
            m *= smoothstep(sat_th - 0.05, sat_th + 0.05, ps);
        }
        let mut o = c;
        for (op, amt) in ops.iter().zip(amounts) {
            let k = m * amt;
            if k <= 0.0 {
                continue;
            }
            match *op {
                1 => {
                    let others = [0, 1, 2].into_iter().filter(|j| *j != ki).map(|j| o[j]).fold(0.0f32, f32::max);
                    let excess = o[ki] - others;
                    if excess > 0.0 {
                        o[ki] -= excess * k;
                    }
                }
                2 => o = [0, 1, 2].map(|j| o[j] + (target[j] - o[j]) * k),
                3 => {
                    let l = 0.2126 * o[0] + 0.7152 * o[1] + 0.0722 * o[2];
                    o = o.map(|v| v + (l - v) * k);
                }
                4 => o = o.map(|v| (v + (amt - 0.5) * m).clamp(0.0, 1.0)),
                _ => {}
            }
        }
        *p = if show { [m, m, m, 1.0] } else { [o[0], o[1], o[2], if to_alpha { m } else { a }] };
    });
    d.store(img);
}

// ------------------------------------------------------------------ Premultiply

/// Premultiply or Unpremultiply: multiply (or divide) the colour by the alpha, in display values.
pub fn premultiply(img: &mut Image, e: &EffectInstance) {
    let un = flag(e, "unpremultiply");
    let mut d = Disp::of(img);
    d.px.par_iter_mut().for_each(|p| {
        let a = p[3];
        for k in 0..3 {
            p[k] = if un { if a > 1e-6 { (p[k] / a).min(1.0) } else { 0.0 } } else { p[k] * a };
        }
    });
    d.store(img);
}

// ------------------------------------------------------------------ file mattes (Shape Alpha)

type SourceCache = Mutex<HashMap<String, Option<redrob_film_media::SharedSource>>>;
fn sources() -> &'static SourceCache {
    static C: OnceLock<SourceCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The image or video at `path`, `t` seconds in, as straight (luma, alpha) planes resized to
/// `w` × `h`.
fn file_planes(path: &str, t: f64, w: usize, h: usize) -> Option<(Vec<f32>, Vec<f32>)> {
    let path = path.trim();
    if path.is_empty() || w == 0 || h == 0 {
        return None;
    }
    let src = {
        let mut c = sources().lock().unwrap_or_else(|p| p.into_inner());
        if c.len() > 16 {
            c.clear();
        }
        c.entry(path.to_string()).or_insert_with(|| redrob_film_media::open_path(std::path::Path::new(path), &[]).ok()).clone()?
    };
    let f = src.video_frame(redrob_film_media::FrameRequest { time: redrob_film_time::Tick::from_seconds_f64(t.max(0.0)), scale: 1.0 }).ok()?;
    let rgba = f.to_rgba8();
    let (fw, fh) = (f.width as usize, f.height as usize);
    if fw == 0 || fh == 0 {
        return None;
    }
    let mut luma = vec![0.0f32; w * h];
    let mut alpha = vec![0.0f32; w * h];
    luma.par_chunks_mut(w).zip(alpha.par_chunks_mut(w)).enumerate().for_each(|(y, (lr, ar))| {
        let sy = ((y as f32 + 0.5) * fh as f32 / h as f32) as usize;
        for x in 0..w {
            let sx = ((x as f32 + 0.5) * fw as f32 / w as f32) as usize;
            let i = (sy.min(fh - 1) * fw + sx.min(fw - 1)) * 4;
            let s = &rgba[i..i + 4];
            lr[x] = (0.2126 * s[0] as f32 + 0.7152 * s[1] as f32 + 0.0722 * s[2] as f32) / 255.0;
            ar[x] = s[3] as f32 / 255.0;
        }
    });
    Some((luma, alpha))
}

/// The matte of Shape Alpha / Shape Alpha (Mask): the file's alpha (or luma), thresholded with a
/// soft edge when asked, inverted when asked. `None` when there is no readable file.
pub(crate) fn shape_matte(e: &EffectInstance, cx: &FxCtx, w: usize, h: usize) -> Option<Vec<f32>> {
    let offset = fv(e, "offset", cx) as f64;
    let out = fv(e, "out", cx) as f64;
    let mut t = offset + cx.seconds;
    if out > offset {
        t = t.min(out);
    }
    let (luma, alpha) = file_planes(&text_param(e, "resource"), t, w, h)?;
    let mut m = if flag(e, "use_luma") { luma } else { alpha };
    if flag(e, "use_threshold") {
        let th = fv(e, "threshold", cx) / 100.0;
        let soft = fv(e, "softness", cx).clamp(0.0, 1.0) / 2.0;
        // below the threshold opaque, above it transparent
        m.iter_mut().for_each(|v| *v = 1.0 - smoothstep(th - soft, th + soft + 1e-4, *v));
    }
    if flag(e, "invert") {
        m.iter_mut().for_each(|v| *v = 1.0 - *v);
    }
    Some(m)
}

/// Shape Alpha: the picture's alpha is multiplied by the file matte.
pub fn shape_alpha(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some(m) = shape_matte(e, cx, img.w, img.h) else { return };
    img.px.par_chunks_mut(4).zip(m.par_iter()).for_each(|(p, &k)| {
        let k = k.clamp(0.0, 1.0);
        for v in p.iter_mut() {
            *v *= k;
        }
    });
}

/// The selection of Secondary Color Correction Area Selection (Mask): pixels near the colour.
pub(crate) fn color_selection(img: &Image, e: &EffectInstance, cx: &FxCtx) -> Vec<f32> {
    let c = color(e, "color", cx);
    let key = [c[0], c[1], c[2]];
    let tol = fv(e, "tolerance", cx) / 100.0;
    let soft = (fv(e, "softness", cx) / 100.0).max(1e-4);
    let inv = flag(e, "invert");
    img.px
        .par_chunks(4)
        .map(|p| {
            let a = p[3];
            let m = if a <= 1e-6 {
                0.0
            } else {
                let q = crate::effects::enc([p[0] / a, p[1] / a, p[2] / a]);
                let d = dist3([q[0].min(1.0), q[1].min(1.0), q[2].min(1.0)], key) / 3f32.sqrt();
                1.0 - smoothstep(tol, tol + soft, d)
            };
            if inv { 1.0 - m } else { m }
        })
        .collect()
}

/// The selection a mask-chain start effect makes on `img` (`None` = everything).
pub fn mask_selection(img: &Image, e: &EffectInstance, cx: &FxCtx) -> Option<Vec<f32>> {
    match e.effect.as_str() {
        "k_color_select_mask" => Some(color_selection(img, e, cx)),
        "k_shape_alpha_mask" => shape_matte(e, cx, img.w, img.h),
        _ => None,
    }
}

// ------------------------------------------------------------------ Spot Remover

/// The `rect_*` percent rectangle in working pixels: `[x0, y0, x1, y1]`, clamped to the frame.
pub(crate) fn pct_rect(e: &EffectInstance, cx: &FxCtx, w: usize, h: usize) -> [f32; 4] {
    let (wf, hf) = (w as f32, h as f32);
    let x0 = (fv(e, "rect_x", cx) / 100.0 * wf).clamp(0.0, wf);
    let y0 = (fv(e, "rect_y", cx) / 100.0 * hf).clamp(0.0, hf);
    let x1 = (x0 + fv(e, "rect_w", cx).max(0.0) / 100.0 * wf).clamp(0.0, wf);
    let y1 = (y0 + fv(e, "rect_h", cx).max(0.0) / 100.0 * hf).clamp(0.0, hf);
    [x0, y0, x1, y1]
}

/// Spot Remover: the rectangle is refilled from the pixels just outside it, interpolated across
/// (each pixel mixes the horizontal and vertical interpolations, the nearer border weighing more).
pub fn spot_remover(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let [x0, y0, x1, y1] = pct_rect(e, cx, img.w, img.h);
    let (xa, ya, xb, yb) = (x0.floor() as isize, y0.floor() as isize, x1.ceil() as isize, y1.ceil() as isize);
    if xb - xa < 1 || yb - ya < 1 {
        return;
    }
    let src = img.clone();
    let iw = img.w;
    // border samples just outside the rectangle (clamped to the frame)
    let left = |y: isize| src.get_clamped(xa - 1, y);
    let right = |y: isize| src.get_clamped(xb, y);
    let top = |x: isize| src.get_clamped(x, ya - 1);
    let bottom = |x: isize| src.get_clamped(x, yb);
    img.px.par_chunks_mut(iw * 4).enumerate().for_each(|(y, row)| {
        let y = y as isize;
        if y < ya || y >= yb {
            return;
        }
        for x in xa.max(0)..xb.min(iw as isize) {
            let tx = (x - xa + 1) as f32 / (xb - xa + 1) as f32;
            let ty = (y - ya + 1) as f32 / (yb - ya + 1) as f32;
            let (l, r, t, b) = (left(y), right(y), top(x), bottom(x));
            let dx = tx.min(1.0 - tx).max(1e-3);
            let dy = ty.min(1.0 - ty).max(1e-3);
            let (wh, wv) = (1.0 / dx, 1.0 / dy);
            let p = &mut row[x as usize * 4..x as usize * 4 + 4];
            for k in 0..4 {
                let hv = l[k] + (r[k] - l[k]) * tx;
                let vv = t[k] + (b[k] - t[k]) * ty;
                p[k] = (hv * wh + vv * wv) / (wh + wv);
            }
        }
    });
}

// ------------------------------------------------------------------ Motion Tracker

/// Coverage of a point by the tracker's shape over `[x0, y0, x1, y1]` (ellipse or rectangle).
#[inline]
fn in_shape(shape: u32, r: [f32; 4], x: f32, y: f32) -> bool {
    if x < r[0] || x >= r[2] || y < r[1] || y >= r[3] {
        return false;
    }
    if shape == 1 {
        let (cxp, cyp) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        let (rx, ry) = (((r[2] - r[0]) / 2.0).max(0.5), ((r[3] - r[1]) / 2.0).max(0.5));
        return ((x - cxp) / rx).powi(2) + ((y - cyp) / ry).powi(2) <= 1.0;
    }
    true
}

/// Paint a straight line of `width` from `a` to `b` (premultiplied `col`, antialiased).
pub(crate) fn draw_segment(img: &mut Image, a: Vec2, b: Vec2, width: f32, col: [f32; 4]) {
    let hw = (width / 2.0).max(0.5);
    let (x0, x1) = ((a.x.min(b.x) as f32 - hw - 1.0).max(0.0) as usize, ((a.x.max(b.x) as f32 + hw + 1.0).max(0.0) as usize).min(img.w));
    let (y0, y1) = ((a.y.min(b.y) as f32 - hw - 1.0).max(0.0) as usize, ((a.y.max(b.y) as f32 + hw + 1.0).max(0.0) as usize).min(img.h));
    let (ax, ay, bx, by) = (a.x as f32, a.y as f32, b.x as f32, b.y as f32);
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = (dx * dx + dy * dy).max(1e-6);
    let w = img.w;
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let t = (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0);
            let d = ((px - ax - dx * t).powi(2) + (py - ay - dy * t).powi(2)).sqrt();
            let cov = (hw + 0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                let i = (y * w + x) * 4;
                over(&mut img.px[i..i + 4], col.map(|v| v * cov));
            }
        }
    }
}

/// The region the tracker covers now: the bounds of its first active mask (a mask tracked with
/// the Mask ▸ Track buttons follows the object), else its rectangle parameters.
fn tracker_region(img: &Image, e: &EffectInstance, cx: &FxCtx) -> [f32; 4] {
    if let Some(m) = e.masks.iter().find(|m| m.mode != MaskMode::None) {
        let pts = m.path_at(cx.t).flatten(1.0);
        if pts.len() >= 2 {
            let k = cx.px_scale as f64;
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for p in &pts {
                x0 = x0.min(p.x * k);
                y0 = y0.min(p.y * k);
                x1 = x1.max(p.x * k);
                y1 = y1.max(p.y * k);
            }
            let (w, h) = (img.w as f64, img.h as f64);
            return [x0.clamp(0.0, w) as f32, y0.clamp(0.0, h) as f32, x1.clamp(0.0, w) as f32, y1.clamp(0.0, h) as f32];
        }
    }
    pct_rect(e, cx, img.w, img.h)
}

/// Motion Tracker: the tracked region is obscured (median / Gaussian blur, pixelate, opaque
/// fill) and framed with a rectangle, an ellipse or an arrow pointing at it.
pub fn motion_tracker(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let r = tracker_region(img, e, cx);
    if r[2] - r[0] < 1.0 || r[3] - r[1] < 1.0 {
        return;
    }
    let shape = pick(e, "shape");
    let sc = color(e, "shape_color", cx);
    let blur = fv(e, "blur", cx).max(0.0) * cx.px_scale;
    let area = if shape == 2 { 0 } else { shape };
    let (xa, ya, xb, yb) = (r[0].floor() as usize, r[1].floor() as usize, (r[2].ceil() as usize).min(img.w), (r[3].ceil() as usize).min(img.h));
    let cw = xb.saturating_sub(xa);
    let treated: Option<Image> = match pick(e, "blur_type") {
        1 | 2 | 3 => {
            // a crop with a margin, processed on its own
            let m = (blur.ceil() as usize * 3).min(256);
            let (ox, oy) = (xa.saturating_sub(m), ya.saturating_sub(m));
            let (ex, ey) = ((xb + m).min(img.w), (yb + m).min(img.h));
            let mut sub = Image::new(ex - ox, ey - oy);
            for y in oy..ey {
                let s = (y * img.w + ox) * 4;
                sub.px[(y - oy) * sub.w * 4..(y - oy + 1) * sub.w * 4].copy_from_slice(&img.px[s..s + sub.w * 4]);
            }
            match pick(e, "blur_type") {
                1 => median_img(&mut sub, (blur.round() as usize).clamp(1, 8)),
                2 => crate::effects::gaussian(&mut sub, blur.max(0.5), blur.max(0.5), true),
                _ => pixelate(&mut sub, (blur.round() as usize).max(2), xa - ox, ya - oy),
            }
            let mut full = img.clone();
            for y in oy..ey {
                let d = (y * img.w + ox) * 4;
                full.px[d..d + sub.w * 4].copy_from_slice(&sub.px[(y - oy) * sub.w * 4..(y - oy + 1) * sub.w * 4]);
            }
            Some(full)
        }
        4 => Some(Image::filled(img.w, img.h, crate::graphics::premul_linear([sc[0], sc[1], sc[2], 1.0]))),
        _ => None,
    };
    if let Some(t) = treated {
        let w = img.w;
        for y in ya..yb {
            for x in xa..xa + cw {
                if in_shape(area, r, x as f32 + 0.5, y as f32 + 0.5) {
                    let i = (y * w + x) * 4;
                    img.px[i..i + 4].copy_from_slice(&t.px[i..i + 4]);
                }
            }
        }
    }
    let sw = fv(e, "shape_width", cx).max(0.0) * cx.px_scale.max(0.25);
    if sw <= 0.0 {
        return;
    }
    let col = crate::graphics::premul_linear(sc);
    let (x0, y0, x1, y1) = (r[0] as f64, r[1] as f64, r[2] as f64, r[3] as f64);
    match shape {
        1 => {
            let (cxp, cyp, rx, ry) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0, (y1 - y0) / 2.0);
            let n = 72;
            for i in 0..n {
                let (a0, a1) = (i as f64 / n as f64 * std::f64::consts::TAU, (i + 1) as f64 / n as f64 * std::f64::consts::TAU);
                draw_segment(img, Vec2::new(cxp + rx * a0.cos(), cyp + ry * a0.sin()), Vec2::new(cxp + rx * a1.cos(), cyp + ry * a1.sin()), sw, col);
            }
        }
        2 => {
            // an arrow from below-left pointing at the region's lower-left corner
            let len = ((x1 - x0).max(y1 - y0)).max(8.0);
            let tip = Vec2::new(x0, y1);
            let tail = Vec2::new(x0 - len * 0.7, y1 + len * 0.7);
            draw_segment(img, tail, tip, sw, col);
            let head = len * 0.25;
            draw_segment(img, tip, Vec2::new(tip.x - head, tip.y), sw, col);
            draw_segment(img, tip, Vec2::new(tip.x, tip.y + head), sw, col);
        }
        _ => {
            let c = [Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)];
            for i in 0..4 {
                draw_segment(img, c[i], c[(i + 1) % 4], sw, col);
            }
        }
    }
}

/// Median of each channel over a (2r+1)² window (premultiplied values).
fn median_img(img: &mut Image, r: usize) {
    let src = img.clone();
    let (w, h) = (img.w, img.h);
    let ri = r as isize;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let mut buf: Vec<f32> = Vec::with_capacity((2 * r + 1) * (2 * r + 1));
        for x in 0..w {
            for k in 0..4 {
                buf.clear();
                for dy in -ri..=ri {
                    for dx in -ri..=ri {
                        let p = src.get_clamped(x as isize + dx, (y as isize + dy).clamp(0, h as isize - 1));
                        buf.push(p[k]);
                    }
                }
                buf.sort_by(f32::total_cmp);
                row[x * 4 + k] = buf[buf.len() / 2];
            }
        }
    });
}

/// Average blocks of `n` pixels, aligned on (`ox`, `oy`).
fn pixelate(img: &mut Image, n: usize, ox: usize, oy: usize) {
    let (w, h) = (img.w, img.h);
    let n = n.max(1);
    let bx0 = ox % n;
    let by0 = oy % n;
    let starts = |o: usize, len: usize| {
        let mut v = vec![0];
        let mut s = if o == 0 { n } else { o };
        while s < len {
            v.push(s);
            s += n;
        }
        v
    };
    let xs = starts(bx0, w);
    let ys = starts(by0, h);
    for (yi, &sy) in ys.iter().enumerate() {
        let ey = ys.get(yi + 1).copied().unwrap_or(h);
        for (xi, &sx) in xs.iter().enumerate() {
            let ex = xs.get(xi + 1).copied().unwrap_or(w);
            let mut acc = [0.0f32; 4];
            for yy in sy..ey {
                for xx in sx..ex {
                    let p = img.get(xx, yy);
                    for k in 0..4 {
                        acc[k] += p[k];
                    }
                }
            }
            let cnt = ((ey - sy) * (ex - sx)).max(1) as f32;
            for yy in sy..ey {
                for xx in sx..ex {
                    let i = (yy * w + xx) * 4;
                    for k in 0..4 {
                        img.px[i + k] = acc[k] / cnt;
                    }
                }
            }
        }
    }
}
