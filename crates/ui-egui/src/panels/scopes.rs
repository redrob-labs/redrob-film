//! Lumetri Scopes: Vectorscope YUV / HLS, Histogram, Parade (RGB / YUV / RGB-White) and Waveform
//! (RGB / Luma / YC / YC no Chroma) of the Program frame, in a grid when several are shown.
//!
//! The maths is in `redrob-film-scopes` (count grids from the decimated frame); this module turns
//! them into textures (cached per frame and settings) and draws the graticules, labels and the
//! panel chrome: the footer (colour space, wrench, Clamp Signal, 8 Bit / Float / HDR) and the
//! settings menu (wrench or right-click: Presets, the scopes, Waveform / Parade Type, Colour
//! Space, Brightness, Vectorscope Targets).
//!
//! Automation ids: `scopes.view.<kind>` (each scope), `scopes.wrench`, `scopes.clamp`,
//! `scopes.scale`, `scopes.space`; menu items `scopes.menu.<kind>`, `scopes.menu.preset.<n>`,
//! `scopes.menu.waveformType.<type>`, `scopes.menu.paradeType.<type>`,
//! `scopes.menu.colorSpace.<space>`, `scopes.menu.brightness.<b>`, `scopes.menu.targets.<75|100>`;
//! `scopes.hdrWaveform` while levels are shown in cd/m². State: `ui.panels.scopes`.
//!
//! Kdenlive-style options in the same menu: Auto Refresh / Refresh Now (`scopes.menu.autoRefresh`,
//! `scopes.menu.refresh`; a click on the scopes refreshes too), Realtime, Scope Source
//! (`scopes.menu.source.<active|program|source>`), Histogram Components
//! (`scopes.menu.histogram.<y|sum|r|g|b|unscaled>`), Paint Mode (`scopes.menu.paintMode.<m>`),
//! Gradient Reference Line, and under Vectorscope the YPbPr / YUV model
//! (`scopes.menu.colorModel.*`), I and Q Lines, Gain (`scopes.menu.gain.<n>`) and Export
//! Background (`scopes.menu.exportBackground.<rgb|yuv|ycbcr>`). Readouts: `scopes.hoverLevel`,
//! `scopes.histogram.minMax`, `scopes.vectorscope.mode`.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{Align2, Color32, Rect, Sense, Stroke, TextureHandle, TextureOptions, pos2, vec2};
use redrob_film_scopes::{self as sc, Brightness, ColorSpace, PaintMode, ParadeType, Params, Scale, ScopeKind, Signal, Targets, WaveformType};

use crate::FilmcraftApp;
use crate::frames::{FrameKey, Target};
use crate::icons::{self, Icon};
use crate::panels::panel_state::{SCOPE_PRESETS, ScopeSource, ScopesState, VECTOR_GAINS};
use crate::theme::Tokens;

const GRATICULE: Color32 = Color32::from_rgb(0x2a, 0x2a, 0x2a);
const LABEL: Color32 = Color32::from_rgb(0xb0, 0xb0, 0xb0);
const FOOTER_H: f32 = 30.0;

/// A frame prepared for the scopes.
#[derive(Clone)]
pub struct ScopeFrame {
    pub signal: Arc<Signal>,
    /// Identity of the frame (cache key).
    pub key: u64,
    /// PQ code values of a working-space render (levels in cd/m²).
    pub pq: bool,
    pub space: ColorSpace,
}

fn hash_of(v: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// The active sequence's frame `frame` for the scopes: the monitor picture (¼ resolution, from
/// the frame workers at priority `prio`) or, for Rec. 2100 scopes of an HDR sequence, a small
/// working-space render. While a new frame renders the last one stays.
pub fn frame_signal(app: &mut FilmcraftApp, ctx: &egui::Context, slot: &str, frame: i64, st: &ScopesState, prio: u32) -> Option<ScopeFrame> {
    let seq_id = app.session.state.active_sequence?;
    let q = app.session.active_sequence()?;
    let hdr_seq = q.settings.color.working.is_hdr();
    let space = st.color_space.resolve(hdr_seq);
    let pq = hdr_seq && space == ColorSpace::Rec2100;
    let rate = q.settings.frame_rate;
    let rev = app.session.revision;
    let id = egui::Id::new(("scope-signal", slot));
    let cached: Option<(u64, Arc<Signal>, bool)> = ctx.data(|d| d.get_temp(id));
    let key = hash_of((seq_id.0, frame, rev, pq, st.realtime));
    if let Some((k, s, p)) = &cached
        && *k == key
    {
        return Some(ScopeFrame { signal: s.clone(), key, pq: *p, space });
    }
    // Realtime: a quarter-resolution frame decimated to MAX_W × MAX_H; off: every pixel
    let fresh = if pq {
        redrob_film_engine::scopes::scope_signal(&app.session, rate.tick_of(frame), if st.realtime { 0.125 } else { 0.5 }, true)
    } else {
        let scale = if st.realtime { 0.25 } else { 1.0 };
        let fkey = FrameKey { target: Target::Sequence(seq_id), frame, size: (scale * 1000.0) as u32, revision: rev, draft: false };
        let project = app.session.project.clone();
        app.frames.request(fkey, rate.tick_of(frame), scale, &project, prio);
        app.frames.get(&fkey).map(|img| decimate(img.w, img.h, &img.px, st.realtime))
    };
    match fresh {
        Some(s) => {
            let s = Arc::new(s);
            ctx.data_mut(|d| d.insert_temp(id, (key, s.clone(), pq)));
            Some(ScopeFrame { signal: s, key, pq, space })
        }
        None => cached.filter(|c| c.2 == pq).map(|(k, s, p)| ScopeFrame { signal: s, key: k, pq: p, space }),
    }
}

/// A frame's RGBA8 pixels as a scope signal: decimated (Realtime) or every pixel.
fn decimate(w: usize, h: usize, px: &[u8], realtime: bool) -> Signal {
    if realtime { Signal::from_rgba8(w, h, px) } else { Signal::from_rgba8_max(w, h, px, FULL_W, FULL_H) }
}

/// Largest signal analysed with Realtime off (UHD every pixel; bigger frames are sampled).
const FULL_W: usize = 3840;
const FULL_H: usize = 2160;

/// The Source monitor's frame for the scopes (the clip at the Source playhead).
pub fn source_signal(app: &mut FilmcraftApp, ctx: &egui::Context, st: &ScopesState, prio: u32) -> Option<ScopeFrame> {
    let item = app.session.state.source_item?;
    let view = redrob_film_engine::clip_ops::source_view(&app.session, item)?;
    let frame = view.rate.frame_at(app.session.state.source_playhead);
    let rev = app.item_revision(item);
    let space = st.color_space.resolve(false);
    let id = egui::Id::new(("scope-signal", "source"));
    let cached: Option<(u64, Arc<Signal>, bool)> = ctx.data(|d| d.get_temp(id));
    let key = hash_of(("source", item.0, frame, rev, st.realtime));
    if let Some((k, s, _)) = &cached
        && *k == key
    {
        return Some(ScopeFrame { signal: s.clone(), key, pq: false, space });
    }
    let scale = if st.realtime { 0.25 } else { 1.0 };
    let fkey = FrameKey { target: Target::Item(item), frame, size: (scale * 1000.0) as u32, revision: rev, draft: false };
    let project = app.session.project.clone();
    app.frames.request(fkey, view.rate.tick_of(frame), scale, &project, prio);
    match app.frames.get(&fkey).map(|img| decimate(img.w, img.h, &img.px, st.realtime)) {
        Some(s) => {
            let s = Arc::new(s);
            ctx.data_mut(|d| d.insert_temp(id, (key, s.clone(), false)));
            Some(ScopeFrame { signal: s, key, pq: false, space })
        }
        None => cached.map(|(k, s, _)| ScopeFrame { signal: s, key: k, pq: false, space }),
    }
}

/// Whether the scopes read the Source monitor now (Scope Source, or the focused monitor).
pub fn reads_source(app: &FilmcraftApp, st: &ScopesState) -> bool {
    match st.source {
        ScopeSource::Program => false,
        ScopeSource::Source => true,
        ScopeSource::Active => app.ui.focused == crate::dock::PanelKind::Source && app.session.state.source_item.is_some(),
    }
}

fn frozen_id() -> egui::Id {
    egui::Id::new("scopes-frozen")
}
fn refresh_id() -> egui::Id {
    egui::Id::new("scopes-refresh")
}

/// Refresh the scopes once (a click on them, or the menu's Refresh Now) while Auto Refresh is off.
pub fn request_refresh(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(refresh_id(), true));
}

