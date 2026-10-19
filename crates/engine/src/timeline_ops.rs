//! Timeline commands from Kdenlive's timeline manual (clean room: written from the documented
//! behaviour only): edit modes, the current (keyboard-selected) track and the commands that act on
//! it, spaces, snap point navigation, playhead selection variants, groups, track order, sequence
//! tabs, the timeline render switches and group-wide effect parameters.
//!
//! | Id | Menu | Behaviour |
//! |---|---|---|
//! | `timeline.editMode.normal` / `.overwrite` / `.insert` | Sequence ▸ Edit Mode | Normal refuses a drop or move that would cover a clip; Insert makes drops insert and Delete ripple |
//! | `timeline.editMode` | | read / set the mode (`mode`) |
//! | `sequence.useTimelineZone` | Sequence ▸ Use Timeline Zone for Insert | off: source Insert / Overwrite go to the playhead even with a sequence In |
//! | `timeline.selectTrack`, `timeline.trackUp` / `trackDown`, `timeline.selectVideoTrack1…9`, `timeline.selectAudioTrack1…9` | | the current track |
//! | `timeline.toggleTargetCurrent`, `timeline.toggleAllTargets`, `timeline.invertAllTargets` | | track active (targeted) |
//! | `timeline.toggleSourceCurrent`, `timeline.cycleAudioTarget` | | source patch of the current track / next audio destination |
//! | `timeline.toggleLockCurrent`, `timeline.toggleLockAll`, `timeline.toggleOutputCurrent` | | lock, hide / mute the current track |
//! | `timeline.moveTrackUp` / `timeline.moveTrackDown` | | reorder a track within its kind |
//! | `timeline.addClipAtPlayhead` / `deselectClipAtPlayhead` | | selection at the playhead (targeted tracks) |
//! | `timeline.selectTransitionAtPlayhead` / `add…` / `deselect…` | | transitions at the playhead |
//! | `timeline.selectAllInTrack` | | every clip of the current track |
//! | `playhead.nextSnapPoint` / `prevSnapPoint` | | clip edges, clip and sequence markers, In / Out |
//! | `sequence.removeSpaceAllTracks` | Sequence ▸ Remove Space in All Tracks | closes the empty time at the playhead on every unlocked track |
//! | `sequence.removeAllSpacesAfter` | Sequence ▸ Remove All Spaces After Playhead | packs the clips after the playhead on the current track |
//! | `sequence.removeAllClipsAfter` | Sequence ▸ Remove All Clips After Playhead | deletes every clip starting at or after the playhead |
//! | `sequence.insertSpace` | Sequence ▸ Insert Space… | blank time on the current track or all tracks |
//! | `markers.identifyGaps` | Markers ▸ Identify Gaps | a range marker on every gap (all tracks or one) |
//! | `sequence.trackCompositing` / `sequence.disableTimelineEffects` | Sequence | the sequence's render switches |
//! | `sequence.nextTab` / `sequence.prevTab` | | Ctrl+Tab / Ctrl+Shift+Tab |
//! | `clip.toggleEffectStack` | | every standard effect of the clips on / off (the fx badge) |
//! | `clip.setTiming` | Clip ▸ Clip Duration… | position, source In and duration of one clip |
//! | `clip.pitchCompensation` | | a Pitch Shifter that undoes the pitch change of the clip's speed |
//! | `effects.setParamGroup` | | one parameter on every grouped clip with the same effect |

use std::collections::HashMap;

use redrob_film_edit as edit;
use redrob_film_project::{ClipId, ItemId, Label, Marker, MarkerId, MarkerKind, Param, ParamValue, Sequence, TrackId, TrackKind, TransitionId};
use redrob_film_time::{Tick, TimeRange};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, clip_p, clips_p, has_selection, has_seq, json_to_param, str_p, time_p, track_p, with_links};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

fn spec(
    id: &'static str,
    label: &'static str,
    menu: &'static [&'static str],
    shortcut: Option<&'static str>,
    params: &'static str,
    enabled: Enabled,
    run: Run,
) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut, params, enabled, run, journal: true }
}

// ------------------------------------------------------------------ edit modes

/// How drops and moves treat the clips already on the destination track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditMode {
    /// The clip replaces what it covers (Premiere's default drag).
    #[default]
    Overwrite,
    /// A drop or move that would cover another clip is refused.
    Normal,
    /// Drops insert and push the later clips; Delete ripples.
    Insert,
}

impl EditMode {
    pub const ALL: [EditMode; 3] = [EditMode::Normal, EditMode::Overwrite, EditMode::Insert];
    pub fn name(self) -> &'static str {
        match self {
            EditMode::Normal => "normal",
            EditMode::Overwrite => "overwrite",
            EditMode::Insert => "insert",
        }
    }
    /// The menu label (also the mode indicator's text).
    pub fn label(self) -> &'static str {
        match self {
            EditMode::Normal => "Normal Mode",
            EditMode::Overwrite => "Overwrite Mode",
            EditMode::Insert => "Insert Mode",
        }
    }
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name().eq_ignore_ascii_case(s))
    }
}

fn set_mode(s: &mut Session, m: EditMode) -> Result<Value> {
    s.state.edit_mode = m;
    Ok(json!({"mode": m.name()}))
}

