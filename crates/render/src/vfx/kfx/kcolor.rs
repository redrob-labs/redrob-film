//! Color and Image Correction, Image Adjustments and Blur and Sharpen effects of the "More Video
//! Effects" bin (`redrob_film_project::effect::kcolor`).
//!
//! Plane-based effects work on the straight display-encoded picture ([`Disp`]) split into YUV or
//! RGB planes (the `planes` choice), like the rest of the bin. Grayworld and White Balance (LMS)
//! work in linear light on the premultiplied picture, where a per-channel gain commutes with
//! premultiplication. Temporal effects (Normaliz0r / Normalize RGB Video smoothing, Phase and the
//! motion-compensating deinterlacer) read neighbouring frames through [`neighbour`] and behave as
//! a single-frame effect without an environment.

use super::*;
use crate::vfx::blur_plane;

// ------------------------------------------------------------------ plane helpers

/// Run `f(plane, w, h, index)` on every plane an [`redrob_film_project::effect::ANY_PLANES`]
/// choice selects, and write the picture back.
fn on_planes(img: &mut Image, choice: u32, f: impl Fn(&[f32], usize, usize, usize) -> Vec<f32>) {
    let pl = Planes::from_any_choice(choice);
    if pl.none() || img.w == 0 || img.h == 0 {
        return;
    }
    let mut d = Disp::of(img);
    let mut planes = split_planes(&d, pl.yuv);
    for k in 0..4 {
        if pl.on[k] {
            planes[k] = f(&planes[k], d.w, d.h, k);
        }
    }
    merge_planes(&mut d, &planes, pl.yuv);
    d.store(img);
}

/// The YUV planes and alpha of the picture, processed by `f(planes, w, h)`.
fn on_yuva(img: &mut Image, f: impl FnOnce(&mut [Vec<f32>; 4], usize, usize)) {
    if img.w == 0 || img.h == 0 {
        return;
    }
    let mut d = Disp::of(img);
    let mut planes = split_planes(&d, true);
    f(&mut planes, d.w, d.h);
    merge_planes(&mut d, &planes, true);
    d.store(img);
}

fn transpose(p: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut o = vec![0.0f32; p.len()];
    o.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for y in 0..h {
            col[y] = p[y * w + x];
        }
    });
    o
}

/// Horizontal box average of radius `r` (2r+1 taps, edges clamped).
fn box_h(p: &[f32], w: usize, r: usize) -> Vec<f32> {
    if r == 0 || w == 0 {
        return p.to_vec();
    }
    let n = (2 * r + 1) as f32;
    let mut out = vec![0.0f32; p.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &p[y * w..(y + 1) * w];
        let at = |x: isize| s[x.clamp(0, w as isize - 1) as usize];
        let mut acc: f32 = (-(r as isize)..=(r as isize)).map(at).sum();
        for x in 0..w {
            row[x] = acc / n;
            acc += at(x as isize + r as isize + 1) - at(x as isize - r as isize);
        }
    });
    out
}

/// Separable box average with independent radii.
fn box2(p: &[f32], w: usize, h: usize, rx: usize, ry: usize) -> Vec<f32> {
    let a = box_h(p, w, rx);
    if ry == 0 {
        return a;
    }
    let t = box_h(&transpose(&a, w, h), h, ry);
    transpose(&t, h, w)
}

#[inline]
fn at(p: &[f32], w: usize, h: usize, x: isize, y: isize) -> f32 {
    p[y.clamp(0, h as isize - 1) as usize * w + x.clamp(0, w as isize - 1) as usize]
}

/// Edge-preserving weighted average: spatial Gaussian `sigma` (pixels) times a range weight from
/// the difference of `guide` values (`range(Δ)`).
fn range_blur(p: &[f32], guide: &[f32], w: usize, h: usize, sigma: f32, range: impl Fn(f32) -> f32 + Sync) -> Vec<f32> {
    if sigma < 0.05 {
        return p.to_vec();
    }
    let r = (sigma * 2.0).ceil().clamp(1.0, 12.0) as isize;
    let sk: Vec<f32> = (-r..=r).flat_map(|dy| (-r..=r).map(move |dx| (-((dx * dx + dy * dy) as f32) / (2.0 * sigma * sigma)).exp())).collect();
    let mut out = vec![0.0f32; p.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let g0 = guide[y * w + x];
            let (mut acc, mut wsum) = (0.0f32, 0.0f32);
            let mut k = 0;
            for dy in -r..=r {
                for dx in -r..=r {
                    let (xx, yy) = (x as isize + dx, y as isize + dy);
                    let wt = sk[k] * range(at(guide, w, h, xx, yy) - g0);
                    k += 1;
                    acc += wt * at(p, w, h, xx, yy);
                    wsum += wt;
                }
            }
            row[x] = if wsum > 1e-9 { acc / wsum } else { p[y * w + x] };
        }
    });
    out
}

fn px(e: &EffectInstance, id: &str, cx: &FxCtx) -> usize {
    (fv(e, id, cx) * cx.px_scale).round().max(0.0) as usize
}

// ------------------------------------------------------------------ Blur and Sharpen

/// Average Blur: box average of X × Y size on the chosen planes (Y size 0 = the X size).
pub(super) fn avgblur(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let rx = px(e, "size_x", cx);
    let ry = if fv(e, "size_y", cx) <= 0.0 { rx } else { px(e, "size_y", cx) };
    if rx == 0 && ry == 0 {
        return;
    }
    on_planes(img, pick(e, "planes"), |p, w, h, _| box2(p, w, h, rx, ry));
}

/// Bilateral: spatial Gaussian (spatial sigma, pixels) weighted by a Gaussian of the value
/// difference (range sigma, 0…1 of the signal).
pub(super) fn bilateral(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let ss = fv(e, "sigma_s", cx) * cx.px_scale;
    let sr = fv(e, "sigma_r", cx);
    if ss < 0.05 || sr <= 1e-4 {
        return;
    }
    let k = 1.0 / (2.0 * sr * sr);
    on_planes(img, pick(e, "planes"), |p, w, h, _| range_blur(p, p, w, h, ss, |d| (-d * d * k).exp()));
}

/// BoxBlur: horizontal / vertical box radius from the multiplicators (1 % = 0.25 px); with
/// Preserve Alpha the transparency is left sharp.
pub(super) fn boxblur(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let rx = (fv(e, "hori", cx) / 4.0 * cx.px_scale).round().max(0.0) as usize;
    let ry = (fv(e, "vert", cx) / 4.0 * cx.px_scale).round().max(0.0) as usize;
    if rx == 0 && ry == 0 {
        return;
    }
    if flag(e, "preserve_alpha") {
        // Red, Green, Blue planes of the straight picture, alpha untouched
        let mut d = Disp::of(img);
        let mut planes = split_planes(&d, false);
        for k in 0..3 {
            planes[k] = box2(&planes[k], d.w, d.h, rx, ry);
        }
        merge_planes(&mut d, &planes, false);
        d.store(img);
    } else {
        box_blur(img, rx, ry);
    }
}

/// Contrast Adaptive Sharpen: a cross-shaped sharpening kernel whose weight shrinks where the
/// local contrast (min / max of the cross) is already high, so edges do not ring.
pub(super) fn cas(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let s = fv(e, "strength", cx).clamp(0.0, 1.0);
    let peak = -1.0 / (8.0 - 3.0 * s);
    on_planes(img, pick(e, "planes"), |p, w, h, _| {
        let mut out = vec![0.0f32; p.len()];
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let y = y as isize;
            for (x, o) in row.iter_mut().enumerate() {
                let x = x as isize;
                let m = at(p, w, h, x, y);
                let (n, so, we, ea) = (at(p, w, h, x, y - 1), at(p, w, h, x, y + 1), at(p, w, h, x - 1, y), at(p, w, h, x + 1, y));
                let mn = m.min(n).min(so).min(we).min(ea);
                let mx = m.max(n).max(so).max(we).max(ea);
                let amp = (mn.min(1.0 - mx) / mx.max(1e-5)).clamp(0.0, 1.0).sqrt();
                let wt = amp * peak;
                *o = (((n + so + we + ea) * wt + m) / (1.0 + 4.0 * wt)).clamp(0.0, 1.0);
            }
        });
        out
    });
}

