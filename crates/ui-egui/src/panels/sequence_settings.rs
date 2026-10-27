//! Sequence ▸ Sequence Settings…, laid out as Premiere Pro's dialog: General, Color Management and
//! VR Properties tabs, Cancel / OK.
//!
//! File ▸ New ▸ Sequence… opens the same dialog as New Sequence ([`open_new`]): the General tab
//! starting from the default settings, and a Tracks tab with the number of video and audio
//! tracks (`tracks.video`, `tracks.audio`); OK runs `file.newSequence`.
//!
//! What it changes (one `sequence.settings` command, one undo step): Timebase, Frame Size (with
//! "Scale motion effects proportionally when changing frame size", on by default), drop-frame or
//! non-drop-frame timecode for the NTSC rates, audio Channel Format and Sample Rate, Maximum Render
//! Quality, and the Color Management tab (working colour space, wide gamut, auto tone map).
//!
//! Premiere settings Redrob Film has no equivalent for yet are shown greyed out with their current
//! value and a "not supported yet" hint, rather than editable and silently ignored: Editing Mode,
//! Pixel Aspect Ratio (rendered, and set by New Sequence From Clip, but not editable here yet), Fields, the Frames and Feet + Frames display
//! formats, Number of Channels, audio Display Format, the Video Previews format, codec and size,
//! Maximum Bit Depth, and the VR Properties tab. Composite in Linear Color is shown on and greyed:
//! Redrob Film always composites in linear light.
//!
//! Automation ids (`sequenceSettings.` + …): `tab.general`, `tab.color`, `tab.vr`; General:
//! `name`, `editingMode`, `timebase` (+ `timebase.option.<num>/<den>` while open), `width`, `height`,
//! `aspect`, `scaleMotion`, `par`, `fields`, `videoDisplay` (+ `videoDisplay.option.<df|ndf|tc>`),
//! `channelFormat` (+ `channelFormat.option.<Stereo|Mono|5.1|Adaptive>`), `channels`, `sampleRate`
//! (+ `sampleRate.option.<hz>`), `audioDisplay`, `previewFormat`, `previewCodec`, `previewWidth`,
//! `previewHeight`, `maxBitDepth`, `maxRenderQuality`, `linearColor`; Color Management:
//! `workingSpace` (+ `workingSpace.option.<id>`), `wideGamut`, `autoToneMap`; VR Properties:
//! `vr.projection`, `vr.layout`, `vr.horizontal`, `vr.vertical`; `ok`, `cancel`.

use redrob_film_color::WorkingSpace;
use redrob_film_project::{AudioChannels, SequenceSettings};
use redrob_film_time::FrameRate;
use serde_json::{Map, Value, json};

use crate::FilmApp;
use crate::i18n::t;
use crate::state::SequenceSettingsDraft;

const NOT_YET: &str = "Not supported in Redrob Film yet";
const LINEAR: &str = "Redrob Film always composites in linear light";

/// The Timebase list (frames per second as num/den), Premiere's common rates.
const TIMEBASES: [(i64, i64); 15] = [
    (10, 1),
    (12, 1),
    (25, 2),
    (15, 1),
    (24_000, 1001),
    (24, 1),
    (25, 1),
    (30_000, 1001),
    (30, 1),
    (48, 1),
    (50, 1),
    (60_000, 1001),
    (60, 1),
    (120_000, 1001),
    (120, 1),
];
const SAMPLE_RATES: [u32; 5] = [32_000, 44_100, 48_000, 88_200, 96_000];
const MIXES: [&str; 4] = ["Stereo", "Mono", "5.1", "Adaptive"];

fn mix_name(c: AudioChannels) -> &'static str {
    match c {
        AudioChannels::Mono => "Mono",
        AudioChannels::Stereo => "Stereo",
        AudioChannels::Surround51 => "5.1",
        AudioChannels::Adaptive => "Adaptive",
    }
}

/// Fill the draft from the active sequence and open the dialog on the General tab.
pub fn open(app: &mut FilmApp) {
    let Some(q) = app.session.active_sequence() else { return };
    let st = &q.settings;
    let name = app.session.state.active_sequence.and_then(|id| app.session.project.item(id)).map(|i| i.name.clone()).unwrap_or_default();
    app.ui.sequence_settings = SequenceSettingsDraft {
        tab: "general".into(),
        name,
        new_sequence: false,
        video_tracks: 3,
        audio_tracks: 3,
        fps_num: st.frame_rate.num,
        fps_den: st.frame_rate.den,
        width: st.width,
        height: st.height,
        scale_motion: true,
        drop_frame: st.drop_frame && st.frame_rate.supports_drop_frame(),
        mix: mix_name(st.audio_master).into(),
        sample_rate: st.sample_rate,
        max_render_quality: st.max_render_quality,
        working_space: st.color.working.id().into(),
        wide_gamut: st.color.wide_gamut,
        auto_tone_map: st.color.auto_tone_map,
        preset_filter: String::new(),
        preset_name: String::new(),
        preview_codec: st.preview_codec.clone(),
    };
    app.dialog = Some(crate::Dialog::SequenceSettings);
}

