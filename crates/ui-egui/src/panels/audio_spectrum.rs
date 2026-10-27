//! Audio Spectrum: the level per frequency (dBFS, log frequency axis) of the Mix at the
//! playhead, for the frame playing or parked on. One 4096-sample Hann-windowed FFT of the
//! channels summed to mono, from the same Mix the Audio Meters play (`PreviewStore::mix`);
//! recomputed when the playhead or the sequence changes, so a paused frame costs nothing.
//!
//! Hovering the plot shows the frequency and level under the pointer. Automation ids:
//! `audioSpectrum.plot` (label = the loudest frequency), `audioSpectrum.hover`.

use std::sync::Arc;

use egui::{Align2, Color32, Rect, Stroke, pos2, vec2};

use crate::FilmApp;
use crate::theme::Tokens;

/// FFT size (≈ 85 ms at 48 kHz).
pub const N: usize = 4096;
/// Lowest frequency shown.
const F_LO: f32 = 20.0;
/// Level range of the plot.
const DB_LO: f32 = -100.0;
const DB_HI: f32 = 0.0;

/// The spectrum of one block: dBFS per bin 0..N/2 (a full-scale sine reads 0 dB).
#[derive(Clone, Debug, PartialEq)]
pub struct Spectrum {
    pub sample_rate: f32,
    pub db: Vec<f32>,
}

impl Spectrum {
    pub fn bin_hz(&self, bin: usize) -> f32 {
        bin as f32 * self.sample_rate / N as f32
    }
    /// The level at a frequency (nearest bin).
    pub fn at(&self, hz: f32) -> f32 {
        let i = ((hz * N as f32 / self.sample_rate.max(1.0)).round() as usize).min(self.db.len().saturating_sub(1));
        self.db.get(i).copied().unwrap_or(DB_LO)
    }
    /// (frequency, level) of the loudest bin above the lowest shown frequency.
    pub fn peak(&self) -> Option<(f32, f32)> {
        let first = (F_LO * N as f32 / self.sample_rate.max(1.0)).ceil() as usize;
        self.db.iter().enumerate().skip(first.max(1)).max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, &d)| (self.bin_hz(i), d))
    }
}

/// The spectrum of `chans` (summed to mono; shorter input is zero-padded).
pub fn analyse(chans: &[Vec<f32>], sample_rate: f32) -> Spectrum {
    let fft = redrob_film_audio_dsp::fft::Fft::new(N);
    let mut re = vec![0f32; N];
    let mut im = vec![0f32; N];
    let nch = chans.len().max(1) as f32;
    let mut wsum = 0f32;
    for (i, r) in re.iter_mut().enumerate() {
        let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos();
        wsum += w;
        let s: f32 = chans.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / nch;
        *r = s * w;
    }
    fft.forward(&mut re, &mut im);
    let norm = 2.0 / wsum.max(1e-9);
    let db = (0..=N / 2).map(|k| (20.0 * ((re[k] * re[k] + im[k] * im[k]).sqrt() * norm).max(1e-6).log10()).max(DB_LO)).collect();
    Spectrum { sample_rate, db }
}

/// The spectrum at the playhead, cached by (sequence, sample, revision).
fn current(app: &mut FilmApp, ctx: &egui::Context) -> Option<Arc<Spectrum>> {
    let seq_id = app.session.state.active_sequence?;
    let sr = app.session.active_sequence()?.settings.sample_rate as i64;
    let now = app.session.playhead().to_units_floor(sr);
    let key = (seq_id.0, now, app.session.revision);
    let id = egui::Id::new("audio-spectrum");
    if let Some((k, s)) = ctx.data(|d| d.get_temp::<((u64, i64, u64), Arc<Spectrum>)>(id))
        && k == key
    {
        return Some(s);
    }
    let provider = app.session.media.provider(app.session.project.clone(), app.session.services.clone());
    let buf = app.session.previews.mix(&app.session.project, seq_id, now, N, &provider);
    let s = Arc::new(analyse(&buf.channels, sr as f32));
    ctx.data_mut(|d| d.insert_temp(id, (key, s.clone())));
    Some(s)
}

