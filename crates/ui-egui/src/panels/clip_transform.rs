//! Direct control of a video clip in the Program Monitor (Kdenlive's Transform / Position and
//! Zoom and Crop by Padding handles).
//!
//! The selected video clip under the playhead (a media clip, subclip or nested sequence; graphics
//! and masks have handles of their own) gets a frame over the picture where its Motion puts it:
//!
//! * drag inside the frame to move it (Motion ▸ Position; snaps to the frame, guides and the grid
//!   with Snap in Program Monitor on);
//! * drag a corner to resize it: the opposite corner stays where it is, Ctrl / Cmd resizes about the
//!   centre (the anchor point), Shift keeps the aspect ratio when Uniform Scale is off;
//! * with a Crop effect on the clip, drag the small bars on the edges of the cropped picture to
//!   crop it (Crop ▸ Left / Top / Right / Bottom);
//! * Alt+click cycles through the clips stacked under the pointer.
//!
//! A drag is one undo step. With Automatic Keyframes on (the red dot of the overlay toolbar), every
//! change made while the handles are dragged writes a keyframe at the playhead, so a handle moved
//! while the sequence plays records a path. [`keyframe_action`] steps between the clip's Motion and
//! Crop keyframes and adds one at the playhead (the overlay toolbar's keyframe buttons).
//!
//! Automation ids: `program.transform.box`, `program.transform.corner.<0..3>`,
//! `program.transform.crop.<left|top|right|bottom>`.

use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use redrob_film_geom::{Affine, Vec2};
use redrob_film_project::{ClipId, ItemKind, TrackItem};
use redrob_film_time::Tick;
use serde_json::json;

use crate::FilmcraftApp;

const COL: Color32 = Color32::from_rgb(0xff, 0x9f, 0x2e);
const CROP_PARAMS: [&str; 4] = ["left", "top", "right", "bottom"];
const MOTION_PARAMS: [&str; 5] = ["position", "scale", "scale_width", "rotation", "anchor"];

/// A clip the handles act on, as it is at the playhead.
#[derive(Clone)]
pub struct Target {
    pub item: TrackItem,
    /// Media time at the playhead (where Motion and Crop are evaluated).
    pub mt: Tick,
    /// The clip's source size (its own pixels).
    pub src: (u32, u32),
}

/// Whether `it` is a clip with a picture of its own that Motion moves (not a graphic or an
/// adjustment layer).
fn movable(app: &FilmcraftApp, it: &TrackItem) -> bool {
    matches!(app.session.project.item(it.item).map(|i| &i.kind), Some(ItemKind::Media(_) | ItemKind::Sequence(_) | ItemKind::Subclip { .. }))
}

/// The enabled clips on visible video tracks under the playhead that Motion moves, topmost first.
fn under_playhead(app: &FilmcraftApp) -> Vec<TrackItem> {
    let Some(seq) = app.session.active_sequence() else { return Vec::new() };
    let ph = app.session.playhead();
    seq.video_tracks.iter().rev().filter(|t| t.enabled).filter_map(|t| t.item_at(ph)).filter(|it| it.enabled && movable(app, it)).cloned().collect()
}

fn target_of(app: &FilmcraftApp, it: TrackItem) -> Target {
    let ph = app.session.playhead();
    let mt = it.effect_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
    let frame = app.session.active_sequence().map_or((1920, 1080), |q| (q.settings.width, q.settings.height));
    let src = redrob_film_render::source_size(&app.session.project, it.item).unwrap_or(frame);
    Target { item: it, mt, src }
}

/// The selected clip under the playhead the handles act on (the topmost selected one).
pub fn target(app: &FilmcraftApp) -> Option<Target> {
    let sel = &app.session.state.selection;
    let it = under_playhead(app).into_iter().find(|it| sel.contains(&it.id))?;
    Some(target_of(app, it))
}

/// Source pixels → sequence pixels for a target.
fn motion(app: &FilmcraftApp, t: &Target) -> Option<Affine> {
    let seq = app.session.active_sequence()?;
    Some(redrob_film_render::motion_matrix(seq, &t.item, t.src, redrob_film_render::source_par(&app.session.project, t.item.item), t.mt))
}

