//! Burnt-in scopes and measuring overlays of the bin (Kdenlive's Utility effects): histogram,
//! waveform, RGB parade, vectorscopes, CIE diagram, data scope, oscilloscopes, video values and
//! the timeout indicator, plus the Nikon D90 stair-stepping fix. Unlike the Lumetri Scopes panel
//! these draw into the rendered picture, so they show in exports.
//!
//! Scopes read the picture's straight display values (sRGB-encoded, 0..1) and draw over it.

use super::text::{TextBox, draw_text_box};
use super::*;

// ------------------------------------------------------------------ shared drawing

/// Y'CbCr of display RGB with the given (Kr, Kb); Cb and Cr centred on 0.
#[inline]
fn ycc(c: [f32; 4], kr: f32, kb: f32) -> [f32; 3] {
    let y = kr * c[0] + (1.0 - kr - kb) * c[1] + kb * c[2];
    [y, (c[2] - y) / (2.0 * (1.0 - kb)), (c[0] - y) / (2.0 * (1.0 - kr))]
}
const BT709: (f32, f32) = (0.2126, 0.0722);
const BT601: (f32, f32) = (0.299, 0.114);

/// Display RGB of a Y'CbCr triple (Cb / Cr centred on 0).
#[inline]
fn rgb_of(y: f32, cb: f32, cr: f32, (kr, kb): (f32, f32)) -> [f32; 3] {
    let r = y + 2.0 * (1.0 - kr) * cr;
    let b = y + 2.0 * (1.0 - kb) * cb;
    let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]
}

/// A decimated straight display copy of the picture (at most `max_w` samples across).
fn samples(img: &Image, max_w: usize) -> (usize, usize, Vec<[f32; 4]>) {
    let d = Disp::of(img);
    let step = d.w.div_ceil(max_w.max(1)).max(1);
    let (sw, sh) = (d.w.div_ceil(step), d.h.div_ceil(step));
    let mut v = Vec::with_capacity(sw * sh);
    for y in 0..sh {
        for x in 0..sw {
            v.push(d.px[(y * step).min(d.h - 1) * d.w + (x * step).min(d.w - 1)]);
        }
    }
    (sw, sh, v)
}

/// Darken `[x0, y0, x1, y1]` towards black by `o` (background opacity).
fn shade(img: &mut Image, r: [f32; 4], o: f32) {
    if o <= 0.0 {
        return;
    }
    super::text::fill_rect(img, r, [0.0, 0.0, 0.0, 1.0], o.clamp(0.0, 1.0));
}

/// Composite a straight display colour with coverage `k` over a premultiplied pixel.
#[inline]
fn put(p: &mut [f32], c: [f32; 3], k: f32) {
    let k = k.clamp(0.0, 1.0);
    if k <= 0.0 {
        return;
    }
    let l = crate::effects::dec(c);
    for i in 0..3 {
        p[i] = l[i] * k + p[i] * (1.0 - k);
    }
    p[3] = k + p[3] * (1.0 - k);
}

/// A hit-count grid: hits (weighted) and the sum of the hit colours per cell.
struct Grid {
    w: usize,
    h: usize,
    n: Vec<f32>,
    c: Vec<[f32; 3]>,
}

impl Grid {
    fn new(w: usize, h: usize) -> Grid {
        Grid { w: w.max(1), h: h.max(1), n: vec![0.0; w.max(1) * h.max(1)], c: vec![[0.0; 3]; w.max(1) * h.max(1)] }
    }
    /// A hit at (`u`, `v`) in 0..1 (`v` = 0 at the top).
    #[inline]
    fn hit(&mut self, u: f32, v: f32, col: [f32; 3], wgt: f32) {
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return;
        }
        let x = ((u * self.w as f32) as usize).min(self.w - 1);
        let y = ((v * self.h as f32) as usize).min(self.h - 1);
        let i = y * self.w + x;
        self.n[i] += wgt;
        for k in 0..3 {
            self.c[i][k] += col[k] * wgt;
        }
    }
    fn cell(&self, u: f32, v: f32) -> (f32, [f32; 3]) {
        let x = ((u * self.w as f32) as usize).min(self.w - 1);
        let y = ((v * self.h as f32) as usize).min(self.h - 1);
        let i = y * self.w + x;
        (self.n[i], self.c[i])
    }
    /// Whether any 4-neighbour of a hit cell is empty (the hit region's outline).
    fn edge(&self, u: f32, v: f32) -> bool {
        let x = ((u * self.w as f32) as isize).min(self.w as isize - 1);
        let y = ((v * self.h as f32) as isize).min(self.h as isize - 1);
        let at = |x: isize, y: isize| x < 0 || y < 0 || x >= self.w as isize || y >= self.h as isize || self.n[y as usize * self.w + x as usize] <= 0.0;
        !at(x, y) && (at(x - 1, y) || at(x + 1, y) || at(x, y - 1) || at(x, y + 1))
    }
}

/// Paint a grid into `r`: `f(hits, colour sum, u, v)` gives the colour and coverage of a pixel.
fn paint(img: &mut Image, r: [f32; 4], f: impl Fn(f32, [f32; 3], f32, f32) -> Option<([f32; 3], f32)> + Sync) {
    let (x0, y0) = (r[0].max(0.0) as usize, r[1].max(0.0) as usize);
    let (x1, y1) = ((r[2].ceil().max(0.0) as usize).min(img.w), (r[3].ceil().max(0.0) as usize).min(img.h));
    let (rw, rh) = ((r[2] - r[0]).max(1.0), (r[3] - r[1]).max(1.0));
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        if y < y0 || y >= y1 {
            return;
        }
        let v = (y as f32 + 0.5 - r[1]) / rh;
        for x in x0..x1 {
            let u = (x as f32 + 0.5 - r[0]) / rw;
            if let Some((c, k)) = f(0.0, [0.0; 3], u, v) {
                put(&mut row[x * 4..x * 4 + 4], c, k);
            }
        }
    });
}

/// Paint a grid's hits: brightness `min(1, hits × intensity)`, coloured by `colour(avg colour)`.
fn paint_grid(img: &mut Image, g: &Grid, r: [f32; 4], intensity: f32, colour: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
    paint(img, r, |_, _, u, v| {
        let (n, c) = g.cell(u, v);
        if n <= 0.0 {
            return None;
        }
        let avg = [c[0] / n, c[1] / n, c[2] / n];
        Some((colour(avg), (n * intensity).min(1.0)))
    });
}

/// A small text label (display colour) at (`x`, `y`) = its top-left.
fn label(img: &mut Image, text: &str, x: f32, y: f32, size: f32, col: [f32; 3]) {
    let b = TextBox {
        family: redrob_film_text::fonts::DEFAULT_FAMILY.to_string(),
        style: "Regular",
        size: size.max(4.0),
        outline: 0.0,
        padding: 0.0,
        h_align: 0,
        v_align: 0,
        opacity: 1.0,
        fg: [col[0], col[1], col[2], 1.0],
        bg: [0.0; 4],
        outline_color: [0.0; 4],
        rect: [x, y, img.w as f32, size * 1.4],
        underline: false,
        strikethrough: false,
    };
    draw_text_box(img, text, &b);
}

