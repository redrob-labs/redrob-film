//! Kdenlive-parity timeline operations: seek by one second, Duplicate Clip, Restore Audio /
//! Restore Video, adding a colour or title clip in an empty space, and the subtitle split mode.
//!
//! | Id | What | Kdenlive layout key |
//! |---|---|---|
//! | `playhead.stepForwardSecond` / `playhead.stepBackSecond` | Forward / Rewind 1 Second | Shift+Right / Shift+Left |
//! | `timeline.duplicateClip` | Duplicate Clip: a copy of the selected clips right after them (inserted when the space is taken) | Ctrl+D |
//! | `clip.restoreAudio` / `clip.restoreVideo` | bring back the missing audio / video part of a clip from its media, linked | |
//! | `timeline.addClipInGap` | Empty space menu ▸ Add Clip: a colour clip or a title filling the selected gap | |
//! | `captions.splitMode` | subtitle split mode: duplicate the text, or split after the first line | |
//! | `transcript.insertSelection` | speech editor: insert the media under the selected words at the playhead | |
//! | `transcript.selectionToSequence` | speech editor: a new sequence (in the Project panel) of the selected words | |
//! | `transcript.addMarker` | speech editor: a marker at the selected words, named after them | |

use redrob_film_edit as edit;
use redrob_film_project::{ClipId, TrackId, TrackItem, TrackKind};
use redrob_film_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, has_selection, has_seq, str_p};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

fn spec(id: &'static str, label: &'static str, menu: &'static [&'static str], shortcut: Option<&'static str>, params: &'static str, enabled: Enabled, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut, params, enabled, run, journal: true }
}

