//! `music_beats`: where a music track's beats, bars and hits fall, and where it ends, so cuts
//! land on the music and the edit can end on the track's own last chord.
//!
//! Onset strength is spectral flux at 100 frames a second: the summed rise, in dB, of 24
//! log-spaced bands from 40 Hz to 10 kHz (as librosa's `onset_strength`). The tempo is the
//! autocorrelation peak of that curve between 60 and 200 BPM, weighted by a log-normal prior
//! around 120 BPM (one octave wide, as librosa's) so a half or double tempo does not win a tie. Beats follow by
//! dynamic programming (Ellis 2007, "Beat tracking by dynamic programming"): each beat is the
//! best onset about one period after the previous one. Bars assume 4/4 and take the phase whose
//! beats carry the most low-band onset (the kick and bass land on the downbeat).

use serde_json::{Value, json};

/// Analysis frames per second.
pub const FPS: usize = 100;

/// Beats, bars and the ending of a stretch of music. Times are seconds from `offset`.
#[derive(Debug, Clone, PartialEq)]
pub struct Beats {
    pub bpm: f64,
    pub beats: Vec<f64>,
    pub bars: Vec<f64>,
    /// Beats with a much stronger onset than the rest: hits worth a cut or a title.
    pub hits: Vec<f64>,
    /// The last strong onset before the music dies away (the last chord), if it does.
    pub last_hit: Option<f64>,
    /// From here to the end the sound stays 40 dB under its loudest, if it does.
    pub silent_from: Option<f64>,
}

impl Beats {
    pub fn json(&self, snap: Option<f64>) -> Value {
        let at = |t: f64| match snap {
            Some(fps) if fps > 0.0 => (t * fps).round() / fps,
            _ => (t * 1000.0).round() / 1000.0,
        };
        let list = |v: &[f64]| v.iter().map(|t| at(*t)).collect::<Vec<_>>();
        json!({
            "bpm": (self.bpm * 10.0).round() / 10.0,
            "beatSeconds": (60.0 / self.bpm * 10000.0).round() / 10000.0,
            "beats": list(&self.beats),
            "bars": list(&self.bars),
            "hits": list(&self.hits),
            "ending": {"lastHit": self.last_hit.map(at), "silentFrom": self.silent_from.map(at)},
        })
    }
}

/// Spectral bands: log-spaced edges from 40 Hz to 10 kHz.
const BANDS: usize = 24;
/// Bands below this are the low end (kick, bass) the downbeat is found from.
const LOW_HZ: f32 = 200.0;

