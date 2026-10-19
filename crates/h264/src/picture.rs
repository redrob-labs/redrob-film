//! Picture buffers and per-macroblock state.
//!
//! Samples are stored as `u8` for 8-bit sequences and as `u16` for 9- and 10-bit ones
//! ([`Sample`]); everything that touches samples is generic over the sample type, so the 8-bit
//! code is the same machine code it was before high bit depths existed.

use std::sync::{Arc, OnceLock};

/// Sample format of a coded video sequence (7.4.2.1.1): chroma format and bit depths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// chroma_format_idc 2 (4:2:2: chroma has the full luma height); otherwise 4:2:0.
    pub c422: bool,
    pub bit_depth_y: u8,
    pub bit_depth_c: u8,
}

impl Format {
    pub const YUV420_8: Format = Format { c422: false, bit_depth_y: 8, bit_depth_c: 8 };

    /// MbHeightC: chroma lines per macroblock (8 for 4:2:0, 16 for 4:2:2).
    #[inline(always)]
    pub fn mbh_c(self) -> usize {
        if self.c422 { 16 } else { 8 }
    }
    /// log2(MbHeightC).
    #[inline(always)]
    pub fn mbh_c_log2(self) -> u32 {
        if self.c422 { 4 } else { 3 }
    }
    /// Largest luma sample value, (1 << BitDepthY) - 1.
    #[inline(always)]
    pub fn max_y(self) -> i32 {
        (1 << self.bit_depth_y) - 1
    }
    /// Largest chroma sample value, (1 << BitDepthC) - 1.
    #[inline(always)]
    pub fn max_c(self) -> i32 {
        (1 << self.bit_depth_c) - 1
    }
    /// QpBdOffsetY = 6 * bit_depth_luma_minus8.
    #[inline(always)]
    pub fn qp_bd_offset_y(self) -> i32 {
        6 * (self.bit_depth_y as i32 - 8)
    }
    /// QpBdOffsetC = 6 * bit_depth_chroma_minus8.
    #[inline(always)]
    pub fn qp_bd_offset_c(self) -> i32 {
        6 * (self.bit_depth_c as i32 - 8)
    }
    /// Samples need more than 8 bits (stored as `u16`).
    #[inline(always)]
    pub fn high(self) -> bool {
        self.bit_depth_y > 8 || self.bit_depth_c > 8
    }
}

/// A sample type: `u8` (8-bit sequences) or `u16` (9- and 10-bit sequences).
pub trait Sample: Copy + Default + PartialEq + Send + Sync + std::fmt::Debug + 'static {
    /// The upper bound Clip1 uses for a plane whose largest value is `max`. Always 255 for `u8`
    /// (only 8-bit sequences use it), so the 8-bit code keeps its constant bounds.
    fn clip_max(max: i32) -> i32;
    /// `v`, which is already within range.
    fn from_i32(v: i32) -> Self;
    fn to_i32(self) -> i32;
    /// Clip1: `v` clipped to 0..=max.
    #[inline(always)]
    fn clip(v: i32, max: i32) -> Self {
        Self::from_i32(v.clamp(0, Self::clip_max(max)))
    }
    /// (a + b + 1) >> 1.
    fn avg(a: Self, b: Self) -> Self;
    /// Chroma bilinear interpolation (8.4.2.2.2): (w0 A + w1 B + w2 C + w3 D + 32) >> 6, with
    /// weights that sum to 64.
    fn bilin(w: [u16; 4], a: Self, b: Self, c: Self, d: Self) -> Self;
    /// An empty output plane buffer with room for `n` samples.
    fn alloc(n: usize) -> Vec<Self>;
    /// Load samples p3 .. q3 (k = 0..8) of `N` lines across a vertical edge; line `i` starts at
    /// `pix[o + i * stride]` (its p3). `None` when the window leaves the plane.
    fn load_lines<const N: usize>(pix: &[Self], stride: usize, o: usize) -> Option<[[i16; N]; 8]>;
    /// Store what [`Sample::load_lines`] loaded (values already in range).
    fn store_lines<const N: usize>(pix: &mut [Self], stride: usize, o: usize, s: &[[i16; N]; 8]);
    fn any_frame(f: Arc<Frame<Self>>) -> AnyFrame;
    fn frame_of(f: &AnyFrame) -> Option<&Arc<Frame<Self>>>;
}

