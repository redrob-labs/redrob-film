//! `review_video`: the checks a human reviewer of a short makes, measured instead of eyeballed, so
//! an agent can fix its edit before showing it. Each check is a pure function over measurements
//! the server takes from the engine (frame thumbnails, the mixed audio, the caption tracks, a
//! transcript of the mix), so it is testable without footage.
//!
//! What it catches, and why each one is here (all found by people on real edits):
//! - a caption with no voice under it, or starting long before / ending long after its line;
//! - a spoken line with no caption;
//! - cuts that are not edit points: the source's own fast cuts inside a chosen range (a montage
//!   burst flashing by), single-frame flashes, black or flat frames, a black first frame;
//! - an ending that stops instead of resolving (picture or sound still up in the last frames);
//! - peaks above -1 dBFS and big level jumps between shots;
//! - caption faces that are too thin to read on a phone.
//!
//! Approach after browser-use/video-use (self-eval at every boundary before showing anything),
//! win4r/video-director (measure what you cannot watch) and the gaps both left in our tests: no
//! per-caption speech check, no look inside a range for source cuts, no ending check.

use serde_json::{Value, json};

/// One frame sample: time (s), mean luma 0..255, and a small luma grid for differencing.
#[derive(Clone, Debug)]
pub struct FrameSample {
    pub t: f64,
    pub luma: f32,
    pub grid: Vec<u8>,
}

/// A finding: severity ("fail" | "warn"), time span, check id and a plain description with the fix.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub severity: &'static str,
    pub start: f64,
    pub end: f64,
    pub check: &'static str,
    pub message: String,
}

impl Finding {
    pub fn json(&self) -> Value {
        json!({"severity": self.severity, "start": round(self.start), "end": round(self.end), "check": self.check, "message": self.message})
    }
}

fn round(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Mean absolute difference of two luma grids (0..255).
pub fn grid_diff(a: &[u8], b: &[u8]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| (i16::from(*x) - i16::from(*y)).unsigned_abs() as u32).sum::<u32>() as f32 / a.len() as f32
}

/// Cut times between consecutive samples whose grids differ by more than `threshold`.
pub fn detect_cuts(frames: &[FrameSample], threshold: f32) -> Vec<f64> {
    frames.windows(2).filter(|w| grid_diff(&w[0].grid, &w[1].grid) > threshold).map(|w| w[1].t).collect()
}

