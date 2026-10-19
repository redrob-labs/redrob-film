//! Intra prediction (8.3): Intra_4x4, Intra_8x8 (with reference filtering), Intra_16x16 and chroma
//! (4:2:0 and 4:2:2), for 8-bit (`u8`) and high-bit-depth (`u16`) samples.

use crate::picture::Sample;

/// Neighbouring sample availability for one block.
#[derive(Clone, Copy, Debug, Default)]
pub struct Avail {
    pub left: bool,
    pub top: bool,
    pub top_left: bool,
    pub top_right: bool,
}

/// Neighbouring samples of an NxN block: p[x,-1] for x = 0..2N-1, p[-1,y] for y = 0..N-1, p[-1,-1].
#[derive(Clone, Copy)]
struct Edge {
    top: [i32; 16],
    left: [i32; 8],
    tl: i32,
}

impl Edge {
    /// p[x, -1] for x >= -1
    #[inline(always)]
    fn t(&self, x: i32) -> i32 {
        if x < 0 { self.tl } else { self.top[x as usize] }
    }
    /// p[-1, y] for y >= -1
    #[inline(always)]
    fn l(&self, y: i32) -> i32 {
        if y < 0 { self.tl } else { self.left[y as usize] }
    }
}

/// 1 << (BitDepth - 1): the value of unavailable neighbours (128 for 8-bit samples).
#[inline(always)]
fn mid<T: Sample>(max: i32) -> i32 {
    (T::clip_max(max) + 1) >> 1
}

fn load_edge<T: Sample>(plane: &[T], stride: usize, x: usize, y: usize, n: usize, a: Avail, max: i32) -> Edge {
    let m = mid::<T>(max);
    let mut e = Edge { top: [m; 16], left: [m; 8], tl: m };
    if a.top {
        let row = &plane[(y - 1) * stride + x..];
        for i in 0..n {
            e.top[i] = row[i].to_i32();
        }
        if a.top_right {
            for i in n..2 * n {
                e.top[i] = row[i].to_i32();
            }
        } else {
            let v = e.top[n - 1];
            for t in &mut e.top[n..2 * n] {
                *t = v;
            }
        }
    }
    if a.left {
        for i in 0..n {
            e.left[i] = plane[(y + i) * stride + x - 1].to_i32();
        }
    }
    if a.top_left {
        e.tl = plane[(y - 1) * stride + x - 1].to_i32();
    }
    e
}