impl Sample for u8 {
    #[inline(always)]
    fn clip_max(_max: i32) -> i32 {
        255
    }
    #[inline(always)]
    fn from_i32(v: i32) -> Self {
        v as u8
    }
    #[inline(always)]
    fn to_i32(self) -> i32 {
        self as i32
    }
    #[inline(always)]
    fn avg(a: Self, b: Self) -> Self {
        ((a as u16 + b as u16 + 1) >> 1) as u8
    }
    #[inline(always)]
    fn bilin(w: [u16; 4], a: Self, b: Self, c: Self, d: Self) -> Self {
        let v = w[0] * a as u16 + w[1] * b as u16 + w[2] * c as u16 + w[3] * d as u16;
        ((v + 32) >> 6) as u8
    }
    fn alloc(n: usize) -> Vec<Self> {
        plane_allocator()(n)
    }
    #[inline(always)]
    fn load_lines<const N: usize>(pix: &[Self], stride: usize, o: usize) -> Option<[[i16; N]; 8]> {
        // Each line's eight samples as one little-endian word: byte k = sample k (p3 .. q3).
        let mut w = [0u64; N];
        for (i, w) in w.iter_mut().enumerate() {
            let &b = pix.get(o + i * stride..).and_then(|p| p.first_chunk::<8>())?;
            *w = u64::from_le_bytes(b);
        }
        let mut s = [[0i16; N]; 8];
        for (k, row) in s.iter_mut().enumerate() {
            for i in 0..N {
                row[i] = ((w[i] >> (8 * k)) & 0xff) as i16;
            }
        }
        Some(s)
    }
    #[inline(always)]
    fn store_lines<const N: usize>(pix: &mut [Self], stride: usize, o: usize, s: &[[i16; N]; 8]) {
        let mut w = [0u64; N];
        for (i, w) in w.iter_mut().enumerate() {
            *w = (0..8).fold(0u64, |acc, k| acc | ((s[k][i] as u8 as u64) << (8 * k)));
        }
        for (i, w) in w.iter().enumerate() {
            let p = o + i * stride;
            pix[p..p + 8].copy_from_slice(&w.to_le_bytes());
        }
    }
    fn any_frame(f: Arc<Frame<Self>>) -> AnyFrame {
        AnyFrame::U8(f)
    }
    fn frame_of(f: &AnyFrame) -> Option<&Arc<Frame<Self>>> {
        match f {
            AnyFrame::U8(f) => Some(f),
            AnyFrame::U16(_) => None,
        }
    }
}

impl Sample for u16 {
    #[inline(always)]
    fn clip_max(max: i32) -> i32 {
        max
    }
    #[inline(always)]
    fn from_i32(v: i32) -> Self {
        v as u16
    }
    #[inline(always)]
    fn to_i32(self) -> i32 {
        self as i32
    }
    #[inline(always)]
    fn avg(a: Self, b: Self) -> Self {
        ((a as u32 + b as u32 + 1) >> 1) as u16
    }
    #[inline(always)]
    fn bilin(w: [u16; 4], a: Self, b: Self, c: Self, d: Self) -> Self {
        let v = w[0] as u32 * a as u32 + w[1] as u32 * b as u32 + w[2] as u32 * c as u32 + w[3] as u32 * d as u32;
        ((v + 32) >> 6) as u16
    }
    fn alloc(n: usize) -> Vec<Self> {
        plane_allocator_u16()(n)
    }
    #[inline(always)]
    fn load_lines<const N: usize>(pix: &[Self], stride: usize, o: usize) -> Option<[[i16; N]; 8]> {
        let mut s = [[0i16; N]; 8];
        for i in 0..N {
            let line = pix.get(o + i * stride..).and_then(|p| p.first_chunk::<8>())?;
            for k in 0..8 {
                s[k][i] = line[k] as i16;
            }
        }
        Some(s)
    }
    #[inline(always)]
    fn store_lines<const N: usize>(pix: &mut [Self], stride: usize, o: usize, s: &[[i16; N]; 8]) {
        for i in 0..N {
            let p = o + i * stride;
            for k in 0..8 {
                pix[p + k] = s[k][i] as u16;
            }
        }
    }
    fn any_frame(f: Arc<Frame<Self>>) -> AnyFrame {
        AnyFrame::U16(f)
    }
    fn frame_of(f: &AnyFrame) -> Option<&Arc<Frame<Self>>> {
        match f {
            AnyFrame::U16(f) => Some(f),
            AnyFrame::U8(_) => None,
        }
    }
}

