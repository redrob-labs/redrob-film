//! Project panel organisation from Kdenlive's Project Bin: star ratings, bin tags (any number per
//! item; the project's tags can be added, edited, deleted and reordered), the toolbar Filter
//! (tags / ratings / types / usage) and the default folders new sequences and audio captures go to.
//!
//! Ratings and tags are project data (`ProjectItem::rating`, `ProjectItem::tags`,
//! `Project::tags`), so they are undoable and saved with the project; the filter is a Project
//! panel preference (`Preferences::project_panel.filter`).
//!
//! | Id | What |
//! |---|---|
//! | `clip.setRating` | rate the selected items 0–5 stars |
//! | `clip.setTags` / `clip.toggleTag` | the tags of the selected items: replace them / add or remove one |
//! | `project.tags.list` | the project's tags in order, with how many items carry each |
//! | `project.tags.add` / `.edit` / `.delete` / `.move` | add a tag, edit its name and colour, delete it (from every item too), move it in the order |
//! | `project.filter.get` / `project.filter.set` | the Filter menu: tags, ratings, types, usage (OR within a group, AND across groups) |
//! | `project.setDefaultFolder` | the folder new sequences or audio captures go to |

use redrob_film_project::{BinId, BinTag, ItemId, Project};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, item_p, str_p, u64_p};
use crate::project_panel::{BinFilter, FILTER_TYPES, FILTER_USAGE};
use crate::{EngineError, Result, Session};

/// Most tags a project can have.
pub const MAX_TAGS: usize = 64;

fn items_p(s: &Session, p: &Value) -> Vec<ItemId> {
    match p.get("items").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(|v| v.as_u64().map(ItemId)).collect(),
        None => item_p(p, "item").map(|i| vec![i]).unwrap_or_else(|| s.state.project_selection.clone()),
    }
}

/// `#rrggbb` (or `rrggbb`) → `[r, g, b]`.
pub fn parse_color(v: &str) -> Option<[u8; 3]> {
    let h = v.trim().trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([c(0)?, c(2)?, c(4)?])
}

pub fn color_hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn tag_json(p: &Project, t: &BinTag) -> Value {
    let n = p.items.values().filter(|it| it.tags.contains(&t.id)).count();
    json!({"id": t.id, "name": t.name, "color": color_hex(t.color), "items": n})
}

fn tags_json(p: &Project) -> Value {
    let tags = p.bin_tags();
    json!({"tags": tags.iter().map(|t| tag_json(p, t)).collect::<Vec<_>>()})
}

fn tag_p(p: &Value, cmd: &str) -> Result<u32> {
    u64_p(p, "tag").and_then(|t| u32::try_from(t).ok()).ok_or_else(|| bad(cmd, "need `tag` (a tag id from project.tags.list)"))
}

fn checked_items(s: &Session, p: &Value, cmd: &str) -> Result<Vec<ItemId>> {
    let items = items_p(s, p);
    if items.is_empty() {
        return Err(bad(cmd, "select items in the Project panel"));
    }
    if let Some(i) = items.iter().find(|i| s.project.item(**i).is_none()) {
        return Err(bad(cmd, format!("no item {}", i.0)));
    }
    Ok(items)
}

// ------------------------------------------------------------------------------------ rating

fn set_rating(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "clip.setRating";
    let r = u64_p(p, "rating").ok_or_else(|| bad(CMD, "need `rating` (0–5 stars)"))?;
    if r > 5 {
        return Err(bad(CMD, "`rating` is 0–5 stars"));
    }
    let items = checked_items(s, p, CMD)?;
    s.edit("Rating", |pr, _| {
        for i in &items {
            if let Some(it) = pr.item_mut(*i) {
                it.rating = r as u8;
            }
        }
        Ok(())
    })?;
    Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "rating": r}))
}

// ------------------------------------------------------------------------------------ item tags

fn known_tag(p: &Project, id: u32) -> bool {
    p.bin_tags().iter().any(|t| t.id == id)
}