/// Per analysis frame: the onset strength (spectral flux: the summed rise of each band's level in
/// dB, as librosa's onset_strength), the low end's own flux, and the frame's RMS.
fn features(samples: &[f32], rate: u32) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    const N: usize = 2048;
    let fft = redrob_film_audio_dsp::fft::Fft::new(N);
    let window: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect();
    let r = rate.max(1) as f32;
    let edges: Vec<usize> = (0..=BANDS)
        .map(|b| {
            let hz = 40.0 * (10_000.0f32 / 40.0).powf(b as f32 / BANDS as f32);
            ((hz / r * N as f32).round() as usize).clamp(1, N / 2)
        })
        .collect();
    let low_bands = (0..BANDS).filter(|b| 40.0 * (10_000.0f32 / 40.0).powf((*b as f32 + 0.5) / BANDS as f32) < LOW_HZ).count().max(1);
    // frame k is centred on sample k * rate / FPS exactly, so a rate that is not a multiple of
    // FPS (22050) does not drift the times
    let frames = samples.len() * FPS / rate.max(1) as usize;
    let (mut re, mut im) = (vec![0.0f32; N], vec![0.0f32; N]);
    let mut level: Vec<[f32; BANDS]> = Vec::with_capacity(frames);
    let mut rms = Vec::with_capacity(frames);
    for k in 0..frames {
        let centre = k * rate as usize / FPS;
        // the window's peak sits a quarter window before the frame time: a centred long window's
        // level rises before the note it is reaching (onsets read ~20 ms early)
        let start = centre as isize - (3 * N / 4) as isize;
        let mut sq = 0.0f32;
        for i in 0..N {
            let x = usize::try_from(start + i as isize).ok().and_then(|j| samples.get(j)).copied().unwrap_or(0.0);
            sq += x * x;
            re[i] = x * window[i];
            im[i] = 0.0;
        }
        rms.push((sq / N as f32).sqrt());
        fft.forward(&mut re, &mut im);
        let mut row = [0.0f32; BANDS];
        for (b, v) in row.iter_mut().enumerate() {
            let (lo, hi) = (edges[b], edges[b + 1].max(edges[b] + 1));
            let e: f32 = (lo..hi).map(|i| re[i] * re[i] + im[i] * im[i]).sum::<f32>() / (hi - lo) as f32;
            *v = 10.0 * (e + 1e-10).log10();
        }
        level.push(row);
    }
    // levels more than 80 dB under the loudest are the floor: noise in a quiet band must not read
    // as onsets (librosa's top_db); the rise is taken over 2 frames (20 ms), about librosa's hop
    let top = level.iter().flat_map(|r| r.iter().copied()).fold(f32::NEG_INFINITY, f32::max);
    for r in &mut level {
        for v in r.iter_mut() {
            *v = v.max(top - 80.0);
        }
    }
    let (mut flux, mut low) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    for k in 2..frames {
        let rise = |b: usize| (level[k][b] - level[k - 2][b]).max(0.0);
        flux[k] = (0..BANDS).map(rise).sum::<f32>() / BANDS as f32;
        low[k] = (0..low_bands).map(rise).sum::<f32>() / low_bands as f32;
    }
    (flux, low, rms)
}

/// `x` minus its mean over about half a second either side, floored at 0, then scaled to unit
/// standard deviation.
fn whiten(x: &[f32]) -> Vec<f32> {
    let k = FPS / 2;
    let mut prefix = vec![0.0f64; x.len() + 1];
    for (i, v) in x.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(*v);
    }
    let out: Vec<f32> = (0..x.len())
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(k), (i + k + 1).min(x.len()));
            let mean = (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
            (f64::from(x[i]) - mean).max(0.0) as f32
        })
        .collect();
    let n = out.len().max(1) as f32;
    let mean = out.iter().sum::<f32>() / n;
    let sd = (out.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n).sqrt().max(1e-6);
    out.into_iter().map(|v| v / sd).collect()
}

/// The beat period in analysis frames (fractional), from the onset curve's autocorrelation.
fn period(odf: &[f32]) -> Option<f64> {
    let (lo, hi) = (FPS * 60 / 200, FPS * 60 / 60);
    if odf.len() < hi * 3 {
        return None;
    }
    // a tempogram: the autocorrelation of 8 s windows a second apart, each scaled by its own
    // energy, then averaged (as librosa's tempo estimate), so a few loud bars do not decide it
    let win = (8 * FPS).min(odf.len());
    let starts: Vec<usize> = (0..=odf.len() - win).step_by(FPS).collect();
    let mut raw = vec![0.0f64; hi + 3 - lo];
    for s in &starts {
        let w = &odf[*s..*s + win];
        let zero: f64 = w.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>().max(1e-9);
        for (j, lag) in (lo - 1..=hi + 1).enumerate() {
            raw[j] += (lag..win).map(|i| f64::from(w[i]) * f64::from(w[i - lag])).sum::<f64>() / zero;
        }
    }
    let weight = |lag: usize| {
        let bpm = 60.0 * FPS as f64 / lag as f64;
        (-0.5 * (bpm / 120.0).log2().powi(2)).exp()
    };
    let best = (1..raw.len() - 1).max_by(|a, b| (raw[*a] * weight(lo - 1 + a)).total_cmp(&(raw[*b] * weight(lo - 1 + b))))?;
    // parabolic peak between the neighbouring lags
    let (y0, y1, y2) = (raw[best - 1], raw[best], raw[best + 1]);
    let d = y0 - 2.0 * y1 + y2;
    let shift = if d.abs() > 1e-12 { (0.5 * (y0 - y2) / d).clamp(-0.5, 0.5) } else { 0.0 };
    Some((lo - 1 + best) as f64 + shift)
}

