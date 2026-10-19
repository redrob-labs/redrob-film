//! Timeline marker tools (Kdenlive parity): categories, add/remove at the playhead, add by
//! category, add at an interval, delete several, lock, export as chapter text / JSON and import,
//! and moving bin clip markers from the Timeline.
//!
//! | Id | What |
//! |---|---|
//! | `markers.toggleAtPlayhead` | Add/Remove Timeline Marker: removes the marker under the playhead, else adds one |
//! | `markers.addCategory1` … `markers.addCategory10` | a marker of project category N at the playhead |
//! | `markers.addInterval` | markers every `interval` frames from `from` (playhead) to `to` (sequence end) or `count` times |
//! | `markers.delete` | delete the markers named in `markers` |
//! | `markers.lock` | Timeline Markers Locked: locked markers stay put when clips ripple (= Ripple Sequence Markers off) and cannot be dragged |
//! | `markers.categories` | read or replace the project's categories and the default one |
//! | `markers.deleteCategory` | remove a category, moving its markers to another category (or deleting them) |
//! | `markers.exportChapters` | chapter text from a format (`{{timecode}}`, `{{comment}}`, `{{frame}}`, `{{index}}`, `{{nexttimecode}}`, `{{realtimecode}}`) or JSON, with an offset; optional file |
//! | `markers.import` | import markers from that JSON or from chapter text (`0:00 Intro` lines) |
//! | `markers.moveClipMarker` | move or resize a marker of a bin clip (media time) |
//!
//! Categories are marker colours with project-specific names (Project Settings ▸ Markers).

use redrob_film_project::{ItemId, ItemKind, Label, Marker, MarkerCategory, MarkerId, MarkerKind};
use redrob_film_time::{Tick, format_timecode_frames};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, has_seq, str_p, time_p, u64_p};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;

fn spec(id: &'static str, label: &'static str, menu: &'static [&'static str], shortcut: Option<&'static str>, params: &'static str, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut, params, enabled: has_seq, run, journal: true }
}

/// The default chapter format: `{{timecode}} {{comment}}`.
pub const DEFAULT_CHAPTER_FORMAT: &str = "{{timecode}} {{comment}}";