/// Planar sample buffers (MB-aligned dimensions, stride == width): 4:2:0 or 4:2:2.
#[derive(Clone)]
pub struct Planes<T> {
    pub y: Vec<T>,
    pub cb: Vec<T>,
    pub cr: Vec<T>,
    pub width: usize,
    pub height: usize,
    pub cwidth: usize,
    pub cheight: usize,
}

impl<T: Sample> Planes<T> {
    pub fn new(width: usize, height: usize, fmt: Format) -> Self {
        let (cw, ch) = (width / 2, if fmt.c422 { height } else { height / 2 });
        let mid_c = T::from_i32((fmt.max_c() + 1) >> 1);
        Planes { y: vec![T::default(); width * height], cb: vec![mid_c; cw * ch], cr: vec![mid_c; cw * ch], width, height, cwidth: cw, cheight: ch }
    }
    pub fn gray(width: usize, height: usize, fmt: Format) -> Self {
        let mut p = Self::new(width, height, fmt);
        p.y.fill(T::from_i32((fmt.max_y() + 1) >> 1));
        p
    }
    /// MbHeightC of these planes.
    #[inline(always)]
    pub fn mbh_c(&self) -> usize {
        if self.cheight == self.height { 16 } else { 8 }
    }
}

/// Macroblock coding category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MbKind {
    #[default]
    None,
    I4x4,
    I8x8,
    I16x16,
    IPcm,
    PSkip,
    BSkip,
    BDirect16x16,
    /// Any other P or B inter macroblock.
    Inter,
}

impl MbKind {
    #[inline]
    pub fn is_intra(self) -> bool {
        matches!(self, MbKind::I4x4 | MbKind::I8x8 | MbKind::I16x16 | MbKind::IPcm)
    }
}

/// Per-macroblock state kept for the whole picture (neighbour derivations, deblocking, co-located data).
#[derive(Clone, Copy)]
pub struct MbState {
    /// Slice number within the picture; `u32::MAX` = not (yet) decoded.
    pub slice_num: u32,
    pub kind: MbKind,
    pub transform_8x8: bool,
    /// coded_block_pattern: bits 0..3 luma, bits 4..5 chroma.
    pub cbp: u8,
    /// QPY (negative down to -QpBdOffsetY for high bit depths).
    pub qp: i8,
    /// QPc for Cb / Cr (used by deblocking; may be negative like `qp`).
    pub qpc: [i8; 2],
    pub intra_chroma_mode: u8,
    /// Intra4x4PredMode / Intra8x8PredMode per 4x4 block (raster order).
    pub intra_modes: [u8; 16],
    /// CAVLC: TotalCoeff per luma 4x4 block (raster). CABAC: coded_block_flag.
    pub nnz: [u8; 16],
    /// Same for chroma AC blocks [Cb, Cr][raster 2 wide: 2x2 for 4:2:0, 2x4 for 4:2:2].
    pub nnz_c: [[u8; 8]; 2],
    /// CABAC coded_block_flag of DC blocks: bit0 luma (I16x16), bit1 Cb, bit2 Cr.
    pub cbf_dc: u8,
    /// Luma 4x4 blocks (raster bit) with non-zero coefficients, for deblocking bS = 2.
    pub nz_mask: u16,
    pub ref_idx: [[i8; 4]; 2],
    /// Motion vectors per 4x4 block (raster order).
    pub mv: [[[i16; 2]; 16]; 2],
    /// |mvd| per 4x4 block (raster), saturated, for CABAC context selection.
    pub mvd: [[[u8; 2]; 16]; 2],
    /// Bit per 8x8 block predicted in direct mode.
    pub direct8x8: u8,
}