/// File ▸ New ▸ Sequence…: the dialog as New Sequence, from the default settings, named like
/// `file.newSequence` would name it, with three video and three audio tracks.
pub fn open_new(app: &mut FilmApp) {
    let st = SequenceSettings::default();
    let n = app.session.project.sequences().count() + 1;
    app.ui.sequence_settings = SequenceSettingsDraft {
        tab: "general".into(),
        new_sequence: true,
        video_tracks: 3,
        audio_tracks: 3,
        fps_num: st.frame_rate.num,
        fps_den: st.frame_rate.den,
        width: st.width,
        height: st.height,
        scale_motion: true,
        drop_frame: st.drop_frame && st.frame_rate.supports_drop_frame(),
        mix: mix_name(st.audio_master).into(),
        sample_rate: st.sample_rate,
        max_render_quality: st.max_render_quality,
        working_space: st.color.working.id().into(),
        wide_gamut: st.color.wide_gamut,
        auto_tone_map: st.color.auto_tone_map,
        preview_codec: st.preview_codec.clone(),
        name: format!("Sequence {n:02}"),
        preset_filter: String::new(),
        preset_name: String::new(),
    };
    // the default sequence preset (Sequence Presets ▸ Set as Default), when one is chosen
    if let Some(pr) = redrob_film_engine::sequence_presets::default_preset(&app.session.prefs) {
        apply_preset(&mut app.ui.sequence_settings, &pr);
    }
    app.dialog = Some(crate::Dialog::SequenceSettings);
}

/// Put a preset's settings into the dialog's draft (the name of the sequence stays).
fn apply_preset(d: &mut SequenceSettingsDraft, pr: &redrob_film_engine::sequence_presets::SequencePreset) {
    d.width = pr.width;
    d.height = pr.height;
    d.fps_num = pr.fps_num;
    d.fps_den = pr.fps_den;
    d.drop_frame = pr.drop_frame && pr.rate().supports_drop_frame();
    d.sample_rate = pr.sample_rate;
    d.mix = pr.mix.clone();
    if d.new_sequence {
        d.video_tracks = pr.video_tracks;
        d.audio_tracks = pr.audio_tracks;
    }
    d.preset_name = pr.name.clone();
}

/// What the Sequence Presets tab asks for once the dialog is drawn.
enum PresetAction {
    Save(String),
    Delete(String),
    SetDefault(String),
}

/// The Sequence Presets tab: the presets (built-in, then the user's) narrowed to a frame rate; a
/// click applies one to the other tabs. The current settings can be saved as a preset, a user
/// preset deleted, and one made the default of File ▸ New ▸ Sequence….
fn presets(
    ui: &mut egui::Ui,
    d: &mut SequenceSettingsDraft,
    list: &[(redrob_film_engine::sequence_presets::SequencePreset, bool)],
    default: &str,
    elems: &mut Elems,
    action: &mut Option<PresetAction>,
) {
    ui.horizontal(|ui| {
        ui.label(tl!("Frame rate:"));
        let shown = if d.preset_filter.is_empty() {
            tl!("All frame rates").to_string()
        } else {
            d.preset_filter.split_once('/').and_then(|(n, dd)| Some(FrameRate { num: n.parse().ok()?, den: dd.parse().ok()? }.label())).unwrap_or_default()
        };
        let resp = egui::ComboBox::from_id_salt("seq-presets-filter").selected_text(&shown).width(160.0).show_ui(ui, |ui| {
            let o = ui.selectable_value(&mut d.preset_filter, String::new(), tl!("All frame rates"));
            elems.push(("sequenceSettings.presets.filter.all".into(), o.rect, "All".into()));
            let mut rates: Vec<FrameRate> = Vec::new();
            for (p, _) in list {
                if !rates.contains(&p.rate()) {
                    rates.push(p.rate());
                }
            }
            rates.sort_by(|a, b| a.as_f64().total_cmp(&b.as_f64()));
            for r in rates {
                let key = format!("{}/{}", r.num, r.den);
                let o = ui.selectable_value(&mut d.preset_filter, key.clone(), r.label());
                elems.push((format!("sequenceSettings.presets.filter.{key}"), o.rect, r.label()));
            }
        });
        elems.push(("sequenceSettings.presets.filter".into(), resp.response.rect, shown));
    });
    ui.add_space(4.0);
    egui::ScrollArea::vertical().id_salt("seq-presets-list").max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
        for (i, (p, user)) in list.iter().enumerate() {
            let key = format!("{}/{}", p.rate().num, p.rate().den);
            if !d.preset_filter.is_empty() && d.preset_filter != key {
                continue;
            }
            let mut label = format!("{}  —  {} x {}, {} fps, {} Hz {}", p.name, p.width, p.height, p.rate().label(), p.sample_rate, t(&p.mix));
            if *user {
                label.push_str(&format!("  ({})", tl!("custom")));
            }
            if p.name == default {
                label.push_str(&format!("  ★ {}", tl!("default")));
            }
            let o = ui.selectable_label(d.preset_name == p.name, label);
            elems.push((format!("sequenceSettings.presets.item.{i}"), o.rect, p.name.clone()));
            if o.clicked() {
                apply_preset(d, p);
            }
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(tl!("Preset name:"));
        let r = ui.add(egui::TextEdit::singleline(&mut d.preset_name).desired_width(220.0));
        elems.push(("sequenceSettings.presets.name".into(), r.rect, d.preset_name.clone()));
        let name = d.preset_name.trim().to_string();
        let builtin = list.iter().any(|(p, u)| !*u && p.name == name);
        let r = ui
            .add_enabled(!name.is_empty() && !builtin, egui::Button::new(tl!("Save Preset")))
            .on_hover_text(tl!("Save the settings of the General and Tracks tabs as a preset"));
        elems.push(("sequenceSettings.presets.save".into(), r.rect, "Save Preset".into()));
        if r.clicked() {
            *action = Some(PresetAction::Save(name.clone()));
        }
        let own = list.iter().any(|(p, u)| *u && p.name == name);
        let r = ui.add_enabled(own, egui::Button::new(tl!("Delete")));
        elems.push(("sequenceSettings.presets.delete".into(), r.rect, "Delete".into()));
        if r.clicked() {
            *action = Some(PresetAction::Delete(name.clone()));
        }
        let known = list.iter().any(|(p, _)| p.name == name);
        let is_default = default == name;
        let r = ui
            .add_enabled(known || is_default || !default.is_empty(), egui::Button::new(if is_default { tl!("Clear Default") } else { tl!("Set as Default") }));
        elems.push(("sequenceSettings.presets.default".into(), r.rect, "Set as Default".into()));
        if r.clicked() {
            *action = Some(PresetAction::SetDefault(if is_default || !known { String::new() } else { name }));
        }
    });
}

