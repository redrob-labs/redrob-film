//! Render extras around the export pipeline (Kdenlive's Render dialog): what happens after an
//! export finishes, render scripts saved for later, one export per marker section and one audio
//! file per audio track.
//!
//! - **After export** ([`AfterExport`], the `after` object of `file.exportMedia`,
//!   `export.queue.add` and `export.scripts.*`): open the output's folder, play the output, add it
//!   to the project, keep a log file next to it (`<output>.log.json`: the settings, the result or
//!   the error) and shut the computer down once every export has finished. [`poll`] runs them when
//!   the job ends; opening and shutting down are [`Event::OpenPath`] / [`Event::Shutdown`] for the
//!   host.
//! - **Finished outputs** (`export.output.open`, `export.output.addToProject`): the export queue's
//!   Play / Open Folder / Add to Project buttons on a finished item.
//! - **Render scripts** (`export.scripts.{save,list,start,delete}`): the project as it is now, the
//!   sequence, the resolved settings and the after-export actions, saved under
//!   `<data dir>/render-scripts/` (`<name>.json` + `<name>.project`) and rendered later, also after
//!   the project has changed or closed.
//! - **Marker sections** (`export.queue.addMarkers`): one queued export per section between two
//!   consecutive sequence markers of the chosen colours (the last up to the sequence end), the file
//!   named after the section's first marker.
//! - **Audio stems** (`separateAudioTracks` in `after`): besides the main export, one WAV file per
//!   audio track with clips, each rendered with every other audio track muted.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use redrob_film_export::{ExportSettings, Format};
use redrob_film_project::{ItemId, Label, Project};
use redrob_film_time::{Tick, TimeRange};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, has_seq, str_p};
use crate::export_tools::{range_param, sequence_param, settings_from_params, spawn_export};
use crate::{EngineError, Event, Result, Session};

/// What to do when an export finishes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AfterExport {
    /// Reveal the output in the file manager.
    pub open_folder: bool,
    /// Open the output in the system's default player.
    pub play: bool,
    /// Import the output into the project.
    pub add_to_project: bool,
    /// Write `<output>.log.json` (settings, result or error) next to the output.
    pub keep_log: bool,
    /// Shut the computer down once every export (and the queue) has finished.
    pub shutdown: bool,
    /// Also export one WAV file per audio track (the other audio tracks muted).
    pub separate_audio_tracks: bool,
}

