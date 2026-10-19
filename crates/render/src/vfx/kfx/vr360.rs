//! VR360 and stereoscopic effects of the bin, on equirectangular frames (x = longitude −180°…180°
//! wrapping, y = latitude 90°…−90°, the conventions of [`crate::vfx::immersive`]): viewing
//! projections (rectilinear / fisheye, stereographic "little planet"), the pole caps, the
//! equirectangular matte, dual-fisheye to equirectangular, rotation stabilisation, wrapping a
//! partial panorama over the sphere, and stereoscopic format conversion with anaglyphs.

use std::collections::HashMap;
use std::f64::consts::{PI, TAU};
use std::sync::{Arc, Mutex, OnceLock};

use redrob_film_geom::Affine;

use super::*;
use crate::vfx::immersive::{V3, dir, lonlat, mvec, rotation, sample_dir};
use crate::vfx::smoothstep;

/// Sample the equirectangular `img` in direction `d`, nearest-neighbour or bilinear.
fn sample(img: &Image, d: V3, nearest: bool) -> [f32; 4] {
    if !nearest {
        return sample_dir(img, d);
    }
    let (lon, lat) = lonlat(d);
    let x = ((lon / TAU + 0.5) * img.w as f64).floor() as isize;
    let y = ((0.5 - lat / PI) * img.h as f64).floor() as isize;
    img.get(x.rem_euclid(img.w as isize) as usize, y.clamp(0, img.h as isize - 1) as usize)
}

fn nearest(e: &EffectInstance) -> bool {
    pick(e, "interpolation") == 0
}

/// Longitude / latitude (radians) of a pixel centre.
#[inline]
fn px_lonlat(x: usize, y: usize, w: usize, h: usize) -> (f64, f64) {
    (((x as f64 + 0.5) / w as f64 - 0.5) * TAU, (0.5 - (y as f64 + 0.5) / h as f64) * PI)
}

/// Fill every pixel from a direction function of the output pixel.
fn remap(img: &mut Image, nearest: bool, f: impl Fn(usize, usize) -> Option<V3> + Sync) {
    let src = img.clone();
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let o = match f(x, y) {
                Some(d) => sample(&src, d, nearest),
                None => [0.0, 0.0, 0.0, 1.0],
            };
            row[x * 4..x * 4 + 4].copy_from_slice(&o);
        }
    });
}

/// A camera ray for a view: `r` (0 at the centre, 1 at half the frame width), angle `phi`, polar
/// angle `theta` from the view axis.
#[inline]
fn ray(theta: f64, phi: f64) -> V3 {
    let s = theta.sin();
    [s * phi.cos(), s * phi.sin(), theta.cos()]
}

/// The view rotation of Yaw / Pitch / Roll (degrees).
fn view_rotation(e: &EffectInstance, cx: &FxCtx) -> [[f64; 3]; 3] {
    rotation(fv(e, "pitch", cx) as f64, fv(e, "yaw", cx) as f64, fv(e, "roll", cx) as f64)
}

/// Normalised view coordinates of a pixel: x in −1…1 across the width, y scaled by the aspect.
#[inline]
fn view_xy(x: usize, y: usize, w: usize, h: usize) -> (f64, f64) {
    let nx = (x as f64 + 0.5) / w as f64 * 2.0 - 1.0;
    let ny = (1.0 - (y as f64 + 0.5) / h as f64 * 2.0) * h as f64 / w as f64;
    (nx, ny)
}

/// VR360 Equirectangular to Rectilinear: the view a 360 player shows, with FOV the horizontal
/// field of view; Fisheye mixes in an equidistant fisheye (used on its own past 179°).
pub fn equi2rect(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let rot = view_rotation(e, cx);
    let hf = (fv(e, "fov", cx) as f64).clamp(1.0, 360.0).to_radians() / 2.0;
    let fish = if hf.to_degrees() > 89.5 { 1.0 } else { (fv(e, "fisheye", cx) as f64 / 100.0).clamp(0.0, 1.0) };
    let tan_hf = hf.min(89.5f64.to_radians()).tan();
    let (w, h) = (img.w, img.h);
    remap(img, nearest(e), |x, y| {
        let (nx, ny) = view_xy(x, y, w, h);
        let r = (nx * nx + ny * ny).sqrt();
        let theta = (r * tan_hf).atan() * (1.0 - fish) + r * hf * fish;
        if theta > PI {
            return None;
        }
        Some(mvec(&rot, ray(theta, ny.atan2(nx))))
    });
}

