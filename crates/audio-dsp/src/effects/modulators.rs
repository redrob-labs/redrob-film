//! Modulators: Pulsator (tremolo / auto-pan), Vibrato (pitch modulation through a swept
//! delay), and the Hilbert-transform pair Frequency Shifter / Phase Shifter.

use super::util::{DelayLine, Lfo, uni_sine, uni_tri};
use crate::{AudioEffect, ParamSpec, ParamValues, Smoothed, Unit, block_len, db_to_gain, flush, param_plumbing};
use std::f64::consts::PI;

const SM: f32 = 20.0;

fn sm(v: f32, sr: f32) -> Smoothed {
    Smoothed::with_ms(v, sr, SM)
}

/// LFO shapes shared by the modulators (0..1 output).
pub const WAVEFORMS: &[&str] = &["Sine", "Triangle", "Square", "Saw Up", "Saw Down"];

fn wave(shape: usize, phase: f64) -> f32 {
    match shape {
        0 => uni_sine(phase),
        1 => uni_tri(phase),
        2 => {
            if phase < 0.5 {
                1.0
            } else {
                0.0
            }
        }
        3 => phase as f32,
        _ => 1.0 - phase as f32,
    }
}

// ------------------------------------------------------------------------------ pulsator

/// Pulsator: the level of each side follows an LFO (`gain = 1 − amount · lfo`). The two sides
/// read the LFO at their own phase offsets: equal offsets give a tremolo, offsets half a cycle
/// apart an auto-pan.
pub struct Pulsator {
    pv: ParamValues,
    sr: f32,
    lfo: Lfo,
    amount: Smoothed,
    level_in: Smoothed,
    level_out: Smoothed,
}

impl Pulsator {
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::choice("mode", "Waveform", WAVEFORMS, 0),
        ParamSpec::new("amount", "Amount", 0.0, 100.0, 100.0, Unit::Percent),
        ParamSpec::log("rate", "Rate", 0.01, 100.0, 2.0, Unit::Hertz),
        ParamSpec::new("offset_l", "Offset Left", 0.0, 100.0, 0.0, Unit::Percent),
        ParamSpec::new("offset_r", "Offset Right", 0.0, 100.0, 50.0, Unit::Percent),
        ParamSpec::new("level_in", "Input Level", -24.0, 12.0, 0.0, Unit::Decibels),
        ParamSpec::new("level_out", "Output Level", -24.0, 12.0, 0.0, Unit::Decibels),
    ];

    pub fn new(sample_rate: f32, _channels: usize) -> Self {
        let mut s = Pulsator {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            lfo: Lfo::default(),
            amount: sm(1.0, sample_rate),
            level_in: sm(1.0, sample_rate),
            level_out: sm(1.0, sample_rate),
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.amount.set(self.pv.v("amount") / 100.0);
        self.level_in.set(db_to_gain(self.pv.v("level_in")));
        self.level_out.set(db_to_gain(self.pv.v("level_out")));
        if snap {
            for s in [&mut self.amount, &mut self.level_in, &mut self.level_out] {
                s.snap();
            }
        }
    }
}

impl AudioEffect for Pulsator {
    param_plumbing!("pulsator");
    fn reset(&mut self) {
        self.lfo.reset();
        for s in [&mut self.amount, &mut self.level_in, &mut self.level_out] {
            s.snap();
        }
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        let shape = self.pv.idx("mode");
        let rate = self.pv.v("rate");
        let offs = [self.pv.v("offset_l") as f64 / 100.0, self.pv.v("offset_r") as f64 / 100.0];
        for i in 0..n {
            let (amt, gi, go) = (self.amount.tick(), self.level_in.tick(), self.level_out.tick());
            for (c, ch) in channels.iter_mut().enumerate() {
                // channels past the stereo pair follow the side of the same parity
                let w = wave(shape, self.lfo.phase(offs[c % 2]));
                ch[i] *= gi * (1.0 - amt * w) * go;
            }
            self.lfo.advance(rate, self.sr);
        }
    }
}

// ------------------------------------------------------------------------------ vibrato

/// Vibrato: the signal is read from a delay line whose length is swept by a sine LFO (up to
/// 7 ms at full depth), which modulates its pitch. Fully wet.
pub struct Vibrato {
    pv: ParamValues,
    sr: f32,
    lfo: Lfo,
    lines: Vec<DelayLine>,
    depth: Smoothed,
}

impl Vibrato {
    const MAX_MS: f32 = 7.0;
    pub const PARAMS: &'static [ParamSpec] =
        &[ParamSpec::log("frequency", "Modulation Frequency", 0.1, 20.0, 5.0, Unit::Hertz), ParamSpec::new("depth", "Depth", 0.0, 100.0, 50.0, Unit::Percent)];

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let max = (sample_rate * Self::MAX_MS * 0.001).ceil() as usize + 4;
        let mut s = Vibrato {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            lfo: Lfo::default(),
            lines: vec![DelayLine::new(max); channels.max(2)],
            depth: sm(0.5, sample_rate),
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.depth.set(self.pv.v("depth") / 100.0);
        if snap {
            self.depth.snap();
        }
    }
}

