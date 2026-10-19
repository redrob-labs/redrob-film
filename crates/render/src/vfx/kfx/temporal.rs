//! Effects of the "More Video Effects" bin that read other frames of the clip: Analog Video Echo,
//! Burning TV, Light Graffiti and the Photosensitivity Filter.
//!
//! Light Graffiti and the Photosensitivity Filter carry state from frame to frame (a light mask,
//! a flash history). The state is kept per clip and parameter set; playing forward advances it
//! one frame at a time, and any other jump replays the frames before the new position (at most
//! [`MAX_REPLAY`] of them) so a frame looks the same however it was reached.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use super::*;

/// Most frames replayed to rebuild a stateful effect after a jump.
pub const MAX_REPLAY: i64 = 250;

/// Analog Video Echo: earlier frames (every Strobe Period + 1 frames) fade behind the current
/// one, keeping whichever is brighter (or darker) per kept channel.
pub fn analog_echo(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let fade = fv(e, "fade", cx).clamp(0.0, 0.99);
    if fade <= 0.0 || cx.env.is_none() {
        return;
    }
    let darker = pick(e, "direction") == 1;
    let keep = [flag(e, "keep_r"), flag(e, "keep_g"), flag(e, "keep_b")];
    let step = fv(e, "strobe", cx).round().clamp(0.0, 8.0) + 1.0;
    let mut d = Disp::of(img);
    let mut wgt = 1.0f32;
    for j in 1..=12 {
        wgt *= fade;
        if wgt < 0.03 {
            break;
        }
        let Some(p) = neighbour(img, e, cx, -(j as f64) * step as f64) else { break };
        let pd = Disp::of(&p);
        let k = wgt;
        d.px.par_iter_mut().zip(pd.px.par_iter()).for_each(|(o, q)| {
            for c in 0..3 {
                if !keep[c] {
                    continue;
                }
                o[c] = if darker { o[c].min(1.0 - (1.0 - q[c]) * k) } else { o[c].max(q[c] * k) };
            }
            o[3] = o[3].max(q[3] * k);
        });
    }
    d.store(img);
}

fn fire_color(v: f32) -> [f32; 3] {
    let v = v.clamp(0.0, 1.0);
    [(v * 3.0).min(1.0), (v * 3.0 - 1.0).clamp(0.0, 1.0), (v * 3.0 - 2.0).clamp(0.0, 1.0)]
}

/// Burning TV: what moves (against the previous frame, or against the clip's first frame with
/// Foreground Only) catches fire; the flames rise and flicker over the picture.
pub fn burning_tv(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some(env) = cx.env else { return };
    let thr = fv(e, "threshold", cx) / 255.0;
    let reference = if flag(e, "foreground") { neighbour(img, e, cx, -(env.clip_offset() * fps(cx)).round()) } else { neighbour(img, e, cx, -1.0) };
    let Some(r) = reference else { return };
    let rd = Disp::of(&r);
    let mut d = Disp::of(img);
    let (w, h) = (d.w, d.h);
    let mask: Vec<f32> =
        d.px.par_iter()
            .zip(rd.px.par_iter())
            .map(|(a, b)| {
                let m = (a[0] - b[0]).abs().max((a[1] - b[1]).abs()).max((a[2] - b[2]).abs());
                if m > thr { 1.0 } else { 0.0 }
            })
            .collect();
    let n = frame_no(cx);
    let s = cx.px_scale.max(0.05);
    let decay = 0.93f32.powf(1.0 / s);
    // flames: each column carries heat upward, losing some each pixel, with flicker
    let mut fire = vec![0.0f32; w * h];
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let mut col = vec![0.0f32; h];
            let mut heat = 0.0f32;
            for y in (0..h).rev() {
                let fl = 0.85 + 0.3 * rnd(x as i64 / 2, y as i64 / 2, n, 91);
                heat = (heat * decay * fl).max(mask[y * w + x]);
                col[y] = heat.min(1.0);
            }
            col
        })
        .collect();
    for (x, col) in cols.into_iter().enumerate() {
        for (y, v) in col.into_iter().enumerate() {
            fire[y * w + x] = v;
        }
    }
    d.px.par_iter_mut().zip(fire.par_iter()).for_each(|(o, f)| {
        if *f <= 0.01 {
            return;
        }
        let c = fire_color(*f);
        for k in 0..3 {
            o[k] = 1.0 - (1.0 - o[k]) * (1.0 - c[k]);
        }
        o[3] = o[3].max(*f);
    });
    d.store(img);
}

// ------------------------------------------------------------------ frame-to-frame state

struct Seq<S> {
    n: i64,
    state: S,
    out: Vec<[f32; 4]>,
}

type Store<S> = Mutex<HashMap<u64, Seq<S>>>;