fn set_tags(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "clip.setTags";
    let tags: Vec<u32> = match p.get("tags").and_then(Value::as_array) {
        Some(a) => a.iter().map(|v| v.as_u64().and_then(|t| u32::try_from(t).ok()).ok_or_else(|| bad(CMD, "`tags` are tag ids"))).collect::<Result<_>>()?,
        None => return Err(bad(CMD, "need `tags`: [tag id]")),
    };
    if let Some(t) = tags.iter().find(|t| !known_tag(&s.project, **t)) {
        return Err(bad(CMD, format!("no tag {t}")));
    }
    let mut uniq: Vec<u32> = Vec::new();
    for t in tags {
        if !uniq.contains(&t) {
            uniq.push(t);
        }
    }
    let items = checked_items(s, p, CMD)?;
    s.edit("Edit Tags", |pr, _| {
        for i in &items {
            if let Some(it) = pr.item_mut(*i) {
                it.tags = uniq.clone();
            }
        }
        Ok(())
    })?;
    Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "tags": uniq}))
}

/// `clip.toggleTag {tag, on?, items?}`: without `on`, the tag is removed when every item has it
/// and added to all of them otherwise (the tag menu's check mark).
fn toggle_tag(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "clip.toggleTag";
    let tag = tag_p(p, CMD)?;
    if !known_tag(&s.project, tag) {
        return Err(bad(CMD, format!("no tag {tag}")));
    }
    let items = checked_items(s, p, CMD)?;
    let all = items.iter().all(|i| s.project.item(*i).is_some_and(|it| it.tags.contains(&tag)));
    let on = bool_p(p, "on").unwrap_or(!all);
    s.edit(if on { "Add Tag" } else { "Remove Tag" }, |pr, _| {
        for i in &items {
            if let Some(it) = pr.item_mut(*i) {
                if on {
                    if !it.tags.contains(&tag) {
                        it.tags.push(tag);
                    }
                } else {
                    it.tags.retain(|t| *t != tag);
                }
            }
        }
        Ok(())
    })?;
    Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "tag": tag, "on": on}))
}

// ------------------------------------------------------------------------------------ project tags

fn add_tag(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "project.tags.add";
    let mut tags = s.project.bin_tags();
    if tags.len() >= MAX_TAGS {
        return Err(bad(CMD, format!("a project has at most {MAX_TAGS} tags")));
    }
    let color = match str_p(p, "color") {
        Some(c) => parse_color(c).ok_or_else(|| bad(CMD, "`color` is #rrggbb"))?,
        None => [0x80, 0x80, 0x80],
    };
    let id = tags.iter().map(|t| t.id).max().unwrap_or(0) + 1;
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| format!("Tag {id}"));
    tags.push(BinTag { id, name, color });
    s.edit("Add Tag", |pr, _| {
        pr.tags = Some(tags);
        Ok(())
    })?;
    let mut out = tags_json(&s.project);
    out["tag"] = json!(id);
    Ok(out)
}

fn edit_tag(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "project.tags.edit";
    let id = tag_p(p, CMD)?;
    let mut tags = s.project.bin_tags();
    let t = tags.iter_mut().find(|t| t.id == id).ok_or_else(|| bad(CMD, format!("no tag {id}")))?;
    if let Some(n) = str_p(p, "name") {
        let n = n.trim();
        if n.is_empty() {
            return Err(bad(CMD, "a tag's name can't be empty"));
        }
        t.name = n.to_string();
    }
    if let Some(c) = str_p(p, "color") {
        t.color = parse_color(c).ok_or_else(|| bad(CMD, "`color` is #rrggbb"))?;
    }
    s.edit("Edit Tag", |pr, _| {
        pr.tags = Some(tags);
        Ok(())
    })?;
    Ok(tags_json(&s.project))
}

