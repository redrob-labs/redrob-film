//! Stylize, Generate and Misc effects of the "More Video Effects" bin.

use super::*;

/// Sorted lumas of the visible pixels (sub-sampled), for dynamic thresholds.
fn luma_quantiles(d: &Disp, qs: &[f32]) -> Vec<f32> {
    let step = (d.px.len() / 40_000).max(1);
    let mut v: Vec<f32> = d.px.iter().step_by(step).filter(|p| p[3] > 0.01).map(|p| luma(*p)).collect();
    if v.is_empty() {
        return qs.iter().map(|_| 0.5).collect();
    }
    v.sort_by(|a, b| a.total_cmp(b));
    qs.iter().map(|q| v[((v.len() - 1) as f32 * q).round() as usize]).collect()
}

/// 3-Level Threshold: black / grey / white at the frame's own luma thirds.
pub fn threshold3(img: &mut Image) {
    let mut d = Disp::of(img);
    let t = luma_quantiles(&d, &[1.0 / 3.0, 2.0 / 3.0]);
    d.px.par_iter_mut().for_each(|p| {
        let l = luma(*p);
        let v = if l < t[0] {
            0.0
        } else if l < t[1] {
            0.5
        } else {
            1.0
        };
        *p = [v, v, v, p[3]];
    });
    d.store(img);
}

/// Binarize Dynamically: black and white split at the frame's median luma.
pub fn binarize_dynamic(img: &mut Image) {
    let mut d = Disp::of(img);
    let t = luma_quantiles(&d, &[0.5])[0];
    d.px.par_iter_mut().for_each(|p| {
        let v = if luma(*p) > t { 1.0 } else { 0.0 };
        *p = [v, v, v, p[3]];
    });
    d.store(img);
}

/// Binarize: white where luma (or alpha) reaches the threshold, black elsewhere.
pub fn binarize(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let thr = fv(e, "threshold", cx) / 255.0;
    let use_alpha = flag(e, "use_alpha");
    let mut d = Disp::of(img);
    d.px.par_iter_mut().for_each(|p| {
        let val = if use_alpha { p[3] } else { luma(*p) };
        let v = if val >= thr { 1.0 } else { 0.0 };
        *p = if use_alpha { [v, v, v, 1.0] } else { [v, v, v, p[3]] };
    });
    d.store(img);
}

/// Threshold: 0 turns everything white, 255 everything black.
pub fn threshold(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let thr = fv(e, "threshold", cx) / 255.0;
    map_disp(img, |c, _, _| {
        let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        if l >= thr { [1.0; 3] } else { [0.0; 3] }
    });
}

/// Cartoon: flattened colours with dark outlines. Lower trip levels pick up more edges; the
/// difference space is the outline thickness in pixels.
pub fn cartoon(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let trip = fv(e, "trip", cx).clamp(0.0, 1.0);
    let ds = (fv(e, "diffspace", cx) * cx.px_scale).round().max(1.0) as isize;
    let src = Disp::of(img);
    let thr = 0.02 + trip * 0.3;
    let mut d = Disp { w: src.w, h: src.h, px: src.px.clone() };
    let w = src.w;
    d.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let y = y as isize;
        for (x, p) in row.iter_mut().enumerate() {
            let x = x as isize;
            let l = luma(src.get(x, y));
            let mut m: f32 = 0.0;
            for (dx, dy) in [(ds, 0), (-ds, 0), (0, ds), (0, -ds)] {
                m = m.max((luma(src.get(x + dx, y + dy)) - l).abs());
            }
            if m > thr {
                *p = [0.0, 0.0, 0.0, p[3]];
            } else {
                // flatten: 6 levels per channel
                let q = |v: f32| ((v * 5.0).round() / 5.0).clamp(0.0, 1.0);
                *p = [q(p[0]), q(p[1]), q(p[2]), p[3]];
            }
        }
    });
    d.store(img);
}

/// Charcoal: luma gradients over the scatter distances drawn as dark strokes on white.
pub fn charcoal(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let hs = (fv(e, "h_scatter", cx) * cx.px_scale).round().max(1.0) as isize;
    let vs = (fv(e, "v_scatter", cx) * cx.px_scale).round().max(1.0) as isize;
    let scale = fv(e, "scale", cx).max(0.0);
    let mix = (fv(e, "mix", cx) / 10.0).clamp(0.0, 1.0);
    let inv = flag(e, "invert");
    let src = Disp::of(img);
    let mut d = Disp { w: src.w, h: src.h, px: src.px.clone() };
    let w = src.w;
    d.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let y = y as isize;
        for (x, p) in row.iter_mut().enumerate() {
            let x = x as isize;
            let gx = luma(src.get(x + hs, y)) - luma(src.get(x - hs, y));
            let gy = luma(src.get(x, y + vs)) - luma(src.get(x, y - vs));
            let m = ((gx * gx + gy * gy).sqrt() * scale * 2.0).clamp(0.0, 1.0);
            let v = if inv { m } else { 1.0 - m };
            let o = src.get(x, y);
            let l = luma(o).max(1e-3);
            let tint = [(o[0] / l).min(2.0), (o[1] / l).min(2.0), (o[2] / l).min(2.0)];
            *p = [
                (v * (1.0 - mix + mix * tint[0])).clamp(0.0, 1.0),
                (v * (1.0 - mix + mix * tint[1])).clamp(0.0, 1.0),
                (v * (1.0 - mix + mix * tint[2])).clamp(0.0, 1.0),
                o[3],
            ];
        }
    });
    d.store(img);
}