/// Planes Blur: box blur of the luma, chroma and alpha planes, each with its own radius applied
/// `power` times (power 0 leaves the plane).
pub(super) fn planes_blur(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let r = [px(e, "luma_radius", cx), px(e, "chroma_radius", cx), px(e, "alpha_radius", cx)];
    let pw = [fv(e, "luma_power", cx), fv(e, "chroma_power", cx), fv(e, "alpha_power", cx)].map(|v| v.round().clamp(0.0, 10.0) as usize);
    on_yuva(img, |planes, w, h| {
        for (k, plane) in planes.iter_mut().enumerate() {
            let g = [0, 1, 1, 2][k];
            for _ in 0..pw[g] {
                *plane = box2(plane, w, h, r[g], r[g]);
            }
        }
    });
}

/// Shape Adaptive Blur: Gaussian blur (radius) whose neighbours count only while their
/// pre-filtered value stays within `strength` levels (of 255) of the centre's.
pub(super) fn sab(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let s = cx.px_scale;
    let luma = (fv(e, "luma_radius", cx) * s, fv(e, "luma_pre", cx) * s, fv(e, "luma_strength", cx));
    let chroma = (fv(e, "chroma_radius", cx) * s, fv(e, "chroma_pre", cx) * s, fv(e, "chroma_strength", cx));
    on_yuva(img, |planes, w, h| {
        for (k, plane) in planes.iter_mut().enumerate().take(3) {
            let (radius, pre, strength) = if k == 0 { luma } else { chroma };
            if radius < 0.05 || strength <= 0.0 {
                continue;
            }
            let guide = blur_plane(plane, w, h, pre);
            let kk = 1.0 / (2.0 * strength * strength);
            *plane = range_blur(plane, &guide, w, h, radius, |d| {
                let d = d * 255.0;
                (-d * d * kk).exp()
            });
        }
    });
}

/// Smartblur: Gaussian blur (radius) mixed in by `strength` (negative sharpens); a positive
/// threshold limits it to flat areas, a negative one to edges (difference to the blur, of 255).
pub(super) fn smartblur(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let s = cx.px_scale;
    let luma = (fv(e, "luma_radius", cx) * s, fv(e, "luma_strength", cx), fv(e, "luma_threshold", cx).round());
    let chroma = (fv(e, "chroma_radius", cx) * s, fv(e, "chroma_strength", cx), fv(e, "chroma_threshold", cx).round());
    on_yuva(img, |planes, w, h| {
        for (k, plane) in planes.iter_mut().enumerate().take(3) {
            let (radius, strength, thr) = if k == 0 { luma } else { chroma };
            if strength == 0.0 {
                continue;
            }
            let blurred = blur_plane(plane, w, h, radius.max(0.1));
            plane.par_iter_mut().zip(blurred.par_iter()).for_each(|(v, b)| {
                let d = (*v - b).abs() * 255.0;
                let apply = thr == 0.0 || (thr > 0.0 && d < thr) || (thr < 0.0 && d > -thr);
                if apply {
                    *v = (*v + strength * (b - *v)).clamp(0.0, 1.0);
                }
            });
        }
    });
}

/// Square Blur: a square box kernel whose size is a proportion of the picture (500 = 10 % of
/// its larger side).
pub(super) fn squareblur(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let k = fv(e, "kernel", cx).clamp(0.0, 500.0) / 500.0;
    let r = (k * 0.1 * img.w.max(img.h) as f32).round() as usize;
    box_blur(img, r, r);
}

// ------------------------------------------------------------------ colour helpers

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0);
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let ch = |t: f32| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [ch(h + 1.0 / 3.0), ch(h), ch(h - 1.0 / 3.0)]
}
#[inline]
fn hsl_l(c: [f32; 3]) -> f32 {
    (c[0].max(c[1]).max(c[2]) + c[0].min(c[1]).min(c[2])) / 2.0
}

/// 3-Point Balance: per channel, the curve through (black → 0, gray → ½, white → 1) of the three
/// picked colours (a quadratic, linear when the gray point is degenerate). Split Preview keeps
/// the source on one half.
pub(super) fn three_point(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (bk, gy, wh) = (color(e, "black", cx), color(e, "gray", cx), color(e, "white", cx));
    let curves: Vec<[f32; 3]> = (0..3)
        .map(|c| {
            // y = a x² + b x + d through (x0, 0), (x1, .5), (x2, 1)
            let (x0, x1, x2) = (bk[c], gy[c], wh[c]);
            let det = (x0 - x1) * (x0 - x2) * (x1 - x2);
            if det.abs() < 1e-6 {
                let s = 1.0 / (x2 - x0).abs().max(1e-3);
                [0.0, s, -x0 * s]
            } else {
                let (y0, y1, y2) = (0.0f32, 0.5f32, 1.0f32);
                let a = (x2 * (y1 - y0) + x1 * (y0 - y2) + x0 * (y2 - y1)) / det;
                let b = (x2 * x2 * (y0 - y1) + x1 * x1 * (y2 - y0) + x0 * x0 * (y1 - y2)) / det;
                let d = (x1 * x2 * (x1 - x2) * y0 + x2 * x0 * (x2 - x0) * y1 + x0 * x1 * (x0 - x1) * y2) / det;
                [a, b, d]
            }
        })
        .collect();
    let split = flag(e, "split");
    let left = flag(e, "split_left");
    let w = img.w;
    map_disp(img, |c, x, _| {
        if split && ((x < w / 2) == left) {
            return c;
        }
        let mut o = [0.0; 3];
        for k in 0..3 {
            let [a, b, d] = curves[k];
            o[k] = (a * c[k] * c[k] + b * c[k] + d).clamp(0.0, 1.0);
        }
        o
    });
}

/// CMYK Adjust: Cyan / Magenta / Yellow / Black adjustments per colour range, weighted by how
/// strongly a pixel belongs to that range; Relative scales them by the ink already present.
pub(super) fn cmyk_adjust(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    const RANGES: [&str; 9] = ["reds", "yellows", "greens", "cyans", "blues", "magentas", "whites", "neutrals", "blacks"];
    let adj: Vec<[f32; 4]> = RANGES
        .iter()
        .map(|r| ["c", "m", "y", "k"].map(|k| fv(e, &format!("{r}_{k}"), cx) / 100.0))
        .collect();
    if adj.iter().all(|a| a.iter().all(|v| *v == 0.0)) {
        return;
    }
    let relative = pick(e, "method") == 1;
    map_disp(img, |c, _, _| {
        let mx = c[0].max(c[1]).max(c[2]);
        let mn = c[0].min(c[1]).min(c[2]);
        let mid = c[0] + c[1] + c[2] - mx - mn;
        let is_max = |k: usize| c[k] >= mx && mx > mn;
        let is_min = |k: usize| c[k] <= mn && mx > mn;
        let weights = [
            if is_max(0) { mx - mid } else { 0.0 },
            if is_min(2) { mid - mn } else { 0.0 },
            if is_max(1) { mx - mid } else { 0.0 },
            if is_min(0) { mid - mn } else { 0.0 },
            if is_max(2) { mx - mid } else { 0.0 },
            if is_min(1) { mid - mn } else { 0.0 },
            if mn > 0.5 { (mn - 0.5) * 2.0 } else { 0.0 },
            if mx > 0.0 && mn < 1.0 { (1.0 - ((mx - 0.5).abs() + (mn - 0.5).abs())).clamp(0.0, 1.0) } else { 0.0 },
            if mx < 0.5 { (0.5 - mx) * 2.0 } else { 0.0 },
        ];
        // ink amounts
        let mut ink = [1.0 - c[0], 1.0 - c[1], 1.0 - c[2]];
        let base = ink;
        for (r, wgt) in weights.iter().enumerate() {
            if *wgt <= 0.0 {
                continue;
            }
            let a = adj[r];
            for k in 0..3 {
                let amount = a[k] + a[3];
                let scale = if relative { base[k] } else { 1.0 };
                ink[k] += wgt * amount * scale;
            }
        }
        [(1.0 - ink[0]).clamp(0.0, 1.0), (1.0 - ink[1]).clamp(0.0, 1.0), (1.0 - ink[2]).clamp(0.0, 1.0)]
    });
}

