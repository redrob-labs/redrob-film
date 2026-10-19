//! Library (Kdenlive's Library panel): sets of timeline clips kept outside any project, for copying
//! between projects. Each library item is a small Redrob Film project file (`.rfilm`) holding one
//! sequence (the clips, with their effects and transitions) and the media it uses, stored in the
//! library folder (Settings ▸ Media ▸ Library folder; by default `Library` in the data directory)
//! and its sub-folders. The folder is read every time, so files moved with a file manager show up.
//!
//! | Id | What |
//! |---|---|
//! | `library.list` | the library folder's items and sub-folders |
//! | `library.addSelection` | Add Timeline Selection to Library: the selected timeline clips as a new library item |
//! | `library.addToProject` | Add Clip to Project: the item's sequence (a nestable library clip) and its media into the project bin |
//! | `library.newFolder` / `library.move` / `library.rename` / `library.delete` | organise the library |
//! | `sequence.expandClip` | Sequence ▸ Expand Clip: replace a nested (library) clip on the timeline with its contents |

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use redrob_film_project::{ClipId, ItemId, Project, Sequence, TrackId, TrackItem, TrackKind};
use redrob_film_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, has_seq, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// Extension of library items.
pub const EXT: &str = "rfilm";

/// The library folder: Settings ▸ Media ▸ Library folder, else `Library` in the data directory.
pub fn library_dir(s: &Session) -> Option<PathBuf> {
    let set = s.prefs.media.library_folder.trim();
    if !set.is_empty() {
        return Some(PathBuf::from(set));
    }
    s.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join("Library"))
}

fn dir_or_err(s: &Session, cmd: &str) -> Result<PathBuf> {
    let d = library_dir(s).ok_or_else(|| bad(cmd, "no library folder: set Settings ▸ Media ▸ Library folder"))?;
    std::fs::create_dir_all(&d).map_err(|e| EngineError::Other(format!("{}: {e}", d.display())))?;
    Ok(d)
}

/// A path inside the library from a path relative to it (`Audio/Intro.rfilm`); refuses `..` and
/// absolute paths so commands cannot reach outside the library.
fn inside(root: &Path, rel: &str, cmd: &str) -> Result<PathBuf> {
    let rel = rel.trim().trim_start_matches(['/', '\\']);
    let p = Path::new(rel);
    if p.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return Err(bad(cmd, format!("`{rel}` is not a path inside the library")));
    }
    Ok(root.join(p))
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| p.to_string_lossy().to_string())
}

fn safe_name(n: &str) -> String {
    n.chars().map(|c| if c.is_alphanumeric() || " -_().".contains(c) { c } else { '_' }).collect::<String>().trim().to_string()
}

/// `library.list`: every folder and item, depth first, folders before items, by name.
fn list(s: &mut Session, _: &Value) -> Result<Value> {
    let Some(root) = library_dir(s) else { return Ok(json!({"folder": null, "entries": []})) };
    let mut entries = Vec::new();
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<Value>) {
        if depth > 16 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut all: Vec<(bool, String, PathBuf)> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() || p.extension().is_some_and(|x| x == EXT))
            .map(|p| (!p.is_dir(), p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default(), p))
            .collect();
        all.sort();
        for (is_item, _, p) in all {
            let shown = if is_item { p.file_stem() } else { p.file_name() };
            let name = shown.map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            out.push(json!({"path": rel_of(root, &p), "name": name, "folder": !is_item, "depth": depth}));
            if !is_item {
                walk(root, &p, depth + 1, out);
            }
        }
    }
    walk(&root, &root, 0, &mut entries);
    Ok(json!({"folder": root.to_string_lossy(), "entries": entries}))
}

/// Track index of a track of `seq` within its kind.
fn track_index(seq: &Sequence, track: TrackId) -> Option<(TrackKind, usize)> {
    for kind in [TrackKind::Video, TrackKind::Audio] {
        if let Some(i) = seq.tracks(kind).iter().position(|t| t.id == track) {
            return Some((kind, i));
        }
    }
    None
}

