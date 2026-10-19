//! Computational tools: Crusher (bit-depth / sample-rate reduction), Crystalizer (sample
//! difference sharpening), DC Shift, Derivative / Integral, Soft Clipper and the bass
//! generators Sub Boost and Virtual Bass.
//!
//! Every effect works per sample on any number of channels, with its own state per channel, so
//! the output does not depend on how callers cut the stream into blocks.

use super::util::{Lfo, OnePole, uni_sine};
use crate::biquad::{Biquad, Coeffs, FilterType};
use crate::{AudioEffect, ParamSpec, ParamValues, Smoothed, Unit, block_len, db_to_gain, flush, param_plumbing};
use std::f64::consts::PI;

const SM: f32 = 20.0;

fn sm(v: f32, sr: f32) -> Smoothed {
    Smoothed::with_ms(v, sr, SM)
}

/// Per-channel state vectors sized for at least two channels (more are grown on demand).
fn grow<T: Default + Clone>(v: &mut Vec<T>, n: usize) {
    if v.len() < n {
        v.resize(n, T::default());
    }
}

// ------------------------------------------------------------------------------ crusher

/// Bit crusher: quantises the signal to `bits` levels (linear or logarithmic steps), holds every
/// `samples`-th sample (sample-rate reduction, optionally swept by an LFO), with a DC bias that
/// crushes the two half-waves differently and an anti-aliasing amount that softens the steps.
pub struct Crusher {
    pv: ParamValues,
    sr: f32,
    lfo: Lfo,
    /// Per channel: held value and samples left until the next sample is taken.
    hold: Vec<f32>,
    count: Vec<f32>,
    input: Smoothed,
    output: Smoothed,
    mix: Smoothed,
}

impl Crusher {
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::new("level_in", "Input Gain", -24.0, 24.0, 0.0, Unit::Decibels),
        ParamSpec::new("level_out", "Output Gain", -24.0, 24.0, 0.0, Unit::Decibels),
        ParamSpec::new("bits", "Bit Reduction", 1.0, 64.0, 8.0, Unit::None),
        ParamSpec::new("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
        ParamSpec::choice("mode", "Mode", &["Linear", "Logarithmic"], 0),
        ParamSpec::new("dc", "DC", 0.25, 4.0, 1.0, Unit::None),
        ParamSpec::new("aa", "Anti-aliasing", 0.0, 1.0, 0.5, Unit::None),
        ParamSpec::new("samples", "Sample Reduction", 1.0, 250.0, 1.0, Unit::None),
        ParamSpec::toggle("lfo", "Enable LFO", false),
        ParamSpec::new("lfo_range", "LFO Depth", 1.0, 250.0, 20.0, Unit::None),
        ParamSpec::log("lfo_rate", "LFO Rate", 0.01, 200.0, 0.3, Unit::Hertz),
    ];

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let n = channels.max(2);
        let mut s = Crusher {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            lfo: Lfo::default(),
            hold: vec![0.0; n],
            count: vec![0.0; n],
            input: sm(1.0, sample_rate),
            output: sm(1.0, sample_rate),
            mix: sm(1.0, sample_rate),
        };
        s.apply_params(true);
        s
    }

    fn apply_params(&mut self, snap: bool) {
        self.input.set(db_to_gain(self.pv.v("level_in")));
        self.output.set(db_to_gain(self.pv.v("level_out")));
        self.mix.set(self.pv.v("mix") / 100.0);
        if snap {
            for s in [&mut self.input, &mut self.output, &mut self.mix] {
                s.snap();
            }
        }
    }

    /// Quantise one sample.
    fn crush(&self, x: f32) -> f32 {
        let bits = self.pv.v("bits").round().clamp(1.0, 64.0) as i32;
        let levels = 2f64.powi(bits - 1).max(1.0);
        let dc = self.pv.v("dc") as f64;
        let aa = self.pv.v("aa") as f64;
        let x = x as f64;
        // the DC bias makes the steps of the positive half-wave finer than the negative one's
        let step = if x >= 0.0 { 1.0 / (levels * dc) } else { dc / levels };
        let q = if self.pv.idx("mode") == 1 {
            // logarithmic steps: quantise the magnitude in the log domain
            let m = x.abs();
            if m < 1e-9 {
                0.0
            } else {
                let k = levels / 8.0 + 1.0;
                x.signum() * ((m.ln() * k).round() / k).exp()
            }
        } else {
            (x / step).round() * step
        };
        // anti-aliasing: soften the corner of each step towards the input
        let y = q + (x - q) * aa * 0.5;
        y as f32
    }
}

