//! Grain and Noise effects of the "More Video Effects" bin: denoisers (block DCT, wavelet,
//! edge-preserving spatial/temporal, chroma averaging), dithering, debanding and the old-film
//! overlays (grain, dust, scratches).

use super::*;

// ------------------------------------------------------------------ 3D FFT Denoiser

/// Orthonormal DCT-II matrix of size n (row k = basis k).
fn dct_matrix(n: usize) -> Vec<f32> {
    let mut m = vec![0.0f32; n * n];
    for k in 0..n {
        let s = if k == 0 { (1.0 / n as f64).sqrt() } else { (2.0 / n as f64).sqrt() };
        for i in 0..n {
            m[k * n + i] = (s * (std::f64::consts::PI * (i as f64 + 0.5) * k as f64 / n as f64).cos()) as f32;
        }
    }
    m
}

/// `C · X · Cᵀ` (forward) or `Cᵀ · X · C` (inverse) of an n×n block.
fn dct2(c: &[f32], x: &[f32], n: usize, inverse: bool) -> Vec<f32> {
    let mut t = vec![0.0f32; n * n];
    // rows
    for r in 0..n {
        for k in 0..n {
            let mut acc = 0.0;
            for i in 0..n {
                let m = if inverse { c[i * n + k] } else { c[k * n + i] };
                acc += m * x[r * n + i];
            }
            t[r * n + k] = acc;
        }
    }
    let mut o = vec![0.0f32; n * n];
    for col in 0..n {
        for k in 0..n {
            let mut acc = 0.0;
            for i in 0..n {
                let m = if inverse { c[i * n + k] } else { c[k * n + i] };
                acc += m * t[i * n + col];
            }
            o[k * n + col] = acc;
        }
    }
    o
}

fn shrink(c: f32, sigma: f32, hard: bool) -> f32 {
    if hard {
        if c.abs() < 3.0 * sigma { 0.0 } else { c }
    } else {
        let p = c * c;
        if p <= 1e-12 { 0.0 } else { c * ((p - sigma * sigma) / p).max(0.0) }
    }
}

/// Denoise planes jointly in overlapping blocks: 2-D DCT per frame, a DCT across the frames
/// (when neighbours are given), coefficient shrinkage, then the inverse; the current frame is
/// `frames[cur]`.
#[allow(clippy::too_many_arguments)]
fn block_denoise(frames: &[&[f32]], cur: usize, w: usize, h: usize, bs: usize, step: usize, sigma: f32, amount: f32, hard: bool) -> Vec<f32> {
    let c = dct_matrix(bs);
    let nt = frames.len();
    let ct = dct_matrix(nt);
    let mut xs: Vec<usize> = (0..w.saturating_sub(bs) + 1).step_by(step).collect();
    if xs.last() != Some(&(w - bs)) {
        xs.push(w - bs);
    }
    let mut ys: Vec<usize> = (0..h.saturating_sub(bs) + 1).step_by(step).collect();
    if ys.last() != Some(&(h - bs)) {
        ys.push(h - bs);
    }
    let rows: Vec<Vec<(usize, Vec<f32>)>> = ys
        .par_iter()
        .map(|&by| {
            xs.iter()
                .map(|&bx| {
                    let coef: Vec<Vec<f32>> = frames
                        .iter()
                        .map(|f| {
                            let mut blk = vec![0.0f32; bs * bs];
                            for y in 0..bs {
                                blk[y * bs..(y + 1) * bs].copy_from_slice(&f[(by + y) * w + bx..(by + y) * w + bx + bs]);
                            }
                            dct2(&c, &blk, bs, false)
                        })
                        .collect();
                    let mut out = vec![0.0f32; bs * bs];
                    for i in 0..bs * bs {
                        // DCT across time, shrink (keep each frame's DC), back
                        let tv: Vec<f32> = (0..nt).map(|k| (0..nt).map(|j| ct[k * nt + j] * coef[j][i]).sum()).collect();
                        let sv: Vec<f32> = tv.iter().map(|v| if i == 0 { *v } else { shrink(*v, sigma, hard) }).collect();
                        let back: f32 = (0..nt).map(|k| ct[k * nt + cur] * sv[k]).sum();
                        let orig = coef[cur][i];
                        out[i] = orig + (back - orig) * amount;
                    }
                    (bx, dct2(&c, &out, bs, true))
                })
                .collect()
        })
        .collect();
    let mut acc = vec![0.0f32; w * h];
    let mut wt = vec![0.0f32; w * h];
    for (row, &by) in rows.iter().zip(ys.iter()) {
        for (bx, blk) in row {
            for y in 0..bs {
                for x in 0..bs {
                    let i = (by + y) * w + bx + x;
                    acc[i] += blk[y * bs + x];
                    wt[i] += 1.0;
                }
            }
        }
    }
    acc.iter().zip(wt.iter()).zip(frames[cur].iter()).map(|((a, n), o)| if *n > 0.0 { a / n } else { *o }).collect()
}

