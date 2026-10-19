//! Source and Program monitors.
//!
//! Frames come from the background [`FrameServer`](crate::frames::FrameServer) at the *smaller* of
//! the playback resolution and the on-screen resolution (so a small monitor never pays for 4K).
//! While playing we prefetch the next frames in priority order; while scrubbing the exact frame is
//! requested first and the nearest cached frame is shown until it arrives (no black flashes), or
//! failing that the frame asked for a refresh earlier, so the picture follows a scrub or a value
//! that is dragged instead of waiting for the mouse to rest.

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use redrob_film_project::ItemKind;
use redrob_film_time::{Tick, TimeDisplay, format_time};
use serde_json::json;

use crate::FilmcraftApp;
use crate::frames::{FrameKey, Target};
use crate::icons::{self, Icon};
use crate::panels::monitor_view;
use crate::state::{DisplayMode, PlaybackRes};
use crate::theme::Tokens;

#[derive(Clone, Copy, PartialEq)]
pub enum Which {
    Source,
    Program,
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, which: Which) {
    // a monitor in its own window (or full screen without Mirror) is not drawn in the dock too
    let w = window_state(ui.ctx(), which);
    if w.undocked || (w.fullscreen && !app.session.prefs.playback.mirror_fullscreen) {
        let t = app.tokens;
        crate::dock::placeholder(ui, rect, &t, if w.fullscreen { tl!("(full screen)") } else { tl!("(in its own window)") });
        let r = ui.interact(rect, egui::Id::new((which as u8, "redock")), Sense::click());
        if r.double_clicked() {
            set_window_state(ui.ctx(), which, MonitorWindow::default());
        }
        return;
    }
    show_in(app, ui, rect, which, false);
}