/// Color Contrast: pushes each primary away from (or toward) its complement on the red-cyan,
/// green-magenta and blue-yellow axes, by contrast × weight; Preserve Lightness restores luma.
pub(super) fn color_contrast(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let amt = [fv(e, "rc", cx) * fv(e, "rcw", cx), fv(e, "gm", cx) * fv(e, "gmw", cx), fv(e, "by", cx) * fv(e, "byw", cx)];
    let wsum = fv(e, "rcw", cx) + fv(e, "gmw", cx) + fv(e, "byw", cx);
    if wsum <= 0.0 {
        return;
    }
    let pl = fv(e, "pl", cx).clamp(0.0, 1.0);
    map_disp(img, |c, _, _| {
        let mut o = c;
        for k in 0..3 {
            let (a, b) = ((k + 1) % 3, (k + 2) % 3);
            let d = c[k] - (c[a] + c[b]) / 2.0;
            let s = amt[k] / wsum.max(1.0);
            o[k] += s * d;
            o[a] -= s * d / 2.0;
            o[b] -= s * d / 2.0;
        }
        let l0 = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let l1 = 0.2126 * o[0] + 0.7152 * o[1] + 0.0722 * o[2];
        o.map(|v| (v + pl * (l0 - l1)).clamp(0.0, 1.0))
    });
}

/// Color Overlay: a solid HSL colour whose lightness mixes toward the source's by `mix`.
pub(super) fn color_overlay(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let h = fv(e, "hue", cx) / 360.0;
    let s = fv(e, "saturation", cx).clamp(0.0, 1.0);
    let l = fv(e, "lightness", cx).clamp(0.0, 1.0);
    let mix = fv(e, "mix", cx).clamp(0.0, 1.0);
    map_disp(img, |c, _, _| hsl_to_rgb(h, s, l + (hsl_l(c) - l) * mix));
}

/// Colorize: the source's lightness, shifted toward white (lightness > ½) or black (< ½), in the
/// chosen hue and saturation.
pub(super) fn colorize(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let h = fv(e, "hue", cx);
    let s = fv(e, "saturation", cx).clamp(0.0, 1.0);
    let k = (fv(e, "lightness", cx).clamp(0.0, 1.0) - 0.5) * 2.0;
    map_disp(img, |c, _, _| {
        let l = hsl_l(c);
        let l = if k >= 0.0 { l + (1.0 - l) * k } else { l * (1.0 + k) };
        hsl_to_rgb(h, s, l)
    });
}

/// Per-channel 256-bin histograms (opaque-weighted) of the straight display picture.
fn histograms(d: &Disp) -> [[f64; 256]; 3] {
    let mut hst = [[0.0f64; 256]; 3];
    for p in &d.px {
        if p[3] <= 1e-4 {
            continue;
        }
        for k in 0..3 {
            hst[k][(p[k] * 255.0).round().clamp(0.0, 255.0) as usize] += p[3] as f64;
        }
    }
    hst
}
fn cdf(h: &[f64; 256]) -> [f32; 256] {
    let total: f64 = h.iter().sum();
    let mut o = [0.0f32; 256];
    let mut acc = 0.0;
    for i in 0..256 {
        acc += h[i];
        o[i] = if total > 0.0 { (acc / total) as f32 } else { i as f32 / 255.0 };
    }
    o
}

/// Equaliz0r: per-channel histogram equalisation (each level maps to its cumulative share).
pub(super) fn equaliz0r(img: &mut Image) {
    let d = Disp::of(img);
    let maps: Vec<[f32; 256]> = histograms(&d).iter().map(cdf).collect();
    map_disp(img, |c, _, _| [0, 1, 2].map(|k| maps[k][(c[k] * 255.0).round().clamp(0.0, 255.0) as usize]));
}

/// FFT-based FIR: a frequency-domain filter whose weight is the same at every frequency is a
/// gain on the plane, and a DC gain lifts the zero-frequency term — so each YUV plane becomes
/// `weight × plane + gain` (chroma about its neutral ½). Init and Frame evaluate the same constants.
pub(super) fn fftfilt(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let wts = [fv(e, "weight_y", cx), fv(e, "weight_u", cx), fv(e, "weight_v", cx)];
    let dc = [fv(e, "dc_y", cx), fv(e, "dc_u", cx), fv(e, "dc_v", cx)].map(|v| v / 255.0);
    if wts == [1.0; 3] && dc == [0.0; 3] {
        return;
    }
    on_yuva(img, |planes, _, _| {
        for k in 0..3 {
            let c = if k == 0 { 0.0 } else { 0.5 };
            planes[k].par_iter_mut().for_each(|v| *v = (c + (*v - c) * wts[k] + dc[k]).clamp(0.0, 1.0));
        }
    });
}

/// Gradient Map: the pixel's luma picks a colour from the gradient of the first `stops` stops
/// (sorted by position); the stop's alpha scales the pixel's.
pub(super) fn gradient_map(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let n = fv(e, "stops", cx).round().clamp(2.0, 8.0) as usize;
    let mut stops: Vec<(f32, [f32; 4])> =
        (1..=n).map(|k| (fv(e, &format!("stop{k}_pos"), cx) / 100.0, color(e, &format!("stop{k}_color"), cx))).collect();
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut d = Disp::of(img);
    d.px.par_iter_mut().for_each(|p| {
        if p[3] <= 1e-6 {
            return;
        }
        let l = luma(*p);
        let c = if l <= stops[0].0 {
            stops[0].1
        } else if l >= stops[n - 1].0 {
            stops[n - 1].1
        } else {
            let i = stops.iter().rposition(|s| s.0 <= l).unwrap_or(0).min(n - 2);
            let (a, b) = (stops[i], stops[i + 1]);
            let t = ((l - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0);
            [0, 1, 2, 3].map(|k| a.1[k] + (b.1[k] - a.1[k]) * t)
        };
        *p = [c[0], c[1], c[2], p[3] * c[3]];
    });
    d.store(img);
}

// linear sRGB / Rec.709 ↔ XYZ (D65)
const RGB2XYZ: [[f32; 3]; 3] = [[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]];
const XYZ2RGB: [[f32; 3]; 3] = [[3.2406, -1.5372, -0.4986], [-0.9689, 1.8758, 0.0415], [0.0557, -0.2040, 1.0570]];
const D65: [f32; 3] = [0.95047, 1.0, 1.08883];
#[inline]
fn mul3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}
fn lab_f(t: f32) -> f32 {
    if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 }
}
fn lab_finv(t: f32) -> f32 {
    if t > 0.206893 { t * t * t } else { (t - 16.0 / 116.0) / 7.787 }
}
fn to_lab(rgb: [f32; 3]) -> [f32; 3] {
    let x = mul3(&RGB2XYZ, rgb);
    let f = [0, 1, 2].map(|k| lab_f((x[k] / D65[k]).max(0.0)));
    [116.0 * f[1] - 16.0, 500.0 * (f[0] - f[1]), 200.0 * (f[1] - f[2])]
}
fn from_lab(l: [f32; 3]) -> [f32; 3] {
    let fy = (l[0] + 16.0) / 116.0;
    let xyz = [lab_finv(fy + l[1] / 500.0) * D65[0], lab_finv(fy) * D65[1], lab_finv(fy - l[2] / 200.0) * D65[2]];
    mul3(&XYZ2RGB, xyz).map(|v| v.max(0.0))
}

/// Grayworld: in linear light, shift every pixel's Lab a/b by the picture's mean a/b, so the
/// average colour becomes neutral.
pub(super) fn grayworld(img: &mut Image) {
    let (mut sa, mut sb, mut n) = (0.0f64, 0.0f64, 0.0f64);
    for p in img.px.chunks(4) {
        if p[3] <= 1e-4 {
            continue;
        }
        let lab = to_lab([p[0] / p[3], p[1] / p[3], p[2] / p[3]]);
        sa += (lab[1] * p[3]) as f64;
        sb += (lab[2] * p[3]) as f64;
        n += p[3] as f64;
    }
    if n <= 0.0 {
        return;
    }
    let (ma, mb) = ((sa / n) as f32, (sb / n) as f32);
    img.px.par_chunks_mut(4).for_each(|p| {
        let a = p[3];
        if a <= 1e-6 {
            return;
        }
        let mut lab = to_lab([p[0] / a, p[1] / a, p[2] / a]);
        lab[1] -= ma;
        lab[2] -= mb;
        let o = from_lab(lab);
        p[0] = o[0] * a;
        p[1] = o[1] * a;
        p[2] = o[2] * a;
    });
}

