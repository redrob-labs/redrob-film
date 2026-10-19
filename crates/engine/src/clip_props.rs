//! Clip Properties from Kdenlive's Project Bin that FilmCraft keeps on the source item: an image
//! sequence played as a slideshow (time per image, loop, crop to fill, dissolve or wipe with
//! softness, Ken Burns pan and zoom, low-pass filter) and the audio sync offset.
//!
//! | Id | What |
//! |---|---|
//! | `clip.slideshow` | an image sequence's slideshow options (`off: true` plays one image per frame again) |
//! | `clip.audioOffset` | the audio sync offset of a source, in milliseconds |
//! | `clip.setDuration` | the length of a still, colour clip, generator or adjustment layer |
//! | `clip.colorRange` | read a source's picture as full or limited range (or as the file says) |

use redrob_film_media::MediaKind;
use redrob_film_media::slideshow::{SlideWipe, Slideshow};
use redrob_film_project::{ItemId, MediaRef};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, item_p, str_p, u64_p};
use crate::{Result, Session};

fn target(s: &Session, p: &Value, cmd: &str) -> Result<ItemId> {
    match item_p(p, "item") {
        Some(i) => Ok(i),
        None => match s.state.project_selection.as_slice() {
            [i] => Ok(*i),
            _ => Err(bad(cmd, "select one image sequence (or pass `item`)")),
        },
    }
}

/// `clip.slideshow {item?, imageFrames?|imageSeconds?, loop?, crop?, dissolveFrames?, wipe?,
/// softness?, kenBurns?, lowPass?, off?}`.
fn slideshow(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "clip.slideshow";
    let item = target(s, p, CMD)?;
    let it = s.project.item(item).ok_or_else(|| bad(CMD, format!("no item {}", item.0)))?;
    let m = it.as_media().ok_or_else(|| bad(CMD, "not an image sequence"))?;
    let MediaRef::File { path } = &m.media else { return Err(bad(CMD, "not an image sequence")) };
    if m.info.kind != MediaKind::ImageSequence {
        return Err(bad(CMD, format!("{} is not an image sequence", it.name)));
    }
    let images = crate::media_pool::image_sequence_frames(path, &*s.services)?.len().max(1);
    let rate = m.frame_rate();
    if bool_p(p, "off").unwrap_or(false) {
        let dur = rate.tick_of(images as i64);
        s.edit("Slideshow Off", |pr, _| {
            let m = pr.item_mut(item).and_then(|i| i.as_media_mut()).ok_or_else(|| bad(CMD, "no such item"))?;
            m.interpret.slideshow = None;
            m.info.duration = dur;
            Ok(())
        })?;
        return Ok(json!({"item": item.0, "slideshow": null, "images": images, "duration": dur.0}));
    }
    let mut sl = m.interpret.slideshow.unwrap_or_default();
    if let Some(f) = u64_p(p, "imageFrames") {
        sl.image_frames = f.clamp(1, 100_000) as u32;
    }
    if let Some(secs) = f64_p(p, "imageSeconds") {
        if !(secs.is_finite() && secs > 0.0) {
            return Err(bad(CMD, "`imageSeconds` must be more than 0"));
        }
        sl.image_frames = (secs * rate.as_f64()).round().clamp(1.0, 100_000.0) as u32;
    }
    if let Some(v) = bool_p(p, "loop") {
        sl.looped = v;
    }
    if let Some(v) = bool_p(p, "crop") {
        sl.crop = v;
    }
    if let Some(f) = u64_p(p, "dissolveFrames") {
        sl.dissolve_frames = f.min(100_000) as u32;
    }
    if let Some(w) = str_p(p, "wipe") {
        sl.wipe = SlideWipe::from_name(w).ok_or_else(|| bad(CMD, format!("unknown wipe `{w}` (dissolve, left, right, top, bottom, radial)")))?;
    }
    if let Some(v) = f64_p(p, "softness") {
        sl.softness = v.clamp(0.0, 1.0) as f32;
    }
    if let Some(v) = bool_p(p, "kenBurns") {
        sl.ken_burns = v;
    }
    if let Some(v) = bool_p(p, "lowPass") {
        sl.low_pass = v;
    }
    // a transition never takes the whole time of an image
    sl.dissolve_frames = sl.dissolve_frames.min(sl.image_frames.saturating_sub(1));
    let dur = sl.duration(images, rate);
    s.edit("Slideshow", |pr, _| {
        let m = pr.item_mut(item).and_then(|i| i.as_media_mut()).ok_or_else(|| bad(CMD, "no such item"))?;
        m.interpret.slideshow = Some(sl);
        m.info.duration = dur;
        Ok(())
    })?;
    Ok(json!({"item": item.0, "slideshow": sl, "images": images, "duration": dur.0}))
}