/// A horizontal or vertical guide line in display colour.
fn hline(img: &mut Image, x0: f32, x1: f32, y: f32, col: [f32; 3], k: f32, dotted: bool) {
    let yy = y.floor() as isize;
    if yy < 0 || yy >= img.h as isize {
        return;
    }
    let w = img.w;
    for x in (x0.max(0.0) as usize)..(x1.max(0.0) as usize).min(w) {
        if dotted && (x / 2) % 2 == 1 {
            continue;
        }
        let i = (yy as usize * w + x) * 4;
        put(&mut img.px[i..i + 4], col, k);
    }
}
fn vline(img: &mut Image, x: f32, y0: f32, y1: f32, col: [f32; 3], k: f32, dotted: bool) {
    let xx = x.floor() as isize;
    if xx < 0 || xx >= img.w as isize {
        return;
    }
    let w = img.w;
    for y in (y0.max(0.0) as usize)..(y1.max(0.0) as usize).min(img.h) {
        if dotted && (y / 2) % 2 == 1 {
            continue;
        }
        let i = (y * w + xx as usize) * 4;
        put(&mut img.px[i..i + 4], col, k);
    }
}

/// The components of a [`redrob_film_project::effect::YUV_PLANES`] choice, as indices into
/// Y, U, V, A.
fn plane_list(choice: u32) -> Vec<usize> {
    let (m, a) = yuv_planes(choice);
    let mut v: Vec<usize> = (0..3).filter(|i| m[*i]).collect();
    if a {
        v.push(3);
    }
    v
}

/// Y, U (+0.5), V (+0.5), A of a display pixel (BT.709).
#[inline]
fn yuva(p: [f32; 4]) -> [f32; 4] {
    let q = ycc(p, BT709.0, BT709.1);
    [q[0], q[1] + 0.5, q[2] + 0.5, p[3]]
}

/// The trace colour of a Y / U / V / A component.
const COMP_COLORS: [[f32; 3]; 4] = [[0.9, 0.9, 0.9], [0.35, 0.55, 1.0], [1.0, 0.4, 0.35], [0.6, 0.6, 0.6]];

/// Peak-envelope history: the picture of up to `n` earlier frames (for Peak envelopes).
fn history(img: &Image, e: &EffectInstance, cx: &FxCtx, n: usize) -> Vec<Image> {
    (1..=n).filter_map(|k| neighbour(img, e, cx, -(k as f64 * 3.0))).collect()
}

// ------------------------------------------------------------------ Histogram

/// Histogram: 256-bin distributions of the chosen components with a colour scale under each,
/// stacked, side by side (parade) or overlaid, at the top left.
pub fn histogram(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let comps = plane_list(pick(e, "components"));
    if comps.is_empty() {
        return;
    }
    let (_, _, px) = samples(img, 640);
    let mut bins = vec![[0.0f32; 256]; 4];
    for p in &px {
        let q = yuva(*p);
        for &c in &comps {
            bins[c][(q[c].clamp(0.0, 1.0) * 255.0).round() as usize] += 1.0;
        }
    }
    let k = cx.px_scale.max(0.1);
    let gw = (256.0 * k).min(img.w as f32);
    let lh = fv(e, "level_height", cx) * k;
    let sh = fv(e, "scale_height", cx) * k;
    let log = pick(e, "mode") == 1;
    let display = pick(e, "display");
    let fg_o = fv(e, "fg_opacity", cx).clamp(0.0, 1.0);
    let bg_o = fv(e, "bg_opacity", cx).clamp(0.0, 1.0);
    let colors = pick(e, "colors");
    let (fg, bg): ([f32; 3], [f32; 3]) = match colors {
        1 | 5 => ([0.0; 3], [1.0; 3]),
        2 => ([1.0; 3], [0.5; 3]),
        3 => ([0.0; 3], [0.5; 3]),
        6 => ([1.0; 3], [0.5; 3]),
        _ => ([1.0; 3], [0.0; 3]),
    };
    let colored = colors >= 4;
    let n = comps.len();
    let slots = if display == 1 { n } else { 1 };
    let cols = if display == 2 { n } else { 1 };
    let total = [0.0, 0.0, gw * cols as f32, (lh + sh) * slots as f32];
    super::text::fill_rect(img, total, [bg[0], bg[1], bg[2], 1.0], bg_o);
    for (i, &c) in comps.iter().enumerate() {
        let (ox, oy) = match display {
            1 => (0.0, (lh + sh) * i as f32),
            2 => (gw * i as f32, 0.0),
            _ => (0.0, 0.0),
        };
        let max = bins[c].iter().cloned().fold(1.0f32, f32::max);
        let tone = if colored { COMP_COLORS[c] } else { fg };
        let b = &bins[c];
        paint(img, [ox, oy, ox + gw, oy + lh], |_, _, u, v| {
            let bin = ((u * 256.0) as usize).min(255);
            let val = if log { (1.0 + b[bin]).ln() / (1.0 + max).ln() } else { b[bin] / max };
            (1.0 - v <= val).then_some((tone, fg_o))
        });
        if sh >= 1.0 {
            paint(img, [ox, oy + lh, ox + gw, oy + lh + sh], |_, _, u, _| {
                let s = u.clamp(0.0, 1.0);
                let col = match c {
                    0 | 3 => [s, s, s],
                    1 => rgb_of(0.5, s - 0.5, 0.0, BT709),
                    _ => rgb_of(0.5, 0.0, s - 0.5, BT709),
                };
                Some((col, 1.0))
            });
        }
    }
}

// ------------------------------------------------------------------ Waveform / RGB Parade

/// Draw waveform guide lines (0, 16, 128, 235, 255 on the 8-bit scale) into a graph whose value
/// axis runs from `a` (value 0) to `b` (value 1) along y (`column`) or x.
#[allow(clippy::too_many_arguments)]
fn wave_graticule(img: &mut Image, r: [f32; 4], column: bool, mirror: bool, col: [f32; 3], k: f32, numbers: bool, scale: u32, size: f32) {
    for code in [0.0f32, 16.0, 128.0, 235.0, 255.0] {
        let t = code / 255.0;
        let t = if mirror != column { t } else { 1.0 - t };
        let txt = match scale {
            1 => format!("{:.0}", code / 255.0 * 700.0),
            2 => format!("{:.0}", code / 255.0 * 100.0),
            _ => format!("{code:.0}"),
        };
        if column {
            let y = r[1] + (r[3] - r[1]) * t;
            hline(img, r[0], r[2], y, col, k, !numbers);
            if numbers {
                label(img, &txt, r[0] + 2.0, y - size * 1.1, size, col);
            }
        } else {
            let x = r[0] + (r[2] - r[0]) * t;
            vline(img, x, r[1], r[3], col, k, !numbers);
            if numbers {
                label(img, &txt, x + 2.0, r[1] + 2.0, size, col);
            }
        }
    }
}

/// Values a waveform plots for one display pixel under a filter: (value, trace colour).
fn wave_values(filter: u32, comps: &[usize], p: [f32; 4], out: &mut Vec<(usize, f32, [f32; 3])>) {
    out.clear();
    let q = yuva(p);
    let chroma = ((q[1] - 0.5).powi(2) + (q[2] - 0.5).powi(2)).sqrt();
    match filter {
        1 => {
            out.push((0, (q[0] + chroma).min(1.0), COMP_COLORS[0]));
            out.push((0, (q[0] - chroma).max(0.0), COMP_COLORS[0]));
        }
        2 | 3 | 4 => {
            let cols = match filter {
                2 => [COMP_COLORS[0], COMP_COLORS[1], COMP_COLORS[2]],
                3 => [[0.9, 0.9, 0.3], [0.3, 0.9, 0.9], [0.9, 0.3, 0.9]],
                _ => [[0.4, 0.9, 0.4], [0.9, 0.6, 0.2], [0.6, 0.4, 0.9]],
            };
            out.push((0, q[0], cols[0]));
            out.push((1, (q[0] + q[1] - 0.5).clamp(0.0, 1.0), cols[1]));
            out.push((2, (q[0] + q[2] - 0.5).clamp(0.0, 1.0), cols[2]));
        }
        5 => {
            out.push((1, q[1], COMP_COLORS[1]));
            out.push((2, q[2], COMP_COLORS[2]));
        }
        6 => out.push((0, q[0], [p[0], p[1], p[2]])),
        7 => {
            let c = rgb_of(0.6, q[1] - 0.5, q[2] - 0.5, BT709);
            out.push((0, q[0], c));
        }
        _ => {
            for &c in comps {
                out.push((c, q[c], COMP_COLORS[c]));
            }
        }
    }
}

