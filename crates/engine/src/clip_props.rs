//! Clip Properties from Kdenlive's Project Bin that FilmCraft keeps on the source item: an image
//! sequence played as a slideshow (time per image, loop, crop to fill, dissolve or wipe with
//! softness, Ken Burns pan and zoom, low-pass filter).
//!
//! | Id | What |
//! |---|---|
//! | `clip.slideshow` | an image sequence's slideshow options (`off: true` plays one image per frame again) |

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

/// The slideshow options of an item (None when it plays one image per frame).
pub fn slideshow_of(s: &Session, item: ItemId) -> Option<Slideshow> {
    s.project.item(item).and_then(|i| i.as_media()).and_then(|m| m.interpret.slideshow)
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "clip.slideshow",
        label: "Slideshow",
        menu: &[],
        shortcut: None,
        params: r#"{"item":id?,"imageFrames":u32?,"imageSeconds":f64?,"loop":bool?,"crop":bool?,"dissolveFrames":u32?,"wipe":"dissolve|left|right|top|bottom|radial"?,"softness":0..1?,"kenBurns":bool?,"lowPass":bool?,"off":bool?}"#,
        enabled: always,
        run: slideshow,
        journal: true,
    }]
}
