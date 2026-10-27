//! On-monitor box of the audio visualisation meters (Audio Level Visualization, Audio Spectrum,
//! Audio Waveform): with the clip or adjustment layer selected and the playhead over it, each
//! enabled meter's rectangle is drawn on the Program monitor; dragging inside moves it (Position
//! X / Y) and dragging the corner handle resizes it about its centre (Width / Height). One drag is
//! one undo step; an animated parameter gets a keyframe at the playhead; the values are saved
//! with the clip like any parameter edit.
//!
//! Automation ids: `program.vizBox.<effect index>` (body), `program.vizBox.<effect index>.size`.

use egui::{Color32, Rect, Sense, Stroke, pos2, vec2};
use redrob_film_geom::{Affine, Vec2};
use redrob_film_project::{Param, ParamValue};

use crate::FilmApp;

const METERS: [&str; 3] = ["audio_levels_viz", "audio_spectrum_viz", "audio_waveform_viz"];

pub fn monitor_overlay(app: &mut FilmApp, ui: &mut egui::Ui, pic: Rect, frame: (u32, u32)) {
    // masks and the mask pen own the monitor while they are being edited
    if app.ui.mask_pen.is_some() || app.session.state.selected_mask.is_some() || frame.0 == 0 || frame.1 == 0 {
        return;
    }
    let Some(&clip) = app.session.state.selection.first() else { return };
    let Some(seq) = app.session.active_sequence() else { return };
    let ph = app.session.playhead();
    let Some((_, it)) = seq.find_item(clip) else { return };
    if ph < it.start || ph >= it.end() || !it.effects.iter().any(|e| e.enabled && METERS.contains(&e.effect.as_str())) {
        return;
    }
    let it = it.clone();
    let mt = it.source_time_at(ph);
    let Some(seq_id) = app.session.state.active_sequence else { return };
    let project = &app.session.project;
    // an adjustment layer draws its effects over the whole output frame
    let adjustment = project.item(it.item).is_some_and(|p| matches!(p.kind, redrob_film_project::ItemKind::AdjustmentLayer { .. }));
    let size = if adjustment { frame } else { redrob_film_render::source_size(project, it.item).unwrap_or(frame) };
    let view = Affine::translate(pic.min.x as f64, pic.min.y as f64)
        .then_apply(&Affine::scale(pic.width() as f64 / frame.0 as f64, pic.height() as f64 / frame.1 as f64));
    let to_screen = if adjustment {
        view
    } else {
        view.then_apply(&redrob_film_render::motion_matrix(seq, &it, size, redrob_film_render::source_par(project, it.item), mt))
    };
    let Some(to_layer) = to_screen.inverse() else { return };
    let lin_inv = Affine { e: 0.0, f: 0.0, ..to_layer };
    let (sw, sh) = (size.0.max(1) as f64, size.1.max(1) as f64);
    let col = Color32::from_rgb(0x5a, 0xb4, 0xff);
    let painter = ui.painter().with_clip_rect(pic.expand(8.0));
    // one undo step per pointer press
    let gesture_id = egui::Id::new("viz-box-gesture");
    let mut gesture: u64 = ui.data(|d| d.get_temp(gesture_id)).unwrap_or(0);
    if ui.input(|i| i.pointer.any_pressed()) {
        gesture += 1;
        ui.data_mut(|d| d.insert_temp(gesture_id, gesture));
    }
    let mut edits: Vec<(usize, [(&'static str, f64); 2])> = Vec::new();
    for (idx, e) in it.effects.iter().enumerate() {
        if !e.enabled || !METERS.contains(&e.effect.as_str()) {
            continue;
        }
        let (x, y, w, h) = (e.f64_at("x", mt), e.f64_at("y", mt), e.f64_at("width", mt), e.f64_at("height", mt));
        let a = to_screen.apply(Vec2::new((x - w / 2.0) / 100.0 * sw, (y - h / 2.0) / 100.0 * sh));
        let b = to_screen.apply(Vec2::new((x + w / 2.0) / 100.0 * sw, (y + h / 2.0) / 100.0 * sh));
        let r = Rect::from_two_pos(pos2(a.x as f32, a.y as f32), pos2(b.x as f32, b.y as f32));
        let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        painter.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.0, col), 5.0, 3.0));
        // drag deltas → percentages of the layer
        let pct = |d: egui::Vec2| {
            let v = lin_inv.apply(Vec2::new(d.x as f64, d.y as f64));
            (v.x / sw * 100.0, v.y / sh * 100.0)
        };
        let body = ui.interact(r, egui::Id::new(("viz-box", clip.0, idx)), Sense::drag());
        app.auto.add(&format!("program.vizBox.{idx}"), r, "Meter box");
        if body.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
        }
        if body.dragged() && body.drag_delta() != egui::Vec2::ZERO {
            let (dx, dy) = pct(body.drag_delta());
            edits.push((idx, [("x", (x + dx).clamp(0.0, 100.0)), ("y", (y + dy).clamp(0.0, 100.0))]));
        }
        let hp = r.right_bottom();
        let hr = Rect::from_center_size(hp, vec2(9.0, 9.0));
        painter.rect_filled(hr.shrink(1.0), 0.0, Color32::WHITE);
        painter.rect_stroke(hr.shrink(1.0), 0.0, Stroke::new(1.0, col), egui::StrokeKind::Middle);
        let handle = ui.interact(hr.expand(3.0), egui::Id::new(("viz-box-size", clip.0, idx)), Sense::drag());
        app.auto.add(&format!("program.vizBox.{idx}.size"), hr, "Meter size");
        if handle.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
        }
        if handle.dragged() && handle.drag_delta() != egui::Vec2::ZERO {
            let (dx, dy) = pct(handle.drag_delta());
            // about the centre: the corner follows the pointer
            edits.push((idx, [("width", (w + 2.0 * dx).clamp(1.0, 100.0)), ("height", (h + 2.0 * dy).clamp(1.0, 100.0))]));
        }
    }
    if edits.is_empty() {
        return;
    }
    let key = format!("vizbox:{}:{gesture}", clip.0);
    let r = app.session.edit_merged("Change Effect Parameter", &key, |p, _| {
        let seq = p.sequence_mut(seq_id).ok_or(redrob_film_engine::EngineError::NoSequence)?;
        let Some((_, it)) = seq.find_item_mut(clip) else { return Ok(()) };
        for (idx, sets) in &edits {
            let Some(e) = it.effects.get_mut(*idx) else { continue };
            for (pid, v) in sets {
                if !e.params.contains_key(*pid) {
                    let default = e.def().and_then(|d| d.param(pid)).map_or(ParamValue::Float(*v), |d| d.default.clone());
                    e.params.insert(pid.to_string(), Param::new(default));
                }
                if let Some(prm) = e.params.get_mut(*pid) {
                    prm.set_at(mt, ParamValue::Float(*v));
                }
            }
        }
        Ok(())
    });
    if let Err(err) = r {
        app.ui.status = err.to_string();
    }
}
