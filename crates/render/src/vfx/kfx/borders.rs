//! Transform / border effects of the bin: non-linear horizontal stretch (Elastic Scale), border
//! refills (Fill Borders), cinema bars (Letter-boxing), a rolling broken-sync picture (Nosync0r)
//! and the blurred echo behind an area of interest (Pillar Echo).

use redrob_film_geom::Affine;

use super::alpha::pct_rect;
use super::*;

/// Elastic Scale's output → input mapping on 0..1: a linear area of the given width around the
/// centre is scaled by the linear factor, the areas on either side are stretched to the frame's
/// edges, more strongly towards the edges the larger the non-linear factor.
pub(crate) fn elastic_map(x: f32, center: f32, lin_w: f32, lin_k: f32, nl: f32) -> f32 {
    let c = center.clamp(0.0, 1.0);
    let d = x - c;
    let room = if d < 0.0 { c } else { 1.0 - c };
    if room <= 1e-6 {
        return x;
    }
    let inner_o = (lin_w / 2.0 * lin_k).clamp(0.0, room);
    let inner_i = inner_o / lin_k.max(1e-3);
    let ad = d.abs();
    let u = if ad <= inner_o {
        ad / lin_k.max(1e-3)
    } else {
        let t = ((ad - inner_o) / (room - inner_o).max(1e-6)).clamp(0.0, 1.0);
        let f = t + nl * t * (1.0 - t);
        inner_i + (room - inner_i) * f
    };
    c + u * d.signum()
}

/// Elastic Scale: the picture is stretched horizontally, non-linearly (see [`elastic_map`]).
pub fn elastic_scale(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let center = fv(e, "center", cx) / 100.0;
    let lw = fv(e, "linear_width", cx) / 100.0;
    let lk = fv(e, "linear_scale", cx).max(0.1);
    let nl = fv(e, "nonlinear_scale", cx).clamp(0.0, 1.0);
    if lw <= 0.0 && nl <= 0.0 && (center - 0.5).abs() < 1e-6 {
        return;
    }
    let src = img.clone();
    let w = img.w;
    let wf = w as f32;
    let map: Vec<f32> = (0..w).map(|x| elastic_map((x as f32 + 0.5) / wf, center, lw, lk, nl) * wf).collect();
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let p = src.sample_bilinear_clamped(map[x], y as f32 + 0.5);
            row[x * 4..x * 4 + 4].copy_from_slice(&p);
        }
    });
}

/// Fill Borders: the given number of pixels at each edge is replaced (smeared from the edge,
/// mirrored, a colour, reflected, wrapped around, faded to grey, or a weighted average of the rows
/// near the top and bottom borders).
pub fn fill_borders(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (w, h) = (img.w as isize, img.h as isize);
    let k = cx.px_scale;
    let px = |id: &str| (fv(e, id, cx) * k).round().max(0.0) as isize;
    let (l, r, t, b) = (px("left").min(w), px("right").min(w), px("top").min(h), px("bottom").min(h));
    if l + r + t + b == 0 {
        return;
    }
    let mode = pick(e, "mode");
    let fill = crate::graphics::premul_linear(color(e, "color", cx));
    let src = img.clone();
    // inner area every mode samples from
    let (ix0, ix1, iy0, iy1) = (l, (w - r).max(l + 1), t, (h - b).max(t + 1));
    let fold = |v: isize, lo: isize, hi: isize, mirror_half: bool| -> isize {
        // reflect into [lo, hi)
        let n = hi - lo;
        if n <= 1 {
            return lo;
        }
        let period = if mirror_half { 2 * n } else { 2 * (n - 1) };
        let mut q = (v - lo).rem_euclid(period);
        if q >= n {
            q = if mirror_half { period - 1 - q } else { period - q };
        }
        lo + q
    };
    let iw = img.w;
    img.px.par_chunks_mut(iw * 4).enumerate().for_each(|(y, row)| {
        let y = y as isize;
        for x in 0..w {
            let border_x = x < l || x >= w - r;
            let border_y = y < t || y >= h - b;
            if !border_x && !border_y {
                continue;
            }
            let o = match mode {
                1 => src.get_clamped(fold(x, ix0, ix1, true), fold(y, iy0, iy1, true)),
                2 => fill,
                3 => src.get_clamped(fold(x, ix0, ix1, false), fold(y, iy0, iy1, false)),
                4 => src.get_clamped(ix0 + (x - ix0).rem_euclid(ix1 - ix0), iy0 + (y - iy0).rem_euclid(iy1 - iy0)),
                5 => {
                    // fade from the nearest inner pixel to mid grey with the distance into the border
                    let (sx, sy) = (x.clamp(ix0, ix1 - 1), y.clamp(iy0, iy1 - 1));
                    let p = src.get_clamped(sx, sy);
                    let dist = (sx - x).abs().max((sy - y).abs()) as f32;
                    let span = (if border_x { l.max(r) } else { 0 }).max(if border_y { t.max(b) } else { 0 }).max(1) as f32;
                    let g = 0.2140;
                    let kf = (dist / span).clamp(0.0, 1.0);
                    [p[0] + (g - p[0]) * kf, p[1] + (g - p[1]) * kf, p[2] + (g - p[2]) * kf, p[3] + (1.0 - p[3]) * kf]
                }
                6 => {
                    // margins: rows above / below are weighted averages of the inner border rows
                    let sx = x.clamp(ix0, ix1 - 1);
                    if border_y {
                        let (edge, step) = if y < t { (iy0, 1) } else { (iy1 - 1, -1) };
                        let mut acc = [0.0f32; 4];
                        let mut wsum = 0.0;
                        for j in 0..4 {
                            let wgt = 1.0 / (1 + j) as f32;
                            let p = src.get_clamped(sx, edge + step * j);
                            for c in 0..4 {
                                acc[c] += p[c] * wgt;
                            }
                            wsum += wgt;
                        }
                        acc.map(|v| v / wsum)
                    } else {
                        src.get_clamped(sx, y.clamp(iy0, iy1 - 1))
                    }
                }
                _ => src.get_clamped(x.clamp(ix0, ix1 - 1), y.clamp(iy0, iy1 - 1)),
            };
            row[x as usize * 4..x as usize * 4 + 4].copy_from_slice(&o);
        }
    });
}

