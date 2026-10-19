//! Channel and stereo-image tools: Mixdown, Stereo to Mono (copy one channel onto another),
//! headphone Crossfeed, Haas stereo enhancer and Stereo Tools (mid/side encode-decode, levels,
//! balance, phase and mute per side).

use super::util::{DelayLine, OnePole};
use crate::{AudioEffect, ParamSpec, ParamValues, Smoothed, Unit, block_len, db_to_gain, param_plumbing};

const SM: f32 = 20.0;

fn sm(v: f32, sr: f32) -> Smoothed {
    Smoothed::with_ms(v, sr, SM)
}

// ------------------------------------------------------------------------------ mixdown

/// Mixdown: every channel is summed to one mono signal (mean), which is written to the first
/// `N` channels of the output layout (Mono 1, Stereo 2, 2.1 3, 4 Channels 4, 5.1 6, 7.1 8); the
/// remaining channels of the track are silent.
pub struct Mixdown {
    pv: ParamValues,
}

impl Mixdown {
    pub const LAYOUTS: &'static [&'static str] = &["Mono", "Stereo", "2.1", "4 Channels", "5.1", "7.1"];
    const WIDTHS: [usize; 6] = [1, 2, 3, 4, 6, 8];
    pub const PARAMS: &'static [ParamSpec] = &[ParamSpec::choice("channels", "Number of Output Channels", Self::LAYOUTS, 1)];

    pub fn new(_sample_rate: f32, _channels: usize) -> Self {
        Mixdown { pv: ParamValues::new(Self::PARAMS) }
    }
    fn apply_params(&mut self, _snap: bool) {}
}

impl AudioEffect for Mixdown {
    param_plumbing!("mixdown");
    fn reset(&mut self) {}
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        let k = channels.len();
        if k == 0 {
            return;
        }
        let out = Self::WIDTHS[self.pv.idx("channels").min(5)];
        for i in 0..n {
            let m = channels.iter().map(|c| c[i]).sum::<f32>() / k as f32;
            for (c, ch) in channels.iter_mut().enumerate() {
                ch[i] = if c < out { m } else { 0.0 };
            }
        }
    }
}

// ------------------------------------------------------------------------------ stereo to mono

/// Stereo to Mono: copies the `from` channel onto the `to` channel (both channels then carry the
/// same signal). With Swap channels on, the two exchange instead, so nothing is lost.
pub struct StereoToMono {
    pv: ParamValues,
}

impl StereoToMono {
    pub const SIDES: &'static [&'static str] = &["Left", "Right"];
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::choice("from", "From", Self::SIDES, 0),
        ParamSpec::choice("to", "To", Self::SIDES, 1),
        ParamSpec::toggle("swap", "Swap Channels", false),
    ];

    pub fn new(_sample_rate: f32, _channels: usize) -> Self {
        StereoToMono { pv: ParamValues::new(Self::PARAMS) }
    }
    fn apply_params(&mut self, _snap: bool) {}
}

impl AudioEffect for StereoToMono {
    param_plumbing!("stereo_to_mono");
    fn reset(&mut self) {}
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        if channels.len() < 2 {
            return;
        }
        let (from, to) = (self.pv.idx("from").min(1), self.pv.idx("to").min(1));
        if from == to {
            return;
        }
        let swap = self.pv.on("swap");
        for i in 0..n {
            let (a, b) = (channels[from][i], channels[to][i]);
            channels[to][i] = a;
            if swap {
                channels[from][i] = b;
            }
        }
    }
}

// ------------------------------------------------------------------------------ crossfeed

/// Headphone crossfeed: each ear also hears the other channel low-passed (below `cutoff`, the
/// band the head does not shadow) at `strength`, as from loudspeakers; the sum is normalised so
/// a centred (mono) signal keeps its level.
pub struct Crossfeed {
    pv: ParamValues,
    sr: f32,
    lp: [OnePole; 4],
    strength: Smoothed,
    level_in: Smoothed,
    level_out: Smoothed,
}

