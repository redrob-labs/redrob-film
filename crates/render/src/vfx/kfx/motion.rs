//! Motion effects of the bin: gate weave, row / colour glitches, the max-luma trail (Lag Fun),
//! shuffled frame order (Nervous) and the zoom-and-turn feedback of Vertigo.

use redrob_film_geom::Affine;

use super::*;
use crate::vfx::{hash01, noise1};

/// Gate Weave: the picture drifts to a new random offset (up to the maximum movements, in clip
/// pixels) every Interval frames, moving smoothly between them as a film gate does.
pub fn gate_weave(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let interval = fv(e, "interval", cx).max(1.0) as f64;
    let (mx, my) = (fv(e, "max_x", cx) as f64, fv(e, "max_y", cx) as f64);
    if mx <= 0.0 && my <= 0.0 {
        return;
    }
    let seed = fv(e, "seed", cx).max(0.0) as u64;
    let t = frame_no(cx) as f64 / interval;
    let k = cx.px_scale as f64;
    let (dx, dy) = (noise1(t, 1, seed) as f64 * mx * k, noise1(t, 2, seed) as f64 * my * k);
    *img = img.transformed(img.w, img.h, &Affine::translate(dx, dy));
}

/// Glitching (rows and colour): on glitching frames, random bands of rows shift sideways and
/// their colour channels are swapped or pushed.
pub fn glitch0r(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let freq = fv(e, "frequency", cx) / 100.0;
    let f = frame_no(cx);
    if freq <= 0.0 || hash01(f, 0, 0, 0x6c17) > freq {
        return;
    }
    let (w, h) = (img.w, img.h);
    let max_block = ((fv(e, "block_height", cx) / 100.0) * h as f32 / 4.0).max(1.0);
    let shift = fv(e, "shift", cx) / 100.0 * w as f32 / 4.0;
    let color = fv(e, "color", cx) / 100.0;
    let src = img.clone();
    let mut y = 0usize;
    let mut band = 0i64;
    while y < h {
        let bh = (1.0 + hash01(f, band, 1, 0x6c17) * max_block) as usize;
        let glitched = hash01(f, band, 2, 0x6c17) < 0.5;
        let dx = ((hash01(f, band, 3, 0x6c17) * 2.0 - 1.0) * shift).round() as isize;
        let swap = (hash01(f, band, 4, 0x6c17) * 3.0) as usize % 3;
        let push = hash01(f, band, 5, 0x6c17) * color;
        for yy in y..(y + bh).min(h) {
            if !glitched {
                continue;
            }
            for x in 0..w {
                let p = src.get(((x as isize - dx).rem_euclid(w as isize)) as usize, yy);
                let mut o = p;
                if color > 0.0 {
                    o[0] = p[swap];
                    o[1] = p[(swap + 1) % 3];
                    o[2] = p[(swap + 2) % 3] + push * p[3];
                }
                let i = (yy * w + x) * 4;
                img.px[i..i + 4].copy_from_slice(&o);
            }
        }
        y += bh;
        band += 1;
    }
}

/// Glitching (rows only): blocks of rows (fixed height, or random between the minimum and maximum
/// when Block Height is 0) shift left and right by a random amount each frame.
pub fn pixs0r(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let shift = fv(e, "shift", cx).clamp(0.0, 1.0);
    if shift <= 0.0 {
        return;
    }
    let (w, h) = (img.w, img.h);
    let k = cx.px_scale.max(1e-3);
    let fixed = fv(e, "block_height", cx) * k;
    let lo = (fv(e, "min_height", cx) * k).max(1.0);
    let hi = (fv(e, "max_height", cx) * k).max(lo);
    let f = frame_no(cx);
    let src = img.clone();
    let mut y = 0usize;
    let mut band = 0i64;
    while y < h {
        let bh = if fixed >= 1.0 { fixed } else { lo + hash01(f, band, 7, 0x9123) * (hi - lo) }.round().max(1.0) as usize;
        let dx = ((hash01(f, band, 8, 0x9123) * 2.0 - 1.0) * shift * w as f32).round() as isize;
        for yy in y..(y + bh).min(h) {
            for x in 0..w {
                let p = src.get(((x as isize - dx).rem_euclid(w as isize)) as usize, yy);
                let i = (yy * w + x) * 4;
                img.px[i..i + 4].copy_from_slice(&p);
            }
        }
        y += bh;
        band += 1;
    }
}