impl AfterExport {
    /// The `after` object of a command's parameters (nothing when absent or malformed).
    pub fn from_params(p: &Value) -> AfterExport {
        p.get("after").filter(|v| v.is_object()).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    /// Whether any action is asked for once the job ends.
    pub fn any(&self) -> bool {
        self.open_folder || self.play || self.add_to_project || self.keep_log || self.shutdown
    }
}

/// An export job whose after-export actions run when it ends.
#[derive(Clone, Debug)]
pub struct PostExport {
    pub job: u64,
    pub after: AfterExport,
    pub settings: ExportSettings,
    pub sequence_name: String,
    pub started_unix: i64,
}

/// Seconds since the Unix epoch.
pub fn now_unix() -> i64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Remember `after` for `job` (and arm the shutdown) when it asks for something.
pub fn watch(s: &mut Session, job: u64, after: AfterExport, settings: &ExportSettings, sequence_name: &str) {
    if after.shutdown {
        s.shutdown_armed = true;
    }
    if after.any() {
        s.post_exports.push(PostExport { job, after, settings: settings.clone(), sequence_name: sequence_name.to_string(), started_unix: now_unix() });
    }
}

/// Export `seq` with `settings` and start the audio stems when `after` asks for them; registers the
/// after-export actions. Returns the main job's id.
pub fn spawn_with_after(s: &mut Session, project: Arc<Project>, seq: ItemId, settings: ExportSettings, label: String, wait: bool, after: AfterExport) -> Result<u64> {
    let settings = settings.normalized();
    let name = project.item(seq).map(|i| i.name.clone()).unwrap_or_default();
    let stems = if after.separate_audio_tracks { stem_jobs(&project, seq, &settings) } else { Vec::new() };
    let id = spawn_export(s, project.clone(), seq, settings.clone(), label, wait)?;
    watch(s, id, after, &settings, &name);
    for (p, st, track) in stems {
        let file = Path::new(&st.path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let job = spawn_export(s, p, seq, st.clone(), format!("Audio stem {track} → {file}"), wait)?;
        // a stem keeps its own log and joins the shutdown wait; it is not played or imported
        watch(s, job, AfterExport { keep_log: after.keep_log, ..Default::default() }, &st, &name);
    }
    Ok(id)
}

/// One WAV export per audio track that has clips: the project with every other audio track muted.
fn stem_jobs(project: &Arc<Project>, seq: ItemId, settings: &ExportSettings) -> Vec<(Arc<Project>, ExportSettings, String)> {
    let Some(q) = project.sequence(seq) else { return Vec::new() };
    let base = settings.path.rsplit_once('.').map_or(settings.path.as_str(), |x| x.0).to_string();
    let mut out = Vec::new();
    for (i, t) in q.audio_tracks.iter().enumerate() {
        if t.items.is_empty() {
            continue;
        }
        let mut p = (**project).clone();
        if let Some(q2) = p.sequence_mut(seq) {
            for (j, t2) in q2.audio_tracks.iter_mut().enumerate() {
                t2.solo = false;
                if j != i {
                    t2.muted = true;
                } else {
                    t2.muted = false;
                }
            }
        }
        let mut st = ExportSettings { format: Format::Wav, path: String::new(), range: settings.range, include_audio: true, ..Default::default() };
        st.audio.sample_rate = settings.audio.sample_rate;
        st.audio.bits = settings.audio.bits.max(16);
        st.audio.channels = settings.audio.channels;
        let label = if t.name.trim().is_empty() { format!("A{}", i + 1) } else { t.name.clone() };
        st.path = format!("{base}_{}.wav", safe_name(&label));
        out.push((Arc::new(p), st, label));
    }
    out
}

/// A name usable in a file name.
pub fn safe_name(n: &str) -> String {
    let s: String = n.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') { c } else { '_' }).collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "untitled".into() } else { s }
}

/// Run the after-export actions of finished jobs and send the shutdown once nothing is left.
pub fn poll(s: &mut Session) {
    let mut i = 0;
    while i < s.post_exports.len() {
        let job = s.post_exports[i].job;
        let Some(j) = s.jobs.iter().find(|j| j.id == job) else {
            s.post_exports.remove(i);
            continue;
        };
        let res = j.result.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(res) = res else {
            i += 1;
            continue;
        };
        let pe = s.post_exports.remove(i);
        finish(s, pe, res);
    }
    if s.shutdown_armed && s.post_exports.is_empty() && !s.export_queue.is_active() && !s.jobs.iter().any(|j| j.result.lock().unwrap_or_else(|e| e.into_inner()).is_none())
    {
        s.shutdown_armed = false;
        s.log.push(crate::panels::Level::Info, "export", "All exports finished: shutting the computer down".to_string());
        s.events.push(Event::Shutdown);
    }
}

fn finish(s: &mut Session, pe: PostExport, res: std::result::Result<redrob_film_export::Report, String>) {
    if pe.after.keep_log {
        let log = json!({
            "format": "filmcraft.render-log",
            "sequence": pe.sequence_name,
            "startedUnix": pe.started_unix,
            "finishedUnix": now_unix(),
            "settings": serde_json::to_value(&pe.settings).unwrap_or_default(),
            "result": match &res { Ok(r) => serde_json::to_value(r).unwrap_or_default(), Err(e) => json!({"error": e}) },
        });
        let path = format!("{}.log.json", pe.settings.path);
        let bytes = serde_json::to_vec_pretty(&log).unwrap_or_default();
        if let Err(e) = s.services.write_file(&path, &bytes) {
            s.log.push(crate::panels::Level::Error, "export", format!("{path}: {e}"));
        }
    }
    let Ok(report) = res else { return };
    if pe.after.add_to_project {
        let seq = pe.settings.is_image_sequence();
        if let Err(e) = s.execute("file.import", json!({"paths": [report.path], "imageSequence": seq})) {
            s.log.push(crate::panels::Level::Error, "export", format!("adding {} to the project: {e}", report.path));
        }
    }
    if pe.after.open_folder {
        s.events.push(Event::OpenPath { path: report.path.clone(), reveal: true });
    }
    if pe.after.play {
        s.events.push(Event::OpenPath { path: report.path.clone(), reveal: false });
    }
}

// ---------------------------------------------------------------------------------------------
// finished outputs
// ---------------------------------------------------------------------------------------------

fn output_path_p(s: &Session, p: &Value, cmd: &str) -> Result<String> {
    if let Some(path) = str_p(p, "path") {
        return Ok(path.to_string());
    }
    let id = crate::commands::u64_p(p, "id").ok_or_else(|| bad(cmd, "need `path` or a queue item `id`"))?;
    let it = s.export_queue.items.iter().find(|i| i.id == id).ok_or_else(|| bad(cmd, format!("no queued export {id}")))?;
    // the file the job wrote (an export without its video stream writes the audio file)
    let written = it.job.and_then(|j| s.jobs.iter().find(|x| x.id == j)).and_then(|j| match &*j.result.lock().unwrap_or_else(|e| e.into_inner()) {
        Some(Ok(r)) => Some(r.path.clone()),
        _ => None,
    });
    Ok(written.unwrap_or_else(|| it.settings.normalized().path))
}

fn output_open(s: &mut Session, p: &Value) -> Result<Value> {
    let path = output_path_p(s, p, "export.output.open")?;
    let reveal = bool_p(p, "reveal").unwrap_or(false);
    s.events.push(Event::OpenPath { path: path.clone(), reveal });
    Ok(json!({"path": path, "reveal": reveal}))
}

fn output_add(s: &mut Session, p: &Value) -> Result<Value> {
    let path = output_path_p(s, p, "export.output.addToProject")?;
    let seq = Format::from_name(Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("")).is_some_and(|f| f.is_image_sequence());
    s.execute("file.import", json!({"paths": [path], "imageSequence": seq}))
}

// ---------------------------------------------------------------------------------------------
// render scripts
// ---------------------------------------------------------------------------------------------

pub const SCRIPT_FORMAT: &str = "filmcraft.render-script";

/// A render script: an export saved to run later.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderScript {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub created_unix: i64,
    pub sequence: u64,
    pub sequence_name: String,
    /// The project file the script was made from (informational: the project itself is saved
    /// beside the script).
    #[serde(default)]
    pub project_path: Option<String>,
    pub settings: ExportSettings,
    #[serde(default)]
    pub after: AfterExport,
}

/// Where render scripts are kept: `<data dir>/render-scripts`.
pub fn scripts_dir(s: &Session) -> PathBuf {
    s.export_presets
        .dir()
        .map(Path::to_path_buf)
        .or_else(|| s.prefs_path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf))
        .unwrap_or_else(|| crate::temp_dir().join("Redrob Film"))
        .join("render-scripts")
}

