//! Engine side of the M12.6 panels (the scopes are in [`crate::scopes`]).
//!
//! | Id | What |
//! |---|---|
//! | `events.list` / `events.clear` | the Events panel: warnings and errors of commands, background jobs and the auto-save worker ([`EventLog`]) |
//! | `metadata.get` / `metadata.set` | the Metadata panel: an item's clip and file properties; editable log fields (undoable) |
//! | `project.notes` / `project.setNotes` | the Project Notes panel: free plain text saved in the project (undoable) |
//! | `project.notesTimecodes` / `project.notesInsertTimecode` / `project.notesMarkers` | timecodes written in the notes: list them, write the playhead's, make sequence markers of them |
//!
//! The Progress panel uses `jobs.list` / `jobs.cancel`; the Timecode panel and the Reference
//! Monitor are frontend state only.

use std::collections::{BTreeMap, VecDeque};

use redrob_film_project::{ItemId, ItemKind, Label, MediaRef, Project};
use redrob_film_time::{Tick, TimeDisplay, format_time};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, item_p, str_p, u64_p};
use crate::{Result, Session};

// ------------------------------------------------------------------------------------ events log

/// Severity of an Events panel entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Info => "Info",
            Level::Warning => "Warning",
            Level::Error => "Error",
        }
    }
    pub fn from_name(s: &str) -> Option<Level> {
        match s.to_ascii_lowercase().as_str() {
            "info" => Some(Level::Info),
            "warning" | "warn" => Some(Level::Warning),
            "error" => Some(Level::Error),
            _ => None,
        }
    }
}

/// One Events panel entry. Repeats of the newest entry (same level, source and message) bump
/// `count` instead of adding a row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: u64,
    pub level: Level,
    /// Command id, `job`, `autosave` or `app`.
    pub source: String,
    pub message: String,
    pub count: u32,
    /// Wall-clock time of the last occurrence (ms since the Unix epoch).
    pub time_ms: u64,
}

/// The session's event log (newest last, at most [`EventLog::LIMIT`] entries).
#[derive(Clone, Debug, Default)]
pub struct EventLog {
    pub entries: VecDeque<LogEntry>,
    next: u64,
    /// Jobs already reported: id → finished.
    jobs: BTreeMap<u64, bool>,
}

fn now_ms() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl EventLog {
    pub const LIMIT: usize = 1000;

    pub fn push(&mut self, level: Level, source: &str, message: impl Into<String>) {
        let message = message.into();
        let t = now_ms();
        if let Some(last) = self.entries.back_mut()
            && last.level == level
            && last.source == source
            && last.message == message
        {
            last.count += 1;
            last.time_ms = t;
            return;
        }
        self.next += 1;
        self.entries.push_back(LogEntry { id: self.next, level, source: source.to_string(), message, count: 1, time_ms: t });
        while self.entries.len() > Self::LIMIT {
            self.entries.pop_front();
        }
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
    pub fn count(&self, level: Level) -> usize {
        self.entries.iter().filter(|e| e.level == level).count()
    }
}

/// Report background jobs that started or finished since the last call.
pub fn log_jobs(s: &mut Session) {
    let mut out = Vec::new();
    for j in &s.jobs {
        let finished = j.result.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let known = s.log.jobs.get(&j.id).copied();
        if known.is_none() {
            out.push((Level::Info, format!("Started: {}", j.label)));
        }
        if known != Some(true)
            && let Some(r) = finished
        {
            let cancelled = j.progress.cancel.load(std::sync::atomic::Ordering::Relaxed);
            out.push(match r {
                Ok(_) => (Level::Info, format!("Finished: {}", j.label)),
                Err(_) if cancelled => (Level::Warning, format!("Cancelled: {}", j.label)),
                Err(e) => (Level::Error, format!("Failed: {}: {e}", j.label)),
            });
            s.log.jobs.insert(j.id, true);
        } else {
            s.log.jobs.entry(j.id).or_insert(false);
        }
    }
    for (l, m) in out {
        s.log.push(l, "job", m);
    }
}

fn events_list(s: &mut Session, p: &Value) -> Result<Value> {
    log_jobs(s);
    let min = str_p(p, "level").and_then(Level::from_name).unwrap_or(Level::Info);
    let since = u64_p(p, "since").unwrap_or(0);
    let entries: Vec<&LogEntry> = s.log.entries.iter().filter(|e| e.level >= min && e.id > since).collect();
    Ok(json!({
        "entries": entries,
        "counts": {"info": s.log.count(Level::Info), "warning": s.log.count(Level::Warning), "error": s.log.count(Level::Error)},
    }))
}

// ------------------------------------------------------------------------------------ metadata

/// The log fields the Metadata panel edits (stored in `ProjectItem::metadata`), in panel order.
pub const LOG_FIELDS: [&str; 8] = ["Description", "Scene", "Shot", "Log Note", "Comment", "Tape Name", "Client", "Camera Angle"];

/// One Metadata panel row.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    /// "Clip" or "File".
    pub section: &'static str,
    pub name: String,
    pub value: String,
    pub editable: bool,
}

