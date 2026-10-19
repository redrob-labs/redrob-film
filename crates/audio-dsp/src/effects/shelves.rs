//! Single-purpose EQ effects: Low Shelf / High Shelf, All Pass, Tilt Shelf, Sub Cut (steep
//! high-pass below the audible band) and the Dynamic Equalizer (a peaking band whose gain follows
//! the level in a detection band).

use crate::biquad::{Biquad, Coeffs, FilterType};
use crate::{AudioEffect, ParamSpec, ParamValues, Smoothed, Unit, block_len, param_plumbing};

const SM: f32 = 20.0;
/// Coefficients of smoothed filters are recomputed every `GRID` samples of the effect's own
/// sample count, so the result does not depend on block sizes.
const GRID: u64 = 32;

fn sm(v: f32, sr: f32) -> Smoothed {
    Smoothed::with_ms(v, sr, SM)
}

fn grow(v: &mut Vec<Biquad>, n: usize) {
    if v.len() < n {
        v.resize(n, Biquad::new());
    }
}

// ------------------------------------------------------------------------------ shelves

/// `HIGH = false`: Low Shelf; `HIGH = true`: High Shelf (RBJ shelves, `q` = slope; Q 0.71 ≙ S 1).
pub struct Shelf<const HIGH: bool> {
    pv: ParamValues,
    sr: f32,
    st: Vec<Biquad>,
    c: Coeffs,
    freq: Smoothed,
    gain: Smoothed,
    mix: Smoothed,
    n: u64,
}

impl<const HIGH: bool> Shelf<HIGH> {
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::log("frequency", "Frequency", 20.0, 20000.0, if HIGH { 3000.0 } else { 100.0 }, Unit::Hertz),
        ParamSpec::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Decibels),
        ParamSpec::log("q", "Q", 0.1, 4.0, 0.71, Unit::Q),
        ParamSpec::new("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
    ];

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let mut s = Shelf {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            st: vec![Biquad::new(); channels.max(2)],
            c: Coeffs::IDENTITY,
            freq: sm(1000.0, sample_rate),
            gain: sm(0.0, sample_rate),
            mix: sm(1.0, sample_rate),
            n: 0,
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.freq.set(self.pv.v("frequency"));
        self.gain.set(self.pv.v("gain"));
        self.mix.set(self.pv.v("mix") / 100.0);
        if snap {
            for s in [&mut self.freq, &mut self.gain, &mut self.mix] {
                s.snap();
            }
            self.design();
        }
    }
    fn design(&mut self) {
        let kind = if HIGH { FilterType::HighShelf } else { FilterType::LowShelf };
        self.c = Coeffs::design(kind, self.sr as f64, self.freq.value() as f64, self.pv.v("q") as f64, self.gain.value() as f64);
    }
}

impl<const HIGH: bool> AudioEffect for Shelf<HIGH> {
    param_plumbing!(if HIGH { "high_shelf" } else { "low_shelf" });
    fn reset(&mut self) {
        self.st.iter_mut().for_each(Biquad::reset);
        for s in [&mut self.freq, &mut self.gain, &mut self.mix] {
            s.snap();
        }
        self.n = 0;
        self.design();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let len = block_len(channels);
        grow(&mut self.st, channels.len());
        for i in 0..len {
            self.freq.tick();
            self.gain.tick();
            let mix = self.mix.tick() as f64;
            if self.n.is_multiple_of(GRID) {
                self.design();
            }
            self.n += 1;
            for (c, ch) in channels.iter_mut().enumerate() {
                let x = ch[i] as f64;
                let y = self.st[c].tick(&self.c, x);
                ch[i] = (x + (y - x) * mix) as f32;
            }
        }
    }
    fn response_db(&self, freq: f64) -> Option<f64> {
        let kind = if HIGH { FilterType::HighShelf } else { FilterType::LowShelf };
        let c = Coeffs::design(kind, self.sr as f64, self.pv.v("frequency") as f64, self.pv.v("q") as f64, self.pv.v("gain") as f64);
        Some(c.magnitude_db(freq, self.sr as f64))
    }
}

// ------------------------------------------------------------------------------ all pass

/// All Pass: flat magnitude, the phase turns through 360° around `frequency` (`q` sets how fast).
/// Mixed with the dry signal it notches (comb-like phasing).
pub struct AllPassFilter {
    pv: ParamValues,
    sr: f32,
    st: Vec<Biquad>,
    c: Coeffs,
    mix: Smoothed,
}