/// Histogram Equalizer: the luma moves toward its equalised value by `strength`, by at most
/// `intensity` (so the picture does not wash out); antibanding adds a little dither.
pub(super) fn histeq(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let strength = fv(e, "strength", cx).clamp(0.0, 1.0);
    let cap = fv(e, "intensity", cx).clamp(0.0, 1.0);
    if strength <= 0.0 || cap <= 0.0 {
        return;
    }
    let d = Disp::of(img);
    let mut h = [0.0f64; 256];
    for p in &d.px {
        if p[3] > 1e-4 {
            h[(luma(*p) * 255.0).round().clamp(0.0, 255.0) as usize] += p[3] as f64;
        }
    }
    let map = cdf(&h);
    let dither = [0.0, 1.0, 3.0][pick(e, "antibanding").min(2) as usize] / 255.0;
    let frame = frame_no(cx);
    map_disp(img, |c, x, y| {
        let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let eq = map[(l * 255.0).round().clamp(0.0, 255.0) as usize];
        let mut dl = ((eq - l) * strength).clamp(-cap, cap);
        if dither > 0.0 {
            dl += (rnd(x as i64, y as i64, frame, 0x4e51) - 0.5) * 2.0 * dither;
        }
        c.map(|v| (v + dl).clamp(0.0, 1.0))
    });
}

/// Per-channel (min, max) of the opaque part of a picture.
fn channel_range(img: &Image) -> Option<([f32; 3], [f32; 3])> {
    let d = Disp::of(img);
    let (mut lo, mut hi) = ([1.0f32; 3], [0.0f32; 3]);
    let mut any = false;
    for p in &d.px {
        if p[3] <= 0.01 {
            continue;
        }
        any = true;
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    any.then_some((lo, hi))
}

/// Normaliz0r / Normalize RGB Video: stretch each channel's input range (rolling-averaged over
/// `smoothing` previous frames) to the black … white point colours; Independence mixes per-channel
/// and linked ranges, Strength mixes with the source.
pub(super) fn normalize(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some((mut lo, mut hi)) = channel_range(img) else { return };
    let smoothing = fv(e, "smoothing", cx).round().clamp(0.0, 30.0) as usize;
    let mut count = 1.0f32;
    for k in 1..=smoothing {
        let Some(prev) = neighbour(img, e, cx, -(k as f64)) else { break };
        if let Some((l, h)) = channel_range(&prev) {
            for c in 0..3 {
                lo[c] += l[c];
                hi[c] += h[c];
            }
            count += 1.0;
        }
    }
    let (lo, hi) = (lo.map(|v| v / count), hi.map(|v| v / count));
    let ind = fv(e, "independence", cx).clamp(0.0, 1.0);
    let strength = fv(e, "strength", cx).clamp(0.0, 1.0);
    let (blo, bhi) = (lo[0].min(lo[1]).min(lo[2]), hi[0].max(hi[1]).max(hi[2]));
    let lo = lo.map(|v| blo + (v - blo) * ind);
    let hi = hi.map(|v| bhi + (v - bhi) * ind);
    let (bp, wp) = (color(e, "blackpt", cx), color(e, "whitept", cx));
    map_disp(img, |c, _, _| {
        [0, 1, 2].map(|k| {
            let t = (c[k] - lo[k]) / (hi[k] - lo[k]).max(1e-4);
            let o = bp[k] + t.clamp(0.0, 1.0) * (wp[k] - bp[k]);
            (c[k] + (o - c[k]) * strength).clamp(0.0, 1.0)
        })
    });
}

/// Swapuv: exchange the U and V chroma planes.
pub(super) fn swapuv(img: &mut Image) {
    on_yuva(img, |planes, _, _| planes.swap(1, 2));
}

/// Technicolor: over-saturate along the blue-yellow (U) and red-green (V) axes.
pub(super) fn technicolor(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let ku = 1.0 + fv(e, "blue_yellow", cx) / 200.0;
    let kv = 1.0 + fv(e, "red_green", cx) / 200.0;
    on_yuva(img, |planes, _, _| {
        planes[1].par_iter_mut().for_each(|v| *v = (0.5 + (*v - 0.5) * ku).clamp(0.0, 1.0));
        planes[2].par_iter_mut().for_each(|v| *v = (0.5 + (*v - 0.5) * kv).clamp(0.0, 1.0));
    });
}

// Bradford cone response
const BRADFORD: [[f32; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
const BRADFORD_INV: [[f32; 3]; 3] = [[0.9870, -0.1471, 0.1600], [0.4323, 0.5184, 0.0493], [-0.0085, 0.0400, 0.9685]];

/// CIE xy of a blackbody (Planckian locus, Kim et al. cubic approximation), 1667 K … 25000 K.
fn planck_xy(t: f32) -> (f32, f32) {
    let t = t.clamp(1667.0, 25000.0) as f64;
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.2661239e9 / t3 - 0.2343589e6 / t2 + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / t3 + 2.1070379e6 / t2 + 0.2226347e3 / t + 0.240390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.1063814 * x3 - 1.34811020 * x2 + 2.18555832 * x - 0.20219683
    } else if t <= 4000.0 {
        -0.9549476 * x3 - 1.37418593 * x2 + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x3 - 5.87338670 * x2 + 3.75112997 * x - 0.37001483
    };
    (x as f32, y as f32)
}
fn xy_to_xyz(x: f32, y: f32) -> [f32; 3] {
    [x / y.max(1e-6), 1.0, (1.0 - x - y) / y.max(1e-6)]
}

/// White Balance (LMS): von Kries adaptation in Bradford cone space that makes the picked neutral
/// colour the white of the chosen colour temperature (6500 K = D65, no change for a white pick).
pub(super) fn wb_lms(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let n = color(e, "neutral", cx);
    let neutral = mul3(&RGB2XYZ, crate::effects::dec([n[0], n[1], n[2]]));
    let t = fv(e, "temperature", cx);
    // target white relative to D65, at the neutral's luminance
    let (tx, ty) = planck_xy(t);
    let (dx, dy) = planck_xy(6504.0);
    let tw = xy_to_xyz(tx, ty);
    let dw = xy_to_xyz(dx, dy);
    let target = [0, 1, 2].map(|k| D65[k] * tw[k] / dw[k] * neutral[1]);
    let (ln, lt) = (mul3(&BRADFORD, neutral), mul3(&BRADFORD, target));
    if ln.iter().any(|v| *v <= 1e-5) {
        return;
    }
    let g = [0, 1, 2].map(|k| lt[k] / ln[k]);
    if g.iter().all(|v| (v - 1.0).abs() < 1e-5) {
        return;
    }
    img.px.par_chunks_mut(4).for_each(|p| {
        let xyz = mul3(&RGB2XYZ, [p[0], p[1], p[2]]);
        let mut lms = mul3(&BRADFORD, xyz);
        for k in 0..3 {
            lms[k] *= g[k];
        }
        let o = mul3(&XYZ2RGB, mul3(&BRADFORD_INV, lms));
        p[0] = o[0].max(0.0);
        p[1] = o[1].max(0.0);
        p[2] = o[2].max(0.0);
    });
}

// ------------------------------------------------------------------ Image Adjustments: matrices

/// (Kr, Kb) of a [`redrob_film_project::effect::COLOR_MATRICES`] entry.
fn kr_kb(m: u32) -> (f32, f32) {
    match m {
        0 => (0.2126, 0.0722),
        1 => (0.30, 0.11),
        6 => (0.212, 0.087),
        7 => (0.2627, 0.0593),
        _ => (0.299, 0.114),
    }
}
#[inline]
fn rgb_to_ycc(c: [f32; 3], (kr, kb): (f32, f32)) -> [f32; 3] {
    let y = kr * c[0] + (1.0 - kr - kb) * c[1] + kb * c[2];
    [y, (c[2] - y) / (2.0 * (1.0 - kb)), (c[0] - y) / (2.0 * (1.0 - kr))]
}
#[inline]
fn ycc_to_rgb(p: [f32; 3], (kr, kb): (f32, f32)) -> [f32; 3] {
    let r = p[0] + 2.0 * (1.0 - kr) * p[2];
    let b = p[0] + 2.0 * (1.0 - kb) * p[1];
    let g = (p[0] - kr * r - kb * b) / (1.0 - kr - kb);
    [r, g, b]
}

/// Color Matrix: re-read the picture's YCbCr with the destination matrix after it was formed with
/// the source matrix (fixes footage decoded with the wrong matrix, or simulates it).
pub(super) fn colormatrix(img: &mut Image, e: &EffectInstance) {
    let (s, d) = (pick(e, "src"), pick(e, "dst"));
    if kr_kb(s) == kr_kb(d) {
        return;
    }
    let (ks, kd) = (kr_kb(s), kr_kb(d));
    map_disp(img, |c, _, _| ycc_to_rgb(rgb_to_ycc(c, ks), kd).map(|v| v.clamp(0.0, 1.0)));
}

/// Decode a Color Space transfer curve (`CS_TRC` index ≥ 1) to linear light.
fn trc_decode(trc: u32, v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    match trc {
        2 | 4 => v.powf(2.2),
        3 | 5 => v.powf(2.8),
        7 => {
            if v < 0.0913 {
                v / 4.0
            } else {
                ((v + 0.1115) / 1.1115).powf(1.0 / 0.45)
            }
        }
        8 => crate::effects::dec([v, v, v])[0],
        _ => {
            if v < 0.081 {
                v / 4.5
            } else {
                ((v + 0.099) / 1.099).powf(1.0 / 0.45)
            }
        }
    }
}
fn trc_encode(trc: u32, l: f32) -> f32 {
    let l = l.max(0.0);
    match trc {
        2 | 4 => l.powf(1.0 / 2.2),
        3 | 5 => l.powf(1.0 / 2.8),
        7 => {
            if l < 0.0228 {
                4.0 * l
            } else {
                1.1115 * l.powf(0.45) - 0.1115
            }
        }
        8 => crate::effects::enc([l, l, l])[0],
        _ => {
            if l < 0.018 {
                4.5 * l
            } else {
                1.099 * l.powf(0.45) - 0.099
            }
        }
    }
}
/// Primaries (r, g, b, white) xy of a CS_PRIMARIES entry (index ≥ 1).
fn primaries(p: u32) -> [(f32, f32); 4] {
    const D65W: (f32, f32) = (0.3127, 0.3290);
    const CW: (f32, f32) = (0.310, 0.316);
    match p {
        2 => [(0.67, 0.33), (0.21, 0.71), (0.14, 0.08), CW],
        3 => [(0.64, 0.33), (0.29, 0.60), (0.15, 0.06), D65W],
        4 | 5 => [(0.630, 0.340), (0.310, 0.595), (0.155, 0.070), D65W],
        6 => [(0.681, 0.319), (0.243, 0.692), (0.145, 0.049), CW],
        7 => [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046), D65W],
        8 => [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060), (0.314, 0.351)],
        9 => [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060), D65W],
        10 => [(0.630, 0.340), (0.295, 0.605), (0.155, 0.077), D65W],
        _ => [(0.64, 0.33), (0.30, 0.60), (0.15, 0.06), D65W],
    }
}
fn inv3(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let k = if det.abs() < 1e-12 { 0.0 } else { 1.0 / det };
    [
        [(e * i - f * h) * k, (c * h - b * i) * k, (b * f - c * e) * k],
        [(f * g - d * i) * k, (a * i - c * g) * k, (c * d - a * f) * k],
        [(d * h - e * g) * k, (b * g - a * h) * k, (a * e - b * d) * k],
    ]
}
fn matmul(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            o[r][c] = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    o
}
/// RGB → XYZ matrix of a set of primaries.
fn rgb_to_xyz(p: [(f32, f32); 4]) -> [[f32; 3]; 3] {
    let cols = [xy_to_xyz(p[0].0, p[0].1), xy_to_xyz(p[1].0, p[1].1), xy_to_xyz(p[2].0, p[2].1)];
    let m = [[cols[0][0], cols[1][0], cols[2][0]], [cols[0][1], cols[1][1], cols[2][1]], [cols[0][2], cols[1][2], cols[2][2]]];
    let s = mul3(&inv3(&m), xy_to_xyz(p[3].0, p[3].1));
    [0, 1, 2].map(|r| [m[r][0] * s[0], m[r][1] * s[1], m[r][2] * s[2]])
}
/// Chromatic adaptation XYZ(src white) → XYZ(dst white): 0 identity, 1 Bradford, 2 von Kries.
fn adaptation(mode: u32, src: (f32, f32), dst: (f32, f32)) -> [[f32; 3]; 3] {
    const VK: [[f32; 3]; 3] = [[0.40024, 0.70760, -0.08081], [-0.22630, 1.16532, 0.04570], [0.0, 0.0, 0.91822]];
    let cone = match mode {
        1 => BRADFORD,
        2 => VK,
        _ => return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };
    let (s, d) = (mul3(&cone, xy_to_xyz(src.0, src.1)), mul3(&cone, xy_to_xyz(dst.0, dst.1)));
    let g = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    matmul(&inv3(&cone), &matmul(&g, &cone))
}