/// The item the Metadata panel describes: `item`, else the item of timeline clip `clip`, else the
/// Project panel selection, else the timeline selection.
pub fn metadata_target(s: &Session, p: &Value) -> Option<ItemId> {
    if let Some(i) = item_p(p, "item") {
        return Some(i);
    }
    let clip = u64_p(p, "clip").map(redrob_film_project::ClipId);
    let seq = s.active_sequence();
    let clip_item = |c| seq.and_then(|q| q.find_item(c)).map(|(_, it)| it.item);
    if let Some(c) = clip {
        return clip_item(c);
    }
    s.state.project_selection.first().copied().or_else(|| s.state.selection.first().and_then(|c| clip_item(*c)))
}

fn tc(t: Tick, rate: redrob_film_time::FrameRate) -> String {
    format_time(t, rate, false, TimeDisplay::Timecode, 48000)
}

/// Every Metadata panel row of an item (Clip section, then File section).
pub fn metadata_fields(p: &Project, id: ItemId) -> Vec<Field> {
    let Some(it) = p.item(id) else { return Vec::new() };
    let media = match &it.kind {
        ItemKind::Media(m) => Some(m),
        ItemKind::Subclip { parent, .. } => p.item(*parent).and_then(|x| x.as_media()),
        _ => None,
    };
    let rate = match &it.kind {
        ItemKind::Subclip { parent, .. } => p.item(*parent).map(|x| x.frame_rate()).unwrap_or_default(),
        _ => it.frame_rate(),
    };
    let mut out = Vec::new();
    let mut add = |section, name: &str, value: String, editable| out.push(Field { section, name: name.to_string(), value, editable });
    add("Clip", "Name", it.name.clone(), true);
    add("Clip", "Label", it.label.name().to_string(), true);
    add("Clip", "Media Type", it.type_label().to_string(), false);
    let col = |c: &str| redrob_film_project::find::column_text(p, it, c);
    add("Clip", "Frame Rate", col("Frame Rate"), false);
    // media start / end / duration in the item's own timecode
    let start = media.and_then(|m| m.info.start_timecode).map(|f| rate.tick_of(f)).unwrap_or(Tick::ZERO);
    let (offset, dur) = match &it.kind {
        ItemKind::Subclip { range, .. } => (range.start, range.duration),
        _ => (Tick::ZERO, it.duration()),
    };
    let first = start + offset;
    add("Clip", "Media Start", tc(first, rate), false);
    let last = if dur > Tick::ZERO { first + dur - rate.frame_duration() } else { first };
    add("Clip", "Media End", tc(last, rate), false);
    add("Clip", "Media Duration", tc(dur, rate), false);
    let marks = match &it.kind {
        ItemKind::Media(m) => (m.mark_in, m.mark_out),
        ItemKind::Sequence(q) => (q.mark_in, q.mark_out),
        _ => (None, None),
    };
    if let (Some(i), Some(o)) = marks {
        add("Clip", "In Point", tc(start + i, rate), false);
        add("Clip", "Out Point", tc(start + o - rate.frame_duration(), rate), false);
    }
    if let Some(v) = media.and_then(|m| m.info.video.as_ref()) {
        add("Clip", "Video Info", format!("{} x {} ({:.4})", v.width, v.height, v.par.0 as f64 / v.par.1.max(1) as f64), false);
        add("Clip", "Video Codec", v.codec.clone(), false);
        let detected = redrob_film_color::ColorSpace::from_info(&v.color);
        let cs = media.and_then(|m| m.interpret.color_space).unwrap_or(detected);
        add("Clip", "Color Space", cs.label().to_string(), false);
    }
    if let Some(a) = media.and_then(|m| m.info.audio()) {
        add("Clip", "Audio Info", format!("{} Hz - {} ch", a.sample_rate, a.channels), false);
        add("Clip", "Audio Codec", a.codec.clone(), false);
    }
    if let ItemKind::Sequence(q) = &it.kind {
        add("Clip", "Video Info", format!("{} x {}", q.settings.width, q.settings.height), false);
        add("Clip", "Color Space", q.settings.color.working.label().to_string(), false);
    }
    for k in LOG_FIELDS {
        add("Clip", k, it.metadata.get(k).cloned().unwrap_or_default(), true);
    }
    // other stored fields (from imports, Edit Offline, agents…)
    for (k, v) in &it.metadata {
        if !LOG_FIELDS.contains(&k.as_str()) && !crate::project_panel::is_freeform_key(k) {
            add("Clip", k, v.clone(), true);
        }
    }
    if let Some(m) = media {
        let path = match &m.media {
            MediaRef::File { path } => path.clone(),
            other => format!("{other:?}"),
        };
        add("File", "File Path", path, false);
        add("File", "Format", m.info.container.clone(), false);
        if let Some(sz) = m.info.file_size {
            add("File", "File Size", format_bytes(sz), false);
        }
        if let Some(v) = &m.info.video
            && let Some(b) = v.bitrate
        {
            add("File", "Video Bitrate", format!("{:.2} Mbit/s", b as f64 / 1e6), false);
        }
        add("File", "Status", if m.offline { "Offline".into() } else { "Online".into() }, false);
    }
    out
}