fn delete_tag(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "project.tags.delete";
    let id = tag_p(p, CMD)?;
    let mut tags = s.project.bin_tags();
    let before = tags.len();
    tags.retain(|t| t.id != id);
    if tags.len() == before {
        return Err(bad(CMD, format!("no tag {id}")));
    }
    s.edit("Delete Tag", |pr, _| {
        pr.tags = Some(tags);
        for it in pr.items.values_mut() {
            it.tags.retain(|t| *t != id);
        }
        Ok(())
    })?;
    // a filter on the deleted tag would hide everything
    if s.prefs.project_panel.filter.tags.contains(&id) {
        let mut prefs = s.prefs.clone();
        prefs.project_panel.filter.tags.retain(|t| *t != id);
        s.set_prefs(prefs).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
    }
    Ok(tags_json(&s.project))
}

/// `project.tags.move {tag, to}`: move a tag to position `to` (0-based) of the Tags bar.
fn move_tag(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "project.tags.move";
    let id = tag_p(p, CMD)?;
    let to = u64_p(p, "to").ok_or_else(|| bad(CMD, "need `to` (the new position, 0-based)"))? as usize;
    let mut tags = s.project.bin_tags();
    let from = tags.iter().position(|t| t.id == id).ok_or_else(|| bad(CMD, format!("no tag {id}")))?;
    let t = tags.remove(from);
    let to = to.min(tags.len());
    tags.insert(to, t);
    s.edit("Move Tag", |pr, _| {
        pr.tags = Some(tags);
        Ok(())
    })?;
    Ok(tags_json(&s.project))
}

// ------------------------------------------------------------------------------------ filter

fn filter_json(s: &Session) -> Value {
    let f = &s.prefs.project_panel.filter;
    json!({
        "filter": f,
        "active": f.is_active(),
        "types": FILTER_TYPES.iter().map(|(k, l)| json!({"key": k, "label": l})).collect::<Vec<_>>(),
        "usage": FILTER_USAGE.iter().map(|(k, l)| json!({"key": k, "label": l})).collect::<Vec<_>>(),
    })
}

fn u_list<T: TryFrom<u64>>(v: &Value, cmd: &str, what: &str) -> Result<Vec<T>> {
    let a = v.as_array().ok_or_else(|| bad(cmd, format!("`{what}` is a list")))?;
    let mut out: Vec<T> = Vec::new();
    for x in a {
        let n = x.as_u64().and_then(|n| T::try_from(n).ok()).ok_or_else(|| bad(cmd, format!("bad entry in `{what}`")))?;
        out.push(n);
    }
    Ok(out)
}

fn s_list(v: &Value, cmd: &str, what: &str, allowed: &[(&str, &str)]) -> Result<Vec<String>> {
    let a = v.as_array().ok_or_else(|| bad(cmd, format!("`{what}` is a list")))?;
    let mut out: Vec<String> = Vec::new();
    for x in a {
        let k = x.as_str().ok_or_else(|| bad(cmd, format!("bad entry in `{what}`")))?;
        let k = allowed
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(k))
            .map(|(key, _)| key.to_string())
            .ok_or_else(|| bad(cmd, format!("unknown {what} `{k}` ({})", allowed.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", "))))?;
        if !out.contains(&k) {
            out.push(k);
        }
    }
    Ok(out)
}

/// `project.filter.set {tags?, ratings?, types?, usage?, clear?}`: replace the groups given
/// (`clear` resets every group first).
fn filter_set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "project.filter.set";
    let mut f = if bool_p(p, "clear").unwrap_or(false) { BinFilter::default() } else { s.prefs.project_panel.filter.clone() };
    if let Some(v) = p.get("tags") {
        f.tags = u_list::<u32>(v, CMD, "tags")?;
    }
    if let Some(v) = p.get("ratings") {
        let r = u_list::<u8>(v, CMD, "ratings")?;
        if r.iter().any(|x| *x > 5) {
            return Err(bad(CMD, "ratings are 0–5"));
        }
        f.ratings = r;
    }
    if let Some(v) = p.get("types") {
        f.types = s_list(v, CMD, "types", &FILTER_TYPES)?;
    }
    if let Some(v) = p.get("usage") {
        f.usage = s_list(v, CMD, "usage", &FILTER_USAGE)?;
    }
    if f != s.prefs.project_panel.filter {
        let mut prefs = s.prefs.clone();
        prefs.project_panel.filter = f;
        s.set_prefs(prefs).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
    }
    Ok(filter_json(s))
}