impl AllPassFilter {
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::log("frequency", "Frequency", 20.0, 20000.0, 1000.0, Unit::Hertz),
        ParamSpec::log("q", "Q", 0.1, 20.0, 0.71, Unit::Q),
        ParamSpec::new("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
    ];

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let mut s = AllPassFilter {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            st: vec![Biquad::new(); channels.max(2)],
            c: Coeffs::IDENTITY,
            mix: sm(1.0, sample_rate),
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.c = Coeffs::design(FilterType::AllPass, self.sr as f64, self.pv.v("frequency") as f64, self.pv.v("q") as f64, 0.0);
        self.mix.set(self.pv.v("mix") / 100.0);
        if snap {
            self.mix.snap();
        }
    }
}

impl AudioEffect for AllPassFilter {
    param_plumbing!("allpass");
    fn reset(&mut self) {
        self.st.iter_mut().for_each(Biquad::reset);
        self.mix.snap();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let len = block_len(channels);
        grow(&mut self.st, channels.len());
        for i in 0..len {
            let mix = self.mix.tick() as f64;
            for (c, ch) in channels.iter_mut().enumerate() {
                let x = ch[i] as f64;
                let y = self.st[c].tick(&self.c, x);
                ch[i] = (x + (y - x) * mix) as f32;
            }
        }
    }
    fn response_db(&self, freq: f64) -> Option<f64> {
        // dry/wet sum: |1 − m + m·H|
        let c = self.c;
        let m = (self.pv.v("mix") / 100.0) as f64;
        let w = 2.0 * std::f64::consts::PI * freq / self.sr as f64;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let (nr, ni) = (c.b0 + c.b1 * c1 + c.b2 * c2, -(c.b1 * s1 + c.b2 * s2));
        let (dr, di) = (1.0 + c.a1 * c1 + c.a2 * c2, -(c.a1 * s1 + c.a2 * s2));
        let den = dr * dr + di * di;
        let (hr, hi) = ((nr * dr + ni * di) / den, (ni * dr - nr * di) / den);
        let (yr, yi) = (1.0 - m + m * hr, m * hi);
        Some(10.0 * (yr * yr + yi * yi).max(1e-20).log10())
    }
}

// ------------------------------------------------------------------------------ tilt shelf

/// Tilt Shelf: one control tilts the spectrum around `frequency`: the lows go down by half the
/// tilt and the highs up by half (a positive tilt brightens), as a pair of complementary shelves.
pub struct TiltShelf {
    pv: ParamValues,
    sr: f32,
    lo: Vec<Biquad>,
    hi: Vec<Biquad>,
    c_lo: Coeffs,
    c_hi: Coeffs,
    tilt: Smoothed,
    n: u64,
}

impl TiltShelf {
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::log("frequency", "Frequency", 100.0, 10000.0, 1000.0, Unit::Hertz),
        ParamSpec::new("tilt", "Tilt", -12.0, 12.0, 0.0, Unit::Decibels),
        ParamSpec::new("level", "Level", -24.0, 12.0, 0.0, Unit::Decibels),
    ];

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let n = channels.max(2);
        let mut s = TiltShelf {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            lo: vec![Biquad::new(); n],
            hi: vec![Biquad::new(); n],
            c_lo: Coeffs::IDENTITY,
            c_hi: Coeffs::IDENTITY,
            tilt: sm(0.0, sample_rate),
            n: 0,
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.tilt.set(self.pv.v("tilt"));
        if snap {
            self.tilt.snap();
            self.design();
        }
    }
    fn coeffs(&self, tilt: f64) -> (Coeffs, Coeffs) {
        let (sr, f) = (self.sr as f64, self.pv.v("frequency") as f64);
        let level = self.pv.v("level") as f64;
        (
            Coeffs::design(FilterType::LowShelf, sr, f, std::f64::consts::FRAC_1_SQRT_2, -tilt / 2.0 + level),
            Coeffs::design(FilterType::HighShelf, sr, f, std::f64::consts::FRAC_1_SQRT_2, tilt / 2.0),
        )
    }
    fn design(&mut self) {
        (self.c_lo, self.c_hi) = self.coeffs(self.tilt.value() as f64);
    }
}