/// 3D FFT Denoiser: frequency-domain shrinkage of overlapping blocks (block size 2^Block
/// pixels), optionally across the previous and next frames.
pub fn fft_denoise(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let pl = Planes::from_yuv_choice(pick(e, "planes"));
    let sigma = fv(e, "sigma", cx) / 255.0;
    let amount = fv(e, "amount", cx).clamp(0.0, 1.0);
    if pl.none() || sigma <= 0.0 || amount <= 0.0 {
        return;
    }
    let (w, h) = (img.w, img.h);
    let mut bs = 1usize << (fv(e, "block", cx).round().clamp(2.0, 6.0) as u32);
    while bs > w.min(h) && bs > 2 {
        bs /= 2;
    }
    if bs < 2 || bs > w.min(h) {
        return;
    }
    let ov = fv(e, "overlap", cx).clamp(0.2, 0.8);
    let step = ((bs as f32 * (1.0 - ov)).round() as usize).max(1);
    let hard = pick(e, "method") == 1;
    let mut neighbours = Vec::new();
    if flag(e, "prev")
        && let Some(p) = neighbour(img, e, cx, -1.0)
    {
        neighbours.push((0usize, Disp::of(&p)));
    }
    let has_prev = !neighbours.is_empty();
    if flag(e, "next")
        && let Some(p) = neighbour(img, e, cx, 1.0)
    {
        neighbours.push((2usize, Disp::of(&p)));
    }
    let mut d = Disp::of(img);
    let mut planes = split_planes(&d, true);
    let nplanes: Vec<[Vec<f32>; 4]> = neighbours.iter().map(|(_, n)| split_planes(n, true)).collect();
    for k in 0..4 {
        if !pl.on[k] {
            continue;
        }
        let mut frames: Vec<&[f32]> = Vec::new();
        let mut cur = 0;
        if has_prev {
            frames.push(&nplanes[0][k]);
            cur = 1;
        }
        frames.push(&planes[k]);
        if neighbours.len() > usize::from(has_prev) {
            frames.push(&nplanes[nplanes.len() - 1][k]);
        }
        let out = block_denoise(&frames, cur, w, h, bs, step, sigma, amount, hard);
        planes[k] = out;
    }
    merge_planes(&mut d, &planes, true);
    d.store(img);
}

// ------------------------------------------------------------------ Chroma Noise Reduction

