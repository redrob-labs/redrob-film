//! Manage Cached Data (Kdenlive's Project ▸ Manage Cached Data): what the current project and all
//! projects keep on disk, by kind, and deleting a kind (optionally only files older than N days).
//!
//! Kinds: the current project's render previews, proxies and backups; every project's backups;
//! the shared media cache (render previews of unsaved projects and other rebuildable files).
//! Commands: `cache.usage` (sizes) and `cache.delete {kind, olderThanDays?}`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, str_p, u64_p};
use crate::{Result, Session};

/// (path, bytes, modified) of every file under `dir`.
fn files_under(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(p),
                Ok(m) => out.push((p, m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH))),
                Err(_) => {}
            }
        }
    }
    out
}

/// The proxy files of the current project's items that exist: (item, path, bytes).
fn current_proxies(s: &Session) -> Vec<(u64, PathBuf, u64)> {
    s.project
        .items
        .values()
        .filter_map(|it| {
            let m = it.as_media()?;
            let redrob_film_project::MediaRef::File { path } = m.proxy.as_ref()? else { return None };
            let len = std::fs::metadata(path).ok()?.len();
            Some((it.id.0, PathBuf::from(path), len))
        })
        .collect()
}

fn all_backups_dir(s: &Session) -> Option<PathBuf> {
    s.prefs_path.as_ref().and_then(|p| p.parent()).map(|d| d.join("Backups"))
}

/// Each kind: (id, label, scope, folder or None for scattered files, files).
fn kinds(s: &Session) -> Vec<(&'static str, &'static str, &'static str, Option<PathBuf>, Vec<(PathBuf, u64, std::time::SystemTime)>)> {
    let mut v = Vec::new();
    let previews = s.previews.dir();
    v.push(("currentPreviews", "Render previews", "current", previews.clone(), previews.as_deref().map(files_under).unwrap_or_default()));
    let proxies = current_proxies(s).into_iter().map(|(_, p, n)| (p, n, std::time::UNIX_EPOCH)).collect();
    v.push(("currentProxies", "Proxy clips", "current", None, proxies));
    let backups = s.path.as_deref().map(|p| crate::backups::backup_dir(s, p));
    v.push(("currentBackups", "Backups", "current", backups.clone(), backups.as_deref().map(files_under).unwrap_or_default()));
    let all = all_backups_dir(s);
    v.push(("allBackups", "Backups of all projects", "all", all.clone(), all.as_deref().map(files_under).unwrap_or_default()));
    let cache = s.media_cache_dir();
    // the open project's previews are listed above, not here
    let cache_files =
        cache.as_deref().map(files_under).unwrap_or_default().into_iter().filter(|f| previews.as_deref().is_none_or(|d| !f.0.starts_with(d))).collect();
    v.push(("mediaCache", "Media cache (all projects)", "all", cache, cache_files));
    v
}

fn usage(s: &mut Session, _: &Value) -> Result<Value> {
    let cats: Vec<Value> = kinds(s)
        .into_iter()
        .map(|(id, label, scope, dir, files)| {
            let bytes: u64 = files.iter().map(|f| f.1).sum();
            json!({"id": id, "label": label, "scope": scope, "path": dir, "files": files.len(), "bytes": bytes})
        })
        .collect();
    Ok(json!({"categories": cats}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "cache.delete";
    let kind = str_p(p, "kind").ok_or_else(|| bad(cmd, "need `kind` (see cache.usage)"))?.to_string();
    let older = u64_p(p, "olderThanDays").map(|d| std::time::Duration::from_secs(d * 86_400));
    let now = std::time::SystemTime::now();
    let old_enough = |t: std::time::SystemTime| older.is_none_or(|age| now.duration_since(t).is_ok_and(|a| a > age));
    if kind == "currentProxies" {
        let list = current_proxies(s);
        let mut items = Vec::new();
        let (mut n, mut bytes) = (0, 0);
        for (item, path, len) in list {
            if std::fs::remove_file(&path).is_ok() {
                items.push(item);
                n += 1;
                bytes += len;
            }
        }
        if !items.is_empty() {
            s.execute("media.detachProxies", json!({"items": items}))?;
        }
        return Ok(json!({"kind": kind, "files": n, "bytes": bytes}));
    }
    let Some((_, _, _, _, files)) = kinds(s).into_iter().find(|k| k.0 == kind) else {
        return Err(bad(cmd, format!("unknown kind `{kind}`")));
    };
    let (mut n, mut bytes) = (0usize, 0u64);
    for (path, len, t) in files {
        if old_enough(t) && std::fs::remove_file(&path).is_ok() {
            n += 1;
            bytes += len;
        }
    }
    if kind == "currentPreviews" {
        // the store forgets the deleted segments (the render bar turns red again)
        s.previews.set_dir(s.previews.dir());
    }
    Ok(json!({"kind": kind, "files": n, "bytes": bytes}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec { id: "cache.usage", label: "Cached Data Usage", menu: &[], shortcut: None, params: "{}", enabled: always, run: usage, journal: false },
        CommandSpec {
            id: "cache.delete",
            label: "Delete Cached Data",
            menu: &[],
            shortcut: None,
            params: r#"{"kind":"currentPreviews|currentProxies|currentBackups|allBackups|mediaCache","olderThanDays":u64?}"#,
            enabled: always,
            run: delete,
            journal: true,
        },
    ]
}