/// VR360 Equirectangular to Stereo: a stereographic projection ("little planet" looking straight
/// down at Pitch −90°), Amount mixing between rectilinear (0 %) and stereographic (100 %).
pub fn equi2stereo(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let rot = view_rotation(e, cx);
    let hf = (fv(e, "fov", cx) as f64).clamp(1.0, 359.0).to_radians() / 2.0;
    let amount = (fv(e, "amount", cx) as f64 / 100.0).clamp(0.0, 1.0);
    let r_stereo = 2.0 * (hf / 2.0).tan();
    let r_rect = hf.min(89.5f64.to_radians()).tan();
    let (w, h) = (img.w, img.h);
    remap(img, nearest(e), |x, y| {
        let (nx, ny) = view_xy(x, y, w, h);
        let r = (nx * nx + ny * ny).sqrt();
        let ts = 2.0 * (r * r_stereo / 2.0).atan();
        let tr = (r * r_rect).atan();
        let theta = tr * (1.0 - amount) + ts * amount;
        Some(mvec(&rot, ray(theta, ny.atan2(nx))))
    });
}

/// VR360 Equirectangular Mask: black outside a longitude / latitude window (no matte within the
/// Start half-angles, full matte past End); Nearest-Neighbor gives a hard edge at the midpoint.
pub fn equi_mask(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (hs, he) = (fv(e, "h_start", cx), fv(e, "h_end", cx));
    let (vs, ve) = (fv(e, "v_start", cx), fv(e, "v_end", cx));
    let hard = nearest(e);
    let ramp = |v: f32, s: f32, en: f32| -> f32 {
        if en <= s + 1e-4 {
            return if v > s { 1.0 } else { 0.0 };
        }
        if hard { if v > (s + en) / 2.0 { 1.0 } else { 0.0 } } else { smoothstep(s, en, v) }
    };
    let (w, h) = (img.w, img.h);
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (lon, lat) = px_lonlat(x, y, w, h);
            let m = ramp(lon.to_degrees().abs() as f32, hs, he).max(ramp(lat.to_degrees().abs() as f32, vs, ve));
            if m <= 0.0 {
                continue;
            }
            let p = &mut row[x * 4..x * 4 + 4];
            for c in 0..3 {
                p[c] *= 1.0 - m;
            }
            p[3] = p[3] * (1.0 - m) + m;
        }
    });
}

/// Horizontal box blur of one equirectangular row with wrap-around (`r` pixels).
fn blur_row_wrap(row: &[f32], w: usize, r: usize) -> Vec<f32> {
    if r == 0 || w == 0 {
        return row.to_vec();
    }
    let r = r.min(w / 2);
    let n = (2 * r + 1) as f32;
    let wi = w as isize;
    let mut out = vec![0.0f32; w * 4];
    let mut acc = [0.0f32; 4];
    for j in -(r as isize)..=(r as isize) {
        let i = j.rem_euclid(wi) as usize;
        for k in 0..4 {
            acc[k] += row[i * 4 + k];
        }
    }
    for x in 0..wi {
        for k in 0..4 {
            out[x as usize * 4 + k] = acc[k] / n;
        }
        let (o, i) = ((x - r as isize).rem_euclid(wi) as usize, (x + r as isize + 1).rem_euclid(wi) as usize);
        for k in 0..4 {
            acc[k] += row[i * 4 + k] - row[o * 4 + k];
        }
    }
    out
}

/// One pole of VR360 Cap Top and Bottom.
struct Cap {
    on: bool,
    start: f64,
    end: f64,
    blend_in: f64,
    blend_out: f64,
    fade_in: f64,
    bw: (f64, f64),
    bh: (f64, f64),
}

fn cap_of(e: &EffectInstance, cx: &FxCtx, ids: [&str; 10]) -> Cap {
    let g = |i: usize| fv(e, ids[i], cx) as f64;
    Cap {
        on: flag(e, ids[0]),
        start: g(1),
        end: g(2).max(g(1) + 0.01),
        blend_in: g(3),
        blend_out: g(4),
        fade_in: g(5).max(0.01),
        bw: (g(6), g(7)),
        bh: (g(8), g(9)),
    }
}