/// Sequence pixels → screen points over the picture `pic`.
fn view(pic: Rect, frame: (u32, u32)) -> Affine {
    Affine::translate(f64::from(pic.min.x), f64::from(pic.min.y))
        .then_apply(&Affine::scale(f64::from(pic.width()) / f64::from(frame.0.max(1)), f64::from(pic.height()) / f64::from(frame.1.max(1))))
}

fn sp(m: &Affine, v: Vec2) -> Pos2 {
    let q = m.apply(v);
    pos2(q.x as f32, q.y as f32)
}

fn gv(p: Pos2) -> Vec2 {
    Vec2::new(f64::from(p.x), f64::from(p.y))
}

/// The clip's Motion values at its media time: (position in sequence px, scale %, scale width %,
/// uniform). A Position or Anchor that was never set (NaN) is the frame / source centre.
fn motion_values(app: &FilmcraftApp, t: &Target) -> (Vec2, f64, f64, bool) {
    let frame = app.session.active_sequence().map_or((1920, 1080), |q| (q.settings.width, q.settings.height));
    let centre = Vec2::new(f64::from(frame.0) / 2.0, f64::from(frame.1) / 2.0);
    match t.item.effect("motion") {
        Some(m) => {
            let p = m.vec2_at("position", t.mt);
            let pos = if p.x.is_nan() || p.y.is_nan() || m.param("position").is_none() { centre } else { p };
            let uniform = m.param("uniform_scale").and_then(|p| p.value.as_bool()).unwrap_or(true);
            (pos, m.f64_at("scale", t.mt), m.f64_at("scale_width", t.mt), uniform)
        }
        None => (centre, 100.0, 100.0, true),
    }
}

/// The crop (left, top, right, bottom, percent of the source) and the Crop effect's index.
fn crop_values(t: &Target) -> Option<(usize, [f64; 4])> {
    let i = t.item.effects.iter().position(|e| e.effect == "crop" && e.enabled)?;
    let e = &t.item.effects[i];
    Some((i, CROP_PARAMS.map(|k| e.f64_at(k, t.mt).clamp(0.0, 100.0))))
}

#[derive(Clone, Copy, PartialEq)]
enum Grab {
    Move,
    /// A corner: 0 top-left, 1 top-right, 2 bottom-right, 3 bottom-left.
    Corner(usize),
    /// A crop edge: 0 left, 1 top, 2 right, 3 bottom.
    Crop(usize),
}

#[derive(Clone)]
struct Drag {
    clip: ClipId,
    grab: Grab,
    press: Pos2,
    pos0: Vec2,
    scale0: (f64, f64),
    /// The corner opposite the one grabbed, and the anchor, on screen at the press.
    opposite: Pos2,
    anchor: Pos2,
    corner: Pos2,
    /// The screen box at the press (for snapping a move).
    bbox: Rect,
}

fn drag_id() -> egui::Id {
    egui::Id::new("clip-transform-drag")
}

/// Write one Motion / Crop parameter of `clip`: one undo step per drag (`merge`; `begin` on the
/// first write), a keyframe at the playhead with Automatic Keyframes on.
fn set(app: &mut FilmcraftApp, clip: ClipId, effect: serde_json::Value, param: &str, value: serde_json::Value, begin: bool) {
    let mut p = json!({"clip": clip.0, "effect": effect, "param": param, "value": value, "merge": true, "begin": begin});
    if app.ui.program.auto_keyframe {
        p["keyframe"] = json!(true);
    }
    if let Err(e) = app.session.execute("effects.setParam", p) {
        app.ui.status = e.to_string();
    }
}

