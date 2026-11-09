//! Sound effects without a library: each kind is synthesised (noise, sweeps, sines), written once
//! as a 48 kHz 16-bit mono WAV in a cache folder, imported and placed so its accent lands on the
//! cue. Deterministic (fixed seeds), so the same request makes the same sound, and there is no
//! licence to track.
//!
//! The accent is what the ear syncs to: the loudest point of a whoosh, the end of a riser, the
//! attack of a hit. [`Kind::accent`] says where it is, and placement starts the clip that much
//! before the cue (sound a frame or two early reads as in sync; late reads as broken).

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};

pub const RATE: u32 = 48_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Filtered noise swelling past the listener, 0.7 s, loudest at 0.42 s. Cuts and moves.
    Whoosh,
    /// A low thump with a click on top, 1.4 s from the attack. Titles landing, beats.
    Hit,
    /// Noise and a rising tone over 1.6 s that stop on the cue. Into a payoff or a cut.
    Riser,
    /// High, soft bell partials with a slow tremolo, 1.8 s. A logo or end card appearing.
    Shimmer,
    /// A short tick, 60 ms. Small UI-like elements.
    Click,
    /// A long sub-bass swell and decay, 2.6 s. The biggest moment, once.
    Boom,
    /// A whoosh that swells to its end, 0.7 s, peaking on the cue. Into a title.
    Reverse,
}

impl Kind {
    pub const ALL: [Kind; 7] = [Kind::Whoosh, Kind::Hit, Kind::Riser, Kind::Shimmer, Kind::Click, Kind::Boom, Kind::Reverse];

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "whoosh" | "swoosh" => Kind::Whoosh,
            "hit" | "impact" | "thump" => Kind::Hit,
            "riser" | "rise" => Kind::Riser,
            "shimmer" | "sparkle" => Kind::Shimmer,
            "click" | "tick" => Kind::Click,
            "boom" | "bass" | "sub" => Kind::Boom,
            "reverse" | "reverse_whoosh" | "suck" => Kind::Reverse,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Whoosh => "whoosh",
            Kind::Hit => "hit",
            Kind::Riser => "riser",
            Kind::Shimmer => "shimmer",
            Kind::Click => "click",
            Kind::Boom => "boom",
            Kind::Reverse => "reverse",
        }
    }

    /// Seconds from the start of the sound to its accent.
    pub fn accent(self) -> f64 {
        match self {
            Kind::Whoosh => 0.42,
            Kind::Riser => 1.6,
            Kind::Reverse => 0.66,
            Kind::Hit | Kind::Boom => 0.01,
            Kind::Shimmer | Kind::Click => 0.0,
        }
    }

    pub fn seconds(self) -> f32 {
        match self {
            Kind::Whoosh | Kind::Reverse => 0.7,
            Kind::Hit => 1.4,
            Kind::Riser => 1.7,
            Kind::Shimmer => 1.8,
            Kind::Click => 0.06,
            Kind::Boom => 2.6,
        }
    }
}

/// A small deterministic noise source (xorshift), in -1..1.
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// One-pole low-pass coefficient for a cutoff in Hz.
fn lp(cutoff: f32) -> f32 {
    1.0 - (-TAU * cutoff.max(10.0) / RATE as f32).exp()
}