/// The Lumetri Scopes panel.
pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let st = app.ui.panels.scopes.clone();
    let area = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - FOOTER_H));
    ui.painter().rect_filled(area, 0.0, Color32::BLACK);
    let ctx = ui.ctx().clone();
    let from_source = reads_source(app, &st);
    let playing = if from_source { app.source_playback.clock.playing } else { app.playback.playing };
    // while playing, scopes come after the monitor's prefetch
    let prio = if playing { 40 } else { 2 };
    // Auto Refresh off: the frame last analysed stays until the scopes are clicked
    let frozen: Option<(bool, ScopeFrame)> = ctx.data(|d| d.get_temp(frozen_id()));
    let refresh = ctx.data_mut(|d| d.remove_temp::<bool>(refresh_id())).unwrap_or(false);
    let keep = !st.auto_refresh && !refresh && frozen.as_ref().is_some_and(|f| f.0 == from_source);
    let sf = if keep {
        frozen.map(|f| f.1)
    } else if from_source {
        source_signal(app, &ctx, &st, prio)
    } else {
        let frame = app.session.state.active_sequence.map(|_| app.session.sequence_rate().frame_at(app.session.playhead()));
        frame.and_then(|f| frame_signal(app, &ctx, "program", f, &st, prio))
    };
    if !keep {
        match &sf {
            Some(f) => ctx.data_mut(|d| d.insert_temp(frozen_id(), (from_source, f.clone()))),
            // keep asking until the refreshed frame has rendered
            None if refresh => request_refresh(&ctx),
            None => {}
        }
    }
    match &sf {
        Some(sf) => draw(app, ui, area, &st.shown, sf, &st, "scopes"),
        None => crate::dock::placeholder(
            ui,
            area,
            &t,
            if from_source {
                tl!("(no clips)")
            } else if app.session.state.active_sequence.is_some() {
                "…"
            } else {
                tl!("(no sequence)")
            },
        ),
    }
    if !st.auto_refresh {
        ui.painter().text(area.right_top() + vec2(-6.0, 4.0), Align2::RIGHT_TOP, tl!("Auto Refresh off: click to refresh"), Tokens::ui(9.0), Color32::from_gray(120));
    }
    // right-click anywhere on the scopes: the settings menu; a click refreshes
    let resp = ui.interact(area, egui::Id::new("scopes-area"), Sense::click());
    if resp.clicked() {
        request_refresh(&ctx);
    }
    egui::Popup::context_menu(&resp).show(|ui| settings_menu(app, ui));
    footer(app, ui, Rect::from_min_max(pos2(rect.min.x, area.max.y), rect.max), sf.as_ref().map(|s| s.space));
}