/// Video Waveform Monitor: each column (or row) of the picture plotted as component values, as
/// one overlaid graph, stacked graphs or a parade, over the whole frame.
pub fn waveform(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let column = pick(e, "mode") == 1;
    let mirror = flag(e, "mirror");
    let display = pick(e, "display");
    let comps: Vec<usize> = match pick(e, "components") {
        0 => vec![0],
        1 => vec![1],
        2 => vec![0, 1],
        3 => vec![2],
        4 => vec![0, 2],
        5 => vec![1, 2],
        _ => vec![0, 1, 2],
    };
    let filter = pick(e, "filter");
    let intensity = fv(e, "intensity", cx).max(0.0);
    let envelope = pick(e, "envelope");
    let (w, h) = (img.w as f32, img.h as f32);
    // the graphs: one per component slot
    let traces: Vec<usize> = if filter == 0 { comps.clone() } else { vec![0, 1, 2] };
    let nslots = if display == 0 { 1 } else { traces.len().max(1) };
    let slot_rect = |i: usize| -> [f32; 4] {
        if display == 0 {
            return [0.0, 0.0, w, h];
        }
        // stack: along the value axis; parade: along the position axis
        let along_value = display == 1;
        let f0 = i as f32 / nslots as f32;
        let f1 = (i + 1) as f32 / nslots as f32;
        if along_value == column { [0.0, h * f0, w, h * f1] } else { [w * f0, 0.0, w * f1, h] }
    };
    let (sw, sh, px) = samples(img, 720);
    let mut grids: Vec<Grid> = (0..nslots).map(|_| Grid::new(sw.max(64), 256)).collect();
    let add = |px: &[[f32; 4]], wgt: f32, grids: &mut Vec<Grid>| {
        let mut vals = Vec::with_capacity(4);
        for y in 0..sh {
            for x in 0..sw {
                let p = px[y * sw + x];
                if p[3] <= 0.0 {
                    continue;
                }
                wave_values(filter, &comps, p, &mut vals);
                let pos = if column { (x as f32 + 0.5) / sw as f32 } else { (y as f32 + 0.5) / sh as f32 };
                for &(c, v, col) in &vals {
                    let slot = if display == 0 { 0 } else { traces.iter().position(|t| *t == c).unwrap_or(0).min(nslots - 1) };
                    let v = if mirror { 1.0 - v } else { v };
                    // grid: u = position, v = value from the top (0 at the top in column mode)
                    grids[slot].hit(pos, v.clamp(0.0, 1.0), col, wgt);
                }
            }
        }
    };
    add(&px, 1.0, &mut grids);
    let mut peak: Vec<Grid> = Vec::new();
    if envelope >= 2 {
        peak = (0..nslots).map(|_| Grid::new(sw.max(64), 256)).collect();
        for prev in history(img, e, cx, 6) {
            let (pw, ph, pp) = samples(&prev, 720);
            if (pw, ph) == (sw, sh) {
                add(&pp, 1.0, &mut peak);
            }
        }
        add(&px, 1.0, &mut peak);
    }
    let k = intensity * (720.0 / sh.max(1) as f32).max(1.0);
    shade(img, [0.0, 0.0, w, h], fv(e, "bg_opacity", cx));
    for (i, g) in grids.iter().enumerate() {
        let r = slot_rect(i);
        // the grid's u is position, v is value: column mode maps them to x / y, row mode swaps
        paint(img, r, |_, _, u, v| {
            let (gu, gv) = if column { (u, v) } else { (v, u) };
            let (n, c) = g.cell(gu, gv);
            let mut out = None;
            if n > 0.0 {
                out = Some(([c[0] / n, c[1] / n, c[2] / n], (n * k).min(1.0)));
            }
            if (envelope == 1 || envelope == 3) && g.edge(gu, gv) {
                out = Some(([1.0, 1.0, 1.0], 1.0));
            }
            if envelope >= 2 && peak.get(i).is_some_and(|pg| pg.edge(gu, gv)) {
                out = Some(([1.0, 0.85, 0.2], 1.0));
            }
            out
        });
        let gc: [f32; 3] = match pick(e, "graticule") {
            1 => [0.2, 0.9, 0.3],
            2 => [1.0, 0.6, 0.1],
            3 => [1.0, 1.0, 1.0],
            _ => continue,
        };
        let go = fv(e, "graticule_opacity", cx).clamp(0.0, 1.0);
        wave_graticule(img, r, column, mirror, gc, go, pick(e, "flags") == 0, pick(e, "scale"), (h / 45.0).max(6.0));
    }
}

/// RGB Parade: R, G and B waveforms side by side over the whole frame.
pub fn rgb_parade(img: &mut Image, _e: &EffectInstance, _cx: &FxCtx) {
    let (w, h) = (img.w as f32, img.h as f32);
    let (sw, sh, px) = samples(img, 480);
    let mut grids: Vec<Grid> = (0..3).map(|_| Grid::new(sw.max(64), 256)).collect();
    for y in 0..sh {
        for x in 0..sw {
            let p = px[y * sw + x];
            if p[3] <= 0.0 {
                continue;
            }
            let u = (x as f32 + 0.5) / sw as f32;
            for c in 0..3 {
                grids[c].hit(u, 1.0 - p[c], [0.0; 3], 1.0);
            }
        }
    }
    shade(img, [0.0, 0.0, w, h], 0.7);
    let k = 0.05 * (480.0 / sh.max(1) as f32).max(1.0);
    let tones = [[1.0, 0.25, 0.25], [0.3, 1.0, 0.3], [0.35, 0.5, 1.0]];
    for (c, g) in grids.iter().enumerate() {
        let r = [w * c as f32 / 3.0, 0.0, w * (c + 1) as f32 / 3.0, h];
        let tone = tones[c];
        paint_grid(img, g, r, k, |_| tone);
        for t in [0.0, 0.5, 1.0] {
            hline(img, r[0], r[2], (h - 1.0) * t, [0.6, 0.6, 0.6], 0.5, true);
        }
    }
}

// ------------------------------------------------------------------ Vectorscopes

/// The 75 % colour-bar targets (display RGB) and their names.
const TARGETS: [([f32; 3], &str); 6] =
    [([0.75, 0.0, 0.0], "R"), ([0.75, 0.0, 0.75], "Mg"), ([0.0, 0.0, 0.75], "B"), ([0.0, 0.75, 0.75], "Cy"), ([0.0, 0.75, 0.0], "G"), ([0.75, 0.75, 0.0], "Yl")];

/// The vectorscope square: centred, the frame's height high.
fn vector_rect(img: &Image) -> [f32; 4] {
    let s = (img.h.min(img.w) as f32) * 0.9;
    let (cxp, cyp) = (img.w as f32 / 2.0, img.h as f32 / 2.0);
    [cxp - s / 2.0, cyp - s / 2.0, cxp + s / 2.0, cyp + s / 2.0]
}