fn script_paths(s: &Session, name: &str) -> (PathBuf, PathBuf) {
    let d = scripts_dir(s);
    let n = safe_name(name);
    (d.join(format!("{n}.json")), d.join(format!("{n}.project")))
}

fn read_script(path: &Path) -> std::result::Result<RenderScript, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let sc: RenderScript = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if sc.format != SCRIPT_FORMAT {
        return Err(format!("{}: not a render script", path.display()));
    }
    Ok(sc)
}

fn scripts_save(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "export.scripts.save";
    let seq = sequence_param(s, p, cmd)?;
    let (_, mut settings) = settings_from_params(s, p, cmd)?;
    let project = s.project.clone();
    settings.range = range_param(s, &project, seq, p.get("range"), p, cmd)?;
    settings.path = crate::export_tools::resolve_output_path(s, p, seq, &settings);
    let seq_name = project.item(seq).map(|i| i.name.clone()).unwrap_or_default();
    let name = match str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => {
            let stem = Path::new(&settings.path).file_stem().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| seq_name.clone());
            format!("{stem} {}", now_unix())
        }
    };
    let (json_path, project_path) = script_paths(s, &name);
    if json_path.exists() && !bool_p(p, "overwrite").unwrap_or(false) {
        return Err(bad(cmd, format!("a render script named `{name}` exists (pass overwrite)")));
    }
    let sc = RenderScript {
        format: SCRIPT_FORMAT.into(),
        version: 1,
        name: name.clone(),
        created_unix: now_unix(),
        sequence: seq.0,
        sequence_name: seq_name,
        project_path: s.path.clone(),
        settings,
        after: AfterExport::from_params(p),
    };
    if let Some(d) = json_path.parent() {
        std::fs::create_dir_all(d).map_err(|e| EngineError::Other(format!("{}: {e}", d.display())))?;
    }
    let bytes = redrob_film_format::encode_with_view(&project, None, false);
    redrob_film_format::atomic_write(&project_path, &bytes).map_err(|e| EngineError::Other(format!("{}: {e}", project_path.display())))?;
    let js = serde_json::to_vec_pretty(&sc).map_err(|e| EngineError::Other(e.to_string()))?;
    redrob_film_format::atomic_write(&json_path, &js).map_err(|e| EngineError::Other(format!("{}: {e}", json_path.display())))?;
    Ok(json!({"name": name, "path": json_path, "output": sc.settings.path}))
}