/// `clip.audioOffset {item?|items?, ms}`: Clip Properties ▸ Audio ▸ sync offset (1 ms steps,
/// ±10 s): the source's audio plays `ms` later (negative: earlier) in every clip of it.
fn audio_offset(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "clip.audioOffset";
    let ms = p.get("ms").and_then(Value::as_i64).ok_or_else(|| bad(CMD, "need `ms` (milliseconds, -10000…10000)"))?;
    if !(-10_000..=10_000).contains(&ms) {
        return Err(bad(CMD, "`ms` is -10000…10000"));
    }
    let items: Vec<ItemId> = match p.get("items").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(|v| v.as_u64().map(ItemId)).collect(),
        None => match item_p(p, "item") {
            Some(i) => vec![i],
            None => s.state.project_selection.clone(),
        },
    };
    if items.is_empty() {
        return Err(bad(CMD, "select clips with audio (or pass `item`)"));
    }
    for i in &items {
        let it = s.project.item(*i).ok_or_else(|| bad(CMD, format!("no item {}", i.0)))?;
        if !it.as_media().is_some_and(|m| m.info.has_audio()) {
            return Err(bad(CMD, format!("{} has no audio", it.name)));
        }
    }
    s.edit("Audio Sync Offset", |pr, _| {
        for i in &items {
            if let Some(m) = pr.item_mut(*i).and_then(|x| x.as_media_mut()) {
                m.interpret.audio_offset_ms = ms as i32;
            }
        }
        Ok(())
    })?;
    Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "ms": ms}))
}

/// `clip.setDuration {item?, seconds|frames}`: Clip Properties ▸ Duration of a still image, a
/// colour clip or another generator, or an adjustment layer: how long it is when edited into a
/// sequence. Clips already in sequences keep their length.
fn set_duration(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "clip.setDuration";
    let item = match item_p(p, "item") {
        Some(i) => i,
        None => match s.state.project_selection.as_slice() {
            [i] => *i,
            _ => return Err(bad(CMD, "select one still, colour clip, generator or adjustment layer (or pass `item`)")),
        },
    };
    let it = s.project.item(item).ok_or_else(|| bad(CMD, format!("no item {}", item.0)))?;
    let rate = it.frame_rate();
    let dur = match (u64_p(p, "frames"), f64_p(p, "seconds")) {
        (Some(f), _) => rate.tick_of(f.clamp(1, 10_000_000) as i64),
        (None, Some(secs)) if secs.is_finite() && secs > 0.0 => rate.snap(redrob_film_time::Tick::from_seconds_f64(secs.min(36_000.0))),
        _ => return Err(bad(CMD, "need `seconds` (more than 0) or `frames`")),
    };
    let dur = dur.max(rate.frame_duration());
    let ok = match &it.kind {
        redrob_film_project::ItemKind::Media(m) => matches!(m.info.kind, MediaKind::Still | MediaKind::Synthetic) || matches!(m.media, MediaRef::Generator(_)),
        redrob_film_project::ItemKind::AdjustmentLayer { .. } => true,
        _ => false,
    };
    if !ok {
        return Err(bad(CMD, format!("{} has a fixed length (only stills, colour clips, generators and adjustment layers)", it.name)));
    }
    s.edit("Set Duration", |pr, _| {
        match pr.item_mut(item).map(|i| &mut i.kind) {
            Some(redrob_film_project::ItemKind::Media(m)) => m.info.duration = dur,
            Some(redrob_film_project::ItemKind::AdjustmentLayer { duration, .. }) => *duration = dur,
            _ => return Err(bad(CMD, "no such item")),
        }
        Ok(())
    })?;
    Ok(json!({"item": item.0, "duration": dur.0, "seconds": dur.seconds()}))
}

