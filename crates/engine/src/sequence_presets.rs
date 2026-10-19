//! Sequence presets: named sequence formats (frame size, frame rate, timecode, audio, tracks) the
//! New Sequence and Sequence Settings dialogs start from (Kdenlive's project profiles).
//!
//! The built-in presets are common delivery formats written out here; the user's own presets and
//! the default one (used by File ▸ New ▸ Sequence…) are preferences (`sequencePresets`), so they
//! are the same in every project.
//!
//! | Id | What |
//! |---|---|
//! | `sequencePresets.list {fps?, scan?}` | the presets, built-in first, optionally only those at a frame rate (`fps`) |
//! | `sequencePresets.save {name, width, height, fps, dropFrame?, sampleRate?, mix?, video?, audio?}` | add or replace a user preset |
//! | `sequencePresets.delete {name}` | remove a user preset |
//! | `sequencePresets.setDefault {name}` | the preset new sequences start from (`""`: the built-in default) |

use redrob_film_project::SequenceSettings;
use redrob_film_time::FrameRate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// One sequence preset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SequencePreset {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps_num: i64,
    pub fps_den: i64,
    pub drop_frame: bool,
    pub sample_rate: u32,
    /// `Stereo` | `Mono` | `5.1` | `Adaptive`.
    pub mix: String,
    pub video_tracks: u32,
    pub audio_tracks: u32,
}

impl Default for SequencePreset {
    fn default() -> Self {
        let st = SequenceSettings::default();
        Self {
            name: String::new(),
            width: st.width,
            height: st.height,
            fps_num: st.frame_rate.num,
            fps_den: st.frame_rate.den,
            drop_frame: st.drop_frame,
            sample_rate: st.sample_rate,
            mix: "Stereo".into(),
            video_tracks: 3,
            audio_tracks: 3,
        }
    }
}

impl SequencePreset {
    pub fn rate(&self) -> FrameRate {
        FrameRate { num: self.fps_num, den: self.fps_den }.sane()
    }
    fn to_json(&self, user: bool) -> Value {
        json!({
            "name": self.name, "width": self.width, "height": self.height, "fps": self.rate().as_f64(),
            "fpsNum": self.fps_num, "fpsDen": self.fps_den, "dropFrame": self.drop_frame, "sampleRate": self.sample_rate,
            "mix": self.mix, "video": self.video_tracks, "audio": self.audio_tracks, "user": user,
        })
    }
}

/// The user's presets and the default (Settings are not drawn for them: the sequence dialogs edit
/// them).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SequencePresetPrefs {
    pub user: Vec<SequencePreset>,
    /// Name of the preset File ▸ New ▸ Sequence… starts from; empty = the built-in default.
    pub default: String,
}

/// The built-in presets: common HD, UHD, DCI, vertical and square delivery formats.
pub fn builtin() -> Vec<SequencePreset> {
    let p = |name: &str, width: u32, height: u32, (fps_num, fps_den): (i64, i64)| {
        let rate = FrameRate { num: fps_num, den: fps_den };
        SequencePreset { name: name.into(), width, height, fps_num, fps_den, drop_frame: rate.supports_drop_frame(), ..SequencePreset::default() }
    };
    let rates = [("23.976", (24_000, 1001)), ("24", (24, 1)), ("25", (25, 1)), ("29.97", (30_000, 1001)), ("30", (30, 1)), ("50", (50, 1)), ("59.94", (60_000, 1001)), ("60", (60, 1))];
    let mut v = Vec::new();
    for (label, r) in rates {
        v.push(p(&format!("HD 1080p {label} fps"), 1920, 1080, r));
    }
    for (label, r) in rates {
        v.push(p(&format!("UHD 2160p {label} fps"), 3840, 2160, r));
    }
    for (label, r) in [("25", (25, 1)), ("29.97", (30_000, 1001)), ("50", (50, 1)), ("59.94", (60_000, 1001))] {
        v.push(p(&format!("HD 720p {label} fps"), 1280, 720, r));
    }
    for (label, r) in [("24", (24, 1)), ("25", (25, 1))] {
        v.push(p(&format!("DCI 4K {label} fps"), 4096, 2160, r));
        v.push(p(&format!("DCI 2K {label} fps"), 2048, 1080, r));
    }
    for (label, r) in [("25", (25, 1)), ("29.97", (30_000, 1001)), ("30", (30, 1)), ("60", (60, 1))] {
        v.push(p(&format!("Vertical 1080x1920 {label} fps"), 1080, 1920, r));
    }
    for (label, r) in [("25", (25, 1)), ("30", (30, 1))] {
        v.push(p(&format!("Square 1080x1080 {label} fps"), 1080, 1080, r));
    }
    v
}

/// Every preset (built-in, then the user's), with whether it is the user's.
pub fn all(prefs: &crate::autosave::Preferences) -> Vec<(SequencePreset, bool)> {
    let mut v: Vec<(SequencePreset, bool)> = builtin().into_iter().map(|p| (p, false)).collect();
    v.extend(prefs.sequence_presets.user.iter().cloned().map(|p| (p, true)));
    v
}