/// A monitor: picture and, unless `bare` (full screen), the controls under it.
fn show_in(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, which: Which, bare: bool) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let controls_h = if bare { 0.0 } else { 28.0 + 24.0 + 36.0 };
    let video_area = Rect::from_min_max(rect.min + vec2(4.0, 4.0), pos2(rect.max.x - 4.0, rect.max.y - controls_h));
    // the Source monitor's time ruler starts at `origin` (a subclip restricted to its range)
    let mut origin = Tick::ZERO;
    let mut source_tc: i64 = 0;
    let (target, frame_size, rate, time, duration, drop_frame, mark_in, mark_out, name) = match which {
        Which::Program => {
            let Some(seq_id) = app.session.state.active_sequence else {
                crate::dock::placeholder(ui, rect, &t, tl!("(no sequences)"));
                return;
            };
            // A damaged project can name an active sequence that no longer exists.
            let Some(q) = app.session.active_sequence() else {
                crate::dock::placeholder(ui, rect, &t, tl!("(no sequences)"));
                return;
            };
            (
                Target::Sequence(seq_id),
                (q.settings.width, q.settings.height),
                q.settings.frame_rate,
                app.session.playhead(),
                q.duration(),
                q.settings.drop_frame,
                q.mark_in,
                q.mark_out,
                app.session.project.item(seq_id).map(|i| i.name.clone()).unwrap_or_default(),
            )
        }
        Which::Source => {
            let Some(item) = app.session.state.source_item else {
                crate::dock::placeholder(ui, rect, &t, tl!("(no clips)"));
                return;
            };
            let Some(pi) = app.session.project.item(item) else { return };
            let Some(view) = redrob_film_engine::clip_ops::source_view(&app.session, item) else { return };
            // a subclip shows its parent media's frames
            let size = match app.session.project.item(view.media).map(|i| &i.kind) {
                Some(ItemKind::Media(m)) => m.info.video.as_ref().map(|v| (v.width, v.height)).unwrap_or((0, 0)),
                Some(ItemKind::Sequence(s)) => (s.settings.width, s.settings.height),
                _ => (1920, 1080),
            };
            let mut view = view;
            if let Some(range) = crate::panels::source_range::preview(app, &ctx) {
                view.mark_in = Some(range.start);
                view.mark_out = Some(Tick(range.end().0.saturating_sub(view.rate.frame_duration().0)).max(view.start));
            }
            origin = view.start;
            // Show Source Timecode: the media's embedded start timecode (frames at its own rate)
            if app.ui.source.source_timecode
                && let Some(ItemKind::Media(m)) = app.session.project.item(view.media).map(|i| &i.kind)
            {
                source_tc = m.info.start_timecode.unwrap_or(0).max(0);
            }
            (Target::Item(item), size, view.rate, app.session.state.source_playhead, view.end, false, view.mark_in, view.mark_out, pi.name.clone())
        }
    };
    // Display implicit full-clip marks without writing them into the project on open.
    let draft = crate::panels::source_range::preview_for(app, &ctx, which);
    let (mark_in, mark_out) = if which == Which::Source {
        (
            Some(draft.map(|r| r.start).unwrap_or(mark_in.unwrap_or(origin))),
            Some(
                draft
                    .map(|r| Tick(r.end().0.saturating_sub(rate.frame_duration().0)))
                    .unwrap_or_else(|| mark_out.unwrap_or_else(|| (duration - rate.frame_duration()).max(origin))),
            ),
        )
    } else {
        (
            mark_in.map(|m| draft.map(|r| r.start).unwrap_or(m)),
            mark_out.map(|m| draft.map(|r| Tick(r.end().0.saturating_sub(rate.frame_duration().0))).unwrap_or(m)),
        )
    };
    let range_start = mark_in.unwrap_or(origin);
    let prefix = if which == Which::Program { "program" } else { "source" };
    let mv = monitor_view::view(app, which).clone();
    let display = mv.display_mode().unwrap_or(DisplayMode::Composite);
    // Multi-Camera view: the angle grid on the left, the program on the right
    let multicam = which == Which::Program && mv.multicam;
    // Multitrack View: every video track of the sequence in a grid instead of the picture
    let multitrack = which == Which::Program && mv.multitrack && !multicam;
    // without the preview monitor the grid takes the whole picture area
    let grid_only = (multicam && !app.session.state.multicam_view.show_preview) || multitrack;
    let (grid_area, video_area) = if grid_only {
        (Some(video_area), Rect::from_min_size(video_area.max, vec2(0.0, 0.0)))
    } else if multicam {
        let mid = video_area.center().x;
        (Some(Rect::from_min_max(video_area.min, pos2(mid - 2.0, video_area.max.y))), Rect::from_min_max(pos2(mid + 2.0, video_area.min.y), video_area.max))
    } else {
        (None, video_area)
    };
    if let Some(g) = grid_area {
        if multitrack {
            multitrack_grid(app, ui, g);
        } else {
            crate::panels::multicam::grid(app, ui, g);
        }
    }
    // Settings ▸ Playback ▸ Custom monitor background
    let custom_bg = monitor_background(app);
    ui.painter().rect_filled(video_area, 0.0, custom_bg.unwrap_or(if bare { Color32::BLACK } else { t.panel_bg }));
    let has_video = frame_size.0 > 0;
    // Source waveform modes: Audio Waveform (or an audio-only clip) and the video/waveform split.
    let source_item = if which == Which::Source { app.session.state.source_item } else { None };
    let wave_only = source_item.is_some() && (!has_video || display == DisplayMode::AudioWaveform);
    let (video_area, wave_area) = if wave_only {
        (video_area, Some(video_area))
    } else if source_item.is_some() && display == DisplayMode::VideoAndWaveform {
        let split = video_area.min.y + video_area.height() * 0.62;
        (Rect::from_min_max(video_area.min, pos2(video_area.max.x, split - 2.0)), Some(Rect::from_min_max(pos2(video_area.min.x, split + 2.0), video_area.max)))
    } else {
        (video_area, None)
    };
    if let (Some(wa), Some(item)) = (wave_area, source_item) {
        monitor_view::waveform(app, ui, wa, item, time, duration, mark_in, mark_out);
    }
    let show_picture = has_video && !wave_only && !grid_only;
    // Rulers along the top and left of the picture area.
    let rulers = show_picture && mv.show_rulers;
    let (ruler_rects, video_area) = if rulers {
        let r = monitor_view::RULER;
        (
            Some((
                Rect::from_min_max(pos2(video_area.min.x + r, video_area.min.y), pos2(video_area.max.x, video_area.min.y + r)),
                Rect::from_min_max(pos2(video_area.min.x, video_area.min.y + r), pos2(video_area.min.x + r, video_area.max.y)),
            )),
            Rect::from_min_max(video_area.min + vec2(r, r), video_area.max),
        )
    } else {
        (None, video_area)
    };
    let picture_area = video_area;
    // Comparison View: the reference frame on the left, the current frame on the right.
    let compare = which == Which::Program && show_picture && display == DisplayMode::Comparison;
    let (ref_area, video_area) = if compare {
        let mid = video_area.center().x;
        let bar_h = 22.0;
        (
            Some(Rect::from_min_max(video_area.min, pos2(mid - 2.0, video_area.max.y - bar_h))),
            Rect::from_min_max(pos2(mid + 2.0, video_area.min.y), pos2(video_area.max.x, video_area.max.y - bar_h)),
        )
    } else {
        (None, video_area)
    };
    // ---- picture, at its display aspect (non-square pixels are stretched on screen)
    let ppp = ctx.pixels_per_point();
    let display_w = (frame_size.0 as f64 * monitor_view::pixel_aspect(app, which)) as f32;
    let pic = if compare {
        fit(video_area, display_w, frame_size.1 as f32)
    } else if show_picture {
        monitor_view::picture_rect(video_area, display_w, frame_size.1 as f32, &mv, ppp)
    } else {
        video_area
    };
    if which == Which::Program {
        // the voice-over countdown is drawn over the Program monitor's picture
        ctx.data_mut(|d| d.insert_temp(crate::panels::voiceover::program_picture_id(), pic));
    }
    let saved_clip = ui.clip_rect();
    ui.set_clip_rect(saved_clip.intersect(if mv.zoom.is_some() && !compare { video_area } else { picture_area.expand(12.0) }));
    if show_picture {
        ui.painter().rect_filled(pic, 0.0, custom_bg.unwrap_or(t.monitor_bg));
        let playing = if which == Which::Program { app.playback.playing } else { app.source_playback.clock.playing };
        let res = mv.effective_res(playing);
        // frame pixels per screen pixel along the axis that needs the most of them
        let screen_scale = (pic.width() * ppp / frame_size.0.max(1) as f32).max(pic.height() * ppp / frame_size.1.max(1) as f32).min(1.0);
        let scale = quantize_scale(res.scale().min(screen_scale.max(1.0 / 32.0)));
        let frame = rate.frame_at(time);
        let rev = match target {
            Target::Item(i) => app.item_revision(i),
            _ => app.session.revision,
        };
        let size_key = (scale * 1000.0) as u32;
        let channel = display.is_channel();
        let use_gpu = which == Which::Program && app.gpu.is_some() && !channel && !crate::panels::menu_dialogs::software_renderer(app);
        let cpu_target = target;
        let target = match (use_gpu, target) {
            (true, Target::Sequence(s)) => Target::SequencePlan(s),
            (_, t) => t,
        };
        // Draft decoding (opt-in): only while playing at 1/2 resolution or lower.
        let draft = crate::frames::draft_playback(playing, scale, app.session.prefs.playback.draft_decode);
        let key = FrameKey { target, frame, size: size_key, revision: rev, draft };
        let project = app.session.project.clone();
        if playing {
            let preroll = which == Which::Program && app.playback.preroll.is_some();
            let speed = if which == Which::Program { app.playback.speed } else { app.source_playback.clock.speed };
            app.frames.schedule_playback(key, rate, scale, &project, speed, preroll);
            if preroll {
                app.playback.preroll_ready = app.frames.preroll_ready(key, app.playback.speed, rate.frame_at(duration) - 1);
            }
        } else {
            app.frames.request(key, rate.tick_of(frame), scale, &project, 0);
        }
        let asked = asked_before(app, prefix, key, playing);
        let tex_name = if channel { format!("monitor-{prefix}-{display:?}") } else { format!("monitor-{prefix}") };
        // the full-screen window keeps a texture of its own (another size)
        let tex_name = if bare { format!("{tex_name}-window") } else { tex_name };
        let (shown, on_screen) = if use_gpu {
            let found = app
                .frames
                .get_plan(&key)
                .map(|p| (key, p))
                .or_else(|| app.frames.nearest_plan(key, 6))
                .or_else(|| asked.iter().find_map(|k| app.frames.get_plan(k).map(|p| (*k, p))));
            match found {
                Some((k, plan)) => (app.gpu_present(k, &plan).map(|(id, _)| id), Some(k)),
                None => (app.gpu.as_ref().and_then(|g| g.texture), app.gpu.as_ref().and_then(|g| g.last_key)),
            }
        } else {
            cpu_texture(app, &ctx, &tex_name, key, &asked, display)
        };
        let exact = on_screen == Some(key);
        if which == Which::Program && !bare {
            app.program_shown = on_screen;
        }
        if !bare {
            note_shown_frame(&ctx, prefix, on_screen.map(|k| k.frame), playing);
        }
        if !playing && !exact {
            app.monitor_inexact = true;
        }
        if playing {
            let clock = if which == Which::Program { &mut app.playback } else { &mut app.source_playback.clock };
            if std::mem::take(&mut clock.hidden) {
                clock.meter.resync(frame, exact);
            } else {
                clock.meter.refresh(frame, exact);
            }
        }
        if let Some(tex) = shown {
            ui.painter().image(tex, pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        if let Some(ra) = ref_area {
            // the reference frame (CPU path): another frame, or (Compare Effect) this frame
            // without the selected clips' effects
            let without = if app.ui.fx.compare_effects { without_effects(app, &ctx) } else { None };
            let (rkey, rf, rproject) = match &without {
                Some((p, salt)) => (FrameKey { target: cpu_target, frame, size: size_key, revision: rev ^ salt, draft: false }, frame, p.clone()),
                None => {
                    let rf = rate.frame_at(Tick(mv.compare_ref.unwrap_or(time.0)));
                    (FrameKey { target: cpu_target, frame: rf, size: size_key, revision: rev, draft: false }, rf, project.clone())
                }
            };
            app.frames.request(rkey, rate.tick_of(rf), scale, &rproject, 1);
            let rpic = fit(ra, display_w, frame_size.1 as f32);
            ui.painter().rect_filled(rpic, 0.0, custom_bg.unwrap_or(t.monitor_bg));
            if let (Some(tex), _) = cpu_texture(app, &ctx, &format!("monitor-{prefix}-ref"), rkey, &[], DisplayMode::Composite) {
                ui.painter().image(tex, rpic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            app.auto.add("program.compare.reference", rpic, "reference frame");
            let bar = Rect::from_min_max(pos2(ra.min.x, ra.max.y + 2.0), pos2(video_area.max.x, ra.max.y + 22.0));
            if without.is_some() {
                ui.painter().text(pos2(ra.center().x, bar.center().y), Align2::CENTER_CENTER, tl!("Without effects"), Tokens::ui(11.0), t.text_dim);
                ui.painter().text(pos2(video_area.center().x, bar.center().y), Align2::CENTER_CENTER, tl!("With effects"), Tokens::ui(11.0), t.text_dim);
            } else {
                monitor_view::compare_bar(app, ui, bar, rate, drop_frame);
            }
        }
        if mv.safe_margins {
            // Settings ▸ Color ▸ Monitor overlay color (white by default)
            let [or, og, ob] = redrob_film_engine::settings::parse_hex(&app.session.prefs.color.monitor_overlay_color).unwrap_or([255, 255, 255]);
            let overlay = |a: u8| Color32::from_rgba_unmultiplied(or, og, ob, a);
            for (f, c) in [(0.9, overlay(120)), (0.8, overlay(90))] {
                let r = Rect::from_center_size(pic.center(), pic.size() * f);
                ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, c), StrokeKind::Middle);
            }
            let c = pic.center();
            ui.painter().line_segment([c - vec2(8.0, 0.0), c + vec2(8.0, 0.0)], Stroke::new(1.0, overlay(120)));
            ui.painter().line_segment([c - vec2(0.0, 8.0), c + vec2(0.0, 8.0)], Stroke::new(1.0, overlay(120)));
        }
        if display != DisplayMode::Composite && !compare {
            let label = match display {
                DisplayMode::Composite => tl!("Composite"),
                DisplayMode::Alpha => tl!("Alpha"),
                DisplayMode::Red => tl!("Red"),
                DisplayMode::Green => tl!("Green"),
                DisplayMode::Blue => tl!("Blue"),
                DisplayMode::AudioWaveform => tl!("Audio Waveform"),
                DisplayMode::Comparison => tl!("Comparison"),
                DisplayMode::VideoAndWaveform => tl!("Video and Waveform"),
            };
            ui.painter().text(pos2(video_area.max.x - 8.0, video_area.min.y + 6.0), Align2::RIGHT_TOP, label, Tokens::ui(10.0), t.text_dim);
        }
    }
    // Dropped-frame indicator (Program only)
    if which == Which::Program && app.playback.playing {
        let c = if app.playback.meter.counts().1 > 0 { t.render_yellow } else { t.render_green };
        ui.painter().circle_filled(pos2(video_area.min.x + 10.0, video_area.min.y + 10.0), 4.0, c);
    }
    // A click on the Program monitor around the picture lets go of what is selected on it (#683;
    // in the picture the overlays do that). Under the picture and the overlays, so they come first.
    if which == Which::Program && show_picture && ui.interact(video_area, egui::Id::new((prefix, "background")), Sense::click()).clicked() {
        crate::panels::graphics::deselect_on_monitor(app, ui);
    }
    // Click/drag in the picture: the Hand tool pans a magnified picture, otherwise focus.
    let pic_resp = ui.interact(pic, egui::Id::new((prefix, "pic")), if show_picture { Sense::click_and_drag() } else { Sense::hover() });
    app.auto.add(&format!("{prefix}.picture"), pic.intersect(video_area), "picture");
    // (Comparison View always fits both frames)
    if show_picture && !compare {
        monitor_view::zoom_pan_input(app, ui, which, video_area, pic, display_w, ppp);
    }
    if which == Which::Program && show_picture {
        eyedropper(app, ui, &pic_resp, pic, target, rate, time);
        // Edit Mode off: no editing handles over the picture
        if mv.edit_handles {
            crate::panels::graphics::monitor_overlay(app, ui, pic, frame_size);
            crate::panels::masks::monitor_overlay(app, ui, pic, frame_size);
            crate::panels::clip_transform::monitor_overlay(app, ui, pic, frame_size);
        }
        crate::panels::viz_overlay::monitor_overlay(app, ui, pic, frame_size);
    }
    if show_picture {
        monitor_view::guides(app, ui, which, video_area, pic, frame_size);
        let pofs =
            if which == Which::Program { rate.tick_of(app.session.active_sequence().map_or(0, |q| q.start_timecode.max(0))) } else { rate.tick_of(source_tc) };
        info_overlays(app, ui, which, pic.intersect(video_area), rate, time, pofs, drop_frame, duration, &mv);
        overlay_toolbar(app, ui, which, pic.intersect(video_area), &mv);
    }
    ui.set_clip_rect(saved_clip);
    if let Some((top, left)) = ruler_rects {
        monitor_view::rulers(app, ui, which, top, left, video_area, pic, frame_size);
    }
    if which == Which::Source && show_picture && pic_resp.drag_started() && app.ui.tool != crate::state::Tool::Hand && !bare {
        crate::panels::source_drag::begin(app, ui, true, true);
    }
    // the graphics overlay covers the Program picture and takes its clicks; it hands on those
    // that land on no layer
    let (gfx_click, gfx_double) = ctx.data_mut(|d| d.remove_temp::<(bool, bool)>(egui::Id::new("gfx-overlay-click"))).unwrap_or((false, false));
    let (clicked, double) =
        (pic_resp.clicked() || (which == Which::Program && gfx_click), pic_resp.double_clicked() || (which == Which::Program && gfx_double));
    // Kdenlive: a double-click on the picture switches the monitor full screen (and back)
    if show_picture && double {
        toggle_window(&ctx, which, true, None);
        app.ui.status = name.clone();
    } else if show_picture
        && clicked
        && app.session.prefs.playback.play_on_monitor_click
        && app.ui.tool != crate::state::Tool::Hand
        && app.ui.eyedropper.is_none()
        && !ui.input(|i| i.modifiers.alt)
    {
        // Settings ▸ Playback ▸ Play/pause on monitor click
        let cmd = if which == Which::Program { "playback.toggle" } else { "source.playback.toggle" };
        if let Err(e) = crate::menus::invoke(app, &ctx, cmd, json!({"monitor": prefix})) {
            app.ui.status = e;
        }
    }
    // Kdenlive: Shift + moving the pointer over the Source picture seeks through the clip
    if which == Which::Source
        && show_picture
        && pic_resp.hovered()
        && let Some(p) = ctx.pointer_hover_pos()
        && ui.input(|i| i.modifiers.shift && i.pointer.delta().x != 0.0 && !i.pointer.any_down())
        && pic.width() > 0.0
    {
        let f = f64::from(((p.x - pic.min.x) / pic.width()).clamp(0.0, 1.0));
        let to = rate.snap(origin + Tick(((duration - origin).0 as f64 * f) as i64)).min(Tick((duration.0 - rate.frame_duration().0).max(origin.0)));
        app.stop_source();
        if let Err(e) = app.session.execute("source.setPlayhead", json!({"time": to.0})) {
            app.ui.status = e.to_string();
        }
    }
    if bare {
        return;
    }

    // ---- controls row: timecode | zoom | res | wrench | duration
    let row1 = Rect::from_min_size(pos2(rect.min.x + 14.0, rect.max.y - controls_h + 2.0), vec2(rect.width() - 28.0, 26.0));
    // the Program monitor counts from the sequence's start timecode (Sequence Settings ▸ Start Time)
    let pofs =
        if which == Which::Program { rate.tick_of(app.session.active_sequence().map_or(0, |q| q.start_timecode.max(0))) } else { rate.tick_of(source_tc) };
    let tc = format_time(time + pofs, rate, drop_frame, TimeDisplay::Timecode, 48000);
    let tc_rect = Rect::from_min_size(row1.min, vec2(110.0, row1.height()));
    let min = if which == Which::Program { Tick::ZERO } else { origin };
    match crate::widgets::timecode_field(ui, egui::Id::new((prefix, "timecode")), tc_rect, &tc, time + pofs, min + pofs, rate, drop_frame, t.timecode) {
        Some(Ok(to)) => {
            let cmd = if which == Which::Program { "playhead.set" } else { "source.setPlayhead" };
            if let Err(e) = app.session.execute(cmd, json!({"time": (to - pofs).0})) {
                app.ui.status = e.to_string();
            }
        }
        Some(Err(e)) => app.ui.status = e,
        None => {}
    }
    app.auto.add(&format!("{prefix}.timecode"), tc_rect, &tc);
    // Kdenlive's timecode spin arrows: one frame forward (up) or back (down) per click
    for (k, up) in [("up", true), ("down", false)] {
        let ar = Rect::from_min_size(pos2(tc_rect.max.x - 12.0, if up { tc_rect.min.y + 1.0 } else { tc_rect.center().y + 1.0 }), vec2(11.0, 11.0));
        let resp = ui.interact(ar, egui::Id::new((prefix, "timecode-spin", k)), Sense::click()).on_hover_text(if up {
            tl!("Step Forward 1 Frame (Right)")
        } else {
            tl!("Step Back 1 Frame (Left)")
        });
        app.auto.add(&format!("{prefix}.timecode.{k}"), ar, k);
        let c = if resp.hovered() { t.tab_text_active } else { t.text_faint };
        let (cx, cy) = (ar.center().x, ar.center().y);
        let tri = if up {
            vec![pos2(cx - 3.5, cy + 2.0), pos2(cx + 3.5, cy + 2.0), pos2(cx, cy - 2.5)]
        } else {
            vec![pos2(cx - 3.5, cy - 2.0), pos2(cx + 3.5, cy - 2.0), pos2(cx, cy + 2.5)]
        };
        ui.painter().add(egui::Shape::convex_polygon(tri, c, Stroke::NONE));
        if resp.clicked() {
            let cmd = match (which, up) {
                (Which::Program, true) => "playhead.stepForward",
                (Which::Program, false) => "playhead.stepBack",
                (Which::Source, true) => "src.stepFwd",
                (Which::Source, false) => "src.stepBack",
            };
            if which == Which::Source {
                source_nav(app, cmd);
            } else if let Err(e) = app.session.execute(cmd, json!({})) {
                app.ui.status = e.to_string();
            }
        }
    }
    let dur_tc = format_time(mark_out.map(|o| o + rate.frame_duration()).unwrap_or(duration) - range_start, rate, drop_frame, TimeDisplay::Timecode, 48000);
    ui.painter().text(pos2(row1.max.x, row1.center().y), Align2::RIGHT_CENTER, &dur_tc, Tokens::timecode(), t.text_dim);
    // zoom + resolution dropdowns centred-ish
    let zr = Rect::from_min_size(pos2(row1.min.x + 116.0, row1.min.y), vec2(70.0, 24.0));
    let zoom_label = match mv.zoom {
        None => tl!("Fit").to_string(),
        Some(z) => format!("{}%", (z * 100.0).round() as i32),
    };
    let zresp = crate::widgets::dropdown_text(ui, zr, &zoom_label, &t, egui::Id::new((prefix, "zoom")));
    app.auto.add(&format!("{prefix}.zoom"), zr, "Select Zoom Level");
    monitor_view::zoom_menu(app, &zresp, which);
    let res = mv.res;
    let rr = Rect::from_min_size(pos2(row1.max.x - 196.0, row1.min.y), vec2(62.0, 24.0));
    if which == Which::Source && rr.min.x > zr.max.x + 12.0 {
        crate::panels::source_drag::controls(app, ui, Rect::from_min_max(pos2(zr.max.x + 6.0, row1.min.y), pos2(rr.min.x - 6.0, row1.min.y + 24.0)));
    }
    let rresp = crate::widgets::dropdown_text(ui, rr, crate::i18n::t(res.label()), &t, egui::Id::new((prefix, "res")));
    app.auto.add(&format!("{prefix}.resolution"), rr, "Select Playback Resolution");
    egui::Popup::menu(&rresp).show(|ui| {
        for r in PlaybackRes::ALL {
            if ui.selectable_label(r == res, crate::i18n::t(r.label())).clicked() {
                monitor_view::view_mut(app, which).res = r;
            }
        }
    });
    let wr = Rect::from_min_size(pos2(rr.max.x + 6.0, row1.min.y + 1.0), vec2(22.0, 22.0));
    let wresp = ui.interact(wr, egui::Id::new((prefix, "wrench")), Sense::click());
    icons::paint(ui.painter(), wr.shrink(4.0), Icon::Wrench, if wresp.hovered() { t.tab_text_active } else { t.icon });
    app.auto.add(&format!("{prefix}.settings"), wr, "Settings");
    egui::Popup::menu(&wresp).show(|ui| {
        ui.set_min_width(240.0);
        monitor_view::wrench_items(app, ui, which);
        overlay_menu(app, ui, which);
        marker_menu(app, ui, which);
        if which == Which::Source {
            let r = ui.button(tl!("Extract Zone…"));
            app.auto.add("source.settings.extractZone", r.rect, "Extract Zone…");
            if r.clicked() {
                open_extract_zone(app, &ctx);
                ui.close();
            }
        }
        for (cmd, label) in [("monitor.fullscreen", tl!("Switch Monitor Fullscreen")), ("monitor.undock", tl!("Show Monitor in Its Own Window"))] {
            let r = ui.button(label);
            app.auto.add(&format!("{prefix}.settings.{cmd}"), r.rect, label);
            if r.clicked() {
                if let Err(e) = crate::menus::invoke(app, &ctx, cmd, json!({"monitor": prefix})) {
                    app.ui.status = e;
                }
                ui.close();
            }
        }
        ui.separator();
        // Kdenlive: each monitor has its own playback volume (independent of the Mix level)
        let vr = ui.add(egui::Slider::new(&mut monitor_view::view_mut(app, which).volume, 0.0..=100.0).suffix(" %").text(tl!("Volume")));
        app.auto.add(&format!("{prefix}.settings.volume"), vr.rect, "Volume");
        let mv = monitor_view::view_mut(app, which);
        ui.checkbox(&mut mv.safe_margins, tl!("Safe Margins"));
        ui.checkbox(&mut mv.show_transport, tl!("Show Transport Controls"));
        if which == Which::Source {
            let mut looping = app.source_playback.clock.looping;
            let response = ui.checkbox(&mut looping, tl!("Loop marked range"));
            app.auto.add("source.settings.loop", response.rect, "Loop marked range");
            if response.changed()
                && let Err(e) = crate::menus::invoke(app, &ctx, "playback.loop", json!({"monitor":"source"}))
            {
                app.ui.status = e;
            }
        }
        if which == Which::Program {
            ui.separator();
            ui.checkbox(&mut app.ui.show_scopes, tl!("Lumetri Scopes"));
            ui.checkbox(&mut app.playback.looping, tl!("Loop"));
            ui.separator();
            let mut follows = app.session.state.multicam_audio_follows_video;
            if ui.checkbox(&mut follows, tl!("Multi-Camera Audio Follows Video")).changed() {
                let _ = app.session.execute("multicam.audioFollowsVideo", json!({"enabled": follows}));
            }
            ui.checkbox(&mut app.ui.multicam_record, tl!("Multi-Camera Record"));
            let v = app.session.state.multicam_view.clone();
            for (cmd, label, on) in [
                ("multicam.selectionTopDown", tl!("Multi-Camera Selection Top Down"), v.top_down),
                ("multicam.showPreviewMonitor", tl!("Show Multi-Camera Preview Monitor"), v.show_preview),
                ("multicam.autoAdjustQuality", tl!("Auto-Adjust Multi-Camera Playback Quality"), v.auto_quality),
                ("multicam.transmitView", tl!("Transmit Multi-Camera View"), v.transmit),
            ] {
                let mut b = on;
                if ui.checkbox(&mut b, label).changed() {
                    let _ = app.session.execute(cmd, json!({"enabled": b}));
                }
            }
            ui.menu_button(tl!("Multi-Camera Layout"), |ui| {
                for (k, label) in [("auto", tl!("Automatic")), ("2x2", "2 × 2"), ("3x3", "3 × 3"), ("4x4", "4 × 4")] {
                    if ui.selectable_label(v.layout_name() == k, label).clicked() {
                        let _ = app.session.execute("multicam.gridLayout", json!({"layout": k}));
                    }
                }
            });
            if ui.button(tl!("Edit Cameras…")).clicked() {
                let _ = crate::panels::multicam::route(app, "multicam.editCamerasDialog", &json!({}));
                ui.close();
            }
        }
    });
    // ---- mini timeline / scrub bar
    let bar = Rect::from_min_size(pos2(rect.min.x + 14.0, row1.max.y + 2.0), vec2(rect.width() - 28.0, 22.0));
    mini_timeline(app, ui, bar, which, origin, time, duration, rate, mark_in, mark_out);

    // ---- transport buttons
    let row3 = Rect::from_min_size(pos2(rect.min.x, bar.max.y + 4.0), vec2(rect.width(), 32.0));
    transport(app, ui, row3, which);
    level_meter(app, ui, row3, which);
}

/// The monitor's audio level meter (Kdenlive: Show Audio Levels), at the left of the transport
/// row: one bar per channel of what this monitor plays (the Program mix, or the Source clip), fed by
/// the monitor's audio tap. With two channels the peak dB is written beside the bars; with more,
/// only the ticks are shown and hovering gives every channel's dB. Right-click: the shared meter
/// appearance menu. Also applies the monitor's Volume to its tap.
fn level_meter(app: &mut FilmcraftApp, ui: &mut egui::Ui, row: Rect, which: Which) {
    let t = app.tokens;
    let src = which == Which::Source;
    let prefix = if src { "source" } else { "program" };
    let tap = if src { crate::monitor_audio::source() } else { crate::monitor_audio::program() };
    tap.set_volume(monitor_view::view(app, which).volume);
    let playing = if src { app.source_playback.clock.playing } else { app.playback.playing };
    let id = egui::Id::new((prefix, "level-meter"));
    let mut st: Vec<super::mixer::MeterCh> = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    let fresh = tap.take();
    super::mixer::meter_step(&mut st, (playing && !fresh.is_empty()).then_some(fresh.as_slice()), dt);
    if st.is_empty() {
        st = vec![super::mixer::METER_IDLE; 2];
    }
    if playing || st.iter().any(|m| m[0] > -90.0 || m[2] > 0.0) {
        ui.ctx().request_repaint();
    }
    let n = st.len();
    let bars_w = if n > 2 { 4.0 * n as f32 } else { 12.0 };
    let r = Rect::from_min_size(pos2(row.min.x + 8.0, row.min.y + 2.0), vec2(bars_w, 26.0));
    if r.max.x + 40.0 > row.center().x - 6.0 * 30.0 {
        // the transport needs the room: no meter in a narrow monitor
        ui.data_mut(|d| d.insert_temp(id, st));
        return;
    }
    let style = app.ui.meter_style;
    super::mixer::draw_meters(ui, r, &st, n, &t, style);
    if style.show_scale {
        for db in [0.0f32, -12.0, -24.0, -48.0] {
            let y = r.min.y + if style.clip_indicator { 6.0 } else { 0.0 };
            let h = r.max.y - y;
            let yy = y + h * (-db / 60.0);
            ui.painter().line_segment([pos2(r.max.x + 1.0, yy), pos2(r.max.x + 4.0, yy)], Stroke::new(1.0, t.text_faint));
        }
    }
    if n <= 2 {
        let peak = st.iter().map(|m| m[1]).fold(-90.0f32, f32::max);
        let text = if peak <= -89.9 { "-∞".to_string() } else { format!("{peak:.0}") };
        ui.painter().text(pos2(r.max.x + 7.0, r.center().y), Align2::LEFT_CENTER, text, Tokens::ui(9.5), t.text_dim);
    }
    let resp = ui.interact(r.expand2(vec2(2.0, 0.0)), id.with("hover"), Sense::click()).on_hover_text(super::mixer::meter_readout(&st, n));
    super::mixer::meter_menu(app, &resp, &format!("{prefix}.levels"));
    app.auto.add(&format!("{prefix}.levels"), r, &super::mixer::meter_readout(&st, n));
    ui.data_mut(|d| d.insert_temp(id, st));
}

/// How many of a paused monitor's latest requests are remembered ([`asked_before`]): about a
/// second of refreshes, so a frame that took several refreshes to render is still known when it
/// is ready.
const ASKED_KEYS: usize = 64;

/// Note that the monitor `prefix` asks for `key`, and return the frames it asked for on the passes
/// before, newest first: the stand-ins for `key` while it renders.
///
/// A value that is dragged asks for a new revision on every pass, and a scrub for a new frame, so
/// the frame asked for is never the one that is ready; the one asked for a pass earlier usually
/// is, and showing it keeps the picture one pass behind the mouse instead of frozen until the
/// mouse rests. Playback has its own schedule and remembers nothing (what was on screen before
/// Play is no stand-in for where it stopped).
fn asked_before(app: &mut FilmcraftApp, prefix: &str, key: FrameKey, playing: bool) -> Vec<FrameKey> {
    let asked = app.monitor_asked.entry(prefix.to_string()).or_default();
    if playing {
        asked.clear();
        return Vec::new();
    }
    // another sequence or clip: its pictures are not this one's
    asked.retain(|k| k.target == key.target && *k != key);
    let before: Vec<FrameKey> = asked.iter().rev().copied().collect();
    asked.push_back(key);
    while asked.len() > ASKED_KEYS {
        asked.pop_front();
    }
    before
}

/// A monitor texture from the CPU frame path: the exact frame, else the nearest cached one before
/// it, else the newest ready one of `asked` (see [`asked_before`]), with the display mode's
/// channel mapping. Returns the texture and the frame it shows.
fn cpu_texture(
    app: &mut FilmcraftApp,
    ctx: &egui::Context,
    name: &str,
    key: FrameKey,
    asked: &[FrameKey],
    mode: DisplayMode,
) -> (Option<egui::TextureId>, Option<FrameKey>) {
    let found = app
        .frames
        .get(&key)
        .map(|img| (key, img))
        .or_else(|| (1..=6).map(|d| FrameKey { frame: key.frame.saturating_sub(d), ..key }).find_map(|k| app.frames.get(&k).map(|img| (k, img))))
        .or_else(|| asked.iter().find_map(|k| app.frames.get(k).map(|img| (*k, img))));
    match found {
        Some((k, img)) => {
            let tex = if mode.is_channel() {
                app.texture_for_mapped(ctx, name, k, &img, |i| monitor_view::channel_view(i, mode))
            } else {
                app.texture_for(ctx, name, k, &img)
            };
            (Some(tex), Some(k))
        }
        None => (app.texture_existing(name).map(|(id, _)| id), app.texture_key(name)),
    }
}

/// A click of an armed eyedropper waiting for its frame to render.
#[derive(Clone)]
struct EyedropperPick {
    /// The clicked point in the picture, 0..1 across and down.
    uv: (f32, f32),
    key: FrameKey,
    time: Tick,
    /// The project with the armed effect switched off, so a colour that effect keys out can be picked.
    project: std::sync::Arc<redrob_film_project::Project>,
}

/// Output scale of the frame an eyedropper samples (half size is plenty and quick to render).
const EYEDROPPER_SCALE: f32 = 0.5;

/// Eyedropper: while a colour parameter's eyedropper is armed (Effect Controls), the Program picture
/// takes a crosshair and a click fills that parameter with the colour under the pointer. The colour is
/// read from a render of the sequence at the playhead with the armed effect switched off (an Ultra Key
/// has already removed the green it is asked to key); Esc disarms.
fn eyedropper(app: &mut FilmcraftApp, ui: &mut egui::Ui, resp: &egui::Response, pic: Rect, target: Target, rate: redrob_film_time::FrameRate, time: Tick) {
    let Some(armed) = app.ui.eyedropper.clone() else { return };
    let ctx = ui.ctx().clone();
    let pending_id = egui::Id::new("eyedropper-pending");
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.eyedropper = None;
        ctx.data_mut(|d| d.remove::<EyedropperPick>(pending_id));
        return;
    }
    if resp.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if resp.clicked()
        && let (Some(pos), Target::Sequence(seq)) = (resp.interact_pointer_pos(), target)
        && pic.width() > 0.0
        && pic.height() > 0.0
    {
        let uv = (((pos.x - pic.min.x) / pic.width()).clamp(0.0, 1.0), ((pos.y - pic.min.y) / pic.height()).clamp(0.0, 1.0));
        let mut project = (*app.session.project).clone();
        if let Some(e) = project
            .sequence_mut(seq)
            .and_then(|q| q.find_item_mut(redrob_film_project::ClipId(armed.clip)))
            .and_then(|(_, it)| it.effects.get_mut(armed.effect))
        {
            e.enabled = false;
        }
        let frame = rate.frame_at(time);
        // a revision of its own, so this render never replaces what the monitor caches
        let revision = app.session.revision ^ 0x5eed_d70b_0000_0000 ^ armed.clip.rotate_left(17) ^ armed.effect as u64;
        let key = FrameKey { target, frame, size: (EYEDROPPER_SCALE * 1000.0) as u32, revision, draft: false };
        ctx.data_mut(|d| d.insert_temp(pending_id, EyedropperPick { uv, key, time: rate.tick_of(frame), project: std::sync::Arc::new(project) }));
    }
    let Some(pick) = ctx.data(|d| d.get_temp::<EyedropperPick>(pending_id)) else { return };
    app.frames.request(pick.key, pick.time, EYEDROPPER_SCALE, &pick.project, 0);
    let Some(img) = app.frames.get(&pick.key) else {
        ctx.request_repaint();
        return;
    };
    ctx.data_mut(|d| d.remove::<EyedropperPick>(pending_id));
    app.ui.eyedropper = None;
    let Some(rgb) = sample_rgb(&img, pick.uv) else {
        app.ui.status = tl!("Eyedropper: no picture to sample").to_string();
        return;
    };
    // the parameter stores linear values, like the colour picker beside it
    let lin = egui::Rgba::from(Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
    let mut params = json!({"clip": armed.clip, "effect": armed.effect, "param": armed.param, "value": [lin.r(), lin.g(), lin.b(), 1.0]});
    if let Some(k) = armed.mask {
        params["mask"] = json!(k);
    }
    if let Err(e) = app.session.execute("effects.setParam", params) {
        app.ui.status = e.to_string();
    }
}

/// The average colour of the 5×5 pixels around `uv` (0..1 across and down) of a frame, so a noisy
/// green screen gives its true green. `None` for an empty frame.
fn sample_rgb(img: &crate::frames::Rgba, uv: (f32, f32)) -> Option<[u8; 3]> {
    if img.w == 0 || img.h == 0 || img.px.len() < img.w.saturating_mul(img.h).saturating_mul(4) {
        return None;
    }
    let cx = ((uv.0 * img.w as f32) as usize).min(img.w - 1);
    let cy = ((uv.1 * img.h as f32) as usize).min(img.h - 1);
    let (mut sum, mut n) = ([0u32; 3], 0u32);
    for y in cy.saturating_sub(2)..=(cy + 2).min(img.h - 1) {
        for x in cx.saturating_sub(2)..=(cx + 2).min(img.w - 1) {
            let i = (y * img.w + x) * 4;
            for (c, s) in sum.iter_mut().enumerate() {
                *s += u32::from(*img.px.get(i + c)?);
            }
            n += 1;
        }
    }
    let avg = |c: usize| u8::try_from(sum[c].checked_div(n)?).ok();
    Some([avg(0)?, avg(1)?, avg(2)?])
}

pub fn quantize_scale(s: f32) -> f32 {
    // Buckets keep the cache effective while resizing the panel.
    let buckets = [1.0 / 32.0, 1.0 / 16.0, 1.0 / 8.0, 0.1875, 0.25, 0.375, 0.5, 0.75, 1.0];
    *buckets.iter().find(|b| **b >= s - 1e-4).unwrap_or(&1.0)
}

pub fn fit(area: Rect, w: f32, h: f32) -> Rect {
    let s = (area.width() / w).min(area.height() / h);
    Rect::from_center_size(area.center(), vec2(w * s, h * s))
}

#[allow(clippy::too_many_arguments)]
fn mini_timeline(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    bar: Rect,
    which: Which,
    origin: Tick,
    time: Tick,
    end: Tick,
    rate: redrob_film_time::FrameRate,
    mark_in: Option<Tick>,
    mark_out: Option<Tick>,
) {
    let t = app.tokens;
    let duration = end;
    // Kdenlive's monitor ruler zoom: Ctrl / Cmd + the wheel over the ruler zooms it about the
    // pointer; the zoom bar under a zoomed ruler is dragged to scroll, its ends to zoom, and a
    // double-click on it shows the whole clip again
    let zid = egui::Id::new((which as u8, "ruler-zoom"));
    let (mut za, mut zb): (f64, f64) = ui.data(|d| d.get_temp(zid)).unwrap_or((0.0, 1.0));
    if ui.rect_contains_pointer(bar)
        && let Some(hp) = ui.ctx().pointer_hover_pos()
    {
        let wheel = crate::panels::timeline::wheel_input(ui.ctx());
        if wheel.command && wheel.delta.y != 0.0 {
            let span = zb - za;
            let f = f64::from((1.0 - wheel.delta.y * 0.01).clamp(0.5, 2.0));
            let at = za + span * f64::from(((hp.x - bar.min.x) / bar.width().max(1.0)).clamp(0.0, 1.0));
            let ns = (span * f).clamp(0.001, 1.0);
            za = (at - (at - za) * ns / span).clamp(0.0, 1.0 - ns);
            zb = za + ns;
        }
    }
    let zoomed = zb - za < 0.999;
    let zbar = Rect::from_min_max(pos2(bar.min.x, bar.max.y), pos2(bar.max.x, bar.max.y + 4.0));
    if zoomed {
        let win = Rect::from_min_max(pos2(zbar.min.x + zbar.width() * za as f32, zbar.min.y), pos2(zbar.min.x + zbar.width() * zb as f32, zbar.max.y));
        ui.painter().rect_filled(zbar, 1.0, t.tl_ruler_bg);
        ui.painter().rect_filled(win, 1.0, t.text_dim);
        let zr = ui.interact(zbar.expand2(vec2(0.0, 1.0)), egui::Id::new((which as u8, "ruler-zoom-bar")), Sense::click_and_drag());
        app.auto.add(&format!("{}.rulerZoom", if which == Which::Program { "program" } else { "source" }), zbar, "ruler zoom bar");
        let grab_id = egui::Id::new((which as u8, "ruler-zoom-grab"));
        if zr.drag_started()
            && let Some(o) = ui.input(|i| i.pointer.press_origin())
        {
            let g: u8 = if (o.x - win.min.x).abs() <= 4.0 {
                0
            } else if (o.x - win.max.x).abs() <= 4.0 {
                1
            } else {
                2
            };
            ui.data_mut(|d| d.insert_temp(grab_id, g));
        }
        if zr.dragged() {
            let dx = f64::from(zr.drag_delta().x / zbar.width().max(1.0));
            match ui.data(|d| d.get_temp::<u8>(grab_id)).unwrap_or(2) {
                0 => za = (za + dx).clamp(0.0, zb - 0.001),
                1 => zb = (zb + dx).clamp(za + 0.001, 1.0),
                _ => {
                    let sp = zb - za;
                    za = (za + dx).clamp(0.0, 1.0 - sp);
                    zb = za + sp;
                }
            }
        }
        if zr.double_clicked() {
            (za, zb) = (0.0, 1.0);
        }
    }
    ui.data_mut(|d| d.insert_temp(zid, (za, zb)));
    let full = (end - origin).0.max(1) as f64;
    let vis0 = origin.0 as f64 + za * full;
    let dur = ((zb - za) * full).max(1.0);
    let xof = |tk: Tick| bar.min.x + (((tk.0 as f64 - vis0) / dur) as f32).clamp(0.0, 1.0) * bar.width();
    let resp = ui.interact(bar, egui::Id::new((which as u8, "scrub")), Sense::click_and_drag());
    app.auto.add(if which == Which::Program { "program.scrubBar" } else { "source.scrubBar" }, bar, "scrub bar");
    // dragging the marked range works on the whole ruler (a zoomed one scrubs only)
    let interaction = if zoomed { crate::panels::source_range::Interaction::default() } else { crate::panels::source_range::interact(app, ui, bar, which) };
    let p = ui.painter();
    // ticks: minor 4 pt, major 10 pt, ~20 pt spacing
    let n = ((bar.width() / 20.0) as i32).max(2);
    for i in 0..=n {
        let x = bar.min.x + bar.width() * i as f32 / n as f32;
        let h = if i % 5 == 0 { 8.0 } else { 3.5 };
        p.line_segment([pos2(x, bar.max.y - h), pos2(x, bar.max.y)], Stroke::new(1.0, t.text_faint));
    }
    let a = xof(interaction.range.map(|r| r.start).unwrap_or(mark_in.unwrap_or(origin)));
    // Out includes its frame; both the shading and its bracket end at the following boundary.
    let b = xof(interaction.range.map(|r| r.end()).unwrap_or_else(|| mark_out.map(|o| Tick(o.0.saturating_add(rate.frame_duration().0))).unwrap_or(duration)))
        .max(a);
    if mark_in.is_some() || mark_out.is_some() {
        p.rect_filled(Rect::from_min_max(pos2(a, bar.min.y + 8.0), pos2(b, bar.max.y)), 0.0, Color32::from_rgba_unmultiplied(0x5c, 0x5c, 0x5c, 128));
    }
    if mark_in.is_some() || mark_out.is_some() {
        // Curly braces sit inside the gray band, with their waist at its exact range edge.
        let top = bar.min.y + 8.0;
        let bottom = bar.max.y;
        let mid = (top + bottom) * 0.5;
        for (x, direction, hot) in [(a, 1.0_f32, interaction.in_hot), (b, -1.0_f32, interaction.out_hot)] {
            if hot || (direction > 0.0 && mark_in.is_none()) || (direction < 0.0 && mark_out.is_none()) {
                continue;
            }
            let point = |offset: f32, y: f32| pos2(x + direction * offset, y);
            p.line(
                vec![
                    point(4.0, top),
                    point(2.8, top),
                    point(2.0, top + 1.0),
                    point(2.0, mid - 2.0),
                    point(1.4, mid - 1.0),
                    point(0.0, mid),
                    point(1.4, mid + 1.0),
                    point(2.0, mid + 2.0),
                    point(2.0, bottom - 1.0),
                    point(2.8, bottom),
                    point(4.0, bottom),
                ],
                Stroke::new(1.0, t.playhead),
            );
        }
    }
    // markers: the sequence's (Program), the clip's or the subclip's inherited ones (Source)
    let markers = match which {
        Which::Program => app.session.active_sequence().map(|q| q.markers.clone()),
        Which::Source => app.session.state.source_item.and_then(|i| redrob_film_engine::clip_ops::source_view(&app.session, i)).map(|v| v.markers),
    };
    if let Some(markers) = markers {
        for m in &markers {
            let x = xof(m.start);
            let c = m.color.marker_rgb();
            let c = Color32::from_rgb(c[0], c[1], c[2]);
            let y = bar.min.y;
            p.add(egui::Shape::convex_polygon(
                vec![pos2(x - 3.5, y), pos2(x + 3.5, y), pos2(x + 3.5, y + 7.0), pos2(x, y + 10.0), pos2(x - 3.5, y + 7.0)],
                c,
                Stroke::NONE,
            ));
        }
    }
    let x = xof(time);
    // playhead: blue triangle over a line
    let hy = bar.max.y - 11.0;
    p.add(egui::Shape::convex_polygon(
        vec![pos2(x - 5.5, hy), pos2(x + 5.5, hy), pos2(x + 5.5, hy + 5.0), pos2(x, hy + 9.0), pos2(x - 5.5, hy + 5.0)],
        t.playhead,
        Stroke::NONE,
    ));
    p.line_segment([pos2(x, hy + 8.0), pos2(x, bar.max.y)], Stroke::new(1.0, t.playhead));
    if interaction.in_hot {
        crate::panels::source_range::trim_cue(p, a, bar, 4.0);
    }
    if interaction.out_hot {
        crate::panels::source_range::trim_cue(p, b, bar, -4.0);
    }
    let scrub = interaction.scrub.or_else(|| {
        if !interaction.handled && (resp.dragged() || resp.clicked()) {
            resp.interact_pointer_pos().map(|pos| {
                let f = ((pos.x - bar.min.x) / bar.width().max(1.0)).clamp(0.0, 1.0) as f64;
                rate.snap(Tick((vis0 + f * dur) as i64).max(origin))
            })
        } else {
            None
        }
    });
    if let Some(tk) = scrub {
        match which {
            Which::Program => {
                // a drag over the bar while playback carries on (Pause playback when seeking off)
                // does not restart it every frame
                if !app.playback.playing || resp.clicked() || resp.drag_started() {
                    app.seek_program(tk);
                }
            }
            Which::Source => {
                let _ = app.session.execute("source.setPlayhead", json!({"time": tk.0}));
            }
        }
    }
}

// A hidden transport action is still available through its regular shortcut/menu.
fn transport_visible(hidden: &[String], command: &str) -> bool {
    !hidden.iter().any(|id| id == command)
}

fn set_transport_visible(hidden: &mut Vec<String>, command: &str, visible: bool) {
    if visible {
        hidden.retain(|id| id != command);
    } else if transport_visible(hidden, command) {
        hidden.push(command.to_owned());
    }
}

fn transport(app: &mut FilmcraftApp, ui: &mut egui::Ui, row: Rect, which: Which) {
    let t = app.tokens;
    let src = which == Which::Source;
    let buttons: Vec<(Icon, &str, &str)> = if src {
        vec![
            (Icon::Marker, "markers.add", tl!("Add Marker (M)")),
            (Icon::MarkIn, "src.markIn", tl!("Mark In (I)")),
            (Icon::MarkOut, "src.markOut", tl!("Mark Out (O)")),
            (Icon::GoToIn, "src.goIn", tl!("Go to In (Shift+I)")),
            (Icon::StepBack, "src.stepBack", tl!("Step Back 1 Frame (Left)")),
            (Icon::Rewind, "playback.reverse", tl!("Play Backwards (J)")),
            (if app.source_playback.clock.playing { Icon::Pause } else { Icon::Play }, "source.playback.toggle", tl!("Play-Stop Toggle (Space)")),
            (Icon::FastForward, "playback.forward", tl!("Fast Forward (L)")),
            (Icon::StepFwd, "src.stepFwd", tl!("Step Forward 1 Frame (Right)")),
            (Icon::GoToOut, "src.goOut", tl!("Go to Out (Shift+O)")),
            (Icon::Insert, "source.insert", tl!("Insert (,)")),
            (Icon::Overwrite, "source.overwrite", tl!("Overwrite (.)")),
            (Icon::Camera, "exportFrame", tl!("Export Frame")),
            (Icon::Proxy, "media.toggleProxies", tl!("Toggle Proxies")),
        ]
    } else {
        vec![
            (Icon::Marker, "markers.add", tl!("Add Marker (M)")),
            (Icon::MarkIn, "markers.markIn", tl!("Mark In (I)")),
            (Icon::MarkOut, "markers.markOut", tl!("Mark Out (O)")),
            (Icon::GoToIn, "markers.goToIn", tl!("Go to In (Shift+I)")),
            (Icon::StepBack, "playhead.stepBack", tl!("Step Back 1 Frame (Left)")),
            (Icon::Rewind, "playback.reverse", tl!("Play Backwards (J)")),
            (if app.playback.playing { Icon::Pause } else { Icon::Play }, "playback.toggle", tl!("Play-Stop Toggle (Space)")),
            (Icon::FastForward, "playback.forward", tl!("Fast Forward (L)")),
            (Icon::StepFwd, "playhead.stepForward", tl!("Step Forward 1 Frame (Right)")),
            (Icon::GoToOut, "markers.goToOut", tl!("Go to Out (Shift+O)")),
            (Icon::Lift, "sequence.lift", tl!("Lift (;)")),
            (Icon::Extract, "sequence.extract", tl!("Extract (')")),
            (Icon::Camera, "exportFrame", tl!("Export Frame")),
            (Icon::Proxy, "media.toggleProxies", tl!("Toggle Proxies")),
        ]
    };
    let mut buttons = buttons;
    if !src {
        // Optional control, discoverable through the editor without changing existing layouts.
        buttons.push((Icon::Loop, "playback.loop", tl!("Loop")));
    }
    let hidden = monitor_view::view(app, which).transport_hidden.clone();
    let bw = 30.0;
    let total = buttons.iter().filter(|(_, cmd, _)| transport_visible(&hidden, cmd)).count() as f32 * bw;
    let mut x = row.center().x - total / 2.0;
    let ctx = ui.ctx().clone();
    let prefix = if src { "source" } else { "program" };
    for &(icon, cmd, tip) in &buttons {
        if !transport_visible(&hidden, cmd) {
            continue;
        }
        let r = Rect::from_min_size(pos2(x, row.min.y + 2.0), vec2(bw - 2.0, 26.0));
        let resp = ui.interact(r, egui::Id::new((prefix, cmd)), Sense::click()).on_hover_text(tip);
        app.auto.add(&format!("{prefix}.transport.{cmd}"), r, tip);
        let is_play = matches!(cmd, "playback.toggle" | "source.playback.toggle");
        if resp.hovered() {
            ui.painter().rect_filled(r, 4.0, t.hover);
        }
        let sz = if is_play { 16.0 } else { 14.0 };
        let on = cmd == "media.toggleProxies" && app.session.media.use_proxies();
        let col = if on {
            t.accent
        } else if resp.hovered() {
            t.tab_text_active
        } else {
            t.icon
        };
        icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(sz, sz)), icon, col);
        if is_play {
            // Kdenlive: the play button's drop-down of play options (right-click)
            play_options_menu(app, &ctx, &resp, which);
        }
        if resp.clicked() {
            let r = match cmd {
                "src.markIn" => app.session.execute("markers.markIn", json!({"target": "source"})).map_err(|e| e.to_string()),
                "src.markOut" => app.session.execute("markers.markOut", json!({"target": "source"})).map_err(|e| e.to_string()),
                "src.goIn" | "src.goOut" | "src.stepBack" | "src.stepFwd" => {
                    source_nav(app, cmd);
                    Ok(serde_json::Value::Null)
                }

                "exportFrame" => crate::menus::invoke(app, &ctx, "file.exportFrame", json!({"monitor": if src { "source" } else { "program" }})),
                c => crate::menus::invoke(app, &ctx, c, json!({"monitor": if src { "source" } else { "program" }})),
            };
            if let Err(e) = r {
                app.ui.status = e;
            }
        }
        x += bw;
    }
    // Customize this monitor's transport separately from the other monitor.
    let r = Rect::from_min_size(pos2(row.max.x - 30.0, row.min.y + 4.0), vec2(22.0, 22.0));
    let resp = ui.interact(r, egui::Id::new((prefix, "btn-editor")), Sense::click()).on_hover_text(tl!("Button Editor"));
    app.auto.add(&format!("{prefix}.transport.editor"), r, "Button Editor");
    icons::paint(ui.painter(), r.shrink(5.0), Icon::Plus, if resp.hovered() { t.tab_text_active } else { t.text_dim });
    let mut edited_hidden = hidden.clone();
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(225.0);
        for &(_, cmd, tip) in &buttons {
            let mut visible = transport_visible(&edited_hidden, cmd);
            let checkbox = ui.checkbox(&mut visible, tip);
            app.auto.add(&format!("{prefix}.transport.editor.{cmd}"), checkbox.rect, tip);
            if checkbox.changed() {
                set_transport_visible(&mut edited_hidden, cmd, visible);
            }
        }
        ui.separator();
        let reset = ui.button(tl!("Reset"));
        app.auto.add(&format!("{prefix}.transport.editor.reset"), reset.rect, "Reset");
        if reset.clicked() {
            edited_hidden = crate::state::MonitorView::default().transport_hidden;
        }
    });
    if edited_hidden != hidden {
        monitor_view::view_mut(app, which).transport_hidden = edited_hidden;
    }
}