/// Color Distance: each pixel becomes its distance to the source colour (grey; white = farthest).
pub fn color_distance(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let c = color(e, "color", cx);
    map_disp(img, |p, _, _| {
        let d = (((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)) / 3.0).sqrt();
        [d; 3]
    });
}

fn scurve(v: f32, k: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    // smooth contrast around 0.5 (k = 0 identity, 1 = smoothstep)
    v + (v * v * (3.0 - 2.0 * v) - v) * k
}
fn ramp(stops: &[[f32; 4]], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b[0] {
            let k = if b[0] > a[0] { (t - a[0]) / (b[0] - a[0]) } else { 1.0 };
            return [a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k, a[3] + (b[3] - a[3]) * k];
        }
    }
    let l = stops[stops.len() - 1];
    [l[1], l[2], l[3]]
}

/// Color Effect: one of eight fixed looks.
pub fn color_effect(img: &mut Image, e: &EffectInstance) {
    let ty = pick(e, "type");
    map_disp(img, move |c, _, _| {
        let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let sepia = |l: f32| [(l * 1.07 + 0.04).min(1.0), (l * 0.86 + 0.02).min(1.0), (l * 0.62).min(1.0)];
        match ty {
            // cross-processed: contrasty red/green, flattened lifted blue
            0 => [scurve(c[0], 0.8), scurve(c[1], 0.5), 0.2 + c[2] * 0.6],
            1 => sepia(l),
            2 => ramp(&[[0.0, 0.0, 0.0, 0.0], [0.25, 0.1, 0.0, 0.6], [0.5, 0.9, 0.0, 0.1], [0.75, 1.0, 0.8, 0.0], [1.0, 1.0, 1.0, 1.0]], l),
            3 => [l, 1.0 - l, 0.0],
            4 => {
                let s = sepia(scurve(l, -0.4));
                [0.12 + s[0] * 0.75, 0.1 + s[1] * 0.72, 0.08 + s[2] * 0.7]
            }
            5 => {
                let v = 1.0 - l;
                [v * 0.82, v * 0.92, v]
            }
            6 => [scurve(c[0], 1.0), scurve(c[1], 1.0), scurve(c[2], 1.0)],
            _ => [l, l, 1.0 - l],
        }
    });
}

/// Heatmap0r: luma mapped through black → grey → white colours.
pub fn heatmap(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (b, g, w) = (color(e, "black", cx), color(e, "grey", cx), color(e, "white", cx));
    let stops = [[0.0, b[0], b[1], b[2]], [0.5, g[0], g[1], g[2]], [1.0, w[0], w[1], w[2]]];
    map_disp(img, move |c, _, _| ramp(&stops, 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]));
}

/// Kaleidoscope: the picture folded into rotated (and alternately mirrored) copies of one
/// segment around the origin.
pub fn kaleidoscope(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    use std::f32::consts::TAU;
    let src = Disp::of(img);
    let (w, h) = (src.w as f32, src.h as f32);
    let (ox, oy) = (fv(e, "origin_x", cx) / 100.0 * w, fv(e, "origin_y", cx) / 100.0 * h);
    let n = fv(e, "segmentation", cx).round().clamp(1.0, 128.0);
    let seg = TAU / n;
    let dir = pick(e, "direction");
    // the source segment points at the furthest corner (searching from the preferred one) unless
    // given explicitly
    let a0 = if flag(e, "specify_source") {
        fv(e, "source_segment", cx).to_radians()
    } else {
        let corners = [(w, 0.0), (0.0, 0.0), (0.0, h), (w, h)];
        let start = pick(e, "corner").min(3) as usize;
        let ccw = !flag(e, "corner_search");
        let mut best = corners[start];
        let mut bd = -1.0f32;
        for i in 0..4 {
            let k = if ccw { (start + i) % 4 } else { (start + 4 - i) % 4 };
            let c = corners[k];
            let dd = (c.0 - ox).hypot(c.1 - oy);
            if dd > bd + 0.5 {
                bd = dd;
                best = c;
            }
        }
        (-(best.1 - oy)).atan2(best.0 - ox)
    };
    let reflect = flag(e, "reflect_edges");
    let edge = fv(e, "edge_threshold", cx).max(0.0) * 4.0 * cx.px_scale;
    let bg = if flag(e, "no_alpha_bg") { color(e, "bg_color", cx) } else { [0.0; 4] };
    let mut d = Disp { w: src.w, h: src.h, px: src.px.clone() };
    let wi = src.w;
    d.px.par_chunks_mut(wi).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let (dx, dy) = (x as f32 + 0.5 - ox, -(y as f32 + 0.5 - oy));
            let r = dx.hypot(dy);
            let th = dy.atan2(dx);
            let rel = (th - a0 + seg / 2.0).rem_euclid(TAU);
            let k = if dir == 2 { ((TAU - rel) / seg).floor() } else { (rel / seg).floor() };
            let mut local = rel - (rel / seg).floor() * seg;
            if dir != 0 && (k as i64) % 2 == 1 {
                local = seg - local;
            }
            let a = a0 - seg / 2.0 + local;
            let (mut sx, mut sy) = (ox + r * a.cos(), oy - r * a.sin());
            let outside = sx < 0.0 || sy < 0.0 || sx > w || sy > h;
            if outside {
                let near = sx > -edge && sy > -edge && sx < w + edge && sy < h + edge;
                if near {
                    sx = sx.clamp(0.0, w);
                    sy = sy.clamp(0.0, h);
                } else if reflect {
                    let refl = |v: f32, m: f32| {
                        let t = v.rem_euclid(2.0 * m);
                        if t > m { 2.0 * m - t } else { t }
                    };
                    sx = refl(sx, w);
                    sy = refl(sy, h);
                } else {
                    *p = bg;
                    continue;
                }
            }
            *p = src.sample(sx, sy);
        }
    });
    d.store(img);
}