fn state_key(e: &EffectInstance, cx: &FxCtx, img: &Image) -> u64 {
    let mut hh = std::collections::hash_map::DefaultHasher::new();
    cx.env.map(|env| env.clip_key()).unwrap_or(0).hash(&mut hh);
    e.effect.hash(&mut hh);
    serde_json::to_string(&e.params).unwrap_or_default().hash(&mut hh);
    (img.w, img.h).hash(&mut hh);
    hh.finish()
}

/// Advance (or rebuild) a stateful effect to frame `n` of the clip and return its picture.
/// `init` makes the state from the clip's first frame; `step` feeds one frame.
fn sequential<S: Clone + Send>(
    store: &Store<S>,
    img: &Image,
    e: &EffectInstance,
    cx: &FxCtx,
    init: impl Fn(&Disp) -> S,
    step: impl Fn(&mut S, &Disp) -> Vec<[f32; 4]>,
) -> Option<Vec<[f32; 4]>> {
    cx.env?;
    let key = state_key(e, cx, img);
    let n = frame_no(cx).max(0);
    let cached = {
        let map = store.lock().unwrap_or_else(|p| p.into_inner());
        map.get(&key).map(|s| (s.n, s.state.clone(), s.out.clone()))
    };
    let cur = Disp::of(img);
    let (state, out) = match cached {
        Some((cn, _, out)) if cn == n => return Some(out),
        Some((cn, mut st, _)) if cn == n - 1 => {
            let out = step(&mut st, &cur);
            (st, out)
        }
        _ => {
            let first = if n == 0 { Disp { w: cur.w, h: cur.h, px: cur.px.clone() } } else { Disp::of(&neighbour(img, e, cx, -(n as f64))?) };
            let mut st = init(&first);
            let start = (n - MAX_REPLAY).max(0);
            for k in start..n {
                if k == 0 {
                    step(&mut st, &first);
                } else if let Some(f) = neighbour(img, e, cx, (k - n) as f64) {
                    step(&mut st, &Disp::of(&f));
                }
            }
            let out = step(&mut st, &cur);
            (st, out)
        }
    };
    let mut map = store.lock().unwrap_or_else(|p| p.into_inner());
    if map.len() > 8 && !map.contains_key(&key) {
        if let Some(k) = map.keys().next().copied() {
            map.remove(&k);
        }
    }
    map.insert(key, Seq { n, state, out: out.clone() });
    Some(out)
}

// ------------------------------------------------------------------ Light Graffiti

#[derive(Clone)]
struct Graffiti {
    bg: Vec<[f32; 3]>,
    light: Vec<[f32; 3]>,
}

struct GraffitiParams {
    brightness: f32,
    difference: f32,
    difference_sum: f32,
    sensitivity: f32,
    lower: f32,
    dimming: f32,
    nonlinear: bool,
    bg_weight: f32,
    alpha: f32,
    saturation: f32,
    transparent: bool,
    stats: [bool; 3],
}

static GRAFFITI: OnceLock<Store<Graffiti>> = OnceLock::new();

fn graffiti_step(st: &mut Graffiti, f: &Disp, p: &GraffitiParams) -> Vec<[f32; 4]> {
    let mut out = vec![[0.0f32; 4]; f.px.len()];
    st.bg.par_iter_mut().zip(st.light.par_iter_mut()).zip(f.px.par_iter()).zip(out.par_iter_mut()).for_each(|(((bg, l), c), o)| {
        let sum = (c[0] + c[1] + c[2]) * 255.0;
        let dr = [(c[0] - bg[0]).abs(), (c[1] - bg[1]).abs(), (c[2] - bg[2]).abs()];
        let dmax = dr[0].max(dr[1]).max(dr[2]) * 255.0;
        let dsum = (dr[0] + dr[1] + dr[2]) * 255.0;
        let pb = sum >= p.brightness;
        let pd = dmax >= p.difference;
        let ps = dsum >= p.difference_sum;
        if pb && pd && ps {
            let k = p.sensitivity / 100.0 / (1.0 + p.lower);
            for i in 0..3 {
                l[i] = (l[i] + c[i] * k).min(1.0);
            }
        }
        if p.dimming > 0.0 {
            for v in l.iter_mut() {
                let dk = if p.nonlinear { p.dimming * *v } else { p.dimming };
                *v *= 1.0 - dk;
            }
        }
        if p.alpha > 0.0 && !(pb && pd && ps) {
            for i in 0..3 {
                bg[i] += (c[i] - bg[i]) * p.alpha;
            }
        }
        // saturation of the lights
        let g = (l[0] + l[1] + l[2]) / 3.0;
        let lit =
            [(g + (l[0] - g) * p.saturation).clamp(0.0, 1.0), (g + (l[1] - g) * p.saturation).clamp(0.0, 1.0), (g + (l[2] - g) * p.saturation).clamp(0.0, 1.0)];
        *o = if p.transparent {
            [lit[0], lit[1], lit[2], lit[0].max(lit[1]).max(lit[2])]
        } else {
            let base = [c[0] + (bg[0] - c[0]) * p.bg_weight, c[1] + (bg[1] - c[1]) * p.bg_weight, c[2] + (bg[2] - c[2]) * p.bg_weight];
            [(base[0] + lit[0]).min(1.0), (base[1] + lit[1]).min(1.0), (base[2] + lit[2]).min(1.0), c[3]]
        };
        // statistics: tint where each test passes (red brightness, green difference, blue sum)
        if p.stats.iter().any(|s| *s) {
            let marks = [p.stats[0] && pb, p.stats[1] && pd, p.stats[2] && ps];
            for i in 0..3 {
                if marks[i] {
                    o[i] = 1.0;
                }
            }
        }
    });
    out
}