/// Directional / DC prediction shared by Intra_4x4 and Intra_8x8 (8.3.1.2.x, 8.3.2.2.x).
fn pred_nxn<T: Sample>(mode: u8, e: &Edge, n: i32, a: Avail, out: &mut [T], stride: usize, max: i32) {
    let mut put = |x: i32, y: i32, v: i32| out[y as usize * stride + x as usize] = T::from_i32(v);
    match mode {
        0 => {
            for y in 0..n {
                for x in 0..n {
                    put(x, y, e.t(x));
                }
            }
        }
        1 => {
            for y in 0..n {
                for x in 0..n {
                    put(x, y, e.l(y));
                }
            }
        }
        2 => {
            let shift = if n == 4 { 2 } else { 3 };
            let st: i32 = (0..n).map(|x| e.t(x)).sum();
            let sl: i32 = (0..n).map(|y| e.l(y)).sum();
            let dc = match (a.top, a.left) {
                (true, true) => (st + sl + n) >> (shift + 1),
                (false, true) => (sl + (n >> 1)) >> shift,
                (true, false) => (st + (n >> 1)) >> shift,
                _ => mid::<T>(max),
            };
            for y in 0..n {
                for x in 0..n {
                    put(x, y, dc);
                }
            }
        }
        3 => {
            for y in 0..n {
                for x in 0..n {
                    let v = if x == n - 1 && y == n - 1 {
                        (e.t(2 * n - 2) + 3 * e.t(2 * n - 1) + 2) >> 2
                    } else {
                        (e.t(x + y) + 2 * e.t(x + y + 1) + e.t(x + y + 2) + 2) >> 2
                    };
                    put(x, y, v);
                }
            }
        }
        4 => {
            for y in 0..n {
                for x in 0..n {
                    let v = if x > y {
                        (e.t(x - y - 2) + 2 * e.t(x - y - 1) + e.t(x - y) + 2) >> 2
                    } else if x < y {
                        (e.l(y - x - 2) + 2 * e.l(y - x - 1) + e.l(y - x) + 2) >> 2
                    } else {
                        (e.t(0) + 2 * e.tl + e.l(0) + 2) >> 2
                    };
                    put(x, y, v);
                }
            }
        }
        5 => {
            for y in 0..n {
                for x in 0..n {
                    let z = 2 * x - y;
                    let v = if z >= 0 && z % 2 == 0 {
                        (e.t(x - (y >> 1) - 1) + e.t(x - (y >> 1)) + 1) >> 1
                    } else if z > 0 {
                        (e.t(x - (y >> 1) - 2) + 2 * e.t(x - (y >> 1) - 1) + e.t(x - (y >> 1)) + 2) >> 2
                    } else if z == -1 {
                        (e.l(0) + 2 * e.tl + e.t(0) + 2) >> 2
                    } else {
                        (e.l(y - 2 * x - 1) + 2 * e.l(y - 2 * x - 2) + e.l(y - 2 * x - 3) + 2) >> 2
                    };
                    put(x, y, v);
                }
            }
        }
        6 => {
            for y in 0..n {
                for x in 0..n {
                    let z = 2 * y - x;
                    let v = if z >= 0 && z % 2 == 0 {
                        (e.l(y - (x >> 1) - 1) + e.l(y - (x >> 1)) + 1) >> 1
                    } else if z > 0 {
                        (e.l(y - (x >> 1) - 2) + 2 * e.l(y - (x >> 1) - 1) + e.l(y - (x >> 1)) + 2) >> 2
                    } else if z == -1 {
                        (e.l(0) + 2 * e.tl + e.t(0) + 2) >> 2
                    } else {
                        (e.t(x - 2 * y - 1) + 2 * e.t(x - 2 * y - 2) + e.t(x - 2 * y - 3) + 2) >> 2
                    };
                    put(x, y, v);
                }
            }
        }
        7 => {
            for y in 0..n {
                for x in 0..n {
                    let i = x + (y >> 1);
                    let v = if y % 2 == 0 { (e.t(i) + e.t(i + 1) + 1) >> 1 } else { (e.t(i) + 2 * e.t(i + 1) + e.t(i + 2) + 2) >> 2 };
                    put(x, y, v);
                }
            }
        }
        _ => {
            let zmax = 2 * n - 3;
            for y in 0..n {
                for x in 0..n {
                    let z = x + 2 * y;
                    let i = y + (x >> 1);
                    let v = if z < zmax && z % 2 == 0 {
                        (e.l(i) + e.l(i + 1) + 1) >> 1
                    } else if z < zmax {
                        (e.l(i) + 2 * e.l(i + 1) + e.l(i + 2) + 2) >> 2
                    } else if z == zmax {
                        (e.l(n - 2) + 3 * e.l(n - 1) + 2) >> 2
                    } else {
                        e.l(n - 1)
                    };
                    put(x, y, v);
                }
            }
        }
    }
}

/// Intra_4x4 prediction written into `plane` at (x, y).
pub fn pred4x4<T: Sample>(plane: &mut [T], stride: usize, x: usize, y: usize, mode: u8, a: Avail, max: i32) {
    let e = load_edge(plane, stride, x, y, 4, a, max);
    pred_nxn(mode, &e, 4, a, &mut plane[y * stride + x..], stride, max);
}

/// Intra_8x8 prediction (with reference sample filtering) written into `plane` at (x, y).
pub fn pred8x8<T: Sample>(plane: &mut [T], stride: usize, x: usize, y: usize, mode: u8, a: Avail, max: i32) {
    let p = load_edge(plane, stride, x, y, 8, a, max);
    let mut f = p;
    if a.top {
        f.top[0] = if a.top_left { (p.tl + 2 * p.top[0] + p.top[1] + 2) >> 2 } else { (3 * p.top[0] + p.top[1] + 2) >> 2 };
        for i in 1..15 {
            f.top[i] = (p.top[i - 1] + 2 * p.top[i] + p.top[i + 1] + 2) >> 2;
        }
        f.top[15] = (p.top[14] + 3 * p.top[15] + 2) >> 2;
    }
    if a.top_left {
        f.tl = match (a.top, a.left) {
            (true, true) => (p.top[0] + 2 * p.tl + p.left[0] + 2) >> 2,
            (true, false) => (3 * p.tl + p.top[0] + 2) >> 2,
            (false, true) => (3 * p.tl + p.left[0] + 2) >> 2,
            _ => p.tl,
        };
    }
    if a.left {
        f.left[0] = if a.top_left { (p.tl + 2 * p.left[0] + p.left[1] + 2) >> 2 } else { (3 * p.left[0] + p.left[1] + 2) >> 2 };
        for i in 1..7 {
            f.left[i] = (p.left[i - 1] + 2 * p.left[i] + p.left[i + 1] + 2) >> 2;
        }
        f.left[7] = (p.left[6] + 3 * p.left[7] + 2) >> 2;
    }
    pred_nxn(mode, &f, 8, a, &mut plane[y * stride + x..], stride, max);
}