/// The `file.newSequence` parameters for a New Sequence draft. Settings `file.newSequence` doesn't
/// take (drop-frame timecode, Maximum Render Quality) are the second map, for `sequence.settings`
/// on the new sequence; empty when they are the defaults.
fn new_sequence_params(d: &SequenceSettingsDraft) -> (Map<String, Value>, Map<String, Value>) {
    let mut p = Map::new();
    let name = d.name.trim();
    if !name.is_empty() {
        p.insert("name".into(), json!(name));
    }
    p.insert("width".into(), json!(d.width));
    p.insert("height".into(), json!(d.height));
    p.insert("fps".into(), json!(rate(d).as_f64()));
    p.insert("sampleRate".into(), json!(d.sample_rate));
    p.insert("mix".into(), json!(d.mix));
    p.insert("video".into(), json!(d.video_tracks.min(256)));
    p.insert("audio".into(), json!(d.audio_tracks.min(256)));
    // what's left: only the settings `file.newSequence` has no parameter for
    let after = SequenceSettings { frame_rate: rate(d), width: d.width, height: d.height, sample_rate: d.sample_rate, ..Default::default() };
    let mut rest = changes(d, &after, name);
    rest.retain(|k, _| matches!(k.as_str(), "dropFrame" | "maxRenderQuality"));
    (p, rest)
}

/// The Tracks tab of New Sequence: how many video and audio tracks the sequence starts with.
fn tracks(ui: &mut egui::Ui, d: &mut SequenceSettingsDraft, elems: &mut Elems) {
    for (id, label, n) in [("video", "Video:", &mut d.video_tracks), ("audio", "Audio:", &mut d.audio_tracks)] {
        ui.horizontal(|ui| {
            ui.label(t(label));
            let r = ui.add(egui::DragValue::new(n).range(0..=256));
            elems.push((format!("sequenceSettings.tracks.{id}"), r.rect, n.to_string()));
            ui.label(tl!("tracks"));
        });
    }
}

fn rate(d: &SequenceSettingsDraft) -> FrameRate {
    FrameRate { num: d.fps_num, den: d.fps_den }.sane()
}

/// "16:9" for a frame size, or "1.90:1" when the reduced ratio has no small integer form.
fn aspect(w: u32, h: u32) -> String {
    let g = gcd(w, h).max(1);
    let (a, b) = (w / g, h / g);
    if a <= 64 && b <= 64 {
        format!("{a}:{b}")
    } else if h > 0 {
        format!("{:.2}:1", f64::from(w) / f64::from(h))
    } else {
        String::new()
    }
}