/// Chroma Noise Reduction: U and V averaged over the neighbours whose YUV difference is under
/// the thresholds (8-bit units); luma is copied.
pub fn chroma_nr(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let thr = fv(e, "threshold", cx);
    let (ty, tu, tvv) = (fv(e, "thr_y", cx), fv(e, "thr_u", cx), fv(e, "thr_v", cx));
    let s = cx.px_scale.max(0.05);
    let rx = (fv(e, "size_w", cx) * s).round().max(1.0) as isize;
    let ry = (fv(e, "size_h", cx) * s).round().max(1.0) as isize;
    let sx = fv(e, "step_w", cx).round().max(1.0) as usize;
    let sy = fv(e, "step_h", cx).round().max(1.0) as usize;
    let euclid = pick(e, "distance") == 1;
    let mut d = Disp::of(img);
    let (w, h) = (d.w, d.h);
    let yuv: Vec<[f32; 4]> = d.px.par_iter().map(|p| { let q = to_yuv(*p); [q[0] * 255.0, q[1] * 255.0, q[2] * 255.0, q[3]] }).collect();
    let out: Vec<[f32; 4]> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            let c = yuv[i];
            let (mut su, mut sv, mut n) = (0.0f32, 0.0f32, 0.0f32);
            for dy in (-ry..=ry).step_by(sy) {
                let yy = y + dy;
                if yy < 0 || yy >= h as isize {
                    continue;
                }
                for dx in (-rx..=rx).step_by(sx) {
                    let xx = x + dx;
                    if xx < 0 || xx >= w as isize {
                        continue;
                    }
                    let q = yuv[yy as usize * w + xx as usize];
                    let (a, b, cc) = ((q[0] - c[0]).abs(), (q[1] - c[1]).abs(), (q[2] - c[2]).abs());
                    let dist = if euclid { (a * a + b * b + cc * cc).sqrt() } else { a + b + cc };
                    if dist < thr && a < ty && b < tu && cc < tvv {
                        su += q[1];
                        sv += q[2];
                        n += 1.0;
                    }
                }
            }
            if n > 0.0 {
                from_yuv([c[0] / 255.0, su / n / 255.0, sv / n / 255.0, c[3]])
            } else {
                d.px[i]
            }
        })
        .collect();
    d.px = out;
    d.store(img);
}

// ------------------------------------------------------------------ HQ 3D Denoiser

fn spatial_smooth(src: &Disp, strength: f32) -> Disp {
    if strength <= 0.0 {
        return Disp { w: src.w, h: src.h, px: src.px.clone() };
    }
    let sig = strength * 2.5 / 255.0;
    let inv = 1.0 / (2.0 * sig * sig);
    let w = src.w;
    let mut out = Disp { w: src.w, h: src.h, px: src.px.clone() };
    out.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let c = src.get(x as isize, y as isize);
            let mut acc = [0.0f32; 3];
            let mut wsum = [0.0f32; 3];
            for dy in -2..=2isize {
                for dx in -2..=2isize {
                    let q = src.get(x as isize + dx, y as isize + dy);
                    let sp = (-((dx * dx + dy * dy) as f32) / 4.5).exp();
                    for k in 0..3 {
                        let d = q[k] - c[k];
                        let wv = sp * (-(d * d) * inv).exp();
                        acc[k] += q[k] * wv;
                        wsum[k] += wv;
                    }
                }
            }
            *o = [acc[0] / wsum[0], acc[1] / wsum[1], acc[2] / wsum[2], c[3]];
        }
    });
    out
}

/// HQ 3D Denoiser / Hqdn3d / Denoiser: edge-preserving spatial smoothing, then a blend toward
/// the (equally smoothed) previous frame where the two barely differ, so still parts stay still.
pub fn hqdn3d(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let sp = fv(e, "spatial", cx).max(0.0);
    let tm = fv(e, "temporal", cx).max(0.0);
    let cur = spatial_smooth(&Disp::of(img), sp);
    let mut out = Disp { w: cur.w, h: cur.h, px: cur.px.clone() };
    if tm > 0.0
        && let Some(p) = neighbour(img, e, cx, -1.0)
    {
        let prev = spatial_smooth(&Disp::of(&p), sp);
        let sig = tm * 2.5 / 255.0;
        let inv = 1.0 / (2.0 * sig * sig);
        out.px.par_iter_mut().zip(prev.px.par_iter()).for_each(|(o, q)| {
            for k in 0..3 {
                let d = q[k] - o[k];
                let wv = 0.6 * (-(d * d) * inv).exp();
                o[k] += d * wv;
            }
        });
    }
    out.store(img);
}

// ------------------------------------------------------------------ Dither

fn bayer(n: usize) -> Vec<f32> {
    // recursive Bayer index matrix of size n (power of two)
    let mut m = vec![0u32];
    let mut s = 1;
    while s < n {
        let mut nm = vec![0u32; 4 * s * s];
        for y in 0..s {
            for x in 0..s {
                let v = 4 * m[y * s + x];
                nm[y * 2 * s + x] = v;
                nm[y * 2 * s + x + s] = v + 2;
                nm[(y + s) * 2 * s + x] = v + 3;
                nm[(y + s) * 2 * s + x + s] = v + 1;
            }
        }
        m = nm;
        s *= 2;
    }
    m.iter().map(|v| (*v as f32 + 0.5) / (n * n) as f32).collect()
}