impl MbState {
    #[inline]
    pub fn kind_is_skip(&self) -> bool {
        matches!(self.kind, MbKind::PSkip | MbKind::BSkip)
    }
}

impl Default for MbState {
    fn default() -> Self {
        MbState {
            slice_num: u32::MAX,
            kind: MbKind::None,
            transform_8x8: false,
            cbp: 0,
            qp: 0,
            qpc: [0; 2],
            intra_chroma_mode: 0,
            intra_modes: [2; 16],
            nnz: [0; 16],
            nnz_c: [[0; 8]; 2],
            cbf_dc: 0,
            nz_mask: 0,
            ref_idx: [[-1; 4]; 2],
            mv: [[[0; 2]; 16]; 2],
            mvd: [[[0; 2]; 16]; 2],
            direct8x8: 0,
        }
    }
}

/// One published macroblock row of a decoded frame: final (deblocked) samples plus the motion data
/// needed as co-located information for direct prediction.
pub struct FrameRow<T> {
    /// 16 luma lines.
    pub y: Box<[T]>,
    /// MbHeightC lines (8 for 4:2:0, 16 for 4:2:2) per chroma component.
    pub cb: Box<[T]>,
    pub cr: Box<[T]>,
    /// Per 4x4 block: index mb_x * 16 + raster.
    pub mv: [Box<[[i16; 2]]>; 2],
    /// Per 8x8 block: index mb_x * 4 + b8.
    pub ref_idx: [Box<[i8]>; 2],
    /// Unique id of the referenced picture per 8x8 block (u32::MAX = none).
    pub ref_id: [Box<[u32]>; 2],
    pub intra: Box<[bool]>,
}

/// A decoded (or in-progress) frame as stored in the DPB and referenced by later pictures.
///
/// Rows are published in order by the thread decoding the picture; readers block (per row) until the
/// data they need is available, which lets several pictures decode concurrently.
pub struct Frame<T> {
    pub id: u32,
    pub poc: i32,
    pub width: usize,
    pub height: usize,
    pub cwidth: usize,
    pub cheight: usize,
    pub mb_w: usize,
    pub fmt: Format,
    rows: Box<[OnceLock<FrameRow<T>>]>,
}

impl<T: Sample> Frame<T> {
    pub fn new(id: u32, poc: i32, mb_w: usize, mb_h: usize, fmt: Format) -> Self {
        Frame {
            id,
            poc,
            width: mb_w * 16,
            height: mb_h * 16,
            cwidth: mb_w * 8,
            cheight: mb_h * fmt.mbh_c(),
            mb_w,
            fmt,
            rows: (0..mb_h).map(|_| OnceLock::new()).collect(),
        }
    }

    pub fn mb_h(&self) -> usize {
        self.rows.len()
    }

    /// Publish MB row `r` (ignored if already published).
    pub fn publish(&self, r: usize, row: FrameRow<T>) {
        let _ = self.rows[r].set(row);
    }

    pub fn is_published(&self, r: usize) -> bool {
        self.rows[r].get().is_some()
    }

    /// MB row `r`, waiting until it has been published.
    #[inline]
    pub fn row(&self, r: usize) -> &FrameRow<T> {
        #[cfg(feature = "threads")]
        {
            self.rows[r].wait()
        }
        #[cfg(not(feature = "threads"))]
        {
            self.rows[r].get().expect("reference row decoded before use")
        }
    }

