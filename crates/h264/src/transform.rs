//! Scaling (dequantisation) and inverse transforms (8.5.10 - 8.5.13).

use crate::params::ScalingMatrices;
use crate::picture::Sample;
use crate::tables::{norm_adjust4, norm_adjust8};

/// LevelScale4x4 / LevelScale8x8 for all six scaling lists and qP % 6, raster order.
#[derive(Clone)]
pub struct LevelScale {
    /// [list 0..6 (IntraY, IntraCb, IntraCr, InterY, InterCb, InterCr)][qp % 6][pos]
    pub ls4: [[[i32; 16]; 6]; 6],
    /// [list 0..6 (IntraY, InterY, IntraCb, InterCb, IntraCr, InterCr)][qp % 6][pos]
    pub ls8: [[[i32; 64]; 6]; 6],
}

impl LevelScale {
    pub fn new(m: &ScalingMatrices) -> Box<Self> {
        let mut s = Box::new(LevelScale { ls4: [[[0; 16]; 6]; 6], ls8: [[[0; 64]; 6]; 6] });
        for l in 0..6 {
            for q in 0..6 {
                for p in 0..16 {
                    s.ls4[l][q][p] = m.m4[l][p] as i32 * norm_adjust4(q, p) as i32;
                }
                for p in 0..64 {
                    s.ls8[l][q][p] = m.m8[l][p] as i32 * norm_adjust8(q, p) as i32;
                }
            }
        }
        s
    }
}

/// Scale one 4x4 AC coefficient (8.5.12.1, not the DC of Intra16x16 / chroma blocks).
#[inline(always)]
pub fn scale4(c: i32, ls: i32, qp: i32) -> i32 {
    if qp >= 24 { c.wrapping_mul(ls) << (qp / 6 - 4) } else { c.wrapping_mul(ls).wrapping_add(1 << (3 - qp / 6)) >> (4 - qp / 6) }
}

/// Scale one 8x8 coefficient (8.5.13.1).
#[inline(always)]
pub fn scale8(c: i32, ls: i32, qp: i32) -> i32 {
    if qp >= 36 { c.wrapping_mul(ls) << (qp / 6 - 6) } else { c.wrapping_mul(ls).wrapping_add(1 << (5 - qp / 6)) >> (6 - qp / 6) }
}

/// Intra16x16 luma DC: inverse Hadamard + scaling (8.5.10). `c` is raster order (row*4+col);
/// output replaces `c` with dcY (raster: row i -> 4x4 block row, col j -> block column).
pub fn luma_dc_dequant(c: &mut [i32; 16], qp: i32, ls00: i32) {
    let mut t = [0i32; 16];
    // rows
    for i in 0..4 {
        let r = &c[i * 4..i * 4 + 4];
        let s01 = r[0] + r[1];
        let d01 = r[0] - r[1];
        let s23 = r[2] + r[3];
        let d23 = r[2] - r[3];
        t[i * 4] = s01 + s23;
        t[i * 4 + 1] = s01 - s23;
        t[i * 4 + 2] = d01 - d23;
        t[i * 4 + 3] = d01 + d23;
    }
    // columns
    for j in 0..4 {
        let s01 = t[j] + t[4 + j];
        let d01 = t[j] - t[4 + j];
        let s23 = t[8 + j] + t[12 + j];
        let d23 = t[8 + j] - t[12 + j];
        let f = [s01 + s23, s01 - s23, d01 - d23, d01 + d23];
        for i in 0..4 {
            c[i * 4 + j] =
                if qp >= 36 { f[i].wrapping_mul(ls00) << (qp / 6 - 6) } else { f[i].wrapping_mul(ls00).wrapping_add(1 << (5 - qp / 6)) >> (6 - qp / 6) };
        }
    }
}

/// 4:2:0 chroma DC: 2x2 transform + scaling (8.5.11). `c` = [c00, c01, c10, c11].
pub fn chroma_dc_dequant_420(c: &mut [i32; 4], qp: i32, ls00: i32) {
    let s0 = c[0].wrapping_add(c[1]);
    let d0 = c[0].wrapping_sub(c[1]);
    let s1 = c[2].wrapping_add(c[3]);
    let d1 = c[2].wrapping_sub(c[3]);
    let f = [s0.wrapping_add(s1), d0.wrapping_add(d1), s0.wrapping_sub(s1), d0.wrapping_sub(d1)];
    for k in 0..4 {
        c[k] = (f[k].wrapping_mul(ls00) << (qp / 6)) >> 5;
    }
}