/// Picture checks: unplanned cut bursts, single-frame flashes, black first frame, black/flat frames
/// away from planned fades. `edits` are the timeline's edit points (clip boundaries) and
/// `transitions` the spans of transitions, where cuts and dark frames are expected.
pub fn picture(frames: &[FrameSample], step: f64, edits: &[f64], transitions: &[(f64, f64)], duration: f64) -> Vec<Finding> {
    let mut out = Vec::new();
    let near = |t: f64, list: &[f64], tol: f64| list.iter().any(|e| (e - t).abs() <= tol);
    let in_trans = |t: f64| transitions.iter().any(|(a, b)| t >= a - step && t <= b + step);
    let cuts = detect_cuts(frames, 38.0);
    let unplanned: Vec<f64> = cuts.into_iter().filter(|t| !near(*t, edits, step * 1.5) && !in_trans(*t)).collect();
    // group unplanned cuts closer than 1 s: two or more in a second is a source montage burst
    let mut i = 0;
    while i < unplanned.len() {
        let mut j = i;
        while j + 1 < unplanned.len() && unplanned[j + 1] - unplanned[j] < 1.0 {
            j += 1;
        }
        let n = j - i + 1;
        let (a, b) = (unplanned[i], unplanned[j]);
        if n >= 2 {
            out.push(Finding {
                severity: "fail",
                start: a - step,
                end: b + step,
                check: "source_cuts",
                message: format!(
                    "{n} cuts in {:.1} s that are not edit points: the source's own fast cuts inside a chosen range flash by. Move the clip's in/out so the range holds one source shot (find the source's shot boundaries with render_contact_sheet on the source)",
                    (b - a).max(step)
                ),
            });
        } else {
            out.push(Finding {
                severity: "warn",
                start: a - step,
                end: a + step,
                check: "source_cuts",
                message:
                    "a cut that is not an edit point (a shot change inside the source range); fine if intended, otherwise trim the range to one source shot"
                        .into(),
            });
        }
        i = j + 1;
    }
    // single-frame flash: one sample far brighter (or darker) than both neighbours
    for w in frames.windows(3) {
        let (a, b, c) = (w[0].luma, w[1].luma, w[2].luma);
        if ((b - a) > 70.0 && (b - c) > 70.0) || ((a - b) > 70.0 && (c - b) > 70.0) {
            out.push(Finding {
                severity: if in_trans(w[1].t) { "warn" } else { "fail" },
                start: w[1].t - step,
                end: w[1].t + step,
                check: "flash",
                message: "a one-frame flash; on a phone it reads as a glitch. Replace a flash transition with a hard cut or a short dissolve, or trim the source frame".into(),
            });
        }
    }
    if let Some(f) = frames.first()
        && f.luma < 12.0
    {
        out.push(Finding {
            severity: "fail",
            start: 0.0,
            end: step,
            check: "first_frame",
            message: "the first frame is black: feeds and thumbnails show it. Start on a picture (trim the head or move the fade-in later)".into(),
        });
    }
    // black runs away from transitions and the last 2 s
    let mut run: Option<f64> = None;
    for f in frames {
        let dark = f.luma < 10.0 && !in_trans(f.t) && f.t < duration - 2.0 && f.t > step;
        match (dark, run) {
            (true, None) => run = Some(f.t),
            (false, Some(a)) => {
                if f.t - a >= step * 2.0 {
                    out.push(Finding {
                        severity: "warn",
                        start: a,
                        end: f.t,
                        check: "black",
                        message: "black frames inside the edit (a source fade or an empty gap); trim them unless it is a designed beat".into(),
                    });
                }
                run = None;
            }
            _ => {}
        }
    }
    out
}

/// Caption checks against speech. `cues` are (start, end, text); `speech` are spoken spans
/// (start, end, words) from the transcript of the mix, already merged into lines.
pub fn captions(cues: &[(f64, f64, String)], speech: &[(f64, f64, String)]) -> Vec<Finding> {
    let mut out = Vec::new();
    let overlap = |a0: f64, a1: f64, b0: f64, b1: f64| (a1.min(b1) - a0.max(b0)).max(0.0);
    for (s, e, text) in cues {
        let len = (e - s).max(0.01);
        let covered: f64 = speech.iter().map(|(a, b, _)| overlap(*s, *e, *a, *b)).sum();
        let first = speech.iter().filter(|(a, b, _)| overlap(*s, *e, *a, *b) > 0.0).map(|(a, _, _)| *a).fold(f64::INFINITY, f64::min);
        let last = speech.iter().filter(|(a, b, _)| overlap(*s, *e, *a, *b) > 0.0).map(|(_, b, _)| *b).fold(f64::NEG_INFINITY, f64::max);
        if covered / len < 0.15 {
            out.push(Finding {
                severity: "fail",
                start: *s,
                end: *e,
                check: "caption_no_voice",
                message: format!("caption \"{text}\" has no voice under it ({:.0}% of it). Remove it, or if it is a title, make it a graphic so it does not read as dialogue", covered / len * 100.0),
            });
            continue;
        }
        if first.is_finite() && first - s > 0.5 {
            out.push(Finding {
                severity: "fail",
                start: *s,
                end: first,
                check: "caption_early",
                message: format!(
                    "caption \"{text}\" appears {:.1} s before the voice; start it at {:.2} s (0.1 s before the first word)",
                    first - s,
                    (first - 0.1).max(0.0)
                ),
            });
        }
        if last.is_finite() && e - last > 1.0 {
            out.push(Finding {
                severity: "warn",
                start: last,
                end: *e,
                check: "caption_late",
                message: format!("caption \"{text}\" stays {:.1} s after the voice; end it at {:.2} s", e - last, last + 0.5),
            });
        }
    }
    for (a, b, words) in speech {
        let shown: f64 = cues.iter().map(|(s, e, _)| overlap(*s, *e, *a, *b)).sum();
        if shown / (b - a).max(0.01) < 0.3 && b - a > 0.6 {
            out.push(Finding {
                severity: "fail",
                start: *a,
                end: *b,
                check: "voice_no_caption",
                message: format!("a spoken line has no caption: \"{words}\" ({a:.2}-{b:.2} s)"),
            });
        }
    }
    out
}