impl AudioEffect for Crusher {
    param_plumbing!("crusher");
    fn reset(&mut self) {
        self.lfo.reset();
        self.hold.iter_mut().for_each(|v| *v = 0.0);
        self.count.iter_mut().for_each(|v| *v = 0.0);
        for s in [&mut self.input, &mut self.output, &mut self.mix] {
            s.snap();
        }
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        grow(&mut self.hold, channels.len());
        grow(&mut self.count, channels.len());
        let base = self.pv.v("samples").round().max(1.0);
        let lfo_on = self.pv.on("lfo");
        let depth = self.pv.v("lfo_range");
        let rate = self.pv.v("lfo_rate");
        for i in 0..n {
            let (gi, go, mix) = (self.input.tick(), self.output.tick(), self.mix.tick());
            let samples = if lfo_on { (base + depth * uni_sine(self.lfo.phase(0.0))).round().clamp(1.0, 500.0) } else { base };
            for (c, ch) in channels.iter_mut().enumerate() {
                let x = ch[i] * gi;
                if self.count[c] <= 0.0 {
                    self.hold[c] = self.crush(x);
                    self.count[c] = samples;
                }
                self.count[c] -= 1.0;
                ch[i] = (x * (1.0 - mix) + self.hold[c] * mix) * go;
            }
            if lfo_on {
                self.lfo.advance(rate, self.sr);
            }
        }
    }
}

// ------------------------------------------------------------------------------ crystalizer

/// Crystalizer: `y = x + i·(x − x₋₁)` sharpens by widening each sample difference; a negative
/// intensity runs the inverse (a smoothing `y = (x + k·y₋₁) / (1 + k)`). Optional clipping to ±1.
pub struct Crystalizer {
    pv: ParamValues,
    prev: Vec<f64>,
    intensity: Smoothed,
}

impl Crystalizer {
    pub const PARAMS: &'static [ParamSpec] =
        &[ParamSpec::new("intensity", "Intensity", -10.0, 10.0, 2.0, Unit::None), ParamSpec::toggle("clip", "Enable Clipping", true)];

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let mut s = Crystalizer { pv: ParamValues::new(Self::PARAMS), prev: vec![0.0; channels.max(2)], intensity: sm(2.0, sample_rate) };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.intensity.set(self.pv.v("intensity"));
        if snap {
            self.intensity.snap();
        }
    }
}

impl AudioEffect for Crystalizer {
    param_plumbing!("crystalizer");
    fn reset(&mut self) {
        self.prev.iter_mut().for_each(|v| *v = 0.0);
        self.intensity.snap();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        grow(&mut self.prev, channels.len());
        let clip = self.pv.on("clip");
        for i in 0..n {
            let k = self.intensity.tick() as f64;
            for (c, ch) in channels.iter_mut().enumerate() {
                let x = ch[i] as f64;
                let y = if k >= 0.0 {
                    let y = x + k * (x - self.prev[c]);
                    self.prev[c] = x;
                    y
                } else {
                    let k = -k;
                    let y = flush((x + k * self.prev[c]) / (1.0 + k));
                    self.prev[c] = y;
                    y
                };
                ch[i] = if clip { y.clamp(-1.0, 1.0) as f32 } else { y as f32 };
            }
        }
    }
}