/// The play button's options (right-click): Play Zone, Loop Zone and, in the Program monitor,
/// Loop Selected Clip; each runs the same command as its shortcut.
fn play_options_menu(app: &mut FilmcraftApp, ctx: &egui::Context, resp: &egui::Response, which: Which) {
    let src = which == Which::Source;
    let prefix = if src { "source" } else { "program" };
    let mut picked: Option<&str> = None;
    egui::Popup::context_menu(resp).show(|ui| {
        let mut items = vec![("playback.inToOut", tl!("Play Zone")), ("playback.loopZone", tl!("Loop Zone"))];
        if !src {
            items.push(("playback.loopClip", tl!("Loop Selected Clip")));
        }
        for (cmd, label) in items {
            let r = ui.button(label);
            app.auto.add(&format!("{prefix}.transport.playOptions.{cmd}"), r.rect, label);
            if r.clicked() {
                picked = Some(cmd);
                ui.close();
            }
        }
    });
    if let Some(cmd) = picked
        && let Err(e) = crate::menus::invoke(app, ctx, cmd, json!({"monitor": prefix}))
    {
        app.ui.status = e;
    }
}

/// The Source panel menu's clips (#313): the recently opened clips, the shown one ticked, to
/// switch between, then Close / Close All. Returns whether the menu is done.
pub fn source_panel_menu(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> bool {
    let mut done = false;
    let current = app.session.state.source_item;
    let recent: Vec<(u64, String)> =
        app.session.state.source_history.iter().filter_map(|i| app.session.project.item(*i).map(|it| (i.0, it.name.clone()))).collect();
    for (id, name) in &recent {
        let shown = current.is_some_and(|c| c.0 == *id);
        let r = ui.button(if shown { format!("✓ {name}") } else { name.clone() });
        app.auto.add(&format!("panel.menu.Source.recent.{id}"), r.rect, name);
        if r.clicked() {
            if !shown && let Err(e) = app.session.execute("source.open", json!({"item": id})) {
                app.ui.status = e.to_string();
            }
            done = true;
        }
    }
    if !recent.is_empty() {
        ui.separator();
    }
    for (cmd, id, label) in [("source.close", "close", tl!("Close")), ("source.closeAll", "closeAll", tl!("Close All"))] {
        let r = ui.add_enabled(current.is_some(), egui::Button::new(label));
        app.auto.add(&format!("panel.menu.Source.{id}"), r.rect, label);
        if r.clicked() {
            if let Err(e) = app.session.execute(cmd, json!({})) {
                app.ui.status = e.to_string();
            }
            done = true;
        }
    }
    done
}

fn source_nav(app: &mut FilmcraftApp, cmd: &str) {
    let Some(item) = app.session.state.source_item else { return };
    let Some(v) = redrob_film_engine::clip_ops::source_view(&app.session, item) else { return };
    let rate = v.rate;
    let cur = app.session.state.source_playhead;
    let t = match cmd {
        "src.goIn" => v.mark_in.unwrap_or(v.start),
        "src.goOut" => v.mark_out.unwrap_or(v.end - rate.frame_duration()),
        _ if rate.frame_duration() == Tick::ZERO => cur,
        "src.stepBack" => rate.tick_of(rate.frame_at(cur).saturating_sub(1)),
        _ => rate.tick_of(rate.frame_at(cur).saturating_add(1)),
    };
    let _ = app.session.execute("source.setPlayhead", json!({"time": t.0}));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::Rgba;

    fn flat(w: usize, h: usize, rgba: [u8; 4]) -> Rgba {
        Rgba { w, h, px: rgba.iter().copied().cycle().take(w * h * 4).collect() }
    }

    #[test]
    fn button_editor_updates_visibility_without_affecting_other_monitor() {
        let mut program = crate::state::MonitorView::default().transport_hidden;
        let source = program.clone();
        assert!(!transport_visible(&program, "playback.loop"));
        set_transport_visible(&mut program, "playback.loop", true);
        set_transport_visible(&mut program, "playback.loop", true);
        assert!(transport_visible(&program, "playback.loop"));
        assert!(program.is_empty(), "enabling twice must not leave hidden entries");
        set_transport_visible(&mut program, "playhead.stepForward", false);
        set_transport_visible(&mut program, "playhead.stepForward", false);
        assert_eq!(program, vec!["playhead.stepForward"]);
        assert!(transport_visible(&source, "playhead.stepForward"));
    }

    #[test]
    fn eyedropper_averages_the_pixels_around_the_point() {
        let mut img = flat(8, 8, [0, 200, 0, 255]);
        // one noisy pixel next to the centre is averaged away, not picked
        img.px[(4 * 8 + 5) * 4 + 1] = 0;
        assert_eq!(sample_rgb(&img, (0.5, 0.5)), Some([0, 192, 0]));
    }

    #[test]
    fn eyedropper_clamps_to_the_picture_edges() {
        let img = flat(4, 3, [10, 20, 30, 255]);
        for uv in [(0.0, 0.0), (1.0, 1.0), (0.999, 0.0), (0.0, 0.999)] {
            assert_eq!(sample_rgb(&img, uv), Some([10, 20, 30]));
        }
    }

    #[test]
    fn eyedropper_rejects_empty_and_truncated_frames() {
        assert_eq!(sample_rgb(&Rgba { w: 0, h: 0, px: vec![] }, (0.5, 0.5)), None);
        assert_eq!(sample_rgb(&Rgba { w: 4, h: 4, px: vec![0; 10] }, (0.5, 0.5)), None);
    }
}

/// Compare Effect: the project with the selected clips' standard effects switched off, and the
/// revision salt that keeps its frames apart from the monitor's own. Cached per revision and
/// selection (the project is only cloned when either changes). None without a selected clip that
/// has effects.
fn without_effects(app: &FilmcraftApp, ctx: &egui::Context) -> Option<(std::sync::Arc<redrob_film_project::Project>, u64)> {
    let seq_id = app.session.state.active_sequence?;
    let sel: Vec<u64> = app.session.state.selection.iter().map(|c| c.0).collect();
    let rev = app.session.revision;
    let id = egui::Id::new("compare-effects-project");
    type Cached = (u64, Vec<u64>, Option<(std::sync::Arc<redrob_film_project::Project>, u64)>);
    if let Some((r, s, p)) = ctx.data(|d| d.get_temp::<Cached>(id))
        && r == rev
        && s == sel
    {
        return p;
    }
    let mut project = (*app.session.project).clone();
    let mut changed = false;
    if let Some(q) = project.sequence_mut(seq_id) {
        for c in &sel {
            if let Some((_, it)) = q.find_item_mut(redrob_film_project::ClipId(*c)) {
                for e in it.effects.iter_mut().filter(|e| e.def().is_some_and(|d| !d.intrinsic) && !redrob_film_project::graphic::is_layer(e)) {
                    changed |= e.enabled;
                    e.enabled = false;
                }
            }
        }
    }
    let salt = sel.iter().fold(0x0c0f_fe75_0000_0000u64, |a, c| a.rotate_left(13) ^ c);
    let out = changed.then(|| (std::sync::Arc::new(project), salt));
    ctx.data_mut(|d| d.insert_temp::<Cached>(id, (rev, sel, out.clone())));
    out
}

// ---------------------------------------------------------------- monitor windows

/// A monitor shown outside the dock: full screen (Kdenlive's Switch Monitor Fullscreen, a
/// double-click on the picture) or in its own window (an undocked monitor).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MonitorWindow {
    pub fullscreen: bool,
    pub undocked: bool,
}