/// Edge operators of the plane-based edge effects.
#[derive(Clone, Copy, PartialEq)]
pub enum Op {
    Kirsch,
    Prewitt,
    Roberts,
    Scharr,
    Sobel,
}

fn edge_plane(p: &[f32], w: usize, h: usize, op: Op, scale: f32, delta: f32) -> Vec<f32> {
    let at = |x: isize, y: isize| p[y.clamp(0, h as isize - 1) as usize * w + x.clamp(0, w as isize - 1) as usize];
    let mut out = vec![0.0f32; p.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let y = y as isize;
        for (x, o) in row.iter_mut().enumerate() {
            let x = x as isize;
            let n = |dx: isize, dy: isize| at(x + dx, y + dy);
            let g = match op {
                Op::Roberts => {
                    let a = n(0, 0) - n(1, 1);
                    let b = n(1, 0) - n(0, 1);
                    (a * a + b * b).sqrt()
                }
                Op::Kirsch => {
                    // 8 compass directions: 5 on three neighbours, -3 on the other five
                    let ring = [n(-1, -1), n(0, -1), n(1, -1), n(1, 0), n(1, 1), n(0, 1), n(-1, 1), n(-1, 0)];
                    let total: f32 = ring.iter().sum();
                    (0..8)
                        .map(|k| {
                            let three = ring[k] + ring[(k + 1) % 8] + ring[(k + 2) % 8];
                            5.0 * three - 3.0 * (total - three)
                        })
                        .fold(0.0f32, |m, v| m.max(v.abs()))
                }
                _ => {
                    let (c, m) = match op {
                        Op::Prewitt => (1.0, 1.0),
                        Op::Scharr => (3.0 / 16.0, 10.0 / 16.0),
                        _ => (1.0, 2.0),
                    };
                    let gx = c * (n(1, -1) - n(-1, -1)) + m * (n(1, 0) - n(-1, 0)) + c * (n(1, 1) - n(-1, 1));
                    let gy = c * (n(-1, 1) - n(-1, -1)) + m * (n(0, 1) - n(0, -1)) + c * (n(1, 1) - n(1, -1));
                    (gx * gx + gy * gy).sqrt()
                }
            };
            *o = (g * scale + delta).clamp(0.0, 1.0);
        }
    });
    out
}

/// Kirsch / Prewitt / Roberts / Scharr / Sobel with Planes: the operator on the chosen planes,
/// `result × Scale + Delta`, other planes copied.
pub fn edges(img: &mut Image, e: &EffectInstance, cx: &FxCtx, op: Op) {
    let c = pick(e, "planes");
    let pl = if matches!(op, Op::Kirsch | Op::Scharr) { Planes::from_any_choice(c) } else { Planes::from_yuv_choice(c) };
    if pl.none() {
        return;
    }
    let scale = fv(e, "scale", cx);
    let delta = fv(e, "delta", cx) / 255.0;
    let mut d = Disp::of(img);
    let mut planes = split_planes(&d, pl.yuv);
    for k in 0..4 {
        if pl.on[k] {
            planes[k] = edge_plane(&planes[k], d.w, d.h, op, scale, delta);
        }
    }
    merge_planes(&mut d, &planes, pl.yuv);
    d.store(img);
}

/// Sobel: the Sobel gradient magnitude of each colour channel.
pub fn sobel_rgb(img: &mut Image) {
    let mut d = Disp::of(img);
    let mut planes = split_planes(&d, false);
    for k in 0..3 {
        planes[k] = edge_plane(&planes[k], d.w, d.h, Op::Sobel, 0.25, 0.0);
    }
    merge_planes(&mut d, &planes, false);
    d.store(img);
}