/// How many earlier frames a decaying trail still shows (decay^n above 1/255), capped.
fn trail_len(decay: f32, cap: usize) -> usize {
    if decay <= 0.0 {
        return 0;
    }
    if decay >= 0.9999 {
        return cap;
    }
    ((1.0f32 / 255.0).ln() / decay.ln()).ceil().clamp(0.0, cap as f32) as usize
}

/// Lag Fun: darker pixels update slowly — each pixel keeps the brightest of the current frame
/// and the earlier frames dimmed by Decay once per frame.
pub fn lagfun(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let decay = fv(e, "decay", cx).clamp(0.0, 1.0);
    let n = trail_len(decay, 24);
    if n == 0 || cx.env.is_none() {
        return;
    }
    let mut k = 1.0f32;
    for back in 1..=n {
        k *= decay;
        let Some(prev) = neighbour(img, e, cx, -(back as f64)) else { break };
        img.px.par_chunks_mut(4).zip(prev.px.par_chunks(4)).for_each(|(p, q)| {
            for c in 0..3 {
                p[c] = p[c].max(q[c] * k);
            }
            p[3] = p[3].max(q[3] * k);
        });
    }
}

/// The frames Nervous shows: frames are taken in groups of eight and each group is played in a
/// shuffled order, so no frame is dropped.
pub(crate) fn nervous_offset(frame: i64) -> i64 {
    const N: i64 = 8;
    let group = frame.div_euclid(N);
    let mut order: [i64; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
    // Fisher–Yates with a per-group hash
    for i in (1..8usize).rev() {
        let j = (hash01(group, i as i64, 0, 0x4e52) * (i as f32 + 1.0)) as usize % (i + 1);
        order.swap(i, j);
    }
    group * N + order[frame.rem_euclid(N) as usize] - frame
}

/// Nervous: plays the clip's frames in a shuffled ("nervous") order.
pub fn nervous(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let d = nervous_offset(frame_no(cx));
    if d == 0 {
        return;
    }
    if let Some(f) = neighbour(img, e, cx, d as f64) {
        *img = f;
    }
}

/// Vertigo: the frame is alpha-blended with zoomed, turned copies of the earlier output, the
/// turn advancing by Phase Increment each frame.
pub fn vertigo(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    if cx.env.is_none() {
        return;
    }
    let phase_inc = fv(e, "phase", cx) as f64 * 0.01;
    let zoom = fv(e, "zoom", cx) as f64 / 100.0;
    let (w, h) = (img.w as f64, img.h as f64);
    let f = frame_no(cx);
    const DEPTH: usize = 6;
    // oldest first: each step blends a frame with the transformed accumulation
    let mut frames = Vec::with_capacity(DEPTH);
    for back in (1..=DEPTH).rev() {
        if let Some(p) = neighbour(img, e, cx, -(back as f64)) {
            frames.push((back, p));
        }
    }
    let mut acc: Option<Image> = None;
    let step = |acc: &Image, frame_index: i64| -> Image {
        let ang = (frame_index as f64 * phase_inc).sin() * 4.0;
        let m = Affine::translate(w / 2.0, h / 2.0)
            .then_apply(&Affine::rotate_deg(ang))
            .then_apply(&Affine::scale(zoom, zoom))
            .then_apply(&Affine::translate(-w / 2.0, -h / 2.0));
        acc.transformed(acc.w, acc.h, &m)
    };
    for (back, frame) in frames {
        acc = Some(match acc {
            None => frame,
            Some(a) => {
                let t = step(&a, f - back as i64);
                frame.lerp(&t, 0.5)
            }
        });
    }
    if let Some(a) = acc {
        let t = step(&a, f);
        let cur = std::mem::replace(img, Image::new(0, 0));
        *img = cur.lerp(&t, 0.5);
    }
}