/// `library.addSelection {name, folder?}`: the selected timeline clips (with their effects, and
/// the transitions between them) as a library item, shifted to start at zero.
fn add_selection(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "library.addSelection";
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(CMD, "need `name`"))?.to_string();
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let sel: BTreeSet<ClipId> = s.state.selection.iter().copied().collect();
    if sel.is_empty() {
        return Err(bad(CMD, "select clips in the timeline"));
    }
    let mut picked: Vec<(TrackKind, usize, TrackItem)> = Vec::new();
    for t in seq.all_tracks() {
        let Some((kind, idx)) = track_index(&seq, t.id) else { continue };
        for ti in t.items.iter().filter(|ti| sel.contains(&ti.id)) {
            picked.push((kind, idx, ti.clone()));
        }
    }
    if picked.is_empty() {
        return Err(bad(CMD, "the selection has no clips"));
    }
    let start = picked.iter().map(|x| x.2.start).min().unwrap_or(Tick::ZERO);
    let roots: Vec<ItemId> = picked.iter().map(|x| x.2.item).collect();
    let keep = crate::project_tools::closure(&s.project, &roots);
    let mut lib: Project = crate::project_tools::project_subset(&s.project, &keep);
    // the library item's sequence: the selection's tracks, from the lowest used one up
    let (nv, na) = (
        picked.iter().filter(|x| x.0 == TrackKind::Video).map(|x| x.1 + 1).max().unwrap_or(0),
        picked.iter().filter(|x| x.0 == TrackKind::Audio).map(|x| x.1 + 1).max().unwrap_or(0),
    );
    let (v0, a0) = (
        picked.iter().filter(|x| x.0 == TrackKind::Video).map(|x| x.1).min().unwrap_or(0),
        picked.iter().filter(|x| x.0 == TrackKind::Audio).map(|x| x.1).min().unwrap_or(0),
    );
    let sid = lib.new_sequence(&name, seq.settings.clone(), nv.saturating_sub(v0), na.saturating_sub(a0), None);
    let q = lib.sequence_mut(sid).ok_or_else(|| EngineError::Other("library sequence".into()))?;
    for (kind, idx, mut ti) in picked {
        ti.start -= start;
        let at = idx - if kind == TrackKind::Video { v0 } else { a0 };
        if let Some(t) = q.tracks_mut(kind).get_mut(at) {
            t.items.push(ti);
        }
    }
    // transitions whose clips both came along
    for (kind, src_tracks, base) in [(TrackKind::Video, &seq.video_tracks, v0), (TrackKind::Audio, &seq.audio_tracks, a0)] {
        for (i, t) in src_tracks.iter().enumerate() {
            let Some(k) = i.checked_sub(base) else { continue };
            let Some(dst) = q.tracks_mut(kind).get_mut(k) else { continue };
            for tr in &t.transitions {
                let ok = |c: Option<ClipId>| c.is_none_or(|c| sel.contains(&c));
                if (tr.from.is_some() || tr.to.is_some()) && ok(tr.from) && ok(tr.to) {
                    let mut tr = tr.clone();
                    tr.start -= start;
                    dst.transitions.push(tr);
                }
            }
        }
    }
    for t in q.all_tracks_mut() {
        t.sort();
    }
    q.check().map_err(EngineError::Other)?;
    lib.name = name.clone();
    lib.root.name = name.clone();
    let root = dir_or_err(s, CMD)?;
    let folder = match str_p(p, "folder") {
        Some(f) if !f.trim().is_empty() => inside(&root, f, CMD)?,
        _ => root.clone(),
    };
    std::fs::create_dir_all(&folder).map_err(|e| EngineError::Other(format!("{}: {e}", folder.display())))?;
    let mut path = folder.join(format!("{}.{EXT}", safe_name(&name)));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{} {n}.{EXT}", safe_name(&name)));
        n += 1;
    }
    let ps = path.to_string_lossy().to_string();
    s.services.write_file(&ps, &redrob_film_format::encode(&lib, false)).map_err(|e| EngineError::Other(format!("{ps}: {e}")))?;
    Ok(json!({"path": rel_of(&root, &path), "items": keep.len()}))
}