fn window_id(which: Which) -> egui::Id {
    egui::Id::new(("monitor-window", which as u8))
}

pub fn window_state(ctx: &egui::Context, which: Which) -> MonitorWindow {
    ctx.data(|d| d.get_temp(window_id(which))).unwrap_or_default()
}

pub fn set_window_state(ctx: &egui::Context, which: Which, w: MonitorWindow) {
    ctx.data_mut(|d| d.insert_temp(window_id(which), w));
    ctx.request_repaint();
}

/// Switch a monitor full screen (`full`) or into its own window, or back (`enabled` forces the
/// state). Returns whether it is now on.
pub fn toggle_window(ctx: &egui::Context, which: Which, full: bool, enabled: Option<bool>) -> bool {
    let mut w = window_state(ctx, which);
    let on = enabled.unwrap_or(if full { !w.fullscreen } else { !w.undocked });
    if full {
        w.fullscreen = on;
    } else {
        w.undocked = on;
        if !on {
            w.fullscreen = false;
        }
    }
    set_window_state(ctx, which, w);
    on
}

/// Where a full-screen monitor window opens: on the main window's screen, or (Settings ▸ Playback
/// ▸ Screen used for full screen monitor) the one beside it, by placing the new window there before
/// it goes full screen. Screens are taken to be the size of the main window's.
fn fullscreen_position(app: &FilmcraftApp, ctx: &egui::Context) -> Option<egui::Pos2> {
    let side = app.session.prefs.playback.fullscreen_screen.as_str();
    let (outer, monitor) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().monitor_size));
    let outer = outer?;
    let m = monitor?;
    if m.x <= 0.0 || m.y <= 0.0 {
        return None;
    }
    // the origin of the main window's screen, in a row / column of same-sized screens
    let origin = pos2((outer.center().x / m.x).floor() * m.x, (outer.center().y / m.y).floor() * m.y);
    let o = match side {
        "left" => vec2(-m.x, 0.0),
        "right" => vec2(m.x, 0.0),
        "above" => vec2(0.0, -m.y),
        "below" => vec2(0.0, m.y),
        _ => vec2(0.0, 0.0),
    };
    Some(origin + o + vec2(40.0, 40.0))
}