impl AudioEffect for TiltShelf {
    param_plumbing!("tilt_shelf");
    fn reset(&mut self) {
        self.lo.iter_mut().chain(self.hi.iter_mut()).for_each(Biquad::reset);
        self.tilt.snap();
        self.n = 0;
        self.design();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let len = block_len(channels);
        grow(&mut self.lo, channels.len());
        grow(&mut self.hi, channels.len());
        for i in 0..len {
            self.tilt.tick();
            if self.n.is_multiple_of(GRID) {
                self.design();
            }
            self.n += 1;
            for (c, ch) in channels.iter_mut().enumerate() {
                let y = self.lo[c].tick(&self.c_lo, ch[i] as f64);
                ch[i] = self.hi[c].tick(&self.c_hi, y) as f32;
            }
        }
    }
    fn response_db(&self, freq: f64) -> Option<f64> {
        let (a, b) = self.coeffs(self.pv.v("tilt") as f64);
        Some(a.magnitude_db(freq, self.sr as f64) + b.magnitude_db(freq, self.sr as f64))
    }
}

// ------------------------------------------------------------------------------ sub cut

/// Sub Cut: a Butterworth high-pass of `order` (2–20, even) at `cutoff`, removing rumble and
/// infrasonic content; `level` sets how much of the removed band is kept (0 % = all removed).
pub struct SubCut {
    pv: ParamValues,
    sr: f32,
    st: Vec<Vec<Biquad>>,
    c: Vec<Coeffs>,
    keep: Smoothed,
}

impl SubCut {
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::log("cutoff", "Cutoff", 2.0, 200.0, 20.0, Unit::Hertz),
        ParamSpec::new("order", "Order", 2.0, 20.0, 10.0, Unit::None),
        ParamSpec::new("level", "Removed Band Level", 0.0, 100.0, 0.0, Unit::Percent),
    ];
    const MAX_SECTIONS: usize = 10;

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let mut s = SubCut {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            st: vec![vec![Biquad::new(); Self::MAX_SECTIONS]; channels.max(2)],
            c: Vec::with_capacity(Self::MAX_SECTIONS),
            keep: sm(0.0, sample_rate),
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        let order = ((self.pv.v("order").round() as usize).clamp(2, 20) / 2) * 2;
        let k = order / 2;
        let f = self.pv.v("cutoff") as f64;
        self.c.clear();
        for i in 1..=k {
            // Butterworth pole pair i of an order-2k filter: Q = 1 / (2 cos θ), θ = π(2i − 1)/(4k)
            let theta = std::f64::consts::PI * (2 * i - 1) as f64 / (4 * k) as f64;
            self.c.push(Coeffs::design(FilterType::HighPass, self.sr as f64, f, 1.0 / (2.0 * theta.cos()), 0.0));
        }
        self.keep.set(self.pv.v("level") / 100.0);
        if snap {
            self.keep.snap();
        }
    }
}

impl AudioEffect for SubCut {
    param_plumbing!("sub_cut");
    fn reset(&mut self) {
        self.st.iter_mut().flatten().for_each(Biquad::reset);
        self.keep.snap();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let len = block_len(channels);
        if self.st.len() < channels.len() {
            self.st.resize(channels.len(), vec![Biquad::new(); Self::MAX_SECTIONS]);
        }
        for i in 0..len {
            let keep = self.keep.tick() as f64;
            for (c, ch) in channels.iter_mut().enumerate() {
                let x = ch[i] as f64;
                let mut y = x;
                for (s, co) in self.st[c].iter_mut().zip(&self.c) {
                    y = s.tick(co, y);
                }
                ch[i] = (y + (x - y) * keep) as f32;
            }
        }
    }
    fn response_db(&self, freq: f64) -> Option<f64> {
        Some(self.c.iter().map(|c| c.magnitude_db(freq, self.sr as f64)).sum())
    }
}

// ------------------------------------------------------------------------------ dynamic eq

/// Dynamic Equalizer: the level of a detection band (band-pass at `detect_freq`, `detect_q`)
/// drives the gain of a peaking band (`target_freq`, `target_q`). In Cut mode the band is
/// lowered by `(level − threshold)·(1 − 1/ratio)` when the detection band is above the
/// threshold; in Boost mode it is raised by the same amount while the band is below it. The
/// change is limited to `range` dB and follows the attack / release times.
pub struct DynamicEq {
    pv: ParamValues,
    sr: f32,
    det: Vec<Biquad>,
    eq: Vec<Biquad>,
    c_det: Coeffs,
    c_eq: Coeffs,
    env: f32,
    gain_db: f32,
    n: u64,
}