/// Intra_16x16 prediction (8.3.3) for the macroblock at luma (x, y).
pub fn pred16x16<T: Sample>(plane: &mut [T], stride: usize, x: usize, y: usize, mode: u8, a: Avail, max: i32) {
    let mut top = [0i32; 16];
    let mut left = [0i32; 16];
    if a.top {
        for i in 0..16 {
            top[i] = plane[(y - 1) * stride + x + i].to_i32();
        }
    }
    if a.left {
        for i in 0..16 {
            left[i] = plane[(y + i) * stride + x - 1].to_i32();
        }
    }
    let base = y * stride + x;
    match mode {
        0 => {
            for r in 0..16 {
                for c in 0..16 {
                    plane[base + r * stride + c] = T::from_i32(top[c]);
                }
            }
        }
        1 => {
            for r in 0..16 {
                plane[base + r * stride..base + r * stride + 16].fill(T::from_i32(left[r]));
            }
        }
        2 => {
            let st: i32 = top.iter().sum();
            let sl: i32 = left.iter().sum();
            let dc = match (a.top, a.left) {
                (true, true) => (st + sl + 16) >> 5,
                (false, true) => (sl + 8) >> 4,
                (true, false) => (st + 8) >> 4,
                _ => mid::<T>(max),
            };
            for r in 0..16 {
                plane[base + r * stride..base + r * stride + 16].fill(T::from_i32(dc));
            }
        }
        _ => {
            let tl = if a.top_left { plane[(y - 1) * stride + x - 1].to_i32() } else { mid::<T>(max) };
            let t = |i: i32| if i < 0 { tl } else { top[i as usize] };
            let l = |i: i32| if i < 0 { tl } else { left[i as usize] };
            let mut h = 0;
            let mut v = 0;
            for k in 0..8 {
                h += (k + 1) * (t(8 + k) - t(6 - k));
                v += (k + 1) * (l(8 + k) - l(6 - k));
            }
            let aa = 16 * (left[15] + top[15]);
            let b = (5 * h + 32) >> 6;
            let c = (5 * v + 32) >> 6;
            for r in 0..16 {
                for cc in 0..16 {
                    plane[base + r * stride + cc] = T::clip((aa + b * (cc as i32 - 7) + c * (r as i32 - 7) + 16) >> 5, max);
                }
            }
        }
    }
}