fn footer(app: &mut FilmcraftApp, ui: &mut egui::Ui, r: Rect, space: Option<ColorSpace>) {
    let t = app.tokens;
    let (scale0, clamp0, space0) = (app.ui.panels.scopes.scale, app.ui.panels.scopes.clamp, app.ui.panels.scopes.color_space);
    let space = space.unwrap_or(space0.resolve(false));
    let label = match space {
        ColorSpace::Rec2100 => "Rec. 2100".to_string(),
        s => crate::i18n::t(s.label()).to_string(),
    };
    let lr = ui.painter().text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, &label, Tokens::ui(12.0), t.text);
    app.auto.add("scopes.space", lr, &label);
    // right side: wrench · ☑ Clamp Signal · [8 Bit ▾]
    let scale_w = 92.0;
    let sr = Rect::from_min_size(pos2(r.max.x - scale_w - 6.0, r.min.y + 4.0), vec2(scale_w, 22.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sr));
    let mut scale = scale0;
    let cb = egui::ComboBox::from_id_salt("scopes-scale").selected_text(crate::i18n::t(scale.label())).width(scale_w - 8.0).show_ui(&mut child, |ui| {
        for s in Scale::ALL {
            let r = ui.selectable_value(&mut scale, s, crate::i18n::t(s.label()));
            app.auto.add(&format!("scopes.menu.scale.{}", s.label().replace(' ', "")), r.rect, s.label());
        }
    });
    app.auto.add("scopes.scale", cb.response.rect, "Signal scale");
    let galley = ui.painter().layout_no_wrap(tl!("Clamp Signal").into(), Tokens::ui(12.0), t.text);
    let cw = galley.size().x + 24.0;
    let cr = Rect::from_min_size(pos2(sr.min.x - cw - 10.0, r.min.y + 4.0), vec2(cw, 22.0));
    let mut clamp = clamp0;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(cr));
    let cresp = child.checkbox(&mut clamp, tl!("Clamp Signal"));
    app.auto.add("scopes.clamp", cresp.rect, "Clamp Signal");
    let wr = Rect::from_center_size(pos2(cr.min.x - 16.0, r.center().y), vec2(22.0, 22.0));
    let wresp = ui.interact(wr, egui::Id::new("scopes-wrench"), Sense::click());
    icons::paint(ui.painter(), wr.shrink(4.0), Icon::Wrench, if wresp.hovered() { t.tab_text_active } else { t.icon });
    app.auto.add("scopes.wrench", wr, "Scope settings");
    egui::Popup::menu(&wresp).show(|ui| settings_menu(app, ui));
    // only what the footer itself changed (the wrench menu above may have changed more)
    let st = &mut app.ui.panels.scopes;
    if scale != scale0 {
        st.scale = scale;
        st.preset.clear();
    }
    if clamp != clamp0 {
        st.clamp = clamp;
        st.preset.clear();
    }
}

type Picks = Vec<(String, Rect, String, bool)>;

fn pick(ui: &mut egui::Ui, out: &mut Picks, id: &str, label: &str, on: bool) {
    let r = ui.selectable_label(on, label);
    out.push((id.to_string(), r.rect, label.to_string(), r.clicked()));
}

/// The scopes' settings menu (wrench and right-click).
pub fn settings_menu(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    ui.set_min_width(220.0);
    let st = app.ui.panels.scopes.clone();
    let mut out: Picks = Vec::new();
    let pr = ui.menu_button(tl!("Presets"), |ui| {
        for (i, p) in SCOPE_PRESETS.iter().enumerate() {
            pick(ui, &mut out, &format!("scopes.menu.preset.{i}"), crate::i18n::t(p.0), st.preset == p.0);
        }
    });
    out.push(("scopes.menu.presets".into(), pr.response.rect, "Presets".into(), false));
    ui.separator();
    for k in ScopeKind::ALL {
        let label = match k {
            ScopeKind::Parade => tlf!("Parade ({kind})", kind = crate::i18n::t(st.parade_type.label())),
            ScopeKind::Waveform => tlf!("Waveform ({kind})", kind = crate::i18n::t(st.waveform_type.label())),
            k => crate::i18n::t(k.label()).to_string(),
        };
        pick(ui, &mut out, &format!("scopes.menu.{}", k.name()), &label, st.shown.contains(&k));
    }
    ui.separator();
    let sub = |ui: &mut egui::Ui, out: &mut Picks, id: &str, label: &str, f: &mut dyn FnMut(&mut egui::Ui, &mut Picks)| {
        let r = ui.menu_button(label, |ui| f(ui, out));
        out.push((format!("scopes.menu.{id}"), r.response.rect, label.into(), false));
    };
    sub(ui, &mut out, "paradeTypes", tl!("Parade Type"), &mut |ui, out| {
        for p in ParadeType::ALL {
            pick(ui, out, &format!("scopes.menu.paradeType.{}", serde_name(&p)), crate::i18n::t(p.label()), st.parade_type == p);
        }
    });
    sub(ui, &mut out, "waveformTypes", tl!("Waveform Type"), &mut |ui, out| {
        for w in WaveformType::ALL {
            pick(ui, out, &format!("scopes.menu.waveformType.{}", serde_name(&w)), crate::i18n::t(w.label()), st.waveform_type == w);
        }
    });
    sub(ui, &mut out, "vectorscopeTargets", tl!("Vectorscope Targets"), &mut |ui, out| {
        for v in [Targets::Percent75, Targets::Percent100] {
            pick(ui, out, &format!("scopes.menu.targets.{}", serde_name(&v)), crate::i18n::t(v.label()), st.targets == v);
        }
    });
    ui.separator();
    sub(ui, &mut out, "colorSpaces", tl!("Colour Space"), &mut |ui, out| {
        for c in ColorSpace::ALL {
            pick(ui, out, &format!("scopes.menu.colorSpace.{}", serde_name(&c)), crate::i18n::t(c.label()), st.color_space == c);
        }
    });
    sub(ui, &mut out, "brightnesses", tl!("Brightness"), &mut |ui, out| {
        for b in Brightness::ALL {
            pick(ui, out, &format!("scopes.menu.brightness.{}", serde_name(&b)), crate::i18n::t(b.label()), st.brightness == b);
        }
    });
    sub(ui, &mut out, "scales", tl!("Signal Scale"), &mut |ui, out| {
        for s in Scale::ALL {
            pick(ui, out, &format!("scopes.menu.scale.{}", s.label().replace(' ', "")), crate::i18n::t(s.label()), st.scale == s);
        }
    });
    pick(ui, &mut out, "scopes.menu.clamp", tl!("Clamp Signal"), st.clamp);
    ui.separator();
    // Kdenlive-style scope options (all scopes; the histogram, parade and vectorscope ones)
    pick(ui, &mut out, "scopes.menu.autoRefresh", tl!("Auto Refresh"), st.auto_refresh);
    pick(ui, &mut out, "scopes.menu.refresh", tl!("Refresh Now"), false);
    pick(ui, &mut out, "scopes.menu.realtime", tl!("Realtime"), st.realtime);
    sub(ui, &mut out, "sources", tl!("Scope Source"), &mut |ui, out| {
        for s in ScopeSource::ALL {
            pick(ui, out, &format!("scopes.menu.source.{}", s.name()), crate::i18n::t(s.label()), st.source == s);
        }
    });
    sub(ui, &mut out, "histogram", tl!("Histogram Components"), &mut |ui, out| {
        pick(ui, out, "scopes.menu.histogram.y", "Y", st.hist_luma);
        pick(ui, out, "scopes.menu.histogram.sum", tl!("Sum"), st.hist_sum);
        pick(ui, out, "scopes.menu.histogram.r", "R", st.hist_red);
        pick(ui, out, "scopes.menu.histogram.g", "G", st.hist_green);
        pick(ui, out, "scopes.menu.histogram.b", "B", st.hist_blue);
        ui.separator();
        pick(ui, out, "scopes.menu.histogram.unscaled", tl!("Unscaled"), st.hist_unscaled);
    });
    sub(ui, &mut out, "paintModes", tl!("Paint Mode"), &mut |ui, out| {
        for m in PaintMode::ALL {
            pick(ui, out, &format!("scopes.menu.paintMode.{}", serde_name(&m)), crate::i18n::t(m.label()), st.paint_mode == m);
        }
    });
    pick(ui, &mut out, "scopes.menu.gradientLine", tl!("Gradient Reference Line"), st.gradient_line);
    sub(ui, &mut out, "vectorscope", tl!("Vectorscope"), &mut |ui, out| {
        pick(ui, out, "scopes.menu.colorModel.ypbpr", "YPbPr", !st.analog_yuv);
        pick(ui, out, "scopes.menu.colorModel.yuv", "YUV", st.analog_yuv);
        ui.separator();
        pick(ui, out, "scopes.menu.iqLines", tl!("I and Q Lines"), st.iq_lines);
        ui.separator();
        for g in VECTOR_GAINS {
            pick(ui, out, &format!("scopes.menu.gain.{g}"), &tlf!("Gain {n}×", n = g), (st.vector_gain - g).abs() < 1e-3);
        }
        ui.separator();
        for p in sc::paint::Plane::ALL {
            pick(ui, out, &format!("scopes.menu.exportBackground.{}", p.name()), &tlf!("Export Background ({plane})…", plane = p.label()), false);
        }
    });
    let mut close = false;
    for (id, r, label, clicked) in out {
        app.auto.add(&id, r, &label);
        if clicked {
            if !apply_app_menu_id(app, ui.ctx(), &id) {
                apply_menu_id(&mut app.ui.panels.scopes, &id);
            }
            close = true;
        }
    }
    if close {
        ui.close();
    }
}