/// The samples of `kind`, peak-normalised to -1 dBFS.
pub fn synth(kind: Kind) -> Vec<f32> {
    let n = (kind.seconds() * RATE as f32) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Noise(0x9e37_79b9 ^ kind as u32);
    let t = |i: usize| i as f32 / RATE as f32;
    match kind {
        Kind::Whoosh | Kind::Reverse => {
            let (mut a, mut b) = (0.0f32, 0.0f32);
            // a reverse whoosh swells all the way to its end
            let peak = if kind == Kind::Reverse { 0.66f32 } else { 0.42f32 };
            for (i, s) in out.iter_mut().enumerate() {
                let x = t(i);
                // swell to the peak, then fall away faster
                let env = if x < peak { (x / peak).powf(2.2) } else { (1.0 - (x - peak) / (0.7 - peak)).max(0.0).powf(1.6) };
                let cutoff = 300.0 + 3200.0 * env;
                let w = rng.next();
                a += lp(cutoff) * (w - a);
                b += lp(cutoff * 0.35) * (w - b);
                *s = (a - b) * env;
            }
        }
        Kind::Hit => {
            let mut phase = 0.0f32;
            let mut c = 0.0f32;
            for (i, s) in out.iter_mut().enumerate() {
                let x = t(i);
                let f = 48.0 + 70.0 * (-x * 18.0).exp();
                phase += TAU * f / RATE as f32;
                let body = phase.sin() * (-x * 3.2).exp();
                let w = rng.next();
                c += lp(2500.0) * (w - c);
                let click = c * (-x * 90.0).exp() * 0.8;
                *s = (body * 1.6).tanh() + click;
            }
        }
        Kind::Riser => {
            let (mut phase, mut a, mut b) = (0.0f32, 0.0f32, 0.0f32);
            for (i, s) in out.iter_mut().enumerate() {
                let x = t(i);
                let k = (x / 1.6).min(1.0);
                let env = if x <= 1.6 { k.powf(2.5) } else { (1.0 - (x - 1.6) / 0.1).max(0.0) };
                phase += TAU * (180.0 + 900.0 * k * k) / RATE as f32;
                let w = rng.next();
                let cutoff = 600.0 + 6000.0 * k;
                a += lp(cutoff) * (w - a);
                b += lp(cutoff * 0.2) * (w - b);
                *s = env * ((a - b) * 0.8 + phase.sin() * 0.25 + (phase * 1.5).sin() * 0.12);
            }
        }
        Kind::Shimmer => {
            let partials = [(1568.0, 1.0), (2093.0, 0.7), (2637.0, 0.5), (3136.0, 0.35), (4186.0, 0.2)];
            for (i, s) in out.iter_mut().enumerate() {
                let x = t(i);
                let env = (x / 0.08).min(1.0) * (-x * 1.6).exp();
                let trem = 0.75 + 0.25 * (TAU * 6.0 * x).sin();
                *s = env * trem * partials.iter().map(|(f, g)| g * (TAU * f * x + f * 0.001).sin()).sum::<f32>() * 0.35;
            }
        }
        Kind::Click => {
            let mut c = 0.0f32;
            for (i, s) in out.iter_mut().enumerate() {
                let x = t(i);
                let w = rng.next();
                c += lp(6000.0) * (w - c);
                *s = c * (-x * 160.0).exp() + (TAU * 2200.0 * x).sin() * (-x * 120.0).exp() * 0.4;
            }
        }
        Kind::Boom => {
            let mut phase = 0.0f32;
            for (i, s) in out.iter_mut().enumerate() {
                let x = t(i);
                phase += TAU * (38.0 + 30.0 * (-x * 6.0).exp()) / RATE as f32;
                let env = (x / 0.02).min(1.0) * (-x * 1.3).exp();
                *s = (phase.sin() * 1.8).tanh() * env + (phase * 2.0).sin() * env * 0.25;
            }
        }
    }
    // short fades so no clip clicks, then -1 dBFS peak
    let fade = (RATE as usize / 400).min(n / 4);
    for i in 0..fade {
        let g = i as f32 / fade as f32;
        out[i] *= if matches!(kind, Kind::Hit | Kind::Boom | Kind::Click) { 1.0 } else { g };
        out[n - 1 - i] *= g;
    }
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        let g = 0.891 / peak;
        out.iter_mut().for_each(|s| *s *= g);
    }
    out
}

/// A 16-bit mono PCM WAV file.
pub fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&1u16.to_le_bytes()); // mono
    v.extend_from_slice(&RATE.to_le_bytes());
    v.extend_from_slice(&(RATE * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        v.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    v
}

/// The cache file for `kind` under `dir`, written on first use.
pub fn file(dir: &Path, kind: Kind) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let p = dir.join(format!("redrob-sfx-{}-v1.wav", kind.name()));
    if !p.exists() {
        std::fs::write(&p, wav(&synth(kind)))?;
    }
    Ok(p)
}

/// The default cache folder.
pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("redrob-film-sfx")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_finite_audible_and_peaks_where_it_says() {
        for k in Kind::ALL {
            let s = synth(k);
            assert_eq!(s.len(), (k.seconds() * RATE as f32) as usize, "{k:?}");
            assert!(s.iter().all(|x| x.is_finite() && x.abs() <= 0.9), "{k:?}");
            let peak_at = s.iter().enumerate().fold((0, 0.0f32), |m, (i, x)| if x.abs() > m.1 { (i, x.abs()) } else { m }).0 as f64 / RATE as f64;
            assert!((peak_at - k.accent()).abs() < 0.25, "{k:?} peaks at {peak_at}");
            assert_eq!(Kind::parse(k.name()), Some(k));
        }
        assert_eq!(Kind::parse("kazoo"), None);
    }

    #[test]
    fn the_wav_header_matches_the_data() {
        let w = wav(&[0.0, 0.5, -0.5]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        let dir = std::env::temp_dir().join(format!("sfx-test-{}", std::process::id()));
        let p = file(&dir, Kind::Click).unwrap();
        assert!(p.exists() && std::fs::metadata(&p).unwrap().len() > 44);
        let _ = std::fs::remove_dir_all(dir);
    }
}