/// The width a `w` x `h` frame with `par` pixels shows at, in square pixels (rounded).
fn display_width(w: u32, h: u32, par: (u32, u32)) -> u32 {
    let dw = redrob_film_project::conformed_size((w, h), par, (1, 1)).0.round();
    if dw >= f64::from(u32::MAX) { u32::MAX } else { dw.max(0.0) as u32 }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn timebase_label(r: FrameRate) -> String {
    tlf!("{rate} frames/second", rate = r.label())
}

/// The Video Display Format choices for a timebase: drop-frame / non-drop-frame timecode for the
/// NTSC rates, plain timecode otherwise (`(id, label)`).
fn timecode_formats(r: FrameRate) -> Vec<(&'static str, String)> {
    if r.supports_drop_frame() {
        vec![("df", tlf!("{rate} fps Drop-Frame Timecode", rate = r.label())), ("ndf", tlf!("{rate} fps Non Drop-Frame Timecode", rate = r.label()))]
    } else {
        vec![("tc", tlf!("{rate} fps Timecode", rate = r.label()))]
    }
}

fn par_label(par: (u32, u32)) -> String {
    if par.0 == par.1 {
        tl!("Square Pixels (1.0)").into()
    } else if par.1 > 0 {
        tlf!("Custom ({ratio})", ratio = format!("{:.4}", f64::from(par.0) / f64::from(par.1)))
    } else {
        tl!("Custom").into()
    }
}

/// The parameters for `sequence.settings` that differ from `cur` (and the sequence's name
/// `cur_name`); empty when nothing changed. A blank name keeps the current one.
fn changes(d: &SequenceSettingsDraft, cur: &SequenceSettings, cur_name: &str) -> Map<String, Value> {
    let mut p = Map::new();
    let name = d.name.trim();
    if !name.is_empty() && name != cur_name {
        p.insert("name".into(), json!(name));
    }
    let r = rate(d);
    if r != cur.frame_rate {
        p.insert("fps".into(), json!(r.as_f64()));
    }
    if (d.width, d.height) != (cur.width, cur.height) {
        p.insert("width".into(), json!(d.width));
        p.insert("height".into(), json!(d.height));
        p.insert("scaleMotion".into(), json!(d.scale_motion));
    }
    let df = d.drop_frame && r.supports_drop_frame();
    if df != (cur.drop_frame && cur.frame_rate.supports_drop_frame()) {
        p.insert("dropFrame".into(), json!(df));
    }
    if d.mix != mix_name(cur.audio_master) {
        p.insert("mix".into(), json!(d.mix));
    }
    if d.sample_rate != cur.sample_rate {
        p.insert("sampleRate".into(), json!(d.sample_rate));
    }
    if d.max_render_quality != cur.max_render_quality {
        p.insert("maxRenderQuality".into(), json!(d.max_render_quality));
    }
    if d.preview_codec != cur.preview_codec {
        p.insert("previewCodec".into(), json!(d.preview_codec));
    }
    if d.working_space != cur.color.working.id() {
        p.insert("workingSpace".into(), json!(d.working_space));
    }
    if d.wide_gamut != cur.color.wide_gamut {
        p.insert("wideGamut".into(), json!(d.wide_gamut));
    }
    if d.auto_tone_map != cur.color.auto_tone_map {
        p.insert("autoToneMap".into(), json!(d.auto_tone_map));
    }
    p
}

type Elems = Vec<(String, egui::Rect, String)>;

/// One labelled row: the label right-aligned in a `label_w` column, then the control.
fn row<R>(ui: &mut egui::Ui, label_w: f32, label: &str, enabled: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let h = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(egui::vec2(label_w, h), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled(enabled, egui::Label::new(t(label)));
        });
        add(ui)
    })
    .inner
}

/// A greyed-out list showing `value`, with the "not supported yet" hint (or `hint`).
fn fixed_combo(ui: &mut egui::Ui, elems: &mut Elems, id: &str, value: &str, width: f32, hint: &str) {
    let r = ui
        .add_enabled_ui(false, |ui| egui::ComboBox::from_id_salt(("seq-settings-fixed", id)).selected_text(t(value)).width(width).show_ui(ui, |_| {}).response)
        .inner
        .on_disabled_hover_text(t(hint));
    elems.push((format!("sequenceSettings.{id}"), r.rect, value.to_string()));
}

/// A greyed-out read-only value (a number field in Premiere), with the hint.
fn fixed_value(ui: &mut egui::Ui, elems: &mut Elems, id: &str, value: &str) {
    let r = ui.add_enabled(false, egui::Button::new(value).min_size(egui::vec2(64.0, 0.0))).on_disabled_hover_text(t(NOT_YET));
    elems.push((format!("sequenceSettings.{id}"), r.rect, value.to_string()));
}

fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(t(title)).strong());
    ui.group(|ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

fn general(ui: &mut egui::Ui, d: &mut SequenceSettingsDraft, cur: &SequenceSettings, audio_samples: bool, elems: &mut Elems) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let label_w = ["Preview File Format:", "Pixel Aspect Ratio:", "Number of Channels:"]
        .iter()
        .map(|l| ui.painter().layout_no_wrap(t(l).to_string(), font.clone(), egui::Color32::WHITE).size().x)
        .fold(0.0, f32::max);
    let list_w = 260.0;
    row(ui, label_w, "Sequence Name:", true, |ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(list_w));
        elems.push(("sequenceSettings.name".into(), r.rect, d.name.clone()));
    });
    row(ui, label_w, "Editing Mode:", false, |ui| fixed_combo(ui, elems, "editingMode", "Custom", list_w, NOT_YET));
    row(ui, label_w, "Timebase:", true, |ui| {
        let r = rate(d);
        let mut choices: Vec<(i64, i64)> = TIMEBASES.to_vec();
        if !choices.contains(&(r.num, r.den)) {
            choices.insert(0, (r.num, r.den));
        }
        let shown = timebase_label(r);
        // no row limit of our own: the list grows to egui's popup limit (`Spacing::default_area_size`,
        // 400 px), so every rate up to 119.88 shows without scrolling (the default 200 px stopped
        // before 29.97)
        let resp = egui::ComboBox::from_id_salt("seq-settings-timebase").selected_text(&shown).width(list_w).height(f32::INFINITY).show_ui(ui, |ui| {
            for (num, den) in choices {
                let label = timebase_label(FrameRate { num, den });
                let o = ui.selectable_label((d.fps_num, d.fps_den) == (num, den), &label);
                // a new timebase starts on its usual timecode (drop-frame for the NTSC rates);
                // picking the current one again keeps the Display Format chosen for it
                if o.clicked() && (d.fps_num, d.fps_den) != (num, den) {
                    d.fps_num = num;
                    d.fps_den = den;
                    d.drop_frame = FrameRate { num, den }.supports_drop_frame();
                }
                elems.push((format!("sequenceSettings.timebase.option.{num}/{den}"), o.rect, label));
            }
        });
        elems.push(("sequenceSettings.timebase".into(), resp.response.rect, shown));
    });
    section(ui, "Video", |ui| {
        row(ui, label_w, "Frame Size:", true, |ui| {
            let w = ui.add(egui::DragValue::new(&mut d.width).range(1..=redrob_film_project::MAX_FRAME_SIDE).speed(1.0));
            elems.push(("sequenceSettings.width".into(), w.rect, d.width.to_string()));
            ui.label(tl!("horizontal"));
            let h = ui.add(egui::DragValue::new(&mut d.height).range(1..=redrob_film_project::MAX_FRAME_SIDE).speed(1.0));
            elems.push(("sequenceSettings.height".into(), h.rect, d.height.to_string()));
            ui.label(tl!("vertical"));
            // the display aspect: 1440 x 1080 with 4:3 pixels is 16:9
            let a = aspect(display_width(d.width, d.height, cur.par), d.height);
            let ar = ui.label(&a);
            elems.push(("sequenceSettings.aspect".into(), ar.rect, a));
        });
        let r = ui.checkbox(&mut d.scale_motion, tl!("Scale motion effects proportionally when changing frame size"));
        elems.push(("sequenceSettings.scaleMotion".into(), r.rect, d.scale_motion.to_string()));
        row(ui, label_w, "Pixel Aspect Ratio:", false, |ui| fixed_combo(ui, elems, "par", &par_label(cur.par), list_w, NOT_YET));
        row(ui, label_w, "Fields:", false, |ui| fixed_combo(ui, elems, "fields", "No Fields (Progressive Scan)", list_w, NOT_YET));
        row(ui, label_w, "Display Format:", true, |ui| {
            let r = rate(d);
            let formats = timecode_formats(r);
            let chosen = if r.supports_drop_frame() { if d.drop_frame { "df" } else { "ndf" } } else { "tc" };
            let shown = formats.iter().find(|(id, _)| *id == chosen).map(|(_, l)| l.clone()).unwrap_or_default();
            let resp = egui::ComboBox::from_id_salt("seq-settings-video-display").selected_text(&shown).width(list_w).show_ui(ui, |ui| {
                for (id, label) in &formats {
                    let o = ui.selectable_label(*id == chosen, label);
                    if o.clicked() {
                        d.drop_frame = *id == "df";
                    }
                    elems.push((format!("sequenceSettings.videoDisplay.option.{id}"), o.rect, label.clone()));
                }
                for label in ["Feet + Frames 16mm", "Feet + Frames 35mm", "Frames"] {
                    ui.add_enabled(false, egui::Button::selectable(false, t(label))).on_disabled_hover_text(t(NOT_YET));
                }
            });
            elems.push(("sequenceSettings.videoDisplay".into(), resp.response.rect, shown));
        });
    });
    section(ui, "Audio", |ui| {
        row(ui, label_w, "Channel Format:", true, |ui| {
            let shown = d.mix.clone();
            let resp = egui::ComboBox::from_id_salt("seq-settings-mix").selected_text(t(&shown)).width(120.0).show_ui(ui, |ui| {
                for m in MIXES {
                    let o = ui.selectable_value(&mut d.mix, m.to_string(), t(m));
                    elems.push((format!("sequenceSettings.channelFormat.option.{m}"), o.rect, m.to_string()));
                }
            });
            elems.push(("sequenceSettings.channelFormat".into(), resp.response.rect, shown));
            ui.add_enabled(false, egui::Label::new(tl!("Number of Channels:")));
            let n = match d.mix.as_str() {
                "Mono" => "1",
                "5.1" => "6",
                "Adaptive" => "—",
                _ => "2",
            };
            fixed_combo(ui, elems, "channels", n, 60.0, NOT_YET);
        });
        row(ui, label_w, "Sample Rate:", true, |ui| {
            let mut rates = SAMPLE_RATES.to_vec();
            if !rates.contains(&d.sample_rate) {
                rates.insert(0, d.sample_rate);
            }
            let shown = format!("{} Hz", d.sample_rate);
            let resp = egui::ComboBox::from_id_salt("seq-settings-sample-rate").selected_text(&shown).width(list_w).show_ui(ui, |ui| {
                for hz in rates {
                    let label = format!("{hz} Hz");
                    let o = ui.selectable_value(&mut d.sample_rate, hz, &label);
                    elems.push((format!("sequenceSettings.sampleRate.option.{hz}"), o.rect, label));
                }
            });
            elems.push(("sequenceSettings.sampleRate".into(), resp.response.rect, shown));
        });
        let shown = if audio_samples { "Audio Samples" } else { "Milliseconds" };
        row(ui, label_w, "Display Format:", false, |ui| fixed_combo(ui, elems, "audioDisplay", shown, list_w, NOT_YET));
    });
    section(ui, "Video Previews", |ui| {
        row(ui, label_w, "Preview File Format:", false, |ui| {
            fixed_combo(ui, elems, "previewFormat", "QuickTime", list_w, NOT_YET);
            ui.add_enabled(false, egui::Button::new(tl!("Configure…"))).on_disabled_hover_text(t(NOT_YET));
        });
        // Kdenlive Timeline Preview profile: the ProRes flavour of the preview files
        row(ui, label_w, "Codec:", false, |ui| {
            let resp = egui::ComboBox::from_id_salt("seq-settings-previewCodec").selected_text(t(&d.preview_codec)).width(list_w).show_ui(ui, |ui| {
                for (label, _) in redrob_film_engine::previews::PREVIEW_CODECS {
                    let o = ui.selectable_value(&mut d.preview_codec, label.to_string(), t(label));
                    elems.push((format!("sequenceSettings.previewCodec.option.{}", label.replace(' ', "")), o.rect, label.to_string()));
                }
            });
            elems.push(("sequenceSettings.previewCodec".into(), resp.response.rect, d.preview_codec.clone()));
        });
        row(ui, label_w, "Width:", false, |ui| fixed_value(ui, elems, "previewWidth", &d.width.to_string()));
        row(ui, label_w, "Height:", false, |ui| {
            fixed_value(ui, elems, "previewHeight", &d.height.to_string());
            ui.add_enabled(false, egui::Button::new(tl!("Reset"))).on_disabled_hover_text(t(NOT_YET));
        });
        ui.horizontal(|ui| {
            let mut bit_depth = cur.max_bit_depth;
            let r = ui.add_enabled(false, egui::Checkbox::new(&mut bit_depth, tl!("Maximum Bit Depth"))).on_disabled_hover_text(t(NOT_YET));
            elems.push(("sequenceSettings.maxBitDepth".into(), r.rect, bit_depth.to_string()));
            let r = ui.checkbox(&mut d.max_render_quality, tl!("Maximum Render Quality"));
            elems.push(("sequenceSettings.maxRenderQuality".into(), r.rect, d.max_render_quality.to_string()));
        });
        let mut linear = true;
        let r = ui.add_enabled(false, egui::Checkbox::new(&mut linear, tl!("Composite in Linear Color"))).on_disabled_hover_text(t(LINEAR));
        elems.push(("sequenceSettings.linearColor".into(), r.rect, "true".into()));
    });
}