/// Menu items that act on more than the settings: Refresh Now and Export Background.
pub fn apply_app_menu_id(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str) -> bool {
    if id == "scopes.menu.refresh" {
        request_refresh(ctx);
        return true;
    }
    let Some(name) = id.strip_prefix("scopes.menu.exportBackground.") else { return false };
    let Some(plane) = sc::paint::Plane::ALL.into_iter().find(|p| p.name() == name) else { return true };
    let st = &app.ui.panels.scopes;
    let p = Params { matrix: st.color_space.resolve(false).matrix(), analog_yuv: st.analog_yuv, ..Params::default() };
    let suggested = format!("vectorscope-{}.png", plane.name());
    let Some(path) = app.hooks.pick_save_as.as_mut().and_then(|f| f(tl!("PNG image"), &["png"], &suggested)) else { return true };
    let path = if path.to_ascii_lowercase().ends_with(".png") { path } else { format!("{path}.png") };
    app.ui.status = match export_background(&path, plane, &p) {
        Ok(()) => tlf!("Exported the vectorscope background to {path}", path = path),
        Err(e) => tlf!("Could not export the vectorscope background: {error}", error = e),
    };
    true
}

/// Write the colour plane of the vectorscope (512 px square, the scope's extent) as a PNG.
pub fn export_background(path: &str, plane: sc::paint::Plane, p: &Params) -> Result<(), String> {
    let (w, h, px) = sc::paint::background(plane, p.matrix, 512, p.vector_extent());
    let img = image::RgbaImage::from_raw(w as u32, h as u32, px).ok_or("bad image size")?;
    img.save_with_format(path, image::ImageFormat::Png).map_err(|e| e.to_string())
}

fn serde_name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// Apply a settings menu item by its automation id (`scopes.menu.…`).
pub fn apply_menu_id(st: &mut ScopesState, id: &str) -> bool {
    let Some(rest) = id.strip_prefix("scopes.menu.") else { return false };
    let from = |s: &str, all: &[String]| all.iter().position(|x| x == s);
    if let Some(i) = rest.strip_prefix("preset.").and_then(|n| n.parse::<usize>().ok()) {
        return SCOPE_PRESETS.get(i).is_some_and(|p| st.apply_preset(p.0));
    }
    if let Some(k) = ScopeKind::ALL.into_iter().find(|k| k.name() == rest) {
        st.toggle(k);
        return true;
    }
    let (group, name) = rest.split_once('.').unwrap_or((rest, ""));
    let ok = match group {
        "paradeType" => from(name, &ParadeType::ALL.map(|x| serde_name(&x))).map(|i| st.parade_type = ParadeType::ALL[i]).is_some(),
        "waveformType" => from(name, &WaveformType::ALL.map(|x| serde_name(&x))).map(|i| st.waveform_type = WaveformType::ALL[i]).is_some(),
        "targets" => from(name, &[Targets::Percent75, Targets::Percent100].map(|x| serde_name(&x)))
            .map(|i| st.targets = [Targets::Percent75, Targets::Percent100][i])
            .is_some(),
        "colorSpace" => from(name, &ColorSpace::ALL.map(|x| serde_name(&x))).map(|i| st.color_space = ColorSpace::ALL[i]).is_some(),
        "brightness" => from(name, &Brightness::ALL.map(|x| serde_name(&x))).map(|i| st.brightness = Brightness::ALL[i]).is_some(),
        "scale" => Scale::from_name(name).map(|s| st.scale = s).is_some(),
        "clamp" => {
            st.clamp = !st.clamp;
            true
        }
        "autoRefresh" => {
            st.auto_refresh = !st.auto_refresh;
            true
        }
        "realtime" => {
            st.realtime = !st.realtime;
            true
        }
        "gradientLine" => {
            st.gradient_line = !st.gradient_line;
            true
        }
        "iqLines" => {
            st.iq_lines = !st.iq_lines;
            true
        }
        "source" => ScopeSource::ALL.into_iter().find(|s| s.name() == name).map(|s| st.source = s).is_some(),
        "paintMode" => from(name, &PaintMode::ALL.map(|x| serde_name(&x))).map(|i| st.paint_mode = PaintMode::ALL[i]).is_some(),
        "colorModel" => match name {
            "yuv" => {
                st.analog_yuv = true;
                true
            }
            "ypbpr" => {
                st.analog_yuv = false;
                true
            }
            _ => false,
        },
        "gain" => name.parse::<f32>().ok().filter(|g| VECTOR_GAINS.iter().any(|x| (x - g).abs() < 1e-3)).map(|g| st.vector_gain = g).is_some(),
        "histogram" => {
            let f = match name {
                "y" => &mut st.hist_luma,
                "sum" => &mut st.hist_sum,
                "r" => &mut st.hist_red,
                "g" => &mut st.hist_green,
                "b" => &mut st.hist_blue,
                "unscaled" => &mut st.hist_unscaled,
                _ => return false,
            };
            *f = !*f;
            true
        }
        _ => false,
    };
    if ok && !matches!(group, "colorSpace" | "brightness" | "targets" | "autoRefresh" | "realtime" | "gradientLine" | "iqLines" | "source" | "paintMode" | "colorModel" | "gain" | "histogram") {
        st.preset.clear();
    }
    ok
}