/// NDVI Filter: (NIR − VIS) / (NIR + VIS) from the red (near-infrared) and blue (visible)
/// channels of a converted camera, as a quantised false-colour image.
pub fn ndvi(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let levels = fv(e, "levels", cx).round().clamp(2.0, 1000.0);
    let (vs, vo) = (fv(e, "vis_scale", cx), fv(e, "vis_offset", cx) / 100.0);
    let (ns, no) = (fv(e, "nir_scale", cx), fv(e, "nir_offset", cx) / 100.0);
    const PAL: [[f32; 4]; 6] =
        [[0.0, 0.0, 0.0, 0.0], [0.4, 0.5, 0.5, 0.5], [0.5, 0.6, 0.4, 0.2], [0.6, 1.0, 1.0, 0.0], [0.8, 0.0, 0.8, 0.0], [1.0, 0.0, 0.35, 0.0]];
    map_disp(img, move |c, _, _| {
        let vis = (c[2] * vs + vo).max(0.0);
        let nir = (c[0] * ns + no).max(0.0);
        let s = nir + vis;
        let v = if s <= 1e-6 { 0.0 } else { (nir - vis) / s };
        let t = ((v + 1.0) / 2.0 * (levels - 1.0)).round() / (levels - 1.0);
        ramp(&PAL, t)
    });
}

/// NTSC: band-limited, slightly delayed chroma, soft luma, signal noise, optional interlacing
/// (alternate lines from the previous frame) and scanline borders.
pub fn ntsc(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let amt = fv(e, "noise", cx) / 100.0;
    let prev = if flag(e, "progressive") { None } else { neighbour(img, e, cx, -1.0).map(|p| Disp::of(&p)) };
    let src = Disp::of(img);
    let (w, h) = (src.w, src.h);
    let n = frame_no(cx);
    // YIQ planes
    let yiq: Vec<[f32; 4]> = src
        .px
        .par_iter()
        .map(|p| {
            let (r, g, b) = (p[0], p[1], p[2]);
            [0.299 * r + 0.587 * g + 0.114 * b, 0.596 * r - 0.274 * g - 0.322 * b, 0.211 * r - 0.523 * g + 0.312 * b, p[3]]
        })
        .collect();
    let plane = |k: usize| -> Vec<f32> { yiq.iter().map(|p| p[k]).collect() };
    let horiz = |p: &[f32], r: usize, shift: isize| -> Vec<f32> {
        let mut o = vec![0.0; p.len()];
        o.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                let mut acc = 0.0;
                for k in -(r as isize)..=(r as isize) {
                    acc += p[y * w + (x as isize + k - shift).clamp(0, w as isize - 1) as usize];
                }
                *v = acc / (2 * r + 1) as f32;
            }
        });
        o
    };
    let s = cx.px_scale.max(0.1);
    let yr = (1.0 * s).round() as usize;
    let cr = (4.0 * s).round().max(1.0) as usize;
    let shift = (2.0 * s).round() as isize;
    let yp = horiz(&plane(0), yr, 0);
    let ip = horiz(&plane(1), cr, shift);
    let qp = horiz(&plane(2), cr, shift);
    let scan = flag(e, "scanlines");
    let mut d = Disp { w, h, px: vec![[0.0; 4]; w * h] };
    d.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        // per-line horizontal jitter grows with the noise
        let jit = ((rnd(n, y as i64, 1, 11) - 0.5) * amt * 4.0 * s).round() as isize;
        let field = (y as i64 + n) % 2 == 1;
        for (x, o) in row.iter_mut().enumerate() {
            if let (true, Some(pv)) = (field, prev.as_ref()) {
                *o = pv.get(x as isize, y as isize);
                continue;
            }
            let xi = (x as isize + jit).clamp(0, w as isize - 1) as usize;
            let i = y * w + xi;
            let noise = (rnd(x as i64, y as i64, n, 7) - 0.5) * amt * 0.5;
            let (yy, ii, qq) = (yp[i] + noise, ip[i], qp[i]);
            let mut c = [yy + 0.956 * ii + 0.621 * qq, yy - 0.272 * ii - 0.647 * qq, yy - 1.106 * ii + 1.703 * qq];
            if scan && ((y as f32 / s) as i64) % 2 == 1 {
                c = c.map(|v| v * 0.55);
            }
            *o = [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0), yiq[y * w + x][3]];
        }
    });
    d.store(img);
}