/// The preset called `name` (the user's win over a built-in one of the same name).
pub fn find(prefs: &crate::autosave::Preferences, name: &str) -> Option<SequencePreset> {
    prefs.sequence_presets.user.iter().find(|p| p.name == name).cloned().or_else(|| builtin().into_iter().find(|p| p.name == name))
}

/// The preset new sequences start from, if one is chosen.
pub fn default_preset(prefs: &crate::autosave::Preferences) -> Option<SequencePreset> {
    let n = &prefs.sequence_presets.default;
    if n.is_empty() { None } else { find(prefs, n) }
}

fn save_prefs(s: &mut Session, prefs: crate::autosave::Preferences) -> Result<()> {
    if prefs != s.prefs {
        s.set_prefs(prefs).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
    }
    Ok(())
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let fps = p.get("fps").and_then(Value::as_f64).map(FrameRate::from_f64);
    let v: Vec<Value> = all(&s.prefs).into_iter().filter(|(q, _)| fps.is_none_or(|r| q.rate() == r)).map(|(q, user)| q.to_json(user)).collect();
    Ok(json!({"presets": v, "default": s.prefs.sequence_presets.default}))
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("sequencePresets.save", "need `name`"))?;
    if builtin().iter().any(|b| b.name == name) {
        return Err(bad("sequencePresets.save", format!("`{name}` is a built-in preset; choose another name")));
    }
    let side = |k: &str| u64_p(p, k).filter(|v| (1..=u64::from(redrob_film_project::MAX_FRAME_SIDE)).contains(v)).map(|v| v as u32);
    let (width, height) = side("width").zip(side("height")).ok_or_else(|| bad("sequencePresets.save", "need `width` and `height` (frame pixels)"))?;
    let fps = p.get("fps").and_then(Value::as_f64).ok_or_else(|| bad("sequencePresets.save", "need `fps`"))?;
    let rate = FrameRate::from_f64(fps);
    if rate.num <= 0 || rate.den <= 0 {
        return Err(bad("sequencePresets.save", format!("fps must be a positive frame rate, got {fps}")));
    }
    let mix = str_p(p, "mix").unwrap_or("Stereo");
    if !["Stereo", "Mono", "5.1", "Adaptive"].contains(&mix) {
        return Err(bad("sequencePresets.save", "mix must be Stereo, Mono, 5.1 or Adaptive"));
    }
    let preset = SequencePreset {
        name: name.to_string(),
        width,
        height,
        fps_num: rate.num,
        fps_den: rate.den,
        drop_frame: bool_p(p, "dropFrame").unwrap_or(rate.supports_drop_frame()) && rate.supports_drop_frame(),
        sample_rate: u64_p(p, "sampleRate").map_or(48_000, |v| v.clamp(8_000, 192_000) as u32),
        mix: mix.to_string(),
        video_tracks: u64_p(p, "video").map_or(3, |v| v.min(256) as u32),
        audio_tracks: u64_p(p, "audio").map_or(3, |v| v.min(256) as u32),
    };
    let mut prefs = s.prefs.clone();
    match prefs.sequence_presets.user.iter_mut().find(|q| q.name == name) {
        Some(q) => *q = preset.clone(),
        None => prefs.sequence_presets.user.push(preset.clone()),
    }
    save_prefs(s, prefs)?;
    Ok(preset.to_json(true))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "name").ok_or_else(|| bad("sequencePresets.delete", "need `name`"))?;
    let mut prefs = s.prefs.clone();
    let before = prefs.sequence_presets.user.len();
    prefs.sequence_presets.user.retain(|q| q.name != name);
    if prefs.sequence_presets.user.len() == before {
        return Err(bad("sequencePresets.delete", format!("no user preset named `{name}` (built-in presets cannot be deleted)")));
    }
    if prefs.sequence_presets.default == name {
        prefs.sequence_presets.default.clear();
    }
    save_prefs(s, prefs)?;
    Ok(json!({"deleted": name}))
}

fn set_default(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "name").unwrap_or("").to_string();
    if !name.is_empty() && find(&s.prefs, &name).is_none() {
        return Err(bad("sequencePresets.setDefault", format!("unknown preset `{name}`")));
    }
    let mut prefs = s.prefs.clone();
    prefs.sequence_presets.default = name.clone();
    save_prefs(s, prefs)?;
    Ok(json!({"default": name}))
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    let c = |id, label, params, run, journal| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal };
    vec![
        c("sequencePresets.list", "List Sequence Presets", r#"{"fps":f64?}"#, list, false),
        c(
            "sequencePresets.save",
            "Save Sequence Preset",
            r#"{"name":str,"width":u32,"height":u32,"fps":f64,"dropFrame":bool?,"sampleRate":u32=48000,"mix":"Stereo|Mono|5.1|Adaptive"?,"video":n=3,"audio":n=3}"#,
            save,
            true,
        ),
        c("sequencePresets.delete", "Delete Sequence Preset", r#"{"name":str}"#, delete, true),
        c("sequencePresets.setDefault", "Set Default Sequence Preset", r#"{"name":str}"#, set_default, true),
    ]
}