/// Cells for `n` scopes in `area`: the grid whose cells are closest to 4:3.
pub fn layout(area: Rect, n: usize) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let best = (1..=n)
        .map(|cols| {
            let rows = n.div_ceil(cols);
            let a = (area.width() / cols as f32) / (area.height() / rows as f32).max(1.0);
            (cols, (a / 1.33).ln().abs() + if cols * rows > n { 0.15 } else { 0.0 })
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|x| x.0)
        .unwrap_or(1);
    let rows = n.div_ceil(best);
    let (cw, ch) = (area.width() / best as f32, area.height() / rows as f32);
    (0..n).map(|i| Rect::from_min_size(area.min + vec2((i % best) as f32 * cw, (i / best) as f32 * ch), vec2(cw, ch)).shrink(2.0)).collect()
}

/// Draw `shown` scopes of a frame into `area` (black background expected).
pub fn draw(app: &mut FilmcraftApp, ui: &mut egui::Ui, area: Rect, shown: &[ScopeKind], sf: &ScopeFrame, st: &ScopesState, prefix: &str) {
    let cells = layout(area, shown.len());
    if shown.len() > 1 {
        for c in &cells {
            ui.painter().rect_stroke(c.expand(1.0), 0.0, Stroke::new(1.0, Color32::from_gray(24)), egui::StrokeKind::Outside);
        }
    }
    for (k, cell) in shown.iter().zip(cells) {
        draw_one(app, ui, cell, *k, sf, st, prefix);
        app.auto.add(&format!("{prefix}.view.{}", k.name()), cell, k.label());
    }
}

/// Scope parameters; `rows` = the plot's height in pixels (64–256), so levels are not resampled.
fn params(st: &ScopesState, sf: &ScopeFrame, rows: usize) -> Params {
    Params { matrix: sf.space.matrix(), clamp: st.clamp, rows, analog_yuv: st.analog_yuv, vector_gain: st.vector_gain, ..Params::default() }
}

/// Levels on a cd/m² axis: HDR frames, or the HDR scale.
fn nits_axis(st: &ScopesState, sf: &ScopeFrame) -> bool {
    sf.pq || st.scale == Scale::Hdr
}

/// The frame's signal on the axis the scale asks for (SDR frames on the HDR scale go to PQ).
fn plotted(st: &ScopesState, sf: &ScopeFrame) -> Arc<Signal> {
    if st.scale == Scale::Hdr && !sf.pq { Arc::new(sf.signal.map(|c| c.map(sc::sdr_to_pq))) } else { sf.signal.clone() }
}