/// Read a library item.
fn load(s: &Session, rel: &str, cmd: &str) -> Result<Project> {
    let root = library_dir(s).ok_or_else(|| bad(cmd, "no library folder"))?;
    let path = inside(&root, rel, cmd)?;
    let ps = path.to_string_lossy().to_string();
    let bytes = s.services.read_file(&ps).map_err(|e| EngineError::Other(format!("{ps}: {e}")))?;
    let loaded = redrob_film_format::decode(&bytes).map_err(|e| EngineError::Other(format!("{ps}: {e}")))?;
    Ok(loaded.project)
}

/// `library.addToProject {path, bin?}`: the library item's sequence (a library clip, nestable on
/// the timeline) and its media, in a bin named after it.
fn add_to_project(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "library.addToProject";
    let rel = str_p(p, "path").ok_or_else(|| bad(CMD, "need `path` (from library.list)"))?.to_string();
    let lib = load(s, &rel, CMD)?;
    let name = lib.name.clone();
    // top-level sequences: those no other sequence of the item nests
    let nested: BTreeSet<ItemId> =
        lib.sequences().filter_map(|q| q.as_sequence()).flat_map(|q| q.all_tracks().flat_map(|t| t.items.iter().map(|i| i.item))).collect();
    let sequences: Vec<ItemId> = lib.sequences().map(|q| q.id).filter(|id| !nested.contains(id)).collect();
    let parent = u64_p(p, "bin").map(redrob_film_project::BinId);
    let fragment = redrob_film_interchange::Imported { project: lib, sequences };
    let (bin, seqs) = s.edit(&format!("Add {name} to Project"), |pr, st| {
        let bin = pr.add_bin(&name, parent);
        let seqs = redrob_film_interchange::merge_into(pr, fragment, Some(bin));
        st.project_selection = seqs.clone();
        Ok((bin, seqs))
    })?;
    Ok(json!({"bin": bin.0, "sequences": seqs.iter().map(|i| i.0).collect::<Vec<_>>()}))
}

fn new_folder(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "library.newFolder";
    let name = str_p(p, "name").map(safe_name).filter(|n| !n.is_empty()).ok_or_else(|| bad(CMD, "need `name`"))?;
    let root = dir_or_err(s, CMD)?;
    let parent = match str_p(p, "parent") {
        Some(f) if !f.trim().is_empty() => inside(&root, f, CMD)?,
        _ => root.clone(),
    };
    let dir = parent.join(&name);
    std::fs::create_dir_all(&dir).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
    Ok(json!({"path": rel_of(&root, &dir)}))
}

/// `library.move {path, folder}`: move an item or folder into another folder ("" = the top).
fn move_entry(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "library.move";
    let root = dir_or_err(s, CMD)?;
    let from = inside(&root, str_p(p, "path").ok_or_else(|| bad(CMD, "need `path`"))?, CMD)?;
    let to_dir = match str_p(p, "folder") {
        Some(f) if !f.trim().is_empty() => inside(&root, f, CMD)?,
        _ => root.clone(),
    };
    if to_dir.starts_with(&from) {
        return Err(bad(CMD, "a folder can't move into itself"));
    }
    let file = from.file_name().ok_or_else(|| bad(CMD, "nothing to move"))?;
    let to = to_dir.join(file);
    if to.exists() {
        return Err(bad(CMD, format!("{} already exists there", file.to_string_lossy())));
    }
    std::fs::rename(&from, &to).map_err(|e| EngineError::Other(format!("{}: {e}", from.display())))?;
    Ok(json!({"path": rel_of(&root, &to)}))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "library.rename";
    let root = dir_or_err(s, CMD)?;
    let from = inside(&root, str_p(p, "path").ok_or_else(|| bad(CMD, "need `path`"))?, CMD)?;
    let name = str_p(p, "name").map(safe_name).filter(|n| !n.is_empty()).ok_or_else(|| bad(CMD, "need `name`"))?;
    let to = if from.is_dir() { from.with_file_name(&name) } else { from.with_file_name(format!("{name}.{EXT}")) };
    if to.exists() {
        return Err(bad(CMD, format!("{name} already exists")));
    }
    std::fs::rename(&from, &to).map_err(|e| EngineError::Other(format!("{}: {e}", from.display())))?;
    Ok(json!({"path": rel_of(&root, &to)}))
}

