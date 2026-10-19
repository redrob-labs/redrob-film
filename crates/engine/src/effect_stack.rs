//! Effect stack and keyframe tools (Kdenlive parity): reorder effects, switch a clip's whole
//! stack on or off, effect zones, and keyframe clipboard / bulk operations.
//!
//! * `effects.move` — move a standard effect up or down its clip's stack (render order). The
//!   intrinsic effects (Motion, Opacity, Volume…) keep their place.
//! * `effects.toggleStack` — switch every standard effect of the clips off or on at once (the
//!   clip's fx badge is struck through while its stack is off). Graphic layers are not effects
//!   here and stay on.
//! * `effects.setZone` — limit a video effect to a stretch of its clip (`in` / `out` in sequence
//!   time, stored as media time like keyframes) or clear the zone.
//! * `effects.exportKeyframes` — a parameter's keyframes as JSON, times relative to the clip's
//!   start (what Effect Controls copies to the clipboard).
//! * `effects.pasteKeyframes` — put keyframes on a parameter: at the playhead (the first keyframe
//!   lands there, the rest keep their spacing) or at their clip-relative times plus `offset`, with
//!   a value offset and a component mapping (X / Y of a point) for import across parameter kinds.
//! * `effects.shiftKeyframes` — move several keyframes (any parameters of one clip) by one delta.
//! * `effects.applyValueToKeyframes` — give selected keyframes one value (default: the value at
//!   the playhead).

use redrob_film_project::{ClipId, EffectInstance, Keyframe, ParamValue, TrackItem};
use redrob_film_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, clip_p, clips_p, f64_p, has_seq, json_to_param, str_p, time_p, u64_p};
use crate::{Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;

fn spec(id: &'static str, label: &'static str, params: &'static str, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: has_seq, run, journal: true }
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        spec("effects.move", "Move Effect", r#"{"clip":id,"index":n,"direction":"up"|"down"}"#, move_effect),
        spec("effects.toggleStack", "Toggle Effect Stack", r#"{"clips":[id]?|"clip":id?,"enabled":bool?}"#, toggle_stack),
        spec(
            "effects.setZone",
            "Set Effect Zone",
            r#"{"clip":id,"index":n,"in":ticks?,"out":ticks? (sequence time; default the playhead when given as null),"clear":bool?}"#,
            set_zone,
        ),
        CommandSpec {
            id: "effects.exportKeyframes",
            label: "Copy Keyframes to Clipboard",
            menu: &[],
            shortcut: None,
            params: r#"{"clip":id,"effect":str|index,"param":str,"mask":n?}"#,
            enabled: has_seq,
            run: export_keyframes,
            journal: false,
        },
        spec(
            "effects.pasteKeyframes",
            "Paste Keyframes",
            r#"{"clip":id,"effect":str|index,"param":str,"mask":n?,"keyframes":[keyframe]|"data":{"keyframes":[…]}|json text,"anchor":"playhead"|"clip"?="playhead","time":ticks?,"offset":ticks?,"valueOffset":num?,"component":"x"|"y"?}"#,
            paste_keyframes,
        ),
        spec(
            "effects.shiftKeyframes",
            "Move Keyframes",
            r#"{"clip":id,"keyframes":[{"effect":index,"param":str,"mask":n?,"mediaTime":ticks}],"delta":ticks (media time),"merge":bool?,"begin":bool?}"#,
            shift_keyframes,
        ),
        spec(
            "masks.trackToMotion",
            "Copy Mask Track to Position",
            r#"{"clip":id,"effect":index|id,"mask":n,"stabilize":bool? (move against the track, to hold the subject still)}"#,
            track_to_motion,
        ),
        spec(
            "effects.applyValueToKeyframes",
            "Apply Value to Keyframes",
            r#"{"clip":id,"effect":str|index,"param":str,"mask":n?,"mediaTimes":[ticks],"value":any? (default: the value at the playhead)}"#,
            apply_value,
        ),
    ]
}

/// The effect `eff` (index or id) of a clip.
fn effect_mut<'a>(it: &'a mut TrackItem, eff: &Value) -> Option<&'a mut EffectInstance> {
    match eff {
        Value::Number(n) => it.effects.get_mut(n.as_u64()? as usize),
        Value::String(id) => it.effects.iter_mut().find(|e| &e.effect == id),
        _ => None,
    }
}