/// 4:2:2 chroma DC (8.5.11): 2x4 transform + scaling. `c` holds chromaDCLevel in parsing order;
/// on return `c[blkIdx]` is dcC of chroma 4x4 block `blkIdx` (raster, two blocks per row).
/// `qp_dc` is qP,DC = QP'c + 3 and `ls00` LevelScale4x4(qP,DC % 6, 0, 0).
pub fn chroma_dc_dequant_422(c: &mut [i32; 8], qp_dc: i32, ls00: i32) {
    // 8-330: c (4 rows x 2 columns) = [c0 c2; c1 c5; c3 c6; c4 c7]
    let m = [[c[0], c[2]], [c[1], c[5]], [c[3], c[6]], [c[4], c[7]]];
    // f = A * m * B, A = [1 1 1 1; 1 1 -1 -1; 1 -1 -1 1; 1 -1 1 -1], B = [1 1; 1 -1]
    let mut f = [[0i32; 2]; 4];
    for j in 0..2 {
        let (a, b, cc, d) = (m[0][j], m[1][j], m[2][j], m[3][j]);
        let col = [
            a.wrapping_add(b).wrapping_add(cc).wrapping_add(d),
            a.wrapping_add(b).wrapping_sub(cc).wrapping_sub(d),
            a.wrapping_sub(b).wrapping_sub(cc).wrapping_add(d),
            a.wrapping_sub(b).wrapping_add(cc).wrapping_sub(d),
        ];
        for i in 0..4 {
            f[i][j] = col[i];
        }
    }
    for i in 0..4 {
        let (x, y) = (f[i][0], f[i][1]);
        f[i] = [x.wrapping_add(y), x.wrapping_sub(y)];
    }
    for i in 0..4 {
        for j in 0..2 {
            let v = f[i][j].wrapping_mul(ls00);
            c[i * 2 + j] = if qp_dc >= 36 { v << (qp_dc / 6 - 6) } else { v.wrapping_add(1 << (5 - qp_dc / 6)) >> (6 - qp_dc / 6) };
        }
    }
}

/// Inverse 4x4 transform of raster-order `d` and add to `dst` (8.5.12.2, 8.5.14).
///
/// Rows first, then columns (the spec's order: the `>> 1` steps make it matter). The column
/// pass and the reconstruction run across all columns at once so they vectorise.
pub fn idct4_add<T: Sample>(d: &[i32; 16], dst: &mut [T], stride: usize, max: i32) {
    let mut t = [[0i32; 4]; 4];
    for (i, row) in t.iter_mut().enumerate() {
        let r = &d[i * 4..i * 4 + 4];
        let e = r[0] + r[2];
        let f = r[0] - r[2];
        let g = (r[1] >> 1) - r[3];
        let h = r[1] + (r[3] >> 1);
        *row = [e + h, f + g, f - g, e - h];
    }
    let mut o = [[0i32; 4]; 4];
    for j in 0..4 {
        let e = t[0][j] + t[2][j];
        let f = t[0][j] - t[2][j];
        let g = (t[1][j] >> 1) - t[3][j];
        let h = t[1][j] + (t[3][j] >> 1);
        o[0][j] = e + h;
        o[1][j] = f + g;
        o[2][j] = f - g;
        o[3][j] = e - h;
    }
    for (i, row) in o.iter().enumerate() {
        let Some(line) = dst.get_mut(i * stride..).and_then(|d| d.first_chunk_mut::<4>()) else {
            return;
        };
        for (p, &v) in line.iter_mut().zip(row) {
            *p = T::clip(p.to_i32() + ((v + 32) >> 6), max);
        }
    }
}

/// DC-only shortcut: all AC coefficients zero.
#[allow(dead_code)]
pub fn idct4_dc_add<T: Sample>(dc: i32, dst: &mut [T], stride: usize, max: i32) {
    let v = (dc + 32) >> 6;
    for i in 0..4 {
        for p in &mut dst[i * stride..i * stride + 4] {
            *p = T::clip(p.to_i32() + v, max);
        }
    }
}

#[inline(always)]
fn idct8_1d(d: [i32; 8]) -> [i32; 8] {
    let a0 = d[0] + d[4];
    let a4 = d[0] - d[4];
    let a2 = (d[2] >> 1) - d[6];
    let a6 = d[2] + (d[6] >> 1);
    let b0 = a0 + a6;
    let b2 = a4 + a2;
    let b4 = a4 - a2;
    let b6 = a0 - a6;
    let a1 = -d[3] + d[5] - d[7] - (d[7] >> 1);
    let a3 = d[1] + d[7] - d[3] - (d[3] >> 1);
    let a5 = -d[1] + d[7] + d[5] + (d[5] >> 1);
    let a7 = d[3] + d[5] + d[1] + (d[1] >> 1);
    let b1 = a1 + (a7 >> 2);
    let b7 = a7 - (a1 >> 2);
    let b3 = a3 + (a5 >> 2);
    let b5 = (a3 >> 2) - a5;
    [b0 + b7, b2 + b5, b4 + b3, b6 + b1, b6 - b1, b4 - b3, b2 - b5, b0 - b7]
}