/// VR360 Cap Top and Bottom: past Start (degrees from the horizon) towards each pole the picture
/// is refilled from a band of latitudes near Start, stretched and blurred over the pole (blur
/// widths / heights growing from Start to End), faded in over Fade In.
pub fn vr_cap(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let top = cap_of(
        e,
        cx,
        ["top", "top_start", "top_end", "top_blend_in", "top_blend_out", "top_fade_in", "top_bw_start", "top_bw_end", "top_bh_start", "top_bh_end"],
    );
    let bottom = cap_of(
        e,
        cx,
        [
            "bottom",
            "bottom_start",
            "bottom_end",
            "bottom_blend_in",
            "bottom_blend_out",
            "bottom_fade_in",
            "bottom_bw_start",
            "bottom_bw_end",
            "bottom_bh_start",
            "bottom_bh_end",
        ],
    );
    if !top.on && !bottom.on {
        return;
    }
    let src = img.clone();
    let (w, h) = (img.w, img.h);
    let row_of = |lat_deg: f64| -> isize { ((0.5 - lat_deg / 180.0) * h as f64).floor() as isize };
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let lat = (0.5 - (y as f64 + 0.5) / h as f64) * 180.0;
        let (cap, sign) = if lat >= 0.0 { (&top, 1.0) } else { (&bottom, -1.0) };
        let a = lat.abs();
        if !cap.on || a < cap.start {
            return;
        }
        let p = ((a - cap.start) / (cap.end - cap.start)).clamp(0.0, 1.0);
        let sample_lat = (cap.start + cap.blend_out) + (cap.start - cap.blend_in - cap.start - cap.blend_out) * p;
        let bh = cap.bh.0 + (cap.bh.1 - cap.bh.0) * p;
        let bw = cap.bw.0 + (cap.bw.1 - cap.bw.0) * p;
        // vertical blur: average the rows over the blur height around the sampled latitude
        let (r0, r1) = (row_of(sign * (sample_lat + bh / 2.0)), row_of(sign * (sample_lat - bh / 2.0)));
        let (ra, rb) = (r0.min(r1).clamp(0, h as isize - 1), r0.max(r1).clamp(0, h as isize - 1));
        let step = ((rb - ra) / 16).max(1);
        let mut line = vec![0.0f32; w * 4];
        let mut n = 0.0f32;
        let mut ry = ra;
        while ry <= rb {
            let s = ry as usize * w * 4;
            for (o, v) in line.iter_mut().zip(&src.px[s..s + w * 4]) {
                *o += *v;
            }
            n += 1.0;
            ry += step;
        }
        line.iter_mut().for_each(|v| *v /= n.max(1.0));
        let rad = (bw / 360.0 * w as f64 / 2.0).round().max(0.0) as usize;
        let filled = blur_row_wrap(&line, w, rad);
        let k = smoothstep(cap.start as f32, (cap.start + cap.fade_in) as f32, a as f32);
        for (o, f) in row.iter_mut().zip(&filled) {
            *o += (*f - *o) * k;
        }
    });
}

/// VR360 Wrap: the part of the panorama within Left / Right / Top / Bottom degrees of the centre
/// is stretched over the whole sphere, blurred horizontally by Blur Start (at the equator) to
/// Blur End (at the poles), in percent of the kept width.
pub fn vr_wrap(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (l, r) = (fv(e, "left", cx) as f64, fv(e, "right", cx) as f64);
    let (t, b) = (fv(e, "top", cx) as f64, fv(e, "bottom", cx) as f64);
    let (bs, be) = (fv(e, "blur_start", cx) as f64 / 100.0, fv(e, "blur_end", cx) as f64 / 100.0);
    let (w, h) = (img.w, img.h);
    remap(img, nearest(e), |x, y| {
        let (lon, lat) = px_lonlat(x, y, w, h);
        let sl = lon / PI * if lon < 0.0 { l } else { r };
        let sb = lat / (PI / 2.0) * if lat > 0.0 { t } else { b };
        Some(dir(sl.to_radians(), sb.to_radians()))
    });
    if bs <= 0.0 && be <= 0.0 {
        return;
    }
    let kept = (l + r) / 360.0 * w as f64;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let lat = ((0.5 - (y as f64 + 0.5) / h as f64) * 2.0).abs();
        let frac = bs + (be - bs) * lat;
        let rad = (frac * kept / 2.0).round().max(0.0) as usize;
        if rad > 0 {
            let o = blur_row_wrap(row, w, rad);
            row.copy_from_slice(&o);
        }
    });
}