fn color(ui: &mut egui::Ui, d: &mut SequenceSettingsDraft, elems: &mut Elems) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let label_w = ui.painter().layout_no_wrap(tl!("Working Color Space:").to_string(), font, egui::Color32::WHITE).size().x;
    row(ui, label_w, "Working Color Space:", true, |ui| {
        let shown = WorkingSpace::parse(&d.working_space).map_or(d.working_space.clone(), |w| t(w.label()).to_string());
        let resp = egui::ComboBox::from_id_salt("seq-settings-working-space").selected_text(&shown).width(220.0).show_ui(ui, |ui| {
            for w in WorkingSpace::ALL {
                let o = ui.selectable_value(&mut d.working_space, w.id().to_string(), t(w.label()));
                elems.push((format!("sequenceSettings.workingSpace.option.{}", w.id()), o.rect, w.label().to_string()));
            }
        });
        elems.push(("sequenceSettings.workingSpace".into(), resp.response.rect, shown));
    });
    ui.add_space(4.0);
    let r = ui.checkbox(&mut d.wide_gamut, tl!("Wide gamut color (composite in BT.2020)"));
    elems.push(("sequenceSettings.wideGamut".into(), r.rect, d.wide_gamut.to_string()));
    let r = ui.checkbox(&mut d.auto_tone_map, tl!("Auto Tone Map Media (HDR and log into SDR)"));
    elems.push(("sequenceSettings.autoToneMap".into(), r.rect, d.auto_tone_map.to_string()));
}