/// Plot coordinates (0..1, v down) of a chroma pair (Cb, Cr centred on 0).
#[inline]
fn vec_uv(cb: f32, cr: f32) -> (f32, f32) {
    (0.5 + cb / 1.2, 0.5 - cr / 1.2)
}

/// Vectorscope: Cb / Cr of every pixel in its own colour, with the colour-bar targets.
pub fn vectorscope(img: &mut Image, _e: &EffectInstance, _cx: &FxCtx) {
    let r = vector_rect(img);
    let (_, sh, px) = samples(img, 480);
    let mut g = Grid::new(256, 256);
    for p in &px {
        if p[3] <= 0.0 {
            continue;
        }
        let q = ycc(*p, BT709.0, BT709.1);
        let (u, v) = vec_uv(q[1], q[2]);
        g.hit(u, v, [p[0], p[1], p[2]], 1.0);
    }
    shade(img, r, 0.75);
    let k = 0.02 * (270.0 / sh.max(1) as f32).max(1.0);
    paint_grid(img, &g, r, k, |c| c);
    vector_graticule(img, r, BT709, Some([0.2, 0.9, 0.3]), 0.8, 2);
}

/// Targets, the skin-tone line and the outer circle; `col` None = each target in its colour.
fn vector_graticule(img: &mut Image, r: [f32; 4], m: (f32, f32), col: Option<[f32; 3]>, k: f32, flags: u32) {
    let (rw, rh) = (r[2] - r[0], r[3] - r[1]);
    let at = |u: f32, v: f32| (r[0] + u * rw, r[1] + v * rh);
    let ring = col.unwrap_or([0.7, 0.7, 0.7]);
    // outer circle (|chroma| = 0.5) and the centre cross
    let n = 96;
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let (x, y) = at(0.5 + a.cos() * 0.5 / 1.2, 0.5 + a.sin() * 0.5 / 1.2);
        let (xi, yi) = (x as isize, y as isize);
        if xi >= 0 && yi >= 0 && (xi as usize) < img.w && (yi as usize) < img.h {
            let j = (yi as usize * img.w + xi as usize) * 4;
            put(&mut img.px[j..j + 4], ring, k);
        }
    }
    let (cxp, cyp) = at(0.5, 0.5);
    hline(img, cxp - rw * 0.02, cxp + rw * 0.02, cyp, ring, k, false);
    vline(img, cxp, cyp - rh * 0.02, cyp + rh * 0.02, ring, k, false);
    let size = (rh / 28.0).max(6.0);
    for (c, name) in TARGETS {
        let q = ycc([c[0], c[1], c[2], 1.0], m.0, m.1);
        let (u, v) = vec_uv(q[1], q[2]);
        let (x, y) = at(u, v);
        let tc = col.unwrap_or(c.map(|v| (v / 0.75).min(1.0)));
        let s = rw * 0.02;
        hline(img, x - s, x + s, y - s, tc, k, false);
        hline(img, x - s, x + s, y + s, tc, k, false);
        vline(img, x - s, y - s, y + s, tc, k, false);
        vline(img, x + s, y - s, y + s, tc, k, false);
        if flags == 3 {
            label(img, name, x + s * 1.5, y - s * 2.0, size, tc);
        }
    }
    if flags == 1 || flags == 2 {
        // the white (or black) point sits at the centre of the chroma plane
        let pc = if flags == 1 { [1.0, 1.0, 1.0] } else { [0.0, 0.0, 0.0] };
        let s = rw * 0.01;
        super::text::fill_rect(img, [cxp - s, cyp - s, cxp + s, cyp + s], [pc[0], pc[1], pc[2], 1.0], 1.0);
    }
}

/// Vectorscope (advanced): any two of Y / U / V on the axes, seven colouring modes, low / high
/// thresholds on the third component, instant / peak envelopes, graticule styles and flags.
pub fn vectorscope_adv(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let r = vector_rect(img);
    let m = match pick(e, "space") {
        1 => BT601,
        _ => BT709,
    };
    let (xa, ya) = (pick(e, "x_axis").min(2) as usize, pick(e, "y_axis").min(2) as usize);
    let third = (0..3).find(|i| *i != xa && *i != ya).unwrap_or(0);
    let (lo, hi) = (fv(e, "low", cx), fv(e, "high", cx));
    let mode = pick(e, "mode");
    let comp = |p: &[f32; 4]| {
        let q = ycc(*p, m.0, m.1);
        [q[0], q[1] + 0.5, q[2] + 0.5]
    };
    let fill = |g: &mut Grid, px: &[[f32; 4]]| {
        for p in px {
            if p[3] <= 0.0 {
                continue;
            }
            let q = comp(p);
            if q[third] < lo || q[third] > hi {
                continue;
            }
            g.hit(q[xa], 1.0 - q[ya], [p[0], p[1], p[2]], 1.0);
        }
    };
    let (_, sh, px) = samples(img, 480);
    let mut g = Grid::new(256, 256);
    fill(&mut g, &px);
    let envelope = pick(e, "envelope");
    let mut peak = Grid::new(256, 256);
    if envelope >= 2 {
        for prev in history(img, e, cx, 6) {
            let (_, _, pp) = samples(&prev, 480);
            fill(&mut peak, &pp);
        }
        fill(&mut peak, &px);
    }
    shade(img, r, fv(e, "bg_opacity", cx));
    let intensity = fv(e, "intensity", cx).max(0.0) * (270.0 / sh.max(1) as f32).max(1.0) * 4.0;
    let (t1, t2) = (fv(e, "tint1", cx), fv(e, "tint2", cx));
    // the colour a cell stands for: Y = 0.5 with the plotted pair
    let cell_rgb = |u: f32, v: f32| -> [f32; 3] {
        let mut q = [0.5f32, 0.5, 0.5];
        q[xa] = u;
        q[ya] = 1.0 - v;
        rgb_of(q[0], q[1] - 0.5, q[2] - 0.5, m)
    };
    paint(img, r, |_, _, u, v| {
        let (n, c) = g.cell(u, v);
        let mut out = None;
        if n > 0.0 {
            let lvl = (n * intensity).min(1.0);
            let avg = [c[0] / n, c[1] / n, c[2] / n];
            out = Some(match mode {
                0 => ([1.0; 3], lvl),
                1 => (rgb_of(0.7, t1 * 0.5, t2 * 0.5, m), lvl),
                2 => (cell_rgb(u, v), 1.0),
                3 => (cell_rgb(u, v), lvl),
                5 => (avg, 1.0),
                6 => (cell_rgb(u, v).map(|x| x * (0.4 + 0.6 * lvl)), 1.0),
                _ => (avg, lvl),
            });
        }
        if (envelope == 1 || envelope == 3) && g.edge(u, v) {
            out = Some(([1.0; 3], 1.0));
        }
        if envelope >= 2 && peak.edge(u, v) {
            out = Some(([1.0, 0.85, 0.2], 1.0));
        }
        out
    });
    let go = fv(e, "graticule_opacity", cx);
    match pick(e, "graticule") {
        1 => vector_graticule(img, r, m, Some([0.2, 0.9, 0.3]), go, pick(e, "flags")),
        2 => vector_graticule(img, r, m, None, go, pick(e, "flags")),
        _ => {}
    }
}

// ------------------------------------------------------------------ CIE Scope