fn has_gap(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if s.state.gap_selection.is_some() { Ok(()) } else { Err("select an empty space on a track first".into()) }
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        spec("playhead.stepForwardSecond", "Forward 1 Second", &[], None, "{}", has_seq, |s, _| step_second(s, 1)),
        spec("playhead.stepBackSecond", "Rewind 1 Second", &[], None, "{}", has_seq, |s, _| step_second(s, -1)),
        spec("timeline.duplicateClip", "Duplicate Clip", &["Clip"], None, r#"{"clips":[id]?}"#, has_selection, duplicate_clip),
        spec("clip.restoreAudio", "Restore Audio", &[], None, r#"{"clips":[id]?}"#, has_selection, |s, p| restore(s, p, TrackKind::Audio)),
        spec("clip.restoreVideo", "Restore Video", &[], None, r#"{"clips":[id]?}"#, has_selection, |s, p| restore(s, p, TrackKind::Video)),
        spec(
            "timeline.addClipInGap",
            "Add Clip in Empty Space",
            &[],
            None,
            r##"{"kind":"color"|"title","color":"#rrggbb"?,"text":str?}"##,
            has_gap,
            add_clip_in_gap,
        ),
        spec("captions.splitMode", "Split After First Line", &[], None, r#"{"afterFirstLine":bool?}"#, always, |s, p| {
            s.state.caption_split_after_first_line = bool_p(p, "afterFirstLine").unwrap_or(!s.state.caption_split_after_first_line);
            Ok(json!({"afterFirstLine": s.state.caption_split_after_first_line}))
        }),
        spec(
            "transcript.insertSelection",
            "Insert Selected Text at Playhead",
            &[],
            None,
            r#"{"from":word,"to":word?,"time":ticks?}"#,
            has_seq,
            insert_selection,
        ),
        spec(
            "transcript.selectionToSequence",
            "New Sequence from Selected Text",
            &[],
            None,
            r#"{"from":word,"to":word?,"name":str?} (the sequence lands in the Project panel)"#,
            has_seq,
            selection_to_sequence,
        ),
        spec("transcript.addMarker", "Add Marker at Text", &[], None, r#"{"from":word,"to":word?}"#, has_seq, add_marker_at_text),
    ]
}

/// The media pieces under transcript words `from..=to` of the active sequence: (media item,
/// source In, duration), one per run of words heard through one clip, in order.
fn selection_pieces(s: &Session, p: &Value, cmd: &str) -> Result<(Vec<(redrob_film_project::ItemId, Tick, Tick)>, String)> {
    let words = crate::transcript::sequence_words(s);
    let from = crate::commands::u64_p(p, "from").ok_or_else(|| bad(cmd, "`from` (word index) is required"))? as usize;
    let to = crate::commands::u64_p(p, "to").map(|n| n as usize).unwrap_or(from);
    let (a, b) = (from.min(to), from.max(to));
    if b >= words.len() {
        return Err(bad(cmd, format!("word index out of range (the transcript has {} words)", words.len())));
    }
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    let text = words[a..=b].iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    let mut pieces = Vec::new();
    let mut i = a;
    while i <= b {
        let clip = words[i].clip;
        let mut j = i;
        while j < b && words[j + 1].clip == clip {
            j += 1;
        }
        let Some(r) = redrob_film_edit::transcript::word_range(&words, i, j, rate) else { break };
        if let Some((_, it)) = q.find_item(clip) {
            let start = r.start.max(it.start);
            let end = r.end().min(it.end());
            if end > start {
                pieces.push((it.item, it.source_time_at(start), end - start));
            }
        }
        i = j + 1;
    }
    if pieces.is_empty() {
        return Err(bad(cmd, "the selected words are not on a clip"));
    }
    Ok((pieces, text))
}

fn place_pieces(s: &mut Session, pieces: &[(redrob_film_project::ItemId, Tick, Tick)], mut at: Tick, insert: bool) -> Result<Vec<u64>> {
    let mut clips = Vec::new();
    for (item, src_in, dur) in pieces {
        let r = s.execute("timeline.place", json!({"item": item.0, "time": at.0, "sourceIn": src_in.0, "duration": dur.0, "insert": insert}))?;
        clips.extend(r["clips"].as_array().into_iter().flatten().filter_map(Value::as_u64));
        at = at + *dur;
    }
    s.set_playhead(at);
    Ok(clips)
}

fn insert_selection(s: &mut Session, p: &Value) -> Result<Value> {
    let (pieces, _) = selection_pieces(s, p, "transcript.insertSelection")?;
    let at = crate::commands::time_p(s, p, "").unwrap_or(s.playhead());
    let clips = place_pieces(s, &pieces, at, true)?;
    Ok(json!({"clips": clips}))
}

fn selection_to_sequence(s: &mut Session, p: &Value) -> Result<Value> {
    let (pieces, text) = selection_pieces(s, p, "transcript.selectionToSequence")?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let st = q.settings.clone();
    let fps = st.frame_rate.num as f64 / st.frame_rate.den.max(1) as f64;
    let short: String = text.chars().take(32).collect();
    let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| format!("Text: {short}"));
    let made = s.execute("file.newSequence", json!({"name": name, "width": st.width, "height": st.height, "fps": fps, "sampleRate": st.sample_rate}))?;
    let clips = place_pieces(s, &pieces, Tick::ZERO, false)?;
    Ok(json!({"sequence": made["sequence"], "clips": clips}))
}

fn add_marker_at_text(s: &mut Session, p: &Value) -> Result<Value> {
    let words = crate::transcript::sequence_words(s);
    let from = crate::commands::u64_p(p, "from").ok_or_else(|| bad("transcript.addMarker", "`from` (word index) is required"))? as usize;
    let to = crate::commands::u64_p(p, "to").map(|n| n as usize).unwrap_or(from);
    let (a, b) = (from.min(to), from.max(to));
    let w = words.get(a).ok_or_else(|| bad("transcript.addMarker", "word index out of range"))?;
    let text: String = words.get(a..=b.min(words.len().saturating_sub(1))).unwrap_or_default().iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    let t = s.sequence_rate().snap(w.start);
    s.execute("markers.add", json!({"time": t.0, "name": text}))
}

fn step_second(s: &mut Session, dir: i64) -> Result<Value> {
    let base = s.sequence_rate().timecode_base();
    s.execute("playhead.step", json!({"frames": base * dir}))
}

/// The clips a command acts on: `clips` when given, else the selection, with linked partners.
fn clips(s: &Session, p: &Value) -> Vec<ClipId> {
    let named: Vec<ClipId> = p.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(ClipId).collect()).unwrap_or_default();
    let base = if named.is_empty() { s.state.selection.clone() } else { named };
    crate::commands::with_links(s, &base)
}

fn duplicate_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let id = "timeline.duplicateClip";
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let picked: Vec<(ClipId, TrackId, Tick, Tick)> = clips(s, p).into_iter().filter_map(|c| q.find_item(c).map(|(t, it)| (c, t, it.start, it.end()))).collect();
    if picked.is_empty() {
        return Err(bad(id, "select clips first"));
    }
    let first = picked.iter().map(|x| x.2).min().unwrap_or_default();
    let last = picked.iter().map(|x| x.3).max().unwrap_or_default();
    let delta = last - first;
    let moves: Vec<(ClipId, TrackId, Tick)> = picked.iter().map(|(c, t, st, _)| (*c, *t, *st + delta)).collect();
    // the space right after the selection must be free on every track used, else the copy is
    // inserted there and what follows moves down
    let free = picked.iter().all(|(_, t, st, en)| q.track(*t).is_some_and(|tr| edit::track_range_empty(tr, redrob_film_time::TimeRange::new(*st + delta, *en - *st))));
    let ids = s.edit_sequence("Duplicate Clip", |q, ctx, st| {
        let ids = edit::copy_items(q, &moves, !free, ctx)?;
        st.selection = ids.clone();
        Ok(ids)
    })?;
    Ok(json!({"clips": ids.iter().map(|c| c.0).collect::<Vec<_>>(), "inserted": !free}))
}