fn is_standard(e: &EffectInstance) -> bool {
    !e.def().is_some_and(|d| d.intrinsic) && !redrob_film_project::graphic::is_layer(e)
}

fn move_effect(s: &mut Session, p: &Value) -> Result<Value> {
    let c = clip_p(p, "clip").ok_or_else(|| bad("effects.move", "need `clip`"))?;
    let idx = u64_p(p, "index").ok_or_else(|| bad("effects.move", "need `index`"))? as usize;
    let up = match str_p(p, "direction") {
        Some("up") => true,
        Some("down") => false,
        _ => return Err(bad("effects.move", "`direction` must be \"up\" or \"down\"")),
    };
    let to = s.edit_sequence("Move Effect", |q, _, _| {
        let (_, it) = q.find_item_mut(c).ok_or(redrob_film_edit::EditError::NoItem(c))?;
        let e = it.effects.get(idx).ok_or_else(|| bad("effects.move", "no such effect"))?;
        if !is_standard(e) {
            return Err(bad("effects.move", "fixed effects keep their place"));
        }
        // the neighbouring standard effect in that direction
        let other = if up { (0..idx).rev().find(|&i| is_standard(&it.effects[i])) } else { (idx + 1..it.effects.len()).find(|&i| is_standard(&it.effects[i])) };
        let other = other.ok_or_else(|| bad("effects.move", if up { "already the first effect" } else { "already the last effect" }))?;
        it.effects.swap(idx, other);
        Ok(other)
    })?;
    Ok(json!({"index": to}))
}

fn toggle_stack(s: &mut Session, p: &Value) -> Result<Value> {
    let mut clips: Vec<ClipId> = clip_p(p, "clip").into_iter().collect();
    if clips.is_empty() {
        clips = clips_p(s, p);
    }
    if clips.is_empty() {
        return Err(bad("effects.toggleStack", "select clips first"));
    }
    let want = bool_p(p, "enabled");
    let on = s.edit_sequence("Toggle Effect Stack", |q, _, _| {
        // without `enabled`: on when every standard effect of the clips is off, else off
        let any_on = clips.iter().filter_map(|c| q.find_item(*c)).any(|(_, it)| it.effects.iter().any(|e| is_standard(e) && e.enabled));
        let on = want.unwrap_or(!any_on);
        for c in &clips {
            if let Some((_, it)) = q.find_item_mut(*c) {
                for e in it.effects.iter_mut().filter(|e| is_standard(e)) {
                    e.enabled = on;
                }
            }
        }
        Ok(on)
    })?;
    Ok(json!({"enabled": on}))
}

fn set_zone(s: &mut Session, p: &Value) -> Result<Value> {
    let c = clip_p(p, "clip").ok_or_else(|| bad("effects.setZone", "need `clip`"))?;
    let idx = u64_p(p, "index").ok_or_else(|| bad("effects.setZone", "need `index`"))? as usize;
    let clear = bool_p(p, "clear").unwrap_or(false);
    let ph = s.playhead();
    // a key present with `null` means "at the playhead"
    let at = |k: &str| -> Option<Tick> { p.get(k).map(|v| v.as_i64().map(Tick).unwrap_or(ph)) };
    let (zin, zout) = (at("in"), at("out"));
    let zone = s.edit_sequence("Set Effect Zone", |q, _, _| {
        let (_, it) = q.find_item_mut(c).ok_or(redrob_film_edit::EditError::NoItem(c))?;
        let (lo, hi) = {
            let (a, b) = (it.source_in, it.source_out());
            (a.min(b), a.max(b))
        };
        // media time at a sequence time, within the clip (`out` is exclusive: the frame at the
        // playhead is the first one without the effect)
        let mt = |t: Tick, it: &TrackItem| it.effect_time_at(t.clamp(it.start, it.end()));
        let r: &TrackItem = &*it;
        let zin = zin.map(|t| mt(t, r));
        let zout = zout.map(|t| if t >= r.end() { if r.reverse { lo } else { hi } } else { mt(t, r) });
        let e = it.effects.get_mut(idx).ok_or_else(|| bad("effects.setZone", "no such effect"))?;
        if e.def().is_some_and(|d| d.kind != redrob_film_project::EffectKind::Video) {
            return Err(bad("effects.setZone", "only video effects have a zone"));
        }
        if clear {
            e.zone = None;
        } else {
            let (mut a, mut b) = e.zone.unwrap_or((lo, hi));
            if let Some(t) = zin {
                a = t;
            }
            if let Some(t) = zout {
                b = t;
            }
            // reversed clips run media time backwards: keep the zone ordered
            let (a, b) = (a.min(b), a.max(b));
            if b <= a {
                return Err(bad("effects.setZone", "the zone's out point must come after its in point"));
            }
            e.zone = Some((a, b));
        }
        Ok(e.zone)
    })?;
    Ok(match zone {
        Some((a, b)) => json!({"in": a.0, "out": b.0}),
        None => Value::Null,
    })
}