/// Old Film Simulator: random vertical jumps, random brightness changes, and a slow uneven
/// development that drifts the brightness up and down.
pub fn old_film(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let n = frame_no(cx);
    let s = cx.px_scale;
    let mv = fv(e, "move", cx) * s;
    let dy = if rnd(n, 0, 1, 31) * 100.0 < fv(e, "move_every", cx) { ((rnd(n, 0, 2, 31) * 2.0 - 1.0) * mv).round() } else { 0.0 };
    let mut delta = 0.0;
    if rnd(n, 0, 3, 31) * 100.0 < fv(e, "bright_every", cx) {
        let (up, down) = (fv(e, "bright_up", cx), fv(e, "bright_down", cx));
        delta = (rnd(n, 0, 4, 31) * (up + down) - down) / 255.0;
    }
    let dur = fv(e, "develop_duration", cx).max(1.0);
    let ph = (n as f32 / dur) * std::f32::consts::TAU;
    let (du, dd) = (fv(e, "develop_up", cx) / 255.0, fv(e, "develop_down", cx) / 255.0);
    let src = Disp::of(img);
    let h = src.h as f32;
    let mut d = Disp { w: src.w, h: src.h, px: src.px.clone() };
    let w = src.w;
    d.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let sy = y as isize - dy as isize;
        let dev = (ph + y as f32 / h * std::f32::consts::PI).sin();
        let dv = if dev >= 0.0 { dev * du } else { dev * dd };
        for (x, p) in row.iter_mut().enumerate() {
            let o = if sy < 0 || sy >= src.h as isize { [0.0, 0.0, 0.0, 1.0] } else { src.get(x as isize, sy) };
            *p = [(o[0] + delta + dv).clamp(0.0, 1.0), (o[1] + delta + dv).clamp(0.0, 1.0), (o[2] + delta + dv).clamp(0.0, 1.0), o[3]];
        }
    });
    d.store(img);
}

/// Pixelize (advanced): blocks of the chosen size take the average, minimum or maximum of their
/// pixels, on the chosen planes.
pub fn pixelize(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let pl = Planes::from_yuv_choice(pick(e, "planes"));
    if pl.none() {
        return;
    }
    let bw = (fv(e, "block_w", cx) * cx.px_scale).round().max(1.0) as usize;
    let bh = (fv(e, "block_h", cx) * cx.px_scale).round().max(1.0) as usize;
    let mode = pick(e, "mode");
    let mut d = Disp::of(img);
    let (w, h) = (d.w, d.h);
    let mut planes = split_planes(&d, true);
    for k in 0..4 {
        if !pl.on[k] {
            continue;
        }
        let ro: &[f32] = &planes[k];
        let blocks_y: Vec<usize> = (0..h).step_by(bh).collect();
        let results: Vec<(usize, Vec<f32>)> = blocks_y
            .par_iter()
            .map(|&by| {
                let mut vals = Vec::new();
                for bx in (0..w).step_by(bw) {
                    let (mut acc, mut mn, mut mx, mut cnt) = (0.0f32, f32::MAX, f32::MIN, 0.0f32);
                    for y in by..(by + bh).min(h) {
                        for x in bx..(bx + bw).min(w) {
                            let v = ro[y * w + x];
                            acc += v;
                            mn = mn.min(v);
                            mx = mx.max(v);
                            cnt += 1.0;
                        }
                    }
                    vals.push(match mode {
                        1 => mn,
                        2 => mx,
                        _ => acc / cnt.max(1.0),
                    });
                }
                (by, vals)
            })
            .collect();
        let p = &mut planes[k];
        for (by, vals) in results {
            for y in by..(by + bh).min(h) {
                for (bi, bx) in (0..w).step_by(bw).enumerate() {
                    for x in bx..(bx + bw).min(w) {
                        p[y * w + x] = vals[bi];
                    }
                }
            }
        }
    }
    merge_planes(&mut d, &planes, true);
    d.store(img);
}

/// Pixels0rt: pixels sorted by brightness within runs of each line (the threshold is the run
/// length as a fraction of the line); transparent pixels below the transparency threshold stay.
pub fn pixels0rt(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let thr = fv(e, "threshold", cx).clamp(0.0, 1.0);
    let dir = pick(e, "direction");
    let rev = flag(e, "reversed");
    let athr = fv(e, "alpha_threshold", cx).clamp(0.0, 1.0);
    let mut d = Disp::of(img);
    let (w, h) = (d.w, d.h);
    let vertical = dir < 2;
    let backwards = dir == 1 || dir == 3;
    let (lines, len) = if vertical { (w, h) } else { (h, w) };
    let run = ((len as f32 * thr).round() as usize).max(1);
    let idx = |l: usize, i: usize| if vertical { i * w + l } else { l * w + i };
    let sorted: Vec<Vec<[f32; 4]>> = (0..lines)
        .into_par_iter()
        .map(|l| {
            let mut line: Vec<[f32; 4]> = (0..len).map(|i| d.px[idx(l, i)]).collect();
            if backwards {
                line.reverse();
            }
            let mut start = 0;
            while start < len {
                // a run ends at its length or at a pixel that is left alone
                let mut end = start;
                while end < len && end - start < run && !(athr > 0.0 && line[end][3] < athr) {
                    end += 1;
                }
                if end > start {
                    line[start..end].sort_by(|a, b| {
                        let o = luma(*a).total_cmp(&luma(*b));
                        if rev { o.reverse() } else { o }
                    });
                    start = end;
                } else {
                    start += 1;
                }
            }
            if backwards {
                line.reverse();
            }
            line
        })
        .collect();
    for (l, line) in sorted.into_iter().enumerate() {
        for (i, p) in line.into_iter().enumerate() {
            d.px[idx(l, i)] = p;
        }
    }
    d.store(img);
}