/// Draw the monitors that are full screen or in their own windows (native windows; egui embeds
/// them in the main window where the platform cannot open another). Esc, F11, a double-click or
/// closing the window brings a monitor back to the dock; over a full-screen picture the pointer
/// hides after 2 s without moving, and Space, J, K, L and the arrow keys play and step.
pub fn external_windows(app: &mut FilmcraftApp, ctx: &egui::Context) {
    for which in [Which::Program, Which::Source] {
        let w = window_state(ctx, which);
        if !w.fullscreen && !w.undocked {
            continue;
        }
        let title = if which == Which::Program { tl!("Program Monitor") } else { tl!("Source Monitor") };
        let vid = egui::ViewportId::from_hash_of(("monitor-viewport", which as u8, w.fullscreen));
        let mut builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([960.0, 600.0]).with_min_inner_size([320.0, 240.0]);
        if w.fullscreen {
            if let Some(p) = fullscreen_position(app, ctx) {
                builder = builder.with_position(p);
            }
            builder = builder.with_fullscreen(true);
        }
        let mut close = false;
        ctx.show_viewport_immediate(vid, builder, |ui, _class| {
            let r = ui.available_rect_before_wrap();
            let r = Rect::from_min_size(r.min, vec2(r.width().max(320.0), r.height().max(240.0)));
            ui.allocate_rect(r, Sense::hover());
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt(("monitor-viewport-body", which as u8)));
            child.set_clip_rect(r);
            show_in(app, &mut child, r, which, w.fullscreen);
            let vctx = ui.ctx().clone();
            if vctx.input(|i| i.viewport().close_requested()) {
                close = true;
            }
            if w.fullscreen {
                if vctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::F11)) {
                    close = true;
                }
                hide_idle_cursor(&vctx, which);
            }
            let prefix = if which == Which::Program { "program" } else { "source" };
            let keys = vctx.input(|i| {
                [
                    (egui::Key::Space, if which == Which::Program { "playback.toggle" } else { "source.playback.toggle" }),
                    (egui::Key::J, "playback.reverse"),
                    (egui::Key::K, "playback.stop"),
                    (egui::Key::L, "playback.forward"),
                    (egui::Key::ArrowLeft, "playhead.stepBack"),
                    (egui::Key::ArrowRight, "playhead.stepForward"),
                ]
                .into_iter()
                .filter(|(k, _)| i.key_pressed(*k) && i.modifiers.is_none())
                .map(|(_, c)| c)
                .collect::<Vec<_>>()
            });
            for cmd in keys {
                if cmd.starts_with("playhead.step") {
                    monitor_view::step_monitor(app, which, if cmd.ends_with("Back") { -1 } else { 1 });
                } else if let Err(e) = crate::menus::invoke(app, &vctx, cmd, json!({"monitor": prefix})) {
                    app.ui.status = e;
                }
            }
        });
        if close {
            set_window_state(ctx, which, MonitorWindow::default());
        }
    }
}

