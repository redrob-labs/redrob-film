//! Per-monitor audio taps.
//!
//! The Program and Source monitors play through their own mix closures (`playback_mix`,
//! `start_source_audio`). Each closure reads its monitor's tap: the monitor's playback volume
//! (monitor ▸ Settings ▸ Volume, independent of the Mix fader and of the other monitor) is applied
//! to what it plays, and the peak of every channel it played is posted for the monitor's level
//! meter. The taps are process-wide, like the audio device they feed.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// One monitor's volume and the channel peaks played since the meter last read them.
pub struct Tap {
    gain: AtomicU32,
    peaks: Mutex<Vec<f32>>,
}

impl Tap {
    fn new() -> Self {
        Tap { gain: AtomicU32::new(1f32.to_bits()), peaks: Mutex::new(Vec::new()) }
    }
    /// Linear playback gain (1 = unity).
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }
    /// Set the monitor volume in percent (0 … 100, the slider's range).
    pub fn set_volume(&self, percent: f32) {
        let g = if percent.is_finite() { (percent / 100.0).clamp(0.0, 1.0) } else { 1.0 };
        self.gain.store(g.to_bits(), Ordering::Relaxed);
    }
    /// Record the peak of each planar channel's first `n` samples, scaled by `gain`.
    pub fn post(&self, channels: &[Vec<f32>], n: usize, gain: f32) {
        let mut p = self.peaks.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if p.len() != channels.len() {
            *p = vec![0.0; channels.len()];
        }
        for (acc, c) in p.iter_mut().zip(channels) {
            let m = c.iter().take(n).fold(0.0f32, |m, x| m.max(x.abs())) * gain;
            *acc = acc.max(m);
        }
    }
    /// The peaks played since the last call (linear, one per channel; empty when nothing played).
    pub fn take(&self) -> Vec<f32> {
        std::mem::take(&mut *self.peaks.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }
}

/// The Program monitor's tap.
pub fn program() -> Arc<Tap> {
    static T: OnceLock<Arc<Tap>> = OnceLock::new();
    T.get_or_init(|| Arc::new(Tap::new())).clone()
}

/// The Source monitor's tap.
pub fn source() -> Arc<Tap> {
    static T: OnceLock<Arc<Tap>> = OnceLock::new();
    T.get_or_init(|| Arc::new(Tap::new())).clone()
}