// ------------------------------------------------------------------------------ dc shift

/// DC Shift: adds a constant offset; a non-zero limiter gain soft-limits the shifted signal into
/// ±1 (the top `gain` of the range becomes a tanh knee) so the shift does not clip.
pub struct DcShift {
    pv: ParamValues,
    shift: Smoothed,
}

impl DcShift {
    pub const PARAMS: &'static [ParamSpec] =
        &[ParamSpec::new("shift", "DC Shift", -1.0, 1.0, 0.0, Unit::None), ParamSpec::new("limitergain", "Limiter Gain", 0.0, 1.0, 0.0, Unit::None)];

    pub fn new(sample_rate: f32, _channels: usize) -> Self {
        let mut s = DcShift { pv: ParamValues::new(Self::PARAMS), shift: sm(0.0, sample_rate) };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.shift.set(self.pv.v("shift"));
        if snap {
            self.shift.snap();
        }
    }
}

impl AudioEffect for DcShift {
    param_plumbing!("dc_shift");
    fn reset(&mut self) {
        self.shift.snap();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        let lg = self.pv.v("limitergain");
        for i in 0..n {
            let d = self.shift.tick();
            for ch in channels.iter_mut() {
                let y = ch[i] + d;
                ch[i] = if lg > 0.0 && y.abs() > 1.0 - lg {
                    let knee = 1.0 - lg;
                    y.signum() * (knee + lg * ((y.abs() - knee) / lg).tanh())
                } else {
                    y
                };
            }
        }
    }
}

// ------------------------------------------------------------------------------ derivative / integral

/// `INTEGRAL = false`: first difference `y = x − x₋₁` (a 6 dB/oct tilt up).
/// `INTEGRAL = true`: running sum `y = r·y₋₁ + x`, very slightly leaky (pole at 2 Hz) so a DC
/// component in the input cannot grow without bound.
pub struct Calculus<const INTEGRAL: bool> {
    pv: ParamValues,
    r: f64,
    state: Vec<f64>,
}

impl<const INTEGRAL: bool> Calculus<INTEGRAL> {
    pub const PARAMS: &'static [ParamSpec] = &[];

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        Calculus { pv: ParamValues::new(Self::PARAMS), r: 1.0 - 2.0 * PI * 2.0 / sample_rate.max(1.0) as f64, state: vec![0.0; channels.max(2)] }
    }
    fn apply_params(&mut self, _snap: bool) {}
}