/// Hide the pointer over a full-screen monitor once it has rested 2 s.
fn hide_idle_cursor(ctx: &egui::Context, which: Which) {
    let id = egui::Id::new(("monitor-viewport-idle", which as u8));
    let now = ctx.input(|i| i.time);
    let moved = ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down());
    let last = if moved { now } else { ctx.data(|d| d.get_temp::<f64>(id)).unwrap_or(now) };
    ctx.data_mut(|d| d.insert_temp(id, last));
    if now - last >= 2.0 {
        ctx.set_cursor_icon(egui::CursorIcon::None);
    } else {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((2.0 - (now - last)).max(0.05)));
    }
}

// ---------------------------------------------------------------- overlays

fn hex_color(s: &str) -> Option<Color32> {
    redrob_film_engine::settings::parse_hex(s).map(|[r, g, b]| Color32::from_rgb(r, g, b))
}

/// Settings ▸ Playback ▸ Custom monitor background, when it is on.
fn monitor_background(app: &FilmcraftApp) -> Option<Color32> {
    let p = &app.session.prefs.playback;
    if p.custom_monitor_background { hex_color(&p.monitor_background) } else { None }
}

/// Settings ▸ Playback ▸ Monitor overlay color.
pub fn overlay_color(app: &FilmcraftApp) -> Color32 {
    hex_color(&app.session.prefs.playback.overlay_color).unwrap_or(Color32::from_rgb(0x3d, 0xa5, 0xff))
}

/// Playback FPS: the times (egui seconds) at which a monitor showed a new frame in the last second.
#[derive(Clone, Default)]
struct ShownFrames {
    last: Option<i64>,
    times: std::collections::VecDeque<f64>,
}

fn note_shown_frame(ctx: &egui::Context, prefix: &str, frame: Option<i64>, playing: bool) {
    let id = egui::Id::new((prefix, "shown-frames"));
    let now = ctx.input(|i| i.time);
    let mut s: ShownFrames = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    if !playing {
        s = ShownFrames::default();
    } else if frame.is_some() && frame != s.last {
        s.last = frame;
        s.times.push_back(now);
    }
    while s.times.front().is_some_and(|t| now - *t > 1.0) {
        s.times.pop_front();
    }
    ctx.data_mut(|d| d.insert_temp(id, s));
}

/// Frames a monitor showed in the last second (None when it is not playing).
fn playback_fps(ctx: &egui::Context, prefix: &str) -> Option<usize> {
    let s: ShownFrames = ctx.data(|d| d.get_temp(egui::Id::new((prefix, "shown-frames"))))?;
    s.last.map(|_| s.times.len())
}

/// A guide pattern ([`crate::state::OVERLAY_PATTERNS`]) over the picture.
fn draw_pattern(p: &egui::Painter, pic: Rect, n: u8, c: Color32) {
    let st = Stroke::new(1.0, c);
    let x = |f: f32| pic.min.x + pic.width() * f;
    let y = |f: f32| pic.min.y + pic.height() * f;
    let v = |p: &egui::Painter, f: f32| p.line_segment([pos2(x(f), pic.min.y), pos2(x(f), pic.max.y)], st);
    let h = |p: &egui::Painter, f: f32| p.line_segment([pos2(pic.min.x, y(f)), pos2(pic.max.x, y(f))], st);
    match n {
        1 => {
            for f in [1.0 / 3.0, 2.0 / 3.0] {
                v(p, f);
                h(p, f);
            }
        }
        2 => {
            let c0 = pic.center();
            let l = pic.height().min(pic.width()) * 0.06;
            p.line_segment([c0 - vec2(l, 0.0), c0 + vec2(l, 0.0)], st);
            p.line_segment([c0 - vec2(0.0, l), c0 + vec2(0.0, l)], st);
            v(p, 0.5);
            h(p, 0.5);
        }
        3 => {
            p.line_segment([pic.left_top(), pic.right_bottom()], st);
            p.line_segment([pic.right_top(), pic.left_bottom()], st);
        }
        4 => {
            for f in [0.25, 0.5, 0.75] {
                v(p, f);
                h(p, f);
            }
        }
        5 => {
            let g = 2.0 / (1.0 + 5f32.sqrt()); // 1 / golden ratio
            for f in [1.0 - g, g] {
                v(p, f);
                h(p, f);
            }
        }
        _ => {}
    }
}

/// The marker a monitor's playhead is on (its start frame, or inside its duration).
fn marker_at(app: &FilmcraftApp, which: Which, time: Tick, rate: redrob_film_time::FrameRate) -> Option<redrob_film_project::Marker> {
    let markers = match which {
        Which::Program => app.session.active_sequence().map(|q| q.markers.clone())?,
        Which::Source => app.session.state.source_item.and_then(|i| redrob_film_engine::clip_ops::source_view(&app.session, i)).map(|v| v.markers)?,
    };
    let fd = rate.frame_duration();
    markers.into_iter().find(|m| time >= m.start && time < m.start + m.duration.max(fd))
}