/// Normal edit mode: refuse placements (`track`, `start`, `end`) that would cover a clip already on
/// their track. Clips in `moving` are leaving their place, so they do not count.
pub(crate) fn refuse_overlap(q: &Sequence, spans: &[(TrackId, Tick, Tick)], moving: &[ClipId]) -> Result<()> {
    for &(track, start, end) in spans {
        let Some(tr) = q.track(track) else { continue };
        if let Some(it) = tr.items.iter().find(|i| !moving.contains(&i.id) && i.start < end && i.end() > start) {
            return Err(EngineError::Other(format!(
                "Normal edit mode: this would overlap \u{201c}{}\u{201d} on {}; switch to Overwrite or Insert mode to replace or push it",
                it.name,
                edit::track_label(q, track)
            )));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ groups

/// `clips` with their linked partners (as [`with_links`]) and every clip of their groups
/// (Clip ▸ Group): clicking one grouped clip selects the group.
pub fn with_groups(s: &Session, clips: &[ClipId]) -> Vec<ClipId> {
    let mut out = with_links(s, clips);
    let Some(q) = s.active_sequence() else { return out };
    let groups: Vec<u64> = out.iter().filter_map(|c| q.find_item(*c).and_then(|(_, i)| i.group)).collect();
    if groups.is_empty() {
        return out;
    }
    let mut extra = Vec::new();
    for t in q.all_tracks() {
        for i in &t.items {
            if i.group.is_some_and(|g| groups.contains(&g)) && !out.contains(&i.id) {
                extra.push(i.id);
            }
        }
    }
    for c in with_links(s, &extra) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// `timeline.move`: grouped clips that are not listed follow a listed clip of their group by the
/// same time offset, on their own (unlocked) tracks.
pub(crate) fn add_group_partners(s: &Session, listed: &mut Vec<(ClipId, TrackId, Tick)>) {
    let Some(q) = s.active_sequence() else { return };
    let mut extra: Vec<(ClipId, TrackId, Tick)> = Vec::new();
    for &(clip, _, time) in listed.iter() {
        let Some((_, it)) = q.find_item(clip) else { continue };
        let Some(g) = it.group else { continue };
        let Some(off) = time.0.checked_sub(it.start.0) else { continue };
        for t in q.all_tracks().filter(|t| !t.locked) {
            for o in t.items.iter().filter(|i| i.group == Some(g)) {
                if listed.iter().any(|m| m.0 == o.id) || extra.iter().any(|m| m.0 == o.id) {
                    continue;
                }
                if let Some(start) = o.start.0.checked_add(off) {
                    extra.push((o.id, t.id, Tick(start)));
                }
            }
        }
    }
    listed.extend(extra);
}

/// Clip ▸ Group parameters: set one parameter on the clip and on every clip of its group that
/// has the same effect; with `onlyEqual`, only on those whose value was the same as the clip's.
fn set_param_group(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "effects.setParamGroup";
    let c = clip_p(p, "clip").ok_or_else(|| bad(CMD, "need `clip`"))?;
    let pid = str_p(p, "param").ok_or_else(|| bad(CMD, "need `param`"))?.to_string();
    let val = p.get("value").cloned().ok_or_else(|| bad(CMD, "need `value`"))?;
    let only_equal = bool_p(p, "onlyEqual").unwrap_or(false);
    let tl = time_p(s, p, "").unwrap_or(s.playhead());
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = q.find_item(c).ok_or(edit::EditError::NoItem(c))?;
    let effect = match p.get("effect") {
        Some(Value::Number(n)) => it.effects.get(n.as_u64().unwrap_or(0) as usize).map(|e| e.effect.clone()),
        Some(Value::String(id)) => Some(id.clone()),
        _ => None,
    }
    .ok_or_else(|| bad(CMD, "need `effect` (an index or an effect id on the clip)"))?;
    let at = |it: &redrob_film_project::TrackItem| it.source_time_at(tl.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
    let before = it.effect(&effect).and_then(|e| e.param(&pid)).map(|prm| prm.value_at(at(it)));
    let mut targets = vec![c];
    if let Some(g) = it.group {
        targets.extend(q.all_tracks().filter(|t| !t.locked).flat_map(|t| t.items.iter()).filter(|i| i.group == Some(g) && i.id != c).map(|i| i.id));
    }
    let merge = bool_p(p, "merge").unwrap_or(false).then(|| format!("setParamGroup:{}:{effect}:{pid}", c.0));
    if bool_p(p, "begin").unwrap_or(false) {
        s.history.merge_key = None;
    }
    let n = s.edit_sequence_as("Change Effect Parameter (Group)", merge.as_deref(), |q, _, _| {
        let mut n = 0;
        for t in &targets {
            let Some((_, it)) = q.find_item_mut(*t) else { continue };
            let mt = at(&*it);
            let Some(e) = it.effects.iter_mut().find(|e| e.effect == effect) else { continue };
            if !e.params.contains_key(&pid)
                && let Some(d) = e.def().and_then(|d| d.param(&pid))
            {
                e.params.insert(pid.clone(), Param::new(d.default.clone()));
            }
            let Some(prm) = e.params.get_mut(&pid) else { continue };
            if only_equal && *t != c && before.as_ref().is_some_and(|b| *b != prm.value_at(mt)) {
                continue;
            }
            let v = json_to_param(&prm.value, &val).ok_or_else(|| bad(CMD, "value has the wrong type"))?;
            prm.set_at(mt, v);
            n += 1;
        }
        Ok(n)
    })?;
    Ok(json!({"changed": n}))
}

// ------------------------------------------------------------------ current track

/// Tracks top to bottom as the Timeline shows them: the highest video track first, then A1, A2…
fn visual_order(q: &Sequence) -> Vec<TrackId> {
    q.video_tracks.iter().rev().chain(q.audio_tracks.iter()).map(|t| t.id).collect()
}

/// The current track: the one picked last (header click, Select Track, Up / Down), else V1.
pub fn current_track(s: &Session) -> Option<TrackId> {
    let q = s.active_sequence()?;
    s.state.current_track.filter(|t| q.all_tracks().any(|x| x.id == *t)).or_else(|| q.video_tracks.first().or(q.audio_tracks.first()).map(|t| t.id))
}

/// The `track` parameter, or the current track.
fn track_or_current(s: &Session, p: &Value, cmd: &str) -> Result<TrackId> {
    match track_p(s, p, "track", cmd)? {
        Some(t) => Ok(t),
        None => current_track(s).ok_or_else(|| EngineError::Other("the sequence has no tracks".into())),
    }
}

fn select_track(s: &mut Session, t: TrackId) -> Result<Value> {
    s.state.current_track = Some(t);
    let name = s.active_sequence().map(|q| edit::track_label(q, t)).unwrap_or_default();
    Ok(json!({"track": t.0, "label": name}))
}

fn step_track(s: &mut Session, up: bool) -> Result<Value> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let order = visual_order(q);
    if order.is_empty() {
        return Err(EngineError::Other("the sequence has no tracks".into()));
    }
    let cur = current_track(s);
    let i = order.iter().position(|t| Some(*t) == cur).unwrap_or(0);
    let j = if up { i.saturating_sub(1) } else { (i + 1).min(order.len() - 1) };
    select_track(s, order[j])
}

fn select_nth(s: &mut Session, kind: TrackKind, n: usize) -> Result<Value> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let t = q
        .tracks(kind)
        .get(n.saturating_sub(1))
        .map(|t| t.id)
        .ok_or_else(|| EngineError::Other(format!("there is no track {}{n}", if kind == TrackKind::Video { "V" } else { "A" })))?;
    select_track(s, t)
}

fn set_targeting(s: &mut Session, tg: crate::Targeting) -> Result<Value> {
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let out = json!({
        "targeted": tg.targeted.iter().map(|t| t.0).collect::<Vec<_>>(),
        "videoDest": tg.video_dest.map(|t| t.0),
        "audioDest": tg.audio_dest.map(|t| t.0),
    });
    s.state.targeting.insert(seq, tg);
    Ok(out)
}

fn all_track_ids(s: &Session) -> Vec<TrackId> {
    s.active_sequence().map(|q| q.all_tracks().map(|t| t.id).collect()).unwrap_or_default()
}

fn toggle_target_current(s: &mut Session, p: &Value) -> Result<Value> {
    let t = track_or_current(s, p, "timeline.toggleTargetCurrent")?;
    let mut tg = s.targeting();
    if let Some(i) = tg.targeted.iter().position(|x| *x == t) {
        tg.targeted.remove(i);
    } else {
        tg.targeted.push(t);
    }
    set_targeting(s, tg)
}

/// All tracks active (none active when they all are), or every track's state switched.
fn all_targets(s: &mut Session, invert: bool) -> Result<Value> {
    let ids = all_track_ids(s);
    let mut tg = s.targeting();
    if invert {
        let now: Vec<TrackId> = ids.iter().copied().filter(|t| !tg.targeted.contains(t)).collect();
        tg.targeted = now;
    } else if ids.iter().all(|t| tg.targeted.contains(t)) {
        tg.targeted.clear();
    } else {
        tg.targeted = ids;
    }
    set_targeting(s, tg)
}

/// The current track becomes (or stops being) the source patch of its kind.
fn toggle_source_current(s: &mut Session, p: &Value) -> Result<Value> {
    let t = track_or_current(s, p, "timeline.toggleSourceCurrent")?;
    let is_video = s.active_sequence().is_some_and(|q| q.video_tracks.iter().any(|x| x.id == t));
    let mut tg = s.targeting();
    let slot = if is_video { &mut tg.video_dest } else { &mut tg.audio_dest };
    *slot = if *slot == Some(t) { None } else { Some(t) };
    set_targeting(s, tg)
}

/// The source audio goes to the next audio track (A1 after the last; unpatched: A1).
fn cycle_audio_target(s: &mut Session) -> Result<Value> {
    let ids: Vec<TrackId> = s.active_sequence().map(|q| q.audio_tracks.iter().map(|t| t.id).collect()).unwrap_or_default();
    if ids.is_empty() {
        return Err(EngineError::Other("the sequence has no audio tracks".into()));
    }
    let mut tg = s.targeting();
    let next = match tg.audio_dest.and_then(|d| ids.iter().position(|x| *x == d)) {
        Some(i) => ids[(i + 1) % ids.len()],
        None => ids[0],
    };
    tg.audio_dest = Some(next);
    set_targeting(s, tg)
}

/// Lock / output switches of the current track (`flag` = `locked` | `output`).
fn toggle_track_flag(s: &mut Session, p: &Value, flag: &'static str) -> Result<Value> {
    let t = track_or_current(s, p, "timeline.toggleTrack")?;
    let label = if flag == "locked" { "Toggle Track Lock" } else { "Toggle Track Output" };
    let v = s.edit_sequence(label, |q, _, _| {
        let tr = q.track_mut(t).ok_or(edit::EditError::NoTrack(t))?;
        Ok(match (flag, tr.kind) {
            ("locked", _) => {
                tr.locked = !tr.locked;
                json!({"track": t.0, "locked": tr.locked})
            }
            (_, TrackKind::Video) => {
                tr.enabled = !tr.enabled;
                json!({"track": t.0, "enabled": tr.enabled})
            }
            (_, TrackKind::Audio) => {
                tr.muted = !tr.muted;
                json!({"track": t.0, "muted": tr.muted})
            }
        })
    })?;
    Ok(v)
}

fn toggle_lock_all(s: &mut Session) -> Result<Value> {
    let lock = s.active_sequence().is_some_and(|q| q.all_tracks().any(|t| !t.locked));
    s.edit_sequence(if lock { "Lock All Tracks" } else { "Unlock All Tracks" }, |q, _, _| {
        q.all_tracks_mut().for_each(|t| t.locked = lock);
        Ok(())
    })?;
    Ok(json!({"locked": lock}))
}

/// Move a track up or down among the tracks of its kind (as the Timeline shows them).
fn move_track(s: &mut Session, p: &Value, up: bool) -> Result<Value> {
    let t = track_or_current(s, p, "timeline.moveTrack")?;
    s.edit_sequence(if up { "Move Track Up" } else { "Move Track Down" }, |q, _, _| {
        for (kind, list) in [(TrackKind::Video, &mut q.video_tracks), (TrackKind::Audio, &mut q.audio_tracks)] {
            let Some(i) = list.iter().position(|x| x.id == t) else { continue };
            // video tracks stack upward (V1 lowest), audio tracks downward (A1 highest)
            let j = match (kind, up) {
                (TrackKind::Video, true) | (TrackKind::Audio, false) => i + 1,
                _ => i.checked_sub(1).ok_or_else(|| EngineError::Other("the track is already at the edge".into()))?,
            };
            if j >= list.len() {
                return Err(EngineError::Other("the track is already at the edge".into()));
            }
            list.swap(i, j);
            return Ok(());
        }
        Err(edit::EditError::NoTrack(t).into())
    })?;
    Ok(json!({"track": t.0}))
}

// ------------------------------------------------------------------ selection at the playhead

fn clips_at_playhead(s: &Session) -> Vec<ClipId> {
    let t = s.playhead();
    let tg = s.targeting().targeted;
    let Some(q) = s.active_sequence() else { return Vec::new() };
    q.all_tracks().filter(|tr| tg.contains(&tr.id)).filter_map(|tr| tr.item_at(t).map(|i| i.id)).collect()
}

fn selection_json(s: &Session) -> Value {
    json!({
        "selection": s.state.selection.iter().map(|c| c.0).collect::<Vec<_>>(),
        "transitions": s.state.transition_selection.iter().map(|t| t.0).collect::<Vec<_>>(),
    })
}

fn add_clip_at_playhead(s: &mut Session, add: bool) -> Result<Value> {
    let ids = with_groups(s, &clips_at_playhead(s));
    if add {
        for c in ids {
            if !s.state.selection.contains(&c) {
                s.state.selection.push(c);
            }
        }
        s.state.transition_selection.clear();
        s.state.gap_selection = None;
        s.state.edit_points.clear();
    } else {
        s.state.selection.retain(|c| !ids.contains(c));
    }
    Ok(selection_json(s))
}

fn transitions_at_playhead(s: &Session) -> Vec<TransitionId> {
    let t = s.playhead();
    let tg = s.targeting().targeted;
    let Some(q) = s.active_sequence() else { return Vec::new() };
    q.all_tracks().filter(|tr| tg.contains(&tr.id)).flat_map(|tr| tr.transitions.iter().filter(|x| x.range().contains(t)).map(|x| x.id)).collect()
}

/// `mode`: 0 select, 1 add, 2 deselect. Selecting transitions clears the clip selection.
fn transition_at_playhead(s: &mut Session, mode: u8) -> Result<Value> {
    let ids = transitions_at_playhead(s);
    match mode {
        0 => s.state.transition_selection = ids,
        1 => {
            for t in ids {
                if !s.state.transition_selection.contains(&t) {
                    s.state.transition_selection.push(t);
                }
            }
        }
        _ => s.state.transition_selection.retain(|t| !ids.contains(t)),
    }
    if mode < 2 {
        s.state.selection.clear();
        s.state.gap_selection = None;
        s.state.edit_points.clear();
    }
    Ok(selection_json(s))
}

fn select_all_in_track(s: &mut Session, p: &Value) -> Result<Value> {
    let t = track_or_current(s, p, "timeline.selectAllInTrack")?;
    let ids: Vec<ClipId> = s.active_sequence().and_then(|q| q.track(t)).map(|tr| tr.items.iter().map(|i| i.id).collect()).unwrap_or_default();
    s.state.selection = ids;
    s.state.transition_selection.clear();
    s.state.gap_selection = None;
    s.state.edit_points.clear();
    Ok(selection_json(s))
}

// ------------------------------------------------------------------ snap points

/// Every point the Timeline snaps to: clip edges, clip markers, sequence markers (both ends of a
/// range marker), the In and Out points. Sorted, unique.
pub fn snap_points(q: &Sequence) -> Vec<Tick> {
    let rate = q.settings.frame_rate;
    let mut v: Vec<Tick> = Vec::new();
    for tr in q.all_tracks() {
        for it in &tr.items {
            v.push(it.start);
            v.push(it.end());
            for m in &it.markers {
                if let Some(t) = crate::sequence_tools::timeline_time_of(it, m.start) {
                    v.push(t);
                }
            }
        }
    }
    for m in &q.markers {
        v.push(m.start);
        if m.duration > Tick::ZERO {
            v.push(m.start + m.duration);
        }
    }
    v.extend(q.mark_in);
    v.extend(q.mark_out.map(|o| o + rate.frame_duration()));
    v.sort_unstable();
    v.dedup();
    v
}

fn go_to_snap(s: &mut Session, next: bool) -> Result<Value> {
    let t = s.playhead();
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let pts = snap_points(q);
    let to = if next { pts.into_iter().find(|p| *p > t) } else { pts.into_iter().rev().find(|p| *p < t) };
    if let Some(to) = to {
        s.set_playhead(to);
    }
    Ok(json!({"time": s.playhead().0}))
}

// ------------------------------------------------------------------ spaces

fn duration_p(s: &Session, p: &Value) -> Option<Tick> {
    p.get("duration")
        .and_then(Value::as_i64)
        .map(Tick)
        .or_else(|| p.get("durationFrames").and_then(Value::as_i64).map(|f| s.sequence_rate().tick_of(f)))
        .or_else(|| p.get("durationSeconds").and_then(Value::as_f64).map(|x| s.sequence_rate().snap_nearest(Tick::from_seconds_f64(x))))
}

/// Remove Space in All Tracks: the empty time at `t`, empty on every unlocked track, closes; every
/// unlocked track moves left by its length.
fn remove_space_all(s: &mut Session, p: &Value) -> Result<Value> {
    let t = time_p(s, p, "").unwrap_or(s.playhead());
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let open: Vec<&redrob_film_project::Track> = q.all_tracks().filter(|tr| !tr.locked).collect();
    if open.iter().any(|tr| tr.item_at(t).is_some()) {
        return Err(EngineError::Other("there is a clip at the playhead: Remove Space needs empty time on every unlocked track".into()));
    }
    let a = open.iter().flat_map(|tr| tr.items.iter()).filter(|i| i.end() <= t).map(|i| i.end()).max().unwrap_or(Tick::ZERO);
    let b = open
        .iter()
        .flat_map(|tr| tr.items.iter())
        .filter(|i| i.start > t)
        .map(|i| i.start)
        .min()
        .ok_or_else(|| EngineError::Other("no clip after the playhead: there is no space to remove".into()))?;
    let gap = TimeRange::from_bounds(a, b);
    s.edit_sequence("Remove Space in All Tracks", |q, _, st| {
        for tr in q.all_tracks_mut().filter(|tr| !tr.locked) {
            edit::shift_track_from(tr, gap.end(), -gap.duration);
        }
        if st.ripple_sequence_markers {
            crate::sequence_tools::ripple_markers(&mut q.markers, gap.end(), -gap.duration);
        }
        Ok(())
    })?;
    Ok(json!({"removed": gap.duration.0, "start": gap.start.0}))
}

/// Remove All Spaces After Playhead: on one track, the clips after `t` close up on each other (and
/// on the clip at or before `t`).
fn remove_all_spaces_after(s: &mut Session, p: &Value) -> Result<Value> {
    let t = time_p(s, p, "").unwrap_or(s.playhead());
    let track = track_or_current(s, p, "sequence.removeAllSpacesAfter")?;
    let removed = s.edit_sequence("Remove All Spaces After Playhead", |q, _, _| {
        let tr = q.track_mut(track).ok_or(edit::EditError::NoTrack(track))?;
        if tr.locked {
            return Err(edit::EditError::Locked.into());
        }
        let mut pos = match tr.item_at(t) {
            Some(i) => i.end(),
            None => tr.items.iter().filter(|i| i.end() <= t).map(|i| i.end()).max().unwrap_or(Tick::ZERO),
        };
        let mut removed = Tick::ZERO;
        loop {
            let Some((id, start)) = tr.items.iter().filter(|i| i.start >= pos).min_by_key(|i| i.start).map(|i| (i.id, i.start)) else { break };
            if start > pos {
                edit::shift_track_from(tr, start, pos - start);
                removed += start - pos;
            }
            pos = tr.item(id).map(|i| i.end()).unwrap_or(pos);
        }
        if removed == Tick::ZERO {
            return Err(edit::EditError::Nothing.into());
        }
        Ok(removed)
    })?;
    Ok(json!({"removed": removed.0}))
}

/// Remove All Clips After Playhead: every clip on an unlocked track starting at or after `t`, with
/// its linked partners (picture and sound go together).
fn remove_all_clips_after(s: &mut Session, p: &Value) -> Result<Value> {
    let t = time_p(s, p, "").unwrap_or(s.playhead());
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let ids: Vec<ClipId> = q.all_tracks().filter(|tr| !tr.locked).flat_map(|tr| tr.items.iter()).filter(|i| i.start >= t).map(|i| i.id).collect();
    if ids.is_empty() {
        return Err(EngineError::Other("no clip starts after the playhead".into()));
    }
    let ids = with_links(s, &ids);
    let n = s.edit_sequence("Remove All Clips After Playhead", |q, _, st| {
        let n = edit::delete_items(q, &ids);
        st.selection.retain(|c| !ids.contains(c));
        Ok(n)
    })?;
    Ok(json!({"removed": n}))
}

/// Insert Space: `duration` of blank time at `t` on one track (`track`, else the current track)
/// or on every unlocked track (`"track": "all"`); a clip spanning `t` is split.
fn insert_space(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "sequence.insertSpace";
    let t = time_p(s, p, "").unwrap_or(s.playhead());
    let dur = duration_p(s, p).ok_or_else(|| bad(CMD, "need `duration` (ticks), `durationFrames` or `durationSeconds`"))?;
    if dur <= Tick::ZERO {
        return Err(bad(CMD, "the duration must be positive"));
    }
    let all = str_p(p, "track").is_some_and(|x| x.eq_ignore_ascii_case("all"));
    let track = if all { None } else { Some(track_or_current(s, p, CMD)?) };
    s.edit_sequence("Insert Space", |q, ctx, st| {
        let mut links = HashMap::new();
        let mut n = 0;
        for tr in q.all_tracks_mut() {
            if tr.locked || track.is_some_and(|x| x != tr.id) {
                continue;
            }
            edit::insert_track_gap(tr, t, dur, ctx, &mut links);
            n += 1;
        }
        if n == 0 {
            return Err(edit::EditError::Locked.into());
        }
        if all && st.ripple_sequence_markers {
            crate::sequence_tools::ripple_markers(&mut q.markers, t, dur);
        }
        Ok(())
    })?;
    Ok(json!({"time": t.0, "duration": dur.0}))
}

/// The empty stretches between the sequence start and the end of the last clip, on the given
/// tracks together (time where none of them has a clip).
pub fn gaps(q: &Sequence, tracks: &[TrackId]) -> Vec<TimeRange> {
    let mut spans: Vec<(Tick, Tick)> = q.all_tracks().filter(|t| tracks.contains(&t.id)).flat_map(|t| t.items.iter().map(|i| (i.start, i.end()))).collect();
    spans.sort_unstable();
    let mut out = Vec::new();
    let mut pos = Tick::ZERO;
    for (a, b) in spans {
        if a > pos {
            out.push(TimeRange::from_bounds(pos, a));
        }
        pos = pos.max(b);
    }
    out
}

/// Identify Gaps: a range marker ("Gap") on every gap of all tracks (`track` absent or `"all"`), or
/// of one track.
fn identify_gaps(s: &mut Session, p: &Value) -> Result<Value> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let all = match p.get("track") {
        None | Some(Value::Null) => true,
        Some(v) => v.as_str().is_some_and(|x| x.eq_ignore_ascii_case("all")),
    };
    let tracks: Vec<TrackId> = if all { q.all_tracks().map(|t| t.id).collect() } else { vec![track_or_current(s, p, "markers.identifyGaps")?] };
    let found = gaps(q, &tracks);
    if found.is_empty() {
        return Err(EngineError::Other("no gaps found".into()));
    }
    let color = str_p(p, "color").and_then(Label::from_name).unwrap_or(Label::Rose);
    let n = found.len();
    s.edit_sequence("Identify Gaps", |q, ctx, _| {
        for g in &found {
            let id = MarkerId(ctx.alloc());
            q.markers.push(Marker { id, start: g.start, duration: g.duration, name: "Gap".into(), comment: String::new(), kind: MarkerKind::Comment, color });
        }
        q.markers.sort_by_key(|m| m.start);
        Ok(())
    })?;
    Ok(json!({"gaps": n}))
}

// ------------------------------------------------------------------ sequence switches and tabs

fn track_compositing(s: &mut Session, p: &Value) -> Result<Value> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    // on = compositing on; toggles without `on`
    let on = bool_p(p, "on").unwrap_or(q.timeline_options.no_track_compositing);
    s.edit_sequence("Track Compositing", |q, _, _| {
        q.timeline_options.no_track_compositing = !on;
        Ok(())
    })?;
    Ok(json!({"trackCompositing": on}))
}

fn disable_effects(s: &mut Session, p: &Value) -> Result<Value> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let off = bool_p(p, "on").unwrap_or(!q.timeline_options.effects_disabled);
    s.edit_sequence(if off { "Disable Timeline Effects" } else { "Enable Timeline Effects" }, |q, _, _| {
        q.timeline_options.effects_disabled = off;
        Ok(())
    })?;
    Ok(json!({"effectsDisabled": off}))
}

