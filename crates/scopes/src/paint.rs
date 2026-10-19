//! Count grids → pixels: the trace images the UI uploads as textures.
//!
//! A cell's intensity grows with the logarithm of its count relative to a reference count (a
//! "full" cell: a flat colour puts a whole column of samples on one waveform cell), so sparse
//! detail stays visible next to flat areas, like a phosphor trace. Traces of several grids add
//! up (R + G + B traces that coincide read white).

use crate::{Grid, Vectorscope};

/// Intensity 0..1 of a cell holding `count` samples (`reference` = a full cell, `gain` = the
/// Brightness setting).
#[inline]
pub fn intensity(count: u32, reference: f32, gain: f32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let r = (1.0 + reference.max(1.0)).ln();
    (gain * (0.12 + 0.88 * (1.0 + count as f32).ln() / r)).clamp(0.0, 1.0)
}

/// Paint grids of equal size with colours (0..1 RGB), additively, into premultiplied RGBA8 with the
/// highest row at the top. Empty cells are transparent.
pub fn grids(layers: &[(&Grid, [f32; 3])], reference: f32, gain: f32) -> (usize, usize, Vec<u8>) {
    let Some((first, _)) = layers.first() else { return (0, 0, Vec::new()) };
    let (w, h) = (first.cols, first.rows);
    let mut out = vec![0u8; w * h * 4];
    let mut acc = vec![[0f32; 3]; w * h];
    for (g, col) in layers {
        if g.cols != w || g.rows != h {
            continue;
        }
        for row in 0..h {
            let dst = (h - 1 - row) * w;
            for x in 0..w {
                let c = g.data[row * w + x];
                if c > 0 {
                    let i = intensity(c, reference, gain);
                    let a = &mut acc[dst + x];
                    for k in 0..3 {
                        a[k] += col[k] * i;
                    }
                }
            }
        }
    }
    for (o, a) in out.as_chunks_mut::<4>().0.iter_mut().zip(&acc) {
        let m = a[0].max(a[1]).max(a[2]);
        if m <= 0.0 {
            continue;
        }
        o[0] = (a[0].min(1.0) * 255.0).round() as u8;
        o[1] = (a[1].min(1.0) * 255.0).round() as u8;
        o[2] = (a[2].min(1.0) * 255.0).round() as u8;
        o[3] = (m.min(1.0) * 255.0).round() as u8;
    }
    (w, h, out)
}

/// The reference count of a waveform cell: half a column.
pub fn waveform_reference(per_column: usize) -> f32 {
    (per_column as f32 / 2.0).max(1.0)
}

/// Paint a vectorscope: each point takes the colour of its own position (hue = angle) when
/// `colorize`, else `col`, and spreads over its 3 × 3 neighbourhood (falling off), so isolated
/// points stay visible when the scope is drawn smaller than its grid. Premultiplied RGBA8, row
/// 0 = +Cr.
pub fn vectorscope(v: &Vectorscope, col: [f32; 3], gain: f32, colorize: bool) -> (usize, usize, Vec<u8>) {
    let n = v.size;
    let reference = (v.samples as f32 / 64.0).max(1.0);
    let mut level = vec![0f32; n * n];
    let mut tint = vec![[0f32; 3]; n * n];
    for y in 0..n {
        for x in 0..n {
            let c = v.at(x, y);
            if c == 0 {
                continue;
            }
            let i = intensity(c, reference, gain);
            let t = if colorize {
                let [cb, cr] = v.point(x, y);
                // the colour at this chroma with mid luma, desaturated towards white
                let [r, g, b] = redrob_film_color::ycbcr_to_rgb(0.6, cb * 0.8, cr * 0.8, redrob_film_color::Matrix::Bt709);
                [0.55 + 0.45 * r.clamp(0.0, 1.0), 0.55 + 0.45 * g.clamp(0.0, 1.0), 0.55 + 0.45 * b.clamp(0.0, 1.0)]
            } else {
                col
            };
            for (dy, dx, w) in
                [(0i64, 0i64, 1.0f32), (-1, 0, 0.6), (1, 0, 0.6), (0, -1, 0.6), (0, 1, 0.6), (-1, -1, 0.35), (-1, 1, 0.35), (1, -1, 0.35), (1, 1, 0.35)]
            {
                let (yy, xx) = (y as i64 + dy, x as i64 + dx);
                if yy < 0 || xx < 0 || yy >= n as i64 || xx >= n as i64 {
                    continue;
                }
                let k = yy as usize * n + xx as usize;
                if i * w > level[k] {
                    level[k] = i * w;
                    tint[k] = t;
                }
            }
        }
    }
    let mut out = vec![0u8; n * n * 4];
    for ((o, l), t) in out.as_chunks_mut::<4>().0.iter_mut().zip(&level).zip(&tint) {
        if *l > 0.0 {
            o.copy_from_slice(&[
                (t[0] * l * 255.0).round() as u8,
                (t[1] * l * 255.0).round() as u8,
                (t[2] * l * 255.0).round() as u8,
                (l * 255.0).round() as u8,
            ]);
        }
    }
    (n, n, out)
}

/// A colour plane for Export Background of the vectorscope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    /// Hue as the angle (red at the vectorscope's red angle), saturation as the radius, at
    /// HSL lightness ½.
    Rgb,
    /// The analogue U (horizontal) / V (vertical) plane at Y' = ½.
    Yuv,
    /// The digital Cb (horizontal) / Cr (vertical) plane at Y' = ½.
    YCbCr,
}

impl Plane {
    pub const ALL: [Plane; 3] = [Plane::Rgb, Plane::Yuv, Plane::YCbCr];
    pub fn name(self) -> &'static str {
        match self {
            Plane::Rgb => "rgb",
            Plane::Yuv => "yuv",
            Plane::YCbCr => "ycbcr",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Plane::Rgb => "RGB",
            Plane::Yuv => "YUV",
            Plane::YCbCr => "YCbCr",
        }
    }
}

/// A `size`² straight-alpha RGBA8 image of a colour plane spanning ±`extent` (as the
/// vectorscope draws it: +Cb / +U right, +Cr / +V up). Colours outside the RGB gamut are clipped.
pub fn background(plane: Plane, m: redrob_film_color::Matrix, size: usize, extent: f32) -> (usize, usize, Vec<u8>) {
    let n = size.max(2);
    let (kr, kb) = m.kr_kb();
    let red = crate::red_angle(m);
    let mut out = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let px = (x as f32 / (n - 1) as f32 - 0.5) * 2.0 * extent;
            let py = (0.5 - y as f32 / (n - 1) as f32) * 2.0 * extent;
            let rgb = match plane {
                Plane::YCbCr => redrob_film_color::ycbcr_to_rgb(0.5, px, py, m),
                Plane::Yuv => {
                    let cb = px / (crate::U_SCALE * 2.0 * (1.0 - kb));
                    let cr = py / (crate::V_SCALE * 2.0 * (1.0 - kr));
                    redrob_film_color::ycbcr_to_rgb(0.5, cb, cr, m)
                }
                Plane::Rgb => {
                    let hue = (crate::angle_deg(px, py) - red).rem_euclid(360.0) / 360.0;
                    let sat = ((px * px + py * py).sqrt() / 0.5).min(1.0);
                    redrob_film_color::hsl_to_rgb(hue, sat, 0.5)
                }
            };
            let o = (y * n + x) * 4;
            for (d, v) in out[o..o + 3].iter_mut().zip(rgb) {
                *d = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            out[o + 3] = 255;
        }
    }
    (n, n, out)
}