/// Colour quantisation by k-means: `n` colours refined over `steps` iterations. With a distance
/// weight, pixels also cluster by position (K-Means Clustering).
pub fn quantize(img: &mut Image, n: usize, steps: usize, spatial: f32) {
    let mut d = Disp::of(img);
    let (w, h) = (d.w as f32, d.h as f32);
    let feat = |i: usize, p: [f32; 4]| -> [f32; 5] {
        let (x, y) = ((i % d.w) as f32 / w, (i / d.w) as f32 / h);
        [p[0], p[1], p[2], x * spatial, y * spatial]
    };
    let step = (d.px.len() / 20_000).max(1);
    let samples: Vec<[f32; 5]> = d.px.iter().enumerate().step_by(step).filter(|(_, p)| p[3] > 0.01).map(|(i, p)| feat(i, *p)).collect();
    if samples.is_empty() {
        return;
    }
    let dist = |a: &[f32; 5], b: &[f32; 5]| (0..5).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>();
    // farthest-point initialisation from the mean
    let mut mean = [0.0f32; 5];
    for s in &samples {
        for k in 0..5 {
            mean[k] += s[k] / samples.len() as f32;
        }
    }
    let mut centers = vec![mean];
    let mut near: Vec<f32> = samples.iter().map(|s| dist(s, &mean)).collect();
    while centers.len() < n {
        let (bi, bd) = near.iter().enumerate().fold((0, -1.0f32), |b, (i, v)| if *v > b.1 { (i, *v) } else { b });
        if bd <= 1e-9 {
            break;
        }
        let c = samples[bi];
        for (s, m) in samples.iter().zip(near.iter_mut()) {
            *m = m.min(dist(s, &c));
        }
        centers.push(c);
    }
    let nearest = |c: &[[f32; 5]], f: &[f32; 5]| -> usize {
        let mut best = (0, f32::MAX);
        for (i, cc) in c.iter().enumerate() {
            let v = dist(f, cc);
            if v < best.1 {
                best = (i, v);
            }
        }
        best.0
    };
    for _ in 0..steps {
        let mut acc = vec![[0.0f32; 6]; centers.len()];
        for s in &samples {
            let i = nearest(&centers, s);
            for k in 0..5 {
                acc[i][k] += s[k];
            }
            acc[i][5] += 1.0;
        }
        for (c, a) in centers.iter_mut().zip(&acc) {
            if a[5] > 0.0 {
                for k in 0..5 {
                    c[k] = a[k] / a[5];
                }
            }
        }
    }
    let wi = d.w;
    d.px.par_iter_mut().enumerate().for_each(|(i, p)| {
        let f = [p[0], p[1], p[2], (i % wi) as f32 / w * spatial, (i / wi) as f32 / h * spatial];
        let c = centers[nearest(&centers, &f)];
        *p = [c[0], c[1], c[2], p[3]];
    });
    d.store(img);
}

/// Primaries: each channel switched fully on or off against the pixel's own mean (Factor weighs
/// the mean against mid grey).
pub fn primaries(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let k = fv(e, "factor", cx).clamp(0.0, 32.0);
    map_disp(img, move |c, _, _| {
        let m = (c[0] + c[1] + c[2]) / 3.0;
        let t = (m * k + 0.5) / (k + 1.0);
        c.map(|v| if v >= t { 1.0 } else { 0.0 })
    });
}