/// Beat frames by dynamic programming over the onset curve, for a period `p` (frames).
fn track(odf: &[f32], p: f64) -> Vec<usize> {
    let n = odf.len();
    let mut score = vec![0.0f64; n];
    let mut back = vec![usize::MAX; n];
    let (min_gap, max_gap) = ((p * 0.5).round() as usize, (p * 2.0).round() as usize);
    for t in 0..n {
        let mut best = (0.0f64, usize::MAX);
        for gap in min_gap.max(1)..=max_gap {
            if gap > t {
                break;
            }
            let s = score[t - gap] - 100.0 * (gap as f64 / p).ln().powi(2);
            // a chain starts within the first two periods, then only continues
            let reachable = back[t - gap] != usize::MAX || t - gap < max_gap;
            if reachable && (best.1 == usize::MAX || s > best.0) {
                best = (s, t - gap);
            }
        }
        score[t] = f64::from(odf[t]) + if best.1 == usize::MAX { 0.0 } else { best.0 };
        back[t] = best.1;
    }
    // end on the best-scoring frame within the last period
    let tail = n.saturating_sub(p.ceil() as usize);
    let Some(mut t) = (tail..n).max_by(|a, b| score[*a].total_cmp(&score[*b])) else {
        return Vec::new();
    };
    let mut beats = vec![t];
    while back[t] != usize::MAX {
        t = back[t];
        beats.push(t);
    }
    beats.reverse();
    beats
}