fn format_bytes(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.2} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{b} bytes"),
    }
}

fn metadata_get(s: &mut Session, p: &Value) -> Result<Value> {
    let id = metadata_target(s, p).ok_or_else(|| bad("metadata.get", "no item: pass `item` or `clip`, or select one"))?;
    let it = s.project.item(id).ok_or_else(|| bad("metadata.get", "no such item"))?;
    let fields = metadata_fields(&s.project, id);
    Ok(json!({"item": id.0, "name": it.name, "fields": fields, "metadata": it.metadata}))
}

/// `metadata.set {item?, field, value}` or `{item?, fields: {name: value}}`: one undo step.
/// Name renames the item, Label sets its label, every other name is a log field (an empty value
/// removes it).
fn metadata_set(s: &mut Session, p: &Value) -> Result<Value> {
    let id = metadata_target(s, p).ok_or_else(|| bad("metadata.set", "no item: pass `item` or `clip`, or select one"))?;
    let mut changes: Vec<(String, String)> = Vec::new();
    if let Some(f) = str_p(p, "field") {
        let v = p.get("value").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
        changes.push((f.to_string(), v));
    }
    if let Some(m) = p.get("fields").and_then(Value::as_object) {
        for (k, v) in m {
            changes.push((k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())));
        }
    }
    if changes.is_empty() {
        return Err(bad("metadata.set", "need `field` + `value` or `fields`"));
    }
    for (k, v) in &changes {
        if k.trim().is_empty() {
            return Err(bad("metadata.set", "empty field name"));
        }
        if k.eq_ignore_ascii_case("label") && Label::from_name(v).is_none() {
            return Err(bad("metadata.set", format!("unknown label `{v}`")));
        }
        if k.eq_ignore_ascii_case("name") && v.trim().is_empty() {
            return Err(bad("metadata.set", "the name cannot be empty"));
        }
        if metadata_fields(&s.project, id).iter().any(|f| f.name.eq_ignore_ascii_case(k) && !f.editable) {
            return Err(bad("metadata.set", format!("`{k}` is read-only")));
        }
    }
    let unchanged = {
        let it = s.project.item(id).ok_or_else(|| bad("metadata.set", "no such item"))?;
        changes.iter().all(|(k, v)| match k.to_ascii_lowercase().as_str() {
            "name" => it.name == *v,
            "label" => Label::from_name(v) == Some(it.label),
            _ => it.metadata.get(k).map(String::as_str).unwrap_or("") == v.as_str(),
        })
    };
    if !unchanged {
        s.edit("Edit Metadata", |pr, _| {
            let it = pr.item_mut(id).ok_or_else(|| bad("metadata.set", "no such item"))?;
            for (k, v) in &changes {
                match k.to_ascii_lowercase().as_str() {
                    "name" => it.name = v.trim().to_string(),
                    "label" => it.label = Label::from_name(v).unwrap_or(it.label),
                    _ if v.is_empty() => {
                        it.metadata.remove(k);
                    }
                    _ => {
                        it.metadata.insert(k.clone(), v.clone());
                    }
                }
            }
            Ok(())
        })?;
    }
    let it = s.project.item(id).ok_or_else(|| bad("metadata.set", "no such item"))?;
    Ok(json!({"item": id.0, "name": it.name, "label": it.label.name(), "metadata": it.metadata, "changed": !unchanged}))
}