const PALETTES: [&[[f32; 4]]; 21] = [
    &[[0.0, 0.0, 0.0, 0.02], [0.25, 0.32, 0.07, 0.5], [0.5, 0.72, 0.21, 0.47], [0.75, 0.98, 0.53, 0.38], [1.0, 0.99, 0.99, 0.75]],
    &[[0.0, 0.0, 0.0, 0.02], [0.25, 0.34, 0.06, 0.43], [0.5, 0.73, 0.21, 0.33], [0.75, 0.98, 0.55, 0.04], [1.0, 0.99, 1.0, 0.64]],
    &[[0.0, 0.05, 0.03, 0.53], [0.25, 0.49, 0.01, 0.66], [0.5, 0.8, 0.28, 0.47], [0.75, 0.97, 0.58, 0.25], [1.0, 0.94, 0.98, 0.13]],
    &[[0.0, 0.27, 0.0, 0.33], [0.25, 0.23, 0.32, 0.55], [0.5, 0.13, 0.57, 0.55], [0.75, 0.37, 0.79, 0.38], [1.0, 0.99, 0.91, 0.14]],
    &[[0.0, 0.19, 0.07, 0.23], [0.15, 0.27, 0.45, 0.95], [0.35, 0.1, 0.85, 0.7], [0.5, 0.64, 0.99, 0.24], [0.65, 0.95, 0.8, 0.2], [0.8, 0.98, 0.45, 0.08], [1.0, 0.48, 0.02, 0.01]],
    &[[0.0, 0.0, 0.13, 0.3], [0.5, 0.49, 0.49, 0.47], [1.0, 1.0, 0.91, 0.27]],
    &[[0.0, 0.0, 0.0, 1.0], [0.33, 0.0, 1.0, 0.0], [0.66, 1.0, 1.0, 0.0], [1.0, 1.0, 0.0, 0.0]],
    &[[0.0, 0.5, 0.0, 0.5], [0.5, 0.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0]],
    &[[0.0, 0.8, 0.0, 0.0], [0.2, 1.0, 0.4, 0.0], [0.2001, 0.2, 0.2, 0.2], [1.0, 1.0, 1.0, 1.0]],
    &[[0.0, 0.0, 0.0, 0.0], [0.8, 0.8, 0.8, 0.8], [0.8001, 1.0, 0.6, 0.0], [1.0, 1.0, 0.0, 0.0]],
    &[[0.0, 0.2, 0.0, 0.0], [0.5, 1.0, 0.4, 0.0], [1.0, 1.0, 1.0, 0.6]],
    &[[0.0, 0.0, 0.0, 0.6], [0.06, 0.0, 0.0, 1.0], [0.0601, 0.06, 0.06, 0.06], [0.92, 0.92, 0.92, 0.92], [0.9201, 1.0, 0.0, 0.0], [1.0, 1.0, 0.0, 0.0]],
    &[[0.0, 0.0, 0.0, 1.0], [0.1, 0.0, 0.0, 1.0], [0.1001, 0.0, 0.5, 0.0], [0.9, 0.0, 0.95, 0.0], [0.9001, 1.0, 0.0, 0.0], [1.0, 1.0, 0.0, 0.0]],
    &[[0.0, 0.0, 0.0, 1.0], [0.02, 0.0, 0.0, 1.0], [0.0201, 0.02, 0.02, 0.02], [0.98, 0.98, 0.98, 0.98], [0.9801, 1.0, 0.0, 0.0], [1.0, 1.0, 0.0, 0.0]],
    &[[0.0, 0.62, 0.0, 0.26], [0.25, 0.96, 0.43, 0.26], [0.5, 1.0, 1.0, 0.75], [0.75, 0.4, 0.76, 0.65], [1.0, 0.37, 0.31, 0.64]],
    &[[0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 0.0, 1.0]],
    &[[0.0, 0.0, 0.0, 0.0], [0.33, 0.8, 0.0, 0.0], [0.66, 1.0, 0.8, 0.0], [1.0, 1.0, 1.0, 1.0]],
    &[[0.0, 0.0, 0.0, 0.0], [0.25, 0.5, 0.0, 0.0], [0.5, 1.0, 0.3, 0.0], [0.75, 1.0, 0.75, 0.0], [1.0, 1.0, 1.0, 0.8]],
    &[[0.0, 0.97, 0.98, 1.0], [0.5, 0.42, 0.68, 0.84], [1.0, 0.03, 0.19, 0.42]],
    &[[0.0, 0.97, 0.99, 0.96], [0.5, 0.45, 0.77, 0.46], [1.0, 0.0, 0.27, 0.11]],
    &[[0.0, 0.0, 0.0, 0.0], [0.2, 0.1, 0.18, 0.3], [0.4, 0.1, 0.47, 0.3], [0.6, 0.63, 0.47, 0.29], [0.8, 0.77, 0.65, 0.83], [1.0, 1.0, 1.0, 1.0]],
];

/// Pseudo Color: one channel mapped through a palette, mixed by opacity.
pub fn pseudocolor(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let comp = pick(e, "component").min(3) as usize;
    let pal = PALETTES[pick(e, "preset").min(20) as usize];
    let op = (fv(e, "opacity", cx) / 100.0).clamp(0.0, 1.0);
    let mut d = Disp::of(img);
    d.px.par_iter_mut().for_each(|p| {
        let c = ramp(pal, p[comp]);
        for k in 0..3 {
            p[k] += (c[k] - p[k]) * op;
        }
        if comp == 3 {
            p[3] = 1.0;
        }
    });
    d.store(img);
}

/// RGB Splitter: red and blue moved apart by the split distances (green stays).
pub fn rgb_splitter(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (vx, vy) = (fv(e, "h_split", cx) * cx.px_scale, fv(e, "v_split", cx) * cx.px_scale);
    let src = img.clone();
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let r = src.sample_bilinear(fx - vx, fy - vy);
            let g = src.get(x, y);
            let b = src.sample_bilinear(fx + vx, fy + vy);
            let o = &mut row[x * 4..x * 4 + 4];
            o[0] = r[0];
            o[1] = g[1];
            o[2] = b[2];
            o[3] = r[3].max(g[3]).max(b[3]);
        }
    });
}

