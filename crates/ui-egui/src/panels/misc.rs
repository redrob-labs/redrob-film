//! Smaller panels: History, Markers (`marker_ui`), Info (the Media Browser is `panels::media_browser`).

use egui::{Align2, Rect, Sense, pos2, vec2};
use redrob_film_time::{TimeDisplay, format_time};

use crate::FilmApp;
use crate::theme::Tokens;

pub fn history(app: &mut FilmApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(6.0)));
    let undo: Vec<String> = app.session.history.undo.iter().map(|h| h.0.clone()).collect();
    let redo: Vec<String> = app.session.history.redo.iter().rev().map(|h| h.0.clone()).collect();
    let mut target: Option<i64> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
        let row = |ui: &mut egui::Ui, label: &str, current: bool, dim: bool| -> bool {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click());
            if current {
                ui.painter().rect_filled(r, 0.0, t.row_selected);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 0.0, t.hover);
            }
            ui.painter().text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, label, Tokens::ui(12.0), if dim { t.text_faint } else { t.text });
            resp.clicked()
        };
        if row(ui, tl!("Open"), undo.is_empty(), false) {
            target = Some(-(undo.len() as i64));
        }
        for (i, l) in undo.iter().enumerate() {
            if row(ui, crate::i18n::t(l), i + 1 == undo.len(), false) {
                target = Some(i as i64 + 1 - undo.len() as i64);
            }
        }
        for (i, l) in redo.iter().enumerate() {
            if row(ui, crate::i18n::t(l), false, true) {
                target = Some(i as i64 + 1);
            }
        }
    });
    if let Some(n) = target {
        if n < 0 {
            for _ in 0..-n {
                app.session.undo();
            }
        } else {
            for _ in 0..n {
                app.session.redo();
            }
        }
    }
}
/// Markers panel: see `panels::marker_ui` (search, sort, selection, categories, thumbnails).
pub fn markers(app: &mut FilmApp, ui: &mut egui::Ui, rect: Rect) {
    crate::panels::marker_ui::panel(app, ui, rect);
}

pub fn info(app: &mut FilmApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(8.0)));
    let ui = &mut child;
    let line = |ui: &mut egui::Ui, k: &str, v: String| {
        ui.horizontal(|ui| {
            ui.add_sized(vec2(90.0, 16.0), egui::Label::new(egui::RichText::new(k).color(t.text_dim)));
            ui.label(v);
        });
    };
    if let Some(item) = app.session.state.project_selection.first().and_then(|i| app.session.project.item(*i)).cloned() {
        ui.label(egui::RichText::new(&item.name).strong());
        line(ui, tl!("Type:"), crate::i18n::t(item.type_label()).to_string());
        if let Some(m) = item.as_media() {
            if let Some(v) = &m.info.video {
                line(ui, tl!("Video:"), format!("{} fps, {} x {} ({:.4})", v.frame_rate.label(), v.width, v.height, v.par.0 as f32 / v.par.1 as f32));
                line(ui, tl!("Codec:"), v.codec.clone());
            }
            if let Some(a) = m.info.audio() {
                line(ui, tl!("Audio:"), format!("{} Hz - {} ch - {}", a.sample_rate, a.channels, a.codec));
            }
        }
        line(ui, tl!("Duration:"), format_time(item.duration(), item.frame_rate(), false, TimeDisplay::Timecode, 48000));
        ui.separator();
    }
    if let Some(seq) = app.session.active_sequence() {
        let name = app.session.state.active_sequence.and_then(|s| app.session.project.item(s)).map(|i| i.name.clone()).unwrap_or_default();
        ui.label(egui::RichText::new(name).strong());
        line(
            ui,
            tl!("Settings:"),
            format!("{}x{} · {} fps · {} Hz", seq.settings.width, seq.settings.height, seq.settings.frame_rate.label(), seq.settings.sample_rate),
        );
        line(ui, tl!("Playhead:"), format_time(app.session.playhead(), seq.settings.frame_rate, seq.settings.drop_frame, TimeDisplay::Timecode, 48000));
        for (i, tr) in seq.video_tracks.iter().enumerate().rev() {
            let at = tr.item_at(app.session.playhead()).map(|x| x.name.clone()).unwrap_or_default();
            line(ui, &tlf!("Video {n}:", n = i + 1), at);
        }
        for (i, tr) in seq.audio_tracks.iter().enumerate() {
            let at = tr.item_at(app.session.playhead()).map(|x| x.name.clone()).unwrap_or_default();
            line(ui, &tlf!("Audio {n}:", n = i + 1), at);
        }
    }
    ui.separator();
    line(ui, tl!("UI:"), tlf!("{fps} fps · {n} frames queued", fps = format!("{:.0}", app.fps), n = app.frames.queue_len()));
}