/// CIE 1931 2° spectral locus chromaticities (x, y), 380–700 nm.
const LOCUS: [(f32, f32); 32] = [
    (0.1741, 0.0050),
    (0.1738, 0.0049),
    (0.1733, 0.0048),
    (0.1726, 0.0048),
    (0.1714, 0.0051),
    (0.1689, 0.0069),
    (0.1644, 0.0109),
    (0.1566, 0.0177),
    (0.1440, 0.0297),
    (0.1241, 0.0578),
    (0.0913, 0.1327),
    (0.0454, 0.2950),
    (0.0082, 0.5384),
    (0.0139, 0.7502),
    (0.0743, 0.8338),
    (0.1547, 0.8059),
    (0.2296, 0.7543),
    (0.3016, 0.6923),
    (0.3731, 0.6245),
    (0.4441, 0.5547),
    (0.5125, 0.4866),
    (0.5752, 0.4242),
    (0.6270, 0.3725),
    (0.6658, 0.3340),
    (0.6915, 0.3083),
    (0.7079, 0.2920),
    (0.7190, 0.2809),
    (0.7260, 0.2740),
    (0.7300, 0.2700),
    (0.7320, 0.2680),
    (0.7334, 0.2666),
    (0.7347, 0.2653),
];

/// Primaries (x, y of R, G, B) and white point of the CIE Scope's colour systems
/// ([`redrob_film_project::effect::CIE_SYSTEMS`]).
fn cie_system(i: u32) -> ([(f32, f32); 3], (f32, f32)) {
    let d65 = (0.3127, 0.3290);
    match i {
        0 => ([(0.67, 0.33), (0.21, 0.71), (0.14, 0.08)], (0.3101, 0.3162)),
        1 => ([(0.64, 0.33), (0.29, 0.60), (0.15, 0.06)], d65),
        2 => ([(0.630, 0.340), (0.310, 0.595), (0.155, 0.070)], d65),
        3 => ([(0.625, 0.340), (0.280, 0.595), (0.155, 0.070)], d65),
        4 => ([(0.735, 0.265), (0.115, 0.826), (0.157, 0.018)], (0.3457, 0.3585)),
        5 => ([(0.7347, 0.2653), (0.2738, 0.7174), (0.1666, 0.0089)], (0.3333, 0.3333)),
        7 => ([(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)], d65),
        8 => ([(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)], (0.314, 0.351)),
        _ => ([(0.64, 0.33), (0.30, 0.60), (0.15, 0.06)], d65),
    }
}

/// RGB → XYZ matrix of primaries and white.
fn rgb_to_xyz(p: [(f32, f32); 3], wp: (f32, f32)) -> [[f32; 3]; 3] {
    let col = |(x, y): (f32, f32)| [x / y, 1.0, (1.0 - x - y) / y];
    let (r, g, b) = (col(p[0]), col(p[1]), col(p[2]));
    let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let w = col(wp);
    // solve m · s = w
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let det = if det.abs() < 1e-9 { 1e-9 } else { det };
    let inv = [
        [(m[1][1] * m[2][2] - m[1][2] * m[2][1]) / det, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / det, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / det],
        [(m[1][2] * m[2][0] - m[1][0] * m[2][2]) / det, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / det, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / det],
        [(m[1][0] * m[2][1] - m[1][1] * m[2][0]) / det, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / det, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / det],
    ];
    let s = [0, 1, 2].map(|i| inv[i][0] * w[0] + inv[i][1] * w[1] + inv[i][2] * w[2]);
    [[m[0][0] * s[0], m[0][1] * s[1], m[0][2] * s[2]], [m[1][0] * s[0], m[1][1] * s[1], m[1][2] * s[2]], [m[2][0] * s[0], m[2][1] * s[1], m[2][2] * s[2]]]
}

/// xy → the plotted coordinates of a CIE system (xyY, 1960 UCS u v, 1976 u′ v′) and their range.
fn cie_coords(space: u32, x: f32, y: f32) -> (f32, f32) {
    let d = -2.0 * x + 12.0 * y + 3.0;
    let d = if d.abs() < 1e-6 { 1e-6 } else { d };
    match space {
        1 => (4.0 * x / d, 6.0 * y / d),
        2 => (4.0 * x / d, 9.0 * y / d),
        _ => (x, y),
    }
}
fn cie_range(space: u32) -> (f32, f32) {
    match space {
        1 => (0.7, 0.4),
        2 => (0.7, 0.65),
        _ => (0.8, 0.9),
    }
}

/// Whether a plotted point is inside the polygon `poly`.
fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
    }
    c
}

/// CIE Scope: the chromaticity diagram (filled with its colours) with the picture's pixels on
/// it, the chosen gamut's triangle and white point, in xyY, UCS or LUV coordinates.
pub fn cie_scope(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let size = (fv(e, "size", cx) * cx.px_scale).min(img.w.min(img.h) as f32).max(16.0);
    let r = [0.0, 0.0, size, size];
    let space = pick(e, "cie");
    let (rx, ry) = cie_range(space);
    let (prim, wp) = cie_system(pick(e, "system"));
    let to_xyz = rgb_to_xyz(prim, wp);
    let gamma = fv(e, "gamma", cx).max(0.1);
    let correct = flag(e, "correct_gamma");
    let contrast = fv(e, "contrast", cx).clamp(0.0, 1.0);
    let intensity = fv(e, "intensity", cx).max(0.0);
    let locus: Vec<(f32, f32)> = LOCUS.iter().map(|&(x, y)| cie_coords(space, x, y)).collect();
    let (_, sh, px) = samples(img, 400);
    let mut g = Grid::new(size.round() as usize, size.round() as usize);
    for p in &px {
        if p[3] <= 0.0 {
            continue;
        }
        // display values → linear with the input gamma
        let l = [p[0].powf(gamma), p[1].powf(gamma), p[2].powf(gamma)];
        let xyz = [0, 1, 2].map(|i| to_xyz[i][0] * l[0] + to_xyz[i][1] * l[1] + to_xyz[i][2] * l[2]);
        let s = xyz[0] + xyz[1] + xyz[2];
        if s <= 1e-6 {
            continue;
        }
        let (u, v) = cie_coords(space, xyz[0] / s, xyz[1] / s);
        g.hit(u / rx, 1.0 - v / ry, [1.0; 3], 1.0);
    }
    super::text::fill_rect(img, r, [0.0, 0.0, 0.0, 1.0], 1.0);
    let fill_cie = flag(e, "fill");
    // a plotted point back to an xy chromaticity (for the tongue colours)
    let xy_of = |u: f32, v: f32| -> (f32, f32) {
        match space {
            1 | 2 => {
                let vv = if space == 1 { v } else { v * 2.0 / 3.0 };
                let d = (2.0 * u - 8.0 * vv + 4.0).max(1e-6);
                (3.0 * u / d, 2.0 * vv / d)
            }
            _ => (u, v),
        }
    };
    let gain = intensity * (300.0 / sh.max(1) as f32).max(1.0) * 20.0;
    paint(img, r, |_, _, u, v| {
        let (pu, pv) = (u * rx, (1.0 - v) * ry);
        let (n, _) = g.cell(u, v);
        if n > 0.0 {
            return Some(([1.0, 1.0, 1.0], (n * gain).min(1.0).max(0.35)));
        }
        if !fill_cie || !inside(&locus, pu, pv) {
            return None;
        }
        let (x, y) = xy_of(pu, pv);
        let yy = 1.0;
        let xyz = [x / y.max(1e-4) * yy, yy, (1.0 - x - y) / y.max(1e-4) * yy];
        // XYZ → linear Rec.709, normalised to its largest channel
        let rgb = [
            3.2406 * xyz[0] - 1.5372 * xyz[1] - 0.4986 * xyz[2],
            -0.9689 * xyz[0] + 1.8758 * xyz[1] + 0.0415 * xyz[2],
            0.0557 * xyz[0] - 0.2040 * xyz[1] + 1.0570 * xyz[2],
        ];
        let out_of_gamut = rgb.iter().any(|c| *c < 0.0);
        let rgb = rgb.map(|c| c.max(0.0));
        let mx = rgb[0].max(rgb[1]).max(rgb[2]).max(1e-6);
        let lin = rgb.map(|c| c / mx);
        let disp = if correct { crate::effects::enc(lin) } else { lin };
        let k = if out_of_gamut { contrast } else { 1.0 };
        Some((disp.map(|c| c * k * 0.8), 1.0))
    });
    // the locus outline
    let at = |(u, v): (f32, f32)| redrob_film_geom::Vec2::new((u / rx * size) as f64, ((1.0 - v / ry) * size) as f64);
    let line = crate::graphics::premul_linear([0.8, 0.8, 0.8, 1.0]);
    for i in 0..locus.len() {
        super::alpha::draw_segment(img, at(locus[i]), at(locus[(i + 1) % locus.len()]), 1.0, line);
    }
    let gi = pick(e, "gamuts");
    if gi > 0 {
        let (gp, _) = cie_system(gi - 1);
        let tri: Vec<(f32, f32)> = gp.iter().map(|&(x, y)| cie_coords(space, x, y)).collect();
        let tc = crate::graphics::premul_linear([1.0, 1.0, 1.0, 1.0]);
        for i in 0..3 {
            super::alpha::draw_segment(img, at(tri[i]), at(tri[(i + 1) % 3]), 1.5, tc);
        }
    }
    if flag(e, "white_point") {
        let (u, v) = cie_coords(space, wp.0, wp.1);
        let c = at((u, v));
        let s = (size / 100.0).max(2.0);
        super::text::fill_rect(img, [c.x as f32 - s, c.y as f32 - s, c.x as f32 + s, c.y as f32 + s], [1.0, 1.0, 1.0, 1.0], 1.0);
    }
}