/// Inverse 8x8 transform of raster-order `d` and add to `dst` (8.5.13.2): rows, then columns,
/// the column pass and reconstruction across all columns at once (they vectorise).
pub fn idct8_add<T: Sample>(d: &[i32; 64], dst: &mut [T], stride: usize, max: i32) {
    let mut t = [[0i32; 8]; 8];
    for (row, src) in t.iter_mut().zip(d.as_chunks::<8>().0) {
        *row = idct8_1d(*src);
    }
    let mut o = [[0i32; 8]; 8];
    for j in 0..8 {
        let col = idct8_1d(std::array::from_fn(|i| t[i][j]));
        for i in 0..8 {
            o[i][j] = col[i];
        }
    }
    for (i, row) in o.iter().enumerate() {
        let Some(line) = dst.get_mut(i * stride..).and_then(|d| d.first_chunk_mut::<8>()) else {
            return;
        };
        for (p, &v) in line.iter_mut().zip(row) {
            *p = T::clip(p.to_i32() + ((v + 32) >> 6), max);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The straightforward per-column formulation (8.5.12.2 / 8.5.13.2) the vectorised
    /// transforms must equal.
    fn reference_add(d: &[i32], n: usize, dst: &mut [u8], stride: usize) {
        let one = |v: &[i32]| -> Vec<i32> {
            if n == 4 {
                let (e, f, g, h) = (v[0] + v[2], v[0] - v[2], (v[1] >> 1) - v[3], v[1] + (v[3] >> 1));
                vec![e + h, f + g, f - g, e - h]
            } else {
                idct8_1d(v.try_into().unwrap()).to_vec()
            }
        };
        let rows: Vec<Vec<i32>> = (0..n).map(|i| one(&d[i * n..i * n + n])).collect();
        for j in 0..n {
            let col = one(&(0..n).map(|i| rows[i][j]).collect::<Vec<_>>());
            for i in 0..n {
                let p = &mut dst[i * stride + j];
                *p = (*p as i32 + ((col[i] + 32) >> 6)).clamp(0, 255) as u8;
            }
        }
    }

    #[test]
    fn vectorised_transforms_match_reference() {
        let mut rng = 0x1234_5678_9abc_def1u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for iter in 0..5000 {
            let range = [64u64, 1024, 8192, 1 << 15][iter % 4];
            let d4: [i32; 16] = std::array::from_fn(|_| (next() % (2 * range + 1)) as i32 - range as i32);
            let d8: [i32; 64] = std::array::from_fn(|_| (next() % (2 * range + 1)) as i32 - range as i32);
            let base: Vec<u8> = (0..24 * 8).map(|_| next() as u8).collect();
            let (mut a, mut b) = (base.clone(), base.clone());
            idct4_add(&d4, &mut a[3..], 24, 255);
            reference_add(&d4, 4, &mut b[3..], 24);
            assert_eq!(a, b, "4x4 iteration {iter}");
            let (mut a, mut b) = (base.clone(), base);
            idct8_add(&d8, &mut a[5..], 24, 255);
            reference_add(&d8, 8, &mut b[5..], 24);
            assert_eq!(a, b, "8x8 iteration {iter}");
        }
    }

    #[test]
    fn idct4_dc_only_matches_shortcut() {
        let mut d = [0i32; 16];
        d[0] = 640; // -> +10 everywhere
        let mut a = [100u8; 16];
        let mut b = [100u8; 16];
        idct4_add(&d, &mut a, 4, 255);
        idct4_dc_add(640, &mut b, 4, 255);
        assert_eq!(a, b);
        assert!(a.iter().all(|&v| v == 110));
    }

    #[test]
    fn idct4_hand_example() {
        // d = [[64, 64, 0, 0], 0...]: row0: e=64,f=64,g=32,h=64 -> [128, 96, 32, 0]; columns: each col only row0 set
        // col j: e=f=t0j, g=0, h=0 -> all rows = t0j. Result (t+32)>>6 = [2, 2, 1, 0] for every row.
        let mut d = [0i32; 16];
        d[0] = 64;
        d[1] = 64;
        let mut out = [0u8; 16];
        idct4_add(&d, &mut out, 4, 255);
        for i in 0..4 {
            assert_eq!(&out[i * 4..i * 4 + 4], &[2, 2, 1, 0]);
        }
    }

    #[test]
    fn idct8_dc() {
        let mut d = [0i32; 64];
        d[0] = 64 * 5;
        let mut out = [10u8; 64];
        idct8_add(&d, &mut out, 8, 255);
        assert!(out.iter().all(|&v| v == 15));
    }

    #[test]
    fn luma_dc_flat() {
        // single DC coefficient spreads equally to all 16 blocks
        let mut c = [0i32; 16];
        c[0] = 4;
        luma_dc_dequant(&mut c, 36, 16 * 16);
        assert!(c.iter().all(|&v| v == 4 * 256));
    }

    #[test]
    fn chroma_dc() {
        let mut c = [1, 0, 0, 0];
        chroma_dc_dequant_420(&mut c, 6, 16 * 16);
        // f = [1,1,1,1] -> ((1*256) << 1) >> 5 = 16
        assert_eq!(c, [16, 16, 16, 16]);
    }

    #[test]
    fn scale_functions() {
        assert_eq!(scale4(2, 160, 24), 2 * 160);
        assert_eq!(scale4(2, 160, 12), (2 * 160 + 2) >> 2);
        assert_eq!(scale8(1, 320, 36), 320);
        assert_eq!(scale8(1, 320, 30), (320 + 1) >> 1);
    }

    /// Corrupt streams can carry huge coefficient levels: dequantisation wraps (as release
    /// builds always did) instead of panicking with "attempt to multiply with overflow" in
    /// debug builds (found by mutation fuzzing an H.264 transport stream).
    #[test]
    fn huge_levels_dequantise_without_overflow_panics() {
        let mut c = [i32::MAX / 2, i32::MAX / 3, -i32::MAX / 2, 7];
        chroma_dc_dequant_420(&mut c, 51, 16 * 25);
        let _ = scale4(i32::MAX, 16 * 25, 51);
        let _ = scale4(i32::MAX, 16 * 25, 0);
        let _ = scale8(i32::MIN, 16 * 25, 51);
        let _ = scale8(i32::MIN, 16 * 25, 0);
    }

    #[test]
    fn chroma_dc_422_flat_and_placement() {
        // a lone c0 (the DC of the 2x4 matrix) spreads equally over all eight blocks:
        // f = 1 everywhere -> qP,DC = 36: (1 * ls) << 0
        let mut c = [1, 0, 0, 0, 0, 0, 0, 0];
        chroma_dc_dequant_422(&mut c, 36, 16 * 16);
        assert_eq!(c, [256; 8]);
        // c2 sits at row 0, column 1 of the matrix: the horizontal basis function, +1 in the
        // left block column and -1 in the right one
        let mut c = [0, 0, 1, 0, 0, 0, 0, 0];
        chroma_dc_dequant_422(&mut c, 36, 16);
        assert_eq!(c, [16, -16, 16, -16, 16, -16, 16, -16]);
        // c1 sits at row 1, column 0: the first vertical basis function (rows +, +, -, -)
        let mut c = [0, 1, 0, 0, 0, 0, 0, 0];
        chroma_dc_dequant_422(&mut c, 36, 16);
        assert_eq!(c, [16, 16, 16, 16, -16, -16, -16, -16]);
        // below qP 36 the result is rounded down: (f * ls + 2^(5 - qP/6)) >> (6 - qP/6)
        let mut c = [3, 0, 0, 0, 0, 0, 0, 0];
        chroma_dc_dequant_422(&mut c, 15, 13);
        assert_eq!(c, [(3 * 13 + 8) >> 4; 8]);
        // high-bit-depth QPs reach qP,DC = 63 + 3 without overflow panics
        let mut c = [i32::MAX / 3, 7, -5, 0, 1, 2, 3, i32::MIN / 3];
        chroma_dc_dequant_422(&mut c, 66, 16 * 25);
    }

    #[test]
    fn high_bit_depth_reconstruction_clips_to_the_depth() {
        let mut d = [0i32; 16];
        d[0] = 64 * 600;
        let mut out = [600u16; 16];
        idct4_add(&d, &mut out, 4, 1023);
        assert!(out.iter().all(|&v| v == 1023));
        let mut d8 = [0i32; 64];
        d8[0] = -64 * 50;
        let mut out8 = [700u16; 64];
        idct8_add(&d8, &mut out8, 8, 1023);
        assert!(out8.iter().all(|&v| v == 650));
    }
}