fn export_keyframes(s: &mut Session, p: &Value) -> Result<Value> {
    let c = clip_p(p, "clip").ok_or_else(|| bad("effects.exportKeyframes", "need `clip`"))?;
    let pid = str_p(p, "param").ok_or_else(|| bad("effects.exportKeyframes", "need `param`"))?;
    let eff = p.get("effect").cloned().unwrap_or(json!("motion"));
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let (_, it) = q.find_item(c).ok_or_else(|| bad("effects.exportKeyframes", "no such clip"))?;
    let mut it = it.clone();
    let source_in = it.source_in;
    let e = effect_mut(&mut it, &eff).ok_or_else(|| bad("effects.exportKeyframes", "no such effect"))?;
    let effect = e.effect.clone();
    let prm = crate::masks::target_param(e, p, pid).ok_or_else(|| bad("effects.exportKeyframes", "no such param"))?;
    let keyframes: Vec<Keyframe> = prm
        .keyframes
        .iter()
        .map(|k| {
            let mut k = k.clone();
            k.time -= source_in;
            k
        })
        .collect();
    Ok(json!({"format": "redrob-film-keyframes", "effect": effect, "param": pid, "keyframes": keyframes}))
}

/// The keyframes a paste carries: `keyframes`, or `data` (an export, as an object or as text).
fn pasted(p: &Value) -> Result<Vec<Keyframe>> {
    let raw = match (p.get("keyframes"), p.get("data")) {
        (Some(k), _) => k.clone(),
        (None, Some(Value::String(text))) => {
            let v: Value = serde_json::from_str(text.trim()).map_err(|e| bad("effects.pasteKeyframes", format!("not keyframe data: {e}")))?;
            v.get("keyframes").cloned().unwrap_or(v)
        }
        (None, Some(d)) => d.get("keyframes").cloned().unwrap_or_else(|| d.clone()),
        (None, None) => return Err(bad("effects.pasteKeyframes", "need `keyframes` or `data`")),
    };
    let mut ks: Vec<Keyframe> = serde_json::from_value(raw).map_err(|e| bad("effects.pasteKeyframes", format!("not keyframe data: {e}")))?;
    ks.sort_by_key(|k| k.time);
    if ks.is_empty() {
        return Err(bad("effects.pasteKeyframes", "no keyframes to paste"));
    }
    Ok(ks)
}