/// `(space, trc, primaries)` of a CS_ALL preset (index ≥ 1).
fn preset(all: u32) -> (u32, u32, u32) {
    match all {
        2 => (3, 2, 2),
        3 => (3, 3, 3),
        4 => (4, 6, 4),
        5 => (3, 6, 3),
        6 => (4, 6, 4),
        7 => (5, 7, 5),
        8 => (7, 10, 7),
        _ => (1, 1, 1),
    }
}
/// (Kr, Kb) of a CS_SPACE entry; `None` for YCgCo.
fn space_k(s: u32) -> Option<(f32, f32)> {
    match s {
        2 => Some((0.30, 0.11)),
        3 | 4 => Some((0.299, 0.114)),
        5 => Some((0.212, 0.087)),
        6 => None,
        7 => Some((0.2627, 0.0593)),
        _ => Some((0.2126, 0.0722)),
    }
}

/// Color Space: convert the picture from its input properties (BT.709 full range unless
/// overridden) to the output colour space, transfer, primaries (with the chosen whitepoint
/// adaptation; Fast skips gamma and primaries), range and format (chroma subsampling, bit depth,
/// optional Floyd-Steinberg dither). The converted signal is then shown the way the sequence
/// reads a BT.709 picture, which is what the frame looks like downstream.
pub(super) fn colorspace(img: &mut Image, e: &EffectInstance) {
    let pv = |id: &str| pick(e, id);
    let (mut is, mut it, mut ip) = preset(pv("iall").max(1));
    if pv("ispace") > 0 {
        is = pv("ispace");
    }
    if pv("itrc") > 0 {
        it = pv("itrc");
    }
    if pv("iprimaries") > 0 {
        ip = pv("iprimaries");
    }
    let in_limited = pv("irange") == 1;
    let (mut os, mut ot, mut op) = preset(pv("all").max(1));
    if pv("space") > 0 {
        os = pv("space");
    }
    if pv("trc") > 0 {
        ot = pv("trc");
    }
    if pv("primaries") > 0 {
        op = pv("primaries");
    }
    let out_limited = match pv("range") {
        1 => true,
        2 => false,
        _ => in_limited,
    };
    let fast = flag(e, "fast");
    let fmt = pv("format");
    let (sub, bits) = if fmt == 0 { (2u32, 8u32) } else { ((fmt - 1) / 3, [8, 10, 12][((fmt - 1) % 3) as usize]) };
    // sub: 0 = 4:2:0, 1 = 4:2:2, 2 = 4:4:4
    let (pin, pout) = (primaries(ip), primaries(op));
    let conv = matmul(&inv3(&rgb_to_xyz(pout)), &matmul(&adaptation(pv("wpadapt"), pin[3], pout[3]), &rgb_to_xyz(pin)));
    let kin = space_k(is);
    let kout = space_k(os);
    let shown = (0.2126f32, 0.0722f32);
    let mut d = Disp::of(img);
    let (w, h) = (d.w, d.h);
    // 1) input signal → output RGB, then → output Y'CbCr (offset chroma, 0..1)
    let mut ycc: Vec<[f32; 3]> = d
        .px
        .par_iter()
        .map(|p| {
            // the picture as an input signal: Y'CbCr formed with the input matrix and range
            let mut rgb = [p[0], p[1], p[2]];
            if in_limited {
                // a limited-range input occupies 16…235 of the code range
                rgb = rgb.map(|v| (v - 16.0 / 255.0) * 255.0 / 219.0);
            }
            // an input matrix override: the data was formed with that matrix, not the shown one
            if let Some(k) = kin
                && k != shown
            {
                rgb = ycc_to_rgb(rgb_to_ycc(rgb, shown), k);
            }
            if !fast {
                let lin = rgb.map(|v| trc_decode(it, v));
                rgb = mul3(&conv, lin).map(|v| trc_encode(ot, v));
            }
            let c = match kout {
                Some(k) => rgb_to_ycc(rgb, k),
                None => {
                    let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
                    [0.25 * r + 0.5 * g + 0.25 * b, -0.25 * r + 0.5 * g - 0.25 * b, 0.5 * r - 0.5 * b]
                }
            };
            if out_limited { [16.0 / 255.0 + c[0] * 219.0 / 255.0, 0.5 + c[1] * 224.0 / 255.0, 0.5 + c[2] * 224.0 / 255.0] } else { [c[0], c[1] + 0.5, c[2] + 0.5] }
        })
        .collect();
    // 2) chroma subsampling
    if sub < 2 {
        let bh = if sub == 0 { 2 } else { 1 };
        for by in (0..h).step_by(bh) {
            for bx in (0..w).step_by(2) {
                let mut s = [0.0f32; 2];
                let mut n = 0.0;
                for y in by..(by + bh).min(h) {
                    for x in bx..(bx + 2).min(w) {
                        let q = ycc[y * w + x];
                        s[0] += q[1];
                        s[1] += q[2];
                        n += 1.0;
                    }
                }
                for y in by..(by + bh).min(h) {
                    for x in bx..(bx + 2).min(w) {
                        ycc[y * w + x][1] = s[0] / n;
                        ycc[y * w + x][2] = s[1] / n;
                    }
                }
            }
        }
    }
    // 3) quantisation (Floyd-Steinberg error diffusion on request)
    let q = ((1u32 << bits) - 1) as f32;
    if pv("dither") == 1 {
        let mut err = vec![[0.0f32; 3]; w * h];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                for k in 0..3 {
                    let v = ycc[i][k] + err[i][k];
                    let o = (v * q).round().clamp(0.0, q) / q;
                    let er = v - o;
                    ycc[i][k] = o;
                    if x + 1 < w {
                        err[i + 1][k] += er * 7.0 / 16.0;
                    }
                    if y + 1 < h {
                        if x > 0 {
                            err[i + w - 1][k] += er * 3.0 / 16.0;
                        }
                        err[i + w][k] += er * 5.0 / 16.0;
                        if x + 1 < w {
                            err[i + w + 1][k] += er / 16.0;
                        }
                    }
                }
            }
        }
    } else {
        ycc.par_iter_mut().for_each(|c| *c = c.map(|v| (v * q).round().clamp(0.0, q) / q));
    }
    // 4) shown as the sequence reads it: BT.709, in the input's range
    d.px.par_iter_mut().zip(ycc.par_iter()).for_each(|(p, c)| {
        let c = if in_limited {
            [(c[0] - 16.0 / 255.0) * 255.0 / 219.0, (c[1] - 0.5) * 255.0 / 224.0, (c[2] - 0.5) * 255.0 / 224.0]
        } else {
            [c[0], c[1] - 0.5, c[2] - 0.5]
        };
        let mut rgb = ycc_to_rgb(c, shown);
        if in_limited {
            rgb = rgb.map(|v| 16.0 / 255.0 + v * 219.0 / 255.0);
        }
        p[0] = rgb[0].clamp(0.0, 1.0);
        p[1] = rgb[1].clamp(0.0, 1.0);
        p[2] = rgb[2].clamp(0.0, 1.0);
    });
    d.store(img);
}