impl AudioEffect for Vibrato {
    param_plumbing!("vibrato");
    fn reset(&mut self) {
        self.lfo.reset();
        self.lines.iter_mut().for_each(DelayLine::reset);
        self.depth.snap();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        if self.lines.len() < channels.len() {
            let max = self.lines[0].max_delay() as usize;
            self.lines.resize(channels.len(), DelayLine::new(max));
        }
        let rate = self.pv.v("frequency");
        let span = self.sr * Self::MAX_MS * 0.001;
        for i in 0..n {
            let d = 1.0 + self.depth.tick() * span * uni_sine(self.lfo.phase(0.0));
            for (c, ch) in channels.iter_mut().enumerate() {
                self.lines[c].push(ch[i]);
                ch[i] = self.lines[c].read(d);
            }
            self.lfo.advance(rate, self.sr);
        }
    }
}

// ------------------------------------------------------------------------------ hilbert pair

/// Second-order all-pass in z⁻²: `y = a²·(x + y₋₂) − x₋₂`.
#[derive(Clone, Copy, Debug, Default)]
struct Ap2 {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Ap2 {
    #[inline]
    fn tick(&mut self, a2: f64, x: f64) -> f64 {
        let y = a2 * (x + self.y2) - self.x2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = flush(y);
        y
    }
}

/// Two all-pass chains whose outputs stay ~90° apart from about 20 Hz to just below Nyquist
/// (Niemitalo's published polyphase IIR Hilbert design); the first chain is delayed one
/// sample. Their outputs are the real and imaginary parts of the analytic signal.
#[derive(Clone, Copy, Debug, Default)]
struct Hilbert {
    a: [Ap2; 4],
    b: [Ap2; 4],
    delay: f64,
}

const HILBERT_A: [f64; 4] = [0.692_387_8, 0.936_065_432_295_9, 0.988_229_522_686_0, 0.998_748_845_273_7];
const HILBERT_B: [f64; 4] = [0.402_192_116_242_6, 0.856_171_088_242_0, 0.972_290_954_565_1, 0.995_288_479_127_8];

impl Hilbert {
    #[inline]
    fn tick(&mut self, x: f64) -> (f64, f64) {
        let mut re = x;
        for (s, k) in self.a.iter_mut().zip(HILBERT_A) {
            re = s.tick(k * k, re);
        }
        let mut im = x;
        for (s, k) in self.b.iter_mut().zip(HILBERT_B) {
            im = s.tick(k * k, im);
        }
        let out = (self.delay, im);
        self.delay = re;
        out
    }
}

/// `PHASE = false`: Frequency Shifter — every component moves by `shift` Hz (not a pitch shift:
/// harmonic ratios change). `PHASE = true`: Phase Shifter — every component is rotated by
/// `shift` × 180°. Both are single-sideband modulation of the analytic signal.
pub struct Shifter<const PHASE: bool> {
    pv: ParamValues,
    sr: f32,
    h: Vec<Hilbert>,
    phase: f64,
    level: Smoothed,
    shift: Smoothed,
}

impl<const PHASE: bool> Shifter<PHASE> {
    pub const PARAMS: &'static [ParamSpec] = if PHASE {
        &[ParamSpec::new("shift", "Shift", -1.0, 1.0, 0.0, Unit::None), ParamSpec::new("level", "Level", -24.0, 12.0, 0.0, Unit::Decibels)]
    } else {
        &[ParamSpec::new("shift", "Shift", -2000.0, 2000.0, 0.0, Unit::Hertz), ParamSpec::new("level", "Level", -24.0, 12.0, 0.0, Unit::Decibels)]
    };

    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let mut s = Shifter {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            h: vec![Hilbert::default(); channels.max(2)],
            phase: 0.0,
            level: sm(1.0, sample_rate),
            shift: sm(0.0, sample_rate),
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.level.set(db_to_gain(self.pv.v("level")));
        self.shift.set(self.pv.v("shift"));
        if snap {
            self.level.snap();
            self.shift.snap();
        }
    }
}

impl<const PHASE: bool> AudioEffect for Shifter<PHASE> {
    param_plumbing!(if PHASE { "phase_shift" } else { "freq_shift" });
    fn reset(&mut self) {
        self.h.iter_mut().for_each(|h| *h = Hilbert::default());
        self.phase = 0.0;
        self.level.snap();
        self.shift.snap();
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        if self.h.len() < channels.len() {
            self.h.resize(channels.len(), Hilbert::default());
        }
        for i in 0..n {
            let (g, sh) = (self.level.tick() as f64, self.shift.tick() as f64);
            let theta = if PHASE { sh * PI } else { self.phase };
            let (sn, cs) = theta.sin_cos();
            for (c, ch) in channels.iter_mut().enumerate() {
                let (re, im) = self.h[c].tick(ch[i] as f64);
                ch[i] = ((re * cs - im * sn) * g) as f32;
            }
            if !PHASE {
                self.phase = (self.phase + 2.0 * PI * sh / self.sr as f64).rem_euclid(2.0 * PI);
            }
        }
    }
}