/// VR360 Hemispherical to Equirectangular: two equidistant fisheye images (front and back lens,
/// each with its centre, Up direction and image-circle radius) mapped onto the sphere, with lens
/// distortion and vignetting corrections (Hugin's a/b/c and polynomial vignetting), aligned by
/// Yaw / Pitch / Roll, blended where the lenses overlap, and the nadir patched.
pub fn hemi2equi(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let rot = rotation(fv(e, "pitch", cx) as f64, fv(e, "yaw", cx) as f64, fv(e, "roll", cx) as f64);
    let half_fov = (fv(e, "lens_fov", cx) as f64).clamp(90.0, 360.0).to_radians() / 2.0;
    let radius = fv(e, "lens_radius", cx) as f64;
    let lens = [
        (fv(e, "front_x", cx) as f64, fv(e, "front_y", cx) as f64, (fv(e, "front_up", cx) as f64).to_radians()),
        (fv(e, "back_x", cx) as f64, fv(e, "back_y", cx) as f64, (fv(e, "back_up", cx) as f64).to_radians()),
    ];
    let (da, db, dc) = (fv(e, "dist_a", cx) as f64, fv(e, "dist_b", cx) as f64, fv(e, "dist_c", cx) as f64);
    let dr = (fv(e, "dist_radius", cx) as f64).max(1e-3);
    let vig = [fv(e, "vig_a", cx), fv(e, "vig_b", cx), fv(e, "vig_c", cx), fv(e, "vig_d", cx)];
    let vr = fv(e, "vig_radius", cx).max(1e-3);
    let nadir_r = (fv(e, "nadir_radius", cx) as f64).clamp(0.0, 1.0);
    let nadir_s = (fv(e, "nadir_start", cx) as f64).clamp(0.0, 1.0);
    let near = nearest(e);
    let overlap = (half_fov - PI / 2.0).max(0.0);
    let src = img.clone();
    let (w, h) = (img.w, img.h);
    let (wf, hf) = (w as f64, h as f64);
    // one lens: polar angle θ from its axis and angle φ around it (x right, y up)
    let lens_sample = |li: usize, theta: f64, phi: f64| -> [f32; 4] {
        let (cxl, cyl, up) = lens[li];
        let r = theta / half_fov * radius;
        let rn = r / dr;
        let rs = r * (da * rn.powi(3) + db * rn * rn + dc * rn + (1.0 - da - db - dc));
        let ang = up + (PI / 2.0 - phi);
        let (sx, sy) = (cxl * wf + rs * wf * ang.sin(), cyl * hf - rs * wf * ang.cos());
        let mut p = if near { src.get_clamped(sx.floor() as isize, sy.floor() as isize) } else { src.sample_bilinear_clamped(sx as f32, sy as f32) };
        let q = (r / vr as f64) as f32;
        let q2 = q * q;
        let v = (vig[0] + vig[1] * q2 + vig[2] * q2 * q2 + vig[3] * q2 * q2 * q2).max(0.05);
        for c in 0..3 {
            p[c] /= v;
        }
        p
    };
    let sphere = |lon: f64, lat: f64| -> [f32; 4] {
        let d = mvec(&rot, dir(lon, lat));
        let tf = d[2].clamp(-1.0, 1.0).acos();
        let tb = (-d[2]).clamp(-1.0, 1.0).acos();
        let front = (tf <= half_fov).then(|| lens_sample(0, tf, d[1].atan2(d[0])));
        let back = (tb <= half_fov).then(|| lens_sample(1, tb, d[1].atan2(-d[0])));
        match (front, back) {
            (Some(f), Some(b)) => {
                // across the overlap band around the 90° seam
                let k = if overlap > 1e-6 {
                    (((tf - PI / 2.0) / overlap) * 0.5 + 0.5).clamp(0.0, 1.0) as f32
                } else if tf < PI / 2.0 {
                    0.0
                } else {
                    1.0
                };
                [0, 1, 2, 3].map(|c| f[c] + (b[c] - f[c]) * k)
            }
            (Some(f), None) => f,
            (None, Some(b)) => b,
            (None, None) => [0.0, 0.0, 0.0, 1.0],
        }
    };
    let nadir_ring = -(PI / 2.0) * (1.0 - nadir_r);
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (lon, lat) = px_lonlat(x, y, w, h);
            let mut p = sphere(lon, lat);
            if nadir_r > 0.0 {
                // the patch around the nadir (camera mount) is filled from the ring just outside it
                let from_nadir = (lat + PI / 2.0) / (PI / 2.0);
                if from_nadir < nadir_r {
                    let fill = sphere(lon, nadir_ring);
                    let k = 1.0 - smoothstep((nadir_r * nadir_s) as f32, nadir_r as f32, from_nadir as f32);
                    p = [0, 1, 2, 3].map(|c| p[c] + (fill[c] - p[c]) * k);
                }
            }
            row[x * 4..x * 4 + 4].copy_from_slice(&p);
        }
    });
}