/// The information drawn over the picture (Kdenlive's Monitor Info Overlay and Change Overlay):
/// the guide pattern, the timecode, the playback FPS, the marker under the playhead, the audio
/// waveform strip and, on the Source Monitor, the clip's running jobs with a button to cancel each.
#[allow(clippy::too_many_arguments)]
fn info_overlays(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    which: Which,
    pic: Rect,
    rate: redrob_film_time::FrameRate,
    time: Tick,
    pofs: Tick,
    drop_frame: bool,
    duration: Tick,
    mv: &crate::state::MonitorView,
) {
    if pic.width() < 40.0 || pic.height() < 30.0 {
        return;
    }
    let col = overlay_color(app);
    let p = ui.painter().with_clip_rect(pic);
    let prefix = if which == Which::Program { "program" } else { "source" };
    if mv.overlay_pattern > 0 {
        draw_pattern(&p, pic, mv.overlay_pattern, col.gamma_multiply(0.8));
    }
    let shade = Color32::from_black_alpha(150);
    let label = |p: &egui::Painter, pos: egui::Pos2, align: Align2, text: &str, c: Color32| {
        let g = p.layout_no_wrap(text.to_string(), Tokens::ui(11.0), c);
        let r = align.anchor_size(pos, g.size()).expand2(vec2(4.0, 2.0));
        p.rect_filled(r, 2.0, shade);
        p.galley(r.min + vec2(4.0, 2.0), g, c);
    };
    let on = mv.info_overlay;
    if on && mv.overlay_timecode {
        let tc = format_time(time + pofs, rate, drop_frame, TimeDisplay::Timecode, 48000);
        label(&p, pos2(pic.max.x - 6.0, pic.max.y - 6.0), Align2::RIGHT_BOTTOM, &tc, Color32::WHITE);
    }
    if on
        && mv.overlay_fps
        && let Some(fps) = playback_fps(ui.ctx(), prefix)
    {
        label(&p, pos2(pic.min.x + 22.0, pic.min.y + 6.0), Align2::LEFT_TOP, &tlf!("{fps} fps", fps), Color32::WHITE);
    }
    if on
        && mv.overlay_markers
        && let Some(m) = marker_at(app, which, time, rate)
    {
        let c = m.color.marker_rgb();
        let c = Color32::from_rgb(c[0], c[1], c[2]);
        let text = if m.comment.is_empty() { m.name.clone() } else { format!("{} — {}", m.name, m.comment) };
        let text = if text.trim().is_empty() { tl!("Marker").to_string() } else { text };
        label(&p, pos2(pic.center().x, pic.min.y + 6.0), Align2::CENTER_TOP, &text, c);
    }
    // the audio waveform strip along the bottom of the picture: with the waveform overlay on,
    // always (Always Show Audio Thumbnails), or while the pointer is over the bottom of a Source picture
    let strip = Rect::from_min_max(pos2(pic.min.x, pic.max.y - (pic.height() * 0.22).clamp(24.0, 90.0)), pic.max);
    let hovering = which == Which::Source && ui.ctx().pointer_hover_pos().is_some_and(|q| strip.contains(q));
    let wave_item = match which {
        Which::Program => app.session.state.active_sequence,
        Which::Source => app.session.state.source_item,
    };
    if let Some(item) = wave_item
        && ((on && mv.overlay_waveform) || (which == Which::Source && (mv.always_audio_thumbnails || hovering)))
    {
        waveform_strip(app, &p, strip, item, time, duration);
    }
    if which == Which::Source {
        jobs_overlay(app, ui, pic);
    }
}

/// A translucent audio waveform of `item` across `area` (its whole duration), with the playhead.
fn waveform_strip(app: &mut FilmcraftApp, p: &egui::Painter, area: Rect, item: redrob_film_project::ItemId, time: Tick, duration: Tick) {
    p.rect_filled(area, 0.0, Color32::from_black_alpha(110));
    let dur = duration.0.max(1) as f64;
    let mid = area.center().y;
    match crate::panels::timeline::request_peaks(app, item, 0) {
        Some(peaks) if !peaks.is_empty() => {
            let half = area.height() * 0.45;
            let spp = 256.0 * redrob_film_time::TICKS_PER_SECOND as f64 / 48_000.0;
            let mut mesh = egui::Mesh::default();
            let col = Color32::from_rgba_unmultiplied(0x4f, 0xc3, 0x7a, 200);
            for x in (area.min.x.floor() as i32)..(area.max.x.ceil() as i32) {
                let f0 = (x as f32 - area.min.x) / area.width();
                let f1 = (x as f32 + 1.0 - area.min.x) / area.width();
                let s0 = (dur * f64::from(f0) / spp) as usize;
                let s1 = ((dur * f64::from(f1) / spp) as usize).max(s0 + 1);
                let (lo, hi) = peaks.iter().skip(s0).take(s1 - s0).fold((0f32, 0f32), |(l, h), (a, b)| (l.min(*a), h.max(*b)));
                if hi - lo > 0.002 {
                    mesh.add_colored_rect(
                        Rect::from_min_max(pos2(x as f32, mid - hi.min(1.0) * half), pos2(x as f32 + 1.0, mid - lo.max(-1.0) * half + 1.0)),
                        col,
                    );
                }
            }
            p.add(mesh);
        }
        Some(_) => {}
        None => p.ctx().request_repaint_after(std::time::Duration::from_millis(100)),
    }
    let x = area.min.x + ((time.0 as f64 / dur) as f32).clamp(0.0, 1.0) * area.width();
    p.line_segment([pos2(x, area.min.y), pos2(x, area.max.y)], Stroke::new(1.0, Color32::WHITE));
}

/// Kdenlive's clip job overlay: the running jobs of the Source clip, each with its progress and an
/// X that cancels it.
fn jobs_overlay(app: &mut FilmcraftApp, ui: &mut egui::Ui, pic: Rect) {
    let Some(item) = app.session.state.source_item else { return };
    let media = redrob_film_engine::clip_ops::source_view(&app.session, item).map_or(item, |v| v.media);
    let jobs: Vec<(u64, String, f32)> = app
        .session
        .jobs
        .iter()
        .filter(|j| app.session.job_items.get(&j.id).is_some_and(|v| v.contains(&item) || v.contains(&media)))
        .filter(|j| j.result.lock().unwrap_or_else(|e| e.into_inner()).is_none())
        .map(|j| (j.id, j.label.clone(), j.progress.fraction()))
        .collect();
    if jobs.is_empty() {
        return;
    }
    let mut cancel = None;
    for (k, (id, label, f)) in jobs.iter().enumerate() {
        let r = Rect::from_min_size(pos2(pic.min.x + 8.0, pic.min.y + 30.0 + k as f32 * 24.0), vec2((pic.width() - 16.0).min(320.0), 20.0));
        let p = ui.painter();
        p.rect_filled(r, 3.0, Color32::from_black_alpha(170));
        p.rect_filled(Rect::from_min_size(r.min, vec2(r.width() * f.clamp(0.0, 1.0), r.height())), 3.0, Color32::from_rgba_unmultiplied(0x3d, 0xa5, 0xff, 90));
        p.text(r.left_center() + vec2(6.0, 0.0), Align2::LEFT_CENTER, format!("{label}  {:.0}%", f * 100.0), Tokens::ui(10.5), Color32::WHITE);
        let x = Rect::from_center_size(pos2(r.max.x - 10.0, r.center().y), vec2(14.0, 14.0));
        let resp = ui.interact(x, egui::Id::new(("source-job-cancel", *id)), Sense::click()).on_hover_text(tl!("Cancel"));
        app.auto.add(&format!("source.jobs.{id}.cancel"), x, "Cancel");
        let c = if resp.hovered() { Color32::WHITE } else { Color32::from_gray(190) };
        ui.painter().line_segment([x.left_top() + vec2(3.0, 3.0), x.right_bottom() - vec2(3.0, 3.0)], Stroke::new(1.5, c));
        ui.painter().line_segment([x.right_top() + vec2(-3.0, 3.0), x.left_bottom() + vec2(3.0, -3.0)], Stroke::new(1.5, c));
        if resp.clicked() {
            cancel = Some(*id);
        }
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    if let Some(id) = cancel
        && let Err(e) = app.session.execute("jobs.cancel", json!({"job": id}))
    {
        app.ui.status = e.to_string();
    }
}

/// Kdenlive's overlay toolbar: revealed while the pointer is over the top corner of the picture
/// (right, or left after Move Overlay Toolbar). Its buttons: Monitor Info Overlay, Change Overlay,
/// Edit Mode, Fit, full screen, move the toolbar and, in the Program Monitor with a clip selected
/// under the playhead, previous / add / next keyframe and Automatic Keyframes (the red dot).
fn overlay_toolbar(app: &mut FilmcraftApp, ui: &mut egui::Ui, which: Which, pic: Rect, mv: &crate::state::MonitorView) {
    let prefix = if which == Which::Program { "program" } else { "source" };
    let kf = which == Which::Program && crate::panels::clip_transform::target(app).is_some();
    let mut items: Vec<(&str, &str, bool)> = vec![
        ("info", tl!("Monitor Info Overlay"), mv.info_overlay),
        ("pattern", tl!("Change Overlay"), mv.overlay_pattern > 0),
        ("edit", tl!("Edit Mode"), mv.edit_handles),
        ("fit", tl!("Fit"), mv.zoom.is_none()),
        ("full", tl!("Switch Monitor Fullscreen"), window_state(ui.ctx(), which).fullscreen),
        ("move", tl!("Move Overlay Toolbar"), false),
    ];
    if kf {
        items.extend([
            ("kfPrev", tl!("Go to Previous Keyframe"), false),
            ("kfAdd", tl!("Add Keyframe"), false),
            ("kfNext", tl!("Go to Next Keyframe"), false),
            ("kfAuto", tl!("Automatic Keyframes"), mv.auto_keyframe),
        ]);
    }
    let bw = 22.0;
    let w = bw * items.len() as f32 + 4.0;
    let left = mv.toolbar_left;
    let bar = if left {
        Rect::from_min_size(pic.min + vec2(6.0, 6.0), vec2(w, bw + 4.0))
    } else {
        Rect::from_min_size(pos2(pic.max.x - 6.0 - w, pic.min.y + 6.0), vec2(w, bw + 4.0))
    };
    // the hotspot: the toolbar's corner of the picture, a little larger than the toolbar
    let hot = bar.expand2(vec2(12.0, 18.0));
    app.auto.add(&format!("{prefix}.overlayToolbar"), hot, "overlay toolbar");
    if !ui.ctx().pointer_hover_pos().is_some_and(|p| hot.contains(p)) {
        return;
    }
    ui.painter().rect_filled(bar, 4.0, Color32::from_black_alpha(170));
    let mut picked = None;
    for (k, (key, tip, on)) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(bar.min.x + 2.0 + k as f32 * bw, bar.min.y + 2.0), vec2(bw, bw));
        let resp = ui.interact(r, egui::Id::new((prefix, "overlay-toolbar", *key)), Sense::click()).on_hover_text(*tip);
        app.auto.add(&format!("{prefix}.overlayToolbar.{key}"), r, tip);
        if resp.hovered() {
            ui.painter().rect_filled(r.shrink(1.0), 3.0, Color32::from_white_alpha(40));
        }
        let c = if *on { app.tokens.accent } else { Color32::from_gray(220) };
        let ic = r.shrink(5.0);
        let p = ui.painter();
        match *key {
            "info" => icons::paint(p, ic, Icon::Info, c),
            "pattern" => icons::paint(p, ic, Icon::Grid, c),
            "edit" => icons::paint(p, ic, Icon::Selection, c),
            "fit" => icons::paint(p, ic, Icon::Zoom, c),
            "full" => icons::paint(p, ic, Icon::Fullscreen, c),
            "move" => {
                p.text(r.center(), Align2::CENTER_CENTER, if left { "▶" } else { "◀" }, Tokens::ui(11.0), c);
            }
            "kfPrev" => {
                p.text(r.center(), Align2::CENTER_CENTER, "◀◆", Tokens::ui(9.0), c);
            }
            "kfAdd" => icons::paint(p, ic, Icon::Keyframe, c),
            "kfNext" => {
                p.text(r.center(), Align2::CENTER_CENTER, "◆▶", Tokens::ui(9.0), c);
            }
            _ => {
                p.circle_filled(r.center(), 5.0, if *on { Color32::from_rgb(0xe5, 0x3e, 0x3e) } else { Color32::from_gray(140) });
            }
        }
        if resp.clicked() {
            picked = Some(*key);
        }
    }
    let Some(key) = picked else { return };
    let ctx = ui.ctx().clone();
    let r = match key {
        "info" => crate::menus::invoke(app, &ctx, "view.overlay.info", json!({"monitor": prefix})).map(|_| ()),
        "pattern" => crate::menus::invoke(app, &ctx, "view.overlay.nextPattern", json!({"monitor": prefix})).map(|_| ()),
        "edit" => crate::menus::invoke(app, &ctx, "view.overlay.editMode", json!({"monitor": prefix})).map(|_| ()),
        "fit" => crate::menus::invoke(app, &ctx, "view.magnification.fit", json!({"monitor": prefix})).map(|_| ()),
        "full" => {
            toggle_window(&ctx, which, true, None);
            Ok(())
        }
        "move" => crate::menus::invoke(app, &ctx, "view.overlay.moveToolbar", json!({"monitor": prefix})).map(|_| ()),
        "kfPrev" => crate::panels::clip_transform::keyframe_action(app, "prev"),
        "kfNext" => crate::panels::clip_transform::keyframe_action(app, "next"),
        "kfAdd" => crate::panels::clip_transform::keyframe_action(app, "add"),
        _ => {
            app.ui.program.auto_keyframe = !app.ui.program.auto_keyframe;
            Ok(())
        }
    };
    if let Err(e) = r {
        app.ui.status = e;
    }
}