impl Crossfeed {
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::new("strength", "Strength", 0.0, 100.0, 20.0, Unit::Percent),
        ParamSpec::log("cutoff", "Cutoff", 300.0, 2000.0, 700.0, Unit::Hertz),
        ParamSpec::new("level_in", "Input Level", -24.0, 12.0, 0.0, Unit::Decibels),
        ParamSpec::new("level_out", "Output Level", -24.0, 12.0, 0.0, Unit::Decibels),
    ];

    pub fn new(sample_rate: f32, _channels: usize) -> Self {
        let mut s = Crossfeed {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            lp: [OnePole::default(); 4],
            strength: sm(0.2, sample_rate),
            level_in: sm(1.0, sample_rate),
            level_out: sm(1.0, sample_rate),
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.strength.set(self.pv.v("strength") / 100.0);
        self.level_in.set(db_to_gain(self.pv.v("level_in")));
        self.level_out.set(db_to_gain(self.pv.v("level_out")));
        if snap {
            for s in [&mut self.strength, &mut self.level_in, &mut self.level_out] {
                s.snap();
            }
        }
    }
}

impl AudioEffect for Crossfeed {
    param_plumbing!("crossfeed");
    fn reset(&mut self) {
        self.lp.iter_mut().for_each(OnePole::reset);
        for s in [&mut self.strength, &mut self.level_in, &mut self.level_out] {
            s.snap();
        }
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        if channels.len() < 2 {
            return;
        }
        // two cascaded one-poles: 12 dB/oct above the cutoff
        let a = OnePole::alpha(self.pv.v("cutoff"), self.sr);
        let (left, right) = channels.split_at_mut(1);
        let (l, r) = (&mut left[0], &mut right[0]);
        for i in 0..n {
            let (k, gi, go) = (self.strength.tick(), self.level_in.tick(), self.level_out.tick());
            let (x_l, x_r) = (l[i] * gi, r[i] * gi);
            let to_l = self.lp[1].lp(a, self.lp[0].lp(a, x_r));
            let to_r = self.lp[3].lp(a, self.lp[2].lp(a, x_l));
            let norm = 1.0 / (1.0 + k);
            l[i] = (x_l + k * to_l) * norm * go;
            r[i] = (x_r + k * to_r) * norm * go;
        }
    }
}

// ------------------------------------------------------------------------------ haas

/// Haas stereo enhancer: a mono source (mid, side, left or right of the input) is sent to both
/// sides, one of them delayed by a few milliseconds, so the precedence effect spreads it across
/// the stereo field; each side has its own level, and the image can be mirrored.
pub struct Haas {
    pv: ParamValues,
    sr: f32,
    line: DelayLine,
    delay: Smoothed,
    gl: Smoothed,
    gr: Smoothed,
}

impl Haas {
    pub const SOURCES: &'static [&'static str] = &["Mid", "Side", "Left", "Right"];
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::choice("source", "Middle Source", Self::SOURCES, 0),
        ParamSpec::new("delay", "Delay", 0.0, 40.0, 12.0, Unit::Milliseconds),
        ParamSpec::choice("delayed", "Delayed Side", &["Left", "Right"], 1),
        ParamSpec::new("left_gain", "Left Gain", -24.0, 12.0, 0.0, Unit::Decibels),
        ParamSpec::new("right_gain", "Right Gain", -24.0, 12.0, 0.0, Unit::Decibels),
    ];

    pub fn new(sample_rate: f32, _channels: usize) -> Self {
        let mut s = Haas {
            pv: ParamValues::new(Self::PARAMS),
            sr: sample_rate,
            line: DelayLine::new((sample_rate * 0.041).ceil() as usize + 4),
            delay: sm(0.0, sample_rate),
            gl: sm(1.0, sample_rate),
            gr: sm(1.0, sample_rate),
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.delay.set(self.pv.v("delay") * 0.001 * self.sr);
        self.gl.set(db_to_gain(self.pv.v("left_gain")));
        self.gr.set(db_to_gain(self.pv.v("right_gain")));
        if snap {
            for s in [&mut self.delay, &mut self.gl, &mut self.gr] {
                s.snap();
            }
        }
    }
}

impl AudioEffect for Haas {
    param_plumbing!("haas");
    fn reset(&mut self) {
        self.line.reset();
        for s in [&mut self.delay, &mut self.gl, &mut self.gr] {
            s.snap();
        }
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        if channels.len() < 2 {
            return;
        }
        let source = self.pv.idx("source");
        let right_delayed = self.pv.idx("delayed") == 1;
        let (left, right) = channels.split_at_mut(1);
        let (l, r) = (&mut left[0], &mut right[0]);
        for i in 0..n {
            let (d, gl, gr) = (self.delay.tick(), self.gl.tick(), self.gr.tick());
            let m = match source {
                0 => 0.5 * (l[i] + r[i]),
                1 => 0.5 * (l[i] - r[i]),
                2 => l[i],
                _ => r[i],
            };
            self.line.push(m);
            let late = self.line.read(d);
            let (a, b) = if right_delayed { (m, late) } else { (late, m) };
            l[i] = a * gl;
            r[i] = b * gr;
        }
    }
}