// ------------------------------------------------------------------ VR360 Stabilize

type VrPathCache = Mutex<HashMap<u64, Arc<Vec<[f64; 3]>>>>;
fn vr_paths() -> &'static VrPathCache {
    static C: OnceLock<VrPathCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// A small grey equirectangular plane for tracking.
struct Gray {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Gray {
    fn of(img: &Image, w: usize) -> Gray {
        let w = w.max(16);
        let h = (w / 2).max(8);
        let m = Affine::scale(w as f64 / img.w.max(1) as f64, h as f64 / img.h.max(1) as f64);
        let s = img.transformed(w, h, &m);
        let v = s.px.chunks(4).map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).collect();
        Gray { w, h, v }
    }
    #[inline]
    fn at(&self, x: isize, y: isize) -> f32 {
        self.v[y.clamp(0, self.h as isize - 1) as usize * self.w + x.rem_euclid(self.w as isize) as usize]
    }
}

/// Block-match the patch at (`x`, `y`) of `a` in `b` within `search` pixels: the displacement.
fn match_block(a: &Gray, b: &Gray, x: isize, y: isize, patch: isize, search: isize) -> (f64, f64) {
    let mut best = (f32::MAX, 0isize, 0isize);
    let step = (patch / 6).max(1);
    for dy in -search..=search {
        for dx in -search..=search {
            let mut sad = 0.0f32;
            let mut py = -patch;
            while py <= patch {
                let mut px = -patch;
                while px <= patch {
                    sad += (a.at(x + px, y + py) - b.at(x + px + dx, y + py + dy)).abs();
                    px += step;
                }
                py += step;
            }
            if sad < best.0 {
                best = (sad, dx, dy);
            }
        }
    }
    (best.1 as f64, best.2 as f64)
}

/// The frame-to-frame rotation vector (radians, about X / Y / Z) that moves the track points of
/// `a` onto `b` (least squares of δd = ω × d over the points).
fn frame_rotation(a: &Gray, b: &Gray, points: &[(f64, f64)], patch: isize, search: isize) -> [f64; 3] {
    let mut m = [[0.0f64; 3]; 3];
    let mut rhs = [0.0f64; 3];
    for &(lon, lat) in points {
        let x = ((lon / TAU + 0.5) * a.w as f64) as isize;
        let y = ((0.5 - lat / PI) * a.h as f64) as isize;
        let (dx, dy) = match_block(a, b, x, y, patch, search);
        let dlon = dx * TAU / a.w as f64;
        let dlat = -dy * PI / a.h as f64;
        let d = dir(lon, lat);
        let (cl, sl, co, so) = (lat.cos(), lat.sin(), lon.cos(), lon.sin());
        let dd = [cl * co * dlon - sl * so * dlat, cl * dlat, -cl * so * dlon - sl * co * dlat];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += if i == j { 1.0 } else { 0.0 } - d[i] * d[j];
            }
        }
        let c = [d[1] * dd[2] - d[2] * dd[1], d[2] * dd[0] - d[0] * dd[2], d[0] * dd[1] - d[1] * dd[0]];
        for i in 0..3 {
            rhs[i] += c[i];
        }
    }
    solve3(m, rhs).unwrap_or([0.0; 3])
}

fn solve3(m: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-9 {
        return None;
    }
    let mut out = [0.0; 3];
    for (k, o) in out.iter_mut().enumerate() {
        let mut mk = m;
        for r in 0..3 {
            mk[r][k] = b[r];
        }
        let dk = mk[0][0] * (mk[1][1] * mk[2][2] - mk[1][2] * mk[2][1]) - mk[0][1] * (mk[1][0] * mk[2][2] - mk[1][2] * mk[2][0])
            + mk[0][2] * (mk[1][0] * mk[2][1] - mk[1][1] * mk[2][0]);
        *o = dk / det;
    }
    Some(out)
}