/// Chroma intra prediction (8.3.4) for the macroblock whose chroma block (8x8 for 4:2:0, 8x16
/// for 4:2:2) starts at chroma (x, y). Modes: 0 DC, 1 horizontal, 2 vertical, 3 plane.
#[allow(clippy::too_many_arguments)]
pub fn pred_chroma<T: Sample>(plane: &mut [T], stride: usize, x: usize, y: usize, mode: u8, a: Avail, max: i32, c422: bool) {
    const W: usize = 8;
    let h: usize = if c422 { 16 } else { 8 };
    let dflt = mid::<T>(max);
    let mut top = [0i32; W];
    let mut left = [0i32; 16];
    if a.top {
        for i in 0..W {
            top[i] = plane[(y - 1) * stride + x + i].to_i32();
        }
    }
    if a.left {
        for i in 0..h {
            left[i] = plane[(y + i) * stride + x - 1].to_i32();
        }
    }
    let base = y * stride + x;
    match mode {
        0 => {
            // 8.3.4.1-8.3.4.3: per 4x4 chroma block at (xO, yO)
            for by in 0..h / 4 {
                for bx in 0..W / 4 {
                    let st: i32 = top[bx * 4..bx * 4 + 4].iter().sum();
                    let sl: i32 = left[by * 4..by * 4 + 4].iter().sum();
                    let dc = if (bx == 0 && by == 0) || (bx > 0 && by > 0) {
                        match (a.top, a.left) {
                            (true, true) => (st + sl + 4) >> 3,
                            (false, true) => (sl + 2) >> 2,
                            (true, false) => (st + 2) >> 2,
                            _ => dflt,
                        }
                    } else if bx > 0 {
                        if a.top {
                            (st + 2) >> 2
                        } else if a.left {
                            (sl + 2) >> 2
                        } else {
                            dflt
                        }
                    } else if a.left {
                        (sl + 2) >> 2
                    } else if a.top {
                        (st + 2) >> 2
                    } else {
                        dflt
                    };
                    for r in 0..4 {
                        let o = base + (by * 4 + r) * stride + bx * 4;
                        plane[o..o + 4].fill(T::from_i32(dc));
                    }
                }
            }
        }
        1 => {
            for r in 0..h {
                plane[base + r * stride..base + r * stride + W].fill(T::from_i32(left[r]));
            }
        }
        2 => {
            for r in 0..h {
                for c in 0..W {
                    plane[base + r * stride + c] = T::from_i32(top[c]);
                }
            }
        }
        _ => {
            // xCF = 0; yCF = 4 for 4:2:2 (8-138 .. 8-144)
            let tl = if a.top_left { plane[(y - 1) * stride + x - 1].to_i32() } else { dflt };
            let t = |i: i32| if i < 0 { tl } else { top[i as usize] };
            let l = |i: i32| if i < 0 { tl } else { left[i as usize] };
            let ycf = if c422 { 4 } else { 0 };
            let mut hh = 0;
            let mut v = 0;
            for k in 0..4 {
                hh += (k + 1) * (t(4 + k) - t(2 - k));
            }
            for k in 0..4 + ycf {
                v += (k + 1) * (l(4 + ycf + k) - l(2 + ycf - k));
            }
            let aa = 16 * (left[h - 1] + top[W - 1]);
            let b = (34 * hh + 32) >> 6;
            let c = ((if c422 { 5 } else { 34 }) * v + 32) >> 6;
            for r in 0..h {
                for cc in 0..W {
                    plane[base + r * stride + cc] = T::clip((aa + b * (cc as i32 - 3) + c * (r as i32 - 3 - ycf) + 16) >> 5, max);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Vec<u8>, usize) {
        // 12x12 plane; block at (4,4); top row y=3 = 10,20,..; left column x=3 = 1,2,3,...
        let stride = 16;
        let mut p = vec![0u8; stride * 16];
        for i in 0..12 {
            p[3 * stride + 4 + i] = (10 * (i + 1)) as u8;
        }
        for i in 0..8 {
            p[(4 + i) * stride + 3] = (i + 1) as u8;
        }
        p[3 * stride + 3] = 5;
        (p, stride)
    }
    const ALL: Avail = Avail { left: true, top: true, top_left: true, top_right: true };

    #[test]
    fn i4_vertical_horizontal_dc() {
        let (mut p, s) = setup();
        pred4x4(&mut p, s, 4, 4, 0, ALL, 255);
        assert_eq!(&p[4 * s + 4..4 * s + 8], &[10, 20, 30, 40]);
        assert_eq!(&p[7 * s + 4..7 * s + 8], &[10, 20, 30, 40]);
        pred4x4(&mut p, s, 4, 4, 1, ALL, 255);
        assert_eq!(&p[5 * s + 4..5 * s + 8], &[2, 2, 2, 2]);
        pred4x4(&mut p, s, 4, 4, 2, ALL, 255);
        // (10+20+30+40 + 1+2+3+4 + 4) >> 3 = 114 >> 3 = 14
        assert_eq!(p[4 * s + 4], 14);
    }

    #[test]
    fn i4_diag_down_left_hand_computed() {
        let (mut p, s) = setup();
        pred4x4(&mut p, s, 4, 4, 3, ALL, 255);
        // pred[0,0] = (t0 + 2 t1 + t2 + 2) >> 2 = (10 + 40 + 30 + 2) >> 2 = 20
        assert_eq!(p[4 * s + 4], 20);
        // pred[3,3] = (t6 + 3 t7 + 2) >> 2 = (70 + 240 + 2) >> 2 = 78
        assert_eq!(p[7 * s + 7], 78);
    }

    #[test]
    fn i4_diag_down_right_hand_computed() {
        let (mut p, s) = setup();
        pred4x4(&mut p, s, 4, 4, 4, ALL, 255);
        // x==y: (t0 + 2 tl + l0 + 2) >> 2 = (10 + 10 + 1 + 2) >> 2 = 5
        assert_eq!(p[4 * s + 4], 5);
        // x=1,y=0: (tl + 2 t0 + t1 + 2)>>2 = (5 + 20 + 20 + 2) >> 2 = 11
        assert_eq!(p[4 * s + 5], 11);
        // x=0,y=1: (tl + 2 l0 + l1 + 2) >> 2 = (5 + 2 + 2 + 2) >> 2 = 2
        assert_eq!(p[5 * s + 4], 2);
    }

    #[test]
    fn i4_top_right_substitution() {
        let (mut p, s) = setup();
        let a = Avail { top_right: false, ..ALL };
        pred4x4(&mut p, s, 4, 4, 3, a, 255);
        // top-right replaced by t3 = 40: pred[3,3] = (40 + 120 + 2) >> 2 = 40
        assert_eq!(p[7 * s + 7], 40);
    }

    #[test]
    fn i16_plane_flat_is_flat() {
        let s = 32;
        let mut p = vec![77u8; s * 32];
        pred16x16(&mut p, s, 8, 8, 3, ALL, 255);
        assert!(p[8 * s + 8..8 * s + 24].iter().all(|&v| v == 77));
    }

    #[test]
    fn chroma_dc_rules() {
        let s = 16;
        let mut p = vec![0u8; s * 16];
        for i in 0..8 {
            p[3 * s + 4 + i] = if i < 4 { 40 } else { 80 };
            p[(4 + i) * s + 3] = if i < 4 { 8 } else { 16 };
        }
        pred_chroma(&mut p, s, 4, 4, 0, ALL, 255, false);
        assert_eq!(p[4 * s + 4], (160 + 32 + 4) >> 3); // top-left block: both
        assert_eq!(p[4 * s + 8], 80); // top-right block: top only
        assert_eq!(p[8 * s + 4], 16); // bottom-left: left only
        assert_eq!(p[8 * s + 8], ((320 + 64 + 4) >> 3) as u8); // bottom-right: both
    }

    #[test]
    fn chroma_422_dc_rules() {
        // 8x16 block: rows of 4x4 blocks at yO = 4, 8, 12 with xO = 0 prefer left, xO = 4 use both
        let s = 16;
        let mut p = vec![0u16; s * 24];
        for i in 0..8 {
            p[3 * s + 4 + i] = if i < 4 { 400 } else { 800 };
        }
        for i in 0..16 {
            p[(4 + i) * s + 3] = 10 * (i / 4 + 1) as u16;
        }
        pred_chroma(&mut p, s, 4, 4, 0, ALL, 1023, true);
        assert_eq!(p[4 * s + 4], (1600 + 40 + 4) >> 3); // (0, 0): both
        assert_eq!(p[4 * s + 8], 800); // (4, 0): top
        for by in 1..4 {
            let l = 4 * 10 * (by as u16 + 1);
            assert_eq!(p[(4 + by * 4) * s + 4], (l + 2) >> 2, "left-only block row {by}");
            assert_eq!(p[(4 + by * 4) * s + 8], (3200 + l + 4) >> 3, "both block row {by}");
        }
        // nothing available: 1 << (BitDepth - 1)
        pred_chroma(&mut p, s, 4, 4, 0, Avail::default(), 1023, true);
        assert!((0..16).all(|r| p[(4 + r) * s + 4..(4 + r) * s + 12].iter().all(|&v| v == 512)));
    }

    #[test]
    fn chroma_422_plane_follows_the_formula() {
        // neighbours of a ramp; the prediction must follow 8.3.4.4 with xCF = 0, yCF = 4
        let s = 16;
        let f = |x: i32, y: i32| (100 + 4 * x + 2 * y) as u16;
        let mut p = vec![0u16; s * 24];
        for x in -1..8 {
            p[(3 * s as i32 + 4 + x) as usize] = f(x, -1);
        }
        for y in 0..16 {
            p[((4 + y) * s as i32 + 3) as usize] = f(-1, y);
        }
        pred_chroma(&mut p, s, 4, 4, 3, ALL, 1023, true);
        let t = |x: i32| f(x, -1) as i32;
        let l = |y: i32| f(-1, y) as i32;
        let hh: i32 = (0..4).map(|k| (k + 1) * (t(4 + k) - t(2 - k))).sum();
        let vv: i32 = (0..8).map(|k| (k + 1) * (l(8 + k) - l(6 - k))).sum();
        let (b, c) = ((34 * hh + 32) >> 6, (5 * vv + 32) >> 6);
        let a = 16 * (l(15) + t(7));
        for y in 0..16 {
            for x in 0..8 {
                let want = ((a + b * (x - 3) + c * (y - 7) + 16) >> 5).clamp(0, 1023) as u16;
                assert_eq!(p[((4 + y) * s as i32 + 4 + x) as usize], want, "({x}, {y})");
            }
        }
    }

    #[test]
    fn high_bit_depth_unavailable_neighbours_are_mid_grey() {
        let s = 8;
        let mut p = vec![0u16; s * 8];
        pred4x4(&mut p, s, 0, 0, 2, Avail::default(), 1023);
        assert!(p[..4].iter().all(|&v| v == 512));
        let mut q = vec![0u16; 32 * 32];
        pred16x16(&mut q, 32, 0, 0, 2, Avail::default(), 1023);
        assert_eq!(q[0], 512);
    }
}
