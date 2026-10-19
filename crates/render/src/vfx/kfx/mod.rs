//! CPU implementations of the "More Video Effects" bin (`redrob_film_project::effect::kfx`).
//!
//! Like the rest of [`crate::vfx`], images are premultiplied linear f32 and spatial parameters are
//! full-resolution clip pixels scaled by `FxCtx::px_scale`. These effects describe their maths in
//! 8-bit display terms (thresholds 0–255, "levels", planes of a YUV picture), so most of them work
//! on a straight, display-encoded copy of the picture ([`Disp`]) and write it back.
//!
//! Effects that need other frames (echo, burning, light graffiti, flash filtering, temporal
//! denoising) read them through [`crate::vfx::FxEnv`] and leave the picture as a single-frame
//! effect would (or unchanged) without one.

mod gps;
mod noise;
mod stylize;
mod temporal;
mod text;

use rayon::prelude::*;
use redrob_film_project::EffectInstance;

use super::{bv, chv, cv, fv, tv};
use crate::effects::FxCtx;
use crate::image::Image;

/// Effects here that read other frames (skipped when another frame is rendered for one of them).
pub fn is_temporal(id: &str) -> bool {
    matches!(id, "k_analog_echo" | "k_burning_tv" | "k_light_graffiti" | "k_photosensitivity" | "k_hqdn3d" | "k_hqdn3d_legacy" | "k_denoiser" | "k_fft_denoise")
}

/// Apply `e` if it belongs to this bin; `false` for any other id.
pub fn apply(img: &mut Image, e: &EffectInstance, cx: &FxCtx) -> bool {
    match e.effect.as_str() {
        // Stylize
        "k_threshold3" => stylize::threshold3(img),
        "k_analog_echo" => temporal::analog_echo(img, e, cx),
        "k_binarize" => stylize::binarize(img, e, cx),
        "k_binarize_dynamic" => stylize::binarize_dynamic(img),
        "k_burning_tv" => temporal::burning_tv(img, e, cx),
        "k_cartoon" => stylize::cartoon(img, e, cx),
        "k_charcoal" => stylize::charcoal(img, e, cx),
        "k_color_distance" => stylize::color_distance(img, e, cx),
        "k_color_effect" => stylize::color_effect(img, e),
        "k_heatmap" => stylize::heatmap(img, e, cx),
        "k_kaleidoscope" => stylize::kaleidoscope(img, e, cx),
        "k_kirsch" => stylize::edges(img, e, cx, stylize::Op::Kirsch),
        "k_prewitt" => stylize::edges(img, e, cx, stylize::Op::Prewitt),
        "k_roberts" => stylize::edges(img, e, cx, stylize::Op::Roberts),
        "k_scharr" => stylize::edges(img, e, cx, stylize::Op::Scharr),
        "k_sobel_planes" => stylize::edges(img, e, cx, stylize::Op::Sobel),
        "k_sobel" => stylize::sobel_rgb(img),
        "k_light_graffiti" => temporal::light_graffiti(img, e, cx),
        "k_ndvi" => stylize::ndvi(img, e, cx),
        "k_ntsc" => stylize::ntsc(img, e, cx),
        "k_old_film" => stylize::old_film(img, e, cx),
        "k_photosensitivity" => temporal::photosensitivity(img, e, cx),
        "k_pixelize" => stylize::pixelize(img, e, cx),
        "k_pixels0rt" => stylize::pixels0rt(img, e, cx),
        "k_posterize_elbg" => {
            let n = fv(e, "codebook", cx).round().clamp(1.0, 50.0) as usize;
            let steps = fv(e, "steps", cx).round().clamp(1.0, 50.0) as usize;
            stylize::quantize(img, n, steps, 0.0);
        }
        "k_kmeans" => {
            let n = fv(e, "clusters", cx).round().clamp(2.0, 64.0) as usize;
            stylize::quantize(img, n, 4, fv(e, "distance_weight", cx).clamp(0.0, 1.0));
        }
        "k_primaries" => stylize::primaries(img, e, cx),
        "k_pseudocolor" => stylize::pseudocolor(img, e, cx),
        "k_rgb_splitter" => stylize::rgb_splitter(img, e, cx),
        "k_squigglevision" => stylize::squigglevision(img, e, cx),
        "k_threshold" => stylize::threshold(img, e, cx),
        // Graphic clips: the layers' text is cut back while they are drawn
        // (`crate::graphic_clip::apply_typewriter`); the pixels are not touched here.
        "k_typewriter" => {}
        // Grain and Noise
        "k_fft_denoise" => noise::fft_denoise(img, e, cx),
        "k_chroma_nr" => noise::chroma_nr(img, e, cx),
        "k_hqdn3d" | "k_hqdn3d_legacy" | "k_denoiser" => noise::hqdn3d(img, e, cx),
        "k_dither" => noise::dither(img, e, cx),
        "k_dust" => noise::dust(img, fv(e, "diameter", cx), fv(e, "count", cx), cx),
        "k_film_grain" => noise::film_grain(img, e, cx),
        "k_grain" => noise::grain(img, e, cx),
        "k_gradfun" => noise::gradfun(img, e, cx),
        "k_scratch_lines" => noise::scratch_lines(img, e, cx),
        "k_wavelet_denoise" => noise::wavelet(img, e, cx),
        // Generate
        "k_draw_box" => stylize::draw_box(img, e, cx),
        "k_scanline" => stylize::scanline(img),
        "k_timer" => text::timer(img, e, cx),
        "k_gps_text" => gps::gps_text(img, e, cx),
        "k_gps_graphic" => gps::gps_graphic(img, e, cx),
        // Misc
        "k_alphaextract" => stylize::alphaextract(img),
        "k_deinterlace" => stylize::deinterlace(img, e),
        // Deprecated
        "k_box_blur" => {
            let h = (fv(e, "hori", cx) * cx.px_scale).round().max(0.0) as usize;
            let v = (fv(e, "vert", cx) * cx.px_scale).round().max(0.0) as usize;
            box_blur(img, h, v);
        }
        _ => return false,
    }
    true
}