/// `clip.colorRange {item?|items?, range: "auto"|"full"|"limited"}`: Clip Properties ▸ Color
/// range: read the picture as full or limited range whatever the file says ("auto": the file's).
fn color_range(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "clip.colorRange";
    let range = match str_p(p, "range").map(str::to_ascii_lowercase).as_deref() {
        Some("auto") => None,
        Some("full") => Some(redrob_film_color::Range::Full),
        Some("limited") => Some(redrob_film_color::Range::Limited),
        _ => return Err(bad(CMD, "need `range`: auto, full or limited")),
    };
    let items: Vec<ItemId> = match p.get("items").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(|v| v.as_u64().map(ItemId)).collect(),
        None => match item_p(p, "item") {
            Some(i) => vec![i],
            None => s.state.project_selection.clone(),
        },
    };
    if items.is_empty() {
        return Err(bad(CMD, "select video clips (or pass `item`)"));
    }
    for i in &items {
        let it = s.project.item(*i).ok_or_else(|| bad(CMD, format!("no item {}", i.0)))?;
        if !it.as_media().is_some_and(|m| m.info.has_video()) {
            return Err(bad(CMD, format!("{} has no picture", it.name)));
        }
    }
    s.edit("Color Range", |pr, _| {
        for i in &items {
            if let Some(m) = pr.item_mut(*i).and_then(|x| x.as_media_mut()) {
                m.interpret.color_range = range;
            }
        }
        Ok(())
    })?;
    let name = match range {
        None => "auto",
        Some(redrob_film_color::Range::Full) => "full",
        Some(redrob_film_color::Range::Limited) => "limited",
    };
    Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "range": name}))
}

/// The slideshow options of an item (None when it plays one image per frame).
pub fn slideshow_of(s: &Session, item: ItemId) -> Option<Slideshow> {
    s.project.item(item).and_then(|i| i.as_media()).and_then(|m| m.interpret.slideshow)
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "clip.slideshow",
            label: "Slideshow",
            menu: &[],
            shortcut: None,
            params: r#"{"item":id?,"imageFrames":u32?,"imageSeconds":f64?,"loop":bool?,"crop":bool?,"dissolveFrames":u32?,"wipe":"dissolve|left|right|top|bottom|radial"?,"softness":0..1?,"kenBurns":bool?,"lowPass":bool?,"off":bool?}"#,
            enabled: always,
            run: slideshow,
            journal: true,
        },
        CommandSpec {
            id: "clip.audioOffset",
            label: "Audio Sync Offset",
            menu: &[],
            shortcut: None,
            params: r#"{"ms":-10000..10000,"item":id?,"items":[id]?}"#,
            enabled: always,
            run: audio_offset,
            journal: true,
        },
        CommandSpec {
            id: "clip.setDuration",
            label: "Set Duration",
            menu: &[],
            shortcut: None,
            params: r#"{"item":id?,"seconds":f64?,"frames":u64?}"#,
            enabled: always,
            run: set_duration,
            journal: true,
        },
        CommandSpec {
            id: "clip.colorRange",
            label: "Color Range",
            menu: &[],
            shortcut: None,
            params: r#"{"range":"auto|full|limited","item":id?,"items":[id]?}"#,
            enabled: always,
            run: color_range,
            journal: true,
        },
    ]
}