// ------------------------------------------------------------------------------------ project notes

/// Longest project notes accepted (bytes of UTF-8), so a script or a pasted file cannot grow the
/// project (and every undo step) without bound.
pub const MAX_NOTES_BYTES: usize = 1 << 20;

/// `project.notes`: the project notes.
fn notes_get(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"text": s.project.notes}))
}

/// `project.setNotes {text, merge?}`: replace the project notes as one undo step. Calls with the
/// same `merge` key and no other edit in between share that step (typing in the panel).
fn notes_set(s: &mut Session, p: &Value) -> Result<Value> {
    let text = p.get("text").and_then(Value::as_str).ok_or_else(|| bad("project.setNotes", "need `text` (a string)"))?;
    if text.len() > MAX_NOTES_BYTES {
        return Err(bad("project.setNotes", format!("the notes are too long ({} bytes; at most {MAX_NOTES_BYTES})", text.len())));
    }
    let changed = s.project.notes != text;
    if changed {
        let body = |pr: &mut Project, _: &mut crate::EditorState| {
            pr.notes = text.to_string();
            Ok(())
        };
        match str_p(p, "merge") {
            Some(key) => s.edit_merged("Edit Project Notes", &format!("project.notes:{key}"), body)?,
            None => s.edit("Edit Project Notes", body)?,
        }
    }
    Ok(json!({"text": s.project.notes, "changed": changed}))
}

/// A timecode written in the project notes (`01:00:12:05`, `00:02:30;10`, `00:00:42`): its place
/// in the text (character offsets, end exclusive), the text, the sequence time it names and the
/// rest of its line (what the note says about that moment).
#[derive(Clone, Debug, PartialEq)]
pub struct NoteTimecode {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub time: Tick,
    pub rest: String,
}