impl DynamicEq {
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::new("threshold", "Threshold", -60.0, 0.0, -30.0, Unit::Decibels),
        ParamSpec::log("detect_freq", "Detection Frequency", 20.0, 20000.0, 1000.0, Unit::Hertz),
        ParamSpec::log("detect_q", "Detection Q", 0.1, 20.0, 2.0, Unit::Q),
        ParamSpec::log("target_freq", "Target Frequency", 20.0, 20000.0, 1000.0, Unit::Hertz),
        ParamSpec::log("target_q", "Target Q", 0.1, 20.0, 2.0, Unit::Q),
        ParamSpec::new("ratio", "Ratio", 1.0, 30.0, 2.0, Unit::Ratio),
        ParamSpec::log("attack", "Attack", 0.1, 500.0, 20.0, Unit::Milliseconds),
        ParamSpec::log("release", "Release", 1.0, 3000.0, 200.0, Unit::Milliseconds),
        ParamSpec::new("range", "Range", 0.0, 30.0, 12.0, Unit::Decibels),
        ParamSpec::choice("mode", "Mode", &["Cut", "Boost"], 0),
    ];

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let n = channels.max(2);
        let mut s = DynamicEq {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            det: vec![Biquad::new(); n],
            eq: vec![Biquad::new(); n],
            c_det: Coeffs::IDENTITY,
            c_eq: Coeffs::IDENTITY,
            env: 0.0,
            gain_db: 0.0,
            n: 0,
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, _snap: bool) {
        self.c_det = Coeffs::design(FilterType::BandPass, self.sr as f64, self.pv.v("detect_freq") as f64, self.pv.v("detect_q") as f64, 0.0);
        self.design_eq();
    }
    fn design_eq(&mut self) {
        self.c_eq = Coeffs::design(FilterType::Peaking, self.sr as f64, self.pv.v("target_freq") as f64, self.pv.v("target_q") as f64, self.gain_db as f64);
    }
    /// Gain (dB) of the target band for a detection level (dB).
    pub fn gain_for(&self, level_db: f32) -> f32 {
        let (thr, ratio, range) = (self.pv.v("threshold"), self.pv.v("ratio"), self.pv.v("range"));
        let k = 1.0 - 1.0 / ratio.max(1.0);
        if self.pv.idx("mode") == 0 { -((level_db - thr).max(0.0) * k).min(range) } else { ((thr - level_db).max(0.0) * k).min(range) }
    }
}

impl AudioEffect for DynamicEq {
    param_plumbing!("dynamic_eq");
    fn reset(&mut self) {
        self.det.iter_mut().chain(self.eq.iter_mut()).for_each(Biquad::reset);
        self.env = 0.0;
        self.gain_db = 0.0;
        self.n = 0;
        self.design_eq();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let len = block_len(channels);
        grow(&mut self.det, channels.len());
        grow(&mut self.eq, channels.len());
        let att = super::util::coef(self.pv.v("attack"), self.sr);
        let rel = super::util::coef(self.pv.v("release"), self.sr);
        for i in 0..len {
            // linked detection: the loudest channel of the detection band
            let mut peak = 0.0f32;
            for (c, ch) in channels.iter().enumerate() {
                peak = peak.max((self.det[c].tick(&self.c_det, ch[i] as f64) as f32).abs());
            }
            let co = if peak > self.env { att } else { rel };
            self.env = co * self.env + (1.0 - co) * peak;
            if self.env < 1e-20 {
                self.env = 0.0;
            }
            if self.n.is_multiple_of(GRID) {
                let target = self.gain_for(super::util::lin_db(self.env));
                // the gain itself is already smoothed by the envelope; step it on the grid
                if (target - self.gain_db).abs() > 1e-4 {
                    self.gain_db = target;
                    self.design_eq();
                }
            }
            self.n += 1;
            for (c, ch) in channels.iter_mut().enumerate() {
                ch[i] = self.eq[c].tick(&self.c_eq, ch[i] as f64) as f32;
            }
        }
    }
    fn response_db(&self, freq: f64) -> Option<f64> {
        Some(self.c_eq.magnitude_db(freq, self.sr as f64))
    }
}