    /// Wait until the whole frame is available.
    pub fn wait_complete(&self) {
        for r in 0..self.rows.len() {
            self.row(r);
        }
    }

    /// Build a published row from a contiguous plane set (rows of MB row `r`) and motion data.
    pub fn make_row(planes: &Planes<T>, r: usize, mbs: &[MbState], ref_ids: &dyn Fn(&MbState, usize, i8) -> u32) -> FrameRow<T> {
        let w = planes.width;
        let cw = planes.cwidth;
        let ch = planes.mbh_c();
        let mb_w = w / 16;
        let y = planes.y[r * 16 * w..(r + 1) * 16 * w].into();
        let cb = planes.cb[r * ch * cw..(r + 1) * ch * cw].into();
        let cr = planes.cr[r * ch * cw..(r + 1) * ch * cw].into();
        let row_mbs = &mbs[r * mb_w..(r + 1) * mb_w];
        let mut mv = [vec![[0i16; 2]; mb_w * 16], vec![[0i16; 2]; mb_w * 16]];
        let mut ref_idx = [vec![-1i8; mb_w * 4], vec![-1i8; mb_w * 4]];
        let mut ref_id = [vec![u32::MAX; mb_w * 4], vec![u32::MAX; mb_w * 4]];
        let mut intra = vec![true; mb_w];
        for (x, st) in row_mbs.iter().enumerate() {
            if st.slice_num == u32::MAX {
                continue;
            }
            intra[x] = st.kind.is_intra();
            for l in 0..2 {
                mv[l][x * 16..x * 16 + 16].copy_from_slice(&st.mv[l]);
                for b in 0..4 {
                    let ri = st.ref_idx[l][b];
                    ref_idx[l][x * 4 + b] = ri;
                    if ri >= 0 {
                        ref_id[l][x * 4 + b] = ref_ids(st, l, ri);
                    }
                }
            }
        }
        let [mv0, mv1] = mv;
        let [ri0, ri1] = ref_idx;
        let [id0, id1] = ref_id;
        FrameRow { y, cb, cr, mv: [mv0.into(), mv1.into()], ref_idx: [ri0.into(), ri1.into()], ref_id: [id0.into(), id1.into()], intra: intra.into() }
    }

    /// A frame with every row published from `planes` and no motion (intra), e.g. "non-existing"
    /// frames inferred for frame_num gaps.
    pub fn from_planes(id: u32, poc: i32, planes: &Planes<T>, fmt: Format) -> Self {
        let (mb_w, mb_h) = (planes.width / 16, planes.height / 16);
        let f = Frame::new(id, poc, mb_w, mb_h, fmt);
        let blank = vec![MbState::default(); mb_w * mb_h];
        for r in 0..mb_h {
            f.publish(r, Frame::make_row(planes, r, &blank, &|_, _, _| u32::MAX));
        }
        f
    }

    /// Copy the w x h luma window at (x0, y0) (clamped to the picture) into `out` (stride `os`).
    #[inline]
    pub fn luma_window(&self, x0: i32, y0: i32, w: usize, h: usize, out: &mut [T], os: usize) {
        let (width, height) = (self.width, self.height);
        for r in 0..h {
            let yy = (y0 + r as i32).clamp(0, height as i32 - 1) as usize;
            let row = self.row(yy >> 4);
            let line = &row.y[(yy & 15) * width..(yy & 15) * width + width];
            copy_line::<T, 24>(line, x0, &mut out[r * os..], w);
        }
    }

    /// Copy the w x h window of chroma component `c` at (x0, y0) (clamped) into `out`.
    #[inline]
    pub fn chroma_window(&self, c: usize, x0: i32, y0: i32, w: usize, h: usize, out: &mut [T], os: usize) {
        let (cw, ch) = (self.cwidth, self.cheight);
        let sh = self.fmt.mbh_c_log2();
        let mask = (1usize << sh) - 1;
        for r in 0..h {
            let yy = (y0 + r as i32).clamp(0, ch as i32 - 1) as usize;
            let row = self.row(yy >> sh);
            let plane = if c == 0 { &row.cb } else { &row.cr };
            let line = &plane[(yy & mask) * cw..(yy & mask) * cw + cw];
            copy_line::<T, 16>(line, x0, &mut out[r * os..], w);
        }
    }