/// Merge transcript words (start, end, text) into lines: a gap over `gap` s starts a new line.
/// Words with no duration or with repeated text right after an identical line (a known Whisper
/// hallucination over music) are dropped.
pub fn speech_lines(words: &[(f64, f64, String)], gap: f64) -> Vec<(f64, f64, String)> {
    let mut lines: Vec<(f64, f64, String)> = Vec::new();
    for (s, e, t) in words {
        if e - s <= 0.0 || t.trim().is_empty() {
            continue;
        }
        match lines.last_mut() {
            // a sentence end closes the line: one caption per sentence
            Some(l) if s - l.1 <= gap && !l.2.trim_end().ends_with(['.', '?', '!', '…']) => {
                l.1 = *e;
                l.2.push(' ');
                l.2.push_str(t.trim());
            }
            _ => lines.push((*s, *e, t.trim().to_string())),
        }
    }
    let mut out: Vec<(f64, f64, String)> = Vec::new();
    for l in lines {
        let norm = |x: &str| x.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
        if out.last().is_some_and(|p| norm(&p.2) == norm(&l.2)) {
            continue;
        }
        out.push(l);
    }
    // A one- or two-word fragment with no sentence end, a few seconds before the next line, is
    // that line's first words placed early (Whisper anchors them on the music before the voice):
    // join it to the line it opens, just ahead of it.
    let mut k = 0;
    while k + 1 < out.len() {
        let (s, e, t) = out[k].clone();
        let gap_next = out[k + 1].0 - e;
        let open = !t.trim_end().ends_with(['.', '?', '!', '…']);
        if open && t.split_whitespace().count() <= 2 && gap_next > gap && gap_next < 3.0 {
            let next = &mut out[k + 1];
            next.0 = (next.0 - (e - s).min(0.6) - 0.1).max(e.min(next.0));
            next.2 = format!("{t} {}", next.2);
            out.remove(k);
            continue;
        }
        k += 1;
    }
    out
}

/// The voice-band (about 250-3500 Hz) energy of `samples` in 20 ms windows, in linear power.
pub fn voice_envelope(samples: &[f32], rate: u32) -> Vec<f32> {
    let r = rate.max(1) as f32;
    let a_lo = 1.0 - (-std::f32::consts::TAU * 250.0 / r).exp();
    let a_hi = 1.0 - (-std::f32::consts::TAU * 3500.0 / r).exp();
    let (mut lo, mut hi) = (0.0f32, 0.0f32);
    let win = ((rate / 50) as usize).max(1);
    let mut out = Vec::with_capacity(samples.len() / win + 1);
    let (mut acc, mut n) = (0.0f32, 0usize);
    for x in samples {
        hi += a_hi * (x - hi);
        lo += a_lo * (hi - lo);
        let band = hi - lo;
        acc += band * band;
        n += 1;
        if n == win {
            out.push(acc / win as f32);
            acc = 0.0;
            n = 0;
        }
    }
    out
}

/// Speech recognisers stretch a word over the silence or music next to it (the first word of a
/// line often starts seconds early). A word longer than it can be spoken is shrunk to the window
/// of its plausible length where the voice band is strongest inside its span.
pub fn fit_words(words: &[(f64, f64, String)], env: &[f32]) -> Vec<(f64, f64, String)> {
    let at = |t: f64| ((t * 50.0).max(0.0) as usize).min(env.len());
    words
        .iter()
        .map(|(s, e, t)| {
            let chars = t.chars().filter(|c| c.is_alphanumeric()).count().max(1) as f64;
            let cap = (0.25 + 0.09 * chars).clamp(0.35, 1.5);
            if e - s <= cap * 1.3 || env.is_empty() {
                return (*s, *e, t.clone());
            }
            let (i0, i1, w) = (at(*s), at(*e), ((cap * 50.0) as usize).max(1));
            if i1 <= i0 + w {
                return (*s, *e, t.clone());
            }
            let mut best = (i0, f32::NEG_INFINITY);
            let mut sum: f32 = env[i0..i0 + w].iter().sum();
            for k in i0..=(i1 - w) {
                if k > i0 {
                    sum += env[k + w - 1] - env[k - 1];
                }
                if sum > best.1 {
                    best = (k, sum);
                }
            }
            let start = best.0 as f64 / 50.0;
            (start, start + cap, t.clone())
        })
        .collect()
}