/// The bin of the project root called `name`, made when there is none (Settings ▸ Timeline ▸
/// Default bin for new sequences).
pub fn root_bin_named(p: &mut redrob_film_project::Project, name: &str) -> redrob_film_project::BinId {
    let found = p.root.children.iter().find_map(|c| match c {
        redrob_film_project::BinEntry::Bin(b) if b.name == name => Some(b.id),
        _ => None,
    });
    found.unwrap_or_else(|| p.add_bin(name, None))
}

fn step_tab(s: &mut Session, next: bool) -> Result<Value> {
    let open: Vec<ItemId> = s.state.open_sequences.iter().copied().filter(|i| s.project.sequence(*i).is_some()).collect();
    if open.len() < 2 {
        return Ok(json!({"sequence": s.state.active_sequence.map(|i| i.0)}));
    }
    let i = open.iter().position(|x| Some(*x) == s.state.active_sequence).unwrap_or(0);
    let j = if next { (i + 1) % open.len() } else { (i + open.len() - 1) % open.len() };
    s.execute("sequence.open", json!({"item": open[j].0}))?;
    Ok(json!({"sequence": open[j].0}))
}

// ------------------------------------------------------------------ clips

/// The fx badge: every standard effect of the clips off when any is on, else all on.
fn toggle_effect_stack(s: &mut Session, p: &Value) -> Result<Value> {
    let clips = with_links(s, &clips_p(s, p));
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let standard =
        |e: &redrob_film_project::EffectInstance| e.def().is_some_and(|d| !d.intrinsic) && !redrob_film_project::graphic::is_layer(e) && !e.essential;
    let any_on = clips.iter().filter_map(|c| q.find_item(*c)).any(|(_, it)| it.effects.iter().any(|e| standard(e) && e.enabled));
    let on = bool_p(p, "on").unwrap_or(!any_on);
    s.edit_sequence(if on { "Enable Clip Effects" } else { "Disable Clip Effects" }, |q, _, _| {
        for c in &clips {
            if let Some((_, it)) = q.find_item_mut(*c) {
                it.effects.iter_mut().filter(|e| standard(e)).for_each(|e| e.enabled = on);
            }
        }
        Ok(())
    })?;
    Ok(json!({"enabled": on}))
}