/// Letter-boxing: black bars (or transparency) at the top and bottom, each Border Width percent of
/// half the frame height.
pub fn letterbox(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let bar = ((fv(e, "border", cx) / 100.0).clamp(0.0, 1.0) * img.h as f32 / 2.0).round() as usize;
    if bar == 0 {
        return;
    }
    let clear = flag(e, "transparent");
    let (w, h) = (img.w, img.h);
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        if y < bar || y >= h.saturating_sub(bar) {
            let v: [f32; 4] = if clear { [0.0; 4] } else { [0.0, 0.0, 0.0, 1.0] };
            for p in row.chunks_mut(4) {
                p.copy_from_slice(&v);
            }
        }
    });
}

/// Nosync0r: the frame is shifted down by HSync thousandths of its height, wrapping around the
/// bottom edge.
pub fn nosync0r(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let h = img.h;
    let s = ((fv(e, "hsync", cx) / 1000.0).clamp(0.0, 1.0) * h as f32).round() as usize % h.max(1);
    if s == 0 {
        return;
    }
    let row = img.w * 4;
    img.px.rotate_right(s * row);
}

/// Pillar Echo: outside the area of interest the frame shows a blurred copy of that area,
/// enlarged to fill it (for vertical video in a horizontal frame); the area itself stays sharp.
pub fn pillar_echo(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let r = pct_rect(e, cx, img.w, img.h);
    let (rw, rh) = ((r[2] - r[0]) as f64, (r[3] - r[1]) as f64);
    if rw < 1.0 || rh < 1.0 {
        return;
    }
    let (w, h) = (img.w as f64, img.h as f64);
    // the area scaled to cover the frame, centred
    let k = (w / rw).max(h / rh);
    let m = Affine::translate(w / 2.0, h / 2.0)
        .then_apply(&Affine::scale(k, k))
        .then_apply(&Affine::translate(-(r[0] as f64 + rw / 2.0), -(r[1] as f64 + rh / 2.0)));
    let mut back = img.transformed(img.w, img.h, &m);
    let sigma = fv(e, "blur", cx) / 100.0 * (img.w.max(img.h) as f32) * 0.05;
    if sigma > 0.3 {
        crate::effects::gaussian(&mut back, sigma, sigma, true);
    }
    let iw = img.w;
    img.px.par_chunks_mut(iw * 4).zip(back.px.par_chunks(iw * 4)).enumerate().for_each(|(y, (row, brow))| {
        let yf = y as f32 + 0.5;
        for x in 0..iw {
            let xf = x as f32 + 0.5;
            if xf >= r[0] && xf < r[2] && yf >= r[1] && yf < r[3] {
                // the sharp area over the echo
                let p = [row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]];
                let q = &brow[x * 4..x * 4 + 4];
                let a = p[3];
                for c in 0..4 {
                    row[x * 4 + c] = p[c] + q[c] * (1.0 - a);
                }
            } else {
                row[x * 4..x * 4 + 4].copy_from_slice(&brow[x * 4..x * 4 + 4]);
            }
        }
    });
}