/// `library.delete {path}`: an item, or an empty folder.
fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "library.delete";
    let root = dir_or_err(s, CMD)?;
    let rel = str_p(p, "path").ok_or_else(|| bad(CMD, "need `path`"))?;
    let path = inside(&root, rel, CMD)?;
    if path == root {
        return Err(bad(CMD, "the library folder itself can't be deleted"));
    }
    let r = if path.is_dir() { std::fs::remove_dir(&path) } else { std::fs::remove_file(&path) };
    r.map_err(|e| EngineError::Other(format!("{rel}: {e} (only empty folders can be deleted)")))?;
    Ok(json!({"deleted": rel}))
}

// ------------------------------------------------------------------------------------ expand

/// `sequence.expandClip {clip?}`: replace a nested sequence clip (and its linked audio) with the
/// clips inside it, from its own track up (video) and from its audio's track down the list
/// (audio). The tracks it needs must be free for its length.
fn expand_clip(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "sequence.expandClip";
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let clip = match u64_p(p, "clip") {
        Some(c) => ClipId(c),
        None => *s
            .state
            .selection
            .iter()
            .find(|c| seq.find_item(**c).is_some_and(|(_, ti)| s.project.sequence(ti.item).is_some()))
            .ok_or_else(|| bad(CMD, "select a nested sequence (library) clip in the timeline"))?,
    };
    let (track, ti) = seq.find_item(clip).ok_or_else(|| bad(CMD, format!("no clip {}", clip.0)))?;
    let ti = ti.clone();
    let inner = s.project.sequence(ti.item).ok_or_else(|| bad(CMD, "the clip is not a nested sequence"))?.clone();
    if inner.multicam.is_some() {
        return Err(bad(CMD, "multi-camera clips are flattened with Multi-Camera ▸ Flatten"));
    }
    if (ti.speed - 1.0).abs() > 1e-9 || ti.reverse {
        return Err(bad(CMD, "set the clip's speed back to 100 % before expanding it"));
    }
    // the clip and its linked partners go; where they sat, the contents come
    let partners: Vec<(TrackId, ClipId)> = seq
        .all_tracks()
        .flat_map(|t| t.items.iter().filter(|i| i.id == clip || (ti.link.is_some() && i.link == ti.link)).map(|i| (t.id, i.id)).collect::<Vec<_>>())
        .collect();
    let (kind0, base) = track_index(&seq, track).ok_or_else(|| bad(CMD, "no such track"))?;
    let audio_base = partners.iter().filter_map(|(t, _)| track_index(&seq, *t)).find(|(k, _)| *k == TrackKind::Audio).map(|x| x.1).unwrap_or(0);
    let (vbase, abase) = if kind0 == TrackKind::Video { (base, audio_base) } else { (0, base) };
    let window = TimeRange::new(ti.source_in, ti.duration);
    let offset = ti.start - ti.source_in;
    // the inner clips cut to the window, in parent time
    let mut placements: Vec<(TrackKind, usize, TrackItem)> = Vec::new();
    for kind in [TrackKind::Video, TrackKind::Audio] {
        for (k, t) in inner.tracks(kind).iter().enumerate() {
            for c in &t.items {
                let a = c.start.max(window.start);
                let b = c.end().min(window.end());
                if b <= a {
                    continue;
                }
                let mut c = c.clone();
                let head = a - c.start;
                c.source_in += Tick((head.0 as f64 * c.speed) as i64);
                c.start = a + offset;
                c.duration = b - a;
                placements.push((kind, k, c));
            }
        }
    }
    let need_v = placements.iter().filter(|x| x.0 == TrackKind::Video).map(|x| vbase + x.1 + 1).max().unwrap_or(0);
    let need_a = placements.iter().filter(|x| x.0 == TrackKind::Audio).map(|x| abase + x.1 + 1).max().unwrap_or(0);
    if need_v > seq.video_tracks.len() || need_a > seq.audio_tracks.len() {
        return Err(bad(CMD, format!("not enough tracks to expand: needs {need_v} video and {need_a} audio tracks")));
    }
    // room: nothing else may sit where the contents go
    let span = TimeRange::new(ti.start, ti.duration);
    for (kind, k, _) in &placements {
        let base = if *kind == TrackKind::Video { vbase } else { abase };
        let t = &seq.tracks(*kind)[base + k];
        if t.items.iter().any(|o| !partners.iter().any(|(_, c)| *c == o.id) && o.start < span.end() && o.end() > span.start) {
            return Err(bad(CMD, format!("{} has clips where the contents would go: make room first", t.name)));
        }
    }
    let n = placements.len();
    s.edit_sequence("Expand Clip", |q, ctx, st| {
        for (t, c) in &partners {
            if let Some(tr) = q.track_mut(*t) {
                tr.items.retain(|i| i.id != *c);
            }
        }
        // fresh ids; links and groups stay together within the expansion
        let mut links: HashMap<u64, u64> = HashMap::new();
        let mut out: Vec<(TrackId, TrackItem)> = Vec::new();
        let mut new_ids = Vec::new();
        for (kind, k, mut c) in placements {
            let base = if kind == TrackKind::Video { vbase } else { abase };
            let Some(tid) = q.tracks(kind).get(base + k).map(|t| t.id) else { continue };
            c.id = ClipId(ctx.alloc());
            c.link = c.link.map(|l| *links.entry(l | (1 << 62)).or_insert_with(|| ctx.alloc()));
            c.group = c.group.map(|g| *links.entry(g | (1 << 61)).or_insert_with(|| ctx.alloc()));
            new_ids.push(c.id);
            out.push((tid, c));
        }
        redrob_film_edit::overwrite(q, out, ctx)?;
        st.selection = new_ids;
        Ok(())
    })?;
    Ok(json!({"expanded": n, "clip": clip.0}))
}