/// Clip Duration: a clip's position, source In and duration, given in ticks (`start`, `sourceIn`,
/// `duration`) or frames (`startFrame`, `sourceInFrame`, `durationFrames`). Linked partners follow
/// by the same changes. Refused when the clip would cover another clip or run past its media.
fn set_timing(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "clip.setTiming";
    let c = clip_p(p, "clip").or_else(|| s.state.selection.first().copied()).ok_or_else(|| bad(CMD, "need `clip`"))?;
    let rate = s.sequence_rate();
    let get = |k: &str| p.get(k).and_then(Value::as_i64).map(Tick).or_else(|| p.get(format!("{k}Frame")).and_then(Value::as_i64).map(|f| rate.tick_of(f)));
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = q.find_item(c).ok_or(edit::EditError::NoItem(c))?;
    let start = get("start").unwrap_or(it.start);
    let source_in = get("sourceIn").unwrap_or(it.source_in);
    let duration = p
        .get("duration")
        .and_then(Value::as_i64)
        .map(Tick)
        .or_else(|| p.get("durationFrames").and_then(Value::as_i64).map(|f| rate.tick_of(f)))
        .unwrap_or(it.duration);
    if start < Tick::ZERO || source_in < Tick::ZERO {
        return Err(bad(CMD, "the position and the source In cannot be negative"));
    }
    let (ds, di, dd) = (start - it.start, source_in - it.source_in, duration - it.duration);
    let clips = with_links(s, &[c]);
    s.edit_sequence("Clip Duration", |q, ctx, _| {
        for k in &clips {
            let (tid, cur) = q.find_item(*k).map(|(t, i)| (t, i.clone())).ok_or(edit::EditError::NoItem(*k))?;
            let mut it = cur.clone();
            it.start = cur.start + ds;
            it.source_in = cur.source_in + di;
            it.duration = cur.duration + dd;
            if it.duration < ctx.min_duration {
                return Err(edit::EditError::TooShort.into());
            }
            if it.start < Tick::ZERO || it.source_in < (ctx.media_start)(it.item) {
                return Err(edit::EditError::NoHandles.into());
            }
            if let Some(max) = (ctx.media_duration)(it.item)
                && it.frame_hold.is_none()
                && it.source_out() > max
            {
                return Err(edit::EditError::NoHandles.into());
            }
            let tr = q.track_mut(tid).ok_or(edit::EditError::NoTrack(tid))?;
            if tr.locked {
                return Err(edit::EditError::Locked.into());
            }
            if let Some(o) = tr.items.iter().find(|o| !clips.contains(&o.id) && o.start < it.end() && o.end() > it.start) {
                return Err(EngineError::Other(format!("the clip would overlap \u{201c}{}\u{201d}", o.name)));
            }
            if let Some(slot) = tr.item_mut(*k) {
                *slot = it;
            }
            tr.sort();
            edit::remove_orphan_transitions(tr);
        }
        Ok(())
    })?;
    Ok(json!({"clip": c.0, "start": start.0, "sourceIn": source_in.0, "duration": duration.0}))
}