/// Draw the handles of the target clip over the Program picture `pic` and act on drags.
pub fn monitor_overlay(app: &mut FilmcraftApp, ui: &mut egui::Ui, pic: Rect, frame: (u32, u32)) {
    if !app.ui.program.edit_handles || app.session.state.selected_mask.is_some() || app.ui.mask_pen.is_some() || app.ui.eyedropper.is_some() {
        return;
    }
    if app.ui.tool != crate::state::Tool::Selection {
        return;
    }
    alt_click_cycle(app, ui, pic, frame);
    let Some(t) = target(app) else {
        ui.data_mut(|d| d.remove::<Drag>(drag_id()));
        return;
    };
    let Some(m) = motion(app, &t) else { return };
    let to_screen = view(pic, frame).then_apply(&m);
    let (w, h) = (f64::from(t.src.0), f64::from(t.src.1));
    let quad = [Vec2::new(0.0, 0.0), Vec2::new(w, 0.0), Vec2::new(w, h), Vec2::new(0.0, h)].map(|v| sp(&to_screen, v));
    let painter = ui.painter().with_clip_rect(pic.expand(16.0));
    painter.add(egui::Shape::closed_line(quad.to_vec(), Stroke::new(1.0, COL)));
    let (pos, scale, scale_w, uniform) = motion_values(app, &t);
    let anchor_screen = sp(&view(pic, frame), pos);
    painter.circle_stroke(anchor_screen, 4.0, Stroke::new(1.0, COL));
    let bbox = Rect::from_points(&quad);
    let clip = t.item.id;
    let held: Option<Drag> = ui.data(|d| d.get_temp::<Drag>(drag_id())).filter(|d| d.clip == clip);

    // ---- crop bars, then corners, then the body (later hits win)
    let crop = crop_values(&t);
    let mut crop_hits = Vec::new();
    if let Some((_, c)) = crop {
        let (l, tp, r, b) = (c[0] / 100.0 * w, c[1] / 100.0 * h, w - c[2] / 100.0 * w, h - c[3] / 100.0 * h);
        let cq = [Vec2::new(l, tp), Vec2::new(r, tp), Vec2::new(r, b), Vec2::new(l, b)].map(|v| sp(&to_screen, v));
        painter.add(egui::Shape::closed_line(cq.to_vec(), Stroke::new(1.0, Color32::from_rgb(0x5a, 0xb4, 0xff))));
        // edge midpoints: left, top, right, bottom
        let mids = [cq[3].lerp(cq[0], 0.5), cq[0].lerp(cq[1], 0.5), cq[1].lerp(cq[2], 0.5), cq[2].lerp(cq[3], 0.5)];
        for (i, p) in mids.iter().enumerate() {
            let hr = Rect::from_center_size(*p, vec2(9.0, 9.0));
            painter.rect_filled(hr.shrink(1.5), 0.0, Color32::from_rgb(0x5a, 0xb4, 0xff));
            crop_hits.push((i, hr));
        }
    }
    let mut corner_hits = Vec::new();
    for (n, c) in quad.iter().enumerate() {
        let hr = Rect::from_center_size(*c, vec2(8.0, 8.0));
        painter.rect_filled(hr, 0.0, Color32::WHITE);
        painter.rect_stroke(hr, 0.0, Stroke::new(1.0, COL), StrokeKind::Middle);
        corner_hits.push((n, hr.expand(2.0)));
    }
    let mut grabbed: Option<(Grab, egui::Response)> = None;
    let inside = |p: Pos2| {
        to_screen.inverse().is_some_and(|inv| {
            let q = inv.apply(gv(p));
            q.x >= 0.0 && q.y >= 0.0 && q.x <= w && q.y <= h
        })
    };
    // the body: a drag that starts inside the clip's frame
    let body = ui.interact(bbox.intersect(pic.expand(16.0)), egui::Id::new("clip-transform-body"), Sense::drag());
    app.auto.add("program.transform.box", bbox, "clip transform");
    let press_inside = ui.input(|i| i.pointer.press_origin()).is_some_and(inside);
    if body.hovered() && ui.ctx().pointer_hover_pos().is_some_and(inside) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
    }
    if (body.drag_started() && press_inside) || (body.dragged() && held.as_ref().is_some_and(|d| d.grab == Grab::Move)) {
        grabbed = Some((Grab::Move, body.clone()));
    }
    for (n, hr) in corner_hits {
        let r = ui.interact(hr, egui::Id::new(("clip-transform-corner", n)), Sense::drag());
        app.auto.add(&format!("program.transform.corner.{n}"), hr, "resize handle");
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(if n % 2 == 0 { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw });
        }
        if r.drag_started() || r.dragged() || r.drag_stopped() {
            grabbed = Some((Grab::Corner(n), r));
        }
    }
    for (i, hr) in crop_hits {
        let r = ui.interact(hr, egui::Id::new(("clip-transform-crop", i)), Sense::drag());
        app.auto.add(&format!("program.transform.crop.{}", CROP_PARAMS[i]), hr, "crop handle");
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(if i % 2 == 0 { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
        }
        if r.drag_started() || r.dragged() || r.drag_stopped() {
            grabbed = Some((Grab::Crop(i), r));
        }
    }
    if body.drag_stopped() && held.as_ref().is_some_and(|d| d.grab == Grab::Move) {
        grabbed = Some((Grab::Move, body.clone()));
    }
    let Some((grab, resp)) = grabbed else { return };
    let begin = resp.drag_started();
    if begin {
        let n = if let Grab::Corner(n) = grab { n } else { 0 };
        let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(quad[n]);
        ui.data_mut(|d| {
            d.insert_temp(
                drag_id(),
                Drag { clip, grab, press, pos0: pos, scale0: (scale, scale_w), opposite: quad[(n + 2) % 4], anchor: anchor_screen, corner: quad[n], bbox },
            )
        });
    }
    let Some(d) = ui.data(|m| m.get_temp::<Drag>(drag_id())).filter(|d| d.clip == clip && d.grab == grab) else { return };
    if resp.drag_stopped() {
        ui.data_mut(|m| m.remove::<Drag>(drag_id()));
    }
    let Some(now) = resp.interact_pointer_pos().or(ui.ctx().pointer_latest_pos()) else { return };
    let (kx, ky) = (f64::from(pic.width()) / f64::from(frame.0.max(1)), f64::from(pic.height()) / f64::from(frame.1.max(1)));
    let (shift, command) = ui.input(|i| (i.modifiers.shift, i.modifiers.command || i.modifiers.ctrl));
    match grab {
        Grab::Move => {
            let off = now - d.press;
            let (off, lines) = crate::panels::monitor_view::snap_move(app, pic, frame, d.bbox, off);
            crate::panels::monitor_view::draw_snap_lines(&ui.painter().with_clip_rect(pic), pic, &lines);
            let np = Vec2::new(d.pos0.x + f64::from(off.x) / kx, d.pos0.y + f64::from(off.y) / ky);
            set(app, clip, json!("motion"), "position", json!([np.x, np.y]), begin);
        }
        Grab::Corner(_) => {
            // about the anchor (Ctrl / Cmd) or keeping the opposite corner still
            let fixed = if command { d.anchor } else { d.opposite };
            let (a, b) = (d.corner - fixed, now - fixed);
            let (fx, fy) = if a.x.abs() > 1.0 && a.y.abs() > 1.0 { (f64::from(b.x / a.x), f64::from(b.y / a.y)) } else { (1.0, 1.0) };
            let even = {
                let la = a.length().max(1.0);
                f64::from(b.dot(a) / (la * la))
            };
            let keep = uniform || shift;
            let (fw, fh) = if keep { (even, even) } else { (fx, fy) };
            let (fw, fh) = (fw.max(0.01), fh.max(0.01));
            let ns = (d.scale0.0 * fh).clamp(0.0, 10_000.0);
            let nw = (d.scale0.1 * fw).clamp(0.0, 10_000.0);
            if uniform {
                set(app, clip, json!("motion"), "scale", json!(ns), begin);
            } else {
                set(app, clip, json!("motion"), "scale", json!(ns), begin);
                set(app, clip, json!("motion"), "scale_width", json!(nw), false);
            }
            if !command {
                // the anchor moves with the scale so that the opposite corner stays put
                let ax = f64::from(d.anchor.x - fixed.x);
                let ay = f64::from(d.anchor.y - fixed.y);
                let (nx, ny) = (f64::from(fixed.x) + ax * if keep { even } else { fw }, f64::from(fixed.y) + ay * if keep { even } else { fh });
                let np = Vec2::new(d.pos0.x + (nx - f64::from(d.anchor.x)) / kx, d.pos0.y + (ny - f64::from(d.anchor.y)) / ky);
                set(app, clip, json!("motion"), "position", json!([np.x, np.y]), false);
            }
        }
        Grab::Crop(i) => {
            let Some((idx, _)) = crop else { return };
            let Some(inv) = to_screen.inverse() else { return };
            let q = inv.apply(gv(now));
            let v = match i {
                0 => q.x / w * 100.0,
                1 => q.y / h * 100.0,
                2 => (w - q.x) / w * 100.0,
                _ => (h - q.y) / h * 100.0,
            };
            set(app, clip, json!(idx), CROP_PARAMS[i], json!(v.clamp(0.0, 100.0)), begin);
        }
    }
}