/// Rotation matrix of a rotation vector (Rodrigues).
fn rodrigues(v: [f64; 3]) -> [[f64; 3]; 3] {
    let t = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if t < 1e-12 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let k = [v[0] / t, v[1] / t, v[2] / t];
    let (s, c) = t.sin_cos();
    let kk = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]];
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let k2: f64 = (0..3).map(|n| kk[i][n] * kk[n][j]).sum();
            r[i][j] = if i == j { 1.0 } else { 0.0 } + s * kk[i][j] + (1.0 - c) * k2;
        }
    }
    r
}

/// The cumulative rotation of every frame of the clip since its first (radians, X / Y / Z),
/// analysed once from the clip's pictures with the effects above this one, then cached.
fn vr_path(img: &Image, e: &EffectInstance, cx: &FxCtx) -> Option<Arc<Vec<[f64; 3]>>> {
    let env = cx.env?;
    let fps = fps(cx).max(1.0);
    let now = env.clip_offset();
    let patch = fv(e, "sample_radius", cx).round().max(2.0) as isize;
    let search = fv(e, "search_radius", cx).round().max(1.0) as isize;
    let off = (fv(e, "track_offset", cx) as f64).to_radians();
    let back = flag(e, "back_points");
    // the key: the clip, the analysis settings, and a thumbnail of its first frame as the effects
    // above this one leave it (so editing them re-analyses)
    let first = env.frame(-now, e)?;
    let thumb = Gray::of(&first, 32);
    let mut key =
        env.clip_key() ^ (patch as u64).wrapping_mul(0x9E37_79B9) ^ (search as u64).wrapping_mul(0x85EB_CA6B) ^ (off.to_bits()).rotate_left(7) ^ back as u64;
    for v in &thumb.v {
        key = (key ^ ((v * 63.0).round() as u64)).wrapping_mul(0x0100_0000_01b3);
    }
    if let Some(p) = vr_paths().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Some(p);
    }
    let n = ((env.clip_seconds() * fps).round() as usize).clamp(1, 20_000);
    let _ = img;
    let frames: Vec<Option<Gray>> = (0..n)
        .into_par_iter()
        .map(|k| {
            if redrob_film_media::cancel::cancelled() {
                return None;
            }
            env.frame(k as f64 / fps - now, e).map(|f| Gray::of(&f, 512))
        })
        .collect();
    if redrob_film_media::cancel::cancelled() {
        return None;
    }
    let mut points = vec![(0.0, 0.0), (off, 0.0), (-off, 0.0), (0.0, off / 2.0), (0.0, -off / 2.0), (PI / 2.0, 0.0), (-PI / 2.0, 0.0)];
    if back {
        points.extend([(PI - 1e-3, 0.0), (PI - off, 0.0), (-PI + off, 0.0), (PI - 1e-3, off / 2.0), (PI - 1e-3, -off / 2.0)]);
    }
    let steps: Vec<[f64; 3]> = (0..n.saturating_sub(1))
        .into_par_iter()
        .map(|k| match (&frames[k], &frames[k + 1]) {
            (Some(a), Some(b)) if a.w == b.w => frame_rotation(a, b, &points, patch, search),
            _ => [0.0; 3],
        })
        .collect();
    let mut cum = Vec::with_capacity(n);
    let mut acc = [0.0f64; 3];
    cum.push(acc);
    for s in steps {
        for i in 0..3 {
            acc[i] += s[i];
        }
        cum.push(acc);
    }
    let path = Arc::new(cum);
    if let Ok(mut c) = vr_paths().lock() {
        if c.len() > 32 {
            c.clear();
        }
        c.insert(key, path.clone());
    }
    Some(path)
}

