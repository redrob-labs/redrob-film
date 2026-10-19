//! Project backups (Kdenlive's Open Backup File): every save also keeps a dated copy of the project
//! file, and older copies are thinned into tiers so the folder stays small.
//!
//! - **Where.** `<data dir>/Backups/<project file stem>/<stem> <unix seconds>.rfilm` (the data
//!   directory of the preferences; next to the project in `Backups/` without one).
//! - **When.** On every Save / Save As of a project file ([`on_save`]); a copy identical to the
//!   newest backup is not written again.
//! - **Retention** ([`prune`]): the last hour keeps up to 10 copies, the last day one per hour,
//!   the last week one per day, older ones one per week (at most 8).
//! - **Restore** (`file.openBackup {path}`): the backup becomes the current project, unsaved and
//!   still pointing at the original file, so Save writes it back there. Without `path` the
//!   frontend shows the list (`file.listBackups`).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session};

/// The folder holding the backups of the project at `project`.
pub fn backup_dir(s: &Session, project: &str) -> PathBuf {
    let stem = Path::new(project).file_stem().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let base = s
        .prefs_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.join("Backups"))
        .unwrap_or_else(|| Path::new(project).parent().unwrap_or(Path::new(".")).join("Backups"));
    base.join(stem)
}

/// The backups of `project`: (path, unix seconds), newest first.
pub fn list(s: &Session, project: &str) -> Vec<(PathBuf, i64)> {
    let dir = backup_dir(s, project);
    let mut v: Vec<(PathBuf, i64)> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let stem = p.file_stem()?.to_string_lossy().into_owned();
                    let unix = stem.rsplit(' ').next()?.parse::<i64>().ok()?;
                    (p.extension().and_then(|x| x.to_str()) == Some("rfilm")).then_some((p, unix))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

/// The retention tier of a backup `age` seconds old: (tier name, bucket within the tier).
fn tier(age: i64) -> (&'static str, i64) {
    match age {
        a if a < 3600 => ("lastHour", a),
        a if a < 86_400 => ("lastDay", a / 3600),
        a if a < 7 * 86_400 => ("lastWeek", a / 86_400),
        a => ("older", a / (7 * 86_400)),
    }
}

/// Thin the backups of `project` into the tiers; returns how many were deleted.
pub fn prune(s: &Session, project: &str, now: i64) -> usize {
    let mut seen: Vec<(&'static str, i64)> = Vec::new();
    let (mut hour, mut older) = (0, 0);
    let mut deleted = 0;
    for (p, unix) in list(s, project) {
        let (t, bucket) = tier((now - unix).max(0));
        let keep = match t {
            "lastHour" => {
                hour += 1;
                hour <= 10
            }
            "older" if seen.iter().any(|x| *x == (t, bucket)) => false,
            "older" => {
                older += 1;
                seen.push((t, bucket));
                older <= 8
            }
            _ if seen.iter().any(|x| *x == (t, bucket)) => false,
            _ => {
                seen.push((t, bucket));
                true
            }
        };
        if !keep && std::fs::remove_file(&p).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

/// A save of `project` with `bytes` happened: keep a dated copy and thin the older ones.
pub fn on_save(s: &mut Session, project: &str, bytes: &[u8]) {
    // only real files on the local disk (not a host's virtual file system)
    if cfg!(target_arch = "wasm32") || s.services.export_in_memory() || !Path::new(project).is_file() {
        return;
    }
    let dir = backup_dir(s, project);
    if let Some((newest, _)) = list(s, project).first()
        && std::fs::read(newest).is_ok_and(|b| b == bytes)
    {
        return;
    }
    let now = crate::render_jobs::now_unix();
    let stem = Path::new(project).file_stem().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let path = dir.join(format!("{stem} {now}.rfilm"));
    let r = std::fs::create_dir_all(&dir).and_then(|_| redrob_film_format::atomic_write(&path, bytes));
    match r {
        Ok(()) => {
            prune(s, project, now);
        }
        Err(e) => s.log.push(crate::panels::Level::Warning, "backup", format!("{}: {e}", path.display())),
    }
}

fn project_param(s: &Session, p: &Value, cmd: &str) -> Result<String> {
    str_p(p, "project").map(str::to_string).or_else(|| s.path.clone()).ok_or_else(|| bad(cmd, "the project has not been saved yet (no backups)"))
}

fn list_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let project = project_param(s, p, "file.listBackups")?;
    let now = crate::render_jobs::now_unix();
    let items: Vec<Value> = list(s, &project)
        .into_iter()
        .map(|(path, unix)| {
            let size = std::fs::metadata(&path).map_or(0, |m| m.len());
            let time = s.persistence.as_ref().map_or_else(|| redrob_film_format::autosave::display_time(unix, 0), |per| per.local_time(unix));
            json!({"path": path, "unix": unix, "time": time, "tier": tier((now - unix).max(0)).0, "bytes": size})
        })
        .collect();
    Ok(json!({"project": project, "dir": backup_dir(s, &project), "backups": items}))
}

fn open_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "file.openBackup";
    let path = str_p(p, "path").ok_or_else(|| bad(cmd, "need `path` (see file.listBackups)"))?.to_string();
    let original = s.path.clone();
    let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let loaded = redrob_film_format::decode(&bytes).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    crate::commands::install_unsaved(s, loaded.project, original.clone());
    if let Some(seq) = s.state.active_sequence {
        s.events.push(crate::Event::OpenSequence(seq));
    }
    s.toast("Backup restored: save to keep it");
    Ok(json!({"restored": path, "project": original}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "file.listBackups",
            label: "List Project Backups",
            menu: &[],
            shortcut: None,
            params: r#"{"project":path?}"#,
            enabled: always,
            run: list_cmd,
            journal: false,
        },
        CommandSpec {
            id: "file.openBackup",
            label: "Open Backup File…",
            menu: &["File"],
            shortcut: None,
            params: r#"{"path":str}  (without it the frontend lists the backups)"#,
            enabled: |s| if s.path.is_some() { Ok(()) } else { Err("the project has not been saved yet".into()) },
            run: open_cmd,
            journal: true,
        },
    ]
}