// ------------------------------------------------------------------ shared helpers

/// The sequence frame rate (30 without an environment).
pub(crate) fn fps(cx: &FxCtx) -> f64 {
    cx.env.map(|e| e.frame_rate()).filter(|r| *r > 0.0).unwrap_or(30.0)
}
/// The frame number within the clip.
pub(crate) fn frame_no(cx: &FxCtx) -> i64 {
    (cx.seconds * fps(cx)).round() as i64
}

/// A straight (un-premultiplied), display-encoded copy of a picture: `[r, g, b, a]` in 0..1.
pub(crate) struct Disp {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 4]>,
}

impl Disp {
    pub fn of(img: &Image) -> Disp {
        let px = img
            .px
            .par_chunks(4)
            .map(|p| {
                let a = p[3];
                if a <= 1e-6 {
                    [0.0, 0.0, 0.0, 0.0]
                } else {
                    let c = crate::effects::enc([p[0] / a, p[1] / a, p[2] / a]);
                    [c[0].min(1.0), c[1].min(1.0), c[2].min(1.0), a.min(1.0)]
                }
            })
            .collect();
        Disp { w: img.w, h: img.h, px }
    }
    #[inline]
    pub fn get(&self, x: isize, y: isize) -> [f32; 4] {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.px[y * self.w + x]
    }
    /// Bilinear sample at continuous pixel coordinates (pixel centres at +0.5), clamped.
    pub fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let a = self.get(x0, y0);
        let b = self.get(x0 + 1, y0);
        let c = self.get(x0, y0 + 1);
        let d = self.get(x0 + 1, y0 + 1);
        let mut o = [0.0; 4];
        for k in 0..4 {
            o[k] = (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty;
        }
        o
    }
    /// Write back into `img` (premultiplied linear).
    pub fn store(&self, img: &mut Image) {
        img.px.par_chunks_mut(4).zip(self.px.par_iter()).for_each(|(p, s)| {
            let a = s[3].clamp(0.0, 1.0);
            let c = crate::effects::dec([s[0], s[1], s[2]]);
            p[0] = c[0] * a;
            p[1] = c[1] * a;
            p[2] = c[2] * a;
            p[3] = a;
        });
    }
}