// ------------------------------------------------------------------ Data Scope

/// 3 × 5 bitmaps of the digits and A–F, one row per 3 bits (MSB = left).
const GLYPHS: [[u8; 5]; 16] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 1, 1, 1],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
    [7, 5, 7, 5, 5],
    [6, 5, 6, 5, 6],
    [7, 4, 4, 4, 7],
    [6, 5, 5, 5, 6],
    [7, 4, 7, 4, 7],
    [7, 4, 7, 4, 4],
];

/// Draw hexadecimal / decimal digits with the bitmap font, `s` pixels per font pixel.
fn digits(img: &mut Image, text: &str, x: f32, y: f32, s: f32, col: [f32; 3]) {
    let mut cx0 = x;
    for ch in text.chars() {
        if let Some(d) = ch.to_digit(16) {
            for (ry, bits) in GLYPHS[d as usize].iter().enumerate() {
                for rx in 0..3 {
                    if bits & (4 >> rx) != 0 {
                        let (px0, py0) = (cx0 + rx as f32 * s, y + ry as f32 * s);
                        super::text::fill_rect(img, [px0, py0, px0 + s, py0 + s], [col[0], col[1], col[2], 1.0], 1.0);
                    }
                }
            }
        }
        cx0 += 4.0 * s;
    }
}

/// Data Scope: a grid of the pixel values (hex or decimal) of the area at the offset, one cell per
/// pixel, mono, in the pixel's colour, or on the pixel's colour; optional row / column numbers.
pub fn datascope(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let vh = [128.0f32, 360.0, 576.0, 480.0, 480.0, 720.0, 1080.0, 1080.0, 2160.0][pick(e, "size").min(8) as usize];
    let comps = plane_list(pick(e, "components"));
    if comps.is_empty() {
        return;
    }
    let dec = pick(e, "format") == 1;
    let chars = if dec { 3 } else { 2 };
    let s = (img.h as f32 / vh * 2.0).max(1.0).round();
    let cell_w = (chars as f32 * 4.0 + 2.0) * s;
    let cell_h = (comps.len() as f32 * 6.0 + 2.0) * s;
    let axis = flag(e, "axis");
    let (ax, ay) = if axis { (cell_w, 6.0 * s + 2.0 * s) } else { (0.0, 0.0) };
    let ncols = ((img.w as f32 - ax) / cell_w).floor().max(0.0) as usize;
    let nrows = ((img.h as f32 - ay) / cell_h).floor().max(0.0) as usize;
    let k = cx.px_scale.max(1e-3);
    let (ox, oy) = ((fv(e, "x", cx) * k).round().max(0.0) as usize, (fv(e, "y", cx) * k).round().max(0.0) as usize);
    let src = Disp::of(img);
    let opacity = fv(e, "opacity", cx).clamp(0.0, 1.0);
    let mode = pick(e, "mode");
    super::text::fill_rect(img, [0.0, 0.0, img.w as f32, img.h as f32], [0.0, 0.0, 0.0, 1.0], opacity);
    let fmt = |v: f32| if dec { format!("{:03}", (v.clamp(0.0, 1.0) * 255.0).round() as u32) } else { format!("{:02X}", (v.clamp(0.0, 1.0) * 255.0).round() as u32) };
    for row in 0..nrows {
        let sy = oy + row;
        if sy >= src.h {
            break;
        }
        if axis {
            digits(img, &format!("{sy:03}"), s, ay + row as f32 * cell_h + s, s * 0.7, [0.7, 0.7, 0.7]);
        }
        for col in 0..ncols {
            let sx = ox + col;
            if sx >= src.w {
                break;
            }
            let p = src.get(sx as isize, sy as isize);
            let q = yuva(p);
            let (x0, y0) = (ax + col as f32 * cell_w, ay + row as f32 * cell_h);
            let text_col = match mode {
                1 => [p[0], p[1], p[2]],
                2 => {
                    super::text::fill_rect(img, [x0, y0, x0 + cell_w, y0 + cell_h], [p[0], p[1], p[2], 1.0], 1.0);
                    if q[0] > 0.5 { [0.0; 3] } else { [1.0; 3] }
                }
                _ => [1.0; 3],
            };
            for (li, &c) in comps.iter().enumerate() {
                digits(img, &fmt(q[c]), x0 + s, y0 + s + li as f32 * 6.0 * s, s, text_col);
            }
        }
    }
    if axis {
        for col in 0..ncols {
            digits(img, &format!("{:03}", ox + col), ax + col as f32 * cell_w + s, s, s * 0.7, [0.7, 0.7, 0.7]);
        }
    }
}

// ------------------------------------------------------------------ Oscilloscopes

/// `n` samples along a line through (`cxp`, `cyp`) of `len` pixels at `tilt` degrees.
fn line_samples(img: &Image, cxp: f32, cyp: f32, len: f32, tilt: f32, n: usize) -> (Vec<[f32; 4]>, [f32; 4]) {
    let d = Disp::of(img);
    let (s, c) = tilt.to_radians().sin_cos();
    let (x0, y0) = (cxp - c * len / 2.0, cyp - s * len / 2.0);
    let (x1, y1) = (cxp + c * len / 2.0, cyp + s * len / 2.0);
    let v = (0..n)
        .map(|i| {
            let t = (i as f32 + 0.5) / n as f32;
            d.sample(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t)
        })
        .collect();
    (v, [x0, y0, x1, y1])
}