/// The timecodes in `notes` (three or four fields of one to three digits joined by `:` or `;`,
/// not part of a longer run of digits), as times of a sequence at `rate` whose timecode starts at
/// frame `start_frame`. Times before the sequence's start are left out.
pub fn note_timecodes(notes: &str, rate: redrob_film_time::FrameRate, drop_frame: bool, start_frame: i64) -> Vec<NoteTimecode> {
    let chars: Vec<char> = notes.chars().collect();
    let tc_char = |c: char| c.is_ascii_digit() || c == ':' || c == ';';
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() || (i > 0 && (tc_char(chars[i - 1]) || chars[i - 1] == '.')) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && tc_char(chars[j]) {
            j += 1;
        }
        // a trailing separator belongs to the sentence, not the timecode
        while j > i && !chars[j - 1].is_ascii_digit() {
            j -= 1;
        }
        let text: String = chars[i..j].iter().collect();
        let fields: Vec<&str> = text.split([':', ';']).collect();
        let ok = (3..=4).contains(&fields.len()) && fields.iter().all(|f| (1..=3).contains(&f.len()));
        if ok {
            let full = if fields.len() == 3 { format!("{text}:00") } else { text.clone() };
            if let Ok(frame) = redrob_film_time::parse_timecode(&full, rate, drop_frame, 0) {
                let frame = frame - start_frame;
                if frame >= 0 {
                    let rest: String = chars[j..].iter().take_while(|c| **c != '\n').collect();
                    let rest = rest.trim_start_matches([' ', '\t', '-', ':', '–', '—']).trim().to_string();
                    out.push(NoteTimecode { start: i, end: j, text, time: rate.tick_of(frame), rest });
                }
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// The timecodes of the notes against the active sequence (none without one).
pub fn active_note_timecodes(s: &Session) -> Vec<NoteTimecode> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    note_timecodes(&s.project.notes, q.settings.frame_rate, q.settings.drop_frame, q.start_timecode.max(0))
}

/// `project.notesTimecodes`: the timecodes written in the notes, with the sequence times they name.
fn notes_timecodes(s: &mut Session, _: &Value) -> Result<Value> {
    let list: Vec<Value> = active_note_timecodes(s)
        .into_iter()
        .map(|t| json!({"text": t.text, "start": t.start, "end": t.end, "time": t.time.0, "rest": t.rest}))
        .collect();
    Ok(json!({"timecodes": list}))
}

/// `project.notesInsertTimecode {at?}`: write the playhead's timecode into the notes at character
/// offset `at` (default: the end), as one undo step. The written timecode is a link the panel
/// seeks to. Returns the text and the offset just after the timecode.
fn notes_insert_timecode(s: &mut Session, p: &Value) -> Result<Value> {
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let (rate, drop_frame, start) = (q.settings.frame_rate, q.settings.drop_frame, q.start_timecode.max(0));
    let ph = s.playhead();
    let tc = redrob_film_time::format_timecode_frames(rate.frame_at(ph) + start, rate, drop_frame);
    let chars: Vec<char> = s.project.notes.chars().collect();
    let at = p.get("at").and_then(Value::as_u64).map_or(chars.len(), |a| (a as usize).min(chars.len()));
    // a space keeps the timecode apart from the word before and after it
    let before = if at > 0 && !chars[at - 1].is_whitespace() { " " } else { "" };
    let after = if chars.get(at).is_some_and(|c| c.is_whitespace()) { "" } else { " " };
    let insert = format!("{before}{tc}{after}");
    let text: String = chars[..at].iter().collect::<String>() + &insert + &chars[at..].iter().collect::<String>();
    if text.len() > MAX_NOTES_BYTES {
        return Err(bad("project.notesInsertTimecode", "the notes are too long"));
    }
    s.edit("Insert Timecode in Notes", |pr, _| {
        pr.notes = text.clone();
        Ok(())
    })?;
    Ok(json!({"text": s.project.notes, "timecode": tc, "cursor": at + insert.chars().count()}))
}

/// `project.notesMarkers {from?, to?}`: a sequence marker at each timecode of the notes whose
/// text lies in the character range [from, to) (default: all of them), named after the rest of
/// its line. One undo step; timecodes that already have a marker at their time are skipped.
fn notes_markers(s: &mut Session, p: &Value) -> Result<Value> {
    s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let from = p.get("from").and_then(Value::as_u64).map_or(0, |v| v as usize);
    let to = p.get("to").and_then(Value::as_u64).map_or(usize::MAX, |v| v as usize);
    let (from, to) = (from.min(to), from.max(to));
    let wanted: Vec<NoteTimecode> = active_note_timecodes(s).into_iter().filter(|t| t.end > from && t.start < to.max(from + 1)).collect();
    if wanted.is_empty() {
        return Err(bad("project.notesMarkers", "no timecode in the notes (or in the selected text)"));
    }
    let n = s.edit_sequence("Create Markers from Notes", |q, ctx, _| {
        let mut n = 0;
        for t in &wanted {
            if q.markers.iter().any(|m| m.start == t.time) {
                continue;
            }
            let id = redrob_film_project::MarkerId(ctx.alloc());
            q.markers.push(redrob_film_project::Marker {
                id,
                start: t.time,
                duration: Tick::ZERO,
                name: t.rest.chars().take(200).collect(),
                comment: String::new(),
                kind: redrob_film_project::MarkerKind::Comment,
                color: Label::Green,
            });
            n += 1;
        }
        q.markers.sort_by_key(|m| m.start);
        Ok(n)
    })?;
    Ok(json!({"markers": n}))
}

// ------------------------------------------------------------------------------------ project metadata

/// The standard project metadata fields, in the order the dialog shows them (also what exports
/// write).
pub const PROJECT_METADATA_FIELDS: [&str; 5] = ["title", "creator", "copyright", "description", "comment"];

/// `project.metadata`: the project's metadata fields.
fn project_metadata_get(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"fields": s.project.metadata, "standard": PROJECT_METADATA_FIELDS}))
}

/// `project.setMetadata {fields: {name: text|null}}`: set (an empty text or null removes) fields
/// of the project metadata, as one undo step.
fn project_metadata_set(s: &mut Session, p: &Value) -> Result<Value> {
    let fields = p.get("fields").and_then(Value::as_object).ok_or_else(|| bad("project.setMetadata", "need `fields` (an object of name → text)"))?;
    let mut next = s.project.metadata.clone();
    for (k, v) in fields {
        let k = k.trim();
        if k.is_empty() || k.len() > 100 {
            return Err(bad("project.setMetadata", "field names must be 1 to 100 bytes"));
        }
        match v.as_str().map(str::trim) {
            Some(t) if !t.is_empty() => {
                next.insert(k.to_string(), t.chars().take(10_000).collect());
            }
            _ => {
                next.remove(k);
            }
        }
    }
    if next != s.project.metadata {
        s.edit("Edit Project Metadata", |pr, _| {
            pr.metadata = next;
            Ok(())
        })?;
    }
    Ok(json!({"fields": s.project.metadata}))
}