/// Beats, bars, hits and the ending of mono `samples` at `rate`; times are `offset` + seconds.
pub fn analyze(samples: &[f32], rate: u32, offset: f64) -> Option<Beats> {
    let (flux, low_flux, rms) = features(samples, rate);
    let odf = whiten(&flux);
    let p = period(&odf)?;
    let frames = track(&odf, p);
    if frames.len() < 4 {
        return None;
    }
    let sec = |f: usize| offset + f as f64 / FPS as f64;
    // downbeat phase: the one in four whose beats carry the most low-band onset
    let low = whiten(&low_flux);
    let near = |f: usize, curve: &[f32]| curve[f.saturating_sub(2)..(f + 3).min(curve.len())].iter().copied().fold(0.0f32, f32::max);
    let phase = (0..4).max_by(|a, b| {
        let sum = |k: usize| frames.iter().skip(k).step_by(4).map(|f| near(*f, &low)).sum::<f32>() / frames.iter().skip(k).step_by(4).count().max(1) as f32;
        sum(*a).total_cmp(&sum(*b))
    })?;
    // hits: beats whose onset stands well above the beats' own spread
    let strength: Vec<f32> = frames.iter().map(|f| near(*f, &odf)).collect();
    let m = strength.iter().sum::<f32>() / strength.len() as f32;
    let sd = (strength.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / strength.len() as f32).sqrt();
    let hits: Vec<usize> = frames.iter().zip(&strength).filter(|(_, s)| **s > m + 1.5 * sd).map(|(f, _)| *f).collect();
    // ending: the sound stays 40 dB under its loudest from here on
    let peak = rms.iter().copied().fold(0.0f32, f32::max);
    let floor = peak * 0.01;
    let silent = rms.iter().rposition(|r| *r > floor).map(|i| i + 1).filter(|i| *i + FPS / 2 < rms.len());
    let last_hit = silent.and_then(|s| {
        let from = s.saturating_sub(8 * FPS);
        let end = s.min(odf.len());
        let top = odf[from..end].iter().copied().fold(0.0f32, f32::max);
        let loud = rms[from..end].iter().copied().fold(0.0f32, f32::max);
        // the last onset at least half as strong as the strongest of the closing seconds, played
        // at full level (within 6 dB of the loudest): a soft note in the ring-out is not the chord
        let level = |i: usize| rms[i..(i + FPS / 5).min(rms.len())].iter().copied().fold(0.0f32, f32::max);
        (from..end).rev().find(|i| top > 0.0 && odf[*i] >= 0.5 * top && level(*i) >= loud * 0.5)
    });
    Some(Beats {
        bpm: 60.0 * FPS as f64 / p,
        beats: frames.iter().map(|f| sec(*f)).collect(),
        bars: frames.iter().skip(phase).step_by(4).map(|f| sec(*f)).collect(),
        hits: hits.into_iter().map(sec).collect(),
        last_hit: last_hit.map(sec),
        silent_from: silent.map(sec),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drum loop: a low thump on every downbeat, a click on every beat, at `bpm`, for `secs`,
    /// then `tail` seconds of silence.
    fn groove(bpm: f64, secs: f64, tail: f64, rate: u32) -> Vec<f32> {
        let n = ((secs + tail) * f64::from(rate)) as usize;
        let mut s = vec![0.0f32; n];
        let beat = 60.0 / bpm;
        let mut k = 0;
        while k as f64 * beat < secs {
            let at = (k as f64 * beat * f64::from(rate)) as usize;
            let (hz, amp) = if k % 4 == 0 { (60.0, 0.9) } else { (2500.0, 0.35) };
            for i in 0..(rate as usize / 12) {
                if at + i < n {
                    let t = i as f32 / rate as f32;
                    s[at + i] += amp * (-t * 30.0).exp() * (std::f32::consts::TAU * hz * t).sin();
                }
            }
            k += 1;
        }
        s
    }

    #[test]
    fn a_steady_groove_gives_its_tempo_beats_and_downbeats() {
        let rate = 22050;
        let b = analyze(&groove(100.0, 20.0, 0.0, rate), rate, 10.0).unwrap();
        assert!((b.bpm - 100.0).abs() < 1.5, "{}", b.bpm);
        // every beat within 30 ms of the grid (offset 10 s)
        for t in &b.beats {
            let k = ((t - 10.0) / 0.6).round();
            assert!((t - 10.0 - k * 0.6).abs() < 0.025, "beat {t} off the grid: {:?} bpm {}", b.beats, b.bpm);
        }
        // bars on the thumps: every 2.4 s from 10 s
        for t in &b.bars {
            let k = ((t - 10.0) / 2.4).round();
            assert!((t - 10.0 - k * 2.4).abs() < 0.03, "bar {t} is not a downbeat: {:?}", b.bars);
        }
    }

    #[test]
    fn a_fast_groove_is_not_halved() {
        let rate = 22050;
        let b = analyze(&groove(150.0, 16.0, 0.0, rate), rate, 0.0).unwrap();
        assert!((b.bpm - 150.0).abs() < 2.5, "{}", b.bpm);
    }

    #[test]
    fn the_ending_is_the_last_chord_and_the_silence_after_it() {
        let rate = 22050;
        let mut s = groove(120.0, 12.0, 0.0, rate);
        // a big last chord at 12 s ringing out over 2 s, then silence
        let at = s.len();
        s.extend((0..rate as usize * 2).map(|i| {
            let t = i as f32 / rate as f32;
            1.0 * (-t * 2.5).exp() * (std::f32::consts::TAU * 110.0 * t).sin()
        }));
        s.extend(vec![0.0; rate as usize * 2]);
        let b = analyze(&s, rate, 0.0).unwrap();
        let hit = b.last_hit.unwrap();
        assert!((hit - at as f64 / f64::from(rate)).abs() < 0.05, "last hit {hit}");
        let quiet = b.silent_from.unwrap();
        assert!(quiet > 12.5 && quiet < 14.2, "silent from {quiet}");
    }

    #[test]
    fn too_little_music_gives_nothing() {
        assert!(analyze(&vec![0.0; 22050], 22050, 0.0).is_none());
    }
}