/// Light Graffiti: bright light sources that differ from the (first-frame) background are
/// painted into a light mask that stays, so moving lights leave trails.
pub fn light_graffiti(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    if flag(e, "reset") {
        return;
    }
    let p = GraffitiParams {
        brightness: fv(e, "brightness", cx),
        difference: fv(e, "difference", cx),
        difference_sum: fv(e, "difference_sum", cx),
        sensitivity: fv(e, "sensitivity", cx).max(0.0),
        lower: fv(e, "lower_overexposure", cx).max(0.0),
        dimming: (fv(e, "dimming", cx) / 100.0).clamp(0.0, 1.0),
        nonlinear: flag(e, "nonlinear"),
        bg_weight: (fv(e, "bg_weight", cx) / 100.0).clamp(0.0, 1.0),
        alpha: (fv(e, "alpha", cx) / 100.0).clamp(0.0, 1.0) * 0.2,
        saturation: fv(e, "saturation", cx).max(0.0) / 100.0,
        transparent: flag(e, "transparent"),
        stats: [flag(e, "stats_brightness"), flag(e, "stats_diff"), flag(e, "stats_diff_sum")],
    };
    let store = GRAFFITI.get_or_init(Default::default);
    let init = |f: &Disp| Graffiti { bg: f.px.iter().map(|c| [c[0], c[1], c[2]]).collect(), light: vec![[0.0; 3]; f.px.len()] };
    if let Some(out) = sequential(store, img, e, cx, init, |st, f| graffiti_step(st, f, &p)) {
        let d = Disp { w: img.w, h: img.h, px: out };
        d.store(img);
    }
}

// ------------------------------------------------------------------ Photosensitivity Filter

#[derive(Clone)]
struct Flash {
    /// Running average of the shown pictures.
    avg: Vec<[f32; 4]>,
    /// Luma change of each recent frame (8-bit units), newest last.
    history: Vec<f32>,
    last_luma: Vec<f32>,
}

static FLASH: OnceLock<Store<Flash>> = OnceLock::new();

/// Photosensitivity Filter: frames whose brightness swings too much over the recent frames are
/// pulled toward the recent average, so flashes are flattened.
pub fn photosensitivity(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    if flag(e, "bypass") {
        return;
    }
    let frames = fv(e, "frames", cx).round().clamp(2.0, 240.0) as usize;
    let thr = fv(e, "threshold", cx).max(0.01);
    let skip = fv(e, "skip", cx).round().clamp(1.0, 1024.0) as usize;
    let sample = |f: &Disp| -> Vec<f32> { f.px.iter().step_by(skip).map(|p| luma(*p) * 255.0).collect() };
    let init = |f: &Disp| Flash { avg: f.px.clone(), history: Vec::new(), last_luma: sample(f) };
    let step = |st: &mut Flash, f: &Disp| -> Vec<[f32; 4]> {
        let l = sample(f);
        let change = if l.is_empty() { 0.0 } else { l.iter().zip(&st.last_luma).map(|(a, b)| (a - b).abs()).sum::<f32>() / l.len() as f32 };
        st.history.push(change);
        if st.history.len() > frames {
            st.history.remove(0);
        }
        // a flashing stretch: the mean change over the window, against the threshold (×10
        // levels per frame at 1.0)
        let badness = st.history.iter().sum::<f32>() / st.history.len().max(1) as f32;
        let k = ((badness / (thr * 10.0)) - 1.0).clamp(0.0, 1.0);
        let out: Vec<[f32; 4]> =
            f.px.iter().zip(&st.avg).map(|(c, a)| [c[0] + (a[0] - c[0]) * k, c[1] + (a[1] - c[1]) * k, c[2] + (a[2] - c[2]) * k, c[3]]).collect();
        let r = 1.0 / frames as f32;
        for (a, o) in st.avg.iter_mut().zip(&out) {
            for i in 0..4 {
                a[i] += (o[i] - a[i]) * r * 2.0;
            }
        }
        st.last_luma = out.iter().step_by(skip).map(|p| luma(*p) * 255.0).collect();
        out
    };
    let store = FLASH.get_or_init(Default::default);
    if let Some(out) = sequential(store, img, e, cx, init, step) {
        let d = Disp { w: img.w, h: img.h, px: out };
        d.store(img);
    }
}