/// Draw value traces (0..1 each, one per series) into `r`.
fn traces(img: &mut Image, r: [f32; 4], series: &[(Vec<f32>, [f32; 3])], width: f32, k: f32) {
    for (vals, col) in series {
        let n = vals.len();
        if n < 2 {
            continue;
        }
        let c = crate::graphics::premul_linear([col[0], col[1], col[2], k]);
        let at = |i: usize| redrob_film_geom::Vec2::new((r[0] + (r[2] - r[0]) * i as f32 / (n - 1) as f32) as f64, (r[3] - (r[3] - r[1]) * vals[i].clamp(0.0, 1.0)) as f64);
        for i in 0..n - 1 {
            super::alpha::draw_segment(img, at(i), at(i + 1), width, c);
        }
    }
}

/// Oscilloscope (simple): the chosen components along a trace line, plotted in a box, with an
/// optional grid, the trace line drawn on the picture and min / average / max statistics.
pub fn oscilloscope(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let comps = plane_list(pick(e, "components"));
    let (w, h) = (img.w as f32, img.h as f32);
    let diag = (w * w + h * h).sqrt();
    let len = fv(e, "size", cx) / 100.0 * diag;
    let (tx, ty) = (fv(e, "trace_x", cx) / 100.0 * w, fv(e, "trace_y", cx) / 100.0 * h);
    let n = (len.round() as usize).clamp(8, 1024);
    let (pts, line) = line_samples(img, tx, ty, len, fv(e, "tilt", cx), n);
    let op = (fv(e, "opacity", cx) / 100.0).clamp(0.0, 1.0);
    let (sw, sh) = (fv(e, "scope_w", cx) / 100.0 * w, fv(e, "scope_h", cx) / 100.0 * h);
    let (sx, sy) = (fv(e, "scope_x", cx) / 100.0 * w, fv(e, "scope_y", cx) / 100.0 * h);
    let r = [sx - sw / 2.0, sy - sh / 2.0, sx + sw / 2.0, sy + sh / 2.0];
    if flag(e, "trace_line") {
        let c = crate::graphics::premul_linear([1.0, 1.0, 1.0, 0.8]);
        super::alpha::draw_segment(img, redrob_film_geom::Vec2::new(line[0] as f64, line[1] as f64), redrob_film_geom::Vec2::new(line[2] as f64, line[3] as f64), 1.0, c);
    }
    shade(img, r, op * 0.8);
    if flag(e, "grid") {
        for i in 0..=8 {
            vline(img, r[0] + (r[2] - r[0]) * i as f32 / 8.0, r[1], r[3], [0.5, 0.5, 0.5], op * 0.6, true);
        }
        for i in 0..=4 {
            hline(img, r[0], r[2], r[1] + (r[3] - r[1]) * i as f32 / 4.0, [0.5, 0.5, 0.5], op * 0.6, true);
        }
    }
    let series: Vec<(Vec<f32>, [f32; 3])> = comps.iter().map(|&c| (pts.iter().map(|p| yuva(*p)[c]).collect(), COMP_COLORS[c])).collect();
    traces(img, r, &series, (h / 400.0).max(1.0), op);
    if flag(e, "statistics") {
        let names = ["Y", "U", "V", "A"];
        let size = (h / 40.0).max(6.0);
        for (i, (&c, (vals, col))) in comps.iter().zip(&series).enumerate() {
            let (mn, mx) = vals.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
            let avg = vals.iter().sum::<f32>() / vals.len().max(1) as f32;
            let t = format!("{} min {:.0} avg {:.0} max {:.0}", names[c], mn * 255.0, avg * 255.0, mx * 255.0);
            label(img, &t, r[0] + 4.0, r[1] + 2.0 + i as f32 * size * 1.3, size, *col);
        }
    }
}