    /// Crop and copy the frame into planar buffers (waits for completion).
    pub fn copy_cropped(&self, crop: (usize, usize, usize, usize)) -> (Vec<T>, Vec<T>, Vec<T>) {
        let (cx, cy, cw, ch) = crop;
        let mut y = T::alloc(cw * ch);
        for r in cy..cy + ch {
            let row = self.row(r >> 4);
            y.extend_from_slice(&row.y[(r & 15) * self.width + cx..(r & 15) * self.width + cx + cw]);
        }
        let (ccy, cch) = if self.fmt.c422 { (cy, ch) } else { (cy / 2, ch.div_ceil(2)) };
        let (ccx, ccw) = (cx / 2, cw.div_ceil(2));
        let sh = self.fmt.mbh_c_log2();
        let mask = (1usize << sh) - 1;
        let mut u = T::alloc(ccw * cch);
        let mut v = T::alloc(ccw * cch);
        for r in ccy..ccy + cch {
            let row = self.row(r >> sh);
            let o = (r & mask) * self.cwidth + ccx;
            u.extend_from_slice(&row.cb[o..o + ccw]);
            v.extend_from_slice(&row.cr[o..o + ccw]);
        }
        (y, u, v)
    }
}

static PLANE_ALLOCATOR: std::sync::OnceLock<fn(usize) -> Vec<u8>> = std::sync::OnceLock::new();
static PLANE_ALLOCATOR_U16: std::sync::OnceLock<fn(usize) -> Vec<u16>> = std::sync::OnceLock::new();

/// Where output planes come from: an empty buffer with room for the given number of samples.
/// The host sets it once to recycle the planes of pictures it is done with; the first call wins.
pub fn set_plane_allocator(alloc: fn(usize) -> Vec<u8>) {
    let _ = PLANE_ALLOCATOR.set(alloc);
}

/// [`set_plane_allocator`] for the `u16` planes of high-bit-depth pictures.
pub fn set_plane_allocator_u16(alloc: fn(usize) -> Vec<u16>) {
    let _ = PLANE_ALLOCATOR_U16.set(alloc);
}

fn plane_allocator() -> fn(usize) -> Vec<u8> {
    PLANE_ALLOCATOR.get().copied().unwrap_or(Vec::with_capacity)
}

fn plane_allocator_u16() -> fn(usize) -> Vec<u16> {
    PLANE_ALLOCATOR_U16.get().copied().unwrap_or(Vec::with_capacity)
}

/// Copy `w` samples of `line` starting at x0 into `out`; when possible a fixed-size block of `N`
/// samples is copied instead (cheaper than a variable-length copy; `out` must then have room for `N`).
#[inline(always)]
fn copy_line<T: Copy, const N: usize>(line: &[T], x0: i32, out: &mut [T], w: usize) {
    if w <= N
        && x0 >= 0
        && let Some(src) = line.get(x0 as usize..).and_then(|l| l.first_chunk::<N>())
        && let Some(dst) = out.first_chunk_mut::<N>()
    {
        *dst = *src;
    } else {
        copy_clamped(line, x0, &mut out[..w]);
    }
}

/// Copy `out.len()` samples of `line` starting at x0, replicating edge samples outside the line.
#[inline(always)]
fn copy_clamped<T: Copy>(line: &[T], x0: i32, out: &mut [T]) {
    let w = out.len();
    if x0 >= 0 && x0 as usize + w <= line.len() {
        out.copy_from_slice(&line[x0 as usize..x0 as usize + w]);
    } else {
        let max = line.len() as i32 - 1;
        for (c, o) in out.iter_mut().enumerate() {
            *o = line[(x0 + c as i32).clamp(0, max) as usize];
        }
    }
}