/// Alt+click on the picture: select the next of the clips stacked under the pointer (Kdenlive's
/// way to reach a clip hidden under a resized one).
fn alt_click_cycle(app: &mut FilmcraftApp, ui: &mut egui::Ui, pic: Rect, frame: (u32, u32)) {
    let clicked = ui.input(|i| i.modifiers.alt && i.pointer.primary_clicked());
    let Some(p) = ui.ctx().pointer_interact_pos().filter(|p| clicked && pic.contains(*p)) else { return };
    let stacked: Vec<TrackItem> = under_playhead(app)
        .into_iter()
        .filter(|it| {
            let t = target_of(app, it.clone());
            motion(app, &t).and_then(|m| view(pic, frame).then_apply(&m).inverse()).is_some_and(|inv| {
                let q = inv.apply(gv(p));
                q.x >= 0.0 && q.y >= 0.0 && q.x <= f64::from(t.src.0) && q.y <= f64::from(t.src.1)
            })
        })
        .collect();
    if stacked.is_empty() {
        return;
    }
    let sel = &app.session.state.selection;
    let next = match stacked.iter().position(|it| sel.contains(&it.id)) {
        Some(i) => &stacked[(i + 1) % stacked.len()],
        None => &stacked[0],
    };
    if let Err(e) = app.session.execute("timeline.select", json!({"clips": [next.id.0], "add": false, "toggle": false})) {
        app.ui.status = e.to_string();
    }
}