/// Restore Audio / Video: a clip whose media has the other kind of stream gets that part back,
/// on the first track of that kind with room for it, linked to the clip.
fn restore(s: &mut Session, p: &Value, want: TrackKind) -> Result<Value> {
    let id = if want == TrackKind::Audio { "clip.restoreAudio" } else { "clip.restoreVideo" };
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let base: Vec<ClipId> = {
        let named: Vec<ClipId> = p.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(ClipId).collect()).unwrap_or_default();
        if named.is_empty() { s.state.selection.clone() } else { named }
    };
    let mut todo: Vec<(TrackItem, TrackId)> = Vec::new();
    for c in base {
        let Some((tid, it)) = q.find_item(c) else { continue };
        let kind_of = |t: TrackId| if q.video_tracks.iter().any(|x| x.id == t) { TrackKind::Video } else { TrackKind::Audio };
        if kind_of(tid) == want {
            continue;
        }
        // the part is there already when a linked partner of the wanted kind exists
        let has_partner = it.link.is_some_and(|l| q.all_tracks().any(|t| kind_of(t.id) == want && t.items.iter().any(|x| x.link == Some(l) && x.id != c)));
        let media_has = s.project.item(it.item).is_some_and(|pi| if want == TrackKind::Audio { pi.has_audio() } else { pi.has_video() });
        if has_partner || !media_has {
            continue;
        }
        let tracks = if want == TrackKind::Audio { &q.audio_tracks } else { &q.video_tracks };
        let Some(dest) = tracks.iter().find(|t| !t.locked && edit::track_range_empty(t, it.range())) else {
            return Err(bad(id, "no track has room for the restored part; add a track first"));
        };
        todo.push((it.clone(), dest.id));
    }
    if todo.is_empty() {
        return Err(bad(id, if want == TrackKind::Audio { "no selected clip is missing audio its media has" } else { "no selected clip is missing video its media has" }));
    }
    let ids = s.edit_sequence(if want == TrackKind::Audio { "Restore Audio" } else { "Restore Video" }, |q, ctx, _| {
        let mut out = Vec::new();
        for (orig, dest) in todo {
            let link = match orig.link {
                Some(l) => l,
                None => {
                    let l = ctx.alloc();
                    if let Some((_, it)) = q.find_item_mut(orig.id) {
                        it.link = Some(l);
                    }
                    l
                }
            };
            let mut part = orig.clone();
            part.id = ClipId(ctx.alloc());
            part.link = Some(link);
            part.effects = if want == TrackKind::Audio { redrob_film_project::effect::intrinsic_audio() } else { redrob_film_project::effect::intrinsic_video() };
            part.markers.clear();
            out.extend(edit::overwrite(q, vec![(dest, part)], ctx)?);
        }
        Ok(out)
    })?;
    Ok(json!({"clips": ids.iter().map(|c| c.0).collect::<Vec<_>>()}))
}

fn add_clip_in_gap(s: &mut Session, p: &Value) -> Result<Value> {
    let id = "timeline.addClipInGap";
    let gap = s.state.gap_selection.ok_or_else(|| bad(id, "select an empty space on a track first"))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let Some(index) = q.video_tracks.iter().position(|t| t.id == gap.track) else {
        return Err(bad(id, "colour and title clips go on video tracks"));
    };
    let seconds = gap.range.duration.seconds();
    match str_p(p, "kind").unwrap_or("color") {
        "color" => {
            let color = str_p(p, "color").unwrap_or("#000000");
            let item = s.execute("file.newColorMatte", json!({"color": color, "seconds": seconds}))?["item"].as_u64().ok_or_else(|| bad(id, "no colour clip was made"))?;
            s.execute("timeline.place", json!({"item": item, "track": gap.track.0, "time": gap.range.start.0, "duration": gap.range.duration.0, "insert": false, "audio": false}))
        }
        "title" => {
            let text = str_p(p, "text").unwrap_or("Title");
            s.execute("graphics.newText", json!({"text": text, "newClip": true, "track": index, "time": gap.range.start.0, "seconds": seconds}))
        }
        k => Err(bad(id, format!("unknown kind `{k}` (color or title)"))),
    }
}