// ------------------------------------------------------------------------------------ registry

pub(crate) fn commands() -> Vec<CommandSpec> {
    let c = |id, label, params, enabled, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled, run, journal: true };
    vec![
        CommandSpec { id: "library.list", label: "List Library", menu: &[], shortcut: None, params: "{}", enabled: always, run: list, journal: false },
        c("library.addSelection", "Add Timeline Selection to Library", r#"{"name":str,"folder":path?}"#, has_seq, add_selection),
        c("library.addToProject", "Add Clip to Project", r#"{"path":str,"bin":binId?}"#, always, add_to_project),
        c("library.newFolder", "Add Folder", r#"{"name":str,"parent":path?}"#, always, new_folder),
        c("library.move", "Move Library Item", r#"{"path":str,"folder":path}"#, always, move_entry),
        c("library.rename", "Rename Library Clip", r#"{"path":str,"name":str}"#, always, rename),
        c("library.delete", "Delete Library Item", r#"{"path":str}"#, always, delete),
        CommandSpec {
            id: "sequence.expandClip",
            label: "Expand Clip",
            menu: &["Sequence"],
            shortcut: None,
            params: r#"{"clip":clipId?}"#,
            enabled: has_seq,
            run: expand_clip,
            journal: true,
        },
    ]
}