/// Pitch compensation of a speed change: each audio clip gets (or updates) a Pitch Shifter whose
/// semitones undo its speed's pitch change (`-12·log2(speed)`, within the shifter's ±12 st, so
/// fully compensated from 50 % to 200 %). `on: false` removes it.
fn pitch_compensation(s: &mut Session, p: &Value) -> Result<Value> {
    let clips = with_links(s, &clips_p(s, p));
    let on = bool_p(p, "on").unwrap_or(true);
    let n = s.edit_sequence(if on { "Maintain Audio Pitch" } else { "Remove Pitch Compensation" }, |q, _, _| {
        let audio: Vec<ClipId> =
            clips.iter().copied().filter(|c| q.find_item(*c).and_then(|(t, _)| q.track(t)).is_some_and(|t| t.kind == TrackKind::Audio)).collect();
        let mut n = 0;
        for c in &audio {
            let Some((_, it)) = q.find_item_mut(*c) else { continue };
            let speed = it.speed.abs();
            let st = if speed > 0.0 { (-12.0 * speed.log2()).clamp(-12.0, 12.0) } else { 0.0 };
            let pos = it.effects.iter().position(|e| e.effect == PITCH_EFFECT);
            match (on, pos) {
                (true, Some(i)) => {
                    if let Some(prm) = it.effects[i].params.get_mut("semitones") {
                        *prm = Param::new(ParamValue::Float(st));
                    }
                }
                (true, None) => {
                    let Some(d) = redrob_film_project::find_effect(PITCH_EFFECT) else { continue };
                    let mut e = d.instance();
                    e.params.insert("semitones".into(), Param::new(ParamValue::Float(st)));
                    it.effects.push(e);
                }
                (false, Some(i)) => {
                    it.effects.remove(i);
                }
                (false, None) => continue,
            }
            n += 1;
        }
        Ok(n)
    })?;
    Ok(json!({"clips": n}))
}