// ------------------------------------------------------------------------------------ folders

/// `project.setDefaultFolder {kind: "sequences"|"audioCapture", bin: binId|null}`.
fn set_default_folder(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "project.setDefaultFolder";
    let kind = str_p(p, "kind").ok_or_else(|| bad(CMD, "need `kind`: sequences | audioCapture"))?.to_string();
    let bin = match p.get("bin") {
        None | Some(Value::Null) => None,
        Some(b) => Some(BinId(b.as_u64().ok_or_else(|| bad(CMD, "`bin` is a bin id or null"))?)),
    };
    if let Some(b) = bin
        && s.project.root.find_bin(b).is_none()
    {
        return Err(bad(CMD, format!("no bin {}", b.0)));
    }
    // the root bin is the same as no default
    let bin = bin.filter(|b| *b != s.project.root.id);
    s.edit("Set Default Folder", |pr, _| {
        match kind.as_str() {
            "sequences" => pr.settings.sequence_bin = bin,
            "audioCapture" => pr.settings.capture_bin = bin,
            other => return Err(bad(CMD, format!("unknown kind `{other}` (sequences, audioCapture)"))),
        }
        Ok(())
    })?;
    Ok(folders_json(&s.project))
}

fn folders_json(p: &Project) -> Value {
    json!({"sequences": p.default_bin(p.settings.sequence_bin).map(|b| b.0), "audioCapture": p.default_bin(p.settings.capture_bin).map(|b| b.0)})
}

// ------------------------------------------------------------------------------------ registry

pub(crate) fn commands() -> Vec<CommandSpec> {
    let c = |id, label, params, enabled, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled, run, journal: true };
    let q = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        c("clip.setRating", "Rating", r#"{"rating":0..5,"items":[id]?}"#, always, set_rating),
        c("clip.setTags", "Tags", r#"{"tags":[tagId],"items":[id]?}"#, always, set_tags),
        c("clip.toggleTag", "Toggle Tag", r#"{"tag":tagId,"on":bool?,"items":[id]?}"#, always, toggle_tag),
        q("project.tags.list", "List Tags", "{}", |s, _| Ok(tags_json(&s.project))),
        c("project.tags.add", "Add Tag", r##"{"name":str?,"color":"#rrggbb"?}"##, always, add_tag),
        c("project.tags.edit", "Edit Tag", r##"{"tag":tagId,"name":str?,"color":"#rrggbb"?}"##, always, edit_tag),
        c("project.tags.delete", "Delete Tag", r#"{"tag":tagId}"#, always, delete_tag),
        c("project.tags.move", "Move Tag", r#"{"tag":tagId,"to":index}"#, always, move_tag),
        q("project.filter.get", "Project Filter", "{}", |s, _| Ok(filter_json(s))),
        c(
            "project.filter.set",
            "Filter Project Items",
            r#"{"tags":[tagId]?,"ratings":[0..5]?,"types":["av|video|audio|image|imageSequence|generator|title|sequence|subclip|adjustment"]?,"usage":["used|unused"]?,"clear":bool?}"#,
            always,
            filter_set,
        ),
        q("project.defaultFolders", "Default Folders", "{}", |s, _| Ok(folders_json(&s.project))),
        c("project.setDefaultFolder", "Set Default Folder", r#"{"kind":"sequences|audioCapture","bin":binId|null}"#, always, set_default_folder),
    ]
}