/// Dither: each channel reduced to `levels` values with an ordered (Bayer 2–16, clustered dot,
/// line) or random threshold pattern chosen by Matrix ID.
pub fn dither(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let levels = fv(e, "levels", cx).round().clamp(2.0, 255.0);
    let id = fv(e, "matrix", cx).round().clamp(0.0, 6.0) as usize;
    let (n, m): (usize, Vec<f32>) = match id {
        0 => (2, bayer(2)),
        1 => (4, bayer(4)),
        3 => (16, bayer(16)),
        // clustered dot (halftone) 4×4
        4 => (4, [12, 5, 6, 13, 4, 0, 1, 7, 11, 3, 2, 8, 15, 10, 9, 14].iter().map(|v| (*v as f32 + 0.5) / 16.0).collect()),
        // horizontal lines 4×4
        5 => (4, [0, 0, 0, 0, 2, 2, 2, 2, 1, 1, 1, 1, 3, 3, 3, 3].iter().map(|v| (*v as f32 + 0.5) / 4.0).collect()),
        6 => (0, Vec::new()),
        _ => (8, bayer(8)),
    };
    let frame = frame_no(cx);
    map_disp(img, move |c, x, y| {
        let t = if n == 0 { rnd(x as i64, y as i64, frame, 41) } else { m[(y % n) * n + x % n] };
        c.map(|v| ((v * (levels - 1.0) + t).floor() / (levels - 1.0)).clamp(0.0, 1.0))
    });
}

// ------------------------------------------------------------------ old film overlays

/// Dust: up to `count` dark specks per frame, each up to `diameter` % of the frame width, at
/// random places with ragged outlines.
pub fn dust(img: &mut Image, diameter: f32, count: f32, cx: &FxCtx) {
    let n = frame_no(cx);
    let (w, h) = (img.w as f32, img.h as f32);
    let maxr = (diameter / 100.0 * w * 0.5).max(0.5);
    let k = (rnd(n, 0, 0, 51) * count.max(0.0)).round() as i64;
    let specks: Vec<(f32, f32, f32, f32)> =
        (0..k).map(|i| (rnd(n, i, 1, 51) * w, rnd(n, i, 2, 51) * h, (0.3 + 0.7 * rnd(n, i, 3, 51)) * maxr, 0.55 + 0.4 * rnd(n, i, 4, 51))).collect();
    if specks.is_empty() {
        return;
    }
    let wi = img.w;
    img.px.par_chunks_mut(wi * 4).enumerate().for_each(|(y, row)| {
        let fy = y as f32 + 0.5;
        for &(sx, sy, r, dark) in &specks {
            if (fy - sy).abs() > r * 1.5 {
                continue;
            }
            let x0 = ((sx - r * 1.5).floor().max(0.0)) as usize;
            let x1 = ((sx + r * 1.5).ceil().min(wi as f32)) as usize;
            for x in x0..x1 {
                let (dx, dy) = (x as f32 + 0.5 - sx, fy - sy);
                let ang = dy.atan2(dx);
                let rag = 1.0 + 0.35 * crate::vfx::noise3(ang.cos() * 2.0 + sx, ang.sin() * 2.0 + sy, n as f32, 52);
                let dd = (dx * dx + dy * dy).sqrt() / (r * rag).max(0.1);
                let cov = (1.0 - (dd - 0.8) / 0.2).clamp(0.0, 1.0) * dark;
                if cov > 0.0 {
                    let p = &mut row[x * 4..x * 4 + 4];
                    for c in 0..3 {
                        p[c] *= 1.0 - cov;
                    }
                }
            }
        }
    });
}