impl<const INTEGRAL: bool> AudioEffect for Calculus<INTEGRAL> {
    param_plumbing!(if INTEGRAL { "integral" } else { "derivative" });
    fn reset(&mut self) {
        self.state.iter_mut().for_each(|v| *v = 0.0);
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        grow(&mut self.state, channels.len());
        for (c, ch) in channels.iter_mut().enumerate() {
            let s = &mut self.state[c];
            for v in ch[..n].iter_mut() {
                let x = *v as f64;
                if INTEGRAL {
                    *s = flush(self.r * *s + x);
                    *v = *s as f32;
                } else {
                    *v = (x - *s) as f32;
                    *s = x;
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------ soft clipper

/// Soft clipper: `y = t·f(x / t)` with a saturating curve `f` (|f| ≤ 1) above the threshold `t`.
pub struct SoftClipper {
    pv: ParamValues,
    threshold: Smoothed,
    output: Smoothed,
}

impl SoftClipper {
    pub const CURVES: &'static [&'static str] = &["Hard", "Tanh", "Atan", "Cubic", "Exponential", "Algebraic", "Quintic", "Sine"];
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::choice("type", "Type", Self::CURVES, 1),
        ParamSpec::new("threshold", "Threshold", -30.0, 0.0, 0.0, Unit::Decibels),
        ParamSpec::new("output", "Output Gain", -24.0, 24.0, 0.0, Unit::Decibels),
    ];

    pub fn new(sample_rate: f32, _channels: usize) -> Self {
        let mut s = SoftClipper { pv: ParamValues::new(Self::PARAMS), threshold: sm(1.0, sample_rate), output: sm(1.0, sample_rate) };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.threshold.set(db_to_gain(self.pv.v("threshold")));
        self.output.set(db_to_gain(self.pv.v("output")));
        if snap {
            self.threshold.snap();
            self.output.snap();
        }
    }

    /// The saturating curve `curve` at `x` (odd, |f| ≤ 1, slope 1 at 0 where it can be).
    pub fn shape(curve: usize, x: f32) -> f32 {
        match curve {
            0 => x.clamp(-1.0, 1.0),
            1 => x.tanh(),
            2 => (x.atan() * std::f32::consts::FRAC_2_PI).clamp(-1.0, 1.0),
            3 => {
                let x = x.clamp(-1.5, 1.5);
                x - x * x * x * 4.0 / 27.0
            }
            4 => x.signum() * (1.0 - (-x.abs()).exp()),
            5 => x / (1.0 + x * x).sqrt(),
            6 => {
                let x = x.clamp(-1.25, 1.25);
                // f(1.25) = 1 with zero slope there
                x - x.powi(5) * 0.08192
            }
            _ => (x.clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2)).sin(),
        }
    }
}

impl AudioEffect for SoftClipper {
    param_plumbing!("soft_clipper");
    fn reset(&mut self) {
        self.threshold.snap();
        self.output.snap();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        let curve = self.pv.idx("type");
        for i in 0..n {
            let (t, g) = (self.threshold.tick().max(1e-4), self.output.tick());
            for ch in channels.iter_mut() {
                ch[i] = t * Self::shape(curve, ch[i] / t) * g;
            }
        }
    }
}

// ------------------------------------------------------------------------------ sub boost

/// Sub Boost: the band below `cutoff` is taken out with a 2nd-order low-pass, sustained by a
/// decaying recirculation (`decay`, `feedback`) and added back (`wet`) to the dry signal.
pub struct SubBoost {
    pv: ParamValues,
    sr: f32,
    lp: Vec<Biquad>,
    tail: Vec<OnePole>,
    state: Vec<f32>,
    coeffs: Coeffs,
    dry: Smoothed,
    wet: Smoothed,
}

impl SubBoost {
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::new("dry", "Dry", 0.0, 100.0, 100.0, Unit::Percent),
        ParamSpec::new("wet", "Wet", 0.0, 100.0, 80.0, Unit::Percent),
        ParamSpec::new("decay", "Decay", 0.0, 100.0, 70.0, Unit::Percent),
        ParamSpec::new("feedback", "Feedback", 0.0, 100.0, 90.0, Unit::Percent),
        ParamSpec::log("cutoff", "Cutoff", 20.0, 250.0, 100.0, Unit::Hertz),
    ];

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let n = channels.max(2);
        let mut s = SubBoost {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            lp: vec![Biquad::new(); n],
            tail: vec![OnePole::default(); n],
            state: vec![0.0; n],
            coeffs: Coeffs::IDENTITY,
            dry: sm(1.0, sample_rate),
            wet: sm(0.8, sample_rate),
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.coeffs = Coeffs::design(FilterType::LowPass, self.sr as f64, self.pv.v("cutoff") as f64, 0.9, 0.0);
        self.dry.set(self.pv.v("dry") / 100.0);
        self.wet.set(self.pv.v("wet") / 100.0);
        if snap {
            self.dry.snap();
            self.wet.snap();
        }
    }
}

impl AudioEffect for SubBoost {
    param_plumbing!("sub_boost");
    fn reset(&mut self) {
        self.lp.iter_mut().for_each(Biquad::reset);
        self.tail.iter_mut().for_each(OnePole::reset);
        self.state.iter_mut().for_each(|v| *v = 0.0);
        self.dry.snap();
        self.wet.snap();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        let c_n = channels.len();
        grow(&mut self.lp, c_n);
        grow(&mut self.tail, c_n);
        grow(&mut self.state, c_n);
        // recirculation gain < 1 keeps the loop stable; the one-pole tames its top end
        let fb = (self.pv.v("decay") / 100.0 * self.pv.v("feedback") / 100.0).min(0.98);
        let a = OnePole::alpha(self.pv.v("cutoff"), self.sr);
        for i in 0..n {
            let (dry, wet) = (self.dry.tick(), self.wet.tick());
            for (c, ch) in channels.iter_mut().enumerate() {
                let x = ch[i];
                let low = self.lp[c].tick(&self.coeffs, x as f64) as f32;
                let s = low + fb * self.tail[c].lp(a, self.state[c]);
                self.state[c] = if s.abs() < 1e-25 { 0.0 } else { s };
                ch[i] = x * dry + s * (1.0 - fb) * wet;
            }
        }
    }
}

// ------------------------------------------------------------------------------ virtual bass

/// Virtual Bass: harmonics of the band below `cutoff` (full-wave rectified, DC removed and
/// band-limited to 2–6 × the cutoff) are added, so the bass is heard on speakers that cannot
/// reproduce the fundamental.
pub struct VirtualBass {
    pv: ParamValues,
    sr: f32,
    lp: Vec<Biquad>,
    hp: Vec<Biquad>,
    post: Vec<Biquad>,
    c_lp: Coeffs,
    c_hp: Coeffs,
    c_post: Coeffs,
    amount: Smoothed,
}

impl VirtualBass {
    pub const PARAMS: &'static [ParamSpec] =
        &[ParamSpec::log("cutoff", "Cutoff", 40.0, 250.0, 120.0, Unit::Hertz), ParamSpec::new("strength", "Strength", 0.0, 100.0, 50.0, Unit::Percent)];

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let n = channels.max(2);
        let mut s = VirtualBass {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            lp: vec![Biquad::new(); n],
            hp: vec![Biquad::new(); n],
            post: vec![Biquad::new(); n],
            c_lp: Coeffs::IDENTITY,
            c_hp: Coeffs::IDENTITY,
            c_post: Coeffs::IDENTITY,
            amount: sm(0.5, sample_rate),
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        let fc = self.pv.v("cutoff") as f64;
        let sr = self.sr as f64;
        self.c_lp = Coeffs::design(FilterType::LowPass, sr, fc, 0.707, 0.0);
        self.c_hp = Coeffs::design(FilterType::HighPass, sr, fc * 1.5, 0.707, 0.0);
        self.c_post = Coeffs::design(FilterType::LowPass, sr, fc * 6.0, 0.707, 0.0);
        self.amount.set(self.pv.v("strength") / 100.0 * 2.0);
        if snap {
            self.amount.snap();
        }
    }
}

impl AudioEffect for VirtualBass {
    param_plumbing!("virtual_bass");
    fn reset(&mut self) {
        for b in self.lp.iter_mut().chain(self.hp.iter_mut()).chain(self.post.iter_mut()) {
            b.reset();
        }
        self.amount.snap();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        let c_n = channels.len();
        grow(&mut self.lp, c_n);
        grow(&mut self.hp, c_n);
        grow(&mut self.post, c_n);
        for i in 0..n {
            let amt = self.amount.tick() as f64;
            for (c, ch) in channels.iter_mut().enumerate() {
                let x = ch[i] as f64;
                let low = self.lp[c].tick(&self.c_lp, x);
                // rectification doubles the frequency and adds the even harmonics
                let h = self.hp[c].tick(&self.c_hp, low.abs());
                let h = self.post[c].tick(&self.c_post, h);
                ch[i] = (x + amt * h) as f32;
            }
        }
    }
}