/// A texture computed from (frame, settings, scope), kept until either changes.
fn cached_texture(ctx: &egui::Context, slot: String, key: u64, make: impl FnOnce() -> (usize, usize, Vec<u8>)) -> Option<TextureHandle> {
    let id = egui::Id::new(("scope-tex", slot.clone()));
    if let Some((k, tex)) = ctx.data(|d| d.get_temp::<(u64, TextureHandle)>(id))
        && k == key
    {
        return Some(tex);
    }
    let (w, h, px) = make();
    if w == 0 || h == 0 {
        return None;
    }
    let img = egui::ColorImage::from_rgba_premultiplied([w, h], &px);
    let tex = ctx.load_texture(slot, img, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
    Some(tex)
}

fn settings_key(st: &ScopesState) -> u64 {
    hash_of(serde_json::to_string(st).unwrap_or_default())
}

const RED: [f32; 3] = [1.0, 0.16, 0.16];
const GREEN: [f32; 3] = [0.16, 1.0, 0.16];
const BLUE: [f32; 3] = [0.3, 0.38, 1.0];
const WHITE: [f32; 3] = [0.85, 0.85, 0.85];
const LUMA: [f32; 3] = [0.55, 1.0, 0.7];

fn trace_color(name: &str, wt: Option<WaveformType>, mode: PaintMode) -> [f32; 3] {
    match mode {
        PaintMode::Green => return [0.25, 1.0, 0.35],
        PaintMode::Grey => return [0.8, 0.8, 0.8],
        PaintMode::Color => {}
    }
    match (name, wt) {
        ("R", _) => RED,
        ("G", _) => GREEN,
        ("B", _) | ("Cb", _) | ("C", _) => BLUE,
        ("Cr", _) => [1.0, 0.45, 0.45],
        ("Y", Some(WaveformType::Luma)) => WHITE,
        ("Y", Some(_)) => LUMA,
        _ => WHITE,
    }
}

/// Paint Mode Green: samples at exactly 0 or 255 (the rows of 0.0 and 1.0) are painted red.
fn highlight_extremes(px: &mut [u8], grids: &[&sc::Grid], (lo, hi): (f32, f32)) {
    let Some(first) = grids.first() else { return };
    let (w, rows) = (first.cols, first.rows);
    for v in [0.0f32, 1.0] {
        let Some(row) = sc::row_of(v, lo, hi, rows) else { continue };
        let y = rows - 1 - row;
        for x in 0..w {
            if grids.iter().any(|g| g.cols == w && g.rows == rows && g.at(x, row) > 0) {
                let i = (y * w + x) * 4;
                if let Some(o) = px.get_mut(i..i + 4) {
                    o.copy_from_slice(&[255, 40, 40, 255]);
                }
            }
        }
    }
}

fn draw_one(app: &mut FilmcraftApp, ui: &mut egui::Ui, cell: Rect, k: ScopeKind, sf: &ScopeFrame, st: &ScopesState, prefix: &str) {
    let ctx = ui.ctx().clone();
    let rows = ((cell.height() - 20.0) * ctx.pixels_per_point()).clamp(64.0, 256.0) as usize;
    let key = hash_of((sf.key, settings_key(st), k, rows));
    let slot = format!("{prefix}-{}", k.name());
    let gain = st.brightness.gain();
    let p = params(st, sf, rows);
    match k {
        ScopeKind::Waveform | ScopeKind::Parade => {
            let plot = Rect::from_min_max(cell.min + vec2(34.0, 10.0), cell.max - vec2(34.0, 10.0));
            if plot.width() < 8.0 || plot.height() < 8.0 {
                return;
            }
            let mode = st.paint_mode;
            let tex = cached_texture(&ctx, slot, key, || {
                let sig = plotted(st, sf);
                if k == ScopeKind::Waveform {
                    let w = sc::waveform(&sig, st.waveform_type, &p);
                    let layers: Vec<(&sc::Grid, [f32; 3])> = w.traces.iter().map(|g| (g, trace_color(g.name, Some(st.waveform_type), mode))).collect();
                    let (iw, ih, mut px) = sc::paint::grids(&layers, sc::paint::waveform_reference(w.per_column), gain);
                    if mode == PaintMode::Green {
                        highlight_extremes(&mut px, &w.traces.iter().collect::<Vec<_>>(), (w.lo, w.hi));
                    }
                    (iw, ih, px)
                } else {
                    let w = sc::parade(&sig, st.parade_type, &p);
                    parade_image(&w, gain, mode)
                }
            });
            level_graticule(ui, plot, st, sf, p.range());
            if let Some(tex) = tex {
                ui.painter().image(tex.id(), plot, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            if k == ScopeKind::Parade {
                let n = match st.parade_type {
                    ParadeType::RgbWhite => 4,
                    _ => 3,
                };
                for i in 1..n {
                    let x = plot.min.x + plot.width() * i as f32 / n as f32;
                    ui.painter().line_segment([pos2(x, plot.min.y), pos2(x, plot.max.y)], Stroke::new(1.0, Color32::from_gray(40)));
                }
                // Gradient reference line: a black-to-white ramp's trace in each channel
                if st.gradient_line {
                    let (lo, hi) = p.range();
                    let y_of = |v: f32| plot.max.y - (v - lo) / (hi - lo) * plot.height();
                    let sw = plot.width() / n as f32;
                    for i in 0..n {
                        let x0 = plot.min.x + sw * i as f32;
                        ui.painter().line_segment(
                            [pos2(x0, y_of(0.0)), pos2(x0 + sw, y_of(1.0))],
                            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 220, 120, 150)),
                        );
                    }
                    app.auto.add(&format!("{prefix}.gradientLine"), plot, "Gradient reference line");
                }
            }
            hover_level(app, ui, plot, st, sf, p.range(), prefix);
            let title = match k {
                ScopeKind::Parade => tlf!("Parade ({kind})", kind = crate::i18n::t(st.parade_type.label())),
                _ => tlf!("Waveform ({kind})", kind = crate::i18n::t(st.waveform_type.label())),
            };
            ui.painter().text(plot.left_top() + vec2(4.0, 2.0), Align2::LEFT_TOP, title, Tokens::ui(9.0), Color32::from_gray(110));
            if nits_axis(st, sf) {
                app.auto.add(&format!("{prefix}.hdrWaveform"), plot, "HDR levels (cd/m²)");
            }
        }
        ScopeKind::Histogram => {
            let mut plot = Rect::from_min_max(cell.min + vec2(34.0, 10.0), cell.max - vec2(34.0, 10.0));
            if plot.width() < 8.0 || plot.height() < 8.0 {
                return;
            }
            // Unscaled: 256 screen pixels wide (a narrower cell still shrinks it)
            if st.hist_unscaled {
                plot.max.x = plot.min.x + plot.width().min(256.0 / ctx.pixels_per_point());
            }
            let comps = HistComps { y: st.hist_luma, sum: st.hist_sum, r: st.hist_red, g: st.hist_green, b: st.hist_blue };
            let hid = egui::Id::new(("scope-hist", slot.clone()));
            let hist = match ctx.data(|d| d.get_temp::<(u64, Arc<sc::Histogram>)>(hid)) {
                Some((hk, h)) if hk == key => h,
                _ => {
                    let h = Arc::new(sc::histogram(&plotted(st, sf), &p));
                    ctx.data_mut(|d| d.insert_temp(hid, (key, h.clone())));
                    h
                }
            };
            let tex = cached_texture(&ctx, slot, key, || histogram_image(&hist, gain, comps));
            level_graticule(ui, plot, st, sf, (0.0, 1.0));
            if let Some(tex) = tex {
                ui.painter().image(tex.id(), plot, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            ui.painter().text(plot.left_top() + vec2(4.0, 2.0), Align2::LEFT_TOP, tl!("Histogram"), Tokens::ui(9.0), Color32::from_gray(110));
            // min / max of each component shown (code values, or 0–1 on the Float scale)
            let sum = hist.sum();
            let rows: [(bool, &str, &[u32], [f32; 3]); 5] =
                [(comps.y, "Y", &hist.y, WHITE), (comps.sum, "Sum", &sum, SUM), (comps.r, "R", &hist.r, RED), (comps.g, "G", &hist.g, GREEN), (comps.b, "B", &hist.b, BLUE)];
            let mut y = plot.min.y + 2.0;
            let mut readout = Vec::new();
            for (on, name, bins, col) in rows {
                if !on {
                    continue;
                }
                let text = match sc::min_max(bins) {
                    Some((lo, hi)) if st.scale == Scale::Float => format!("{name}  min {:.2}  max {:.2}", lo as f32 / 255.0, hi as f32 / 255.0),
                    Some((lo, hi)) => format!("{name}  min {lo}  max {hi}"),
                    None => format!("{name}  —"),
                };
                let c = Color32::from_rgb((col[0] * 230.0) as u8, (col[1] * 230.0) as u8, (col[2] * 230.0) as u8);
                ui.painter().text(pos2(plot.max.x - 4.0, y), Align2::RIGHT_TOP, &text, Tokens::ui(9.0), c);
                readout.push(text);
                y += 12.0;
            }
            app.auto.add(&format!("{prefix}.histogram.minMax"), plot, &readout.join("; "));
            if nits_axis(st, sf) {
                app.auto.add(&format!("{prefix}.hdrWaveform"), plot, "HDR levels (cd/m²)");
            }
        }
        ScopeKind::VectorscopeYuv | ScopeKind::VectorscopeHls => {
            let side = (cell.width().min(cell.height()) - 16.0).max(8.0);
            let sq = Rect::from_center_size(cell.center(), vec2(side, side));
            let hls = k == ScopeKind::VectorscopeHls;
            let tex = cached_texture(&ctx, slot, key, || {
                let v = if hls { sc::vectorscope_hls(&sf.signal, &p) } else { sc::vectorscope_yuv(&sf.signal, &p) };
                sc::paint::vectorscope(&v, WHITE, gain, true)
            });
            vector_graticule(ui, sq, &p, st, hls);
            if let Some(tex) = tex {
                ui.painter().image(tex.id(), sq, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            ui.painter().text(cell.left_top() + vec2(6.0, 4.0), Align2::LEFT_TOP, k.label(), Tokens::ui(9.0), Color32::from_gray(110));
            let model = if hls { String::new() } else if st.analog_yuv { "YUV".into() } else { "YPbPr".into() };
            let info = if st.vector_gain > 1.0 { format!("{model}  {}×", st.vector_gain) } else { model };
            if !info.is_empty() {
                ui.painter().text(cell.right_top() + vec2(-6.0, 4.0), Align2::RIGHT_TOP, &info, Tokens::ui(9.0), Color32::from_gray(110));
                app.auto.add(&format!("{prefix}.vectorscope.mode"), cell, &info);
            }
        }
    }
}

/// The parade's traces side by side in one image.
fn parade_image(w: &sc::Waveform, gain: f32, mode: PaintMode) -> (usize, usize, Vec<u8>) {
    let n = w.traces.len();
    let Some(first) = w.traces.first() else { return (0, 0, Vec::new()) };
    let (cw, rows) = (first.cols, first.rows);
    let gap = (cw / 24).max(2);
    let width = cw * n + gap * (n - 1);
    let mut out = vec![0u8; width * rows * 4];
    for (i, g) in w.traces.iter().enumerate() {
        let (_, _, mut px) = sc::paint::grids(&[(g, trace_color(g.name, None, mode))], sc::paint::waveform_reference(w.per_column), gain);
        if mode == PaintMode::Green {
            highlight_extremes(&mut px, &[g], (w.lo, w.hi));
        }
        let x0 = i * (cw + gap);
        for y in 0..rows {
            let src = &px[y * cw * 4..(y + 1) * cw * 4];
            out[(y * width + x0) * 4..(y * width + x0 + cw) * 4].copy_from_slice(src);
        }
    }
    (width, rows, out)
}

/// The histogram components shown.
#[derive(Clone, Copy)]
struct HistComps {
    y: bool,
    sum: bool,
    r: bool,
    g: bool,
    b: bool,
}

const SUM: [f32; 3] = [0.5, 0.5, 0.5];

/// The histogram as an image: levels bottom to top, counts to the right; the shown components
/// (Sum grey underneath, R/G/B added, Y white) drawn over each other.
fn histogram_image(h: &sc::Histogram, gain: f32, c: HistComps) -> (usize, usize, Vec<u8>) {
    let (w, rows) = (256usize, sc::BINS);
    let sum = h.sum();
    let layers: Vec<(&[u32], [f32; 3], f32)> = [(c.sum, &sum[..], SUM, 0.6), (c.r, &h.r[..], RED, 0.75), (c.g, &h.g[..], GREEN, 0.75), (c.b, &h.b[..], BLUE, 0.75), (c.y, &h.y[..], WHITE, 0.55)]
        .into_iter()
        .filter(|l| l.0)
        .map(|l| (l.1, l.2, l.3))
        .collect();
    // each component scaled to its own peak, so Sum does not flatten the others
    let peaks: Vec<f32> = layers.iter().map(|l| l.0.iter().copied().max().unwrap_or(0).max(1) as f32).collect();
    let mut out = vec![0u8; w * rows * 4];
    for bin in 0..rows {
        let y = rows - 1 - bin;
        let lens: Vec<usize> = layers.iter().zip(&peaks).map(|(l, m)| ((l.0[bin] as f32 / m).sqrt() * w as f32).round() as usize).collect();
        for x in 0..w {
            let mut s = [0f32; 3];
            for ((_, col, k), len) in layers.iter().zip(&lens) {
                if x < *len {
                    for (a, v) in s.iter_mut().zip(col) {
                        *a += v * k;
                    }
                }
            }
            let s = s.map(|v| (v * gain).min(1.0));
            let a = s[0].max(s[1]).max(s[2]);
            if a > 0.0 {
                let o = &mut out[(y * w + x) * 4..(y * w + x) * 4 + 4];
                o.copy_from_slice(&[(s[0] * 255.0) as u8, (s[1] * 255.0) as u8, (s[2] * 255.0) as u8, (a * 255.0) as u8]);
            }
        }
    }
    (w, rows, out)
}

/// The pointer over a waveform / parade: a horizontal line at its level and the value there.
fn hover_level(app: &mut FilmcraftApp, ui: &egui::Ui, plot: Rect, st: &ScopesState, sf: &ScopeFrame, (lo, hi): (f32, f32), prefix: &str) {
    let Some(pos) = ui.ctx().pointer_hover_pos() else { return };
    if !plot.contains(pos) || !ui.rect_contains_pointer(plot) {
        return;
    }
    let v = lo + (plot.max.y - pos.y) / plot.height().max(1.0) * (hi - lo);
    let text = if nits_axis(st, sf) {
        format!("{:.0} cd/m²", sc::pq_to_nits(v))
    } else {
        match st.scale {
            Scale::Float => format!("{v:.3}"),
            _ => format!("{}", (v * 255.0).round() as i32),
        }
    };
    let painter = ui.painter();
    painter.line_segment([pos2(plot.min.x, pos.y), pos2(plot.max.x, pos.y)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 140)));
    let tr = painter.text(pos2(plot.min.x + 4.0, pos.y - 2.0), Align2::LEFT_BOTTOM, &text, Tokens::ui(10.0), Color32::WHITE);
    app.auto.add(&format!("{prefix}.hoverLevel"), tr, &text);
}

/// Horizontal graticule and labels of a levels scope (`range` = the value range of the plot).
fn level_graticule(ui: &egui::Ui, plot: Rect, st: &ScopesState, sf: &ScopeFrame, (lo, hi): (f32, f32)) {
    let painter = ui.painter();
    let y_of = |v: f32| plot.max.y - (v - lo) / (hi - lo) * plot.height();
    let font = Tokens::ui(10.0);
    if nits_axis(st, sf) {
        for (nits, label) in [(0.0, "0"), (10.0, "10"), (100.0, "100"), (203.0, "203"), (1000.0, "1000"), (4000.0, "4000"), (10_000.0, "10000")] {
            let y = y_of(sc::nits_to_pq(nits));
            let col = if nits == 203.0 { Color32::from_rgba_unmultiplied(255, 200, 80, 90) } else { GRATICULE };
            painter.line_segment([pos2(plot.min.x, y), pos2(plot.max.x, y)], Stroke::new(1.0, col));
            painter.text(pos2(plot.min.x - 4.0, y), Align2::RIGHT_CENTER, label, font.clone(), LABEL);
        }
        painter.text(pos2(plot.max.x + 4.0, plot.min.y), Align2::LEFT_TOP, "cd/m²", Tokens::ui(9.0), LABEL);
        return;
    }
    for i in 0..=10 {
        let v = i as f32 / 10.0;
        let y = y_of(v);
        painter.line_segment([pos2(plot.min.x, y), pos2(plot.max.x, y)], Stroke::new(1.0, GRATICULE));
        painter.text(pos2(plot.min.x - 4.0, y), Align2::RIGHT_CENTER, format!("{}", i * 10), font.clone(), LABEL);
        let right = match st.scale {
            Scale::Float => format!("{v:.1}"),
            _ => format!("{}", (v * 255.0).round() as i32),
        };
        painter.text(pos2(plot.max.x + 4.0, y), Align2::LEFT_CENTER, right, font.clone(), LABEL);
    }
}

/// Circles, axes, colour targets (YUV) or hue labels (HLS) and the skin-tone line.
fn vector_graticule(ui: &egui::Ui, sq: Rect, p: &Params, st: &ScopesState, hls: bool) {
    let targets = st.targets;
    let painter = ui.painter();
    let c = sq.center();
    let extent = if hls { sc::VECTOR_EXTENT } else { p.vector_extent() };
    let scale = sq.width() / 2.0 / extent;
    let at = |cb: f32, cr: f32| pos2(c.x + cb * scale, c.y - cr * scale);
    let ring = 0.5 * scale;
    painter.circle_stroke(c, ring, Stroke::new(1.0, Color32::from_gray(64)));
    painter.circle_stroke(c, ring * 0.5, Stroke::new(1.0, GRATICULE));
    painter.line_segment([pos2(c.x - ring, c.y), pos2(c.x + ring, c.y)], Stroke::new(1.0, GRATICULE));
    painter.line_segment([pos2(c.x, c.y - ring), pos2(c.x, c.y + ring)], Stroke::new(1.0, GRATICULE));
    // 10° ticks on the outer ring
    for d in (0..360).step_by(10) {
        let a = (d as f32).to_radians();
        let (ix, iy) = (a.cos(), a.sin());
        let l = if d % 30 == 0 { 0.94 } else { 0.97 };
        painter
            .line_segment([pos2(c.x + ix * ring * l, c.y - iy * ring * l), pos2(c.x + ix * ring, c.y - iy * ring)], Stroke::new(1.0, Color32::from_gray(64)));
    }
    let m = p.matrix;
    if hls {
        for (name, cb, cr) in sc::hls_targets(m) {
            let pt = at(cb * 1.12, cr * 1.12);
            painter.text(pt, Align2::CENTER_CENTER, name, Tokens::ui(10.0), LABEL);
        }
        return;
    }
    // I and Q lines (I = the skin-tone line, Q at right angles to it), through the centre
    if st.iq_lines {
        for (deg, name, col) in [
            (sc::SKIN_TONE_DEG, "I", Color32::from_rgba_unmultiplied(220, 170, 120, 110)),
            (sc::SKIN_TONE_DEG - 90.0, "Q", Color32::from_rgba_unmultiplied(150, 120, 220, 110)),
        ] {
            let a = deg.to_radians();
            let d = vec2(a.cos() * ring, -a.sin() * ring);
            painter.line_segment([c - d, c + d], Stroke::new(1.0, col));
            painter.text(c + d * 1.06, Align2::CENTER_CENTER, name, Tokens::ui(9.0), col);
        }
    }
    let inside = |q: egui::Pos2| sq.expand(2.0).contains(q);
    for (amp, strong) in [(0.75, targets == Targets::Percent75), (1.0, targets == Targets::Percent100)] {
        for tg in sc::targets(m, amp) {
            let [tx, ty] = p.vector_point(tg.cb, tg.cr);
            let pt = at(tx, ty);
            if !inside(pt) {
                continue;
            }
            let col = Color32::from_rgb((tg.rgb[0] / amp * 200.0) as u8 + 40, (tg.rgb[1] / amp * 200.0) as u8 + 40, (tg.rgb[2] / amp * 200.0) as u8 + 40);
            let size = if strong { 9.0 } else { 5.0 };
            let col = if strong { col } else { col.gamma_multiply(0.45) };
            painter.rect_stroke(Rect::from_center_size(pt, vec2(size, size)), 0.0, Stroke::new(1.0, col), egui::StrokeKind::Middle);
            if strong {
                let off = egui::Vec2::new(tx, -ty).normalized() * 12.0;
                painter.text(pt + off, Align2::CENTER_CENTER, tg.name, Tokens::ui(10.0), col);
            }
        }
    }
}