/// A pasted value for a parameter whose value is `template`: the same kind as is, a point's X or
/// Y for a scalar, a scalar into a point's X or Y (the other coordinate kept); plus `offset`.
fn map_value(template: &ParamValue, v: &ParamValue, component_y: bool, offset: f64) -> Option<ParamValue> {
    use redrob_film_geom::Vec2;
    Some(match (template, v) {
        (ParamValue::Float(_), ParamValue::Float(x)) => ParamValue::Float(x + offset),
        (ParamValue::Float(_), ParamValue::Vec2(pt)) => ParamValue::Float(if component_y { pt.y } else { pt.x } + offset),
        (ParamValue::Vec2(_), ParamValue::Vec2(pt)) => ParamValue::Vec2(Vec2::new(pt.x + offset, pt.y + offset)),
        (ParamValue::Vec2(t), ParamValue::Float(x)) => {
            if component_y {
                ParamValue::Vec2(Vec2::new(t.x, x + offset))
            } else {
                ParamValue::Vec2(Vec2::new(x + offset, t.y))
            }
        }
        (a, b) if std::mem::discriminant(a) == std::mem::discriminant(b) => b.clone(),
        _ => return None,
    })
}

fn paste_keyframes(s: &mut Session, p: &Value) -> Result<Value> {
    let c = clip_p(p, "clip").ok_or_else(|| bad("effects.pasteKeyframes", "need `clip`"))?;
    let pid = str_p(p, "param").ok_or_else(|| bad("effects.pasteKeyframes", "need `param`"))?.to_string();
    let eff = p.get("effect").cloned().unwrap_or(json!("motion"));
    let ks = pasted(p)?;
    let at_clip = str_p(p, "anchor") == Some("clip");
    let offset = p.get("offset").and_then(Value::as_i64).map(Tick).unwrap_or(Tick::ZERO);
    let value_offset = f64_p(p, "valueOffset").unwrap_or(0.0);
    let component_y = str_p(p, "component").is_some_and(|c| c.eq_ignore_ascii_case("y"));
    let tl = time_p(s, p, "").unwrap_or(s.playhead());
    let pq = p.clone();
    let n = s.edit_sequence("Paste Keyframes", |q, _, _| {
        let (_, it) = q.find_item_mut(c).ok_or(redrob_film_edit::EditError::NoItem(c))?;
        let base = if at_clip { it.source_in } else { it.effect_time_at(tl.clamp(it.start, (it.end() - Tick(1)).max(it.start))) - ks[0].time };
        let e = effect_mut(it, &eff).ok_or_else(|| bad("effects.pasteKeyframes", "no such effect"))?;
        let animatable = pq.get("mask").is_some_and(|m| !m.is_null()) || e.def().and_then(|d| d.param(&pid)).is_none_or(|d| d.animatable);
        if !animatable {
            return Err(bad("effects.pasteKeyframes", format!("`{pid}` cannot be animated")));
        }
        if !e.params.contains_key(&pid)
            && let Some(d) = e.def().and_then(|d| d.param(&pid))
        {
            e.params.insert(pid.clone(), redrob_film_project::Param::new(d.default.clone()));
        }
        let prm = crate::masks::target_param(e, &pq, &pid).ok_or_else(|| bad("effects.pasteKeyframes", "no such param"))?;
        let mut n = 0;
        for k in &ks {
            let at = base + k.time + offset;
            let template = prm.value_at(at);
            let Some(v) = map_value(&template, &k.value, component_y, value_offset) else {
                return Err(bad("effects.pasteKeyframes", "the keyframes do not fit this parameter"));
            };
            prm.put_keyframe(at, v);
            if let Some(dst) = prm.keyframes.iter_mut().find(|x| x.time == at) {
                dst.interp = k.interp;
                dst.in_influence = k.in_influence;
                dst.out_influence = k.out_influence;
            }
            n += 1;
        }
        Ok(n)
    })?;
    Ok(json!({"pasted": n}))
}