// ------------------------------------------------------------------ Image Adjustments: spatial

/// Deband: a pixel whose value is within the plane threshold of four reference pixels (at a
/// random or fixed distance and direction) takes their average.
pub(super) fn deband(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let thr = [fv(e, "thr1", cx), fv(e, "thr2", cx), fv(e, "thr3", cx), fv(e, "thr4", cx)];
    let range = fv(e, "range", cx) * cx.px_scale;
    let dir = fv(e, "direction", cx);
    let blur = flag(e, "blur");
    let coupling = flag(e, "coupling");
    let frame = frame_no(cx);
    on_yuva(img, |planes, w, h| {
        let src = planes.clone();
        let out: Vec<[f32; 4]> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                let r = if range >= 0.0 { rnd(x, y, frame, 0xdeb1) * range } else { -range };
                let a = if dir >= 0.0 { rnd(x, y, frame, 0xdeb2) * dir } else { -dir };
                let (dx, dy) = ((a.cos() * r).round() as isize, (a.sin() * r).round() as isize);
                let refs = [(dx, dy), (-dx, -dy), (-dy, dx), (dy, -dx)];
                let mut o = [0.0f32; 4];
                let mut banded = [false; 4];
                for k in 0..4 {
                    let p = &src[k];
                    let c = p[i];
                    let vals = refs.map(|(ox, oy)| at(p, w, h, x as isize + ox, y as isize + oy));
                    let avg = vals.iter().sum::<f32>() / 4.0;
                    banded[k] = if blur { (c - avg).abs() < thr[k] } else { vals.iter().all(|v| (c - v).abs() < thr[k]) };
                    o[k] = if banded[k] { avg } else { c };
                }
                if coupling && !banded.iter().all(|b| *b) {
                    return [src[0][i], src[1][i], src[2][i], src[3][i]];
                }
                o
            })
            .collect();
        for k in 0..4 {
            planes[k] = out.iter().map(|o| o[k]).collect();
        }
    });
}

/// Dilation (`max`) / Erosion (`min`): each plane takes the local 3×3 extreme over the neighbours
/// the Coordinates bits select (bit 0 = top-left … bit 7 = bottom-right), changing by at most the
/// plane threshold (of 65535).
pub(super) fn morph(img: &mut Image, e: &EffectInstance, cx: &FxCtx, dilate: bool) {
    let thr = [fv(e, "thr1", cx), fv(e, "thr2", cx), fv(e, "thr3", cx), fv(e, "thr4", cx)].map(|v| v.clamp(0.0, 65535.0) / 65535.0);
    let bits = fv(e, "coordinates", cx).round().clamp(0.0, 255.0) as u32;
    const N: [(isize, isize); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];
    on_yuva(img, |planes, w, h| {
        for (k, plane) in planes.iter_mut().enumerate() {
            if thr[k] <= 0.0 {
                continue;
            }
            let src = plane.clone();
            plane.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, v) in row.iter_mut().enumerate() {
                    let c = src[y * w + x];
                    let mut m = c;
                    for (b, (dx, dy)) in N.iter().enumerate() {
                        if bits & (1 << b) != 0 {
                            let n = at(&src, w, h, x as isize + dx, y as isize + dy);
                            m = if dilate { m.max(n) } else { m.min(n) };
                        }
                    }
                    *v = c + (m - c).clamp(-thr[k], thr[k]);
                }
            });
        }
    });
}

/// The pixel-art magnifiers: `n`× magnification by the chosen rule set. On a clip shown larger
/// than its pixels the layer itself is enlarged (see [`magnify`], called by the clip renderer);
/// otherwise the magnified picture is area-averaged back to the layer's size.
#[derive(Clone, Copy)]
pub(super) enum Scaler {
    Epx,
    Hqx,
    Sai,
    Xbr,
}