/// VR360 Stabilize: the clip's frame-to-frame rotation is analysed (track points around the
/// front, optionally the back), smoothed per axis over the Smoothing frames (Time Bias leans the
/// window to the past or the future), and the difference is rotated away by the Amounts.
pub fn vr_stabilize(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some(path) = vr_path(img, e, cx) else { return };
    let n = path.len();
    if n < 2 {
        return;
    }
    let k = (frame_no(cx).max(0) as usize).min(n - 1);
    // axis order of the path: X = pitch, Y = yaw, Z = roll
    let axes = [("pitch_amount", "pitch_smooth", "pitch_bias"), ("yaw_amount", "yaw_smooth", "yaw_bias"), ("roll_amount", "roll_smooth", "roll_bias")];
    let mut corr = [0.0f64; 3];
    for (i, (amt, smooth, bias)) in axes.iter().enumerate() {
        let a = (fv(e, amt, cx) as f64 / 100.0).clamp(0.0, 1.0);
        if a <= 0.0 {
            continue;
        }
        let win = fv(e, smooth, cx).max(1.0) as f64;
        let shift = (fv(e, bias, cx) as f64 / 100.0).clamp(-1.0, 1.0) * win / 2.0;
        let lo = (k as f64 + shift - win / 2.0).round().clamp(0.0, (n - 1) as f64) as usize;
        let hi = (k as f64 + shift + win / 2.0).round().clamp(0.0, (n - 1) as f64) as usize;
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let mean = path[lo..=hi].iter().map(|p| p[i]).sum::<f64>() / (hi - lo + 1) as f64;
        corr[i] = (mean - path[k][i]) * a;
    }
    if corr.iter().all(|c| c.abs() < 1e-7) {
        return;
    }
    // output direction → where the current frame shows it: undo the deviation from the smooth path
    let r = rodrigues([-corr[0], -corr[1], -corr[2]]);
    let (w, h) = (img.w, img.h);
    remap(img, nearest(e), |x, y| {
        let (lon, lat) = px_lonlat(x, y, w, h);
        Some(mvec(&r, dir(lon, lat)))
    });
}

// ------------------------------------------------------------------ Stereoscopic 3D

/// The two eyes of a stereoscopic frame in one of [`redrob_film_project::effect::STEREO_INPUTS`].
fn split_eyes(img: &Image, input: u32) -> (Image, Image) {
    let (w, h) = (img.w, img.h);
    let crop = |x0: usize, y0: usize, cw: usize, ch: usize| -> Image {
        let mut o = Image::new(cw.max(1), ch.max(1));
        for y in 0..ch {
            for x in 0..cw {
                let p = img.get((x0 + x).min(w - 1), (y0 + y).min(h - 1));
                let i = (y * cw + x) * 4;
                o.px[i..i + 4].copy_from_slice(&p);
            }
        }
        o
    };
    let every = |rows: bool, first: usize| -> Image {
        let (cw, ch) = if rows { (w, h.div_ceil(2)) } else { (w.div_ceil(2), h) };
        let mut o = Image::new(cw.max(1), ch.max(1));
        for y in 0..ch {
            for x in 0..cw {
                let (sx, sy) = if rows { (x, (2 * y + first).min(h - 1)) } else { ((2 * x + first).min(w - 1), y) };
                let p = img.get(sx, sy);
                let i = (y * cw + x) * 4;
                o.px[i..i + 4].copy_from_slice(&p);
            }
        }
        o
    };
    let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
    match input {
        1 => (crop(hw, 0, w - hw, h), crop(0, 0, hw, h)),
        2 => (crop(0, 0, w, hh), crop(0, hh, w, h - hh)),
        3 => (crop(0, hh, w, h - hh), crop(0, 0, w, hh)),
        4 => (every(true, 0), every(true, 1)),
        5 => (every(true, 1), every(true, 0)),
        6 => (every(false, 0), every(false, 1)),
        7 => (every(false, 1), every(false, 0)),
        _ => (crop(0, 0, hw, h), crop(hw, 0, w - hw, h)),
    }
}

fn resized(img: &Image, w: usize, h: usize) -> Image {
    if img.w == w && img.h == h {
        return img.clone();
    }
    img.transformed(w, h, &Affine::scale(w as f64 / img.w.max(1) as f64, h as f64 / img.h.max(1) as f64))
}

/// Paste `src` into `dst` at (`x0`, `y0`).
fn paste(dst: &mut Image, src: &Image, x0: usize, y0: usize) {
    for y in 0..src.h {
        if y0 + y >= dst.h {
            break;
        }
        let n = src.w.min(dst.w.saturating_sub(x0));
        let d = ((y0 + y) * dst.w + x0) * 4;
        dst.px[d..d + n * 4].copy_from_slice(&src.px[y * src.w * 4..(y * src.w + n) * 4]);
    }
}

/// Dubois least-squares anaglyph matrices (left, right), rows = output R, G, B.
const DUBOIS: [([[f32; 3]; 3], [[f32; 3]; 3]); 3] = [
    // red / cyan
    ([[0.437, 0.449, 0.164], [-0.062, -0.062, -0.024], [-0.048, -0.050, -0.017]], [[-0.011, -0.032, -0.007], [0.377, 0.761, 0.009], [-0.026, -0.093, 1.234]]),
    // green / magenta
    ([[-0.062, -0.158, -0.039], [0.284, 0.668, 0.143], [-0.015, -0.027, 0.021]], [[0.529, 0.705, 0.024], [-0.016, -0.015, -0.065], [0.009, 0.075, 0.937]]),
    // yellow / blue
    ([[1.062, -0.205, 0.299], [-0.026, 0.908, 0.068], [-0.038, -0.173, 0.022]], [[-0.016, -0.123, -0.017], [0.006, 0.062, -0.017], [0.094, 0.185, 0.911]]),
];