fn shift_keyframes(s: &mut Session, p: &Value) -> Result<Value> {
    let c = clip_p(p, "clip").ok_or_else(|| bad("effects.shiftKeyframes", "need `clip`"))?;
    let delta = p.get("delta").and_then(Value::as_i64).map(Tick).ok_or_else(|| bad("effects.shiftKeyframes", "need `delta`"))?;
    let refs: Vec<Value> = p.get("keyframes").and_then(Value::as_array).cloned().unwrap_or_default();
    if refs.is_empty() || delta == Tick::ZERO {
        return Ok(json!({"moved": 0}));
    }
    let merge = bool_p(p, "merge").unwrap_or(false).then(|| format!("shiftKeyframes:{}", c.0));
    if bool_p(p, "begin").unwrap_or(false) {
        s.history.merge_key = None;
    }
    let n = s.edit_sequence_as("Move Keyframes", merge.as_deref(), |q, _, _| {
        let (_, it) = q.find_item_mut(c).ok_or(redrob_film_edit::EditError::NoItem(c))?;
        // group the moves per parameter, so a keyframe never lands on one still to move
        let mut groups: Vec<(Value, String, Option<u64>, Vec<Tick>)> = Vec::new();
        for r in &refs {
            let eff = r.get("effect").cloned().unwrap_or(json!(0));
            let pid = str_p(r, "param").ok_or_else(|| bad("effects.shiftKeyframes", "each keyframe needs `param`"))?.to_string();
            let mask = u64_p(r, "mask");
            let t = r.get("mediaTime").and_then(Value::as_i64).map(Tick).ok_or_else(|| bad("effects.shiftKeyframes", "each keyframe needs `mediaTime`"))?;
            match groups.iter_mut().find(|g| g.0 == eff && g.1 == pid && g.2 == mask) {
                Some(g) => g.3.push(t),
                None => groups.push((eff, pid, mask, vec![t])),
            }
        }
        let mut n = 0;
        for (eff, pid, mask, times) in groups {
            let e = effect_mut(it, &eff).ok_or_else(|| bad("effects.shiftKeyframes", "no such effect"))?;
            let sel = json!({"mask": mask});
            let prm = crate::masks::target_param(e, &sel, &pid).ok_or_else(|| bad("effects.shiftKeyframes", "no such param"))?;
            let (moving, mut rest): (Vec<Keyframe>, Vec<Keyframe>) = prm.keyframes.drain(..).partition(|k| times.contains(&k.time));
            for mut k in moving {
                k.time += delta;
                // a moved keyframe replaces one that sat where it lands
                rest.retain(|x| x.time != k.time);
                rest.push(k);
                n += 1;
            }
            rest.sort_by_key(|k| k.time);
            prm.keyframes = rest;
        }
        Ok(n)
    })?;
    Ok(json!({"moved": n}))
}

/// Kdenlive's "copy the tracker's keyframes into Transform": the clip's Motion Position follows
/// the centre of a tracked mask (one Position keyframe per mask path keyframe, moved by how far the
/// mask's centre moved since its first keyframe, in sequence pixels at the clip's scale), or moves
/// against it to hold the subject still (`stabilize`).
fn track_to_motion(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "masks.trackToMotion";
    let c = clip_p(p, "clip").ok_or_else(|| bad(CMD, "need `clip`"))?;
    let eff = p.get("effect").cloned().unwrap_or(json!("opacity"));
    let mi = u64_p(p, "mask").ok_or_else(|| bad(CMD, "need `mask`"))? as usize;
    let stabilize = bool_p(p, "stabilize").unwrap_or(false);
    let (fw, fh) = s.active_sequence().map(|q| (q.settings.width as f64, q.settings.height as f64)).unwrap_or((1920.0, 1080.0));
    let n = s.edit_sequence("Copy Mask Track to Position", |q, _, _| {
        let (_, it) = q.find_item_mut(c).ok_or(redrob_film_edit::EditError::NoItem(c))?;
        let e = effect_mut(it, &eff).ok_or_else(|| bad(CMD, "no such effect"))?;
        let mask = e.masks.get(mi).ok_or_else(|| bad(CMD, format!("no mask {mi}")))?;
        let centres: Vec<(Tick, redrob_film_geom::Vec2)> = mask
            .path
            .keyframes
            .iter()
            .filter_map(|k| {
                let path = k.value.as_path()?;
                let n = path.vertices.len().max(1) as f64;
                let (x, y) = path.vertices.iter().fold((0.0, 0.0), |(x, y), v| (x + v.p.x, y + v.p.y));
                Some((k.time, redrob_film_geom::Vec2::new(x / n, y / n)))
            })
            .collect();
        if centres.len() < 2 {
            return Err(bad(CMD, "track the mask first (it needs path keyframes)"));
        }
        let motion = it.effects.iter_mut().find(|e| e.effect == "motion").ok_or_else(|| bad(CMD, "the clip has no Motion"))?;
        let scale_at = |m: &EffectInstance, t: Tick| m.f64_at("scale", t) / 100.0;
        let (t0, c0) = centres[0];
        let base = motion.vec2_at("position", t0);
        let base = redrob_film_geom::Vec2::new(if base.x.is_finite() { base.x } else { fw / 2.0 }, if base.y.is_finite() { base.y } else { fh / 2.0 });
        let sign = if stabilize { -1.0 } else { 1.0 };
        let moves: Vec<(Tick, redrob_film_geom::Vec2)> = centres
            .iter()
            .map(|(t, cc)| {
                let k = scale_at(&*motion, *t) * sign;
                (*t, redrob_film_geom::Vec2::new(base.x + (cc.x - c0.x) * k, base.y + (cc.y - c0.y) * k))
            })
            .collect();
        let prm = motion.params.entry("position".to_string()).or_insert_with(|| redrob_film_project::Param::new(ParamValue::Vec2(base)));
        for (t, v) in &moves {
            prm.put_keyframe(*t, ParamValue::Vec2(*v));
        }
        Ok(moves.len())
    })?;
    Ok(json!({"keyframes": n}))
}