/// The effect pitch compensation uses.
pub const PITCH_EFFECT: &str = "pitch_shifter";

// ------------------------------------------------------------------ registry

fn has_clip_or_selection(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if s.state.selection.is_empty() { Err("select a clip".into()) } else { Ok(()) }
}

pub fn commands() -> Vec<CommandSpec> {
    use TrackKind::{Audio, Video};
    macro_rules! pick {
        ($id:literal, $label:literal, $kind:expr, $n:literal) => {
            spec($id, $label, &[], None, "{}", has_seq, |s, _| select_nth(s, $kind, $n))
        };
    }
    const TRACK: &str = r#"{"track":"V1"|id?}"#;
    const TIME: &str = r#"{"time":ticks?}"#;
    vec![
        // ---- edit modes
        spec("timeline.editMode", "Timeline Edit Mode", &[], None, r#"{"mode":"normal|overwrite|insert"?}"#, always, |s, p| match str_p(p, "mode") {
            None => Ok(json!({"mode": s.state.edit_mode.name()})),
            Some(m) => {
                let mode = EditMode::from_name(m).ok_or_else(|| bad("timeline.editMode", format!("unknown mode `{m}` (normal, overwrite, insert)")))?;
                set_mode(s, mode)
            }
        }),
        spec("timeline.editMode.normal", "Normal Mode", &["Sequence", "Edit Mode"], None, "{}", always, |s, _| set_mode(s, EditMode::Normal)),
        spec("timeline.editMode.overwrite", "Overwrite Mode", &["Sequence", "Edit Mode"], None, "{}", always, |s, _| set_mode(s, EditMode::Overwrite)),
        spec("timeline.editMode.insert", "Insert Mode", &["Sequence", "Edit Mode"], None, "{}", always, |s, _| set_mode(s, EditMode::Insert)),
        spec("sequence.useTimelineZone", "Use Timeline Zone for Insert", &["Sequence"], None, r#"{"on":bool?}"#, always, |s, p| {
            let on = bool_p(p, "on").unwrap_or(s.state.ignore_timeline_zone);
            s.state.ignore_timeline_zone = !on;
            Ok(json!({"useTimelineZone": on}))
        }),
        // ---- the current track
        spec("timeline.selectTrack", "Select Track", &[], None, r#"{"track":"V1"|id}"#, has_seq, |s, p| {
            let t = track_p(s, p, "track", "timeline.selectTrack")?.ok_or_else(|| bad("timeline.selectTrack", "need `track`"))?;
            select_track(s, t)
        }),
        spec("timeline.trackUp", "Select Track Above", &[], None, "{}", has_seq, |s, _| step_track(s, true)),
        spec("timeline.trackDown", "Select Track Below", &[], None, "{}", has_seq, |s, _| step_track(s, false)),
        pick!("timeline.selectVideoTrack1", "Select Video Track 1", Video, 1),
        pick!("timeline.selectVideoTrack2", "Select Video Track 2", Video, 2),
        pick!("timeline.selectVideoTrack3", "Select Video Track 3", Video, 3),
        pick!("timeline.selectVideoTrack4", "Select Video Track 4", Video, 4),
        pick!("timeline.selectVideoTrack5", "Select Video Track 5", Video, 5),
        pick!("timeline.selectVideoTrack6", "Select Video Track 6", Video, 6),
        pick!("timeline.selectVideoTrack7", "Select Video Track 7", Video, 7),
        pick!("timeline.selectVideoTrack8", "Select Video Track 8", Video, 8),
        pick!("timeline.selectVideoTrack9", "Select Video Track 9", Video, 9),
        pick!("timeline.selectAudioTrack1", "Select Audio Track 1", Audio, 1),
        pick!("timeline.selectAudioTrack2", "Select Audio Track 2", Audio, 2),
        pick!("timeline.selectAudioTrack3", "Select Audio Track 3", Audio, 3),
        pick!("timeline.selectAudioTrack4", "Select Audio Track 4", Audio, 4),
        pick!("timeline.selectAudioTrack5", "Select Audio Track 5", Audio, 5),
        pick!("timeline.selectAudioTrack6", "Select Audio Track 6", Audio, 6),
        pick!("timeline.selectAudioTrack7", "Select Audio Track 7", Audio, 7),
        pick!("timeline.selectAudioTrack8", "Select Audio Track 8", Audio, 8),
        pick!("timeline.selectAudioTrack9", "Select Audio Track 9", Audio, 9),
        spec("timeline.toggleTargetCurrent", "Toggle Track Active", &[], None, TRACK, has_seq, toggle_target_current),
        spec("timeline.toggleAllTargets", "Toggle All Tracks Active", &[], None, "{}", has_seq, |s, _| all_targets(s, false)),
        spec("timeline.invertAllTargets", "Switch All Tracks Active", &[], None, "{}", has_seq, |s, _| all_targets(s, true)),
        spec("timeline.toggleSourceCurrent", "Toggle Track Target", &[], None, TRACK, has_seq, toggle_source_current),
        spec("timeline.cycleAudioTarget", "Switch Target Audio Track", &[], None, "{}", has_seq, |s, _| cycle_audio_target(s)),
        spec("timeline.toggleLockCurrent", "Toggle Track Lock", &[], None, TRACK, has_seq, |s, p| toggle_track_flag(s, p, "locked")),
        spec("timeline.toggleLockAll", "Toggle All Track Lock", &[], None, "{}", has_seq, |s, _| toggle_lock_all(s)),
        spec("timeline.toggleOutputCurrent", "Toggle Track Hidden / Mute", &[], None, TRACK, has_seq, |s, p| toggle_track_flag(s, p, "output")),
        spec("timeline.moveTrackUp", "Move Track Up", &[], None, TRACK, has_seq, |s, p| move_track(s, p, true)),
        spec("timeline.moveTrackDown", "Move Track Down", &[], None, TRACK, has_seq, |s, p| move_track(s, p, false)),
        // ---- selection at the playhead
        spec("timeline.addClipAtPlayhead", "Add Clip at Playhead to Selection", &[], None, "{}", has_seq, |s, _| add_clip_at_playhead(s, true)),
        spec("timeline.deselectClipAtPlayhead", "Deselect Clip at Playhead", &[], None, "{}", has_seq, |s, _| add_clip_at_playhead(s, false)),
        spec("timeline.selectTransitionAtPlayhead", "Select Transition at Playhead", &[], None, "{}", has_seq, |s, _| transition_at_playhead(s, 0)),
        spec("timeline.addTransitionAtPlayhead", "Add Transition at Playhead to Selection", &[], None, "{}", has_seq, |s, _| transition_at_playhead(s, 1)),
        spec("timeline.deselectTransitionAtPlayhead", "Deselect Transition at Playhead", &[], None, "{}", has_seq, |s, _| transition_at_playhead(s, 2)),
        spec("timeline.selectAllInTrack", "Select All in Current Track", &[], None, TRACK, has_seq, select_all_in_track),
        // ---- snap points
        spec("playhead.nextSnapPoint", "Go to Next Snap Point", &[], None, "{}", has_seq, |s, _| go_to_snap(s, true)),
        spec("playhead.prevSnapPoint", "Go to Previous Snap Point", &[], None, "{}", has_seq, |s, _| go_to_snap(s, false)),
        // ---- spaces
        spec("sequence.removeSpaceAllTracks", "Remove Space in All Tracks", &["Sequence"], None, TIME, has_seq, remove_space_all),
        spec(
            "sequence.removeAllSpacesAfter",
            "Remove All Spaces After Playhead",
            &["Sequence"],
            None,
            r#"{"time":ticks?,"track":"V1"|id?}"#,
            has_seq,
            remove_all_spaces_after,
        ),
        spec("sequence.removeAllClipsAfter", "Remove All Clips After Playhead", &["Sequence"], None, TIME, has_seq, remove_all_clips_after),
        spec(
            "sequence.insertSpace",
            "Insert Space…",
            &["Sequence"],
            None,
            r#"{"time":ticks?,"track":"V1"|id|"all"?,"duration":ticks|"durationFrames":n|"durationSeconds":f64}"#,
            has_seq,
            insert_space,
        ),
        spec("markers.identifyGaps", "Identify Gaps", &["Markers"], None, r#"{"track":"V1"|id|"all"?,"color":label?}"#, has_seq, identify_gaps),
        // ---- sequence switches and tabs
        spec("sequence.trackCompositing", "Track Compositing", &["Sequence"], None, r#"{"on":bool?}"#, has_seq, track_compositing),
        spec("sequence.disableTimelineEffects", "Disable Timeline Effects", &["Sequence"], None, r#"{"on":bool?}"#, has_seq, disable_effects),
        spec("sequence.nextTab", "Next Sequence Tab", &[], Some("Ctrl+Tab"), "{}", has_seq, |s, _| step_tab(s, true)),
        spec("sequence.prevTab", "Previous Sequence Tab", &[], Some("Ctrl+Shift+Tab"), "{}", has_seq, |s, _| step_tab(s, false)),
        // ---- clips
        spec("clip.toggleEffectStack", "Toggle Clip Effects", &[], None, r#"{"clips":[id]?,"on":bool?}"#, has_selection, toggle_effect_stack),
        spec(
            "clip.setTiming",
            "Clip Duration…",
            &["Clip"],
            None,
            r#"{"clip":id?,"start":ticks|"startFrame":n?,"sourceIn":ticks|"sourceInFrame":n?,"duration":ticks|"durationFrames":n?}"#,
            has_clip_or_selection,
            set_timing,
        ),
        spec("clip.pitchCompensation", "Maintain Audio Pitch", &[], None, r#"{"clips":[id]?,"on":bool=true}"#, has_selection, pitch_compensation),
        spec(
            "effects.setParamGroup",
            "Set Effect Parameter on Group",
            &[],
            None,
            r#"{"clip":id,"effect":id|index,"param":str,"value":any,"onlyEqual":bool?,"time":ticks?,"merge":bool?,"begin":bool?}"#,
            has_seq,
            set_param_group,
        ),
    ]
}