fn scaler_of(id: &str) -> Option<Scaler> {
    match id {
        "k_epx" => Some(Scaler::Epx),
        "k_hqx" => Some(Scaler::Hqx),
        "k_super2xsai" => Some(Scaler::Sai),
        "k_xbr" => Some(Scaler::Xbr),
        _ => None,
    }
}

fn scale_factor(e: &EffectInstance, kind: Scaler) -> usize {
    match kind {
        Scaler::Sai => 2,
        Scaler::Epx => [2, 3][pick(e, "n").min(1) as usize],
        Scaler::Hqx | Scaler::Xbr => [2, 3, 4][pick(e, "n").min(2) as usize],
    }
}

/// Sub-pixel (`sx`, `sy`) of the `n`×`n` cell that source pixel (`x`, `y`) magnifies into.
fn scaler_sample(d: &Disp, kind: Scaler, n: usize, x: isize, y: isize, sx: usize, sy: usize) -> [f32; 4] {
    let same = |a: [f32; 4], b: [f32; 4]| {
        let (ya, yb) = (to_yuv(a), to_yuv(b));
        (ya[0] - yb[0]).abs() < 48.0 / 255.0 && (ya[1] - yb[1]).abs() < 7.0 / 255.0 && (ya[2] - yb[2]).abs() < 6.0 / 255.0 && (a[3] - b[3]).abs() < 0.1
    };
    let mix = |a: [f32; 4], b: [f32; 4], t: f32| [0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * t);
    let p = d.get(x, y);
    let (a, b, c, dd) = (d.get(x, y - 1), d.get(x + 1, y), d.get(x - 1, y), d.get(x, y + 1));
    let (u, v) = ((sx as f32 + 0.5) / n as f32, (sy as f32 + 0.5) / n as f32);
    // the corner this sub-pixel leans to: its two edge neighbours and diagonal
    let (h_n, v_n) = (if u < 0.5 { c } else { b }, if v < 0.5 { a } else { dd });
    let diag = d.get(if u < 0.5 { x - 1 } else { x + 1 }, if v < 0.5 { y - 1 } else { y + 1 });
    let centre = (u - 0.5).abs() < 0.17 && (v - 0.5).abs() < 0.17 && n == 3;
    let edge = same(h_n, v_n) && !same(h_n, p);
    match kind {
        // EPX: a corner takes the colour of its two edge neighbours when they agree and neither
        // continues into the opposite neighbour across the pixel (3x: the corners of the cell)
        Scaler::Epx => {
            let (oh, ov) = (if u < 0.5 { b } else { c }, if v < 0.5 { dd } else { a });
            let corner = !centre && (u - 0.5).abs() > 0.2 && (v - 0.5).abs() > 0.2;
            if corner && same(h_n, v_n) && !same(v_n, oh) && !same(h_n, ov) { h_n } else { p }
        }
        // HQx: blend toward the agreeing neighbours, weighted by closeness to the corner, more
        // when the diagonal continues the edge
        Scaler::Hqx => {
            if edge {
                let k = ((u - 0.5).abs() + (v - 0.5).abs()).clamp(0.0, 1.0);
                let t = if same(diag, h_n) { 0.75 * k } else { 0.5 * k };
                mix(p, h_n, t)
            } else {
                p
            }
        }
        // 2xSaI-style: corners blend half with the agreeing pair, the pixel otherwise
        // interpolates along the dominant diagonal
        Scaler::Sai => {
            if edge {
                mix(p, h_n, 0.5)
            } else if same(p, diag) {
                p
            } else {
                mix(p, mix(h_n, v_n, 0.5), 0.25)
            }
        }
        // xBR: an edge across the corner (neighbours agree, the diagonal differs from them) is
        // cut along the anti-diagonal with a soft border
        Scaler::Xbr => {
            if edge && !same(diag, p) {
                let dist = (u - 0.5).abs() + (v - 0.5).abs();
                mix(p, h_n, smooth(0.45, 0.75, dist))
            } else {
                p
            }
        }
    }
}

/// In-place form (the layer keeps its size): each pixel becomes the average of its magnified cell.
pub(super) fn pixel_scaler(img: &mut Image, e: &EffectInstance, kind: Scaler) {
    let n = scale_factor(e, kind);
    let d = Disp::of(img);
    let (w, h) = (d.w, d.h);
    if w < 2 || h < 2 {
        return;
    }
    let out: Vec<[f32; 4]> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            let mut acc = [0.0f32; 4];
            for sy in 0..n {
                for sx in 0..n {
                    let s = scaler_sample(&d, kind, n, x, y, sx, sy);
                    for k in 0..4 {
                        acc[k] += s[k];
                    }
                }
            }
            acc.map(|v| v / (n * n) as f32)
        })
        .collect();
    Disp { w, h, px: out }.store(img);
}

/// Enlarging form, for a layer shown at `shown` output pixels per layer pixel: when that is at
/// least 2, replace the picture by its magnification (factor capped by `shown` and by a pixel
/// budget) and return the factor; `None` leaves the effect to [`pixel_scaler`].
pub(crate) fn magnify(img: &mut Image, e: &EffectInstance, shown: f64) -> Option<usize> {
    let kind = scaler_of(&e.effect)?;
    let n = scale_factor(e, kind).min(shown.ceil().max(1.0) as usize);
    let (w, h) = (img.w, img.h);
    if n < 2 || w < 2 || h < 2 || w * h * n * n > 16_000_000 {
        return None;
    }
    let d = Disp::of(img);
    let (ow, oh) = (w * n, h * n);
    let px: Vec<[f32; 4]> = (0..ow * oh)
        .into_par_iter()
        .map(|i| {
            let (ox, oy) = (i % ow, i / ow);
            scaler_sample(&d, kind, n, (ox / n) as isize, (oy / n) as isize, ox % n, oy % n)
        })
        .collect();
    *img = Image::new(ow, oh);
    Disp { w: ow, h: oh, px }.store(img);
    Some(n)
}

/// Whether `id` is one of the magnifiers [`magnify`] can enlarge a layer with.
pub(crate) fn is_magnifier(id: &str) -> bool {
    scaler_of(id).is_some()
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Interleave - Deinterleave: per channel group (luma, chroma, alpha), split the two fields into
/// the top and bottom halves (deinterleave) or weave them back (interleave); Swap exchanges the
/// even and odd lines.
pub(super) fn interleave(img: &mut Image, e: &EffectInstance) {
    let modes = [pick(e, "luma_mode"), pick(e, "chroma_mode"), pick(e, "alpha_mode")];
    let swaps = [flag(e, "luma_swap"), flag(e, "chroma_swap"), flag(e, "alpha_swap")];
    if modes == [0; 3] && swaps == [false; 3] {
        return;
    }
    on_yuva(img, |planes, w, h| {
        let half = h.div_ceil(2);
        for (k, plane) in planes.iter_mut().enumerate() {
            let g = [0, 1, 1, 2][k];
            let src = plane.clone();
            let row = |p: &[f32], y: usize| p[y * w..(y + 1) * w].to_vec();
            let mut out = src.clone();
            match modes[g] {
                1 => {
                    for y in 0..h {
                        let dst = if y % 2 == 0 { y / 2 } else { half + y / 2 };
                        out[dst * w..(dst + 1) * w].copy_from_slice(&row(&src, y));
                    }
                }
                2 => {
                    for y in 0..h {
                        let from = if y % 2 == 0 { y / 2 } else { half + y / 2 };
                        out[y * w..(y + 1) * w].copy_from_slice(&row(&src, from));
                    }
                }
                _ => {}
            }
            if swaps[g] {
                let s2 = out.clone();
                for y in 0..h {
                    let other = if y % 2 == 0 { y + 1 } else { y - 1 };
                    if other < h {
                        out[y * w..(y + 1) * w].copy_from_slice(&row(&s2, other));
                    }
                }
            }
            *plane = out;
        }
    });
}

/// Kernel Deinterlacer: the lines of the second field (the first with Swap Fields) are rebuilt
/// where they comb against their neighbours by more than the threshold (0 = everywhere), from the
/// lines around them plus a vertical-detail term from the field itself; optional extra and
/// two-way sharpening; Map paints rebuilt pixels white.
pub(super) fn kerndeint(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let thr = fv(e, "threshold", cx).clamp(0.0, 255.0) / 255.0;
    let (map, order, sharp, twoway) = (flag(e, "map"), flag(e, "order"), flag(e, "sharp"), flag(e, "twoway"));
    let d = Disp::of(img);
    let (w, h) = (d.w, d.h);
    let rebuild = usize::from(!order);
    let get = |x: usize, y: isize| d.get(x as isize, y);
    let mut out = d.px.clone();
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        if y % 2 != rebuild {
            return;
        }
        let y = y as isize;
        for (x, o) in row.iter_mut().enumerate() {
            let (cur, up, dn) = (get(x, y), get(x, y - 1), get(x, y + 1));
            let comb = (0..3).map(|k| (cur[k] - (up[k] + dn[k]) / 2.0).abs()).fold(0.0f32, f32::max);
            if thr > 0.0 && comb <= thr {
                continue;
            }
            if map {
                *o = [1.0, 1.0, 1.0, cur[3]];
                continue;
            }
            let (u2, d2) = (get(x, y - 2), get(x, y + 2));
            let mut v = [0.0f32; 4];
            for k in 0..4 {
                let mut s = 0.5 * (up[k] + dn[k]) + 0.125 * (2.0 * cur[k] - u2[k] - d2[k]);
                if sharp {
                    let (u3, d3) = (get(x, y - 3), get(x, y + 3));
                    s += 0.0625 * (up[k] + dn[k] - u3[k] - d3[k]);
                }
                if twoway {
                    s += 0.0625 * (2.0 * cur[k] - up[k] - dn[k]);
                }
                v[k] = s.clamp(0.0, 1.0);
            }
            v[3] = cur[3];
            *o = v;
        }
    });
    Disp { w, h, px: out }.store(img);
}