fn scripts_list(s: &mut Session, _: &Value) -> Result<Value> {
    let dir = scripts_dir(s);
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            match read_script(&path) {
                Ok(sc) => out.push(json!({
                    "name": sc.name,
                    "path": path,
                    "sequenceName": sc.sequence_name,
                    "output": sc.settings.path,
                    "format": sc.settings.format.id(),
                    "createdUnix": sc.created_unix,
                    "projectPath": sc.project_path,
                })),
                Err(e) => log::warn!("{e}"),
            }
        }
    }
    out.sort_by(|a, b| a["createdUnix"].as_i64().cmp(&b["createdUnix"].as_i64()));
    Ok(json!({"dir": dir, "scripts": out}))
}

fn scripts_start(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "export.scripts.start";
    let name = str_p(p, "name").ok_or_else(|| bad(cmd, "need `name` (see export.scripts.list)"))?;
    let (json_path, project_path) = script_paths(s, name);
    let sc = read_script(&json_path).map_err(|e| bad(cmd, e))?;
    let bytes = std::fs::read(&project_path).map_err(|e| EngineError::Other(format!("{}: {e}", project_path.display())))?;
    let loaded = redrob_film_format::decode(&bytes).map_err(|e| EngineError::Other(format!("{}: {e}", project_path.display())))?;
    let project = Arc::new(loaded.project);
    let seq = ItemId(sc.sequence);
    if project.sequence(seq).is_none() {
        return Err(bad(cmd, "the script's sequence is not in its saved project"));
    }
    // the media pool opens the saved project's media on demand (keyed by path, so a changed
    // current project does not mix in)
    let wait = bool_p(p, "wait").unwrap_or(false);
    let file = Path::new(&sc.settings.path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let job = spawn_with_after(s, project, seq, sc.settings.clone(), format!("Script {} → {file}", sc.name), wait, sc.after)?;
    Ok(json!({"job": job, "name": sc.name, "output": sc.settings.path}))
}

fn scripts_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "export.scripts.delete";
    let name = str_p(p, "name").ok_or_else(|| bad(cmd, "need `name` (see export.scripts.list)"))?;
    let (json_path, project_path) = script_paths(s, name);
    if !json_path.exists() {
        return Err(bad(cmd, format!("no render script named `{name}`")));
    }
    std::fs::remove_file(&json_path).map_err(|e| EngineError::Other(format!("{}: {e}", json_path.display())))?;
    let _ = std::fs::remove_file(&project_path);
    Ok(json!({"deleted": name}))
}

// ---------------------------------------------------------------------------------------------
// marker sections
// ---------------------------------------------------------------------------------------------