/// One anaglyph pixel from the eyes' straight display colours.
fn anaglyph(kind: u32, l: [f32; 4], r: [f32; 4]) -> [f32; 3] {
    let gl = 0.299 * l[0] + 0.587 * l[1] + 0.114 * l[2];
    let gr = 0.299 * r[0] + 0.587 * r[1] + 0.114 * r[2];
    let dub = |m: &([[f32; 3]; 3], [[f32; 3]; 3])| [0, 1, 2].map(|i| (0..3).map(|j| m.0[i][j] * l[j] + m.1[i][j] * r[j]).sum::<f32>().clamp(0.0, 1.0));
    match kind {
        6 => [gl, 0.0, gr],
        7 => [gl, gr, 0.0],
        8 => [gl, gr, gr],
        9 => [gl, r[1], r[2]],
        10 => [l[0], r[1], r[2]],
        11 => dub(&DUBOIS[0]),
        12 => [gr, gl, gr],
        13 => [r[0], gl, r[2]],
        14 => [r[0], l[1], r[2]],
        15 => dub(&DUBOIS[1]),
        16 => [gl, gl, gr],
        17 => [l[0], l[1], gr],
        18 => [l[0], l[1], r[2]],
        _ => dub(&DUBOIS[2]),
    }
}

/// Stereoscopic 3D: convert between side-by-side, above-below, interleaved and the anaglyph,
/// mono, checkerboard and HDMI frame-packing layouts.
pub fn stereo3d(img: &mut Image, e: &EffectInstance) {
    let (input, output) = (pick(e, "input"), pick(e, "output"));
    let (w, h) = (img.w, img.h);
    let (l, r) = split_eyes(img, input);
    let mut out = Image::filled(w, h, [0.0, 0.0, 0.0, 1.0]);
    let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
    match output {
        0 | 1 => {
            let (a, b) = if output == 0 { (&l, &r) } else { (&r, &l) };
            paste(&mut out, &resized(a, hw, h), 0, 0);
            paste(&mut out, &resized(b, w - hw, h), hw, 0);
        }
        2 | 3 => {
            let (a, b) = if output == 2 { (&l, &r) } else { (&r, &l) };
            paste(&mut out, &resized(a, w, hh), 0, 0);
            paste(&mut out, &resized(b, w, h - hh), 0, hh);
        }
        4 | 5 | 22..=25 => {
            let (a, b) = (resized(&l, w, h), resized(&r, w, h));
            let first_left = matches!(output, 4 | 22 | 24);
            out.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w {
                    let parity = match output {
                        4 | 5 => y % 2,
                        22 | 23 => (x + y) % 2,
                        _ => x % 2,
                    };
                    let left = (parity == 0) == first_left;
                    let s = if left { &a } else { &b };
                    let i = (y * w + x) * 4;
                    row[x * 4..x * 4 + 4].copy_from_slice(&s.px[i..i + 4]);
                }
            });
        }
        20 => out = resized(&l, w, h),
        21 => out = resized(&r, w, h),
        26 => {
            // frame packing: the eyes stacked with a blank band of 45 / 2205 of the height
            let eye_h = (h as f64 * 1080.0 / 2205.0).round().max(1.0) as usize;
            let gap = (h as f64 * 45.0 / 2205.0).round() as usize;
            paste(&mut out, &resized(&l, w, eye_h), 0, 0);
            paste(&mut out, &resized(&r, w, h.saturating_sub(eye_h + gap).max(1)), 0, (eye_h + gap).min(h.saturating_sub(1)));
        }
        _ => {
            let (a, b) = (Disp::of(&resized(&l, w, h)), Disp::of(&resized(&r, w, h)));
            let mut d = Disp::of(&out);
            d.px.par_iter_mut().enumerate().for_each(|(i, p)| {
                let c = anaglyph(output, a.px[i], b.px[i]);
                *p = [c[0], c[1], c[2], a.px[i][3].max(b.px[i][3])];
            });
            d.store(&mut out);
        }
    }
    *img = out;
}