/// Midtone-weighted grain (straight display colour), `amount` 0..1 with per-channel weights.
fn add_grain(img: &mut Image, amount: f32, weights: [f32; 3], cx: &FxCtx, seed: u64) {
    if amount <= 0.0 {
        return;
    }
    let n = frame_no(cx);
    let s = cx.px_scale.max(0.05);
    map_disp(img, move |c, x, y| {
        let (gx, gy) = ((x as f32 / s) as i64, (y as f32 / s) as i64);
        let g = rnd(gx, gy, n, seed) + rnd(gx, gy, n, seed + 1) - 1.0;
        let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let mid = 0.35 + 2.6 * l * (1.0 - l);
        [
            (c[0] + g * amount * weights[0] * mid * 0.3).clamp(0.0, 1.0),
            (c[1] + g * amount * weights[1] * mid * 0.3).clamp(0.0, 1.0),
            (c[2] + g * amount * weights[2] * mid * 0.3).clamp(0.0, 1.0),
        ]
    });
}

/// Film Grain: grain (per-channel amounts), a slight softening, dust and frame-to-frame
/// brightness flicker.
pub fn film_grain(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let blur = fv(e, "blur", cx) / 100.0;
    if blur > 0.0 {
        let sig = blur * 2.0 * cx.px_scale;
        crate::effects::gaussian(img, sig, sig, true);
    }
    let w = [fv(e, "red", cx) / 100.0, fv(e, "green", cx) / 100.0, fv(e, "blue", cx) / 100.0];
    add_grain(img, fv(e, "grain", cx) / 100.0, w, cx, 61);
    let dust_amt = fv(e, "dust", cx);
    if dust_amt > 0.0 {
        dust(img, 1.5, dust_amt * 0.3, cx);
    }
    let fl = fv(e, "flicker", cx) / 100.0;
    if fl > 0.0 {
        let delta = (rnd(frame_no(cx), 0, 0, 62) - 0.5) * fl * 0.3;
        map_disp(img, move |c, _, _| c.map(|v| (v + delta).clamp(0.0, 1.0)));
    }
}

/// Grain (deprecated): uniform grain over all channels, optionally softened first.
pub fn grain(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let blur = fv(e, "blur", cx) / 100.0;
    if blur > 0.0 {
        let sig = blur * 2.0 * cx.px_scale;
        crate::effects::gaussian(img, sig, sig, true);
    }
    add_grain(img, fv(e, "noise", cx) / 100.0, [1.0; 3], cx, 71);
}

/// Gradfun: debanding. Each pixel moves toward its neighbourhood mean where the two differ by
/// less than Strength (8-bit levels), plus a fine ordered dither so the smooth ramp survives
/// 8-bit output.
pub fn gradfun(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let strength = fv(e, "strength", cx).clamp(0.51, 64.0);
    let r = (fv(e, "radius", cx).clamp(4.0, 32.0) * cx.px_scale).round().max(1.0) as usize;
    let mut d = Disp::of(img);
    let (w, h) = (d.w, d.h);
    let mut planes = split_planes(&d, false);
    let dm = bayer(8);
    for plane in planes.iter_mut().take(3) {
        let blurred = blur_plane_box(plane, w, h, r);
        plane.par_iter_mut().zip(blurred.par_iter()).enumerate().for_each(|(i, (v, b))| {
            let diff = b - *v;
            let lv = (diff * 255.0).abs();
            let wv = (1.0 - lv / strength).max(0.0);
            let dith = (dm[((i / w) % 8) * 8 + (i % w) % 8] - 0.5) / 255.0;
            *v += diff * wv * wv + dith * wv;
        });
    }
    merge_planes(&mut d, &planes, false);
    d.store(img);
}