pub type FrameRef<T> = Arc<Frame<T>>;

/// A frame of either sample type, as the DPB stores it (a sequence uses one sample type; the DPB
/// is emptied when the format changes).
#[derive(Clone)]
pub enum AnyFrame {
    U8(Arc<Frame<u8>>),
    U16(Arc<Frame<u16>>),
}

impl AnyFrame {
    pub fn id(&self) -> u32 {
        match self {
            AnyFrame::U8(f) => f.id,
            AnyFrame::U16(f) => f.id,
        }
    }
    pub fn poc(&self) -> i32 {
        match self {
            AnyFrame::U8(f) => f.poc,
            AnyFrame::U16(f) => f.poc,
        }
    }
    pub fn mb_w(&self) -> usize {
        match self {
            AnyFrame::U8(f) => f.mb_w,
            AnyFrame::U16(f) => f.mb_w,
        }
    }
    pub fn mb_h(&self) -> usize {
        match self {
            AnyFrame::U8(f) => f.mb_h(),
            AnyFrame::U16(f) => f.mb_h(),
        }
    }
    pub fn fmt(&self) -> Format {
        match self {
            AnyFrame::U8(f) => f.fmt,
            AnyFrame::U16(f) => f.fmt,
        }
    }
    pub fn is_published(&self, r: usize) -> bool {
        match self {
            AnyFrame::U8(f) => f.is_published(r),
            AnyFrame::U16(f) => f.is_published(r),
        }
    }
    pub fn wait_complete(&self) {
        match self {
            AnyFrame::U8(f) => f.wait_complete(),
            AnyFrame::U16(f) => f.wait_complete(),
        }
    }
}

/// An entry of RefPicList0/1 for the current slice.
pub struct RefPic<T> {
    pub frame: FrameRef<T>,
    pub long_term: bool,
}

impl<T> Clone for RefPic<T> {
    fn clone(&self) -> Self {
        RefPic { frame: self.frame.clone(), long_term: self.long_term }
    }
}

impl<T> RefPic<T> {
    pub fn poc(&self) -> i32 {
        self.frame.poc
    }
    pub fn id(&self) -> u32 {
        self.frame.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u16_edge_lines_round_trip() {
        let stride = 12;
        let pix: Vec<u16> = (0..stride * 4).map(|i| (i * 37 % 1024) as u16).collect();
        let s = u16::load_lines::<4>(&pix, stride, 2).unwrap();
        for i in 0..4 {
            for k in 0..8 {
                assert_eq!(s[k][i] as u16, pix[2 + i * stride + k]);
            }
        }
        let mut out = vec![0u16; stride * 4];
        u16::store_lines::<4>(&mut out, stride, 2, &s);
        for i in 0..4 {
            assert_eq!(&out[2 + i * stride..2 + i * stride + 8], &pix[2 + i * stride..2 + i * stride + 8]);
        }
        assert!(u16::load_lines::<4>(&pix, stride, stride * 3).is_none());
    }

    #[test]
    fn frame_422_chroma_rows() {
        let fmt = Format { c422: true, bit_depth_y: 10, bit_depth_c: 10 };
        let mut p = Planes::<u16>::new(32, 32, fmt);
        assert_eq!((p.cwidth, p.cheight, p.mbh_c()), (16, 32, 16));
        assert!(p.cb.iter().all(|&v| v == 512));
        for (i, v) in p.cb.iter_mut().enumerate() {
            *v = i as u16;
        }
        let f = Frame::from_planes(1, 0, &p, fmt);
        let mut w = [0u16; 16 * 3];
        f.chroma_window(0, 0, 17, 4, 3, &mut w, 16);
        assert_eq!(&w[..4], &[17 * 16, 17 * 16 + 1, 17 * 16 + 2, 17 * 16 + 3]);
        let (_, u, _) = f.copy_cropped((0, 0, 32, 32));
        assert_eq!(u, p.cb);
    }
}