/// Run `f` on every pixel's straight display colour (alpha kept). `f(rgb, x, y)`.
pub(crate) fn map_disp(img: &mut Image, f: impl Fn([f32; 3], usize, usize) -> [f32; 3] + Sync) {
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let p = &mut row[x * 4..x * 4 + 4];
            let a = p[3];
            if a <= 1e-6 {
                continue;
            }
            let c = crate::effects::enc([p[0] / a, p[1] / a, p[2] / a]);
            let o = f([c[0].min(1.0), c[1].min(1.0), c[2].min(1.0)], x, y);
            let o = crate::effects::dec(o);
            p[0] = o[0] * a;
            p[1] = o[1] * a;
            p[2] = o[2] * a;
        }
    });
}

#[inline]
pub(crate) fn luma(c: [f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}
/// Full-range BT.709 RGB → YUV with U and V centred on 0.5.
#[inline]
pub(crate) fn to_yuv(c: [f32; 4]) -> [f32; 4] {
    let y = luma(c);
    [y, (c[2] - y) / 1.8556 + 0.5, (c[0] - y) / 1.5748 + 0.5, c[3]]
}
#[inline]
pub(crate) fn from_yuv(p: [f32; 4]) -> [f32; 4] {
    let (y, u, v) = (p[0], p[1] - 0.5, p[2] - 0.5);
    let r = y + 1.5748 * v;
    let b = y + 1.8556 * u;
    let g = (y - 0.2126 * r - 0.0722 * b) / 0.7152;
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), p[3]]
}

/// Which components a [`redrob_film_project::effect::YUV_PLANES`] choice processes, as
/// `(yuv_mask, alpha)`: `yuv_mask[k]` for Y, U, V.
pub(crate) fn yuv_planes(choice: u32) -> ([bool; 3], bool) {
    match choice {
        0 => ([false; 3], false),
        1 => ([true, false, false], false),
        2 => ([false, true, false], false),
        3 => ([true, true, false], false),
        4 => ([false, false, true], false),
        5 => ([true, false, true], false),
        6 => ([false, true, true], false),
        8 => ([false; 3], true),
        _ => ([true; 3], false),
    }
}

/// A plane selection resolved to the picture representation it works in.
#[derive(Clone, Copy)]
pub(crate) struct Planes {
    /// Work on YUV (true) or RGB (false) components.
    pub yuv: bool,
    /// Components 0..3 of that representation, then alpha.
    pub on: [bool; 4],
}

impl Planes {
    pub fn from_yuv_choice(c: u32) -> Planes {
        let (m, a) = yuv_planes(c);
        Planes { yuv: true, on: [m[0], m[1], m[2], a] }
    }
    /// [`redrob_film_project::effect::ANY_PLANES`]: Alpha, Y, U, V, Red, Green, Blue, All.
    pub fn from_any_choice(c: u32) -> Planes {
        match c {
            0 => Planes { yuv: true, on: [false, false, false, true] },
            1 => Planes { yuv: true, on: [true, false, false, false] },
            2 => Planes { yuv: true, on: [false, true, false, false] },
            3 => Planes { yuv: true, on: [false, false, true, false] },
            4 => Planes { yuv: false, on: [true, false, false, false] },
            5 => Planes { yuv: false, on: [false, true, false, false] },
            6 => Planes { yuv: false, on: [false, false, true, false] },
            _ => Planes { yuv: true, on: [true, true, true, false] },
        }
    }
    pub fn none(&self) -> bool {
        !self.on.iter().any(|v| *v)
    }
}

/// Split a picture into planes (`Planes::yuv` decides the representation), 4 planes of w×h.
pub(crate) fn split_planes(d: &Disp, yuv: bool) -> [Vec<f32>; 4] {
    let conv: Vec<[f32; 4]> = if yuv { d.px.par_iter().map(|p| to_yuv(*p)).collect() } else { d.px.clone() };
    [
        conv.iter().map(|p| p[0]).collect(),
        conv.iter().map(|p| p[1]).collect(),
        conv.iter().map(|p| p[2]).collect(),
        conv.iter().map(|p| p[3]).collect(),
    ]
}
/// Inverse of [`split_planes`].
pub(crate) fn merge_planes(d: &mut Disp, planes: &[Vec<f32>; 4], yuv: bool) {
    d.px.par_iter_mut().enumerate().for_each(|(i, p)| {
        let q = [planes[0][i], planes[1][i], planes[2][i], planes[3][i].clamp(0.0, 1.0)];
        *p = if yuv { from_yuv(q) } else { [q[0].clamp(0.0, 1.0), q[1].clamp(0.0, 1.0), q[2].clamp(0.0, 1.0), q[3]] };
    });
}