/// Oscilloscope (advanced): R / G / B, Y′ / Pr / Pb and alpha traces along the profile line,
/// two markers, and the chosen channel's values (markers, average, RMS, min / max) underneath.
pub fn oscilloscope_adv(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (w, h) = (img.w as f32, img.h as f32);
    let diag = (w * w + h * h).sqrt();
    let len = fv(e, "length", cx) / 100.0 * diag;
    let (lx, ly) = (fv(e, "x", cx) / 100.0 * w, fv(e, "y", cx) / 100.0 * h);
    let n = (len.round() as usize).clamp(8, 1024);
    let (pts, line) = line_samples(img, lx, ly, len, fv(e, "tilt", cx), n);
    let (kr, kb) = if pick(e, "space") == 0 { BT601 } else { BT709 };
    let ypp = |p: &[f32; 4]| {
        let q = ycc(*p, kr, kb);
        [q[0], q[2] + 0.5, q[1] + 0.5]
    };
    let ch = pick(e, "channel") as usize;
    let chan: Vec<f32> = pts
        .iter()
        .map(|p| match ch {
            0..=2 => p[ch],
            3..=5 => ypp(p)[ch - 3],
            _ => p[3],
        })
        .collect();
    let cross = color(e, "crosshair", cx);
    let ccol = crate::graphics::premul_linear(cross);
    let pa = redrob_film_geom::Vec2::new(line[0] as f64, line[1] as f64);
    let pb = redrob_film_geom::Vec2::new(line[2] as f64, line[3] as f64);
    super::alpha::draw_segment(img, pa, pb, 1.0, ccol);
    let marks = [fv(e, "marker1", cx) / 100.0, fv(e, "marker2", cx) / 100.0];
    for m in marks {
        let p = pa.lerp(pb, m.clamp(0.0, 1.0) as f64);
        super::text::fill_rect(img, [p.x as f32 - 3.0, p.y as f32 - 3.0, p.x as f32 + 3.0, p.y as f32 + 3.0], cross, 1.0);
    }
    // the scope: the lower third of the frame
    let r = [w * 0.05, h * 0.6, w * 0.95, h * 0.9];
    shade(img, [r[0], r[1], r[2], h * 0.97], 0.75);
    let mut series = Vec::new();
    if flag(e, "rgb") {
        series.push((pts.iter().map(|p| p[0]).collect::<Vec<f32>>(), [1.0, 0.3, 0.3]));
        series.push((pts.iter().map(|p| p[1]).collect(), [0.3, 1.0, 0.3]));
        series.push((pts.iter().map(|p| p[2]).collect(), [0.4, 0.5, 1.0]));
    }
    if flag(e, "ypp") {
        series.push((pts.iter().map(|p| ypp(p)[0]).collect(), [0.9, 0.9, 0.9]));
        series.push((pts.iter().map(|p| ypp(p)[1]).collect(), [1.0, 0.5, 0.7]));
        series.push((pts.iter().map(|p| ypp(p)[2]).collect(), [0.5, 0.7, 1.0]));
    }
    if flag(e, "alpha") {
        series.push((pts.iter().map(|p| p[3]).collect(), [0.6, 0.6, 0.6]));
    }
    traces(img, r, &series, (h / 400.0).max(1.0), 1.0);
    for m in marks {
        vline(img, r[0] + (r[2] - r[0]) * m.clamp(0.0, 1.0), r[1], r[3], [cross[0], cross[1], cross[2]], 0.9, false);
    }
    let s256 = flag(e, "scale256");
    let fmtv = |v: f32| if s256 { format!("{:.0}", v * 255.0) } else { format!("{v:.3}") };
    let at = |m: f32| chan[((m.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize).min(n - 1)];
    let names = ["R", "G", "B", "Y'", "Pr", "Pb", "Alpha"];
    let mut text = format!("{}  M1 {}  M2 {}", names[ch.min(6)], fmtv(at(marks[0])), fmtv(at(marks[1])));
    if flag(e, "average") {
        text += &format!("  avg {}", fmtv(chan.iter().sum::<f32>() / n as f32));
    }
    if flag(e, "rms") {
        text += &format!("  rms {}", fmtv((chan.iter().map(|v| v * v).sum::<f32>() / n as f32).sqrt()));
    }
    if flag(e, "minmax") {
        let (mn, mx) = chan.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        text += &format!("  min {}  max {}", fmtv(mn), fmtv(mx));
    }
    label(img, &text, r[0], h * 0.91, (h / 32.0).max(6.0), [1.0, 1.0, 1.0]);
}

// ------------------------------------------------------------------ Video Values

/// Video Values: a crosshair box at the position; a window shows the area zoomed and its average
/// in RGB, Y′PbPr (Rec. 601 / 709), HSV or HSL, on a 0–1 or 0–255 scale, with alpha if asked.
pub fn video_values(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (w, h) = (img.w as f32, img.h as f32);
    let k = cx.px_scale.max(1e-3);
    let (sx, sy) = ((fv(e, "size_x", cx) * k).max(1.0), (fv(e, "size_y", cx) * k).max(1.0));
    let (px, py) = (fv(e, "x", cx) / 100.0 * w, fv(e, "y", cx) / 100.0 * h);
    let area = [(px - sx / 2.0).max(0.0), (py - sy / 2.0).max(0.0), (px + sx / 2.0).min(w), (py + sy / 2.0).min(h)];
    let d = Disp::of(img);
    let mut acc = [0.0f32; 4];
    let mut cnt = 0.0f32;
    for y in (area[1] as usize)..(area[3].ceil() as usize).min(d.h) {
        for x in (area[0] as usize)..(area[2].ceil() as usize).min(d.w) {
            let p = d.px[y * d.w + x];
            for c in 0..4 {
                acc[c] += p[c];
            }
            cnt += 1.0;
        }
    }
    if cnt <= 0.0 {
        return;
    }
    let avg = acc.map(|v| v / cnt);
    let big = flag(e, "big");
    let win = if big { h * 0.45 } else { h * 0.25 };
    let wr = [w * 0.02, h * 0.02, w * 0.02 + win, h * 0.02 + win * 1.35];
    // the zoomed area
    super::text::fill_rect(img, wr, [0.0, 0.0, 0.0, 1.0], 0.85);
    let zoom = [wr[0] + win * 0.05, wr[1] + win * 0.05, wr[2] - win * 0.05, wr[1] + win * 0.95];
    let src = d;
    paint(img, zoom, |_, _, u, v| {
        let p = src.get((area[0] + u * (area[2] - area[0])) as isize, (area[1] + v * (area[3] - area[1])) as isize);
        Some(([p[0], p[1], p[2]], 1.0))
    });
    let cc = [1.0, 0.2, 0.2];
    hline(img, area[0], area[2], area[1], cc, 1.0, false);
    hline(img, area[0], area[2], area[3], cc, 1.0, false);
    vline(img, area[0], area[1], area[3], cc, 1.0, false);
    vline(img, area[2], area[1], area[3], cc, 1.0, false);
    let s256 = flag(e, "scale256");
    let f = |v: f32| if s256 { format!("{:.0}", v * 255.0) } else { format!("{v:.3}") };
    let (names, vals): ([&str; 3], [f32; 3]) = match pick(e, "measurement") {
        1 | 2 => {
            let (kr, kb) = if pick(e, "measurement") == 1 { BT601 } else { BT709 };
            let q = ycc(avg, kr, kb);
            (["Y'", "Pb", "Pr"], [q[0], q[1], q[2]])
        }
        3 => {
            let (hh, s, v) = super::alpha::hsv([avg[0], avg[1], avg[2]]);
            (["H", "S", "V"], [hh, s, v])
        }
        4 => {
            let (hh, _, _) = super::alpha::hsv([avg[0], avg[1], avg[2]]);
            let mx = avg[0].max(avg[1]).max(avg[2]);
            let mn = avg[0].min(avg[1]).min(avg[2]);
            let l = (mx + mn) / 2.0;
            let s = if mx - mn <= 1e-6 { 0.0 } else { (mx - mn) / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6) };
            (["H", "S", "L"], [hh, s, l])
        }
        _ => (["R", "G", "B"], [avg[0], avg[1], avg[2]]),
    };
    let size = (win / 14.0).max(6.0);
    let mut lines: Vec<String> = names.iter().zip(vals).map(|(n, v)| format!("{n} {}", f(v))).collect();
    if flag(e, "show_alpha") {
        lines.push(format!("A {}", f(avg[3])));
    }
    for (i, l) in lines.iter().enumerate() {
        label(img, l, wr[0] + win * 0.06, wr[1] + win * 0.98 + i as f32 * size * 1.0, size * 0.8, [1.0, 1.0, 1.0]);
    }
}

// ------------------------------------------------------------------ Timeout Indicator

/// Timeout Indicator: a bar in the bottom right corner, full at Time 0 and gone at 1000 (animate
/// Time with keyframes for a countdown).
pub fn timeout(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let frac = (1.0 - fv(e, "time", cx) / 1000.0).clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let (w, h) = (img.w as f32, img.h as f32);
    let (bw, bh, m) = (w / 6.0, (h / 30.0).max(2.0), h / 40.0);
    let x1 = w - m;
    let r = [x1 - bw * frac, h - m - bh, x1, h - m];
    let c = color(e, "color", cx);
    super::text::fill_rect(img, r, [c[0], c[1], c[2], 1.0], 1.0 - (fv(e, "transparency", cx) / 100.0).clamp(0.0, 1.0));
}

// ------------------------------------------------------------------ Nikon D90 fix

/// Nikon D90 Stairstepping Fix: the camera's 720p lines come from sensor lines with one line
/// skipped about every 8.5 lines, which shows as steps on sharp diagonals; each output row is
/// re-sampled at the sensor position a uniform line spacing would have used, interpolated
/// between the two stored rows that bracket it.
pub fn d90_fix(img: &mut Image) {
    let h = img.h;
    if h < 16 {
        return;
    }
    let period = 8.5 * h as f64 / 720.0;
    // the sensor line each stored row came from
    let sensor = |r: f64| r + ((r + 0.5) / period).floor();
    let total = sensor((h - 1) as f64);
    let src = img.clone();
    let w = img.w;
    let mut lut = Vec::with_capacity(h);
    let mut r = 0usize;
    for y in 0..h {
        let u = y as f64 * total / (h - 1) as f64;
        while r + 1 < h - 1 && sensor((r + 1) as f64) < u {
            r += 1;
        }
        let (s0, s1) = (sensor(r as f64), sensor((r + 1).min(h - 1) as f64));
        let t = if s1 > s0 { ((u - s0) / (s1 - s0)).clamp(0.0, 1.0) } else { 0.0 };
        lut.push((r, (r + 1).min(h - 1), t as f32));
    }
    img.px.par_chunks_mut(w * 4).zip(lut.par_iter()).for_each(|(row, &(a, b, t))| {
        let ra = &src.px[a * w * 4..(a + 1) * w * 4];
        let rb = &src.px[b * w * 4..(b + 1) * w * 4];
        for i in 0..w * 4 {
            row[i] = ra[i] + (rb[i] - ra[i]) * t;
        }
    });
}