fn hz_label(hz: f32) -> String {
    if hz >= 1000.0 { format!("{}k", (hz / 100.0).round() / 10.0) } else { format!("{}", hz.round()) }
}

pub fn show(app: &mut FilmApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
    let Some(spec) = current(app, &ctx) else {
        crate::dock::placeholder(ui, rect, &t, tl!("(no sequence)"));
        return;
    };
    let plot = Rect::from_min_max(rect.min + vec2(38.0, 10.0), rect.max - vec2(10.0, 22.0));
    if plot.width() < 20.0 || plot.height() < 20.0 {
        return;
    }
    let f_hi = (spec.sample_rate / 2.0).max(F_LO * 2.0);
    let (l_lo, l_hi) = (F_LO.ln(), f_hi.ln());
    let x_of = |hz: f32| plot.min.x + (hz.max(F_LO).ln() - l_lo) / (l_hi - l_lo) * plot.width();
    let hz_of = |x: f32| (l_lo + (x - plot.min.x) / plot.width() * (l_hi - l_lo)).exp();
    let y_of = |db: f32| plot.max.y - (db.clamp(DB_LO, DB_HI) - DB_LO) / (DB_HI - DB_LO) * plot.height();
    let painter = ui.painter();
    let grid = Stroke::new(1.0, Color32::from_gray(40));
    let label = Color32::from_gray(150);
    let font = Tokens::ui(9.5);
    for db in (DB_LO as i32..=DB_HI as i32).step_by(20) {
        let y = y_of(db as f32);
        painter.line_segment([pos2(plot.min.x, y), pos2(plot.max.x, y)], grid);
        painter.text(pos2(plot.min.x - 4.0, y), Align2::RIGHT_CENTER, format!("{db}"), font.clone(), label);
    }
    for hz in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0] {
        if hz > f_hi {
            continue;
        }
        let x = x_of(hz);
        painter.line_segment([pos2(x, plot.min.y), pos2(x, plot.max.y)], grid);
        painter.text(pos2(x, plot.max.y + 3.0), Align2::CENTER_TOP, hz_label(hz), font.clone(), label);
    }
    painter.text(pos2(plot.min.x - 4.0, plot.max.y + 3.0), Align2::RIGHT_TOP, "dB", font.clone(), label);
    // the curve: one point per screen column (the loudest bin under it)
    let cols = plot.width().max(1.0) as usize;
    let mut pts = Vec::with_capacity(cols);
    for c in 0..=cols {
        let x = plot.min.x + c as f32;
        let (a, b) = (hz_of(x), hz_of(x + 1.0));
        let (ia, ib) =
            ((a * N as f32 / spec.sample_rate) as usize, ((b * N as f32 / spec.sample_rate) as usize).max((a * N as f32 / spec.sample_rate) as usize + 1));
        let db = spec.db.get(ia.min(spec.db.len() - 1)..ib.min(spec.db.len())).and_then(|s| s.iter().copied().reduce(f32::max)).unwrap_or_else(|| spec.at(a));
        pts.push(pos2(x, y_of(db)));
    }
    painter.add(egui::Shape::line(pts, Stroke::new(1.2, Color32::from_rgb(0x5c, 0xd0, 0x8a))));
    let peak = spec.peak().map(|(hz, db)| format!("{} Hz  {db:.1} dB", hz.round())).unwrap_or_default();
    painter.text(plot.right_top() + vec2(-4.0, 2.0), Align2::RIGHT_TOP, &peak, font.clone(), label);
    app.auto.add("audioSpectrum.plot", plot, &peak);
    // hover: frequency and level under the pointer
    if let Some(pos) = ctx.pointer_hover_pos()
        && plot.contains(pos)
        && ui.rect_contains_pointer(plot)
    {
        let hz = hz_of(pos.x);
        let text = format!("{} Hz  {:.1} dB", hz.round(), spec.at(hz));
        let painter = ui.painter();
        painter.line_segment([pos2(pos.x, plot.min.y), pos2(pos.x, plot.max.y)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 120)));
        let r = painter.text(pos + vec2(6.0, -6.0), Align2::LEFT_BOTTOM, &text, Tokens::ui(10.0), Color32::WHITE);
        app.auto.add("audioSpectrum.hover", r, &text);
    }
}