/// The wrench menu's Go to Marker list: the monitor's markers (the sequence's, or the Source
/// clip's), each moving the playhead to it.
fn marker_menu(app: &mut FilmcraftApp, ui: &mut egui::Ui, which: Which) {
    let (markers, rate) = match which {
        Which::Program => match app.session.active_sequence() {
            Some(q) => (q.markers.clone(), q.settings.frame_rate),
            None => return,
        },
        Which::Source => match app.session.state.source_item.and_then(|i| redrob_film_engine::clip_ops::source_view(&app.session, i)) {
            Some(v) => (v.markers, v.rate),
            None => return,
        },
    };
    let prefix = if which == Which::Program { "program" } else { "source" };
    ui.menu_button(tl!("Go to Marker"), |ui| {
        if markers.is_empty() {
            ui.label(tl!("(no markers)"));
        }
        for (k, m) in markers.iter().enumerate() {
            let tc = format_time(m.start, rate, false, TimeDisplay::Timecode, 48000);
            let text = if m.name.is_empty() { tc.clone() } else { format!("{tc}  {}", m.name) };
            let r = ui.button(text.as_str());
            app.auto.add(&format!("{prefix}.settings.goToMarker.{k}"), r.rect, &text);
            if r.clicked() {
                let res = match which {
                    Which::Program => {
                        app.stop();
                        app.session.execute("playhead.set", json!({"time": m.start.0}))
                    }
                    Which::Source => app.session.execute("source.setPlayhead", json!({"time": m.start.0})),
                };
                if let Err(e) = res {
                    app.ui.status = e.to_string();
                }
                ui.close();
            }
        }
    });
}

/// Extract Zone… (Source wrench menu): where to write the Source zone and whether to add it to the
/// project; OK runs `clip.extractZone`.
#[derive(Clone, Default)]
struct ExtractZoneDraft {
    path: String,
    add: bool,
}

fn extract_zone_id() -> egui::Id {
    egui::Id::new("extract-zone-dialog")
}

fn open_extract_zone(app: &FilmcraftApp, ctx: &egui::Context) {
    let path = app
        .session
        .state
        .source_item
        .and_then(|i| redrob_film_engine::clip_ops::source_view(&app.session, i))
        .and_then(|v| match app.session.project.item(v.media).map(|i| &i.kind) {
            Some(ItemKind::Media(m)) => match &m.media {
                redrob_film_project::MediaRef::File { path } => {
                    let p = std::path::Path::new(path);
                    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "clip".into());
                    let ext = if m.info.video.is_some() { "mov" } else { "wav" };
                    Some(p.with_file_name(format!("{stem}_zone.{ext}")).to_string_lossy().into_owned())
                }
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_default();
    ctx.data_mut(|d| d.insert_temp(extract_zone_id(), ExtractZoneDraft { path, add: true }));
}

/// Draw the Extract Zone dialog while it is open.
pub fn extract_zone_dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = ctx.data(|m| m.get_temp::<ExtractZoneDraft>(extract_zone_id())) else { return };
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let mut run = false;
    egui::Window::new(tl!("Extract Zone"))
        .id(egui::Id::new("Extract Zone"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(tl!("Write the Source Monitor's zone (In to Out) to a new file."));
            ui.horizontal(|ui| {
                ui.label(tl!("File:"));
                let r = ui.add(egui::TextEdit::singleline(&mut d.path).desired_width(360.0));
                app.auto.add("extractZone.path", r.rect, "File");
            });
            let r = ui.checkbox(&mut d.add, tl!("Add clip to project"));
            app.auto.add("extractZone.add", r.rect, "Add clip to project");
            ui.horizontal(|ui| {
                let r = ui.button(tl!("Cancel"));
                app.auto.add("extractZone.cancel", r.rect, "Cancel");
                close |= r.clicked();
                let r = ui.button(tl!("OK"));
                app.auto.add("extractZone.ok", r.rect, "OK");
                run = r.clicked();
            });
        });
    if run {
        match app.session.execute("clip.extractZone", json!({"path": d.path, "addToProject": d.add})) {
            Ok(_) => {
                app.ui.status = tl!("Extracting the zone…").into();
                close = true;
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    if close {
        ctx.data_mut(|m| m.remove::<ExtractZoneDraft>(extract_zone_id()));
    } else {
        ctx.data_mut(|m| m.insert_temp(extract_zone_id(), d));
    }
}

/// The wrench menu's Monitor Overlay fly-out (Kdenlive's Current Monitor Overlay / Monitor Config):
/// the info overlays, the guide pattern, Edit Mode, and on the Source Monitor the audio thumbnails
/// and source timecode switches. Each item runs its `view.*` command on this monitor.
fn overlay_menu(app: &mut FilmcraftApp, ui: &mut egui::Ui, which: Which) {
    let prefix = if which == Which::Program { "program" } else { "source" };
    let mv = monitor_view::view(app, which).clone();
    let mut run: Option<String> = None;
    ui.menu_button(tl!("Monitor Overlay"), |ui| {
        let mut rows: Vec<(&str, &str, bool)> = vec![
            ("overlay.info", tl!("Monitor Info Overlay"), mv.info_overlay),
            ("overlay.timecode", tl!("Timecode"), mv.overlay_timecode),
            ("overlay.fps", tl!("Playback FPS"), mv.overlay_fps),
            ("overlay.markers", tl!("Markers"), mv.overlay_markers),
            ("overlay.waveform", tl!("Audio Waveform"), mv.overlay_waveform),
            ("overlay.editMode", tl!("Edit Mode"), mv.edit_handles),
        ];
        if which == Which::Source {
            rows.push(("overlay.audioThumbnails", tl!("Always Show Audio Thumbnails"), mv.always_audio_thumbnails));
            rows.push(("sourceTimecode", tl!("Show Source Timecode"), mv.source_timecode));
        } else {
            rows.push(("multitrack", tl!("Multitrack View"), mv.multitrack));
        }
        for (cmd, label, on) in rows {
            let mut b = on;
            let r = ui.checkbox(&mut b, label);
            app.auto.add(&format!("{prefix}.settings.{cmd}"), r.rect, label);
            if r.changed() {
                run = Some(cmd.to_string());
            }
        }
        ui.separator();
        ui.label(tl!("Guide Pattern"));
        for (n, name) in crate::state::OVERLAY_PATTERNS.iter().enumerate() {
            let r = ui.selectable_label(usize::from(mv.overlay_pattern) == n, crate::i18n::t(name));
            app.auto.add(&format!("{prefix}.settings.overlay.pattern.{n}"), r.rect, name);
            if r.clicked() {
                run = Some(format!("overlay.pattern.{n}"));
            }
        }
    });
    if let Some(cmd) = run
        && let Some(Err(e)) = monitor_view::route(app, &format!("view.{cmd}"), &json!({"monitor": prefix}))
    {
        app.ui.status = e;
    }
}

/// Copies of the project in which only one video track of `seq` is visible (index = track
/// index), cached per revision for Multitrack View.
fn solo_track_projects(app: &FilmcraftApp, ctx: &egui::Context, seq: redrob_film_project::ItemId) -> Vec<std::sync::Arc<redrob_film_project::Project>> {
    let id = egui::Id::new(("multitrack-projects", seq.0));
    let rev = app.session.revision;
    type Cached = (u64, Vec<std::sync::Arc<redrob_film_project::Project>>);
    if let Some((r, v)) = ctx.data(|d| d.get_temp::<Cached>(id))
        && r == rev
    {
        return v;
    }
    let n = app.session.project.sequence(seq).map_or(0, |q| q.video_tracks.len());
    let v: Vec<_> = (0..n)
        .map(|k| {
            let mut p = (*app.session.project).clone();
            if let Some(q) = p.sequence_mut(seq) {
                for (i, t) in q.video_tracks.iter_mut().enumerate() {
                    t.enabled = i == k;
                }
            }
            std::sync::Arc::new(p)
        })
        .collect();
    ctx.data_mut(|d| d.insert_temp::<Cached>(id, (rev, v.clone())));
    v
}

/// Kdenlive's Multitrack View: each video track of the sequence alone in a cell of a grid (V1 at
/// the top left). A click on a cell targets that track and patches the source video to it.
fn multitrack_grid(app: &mut FilmcraftApp, ui: &mut egui::Ui, area: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let Some(seq_id) = app.session.state.active_sequence else { return };
    let Some(q) = app.session.active_sequence() else { return };
    let tracks: Vec<(redrob_film_project::TrackId, String)> = q.video_tracks.iter().map(|tr| (tr.id, tr.name.clone())).collect();
    let (fw, fh, rate) = (q.settings.width.max(1) as f32, q.settings.height.max(1) as f32, q.settings.frame_rate);
    if tracks.is_empty() {
        crate::dock::placeholder(ui, area, &t, tl!("(no video tracks)"));
        return;
    }
    let projects = solo_track_projects(app, &ctx, seq_id);
    let n = tracks.len();
    let cols = (n as f32).sqrt().ceil().max(1.0) as usize;
    let rows = n.div_ceil(cols);
    let cw = area.width() / cols as f32;
    let ch = area.height() / rows as f32;
    let frame = rate.frame_at(app.session.playhead());
    let ppp = ctx.pixels_per_point();
    let target = app.session.targeting().video_dest;
    let rev = app.session.revision;
    let mut pick = None;
    for (k, (tid, name)) in tracks.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(area.min.x + (k % cols) as f32 * cw, area.min.y + (k / cols) as f32 * ch), vec2(cw, ch)).shrink(2.0);
        let pic = fit(cell.shrink2(vec2(0.0, 9.0)), fw, fh);
        ui.painter().rect_filled(pic, 0.0, t.monitor_bg);
        let scale = quantize_scale((pic.width() * ppp / fw).clamp(1.0 / 32.0, 1.0));
        if let Some(p) = projects.get(k) {
            let salt = 0x6d75_6c74_0000_0000u64 ^ (k as u64 + 1).rotate_left(29);
            let key = FrameKey { target: Target::Sequence(seq_id), frame, size: (scale * 1000.0) as u32, revision: rev ^ salt, draft: false };
            app.frames.request(key, rate.tick_of(frame), scale, p, 1);
            if let (Some(tex), _) = cpu_texture(app, &ctx, &format!("multitrack-{k}"), key, &[], DisplayMode::Composite) {
                ui.painter().image(tex, pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        }
        let on = target == Some(*tid);
        ui.painter().rect_stroke(pic, 0.0, Stroke::new(if on { 2.0 } else { 1.0 }, if on { t.accent } else { t.control_border }), StrokeKind::Outside);
        ui.painter().text(pos2(pic.min.x + 4.0, pic.min.y + 3.0), Align2::LEFT_TOP, name, Tokens::ui(10.5), Color32::WHITE);
        let resp = ui.interact(cell, egui::Id::new(("multitrack-cell", k)), Sense::click());
        app.auto.add(&format!("program.multitrack.{k}"), cell, name);
        if resp.clicked() {
            pick = Some(*tid);
        }
    }
    if let Some(tid) = pick
        && let Err(e) = app.session.execute("timeline.setTargeting", json!({"track": tid.0, "targeted": true, "sourcePatch": true}))
    {
        app.ui.status = e.to_string();
    }
}