/// `project.filesStats`: the project's file-based media, split into the files some sequence uses
/// and the unused ones, with their counts and sizes on disk (Project Settings ▸ Project Files).
fn project_files_stats(s: &mut Session, _: &Value) -> Result<Value> {
    let mut used: std::collections::BTreeSet<ItemId> = std::collections::BTreeSet::new();
    for q in s.project.sequences() {
        if let ItemKind::Sequence(seq) = &q.kind {
            for tr in seq.all_tracks() {
                used.extend(tr.items.iter().map(|i| i.item));
            }
        }
    }
    let (mut u, mut n) = ((0u64, 0u64), (0u64, 0u64));
    let mut unused_items = Vec::new();
    let mut missing = 0u64;
    for (id, it) in &s.project.items {
        let Some(m) = it.as_media() else { continue };
        let MediaRef::File { path } = &m.media else { continue };
        let size = match std::fs::metadata(path) {
            Ok(md) => md.len(),
            Err(_) => {
                missing += 1;
                0
            }
        };
        if used.contains(id) {
            u = (u.0 + 1, u.1 + size);
        } else {
            n = (n.0 + 1, n.1 + size);
            unused_items.push(json!({"item": id.0, "name": it.name, "path": path, "bytes": size}));
        }
    }
    Ok(json!({
        "used": {"files": u.0, "bytes": u.1},
        "unused": {"files": n.0, "bytes": n.1, "items": unused_items},
        "missing": missing,
    }))
}

// ------------------------------------------------------------------------------------ registry

pub(crate) fn commands() -> Vec<CommandSpec> {
    let q = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        q("events.list", "List Events", r#"{"level":"info|warning|error"?,"since":id?}"#, events_list),
        CommandSpec {
            id: "events.clear",
            label: "Clear All Events",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: always,
            run: |s, _| {
                let n = s.log.entries.len();
                s.log.clear();
                Ok(json!({"cleared": n}))
            },
            journal: true,
        },
        q("metadata.get", "Get Metadata", r#"{"item":id?,"clip":id?}"#, metadata_get),
        CommandSpec {
            id: "metadata.set",
            label: "Edit Metadata",
            menu: &[],
            shortcut: None,
            params: r#"{"item":id?,"clip":id?,"field":"Name|Label|Description|Scene|Shot|Log Note|Comment|Tape Name|Client|Camera Angle|…","value":str} | {"item":id?,"fields":{name:str}}"#,
            enabled: always,
            run: metadata_set,
            journal: true,
        },
        q("project.notes", "Get Project Notes", "{}", notes_get),
        CommandSpec {
            id: "project.setNotes",
            label: "Edit Project Notes",
            menu: &[],
            shortcut: None,
            params: r#"{"text":str,"merge":str?}"#,
            enabled: always,
            run: notes_set,
            journal: true,
        },
        q("project.notesTimecodes", "List Timecodes in Project Notes", "{}", notes_timecodes),
        q("project.metadata", "Get Project Metadata", "{}", project_metadata_get),
        q("project.filesStats", "Project Files Statistics", "{}", project_files_stats),
        CommandSpec {
            id: "project.setMetadata",
            label: "Edit Project Metadata",
            menu: &[],
            shortcut: None,
            params: r#"{"fields":{name:str|null}}"#,
            enabled: always,
            run: project_metadata_set,
            journal: true,
        },
        CommandSpec {
            id: "project.notesInsertTimecode",
            label: "Insert Timecode in Project Notes",
            menu: &[],
            shortcut: None,
            params: r#"{"at":charOffset?}"#,
            enabled: always,
            run: notes_insert_timecode,
            journal: true,
        },
        CommandSpec {
            id: "project.notesMarkers",
            label: "Create Markers from Project Notes",
            menu: &[],
            shortcut: None,
            params: r#"{"from":charOffset?,"to":charOffset?}"#,
            enabled: always,
            run: notes_markers,
            journal: true,
        },
    ]
}