fn apply_value(s: &mut Session, p: &Value) -> Result<Value> {
    let c = clip_p(p, "clip").ok_or_else(|| bad("effects.applyValueToKeyframes", "need `clip`"))?;
    let pid = str_p(p, "param").ok_or_else(|| bad("effects.applyValueToKeyframes", "need `param`"))?.to_string();
    let eff = p.get("effect").cloned().unwrap_or(json!("motion"));
    let times: Vec<Tick> = p.get("mediaTimes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).map(Tick).collect()).unwrap_or_default();
    let ph = s.playhead();
    let pq = p.clone();
    let n = s.edit_sequence("Apply Value to Keyframes", |q, _, _| {
        let (_, it) = q.find_item_mut(c).ok_or(redrob_film_edit::EditError::NoItem(c))?;
        let mt = it.effect_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
        let e = effect_mut(it, &eff).ok_or_else(|| bad("effects.applyValueToKeyframes", "no such effect"))?;
        let prm = crate::masks::target_param(e, &pq, &pid).ok_or_else(|| bad("effects.applyValueToKeyframes", "no such param"))?;
        let v = match pq.get("value") {
            Some(v) => json_to_param(&prm.value, v).ok_or_else(|| bad("effects.applyValueToKeyframes", "value has the wrong type"))?,
            None => prm.value_at(mt),
        };
        let mut n = 0;
        for k in prm.keyframes.iter_mut().filter(|k| times.contains(&k.time)) {
            k.value = v.clone();
            n += 1;
        }
        Ok(n)
    })?;
    Ok(json!({"keyframes": n}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> (Session, u64) {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        (s, clip)
    }

    fn item(s: &Session, clip: u64) -> TrackItem {
        s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1.clone()
    }

    #[test]
    fn effects_move_within_the_standard_stack() {
        let (mut s, clip) = demo();
        s.execute("effects.apply", json!({"clips": [clip], "effect": "gaussian_blur"})).unwrap();
        s.execute("effects.apply", json!({"clips": [clip], "effect": "tint"})).unwrap();
        let names = |s: &Session| item(s, clip).effects.iter().map(|e| e.effect.clone()).collect::<Vec<_>>();
        let before = names(&s);
        let blur = before.iter().position(|e| e == "gaussian_blur").unwrap();
        let tint = before.iter().position(|e| e == "tint").unwrap();
        let r = s.execute("effects.move", json!({"clip": clip, "index": tint, "direction": "up"})).unwrap();
        assert_eq!(r["index"], json!(blur));
        assert_eq!(names(&s)[blur], "tint");
        assert!(s.execute("effects.move", json!({"clip": clip, "index": blur, "direction": "up"})).is_err(), "already first");
        s.undo().unwrap();
        assert_eq!(names(&s), before);
    }

    #[test]
    fn the_stack_switches_off_and_on() {
        let (mut s, clip) = demo();
        s.execute("effects.apply", json!({"clips": [clip], "effect": "gaussian_blur"})).unwrap();
        assert_eq!(s.execute("effects.toggleStack", json!({"clip": clip})).unwrap()["enabled"], json!(false));
        assert!(item(&s, clip).effects.iter().filter(|e| is_standard(e)).all(|e| !e.enabled));
        assert!(item(&s, clip).effect("motion").unwrap().enabled, "fixed effects stay on");
        assert_eq!(s.execute("effects.toggleStack", json!({"clip": clip})).unwrap()["enabled"], json!(true));
    }

    #[test]
    fn keyframes_copy_paste_shift_and_apply() {
        let (mut s, clip) = demo();
        let it = item(&s, clip);
        s.execute("playhead.set", json!({"time": it.start.0})).unwrap();
        s.execute("effects.setParam", json!({"clip": clip, "effect": "opacity", "param": "opacity", "value": 20.0, "keyframe": true})).unwrap();
        let later = it.start + Tick(it.duration.0 / 2);
        s.execute("effects.setParam", json!({"clip": clip, "effect": "opacity", "param": "opacity", "value": 80.0, "keyframe": true, "time": later.0}))
            .unwrap();
        let data = s.execute("effects.exportKeyframes", json!({"clip": clip, "effect": "opacity", "param": "opacity"})).unwrap();
        assert_eq!(data["keyframes"].as_array().unwrap().len(), 2);
        // paste onto Scale (a scalar) at the clip's own times, 5 higher
        s.execute(
            "effects.pasteKeyframes",
            json!({"clip": clip, "effect": "motion", "param": "scale", "data": data.to_string(), "anchor": "clip", "valueOffset": 5.0}),
        )
        .unwrap();
        let scale = item(&s, clip).effect("motion").unwrap().params["scale"].clone();
        assert_eq!(scale.keyframes.len(), 2);
        assert_eq!(scale.keyframes[0].value, ParamValue::Float(25.0));
        // move both by one second, then give them one value
        let times: Vec<Value> = scale.keyframes.iter().map(|k| json!({"effect": "motion", "param": "scale", "mediaTime": k.time.0})).collect();
        let sec = redrob_film_time::TICKS_PER_SECOND;
        assert_eq!(s.execute("effects.shiftKeyframes", json!({"clip": clip, "keyframes": times, "delta": sec})).unwrap()["moved"], json!(2));
        let moved = item(&s, clip).effect("motion").unwrap().params["scale"].clone();
        assert_eq!(moved.keyframes[0].time, scale.keyframes[0].time + Tick(sec));
        let mts: Vec<i64> = moved.keyframes.iter().map(|k| k.time.0).collect();
        s.execute("effects.applyValueToKeyframes", json!({"clip": clip, "effect": "motion", "param": "scale", "mediaTimes": mts, "value": 50.0})).unwrap();
        assert!(item(&s, clip).effect("motion").unwrap().params["scale"].keyframes.iter().all(|k| k.value == ParamValue::Float(50.0)));
    }

    #[test]
    fn an_effect_zone_limits_the_effect() {
        let (mut s, clip) = demo();
        s.execute("effects.apply", json!({"clips": [clip], "effect": "gaussian_blur"})).unwrap();
        let it = item(&s, clip);
        let idx = it.effects.iter().position(|e| e.effect == "gaussian_blur").unwrap();
        let mid = it.start + Tick(it.duration.0 / 2);
        s.execute("effects.setZone", json!({"clip": clip, "index": idx, "in": mid.0})).unwrap();
        let e = item(&s, clip).effects[idx].clone();
        assert!(!e.in_zone(it.source_in) && e.in_zone(it.effect_time_at(mid)));
        s.execute("effects.setZone", json!({"clip": clip, "index": idx, "clear": true})).unwrap();
        assert!(item(&s, clip).effects[idx].zone.is_none());
    }
}