/// Scratch Lines: thin vertical scratches that drift and flicker; each one darkens or lightens
/// the picture behind it.
pub fn scratch_lines(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let n = frame_no(cx);
    let s = cx.px_scale.max(0.05);
    let width = (fv(e, "width", cx) * s).max(0.5);
    let count = fv(e, "count", cx).round().max(0.0) as i64;
    let (dark, light) = (fv(e, "darker", cx) / 255.0 * 2.0, fv(e, "lighter", cx) / 255.0 * 2.0);
    let w = img.w as f32;
    // a scratch lives for a few frames, then another takes its place
    let lines: Vec<(f32, f32, i64)> = (0..count)
        .filter_map(|i| {
            let life = n / 8 + i * 3;
            if rnd(life, i, 0, 81) > 0.7 {
                return None;
            }
            let x = rnd(life, i, 1, 81) * w + (rnd(n, i, 2, 81) - 0.5) * 2.0 * s;
            let amt = if rnd(life, i, 3, 81) < 0.5 { -dark * rnd(n, i, 4, 81) } else { light * rnd(n, i, 4, 81) };
            Some((x, amt, i))
        })
        .collect();
    if lines.is_empty() {
        return;
    }
    let h = img.h as f32;
    map_disp(img, move |c, x, y| {
        let mut o = c;
        for &(lx, amt, i) in &lines {
            let dx = (x as f32 + 0.5 - lx).abs();
            if dx > width {
                continue;
            }
            let along = 0.6 + 0.4 * crate::vfx::noise3(i as f32 * 3.1, y as f32 / h * 6.0, n as f32 * 0.2, 82);
            let k = (1.0 - dx / width) * along;
            o = o.map(|v| (v + amt * k).clamp(0.0, 1.0));
        }
        o
    });
}

// ------------------------------------------------------------------ Wavelet Denoiser

const LA: f32 = -1.586_134_3;
const LB: f32 = -0.052_980_118;
const LG: f32 = 0.882_911_1;
const LD: f32 = 0.443_506_87;
const LK: f32 = 1.149_604_4;

/// One level of the CDF 9/7 lifting transform of `x` (in place: low half, then high half).
fn fwt97(x: &mut [f32], tmp: &mut Vec<f32>) {
    let n = x.len();
    if n < 2 {
        return;
    }
    let ns = n.div_ceil(2);
    let nd = n / 2;
    let mut s: Vec<f32> = (0..ns).map(|i| x[2 * i]).collect();
    let mut d: Vec<f32> = (0..nd).map(|i| x[2 * i + 1]).collect();
    let sa = |s: &[f32], i: usize| s[i.min(ns - 1)];
    let da = |d: &[f32], i: isize| d[i.clamp(0, nd as isize - 1) as usize];
    for i in 0..nd {
        d[i] += LA * (s[i] + sa(&s, i + 1));
    }
    for i in 0..ns {
        s[i] += LB * (da(&d, i as isize - 1) + da(&d, i as isize));
    }
    for i in 0..nd {
        d[i] += LG * (s[i] + sa(&s, i + 1));
    }
    for i in 0..ns {
        s[i] += LD * (da(&d, i as isize - 1) + da(&d, i as isize));
    }
    tmp.clear();
    tmp.extend(s.iter().map(|v| v / LK));
    tmp.extend(d.iter().map(|v| v * LK));
    x.copy_from_slice(tmp);
}

/// Inverse of [`fwt97`].
fn iwt97(x: &mut [f32], tmp: &mut Vec<f32>) {
    let n = x.len();
    if n < 2 {
        return;
    }
    let ns = n.div_ceil(2);
    let nd = n / 2;
    let mut s: Vec<f32> = x[..ns].iter().map(|v| v * LK).collect();
    let mut d: Vec<f32> = x[ns..].iter().map(|v| v / LK).collect();
    let sa = |s: &[f32], i: usize| s[i.min(ns - 1)];
    let da = |d: &[f32], i: isize| d[i.clamp(0, nd as isize - 1) as usize];
    for i in 0..ns {
        s[i] -= LD * (da(&d, i as isize - 1) + da(&d, i as isize));
    }
    for i in 0..nd {
        d[i] -= LG * (s[i] + sa(&s, i + 1));
    }
    for i in 0..ns {
        s[i] -= LB * (da(&d, i as isize - 1) + da(&d, i as isize));
    }
    for i in 0..nd {
        d[i] -= LA * (s[i] + sa(&s, i + 1));
    }
    tmp.clear();
    tmp.resize(n, 0.0);
    for i in 0..ns {
        tmp[2 * i] = s[i];
    }
    for i in 0..nd {
        tmp[2 * i + 1] = d[i];
    }
    x.copy_from_slice(tmp);
}