/// RMS in dBFS of `samples` (mono, -1..1).
pub fn rms_db(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return -120.0;
    }
    let ms = samples.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / samples.len() as f64;
    10.0 * ms.max(1e-12).log10()
}

/// Sound and ending checks: peak, the level jump between consecutive shots, and whether picture
/// and sound resolve at the end.
pub fn sound_and_ending(samples: &[f32], rate: u32, frames: &[FrameSample], edits: &[f64], duration: f64) -> Vec<Finding> {
    let mut out = Vec::new();
    let (pi, peak) = samples.iter().enumerate().fold((0, 0.0f32), |m, (i, x)| if x.abs() > m.1 { (i, x.abs()) } else { m });
    let peak_db = 20.0 * f64::from(peak).max(1e-9).log10();
    if peak_db > -1.0 {
        // where it happens, so the fix can go to the clip or effect playing there
        let t = pi as f64 / f64::from(rate.max(1));
        out.push(Finding {
            severity: "fail",
            start: round((t - 0.25).max(0.0)),
            end: round((t + 0.25).min(duration)),
            check: "peak",
            message: format!("peaks reach {peak_db:.1} dBFS at {t:.2} s (limit -1): lower the clip or effect playing there, or export with loudnessLufs -14"),
        });
    }
    let at = |t: f64| ((t.max(0.0) * f64::from(rate)) as usize).min(samples.len());
    let level = |a: f64, b: f64| rms_db(&samples[at(a)..at(b).max(at(a))]);
    let mut bounds: Vec<f64> =
        std::iter::once(0.0).chain(edits.iter().copied().filter(|e| *e > 0.2 && *e < duration - 0.2)).chain(std::iter::once(duration)).collect();
    bounds.sort_by(f64::total_cmp);
    bounds.dedup_by(|a, b| (*a - *b).abs() < 0.2);
    let shots: Vec<(f64, f64, f64)> = bounds.windows(2).map(|w| (w[0], w[1], level(w[0], w[1]))).collect();
    for p in shots.windows(2) {
        let (a, b) = (p[0].2, p[1].2);
        if a > -50.0 && b > -50.0 && (a - b).abs() > 9.0 {
            out.push(Finding {
                severity: "warn",
                start: p[1].0 - 0.3,
                end: p[1].0 + 0.3,
                check: "level_jump",
                message: format!(
                    "the sound jumps {:+.0} dB at this cut ({a:.0} -> {b:.0} dBFS RMS); even the shots with clip.audioGain or a short audio crossfade",
                    b - a
                ),
            });
        }
    }
    // ending: the last 0.3 s should be quiet and dark (or held still after a fade)
    if duration > 2.0 {
        let tail = level(duration - 0.3, duration);
        let before = level(duration - 2.3, duration - 0.3);
        if tail > -30.0 && tail > before - 6.0 {
            out.push(Finding {
                severity: "fail",
                start: duration - 2.0,
                end: duration,
                check: "ending_sound",
                message: format!("the sound is still up at the very end ({tail:.0} dBFS in the last 0.3 s): fade the music out over the last 1.5-2 s"),
            });
        }
        let last: Vec<&FrameSample> = frames.iter().filter(|f| f.t >= duration - 0.4).collect();
        let earlier: Vec<&FrameSample> = frames.iter().filter(|f| f.t >= duration - 2.0 && f.t < duration - 1.0).collect();
        let mean = |v: &[&FrameSample]| if v.is_empty() { 0.0 } else { v.iter().map(|f| f.luma).sum::<f32>() / v.len() as f32 };
        let (end_l, mid_l) = (mean(&last), mean(&earlier));
        let moving = last.windows(2).any(|w| grid_diff(&w[0].grid, &w[1].grid) > 6.0);
        if end_l > 25.0 && moving {
            out.push(Finding {
                severity: "fail",
                start: duration - 1.0,
                end: duration,
                check: "ending_picture",
                message: format!(
                    "the picture is still moving at full brightness in the last frames (luma {end_l:.0}, {mid_l:.0} a second earlier): hold the end card at least 2 s, then fade to black (scene_build fadeOut 1-1.5)"
                ),
            });
        }
    }
    out
}