macro_rules! category_cmds {
    ($($n:literal => $id:literal, $label:literal, $f:ident;)*) => {
        $(fn $f(s: &mut Session, p: &Value) -> Result<Value> { add_category(s, p, $n) })*
        fn category_specs() -> Vec<CommandSpec> {
            vec![$(spec($id, $label, &[], None, r#"{"time":ticks?,"target":"program|source"}"#, $f)),*]
        }
    };
}

category_cmds! {
    1 => "markers.addCategory1", "Add Marker of Category 1", cat1;
    2 => "markers.addCategory2", "Add Marker of Category 2", cat2;
    3 => "markers.addCategory3", "Add Marker of Category 3", cat3;
    4 => "markers.addCategory4", "Add Marker of Category 4", cat4;
    5 => "markers.addCategory5", "Add Marker of Category 5", cat5;
    6 => "markers.addCategory6", "Add Marker of Category 6", cat6;
    7 => "markers.addCategory7", "Add Marker of Category 7", cat7;
    8 => "markers.addCategory8", "Add Marker of Category 8", cat8;
    9 => "markers.addCategory9", "Add Marker of Category 9", cat9;
    10 => "markers.addCategory10", "Add Marker of Category 10", cat10;
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    let mut v = vec![
        spec("markers.toggleAtPlayhead", "Add/Remove Timeline Marker", &["Markers"], None, r#"{"time":ticks?}"#, toggle_at_playhead),
        spec(
            "markers.addInterval",
            "Add Markers at Interval",
            &[],
            None,
            r#"{"intervalFrames":i64,"from":ticks?,"to":ticks?,"count":n?,"color":label?,"name":str?}"#,
            add_interval,
        ),
        spec("markers.delete", "Delete Markers", &[], None, r#"{"markers":[id]}"#, delete),
        CommandSpec {
            id: "markers.lock",
            label: "Timeline Markers Locked",
            menu: &["Markers"],
            shortcut: None,
            params: r#"{"on":bool?}"#,
            enabled: always,
            run: lock,
            journal: true,
        },
        CommandSpec {
            id: "markers.categories",
            label: "Marker Categories",
            menu: &[],
            shortcut: None,
            params: r#"{"categories":[{"color":label,"name":str}]?,"default":label|null?} (no params: read them)"#,
            enabled: always,
            run: categories,
            journal: true,
        },
        CommandSpec {
            id: "markers.deleteCategory",
            label: "Delete Marker Category",
            menu: &[],
            shortcut: None,
            params: r#"{"color":label,"reassign":label?|"deleteMarkers":bool?}"#,
            enabled: always,
            run: delete_category,
            journal: true,
        },
        spec(
            "markers.exportChapters",
            "Export Markers…",
            &["Markers"],
            None,
            r#"{"format":str?,"json":bool?,"offsetFrames":i64?,"color":label?,"path":str?} -> {"text","warnings"}"#,
            export_chapters,
        ),
        spec("markers.import", "Import Markers…", &["Markers"], None, r#"{"path":str?|"text":str?,"offsetFrames":i64?}"#, import),
        CommandSpec {
            id: "markers.moveClipMarker",
            label: "Move Clip Marker",
            menu: &[],
            shortcut: None,
            params: r#"{"item":id,"marker":id,"time":mediaTicks?,"duration":ticks?,"name":str?,"comment":str?,"color":label?}"#,
            enabled: always,
            run: move_clip_marker,
            journal: true,
        },
        CommandSpec {
            id: "markers.deleteClipMarker",
            label: "Delete Clip Marker",
            menu: &[],
            shortcut: None,
            params: r#"{"item":id,"marker":id}"#,
            enabled: always,
            run: delete_clip_marker,
            journal: true,
        },
    ];
    v.extend(category_specs());
    v
}

fn add_category(s: &mut Session, p: &Value, n: usize) -> Result<Value> {
    let id = "markers.addCategory";
    let color = s.project.settings.marker_category(n).ok_or_else(|| bad(id, format!("the project has no category {n}")))?;
    let mut q = p.clone();
    if !q.is_object() {
        q = json!({});
    }
    q["color"] = json!(color.name());
    s.execute("markers.add", q)
}

fn toggle_at_playhead(s: &mut Session, p: &Value) -> Result<Value> {
    let t = time_p(s, p, "").unwrap_or(s.playhead());
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    let at = rate.frame_at(t);
    if let Some(m) = q.markers.iter().find(|m| rate.frame_at(m.start) == at).map(|m| m.id) {
        s.edit_sequence("Remove Marker", |q, _, _| {
            q.markers.retain(|x| x.id != m);
            Ok(())
        })?;
        return Ok(json!({"removed": m.0}));
    }
    s.execute("markers.add", json!({"time": t.0}))
}

fn add_interval(s: &mut Session, p: &Value) -> Result<Value> {
    let id = "markers.addInterval";
    let rate = s.sequence_rate();
    let step = p.get("intervalFrames").and_then(Value::as_i64).filter(|n| *n > 0).ok_or_else(|| bad(id, "need a positive `intervalFrames`"))?;
    let step = rate.tick_of(step);
    let end_of_seq = s.active_sequence().ok_or(EngineError::NoSequence)?.duration();
    let from = p.get("from").and_then(Value::as_i64).map(Tick).unwrap_or(s.playhead()).max(Tick::ZERO);
    let count = u64_p(p, "count").map(|c| c.min(10_000) as usize);
    let to = p.get("to").and_then(Value::as_i64).map(Tick).unwrap_or(end_of_seq);
    let color = str_p(p, "color").and_then(Label::from_name).unwrap_or_else(|| s.project.settings.default_marker_color());
    let name = str_p(p, "name").unwrap_or("").to_string();
    let mut times = Vec::new();
    let mut t = from;
    while times.len() < count.unwrap_or(10_000) && (count.is_some() || t <= to) {
        times.push(t);
        t += step;
    }
    if times.is_empty() {
        return Err(bad(id, "no marker fits between `from` and `to`"));
    }
    let ids = s.edit_sequence("Add Markers", |q, ctx, _| {
        let mut ids = Vec::new();
        for (i, t) in times.iter().enumerate() {
            let mid = MarkerId(ctx.alloc());
            let n = if name.is_empty() { String::new() } else { format!("{name} {}", i + 1) };
            q.markers.push(Marker { id: mid, start: *t, duration: Tick::ZERO, name: n, comment: String::new(), kind: MarkerKind::Comment, color });
            ids.push(mid.0);
        }
        q.markers.sort_by_key(|m| m.start);
        Ok(ids)
    })?;
    Ok(json!({"markers": ids}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let ids: Vec<u64> = p.get("markers").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    if ids.is_empty() {
        return Err(bad("markers.delete", "need `markers`"));
    }
    let n = s.edit_sequence("Delete Markers", |q, _, _| {
        let before = q.markers.len();
        q.markers.retain(|m| !ids.contains(&m.id.0));
        Ok(before - q.markers.len())
    })?;
    Ok(json!({"deleted": n}))
}

fn lock(s: &mut Session, p: &Value) -> Result<Value> {
    let locked = bool_p(p, "on").unwrap_or(s.state.ripple_sequence_markers);
    s.state.ripple_sequence_markers = !locked;
    Ok(json!({"locked": locked}))
}

/// Whether timeline markers are locked (they stay put when clips move, and do not drag).
pub fn markers_locked(s: &Session) -> bool {
    !s.state.ripple_sequence_markers
}

fn categories_json(s: &Session) -> Value {
    let st = &s.project.settings;
    json!({
        "categories": st.marker_categories().iter().enumerate().map(|(i, c)| json!({"index": i + 1, "color": c.color.name(), "name": c.name})).collect::<Vec<_>>(),
        "default": st.default_marker_color().name(),
    })
}

fn categories(s: &mut Session, p: &Value) -> Result<Value> {
    let id = "markers.categories";
    let list = match p.get("categories").and_then(Value::as_array) {
        None => None,
        Some(a) => {
            let mut out: Vec<MarkerCategory> = Vec::new();
            for c in a {
                let n = str_p(c, "color").unwrap_or_default();
                let color = Label::from_name(n).ok_or_else(|| bad(id, format!("unknown colour `{n}`")))?;
                if out.iter().any(|x| x.color == color) {
                    return Err(bad(id, format!("two categories use {n}")));
                }
                let name = str_p(c, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or(color.name()).to_string();
                out.push(MarkerCategory { color, name });
            }
            if out.is_empty() {
                return Err(bad(id, "a project keeps at least one category"));
            }
            Some(out)
        }
    };
    let default = match p.get("default") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => {
            let n = v.as_str().unwrap_or_default();
            Some(Some(Label::from_name(n).ok_or_else(|| bad(id, format!("unknown colour `{n}`")))?))
        }
    };
    if list.is_none() && default.is_none() {
        return Ok(categories_json(s));
    }
    s.edit("Edit Marker Categories", |pr, _| {
        if let Some(l) = list {
            pr.settings.marker_categories = l;
        }
        if let Some(d) = default {
            pr.settings.default_marker_category = d;
        }
        // the default must still be a category
        let cats = pr.settings.marker_categories();
        if pr.settings.default_marker_category.is_some_and(|d| !cats.iter().any(|c| c.color == d)) {
            pr.settings.default_marker_category = None;
        }
        Ok(())
    })?;
    Ok(categories_json(s))
}

fn delete_category(s: &mut Session, p: &Value) -> Result<Value> {
    let id = "markers.deleteCategory";
    let n = str_p(p, "color").unwrap_or_default();
    let color = Label::from_name(n).ok_or_else(|| bad(id, format!("unknown colour `{n}`")))?;
    let mut cats = s.project.settings.marker_categories();
    if !cats.iter().any(|c| c.color == color) {
        return Err(bad(id, format!("no category uses {n}")));
    }
    cats.retain(|c| c.color != color);
    if cats.is_empty() {
        return Err(bad(id, "a project keeps at least one category"));
    }
    let reassign = match str_p(p, "reassign") {
        Some(r) => {
            let l = Label::from_name(r).ok_or_else(|| bad(id, format!("unknown colour `{r}`")))?;
            if !cats.iter().any(|c| c.color == l) {
                return Err(bad(id, "markers can only move to another category"));
            }
            Some(l)
        }
        None => None,
    };
    let drop_markers = bool_p(p, "deleteMarkers").unwrap_or(false);
    let used = s.project.sequences().filter_map(|i| i.as_sequence()).map(|q| q.markers.iter().filter(|m| m.color == color).count()).sum::<usize>();
    if used > 0 && reassign.is_none() && !drop_markers {
        return Err(bad(id, format!("{used} markers use this category: pass `reassign` (another category) or `deleteMarkers`")));
    }
    let seqs: Vec<ItemId> = s.project.sequences().map(|i| i.id).collect();
    s.edit("Delete Marker Category", |pr, _| {
        for sid in &seqs {
            if let Some(it) = pr.item_mut(*sid)
                && let ItemKind::Sequence(q) = &mut it.kind
            {
                let q = std::sync::Arc::make_mut(q);
                match reassign {
                    Some(l) => q.markers.iter_mut().filter(|m| m.color == color).for_each(|m| m.color = l),
                    None => q.markers.retain(|m| m.color != color),
                }
            }
        }
        if pr.settings.default_marker_category == Some(color) {
            pr.settings.default_marker_category = None;
        }
        pr.settings.marker_categories = cats;
        Ok(())
    })?;
    Ok(json!({"markers": used}))
}

/// `M:SS`, or `H:MM:SS` from one hour on (YouTube's chapter notation).
fn chapter_time(secs: i64) -> String {
    let secs = secs.max(0);
    if secs >= 3600 { format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60) } else { format!("{}:{:02}", secs / 60, secs % 60) }
}

/// The chapter text (or JSON) of the active sequence's markers.
pub fn chapters(s: &Session, p: &Value) -> Result<(String, Vec<String>)> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    let offset = p.get("offsetFrames").and_then(Value::as_i64).unwrap_or(0);
    let filter = str_p(p, "color").and_then(Label::from_name);
    let mut ms: Vec<Marker> = q.markers.iter().filter(|m| filter.is_none_or(|c| m.color == c)).cloned().collect();
    ms.sort_by_key(|m| (m.start, m.id));
    let frame = |m: &Marker| rate.frame_at(m.start) + offset;
    let secs = |f: i64| f * rate.den / rate.num.max(1);
    if bool_p(p, "json").unwrap_or(false) {
        let v: Vec<Value> = ms
            .iter()
            .map(|m| json!({"position": frame(m), "seconds": secs(frame(m)), "comment": if m.name.is_empty() { &m.comment } else { &m.name }, "type": s.project.settings.marker_category_name(m.color), "color": m.color.name(), "duration": rate.frame_at(m.duration)}))
            .collect();
        return Ok((serde_json::to_string_pretty(&v).unwrap_or_default(), Vec::new()));
    }
    let fmt = str_p(p, "format").filter(|f| !f.trim().is_empty()).unwrap_or(DEFAULT_CHAPTER_FORMAT);
    let mut lines = Vec::new();
    for (i, m) in ms.iter().enumerate() {
        let f = frame(m);
        let next = ms.get(i + 1).map(|n| chapter_time(secs(frame(n)))).unwrap_or_else(|| chapter_time(secs(rate.frame_at(q.duration()) + offset)));
        let text = if m.name.is_empty() { m.comment.as_str() } else { m.name.as_str() };
        let line = fmt
            .replace("{{timecode}}", &chapter_time(secs(f)))
            .replace("{{nexttimecode}}", &next)
            .replace("{{realtimecode}}", &format_timecode_frames(f, rate, q.settings.drop_frame))
            .replace("{{frame}}", &f.to_string())
            .replace("{{index}}", &(i + 1).to_string())
            .replace("{{comment}}", text);
        lines.push(line);
    }
    // YouTube's chapter rules, checked only for the default format
    let mut warnings = Vec::new();
    if fmt == DEFAULT_CHAPTER_FORMAT {
        if ms.first().is_none_or(|m| secs(frame(m)) != 0) {
            warnings.push("The first chapter must start at 0:00.".to_string());
        }
        if ms.len() < 3 {
            warnings.push("At least 3 chapters are needed.".to_string());
        }
        if ms.windows(2).any(|w| secs(frame(&w[1])) - secs(frame(&w[0])) < 10) {
            warnings.push("Each chapter must be at least 10 seconds long.".to_string());
        }
    }
    Ok((lines.join("\n"), warnings))
}

fn export_chapters(s: &mut Session, p: &Value) -> Result<Value> {
    let (text, warnings) = chapters(s, p)?;
    if let Some(path) = str_p(p, "path").filter(|v| !v.trim().is_empty()) {
        s.services.write_file(path, text.as_bytes()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    }
    Ok(json!({"text": text, "warnings": warnings}))
}

/// Parse `H:MM:SS`, `M:SS` or `SS` (whole seconds).
fn parse_clock(t: &str) -> Option<i64> {
    let parts: Vec<&str> = t.split(':').collect();
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    parts.iter().try_fold(0i64, |acc, p| p.parse::<i64>().ok().map(|v| acc * 60 + v))
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let id = "markers.import";
    let text = match (str_p(p, "text"), str_p(p, "path")) {
        (Some(t), _) => t.to_string(),
        (None, Some(path)) => {
            let b = s.services.read_file(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
            String::from_utf8_lossy(&b).into_owned()
        }
        (None, None) => return Err(bad(id, "need `path` or `text`")),
    };
    let rate = s.sequence_rate();
    let offset = p.get("offsetFrames").and_then(Value::as_i64).unwrap_or(0);
    let cats = s.project.settings.marker_categories();
    let default = s.project.settings.default_marker_color();
    // (frame, duration frames, text, colour)
    let mut rows: Vec<(i64, i64, String, Label)> = Vec::new();
    if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(text.trim()) {
        for m in a {
            let f = m
                .get("position")
                .and_then(Value::as_i64)
                .or_else(|| m.get("seconds").and_then(Value::as_f64).map(|x| (x * rate.num as f64 / rate.den as f64).round() as i64));
            let Some(f) = f else { continue };
            let color = str_p(&m, "color")
                .and_then(Label::from_name)
                .or_else(|| str_p(&m, "type").and_then(|t| cats.iter().find(|c| c.name == t).map(|c| c.color)))
                .or_else(|| m.get("type").and_then(Value::as_u64).and_then(|i| cats.get(i as usize).map(|c| c.color)))
                .unwrap_or(default);
            let d = m.get("duration").and_then(Value::as_i64).unwrap_or(0).max(0);
            rows.push((f, d, str_p(&m, "comment").unwrap_or("").to_string(), color));
        }
    } else {
        // chapter text: a time at the start of each line, then the title
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let (t, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            if let Some(secs) = parse_clock(t) {
                rows.push((secs * rate.num / rate.den.max(1), 0, rest.trim().to_string(), default));
            }
        }
    }
    if rows.is_empty() {
        return Err(bad(id, "no markers found (expected exported marker JSON or `0:00 Title` lines)"));
    }
    let ids = s.edit_sequence("Import Markers", |q, ctx, _| {
        let mut ids = Vec::new();
        for (f, d, text, color) in rows {
            let mid = MarkerId(ctx.alloc());
            let start = rate.tick_of((f + offset).max(0));
            q.markers.push(Marker { id: mid, start, duration: rate.tick_of(d), name: text, comment: String::new(), kind: MarkerKind::Comment, color });
            ids.push(mid.0);
        }
        q.markers.sort_by_key(|m| m.start);
        Ok(ids)
    })?;
    Ok(json!({"markers": ids}))
}

fn move_clip_marker(s: &mut Session, p: &Value) -> Result<Value> {
    let id = "markers.moveClipMarker";
    let item = u64_p(p, "item").map(ItemId).ok_or_else(|| bad(id, "need `item`"))?;
    let marker = u64_p(p, "marker").map(MarkerId).ok_or_else(|| bad(id, "need `marker`"))?;
    let time = p.get("time").and_then(Value::as_i64).map(|t| Tick(t.max(0)));
    let dur = p.get("duration").and_then(Value::as_i64).map(|t| Tick(t.max(0)));
    let len = s.project.item(item).map(|i| i.duration()).ok_or_else(|| bad(id, "no such project item"))?;
    let pq = p.clone();
    s.edit("Edit Clip Marker", |pr, _| {
        let it = pr.item_mut(item).ok_or_else(|| bad(id, "no such project item"))?;
        let list = match &mut it.kind {
            ItemKind::Media(m) => &mut m.markers,
            ItemKind::Sequence(q) => &mut std::sync::Arc::make_mut(q).markers,
            _ => return Err(bad(id, "only media and sequences have clip markers")),
        };
        let m = list.iter_mut().find(|m| m.id == marker).ok_or_else(|| bad(id, "no such marker"))?;
        if let Some(t) = time {
            m.start = if len > Tick::ZERO { t.min(len) } else { t };
        }
        if let Some(d) = dur {
            m.duration = d;
        }
        if let Some(n) = str_p(&pq, "name") {
            m.name = n.to_string();
        }
        if let Some(c) = str_p(&pq, "comment") {
            m.comment = c.to_string();
        }
        if let Some(c) = str_p(&pq, "color").and_then(Label::from_name) {
            m.color = c;
        }
        list.sort_by_key(|m| m.start);
        Ok(())
    })?;
    Ok(Value::Null)
}

fn delete_clip_marker(s: &mut Session, p: &Value) -> Result<Value> {
    let id = "markers.deleteClipMarker";
    let item = u64_p(p, "item").map(ItemId).ok_or_else(|| bad(id, "need `item`"))?;
    let marker = u64_p(p, "marker").map(MarkerId).ok_or_else(|| bad(id, "need `marker`"))?;
    s.edit("Delete Clip Marker", |pr, _| {
        let it = pr.item_mut(item).ok_or_else(|| bad(id, "no such project item"))?;
        let list = match &mut it.kind {
            ItemKind::Media(m) => &mut m.markers,
            ItemKind::Sequence(q) => &mut std::sync::Arc::make_mut(q).markers,
            _ => return Err(bad(id, "only media and sequences have clip markers")),
        };
        let before = list.len();
        list.retain(|m| m.id != marker);
        if list.len() == before {
            return Err(bad(id, "no such marker"));
        }
        Ok(())
    })?;
    Ok(Value::Null)
}