/// 2-D transform of the top-left `cw`×`ch` region of a `w`-wide plane (rows, then columns).
fn wt2(p: &mut [f32], w: usize, cw: usize, ch: usize, inverse: bool) {
    let rows = |p: &mut [f32]| {
        p.par_chunks_mut(w).take(ch).for_each(|row| {
            let mut tmp = Vec::new();
            if inverse { iwt97(&mut row[..cw], &mut tmp) } else { fwt97(&mut row[..cw], &mut tmp) }
        });
    };
    let cols = |p: &mut [f32]| {
        let colv: Vec<Vec<f32>> = (0..cw)
            .into_par_iter()
            .map(|x| {
                let mut c: Vec<f32> = (0..ch).map(|y| p[y * w + x]).collect();
                let mut tmp = Vec::new();
                if inverse { iwt97(&mut c, &mut tmp) } else { fwt97(&mut c, &mut tmp) }
                c
            })
            .collect();
        for (x, c) in colv.into_iter().enumerate() {
            for (y, v) in c.into_iter().enumerate() {
                p[y * w + x] = v;
            }
        }
    };
    if inverse {
        cols(p);
        rows(p);
    } else {
        rows(p);
        cols(p);
    }
}

/// Wavelet Denoiser: CDF 9/7 decomposition (Steps levels), soft / hard / garrote shrinkage of
/// the detail bands with a universal or per-band (Bayes) threshold, partial by Percentage.
pub fn wavelet(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let pl = Planes::from_yuv_choice(pick(e, "planes"));
    let thr = fv(e, "threshold", cx).max(0.0);
    let pctv = (fv(e, "percent", cx) / 100.0).clamp(0.0, 1.0);
    if pl.none() || thr <= 0.0 || pctv <= 0.0 {
        return;
    }
    let method = pick(e, "method");
    let bayes = pick(e, "threshold_type") == 1;
    let max_steps = fv(e, "steps", cx).round().clamp(1.0, 32.0) as usize;
    let mut d = Disp::of(img);
    let (w, h) = (d.w, d.h);
    let mut planes = split_planes(&d, true);
    let shrinkf = |c: f32, t: f32| -> f32 {
        let a = c.abs();
        let s = match method {
            0 => c.signum() * (a - t).max(0.0),
            1 => {
                if a < t { 0.0 } else { c }
            }
            _ => {
                if a < t { 0.0 } else { c - t * t / c }
            }
        };
        c + (s - c) * pctv
    };
    for k in 0..4 {
        if !pl.on[k] {
            continue;
        }
        let p = &mut planes[k];
        p.iter_mut().for_each(|v| *v *= 255.0);
        let mut sizes = Vec::new();
        let (mut cw, mut ch) = (w, h);
        while sizes.len() < max_steps && cw >= 8 && ch >= 8 {
            wt2(p, w, cw, ch, false);
            sizes.push((cw, ch));
            cw = cw.div_ceil(2);
            ch = ch.div_ceil(2);
        }
        // shrink each detail band of each level
        for &(lw, lh) in &sizes {
            let (lw2, lh2) = (lw.div_ceil(2), lh.div_ceil(2));
            for band in 0..3 {
                let (x0, x1, y0, y1) = match band {
                    0 => (lw2, lw, 0, lh2),
                    1 => (0, lw2, lh2, lh),
                    _ => (lw2, lw, lh2, lh),
                };
                let t = if bayes {
                    let mut ss = 0.0f64;
                    let mut cnt = 0.0f64;
                    for y in y0..y1 {
                        for x in x0..x1 {
                            ss += (p[y * w + x] as f64).powi(2);
                            cnt += 1.0;
                        }
                    }
                    let var = if cnt > 0.0 { ss / cnt } else { 0.0 };
                    let sx = (var - (thr as f64).powi(2)).max(1e-6).sqrt();
                    ((thr as f64).powi(2) / sx).min(thr as f64 * 8.0) as f32
                } else {
                    thr
                };
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = y * w + x;
                        p[i] = shrinkf(p[i], t);
                    }
                }
            }
        }
        for &(lw, lh) in sizes.iter().rev() {
            wt2(p, w, lw, lh, true);
        }
        p.iter_mut().for_each(|v| *v /= 255.0);
    }
    merge_planes(&mut d, &planes, true);
    d.store(img);
}