// ------------------------------------------------------------------------------ stereo tools

/// Stereo Tools: converts between left/right and mid/side (or folds to one side), scales the
/// mid and side signals, then applies balance, polarity inversion and mute per output side.
pub struct StereoTools {
    pv: ParamValues,
    mid: Smoothed,
    side: Smoothed,
    bl: Smoothed,
    br: Smoothed,
}

impl StereoTools {
    pub const MODES: &'static [&'static str] = &["LR > LR", "LR > MS", "MS > LR", "LR > LL", "LR > RR", "LR > L+R", "LR > RL"];
    pub const PARAMS: &'static [ParamSpec] = &[
        ParamSpec::choice("mode", "Mode", Self::MODES, 0),
        ParamSpec::new("mid_level", "Mid Level", -36.0, 12.0, 0.0, Unit::Decibels),
        ParamSpec::new("side_level", "Side Level", -36.0, 12.0, 0.0, Unit::Decibels),
        ParamSpec::new("balance", "Balance", -100.0, 100.0, 0.0, Unit::Pan),
        ParamSpec::toggle("invert_l", "Invert Left Phase", false),
        ParamSpec::toggle("invert_r", "Invert Right Phase", false),
        ParamSpec::toggle("mute_l", "Mute Left", false),
        ParamSpec::toggle("mute_r", "Mute Right", false),
    ];

    pub fn new(sample_rate: f32, _channels: usize) -> Self {
        let mut s = StereoTools {
            pv: ParamValues::new(Self::PARAMS),
            mid: sm(1.0, sample_rate),
            side: sm(1.0, sample_rate),
            bl: sm(1.0, sample_rate),
            br: sm(1.0, sample_rate),
        };
        s.apply_params(true);
        s
    }
    fn apply_params(&mut self, snap: bool) {
        self.mid.set(db_to_gain(self.pv.v("mid_level")));
        self.side.set(db_to_gain(self.pv.v("side_level")));
        let bal = self.pv.v("balance") / 100.0;
        let sign = |inv: bool, mute: bool| -> f32 {
            if mute {
                0.0
            } else if inv {
                -1.0
            } else {
                1.0
            }
        };
        self.bl.set((1.0 - bal.max(0.0)) * sign(self.pv.on("invert_l"), self.pv.on("mute_l")));
        self.br.set((1.0 + bal.min(0.0)) * sign(self.pv.on("invert_r"), self.pv.on("mute_r")));
        if snap {
            for s in [&mut self.mid, &mut self.side, &mut self.bl, &mut self.br] {
                s.snap();
            }
        }
    }
}

impl AudioEffect for StereoTools {
    param_plumbing!("stereo_tools");
    fn reset(&mut self) {
        for s in [&mut self.mid, &mut self.side, &mut self.bl, &mut self.br] {
            s.snap();
        }
    }
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        let n = block_len(channels);
        if channels.len() < 2 {
            return;
        }
        let mode = self.pv.idx("mode");
        let (left, right) = channels.split_at_mut(1);
        let (l, r) = (&mut left[0], &mut right[0]);
        for i in 0..n {
            let (gm, gs, bl, br) = (self.mid.tick(), self.side.tick(), self.bl.tick(), self.br.tick());
            let (x_l, x_r) = (l[i], r[i]);
            let (y_l, y_r) = match mode {
                // MS > LR: the input carries mid on the left and side on the right
                2 => (x_l * gm + x_r * gs, x_l * gm - x_r * gs),
                _ => {
                    let (m, s) = (0.5 * (x_l + x_r) * gm, 0.5 * (x_l - x_r) * gs);
                    let (a, b) = (m + s, m - s);
                    match mode {
                        1 => (m, s),
                        3 => (a, a),
                        4 => (b, b),
                        5 => (a + b, a + b),
                        6 => (b, a),
                        _ => (a, b),
                    }
                }
            };
            l[i] = y_l * bl;
            r[i] = y_r * br;
        }
    }
}