/// Motion Compensation Deinterlacer: keeps the first field (by parity) and rebuilds the other
/// from the previous frame along per-block motion vectors (search radius by mode; Extra Slow also
/// searches the next frame), smoothed `qp` times over neighbouring blocks; where no good match is
/// found (or without neighbouring frames) the line is interpolated from the kept field.
pub(super) fn mcdeint(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let mode = pick(e, "mode").min(3);
    let keep = usize::from(pick(e, "parity") == 1); // 0: top field first (keep even lines)
    let qp = fv(e, "qp", cx).round().clamp(0.0, 60.0) as usize;
    let d = Disp::of(img);
    let (w, h) = (d.w, d.h);
    let refs: Vec<Disp> = [Some(-1.0), (mode == 3).then_some(1.0)]
        .into_iter()
        .flatten()
        .filter_map(|dt| neighbour(img, e, cx, dt).map(|f| Disp::of(&f)))
        .collect();
    let spatial = |x: usize, y: usize| {
        let (u, dn) = (d.get(x as isize, y as isize - 1), d.get(x as isize, y as isize + 1));
        [0, 1, 2, 3].map(|k| 0.5 * (u[k] + dn[k]))
    };
    let radius = [2isize, 4, 8, 8][mode as usize];
    const B: usize = 8;
    let (bw, bh) = (w.div_ceil(B), h.div_ceil(B));
    // per block: (ref index, dx, dy, cost)
    let mut vecs: Vec<(usize, f32, f32, f32)> = (0..bw * bh)
        .into_par_iter()
        .map(|bi| {
            let (bx, by) = ((bi % bw) * B, (bi / bw) * B);
            let mut best = (0usize, 0isize, 0isize, f32::INFINITY);
            for (ri, r) in refs.iter().enumerate() {
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        let mut sad = 0.0;
                        let mut n = 0.0;
                        for y in (by..(by + B).min(h)).filter(|y| y % 2 == keep) {
                            for x in bx..(bx + B).min(w) {
                                let (a, b) = (d.px[y * w + x], r.get(x as isize + dx, y as isize + dy));
                                sad += (luma(a) - luma(b)).abs();
                                n += 1.0;
                            }
                        }
                        let c = sad / f32::max(n, 1.0) + (dx.abs() + dy.abs()) as f32 * 1e-4;
                        if c < best.3 {
                            best = (ri, dx, dy, c);
                        }
                    }
                }
            }
            (best.0, best.1 as f32, best.2 as f32, best.3)
        })
        .collect();
    for _ in 0..qp.min(8) {
        let src = vecs.clone();
        for by in 0..bh {
            for bx in 0..bw {
                let i = by * bw + bx;
                let (mut sx, mut sy, mut n) = (src[i].1, src[i].2, 1.0);
                for (ox, oy) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (bx as isize + ox, by as isize + oy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < bw && (ny as usize) < bh {
                        let j = ny as usize * bw + nx as usize;
                        if src[j].0 == src[i].0 {
                            sx += src[j].1;
                            sy += src[j].2;
                            n += 1.0;
                        }
                    }
                }
                vecs[i].1 = sx / n;
                vecs[i].2 = sy / n;
            }
        }
    }
    let mut out = d.px.clone();
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        if y % 2 == keep {
            return;
        }
        for (x, o) in row.iter_mut().enumerate() {
            let sp = spatial(x, y);
            let v = vecs[(y / B).min(bh - 1) * bw + (x / B).min(bw - 1)];
            *o = match refs.get(v.0) {
                Some(r) if v.3 < 0.06 => {
                    let mc = r.sample(x as f32 + 0.5 + v.1, y as f32 + 0.5 + v.2);
                    let k = (1.0 - v.3 / 0.06).clamp(0.0, 1.0);
                    [0, 1, 2, 3].map(|c| sp[c] + (mc[c] - sp[c]) * k)
                }
                _ => sp,
            };
        }
    });
    Disp { w, h, px: out }.store(img);
}

/// Phase: delays one field by one frame time (its lines come from the previous frame), so the
/// field order changes. The analysing modes pick, per frame, the candidate (delay top, delay
/// bottom, keep) whose woven picture combs least; with no field flags available the flag modes
/// analyse too.
pub(super) fn phase(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let mode = pick(e, "mode");
    if mode == 2 {
        return;
    }
    let Some(prev) = neighbour(img, e, cx, -1.0) else { return };
    let (cur, prev) = (Disp::of(img), Disp::of(&prev));
    let (w, h) = (cur.w, cur.h);
    let weave = |delay: Option<usize>| -> Vec<[f32; 4]> {
        let mut px = cur.px.clone();
        if let Some(f) = delay {
            for y in (f..h).step_by(2) {
                px[y * w..(y + 1) * w].copy_from_slice(&prev.px[y * w..(y + 1) * w]);
            }
        }
        px
    };
    let comb = |px: &[[f32; 4]]| -> f32 {
        (1..h.saturating_sub(1))
            .into_par_iter()
            .map(|y| (0..w).map(|x| (luma(px[y * w + x]) - 0.5 * (luma(px[(y - 1) * w + x]) + luma(px[(y + 1) * w + x]))).abs()).sum::<f32>())
            .sum()
    };
    let candidates: &[Option<usize>] = match mode {
        0 => &[Some(0)],
        1 => &[Some(1)],
        3 | 4 => &[Some(0), Some(1)],
        5 => &[Some(1), None],
        6 => &[Some(0), None],
        _ => &[Some(1), Some(0), None],
    };
    let best = if candidates.len() == 1 {
        weave(candidates[0])
    } else {
        candidates.iter().map(|c| weave(*c)).min_by(|a, b| comb(a).total_cmp(&comb(b))).unwrap_or_else(|| cur.px.clone())
    };
    Disp { w, h, px: best }.store(img);
}

/// Set Range: Limited re-reads the picture as limited range (16…235 expands to full); Auto,
/// Unspecified and Full keep the range the clip was decoded with.
pub(super) fn setrange(img: &mut Image, e: &EffectInstance) {
    if pick(e, "range") != 2 {
        return;
    }
    map_disp(img, |c, _, _| c.map(|v| ((v - 16.0 / 255.0) * 255.0 / 219.0).clamp(0.0, 1.0)));
}