/// Caption face check: the face each caption track resolves to for its text, its weight, and
/// whether a background box or an outline backs it.
pub fn caption_faces(tracks: &[(String, String, bool, f32, String)]) -> Vec<Finding> {
    let mut out = Vec::new();
    for (name, font, background, outline, sample) in tracks {
        let style = redrob_film_text::TextStyle {
            family: if font.trim().is_empty() { "Inter".into() } else { font.clone() },
            style: "SemiBold".into(),
            size: 54.0,
            ..Default::default()
        };
        let base = redrob_film_text::resolve(&style.family, &style.style).face;
        let mut weights: Vec<(String, u16)> = Vec::new();
        for c in sample.chars().filter(|c| !c.is_whitespace()) {
            let f = redrob_film_text::fonts::face(redrob_film_text::fonts::fallback_for(c, base));
            if !weights.iter().any(|(n, _)| *n == f.info.family) {
                weights.push((f.info.family.clone(), f.info.weight));
            }
        }
        for (fam, w) in &weights {
            if *w < 500 {
                out.push(Finding {
                    severity: "fail",
                    start: 0.0,
                    end: 0.0,
                    check: "caption_font",
                    message: format!("caption track \"{name}\" draws text in {fam} at weight {w}: too thin for a phone. Set captions.setStyle font \"Pretendard\" (bundled, SemiBold)"),
                });
            }
        }
        if weights.len() > 1 {
            out.push(Finding {
                severity: "warn",
                start: 0.0,
                end: 0.0,
                check: "caption_font_mix",
                message: format!(
                    "caption track \"{name}\" mixes faces ({}): punctuation and Hangul look different. Use one face that covers both, e.g. font \"Pretendard\"",
                    weights.iter().map(|(n, w)| format!("{n} {w}")).collect::<Vec<_>>().join(", ")
                ),
            });
        }
        if !background && *outline < 1.0 {
            out.push(Finding {
                severity: "warn",
                start: 0.0,
                end: 0.0,
                check: "caption_backing",
                message: format!("caption track \"{name}\" has neither a box nor an outline; it will vanish over bright frames. Add outline 3-4 or a box"),
            });
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn frame(t: f64, v: u8) -> FrameSample {
        FrameSample { t, luma: f32::from(v), grid: vec![v; 16] }
    }

    #[test]
    fn a_burst_of_cuts_inside_a_range_fails_and_edit_points_pass() {
        // shots change at 1.0 (an edit), then 2.0, 2.25, 2.5 (inside one clip)
        let vals = [(0.0, 50), (0.5, 50), (1.0, 150), (1.5, 150), (2.0, 30), (2.25, 200), (2.5, 90), (3.0, 90)];
        let frames: Vec<FrameSample> = vals.iter().map(|(t, v)| frame(*t, *v)).collect();
        let f = picture(&frames, 0.25, &[1.0], &[], 3.0);
        let burst: Vec<&Finding> = f.iter().filter(|x| x.check == "source_cuts").collect();
        assert_eq!(burst.len(), 1, "{f:?}");
        assert_eq!(burst[0].severity, "fail");
        assert!(burst[0].start < 2.0 && burst[0].end > 2.5);
    }

    #[test]
    fn captions_need_a_voice_and_lines_need_captions() {
        let cues = vec![(3.4, 5.9, "A".to_string()), (6.0, 10.4, "B".to_string()), (14.0, 16.0, "C".to_string())];
        let speech =
            vec![(3.5, 5.8, "what brings you".to_string()), (7.5, 8.6, "looking for someone".to_string()), (21.3, 24.0, "a dangerous quest".to_string())];
        let f = captions(&cues, &speech);
        let ids: Vec<&str> = f.iter().map(|x| x.check).collect();
        assert!(ids.contains(&"caption_early"), "B starts 1.5 s early: {f:?}");
        assert!(ids.contains(&"caption_late"), "B ends 1.8 s late");
        assert!(ids.contains(&"caption_no_voice"), "C has no voice");
        assert!(ids.contains(&"voice_no_caption"), "the 21.3 s line has no caption");
        assert!(!f.iter().any(|x| x.message.contains("\"A\"")), "A is fine");
    }

    #[test]
    fn repeated_lines_and_empty_words_are_dropped() {
        let w = vec![
            (1.0, 1.4, "I'm".to_string()),
            (1.4, 2.0, "searching".to_string()),
            (5.0, 5.0, "x".to_string()),
            (6.0, 6.4, "I'm".to_string()),
            (6.4, 7.0, "searching.".to_string()),
        ];
        let l = speech_lines(&w, 0.6);
        assert_eq!(l.len(), 1, "{l:?}");
        assert_eq!(l[0].2, "I'm searching");
    }

    #[test]
    fn an_early_lone_opening_word_joins_its_line() {
        let w =
            vec![(0.84, 1.45, "What".to_string()), (3.54, 4.2, "brings".to_string()), (4.2, 6.0, "you?".to_string()), (6.2, 9.0, "For someone.".to_string())];
        let l = speech_lines(&w, 0.6);
        assert_eq!(l.len(), 2, "{l:?}");
        assert_eq!(l[0].2, "What brings you?");
        assert!((l[0].0 - 2.83).abs() < 0.01, "{l:?}");
        assert_eq!(l[1].2, "For someone.");
    }

    #[test]
    fn a_stretched_word_moves_to_where_the_voice_is() {
        // silence for 9 s, then a 1 kHz "voice" for 0.5 s
        let rate = 8000;
        let mut s = vec![0.0f32; rate as usize * 9];
        s.extend((0..rate / 2).map(|i| (i as f32 * std::f32::consts::TAU * 1000.0 / rate as f32).sin() * 0.5));
        s.extend(vec![0.0f32; rate as usize]);
        let env = voice_envelope(&s, rate);
        let w = fit_words(&[(0.5, 9.6, "What".into()), (9.6, 9.9, "brings".into())], &env);
        assert!(w[0].0 > 8.5 && w[0].1 < 9.8, "{w:?}");
        assert_eq!(w[1].0, 9.6, "a word of plausible length is left alone");
    }

    #[test]
    fn an_ending_that_stops_fails_and_a_fade_passes() {
        let rate = 1000;
        let loud: Vec<f32> = (0..5000).map(|i| (i as f32 * 0.3).sin() * 0.5).collect();
        let frames: Vec<FrameSample> = (0..50).map(|i| FrameSample { t: i as f64 * 0.1, luma: 120.0, grid: vec![(i * 7 % 255) as u8; 16] }).collect();
        let f = sound_and_ending(&loud, rate, &frames, &[], 5.0);
        assert!(f.iter().any(|x| x.check == "ending_sound"), "{f:?}");
        assert!(f.iter().any(|x| x.check == "ending_picture"));
        let faded: Vec<f32> = loud.iter().enumerate().map(|(i, x)| x * (1.0 - (i as f32 / 5000.0)).powi(4)).collect();
        let dark: Vec<FrameSample> = frames.iter().map(|f| FrameSample { luma: if f.t > 4.0 { 4.0 } else { 120.0 }, ..f.clone() }).collect();
        let g = sound_and_ending(&faded, rate, &dark, &[], 5.0);
        assert!(!g.iter().any(|x| x.check.starts_with("ending")), "{g:?}");
    }

    #[test]
    fn hangul_captions_get_a_bold_bundled_face() {
        let f = caption_faces(&[("C1".into(), "Pretendard".into(), true, 0.0, "누군가를 찾고 있어요?".into())]);
        assert!(!f.iter().any(|x| x.check == "caption_font"), "{f:?}");
    }
}