/// Squigglevision: a hand-drawn wobble, a noise displacement re-drawn `fps` times a second.
pub fn squigglevision(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let rate = fv(e, "fps", cx).clamp(1.0, 30.0) as f64;
    let step = (cx.seconds * rate).floor() as f32;
    let cell = (fv(e, "scale", cx).clamp(4.0, 128.0) * cx.px_scale).max(1.0);
    let amp = fv(e, "strength", cx) / 100.0 * cell * 0.5;
    if amp <= 0.0 {
        return;
    }
    let src = img.clone();
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (u, v) = (x as f32 / cell, y as f32 / cell);
            let dx = crate::vfx::noise3(u, v, step * 3.17, 101) * amp;
            let dy = crate::vfx::noise3(u, v, step * 3.17 + 50.0, 202) * amp;
            let s = src.sample_bilinear_clamped(x as f32 + 0.5 + dx, y as f32 + 0.5 + dy);
            row[x * 4..x * 4 + 4].copy_from_slice(&s);
        }
    });
}

/// Draw Box: an outlined (or, with enough thickness, filled) rectangle, composited over the
/// picture or replacing its colour and alpha.
pub fn draw_box(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let s = cx.px_scale;
    let (x0, y0) = (fv(e, "x", cx) * s, fv(e, "y", cx) * s);
    let (bw, bh) = (fv(e, "w", cx).max(0.0) * s, fv(e, "h", cx).max(0.0) * s);
    let t = fv(e, "thickness", cx).max(0.0) * s;
    if bw <= 0.0 || bh <= 0.0 || t <= 0.0 {
        return;
    }
    let c = color(e, "color", cx);
    let lin = crate::effects::dec([c[0], c[1], c[2]]);
    let ca = c[3].clamp(0.0, 1.0);
    let replace = flag(e, "replace");
    let (x1, y1) = (x0 + bw, y0 + bh);
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let fy = y as f32 + 0.5;
        if fy < y0 || fy >= y1 {
            return;
        }
        for x in 0..w {
            let fx = x as f32 + 0.5;
            if fx < x0 || fx >= x1 {
                continue;
            }
            let inner = fx >= x0 + t && fx < x1 - t && fy >= y0 + t && fy < y1 - t;
            if inner {
                continue;
            }
            let p = &mut row[x * 4..x * 4 + 4];
            if replace {
                p[0] = lin[0] * ca;
                p[1] = lin[1] * ca;
                p[2] = lin[2] * ca;
                p[3] = ca;
            } else {
                let a = p[3];
                for k in 0..3 {
                    p[k] = p[k] * (1.0 - ca) + lin[k] * ca * a;
                }
            }
        }
    });
}

/// Scanline0r: every other line black, like an old interlaced monitor.
pub fn scanline(img: &mut Image) {
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        if y % 2 == 1 {
            for p in row.chunks_mut(4) {
                p[0] = 0.0;
                p[1] = 0.0;
                p[2] = 0.0;
            }
        }
    });
}

/// Alphaextract: the alpha channel as an opaque grey picture.
pub fn alphaextract(img: &mut Image) {
    img.px.par_chunks_mut(4).for_each(|p| {
        let v = crate::effects::dec([p[3].clamp(0.0, 1.0); 3])[0];
        p[0] = v;
        p[1] = v;
        p[2] = v;
        p[3] = 1.0;
    });
}

/// Xine Deinterlacer: one field kept, the other rebuilt (blend, line doubling or interpolation).
pub fn deinterlace(img: &mut Image, e: &EffectInstance) {
    let (w, h) = (img.w, img.h);
    if h < 3 {
        return;
    }
    let keep = if flag(e, "bottom_first") { 1 } else { 0 };
    let method = pick(e, "method");
    let src = img.px.clone();
    let line = |y: isize| {
        let y = y.clamp(0, h as isize - 1) as usize;
        &src[y * w * 4..(y + 1) * w * 4]
    };
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let yi = y as isize;
        match method {
            // linear blend: both fields mixed (1-2-1 vertical filter)
            0 => {
                let (a, b, c) = (line(yi - 1), line(yi), line(yi + 1));
                for i in 0..w * 4 {
                    row[i] = 0.25 * a[i] + 0.5 * b[i] + 0.25 * c[i];
                }
            }
            _ if y % 2 == keep => {}
            // one field: the line above (or below at the top) repeated
            1 => {
                let s = if yi - 1 >= 0 && (yi - 1) as usize % 2 == keep { line(yi - 1) } else { line(yi + 1) };
                row.copy_from_slice(s);
            }
            // interpolation between the kept lines around it
            _ => {
                let (a, c) = (line(yi - 1), line(yi + 1));
                for i in 0..w * 4 {
                    row[i] = 0.5 * (a[i] + c[i]);
                }
            }
        }
    });
}