/// Separable box blur of radius `rx` / `ry` pixels (each output averages 2r+1 samples, edges
/// clamped), on the premultiplied picture.
pub(crate) fn box_blur(img: &mut Image, rx: usize, ry: usize) {
    let (w, h) = (img.w, img.h);
    if rx > 0 {
        let src = img.px.clone();
        img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            let s = &src[y * w * 4..(y + 1) * w * 4];
            let n = (2 * rx + 1) as f32;
            let mut acc = [0.0f32; 4];
            let at = |x: isize| (x.clamp(0, w as isize - 1) as usize) * 4;
            for k in -(rx as isize)..=(rx as isize) {
                let i = at(k);
                for c in 0..4 {
                    acc[c] += s[i + c];
                }
            }
            for x in 0..w {
                for c in 0..4 {
                    row[x * 4 + c] = acc[c] / n;
                }
                let (add, sub) = (at(x as isize + rx as isize + 1), at(x as isize - rx as isize));
                for c in 0..4 {
                    acc[c] += s[add + c] - s[sub + c];
                }
            }
        });
    }
    if ry > 0 {
        let src = img.px.clone();
        let n = (2 * ry + 1) as f32;
        img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let mut acc = [0.0f32; 4];
                for k in -(ry as isize)..=(ry as isize) {
                    let yy = (y as isize + k).clamp(0, h as isize - 1) as usize;
                    let i = (yy * w + x) * 4;
                    for c in 0..4 {
                        acc[c] += src[i + c];
                    }
                }
                for c in 0..4 {
                    row[x * 4 + c] = acc[c] / n;
                }
            }
        });
    }
}

/// Box-blur one plane (radius `r`, edges clamped).
pub(crate) fn blur_plane_box(p: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 || w == 0 || h == 0 {
        return p.to_vec();
    }
    let n = (2 * r + 1) as f32;
    let mut tmp = vec![0.0f32; p.len()];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &p[y * w..(y + 1) * w];
        let at = |x: isize| s[x.clamp(0, w as isize - 1) as usize];
        let mut acc: f32 = (-(r as isize)..=(r as isize)).map(at).sum();
        for x in 0..w {
            row[x] = acc / n;
            acc += at(x as isize + r as isize + 1) - at(x as isize - r as isize);
        }
    });
    let mut out = vec![0.0f32; p.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut acc = 0.0;
            for k in -(r as isize)..=(r as isize) {
                acc += tmp[(y as isize + k).clamp(0, h as isize - 1) as usize * w + x];
            }
            row[x] = acc / n;
        }
    });
    out
}

/// A neighbouring frame of the clip (`dframes` frames away), with the effects before `e` applied,
/// resized to this picture; `None` without an environment.
pub(crate) fn neighbour(img: &Image, e: &EffectInstance, cx: &FxCtx, dframes: f64) -> Option<Image> {
    let env = cx.env?;
    let dt = dframes / fps(cx);
    // stay inside the clip
    let at = env.clip_offset() + dt;
    if at < -1e-6 || at > env.clip_seconds() + 1e-6 {
        return None;
    }
    let f = env.frame(dt, e)?;
    if f.w == img.w && f.h == img.h {
        return Some(f);
    }
    let m = redrob_film_geom::Affine::scale(img.w as f64 / f.w.max(1) as f64, img.h as f64 / f.h.max(1) as f64);
    Some(f.transformed(img.w, img.h, &m))
}

/// Deterministic 0..1 random from integers.
#[inline]
pub(crate) fn rnd(a: i64, b: i64, c: i64, seed: u64) -> f32 {
    super::hash01(a, b, c, seed)
}

/// A colour param as straight display RGB plus alpha.
pub(crate) fn color(e: &EffectInstance, id: &str, cx: &FxCtx) -> [f32; 4] {
    cv(e, id, cx)
}
pub(crate) fn flag(e: &EffectInstance, id: &str) -> bool {
    bv(e, id)
}
pub(crate) fn pick(e: &EffectInstance, id: &str) -> u32 {
    chv(e, id)
}
pub(crate) fn text_param(e: &EffectInstance, id: &str) -> String {
    tv(e, id)
}