fn vr(ui: &mut egui::Ui, elems: &mut Elems) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let label_w = ui.painter().layout_no_wrap(tl!("Captured View:").to_string(), font, egui::Color32::WHITE).size().x;
    row(ui, label_w, "Projection:", false, |ui| fixed_combo(ui, elems, "vr.projection", "None", 200.0, NOT_YET));
    row(ui, label_w, "Layout:", false, |ui| fixed_combo(ui, elems, "vr.layout", "Monoscopic", 200.0, NOT_YET));
    row(ui, label_w, "Captured View:", false, |ui| {
        fixed_value(ui, elems, "vr.horizontal", "0°");
        ui.add_enabled(false, egui::Label::new(tl!("Horizontal")));
        fixed_value(ui, elems, "vr.vertical", "0°");
        ui.add_enabled(false, egui::Label::new(tl!("Vertical")));
    });
}

/// Draw the dialog. Returns whether it stays open.
pub fn show(app: &mut FilmApp, ctx: &egui::Context) -> bool {
    let new_sequence = app.ui.sequence_settings.new_sequence;
    // New Sequence compares against the defaults it started from; Sequence Settings against the
    // active sequence
    let (cur, cur_name) = if new_sequence {
        (SequenceSettings::default(), String::new())
    } else {
        let Some(q) = app.session.active_sequence() else { return false };
        let name = app.session.state.active_sequence.and_then(|id| app.session.project.item(id)).map(|i| i.name.clone()).unwrap_or_default();
        (q.settings.clone(), name)
    };
    let audio_samples = app.session.project.settings.audio_display_samples;
    let accent = app.tokens.accent;
    let mut d = app.ui.sequence_settings.clone();
    let mut keep = true;
    let mut apply = false;
    let mut elems: Elems = Vec::new();
    let max_h = ctx.content_rect().height() * 0.8;
    let title = if new_sequence { tl!("New Sequence") } else { tl!("Sequence Settings") };
    let tabs: &[(&str, &str)] = if new_sequence {
        &[("presets", "Sequence Presets"), ("general", "General"), ("tracks", "Tracks")]
    } else {
        &[("presets", "Sequence Presets"), ("general", "General"), ("color", "Color Management"), ("vr", "VR Properties")]
    };
    let preset_list = redrob_film_engine::sequence_presets::all(&app.session.prefs);
    let preset_default = app.session.prefs.sequence_presets.default.clone();
    let mut preset_action: Option<PresetAction> = None;
    egui::Window::new(title)
        .id(egui::Id::new("Sequence Settings"))
        .collapsible(false)
        .resizable(false)
        .default_width(620.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(620.0);
            ui.horizontal(|ui| {
                for &(id, label) in tabs {
                    let r = ui.selectable_label(d.tab == id, t(label));
                    if r.clicked() {
                        d.tab = id.to_string();
                    }
                    elems.push((format!("sequenceSettings.tab.{id}"), r.rect, label.to_string()));
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| match d.tab.as_str() {
                "color" if !new_sequence => color(ui, &mut d, &mut elems),
                "tracks" if new_sequence => tracks(ui, &mut d, &mut elems),
                "vr" if !new_sequence => vr(ui, &mut elems),
                "presets" => presets(ui, &mut d, &preset_list, &preset_default, &mut elems, &mut preset_action),
                _ => general(ui, &mut d, &cur, audio_samples, &mut elems),
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let o = ui.add(egui::Button::new(egui::RichText::new(tl!("OK")).color(egui::Color32::WHITE)).fill(accent));
                elems.push(("sequenceSettings.ok".into(), o.rect, "OK".into()));
                if o.clicked() {
                    apply = true;
                }
                let c = ui.button(tl!("Cancel"));
                elems.push(("sequenceSettings.cancel".into(), c.rect, "Cancel".into()));
                if c.clicked() {
                    keep = false;
                }
            });
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if let Some(a) = preset_action {
        let r = match a {
            PresetAction::Save(name) => app.session.execute(
                "sequencePresets.save",
                json!({"name": name, "width": d.width, "height": d.height, "fps": rate(&d).as_f64(), "dropFrame": d.drop_frame, "sampleRate": d.sample_rate, "mix": d.mix, "video": d.video_tracks, "audio": d.audio_tracks}),
            ),
            PresetAction::Delete(name) => app.session.execute("sequencePresets.delete", json!({"name": name})),
            PresetAction::SetDefault(name) => app.session.execute("sequencePresets.setDefault", json!({"name": name})),
        };
        if let Err(e) = r {
            app.ui.status = e.to_string();
        }
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        keep = false;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !ctx.egui_wants_keyboard_input() {
        apply = true;
    }
    if apply && new_sequence {
        let (p, rest) = new_sequence_params(&d);
        let r = app
            .session
            .execute("file.newSequence", Value::Object(p))
            .and_then(|_| if rest.is_empty() { Ok(Value::Null) } else { app.session.execute("sequence.settings", Value::Object(rest)) });
        if let Err(e) = r {
            app.ui.status = e.to_string();
        }
        app.ui.sequence_settings = d;
        return false;
    }
    if apply {
        let p = changes(&d, &cur, &cur_name);
        // nothing changed: OK closes the dialog and adds no undo step
        if !p.is_empty()
            && let Err(e) = app.session.execute("sequence.settings", Value::Object(p))
        {
            app.ui.status = e.to_string();
        }
        keep = false;
    }
    app.ui.sequence_settings = d;
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_labels() {
        assert_eq!(aspect(1920, 1080), "16:9");
        assert_eq!(aspect(640, 360), "16:9");
        assert_eq!(aspect(1080, 1920), "9:16");
        assert_eq!(aspect(1440, 1080), "4:3");
        assert_eq!(aspect(4096, 2160), "1.90:1");
        // non-square pixels: the display aspect
        assert_eq!(aspect(display_width(1440, 1080, (4, 3)), 1080), "16:9");
        assert_eq!(aspect(display_width(1280, 1080, (3, 2)), 1080), "16:9");
        assert_eq!(display_width(u32::MAX, 1, (8, 1)), u32::MAX);
        assert_eq!(display_width(1440, 1080, (0, 0)), 1440);
    }

    #[test]
    fn display_formats_follow_the_timebase() {
        assert_eq!(timecode_formats(FrameRate::FPS_23_976), vec![("tc", "23.976 fps Timecode".to_string())]);
        let ntsc = timecode_formats(FrameRate::FPS_29_97);
        assert_eq!(ntsc.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec!["df", "ndf"]);
        assert_eq!(ntsc[0].1, "29.97 fps Drop-Frame Timecode");
    }

    #[test]
    fn new_sequence_sends_its_settings_to_the_command() {
        let d = SequenceSettingsDraft {
            new_sequence: true,
            name: " Shorts ".into(),
            width: 1080,
            height: 1920,
            video_tracks: 2,
            audio_tracks: 4,
            ..Default::default()
        };
        let (p, rest) = new_sequence_params(&d);
        assert_eq!(
            Value::Object(p),
            json!({"name":"Shorts","width":1080,"height":1920,"fps":24_000.0/1001.0,"sampleRate":48_000,"mix":"Stereo","video":2,"audio":4})
        );
        assert!(rest.is_empty(), "everything else is the default: {rest:?}");
        // what file.newSequence has no parameter for follows on the new sequence; a blank name
        // lets the command name it
        let d = SequenceSettingsDraft {
            new_sequence: true,
            name: " ".into(),
            fps_num: 30_000,
            fps_den: 1001,
            drop_frame: true,
            max_render_quality: true,
            ..Default::default()
        };
        let (p, rest) = new_sequence_params(&d);
        assert!(p.get("name").is_none());
        assert_eq!(Value::Object(rest), json!({"dropFrame":true,"maxRenderQuality":true}));
    }

    #[test]
    fn only_changed_settings_are_sent() {
        let cur = SequenceSettings::default();
        let mut d = SequenceSettingsDraft::default();
        assert!(changes(&d, &cur, "").is_empty(), "the defaults match: nothing to send");
        d.width = 1280;
        d.height = 720;
        d.max_render_quality = true;
        let p = changes(&d, &cur, "");
        assert_eq!(Value::Object(p), json!({"width":1280,"height":720,"scaleMotion":true,"maxRenderQuality":true}));
        // drop-frame only travels for a rate that has it
        let mut d = SequenceSettingsDraft { drop_frame: true, ..Default::default() };
        assert!(changes(&d, &cur, "").is_empty());
        (d.fps_num, d.fps_den) = (30_000, 1001);
        assert_eq!(Value::Object(changes(&d, &cur, "")), json!({"fps":30_000.0/1001.0,"dropFrame":true}));
        // the name travels when it changed; blank or unchanged it doesn't
        let d = SequenceSettingsDraft { name: "  Rough Cut  ".into(), ..Default::default() };
        assert_eq!(Value::Object(changes(&d, &cur, "Sequence 01")), json!({"name":"Rough Cut"}));
        assert!(changes(&d, &cur, "Rough Cut").is_empty());
        let d = SequenceSettingsDraft { name: "   ".into(), ..Default::default() };
        assert!(changes(&d, &cur, "Sequence 01").is_empty());
    }
}