/// The sections between consecutive sequence markers whose colour is in `colors` (all markers
/// when empty), the last one up to the sequence end: (range, first marker's name).
pub fn marker_sections(project: &Project, seq: ItemId, colors: &[Label]) -> Vec<(TimeRange, String)> {
    let Some(q) = project.sequence(seq) else { return Vec::new() };
    let mut ms: Vec<_> = q.markers.iter().filter(|m| colors.is_empty() || colors.contains(&m.color)).collect();
    ms.sort_by_key(|m| m.start);
    let end = q.duration().max(q.settings.frame_rate.frame_duration());
    let mut out = Vec::new();
    for (i, m) in ms.iter().enumerate() {
        let b = ms.get(i + 1).map(|n| n.start).unwrap_or(end);
        if b <= m.start || m.start < Tick::ZERO {
            continue;
        }
        out.push((TimeRange::from_bounds(m.start, b), m.name.clone()));
    }
    out
}

fn color_list(p: &Value, cmd: &str) -> Result<Vec<Label>> {
    let mut out = Vec::new();
    if let Some(a) = p.get("colors").and_then(Value::as_array) {
        for v in a {
            let n = v.as_str().unwrap_or("");
            let l = Label::ALL.iter().find(|l| l.name().eq_ignore_ascii_case(n)).ok_or_else(|| bad(cmd, format!("unknown marker colour `{n}`")))?;
            out.push(*l);
        }
    }
    Ok(out)
}

fn queue_markers(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "export.queue.addMarkers";
    let seq = sequence_param(s, p, cmd)?;
    let (_, settings) = settings_from_params(s, p, cmd)?;
    let colors = color_list(p, cmd)?;
    let sections = marker_sections(&s.project, seq, &colors);
    if sections.is_empty() {
        return Err(bad(cmd, "the sequence has no markers of those colours"));
    }
    let base = crate::export_tools::resolve_output_path(s, p, seq, &settings);
    let (stem, ext) = base.rsplit_once('.').map_or((base.clone(), String::new()), |(a, e)| (a.to_string(), format!(".{e}")));
    let mut added = Vec::new();
    let n = sections.len();
    for (i, (r, name)) in sections.into_iter().enumerate() {
        let mut q = p.clone();
        let label = if name.trim().is_empty() { format!("{:0w$}", i + 1, w = n.to_string().len()) } else { safe_name(&name) };
        q["path"] = json!(format!("{stem}_{label}{ext}"));
        q["range"] = json!({"mode": "custom", "startTime": r.start.0, "endTime": r.end().0});
        q["sequence"] = json!(seq.0);
        q["start"] = json!(false);
        if let Some(o) = q.as_object_mut() {
            o.remove("sequences");
            o.remove("ranges");
        }
        let v = s.execute("export.queue.add", q)?;
        added.extend(v["added"].as_array().cloned().unwrap_or_default());
    }
    if bool_p(p, "start").unwrap_or(false) {
        s.execute("export.queue.start", json!({"wait": bool_p(p, "wait").unwrap_or(false)}))?;
    }
    Ok(json!({"added": added, "sections": n}))
}

fn spec(id: &'static str, label: &'static str, params: &'static str, enabled: fn(&Session) -> std::result::Result<(), String>, run: fn(&mut Session, &Value) -> Result<Value>, journal: bool) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled, run, journal }
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        spec("export.output.open", "Open Export Output", r#"{"path":str?,"id":queueId?,"reveal":bool=false}"#, always, output_open, false),
        spec("export.output.addToProject", "Add Export Output to Project", r#"{"path":str?,"id":queueId?}"#, always, output_add, true),
        spec(
            "export.scripts.save",
            "Generate Render Script",
            r#"{"name":str?,"overwrite":bool?,"path":str|dir?,"range":range?,"after":AfterExport?, …settings params}"#,
            has_seq,
            scripts_save,
            false,
        ),
        spec("export.scripts.list", "List Render Scripts", "{}", always, scripts_list, false),
        spec("export.scripts.start", "Start Render Script", r#"{"name":str,"wait":bool=false}"#, always, scripts_start, false),
        spec("export.scripts.delete", "Delete Render Script", r#"{"name":str}"#, always, scripts_delete, false),
        spec(
            "export.queue.addMarkers",
            "Queue Marker Sections",
            r#"{"colors":[label]?,"path":str|dir?,"start":bool?,"wait":bool?,"after":AfterExport?, …settings params}"#,
            has_seq,
            queue_markers,
            true,
        ),
    ]
}