/// The overlay toolbar's keyframe buttons on the target clip's Motion and Crop: `prev` / `next`
/// move the playhead to the previous / next keyframe, `add` writes a keyframe of Position, Scale
/// (and the crop) at the playhead.
pub fn keyframe_action(app: &mut FilmcraftApp, action: &str) -> Result<(), String> {
    let t = target(app).ok_or("Select a video clip under the playhead first")?;
    let ph = app.session.playhead();
    match action {
        "prev" | "next" => {
            let mut times: Vec<Tick> = Vec::new();
            for e in t.item.effects.iter().filter(|e| e.effect == "motion" || e.effect == "crop") {
                let ids: &[&str] = if e.effect == "motion" { &MOTION_PARAMS } else { &CROP_PARAMS };
                for id in ids {
                    if let Some(p) = e.param(id) {
                        times.extend(p.keyframes.iter().filter_map(|k| redrob_film_engine::sequence_tools::timeline_time_of(&t.item, k.time)));
                    }
                }
            }
            times.sort_unstable();
            times.dedup();
            let to = if action == "next" { times.into_iter().find(|k| *k > ph) } else { times.into_iter().rev().find(|k| *k < ph) };
            let to = to.ok_or("no more keyframes that way")?;
            app.stop();
            app.session.set_playhead(to);
            Ok(())
        }
        "add" => {
            let (pos, scale, _, _) = motion_values(app, &t);
            let clip = t.item.id.0;
            let mut ops = vec![
                json!({"clip": clip, "effect": "motion", "param": "position", "value": [pos.x, pos.y], "keyframe": true}),
                json!({"clip": clip, "effect": "motion", "param": "scale", "value": scale, "keyframe": true}),
            ];
            if let Some((idx, c)) = crop_values(&t) {
                for (k, v) in CROP_PARAMS.iter().zip(c) {
                    ops.push(json!({"clip": clip, "effect": idx, "param": k, "value": v, "keyframe": true}));
                }
            }
            for op in ops {
                app.session.execute("effects.setParam", op).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        _ => Err(format!("unknown keyframe action `{action}`")),
    }
}
