//! Clip jobs (Kdenlive's Project Bin ▸ Clip Jobs and the bin item menu): background jobs that
//! render a project clip into a new file and add it to the project, plus the bin-to-timeline
//! commands of the same menu.
//!
//! - **Stabilize** (`clip.stabilizeJob`): the clip through Warp Stabilizer into a new clip.
//! - **Speed change** (`clip.speedJob`): a new clip at another speed, optionally reversed, the
//!   audio pitch compensated with the Pitch Shifter.
//! - **Transcode** (`clip.transcode`): the clip to an edit-friendly format with a transcode
//!   profile; the result is added to the project or replaces the original.
//! - **Transcode profiles** (`transcode.profiles.{list,save,delete}`): a name, an export preset and
//!   a scale; the user's persist in `<data dir>/transcode-profiles.json`.
//! - **Custom clip jobs** (`clipJobs.{list,save,delete,run}`): an external program with an
//!   argument list in which `{input}` and `{output}` stand for the clip's file and the new file;
//!   no shell is involved. Persisted in `<data dir>/clip-jobs.json`.
//! - **Clip in Timeline** (`clip.instances`, `clip.goToInstance`), **Reveal in file manager**
//!   (`clip.reveal`), **Replace only audio or video** (`clip.replacePart`), **Adjust profile to
//!   clip** (`sequence.matchClip`) and **Insert at timecode** (`clip.insertAtTimecode`).
//!
//! A render job copies the current project, adds a sequence holding just the clip (with its
//! effect or speed) and exports it on a worker thread; [`poll`] imports the finished files.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use redrob_film_export::ExportSettings;
use redrob_film_project::{ItemId, MediaRef, Project, TrackKind};
use redrob_film_time::{Tick, TimeRange};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, has_seq, str_p, u64_p};
use crate::{EngineError, Event, Result, Session};

/// A clip job whose outputs still have to be added to the project.
pub struct PendingClipJob {
    pub job: u64,
    /// (source item, output file) for each finished output.
    pub outputs: Arc<Mutex<Vec<(ItemId, String)>>>,
    pub add_to_project: bool,
    /// Point the source items at their outputs instead (transcode ▸ replace).
    pub replace: bool,
}

/// Import (or swap in) the outputs of finished clip jobs.
pub fn poll(s: &mut Session) {
    let finished: Vec<usize> = s
        .clip_jobs
        .iter()
        .enumerate()
        .filter(|(_, pj)| s.jobs.iter().find(|j| j.id == pj.job).is_none_or(|j| j.progress.finished.load(Ordering::Relaxed)))
        .map(|(i, _)| i)
        .collect();
    for i in finished.into_iter().rev() {
        let pj = s.clip_jobs.remove(i);
        let outs = std::mem::take(&mut *pj.outputs.lock().unwrap_or_else(|e| e.into_inner()));
        for (item, path) in outs {
            let r = if pj.replace {
                s.execute("media.replaceFootage", json!({"item": item.0, "path": path}))
            } else if pj.add_to_project {
                let bin = s.project.root.parent_of(item).filter(|b| *b != s.project.root.id).map(|b| b.0);
                s.execute("file.import", json!({"paths": [path], "bin": bin}))
            } else {
                Ok(Value::Null)
            };
            if let Err(e) = r {
                s.error_toast("clipJob", format!("{path}: {e}"));
            }
        }
    }
}

/// The file a media item reads (None for generators and other kinds).
fn file_of(p: &Project, item: ItemId) -> Option<(redrob_film_project::MediaClip, String, String)> {
    let it = p.item(item)?;
    let m = it.as_media()?;
    match &m.media {
        MediaRef::File { path } => Some((m.clone(), path.clone(), it.name.clone())),
        MediaRef::Generator(_) => None,
    }
}

fn items_p(s: &Session, p: &Value) -> Vec<ItemId> {
    match p.get("items").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_u64).map(ItemId).collect(),
        None => u64_p(p, "item").map(|i| vec![ItemId(i)]).unwrap_or_else(|| s.state.project_selection.clone()),
    }
}

/// `<dir>/<stem><suffix>.<ext>`, numbered when taken.
fn out_path(dir: &Path, stem: &str, suffix: &str, ext: &str, taken: &[String]) -> String {
    crate::proxies::unique_path(dir, stem, suffix, ext, taken).to_string_lossy().into_owned()
}

/// A copy of `project` with a new sequence holding `item`'s whole media on one video and one
/// audio track (with `fx` applied to the track items). Returns the copy and the sequence.
fn clip_project(project: &Project, item: ItemId, fx: &dyn Fn(&mut redrob_film_project::TrackItem, TrackKind)) -> Option<(Project, ItemId, Tick)> {
    let (m, _, _) = file_of(project, item)?;
    let rate = m.frame_rate();
    let settings = crate::commands::default_seq_settings_for(&m);
    let mut p = project.clone();
    let has_v = m.info.video.is_some();
    let has_a = m.info.has_audio();
    let seq = p.new_sequence("clip job", settings, usize::from(has_v), usize::from(has_a), None);
    let range = TimeRange { start: Tick::ZERO, duration: m.info.duration };
    let mut end = Tick::ZERO;
    for kind in [TrackKind::Video, TrackKind::Audio] {
        if (kind == TrackKind::Video && !has_v) || (kind == TrackKind::Audio && !has_a) {
            continue;
        }
        let Some(mut ti) = p.make_track_item(item, kind, Tick::ZERO, range, rate) else { continue };
        fx(&mut ti, kind);
        end = end.max(ti.end());
        let q = p.sequence_mut(seq)?;
        let t = if kind == TrackKind::Video { q.video_tracks.first_mut()? } else { q.audio_tracks.first_mut()? };
        t.items.push(ti);
    }
    Some((p, seq, end))
}

/// Export settings that keep the clip's quality for a new clip: ProRes 422 HQ with 24-bit PCM, or
/// 24-bit WAV for sound only.
fn intermediate(has_video: bool) -> ExportSettings {
    let name = if has_video { "Apple ProRes 422 HQ" } else { "Waveform Audio 48 kHz 24-bit" };
    let mut st = redrob_film_export::presets::find_builtin(name).map(|p| p.settings).unwrap_or_default();
    st.audio.sample_rate = None;
    st
}

/// Start a background job exporting `tasks` (source item, project, sequence, settings).
fn start_render(s: &mut Session, label: String, tasks: Vec<(ItemId, Arc<Project>, ItemId, ExportSettings)>, add: bool, replace: bool, wait: bool) -> Result<u64> {
    if tasks.is_empty() {
        return Err(EngineError::Other("nothing to render".into()));
    }
    let id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let job = crate::Job { id, label, progress: Default::default(), result: Default::default() };
    let outputs: Arc<Mutex<Vec<(ItemId, String)>>> = Arc::default();
    let (prog, res, outs) = (job.progress.clone(), job.result.clone(), outputs.clone());
    let pool = s.media.clone();
    let services = s.services.clone();
    let total: u64 = tasks
        .iter()
        .map(|(_, p, q, st)| redrob_film_export::export_range(p, *q, st).map_or(1, |r| st.frame_rate.unwrap_or(p.sequence(*q).map_or_else(Default::default, |x| x.settings.frame_rate)).frame_at(r.duration).max(1) as u64))
        .sum();
    prog.total.store(total, Ordering::Relaxed);
    let run = move || {
        let t0 = web_time::Instant::now();
        let (mut bytes, mut err) = (0u64, None);
        let n = tasks.len();
        let mut last = String::new();
        for (k, (item, project, seq, mut st)) in tasks.into_iter().enumerate() {
            if prog.cancel.load(Ordering::Relaxed) {
                err = Some("cancelled".to_string());
                break;
            }
            *prog.status.lock().unwrap_or_else(|e| e.into_inner()) = format!("{} ({} of {n})", Path::new(&st.path).file_name().map(|f| f.to_string_lossy()).unwrap_or_default(), k + 1);
            if let Some(d) = Path::new(&st.path).parent() {
                let _ = std::fs::create_dir_all(d);
            }
            st.part_of_batch = true;
            let provider = pool.full_res_provider(project.clone(), services.clone());
            match redrob_film_export::export(&project, seq, &st, &provider, &prog) {
                Ok(r) => {
                    bytes += r.bytes;
                    last = r.path.clone();
                    outs.lock().unwrap_or_else(|e| e.into_inner()).push((item, r.path));
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&st.path);
                    err = Some(format!("{}: {e}", st.path));
                    break;
                }
            }
        }
        let secs = t0.elapsed().as_secs_f64();
        let frames = prog.done.load(Ordering::Relaxed);
        let r = match err {
            Some(e) => {
                *prog.error.lock().unwrap_or_else(|x| x.into_inner()) = Some(e.clone());
                Err(e)
            }
            None => Ok(redrob_film_export::Report { path: last, frames, seconds: secs, bytes, render_fps: frames as f64 / secs.max(1e-6), extra_files: Vec::new() }),
        };
        *prog.status.lock().unwrap_or_else(|e| e.into_inner()) = match &r {
            Ok(_) => format!("Done in {secs:.1}s"),
            Err(e) => e.clone(),
        };
        prog.finished.store(true, Ordering::Relaxed);
        *res.lock().unwrap_or_else(|x| x.into_inner()) = Some(r);
    };
    let run = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), run);
    s.jobs.push(job);
    s.clip_jobs.push(PendingClipJob { job: id, outputs, add_to_project: add, replace });
    if wait || cfg!(target_arch = "wasm32") {
        run();
        poll(s);
    } else {
        std::thread::Builder::new().name("redrob-film-clip-job".into()).spawn(run).map_err(|e| EngineError::Other(e.to_string()))?;
    }
    Ok(id)
}

fn dest_dir(p: &Value, source: &str) -> PathBuf {
    match str_p(p, "destination").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => Path::new(source).parent().unwrap_or(Path::new(".")).to_path_buf(),
    }
}

fn stem(path: &str) -> String {
    Path::new(path).file_stem().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "clip".into())
}

// ---------------------------------------------------------------------------------------------
// stabilize / speed
// ---------------------------------------------------------------------------------------------

fn stabilize(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clip.stabilizeJob";
    let items = items_p(s, p);
    let smooth = f64_p(p, "smoothness").unwrap_or(50.0);
    let mut tasks = Vec::new();
    let mut taken = Vec::new();
    for item in items {
        let Some((m, path, name)) = file_of(&s.project, item) else { continue };
        if m.info.video.is_none() {
            return Err(bad(cmd, format!("{name} has no picture to stabilize")));
        }
        let fx = |ti: &mut redrob_film_project::TrackItem, kind: TrackKind| {
            if kind == TrackKind::Video
                && let Some(def) = redrob_film_project::find_effect("warp_stabilizer")
            {
                let mut e = def.instance();
                if let Some(pp) = e.param_mut("smoothness") {
                    pp.value = redrob_film_project::ParamValue::Float(smooth);
                }
                ti.effects.push(e);
            }
        };
        let (proj, seq, _) = clip_project(&s.project, item, &fx).ok_or_else(|| bad(cmd, "not a media clip"))?;
        let mut st = intermediate(true);
        st.path = out_path(&dest_dir(p, &path), &stem(&path), "_stabilized", st.extension(), &taken);
        taken.push(st.path.clone());
        tasks.push((item, Arc::new(proj), seq, st));
    }
    if tasks.is_empty() {
        return Err(bad(cmd, "select a video clip in the Project panel"));
    }
    let outs: Vec<String> = tasks.iter().map(|t| t.3.path.clone()).collect();
    let job = start_render(s, format!("Stabilize ({} clip(s))", tasks.len()), tasks, true, false, bool_p(p, "wait").unwrap_or(false))?;
    Ok(json!({"job": job, "outputs": outs}))
}

fn speed_job(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clip.speedJob";
    let speed = f64_p(p, "speed").unwrap_or(100.0) / 100.0;
    if !(speed.is_finite() && speed > 0.0 && speed <= 100.0) {
        return Err(bad(cmd, "speed: a percentage above 0"));
    }
    let reverse = bool_p(p, "reverse").unwrap_or(false);
    let pitch = bool_p(p, "pitchCompensation").unwrap_or(true);
    let mut tasks = Vec::new();
    let mut taken = Vec::new();
    for item in items_p(s, p) {
        let Some((m, path, _)) = file_of(&s.project, item) else { continue };
        let fx = |ti: &mut redrob_film_project::TrackItem, kind: TrackKind| {
            let rate = m.frame_rate();
            ti.speed = speed;
            ti.reverse = reverse;
            ti.duration = rate.snap(Tick((ti.duration.0 as f64 / speed).round() as i64)).max(rate.frame_duration());
            // pitch compensation: shift back by the octaves the speed moved it (within ±12 semitones)
            if kind == TrackKind::Audio
                && pitch
                && (speed - 1.0).abs() > 1e-6
                && let Some(def) = redrob_film_project::find_effect("pitch_shifter")
            {
                let mut e = def.instance();
                let semis = (-12.0 * speed.log2()).clamp(-12.0, 12.0);
                if let Some(pp) = e.param_mut("semitones") {
                    pp.value = redrob_film_project::ParamValue::Float(semis.trunc());
                }
                if let Some(pp) = e.param_mut("cents") {
                    pp.value = redrob_film_project::ParamValue::Float(((semis - semis.trunc()) * 100.0).round());
                }
                ti.effects.push(e);
            }
        };
        let (proj, seq, _) = clip_project(&s.project, item, &fx).ok_or_else(|| bad(cmd, "not a media clip"))?;
        let mut st = intermediate(m.info.video.is_some());
        let suffix = format!("_{}{}", (speed * 100.0).round(), if reverse { "_reverse" } else { "" });
        st.path = out_path(&dest_dir(p, &path), &stem(&path), &suffix, st.extension(), &taken);
        taken.push(st.path.clone());
        tasks.push((item, Arc::new(proj), seq, st));
    }
    if tasks.is_empty() {
        return Err(bad(cmd, "select a media clip in the Project panel"));
    }
    let outs: Vec<String> = tasks.iter().map(|t| t.3.path.clone()).collect();
    let job = start_render(s, format!("Speed change ({} clip(s))", tasks.len()), tasks, true, false, bool_p(p, "wait").unwrap_or(false))?;
    Ok(json!({"job": job, "outputs": outs}))
}

// ---------------------------------------------------------------------------------------------
// transcode + profiles
// ---------------------------------------------------------------------------------------------

/// A transcode profile: an export preset at a scale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscodeProfile {
    pub name: String,
    /// Export preset name (built-in or the user's).
    pub preset: String,
    /// Frame size relative to the clip (1 = same size).
    #[serde(default = "one")]
    pub scale: f32,
    #[serde(skip)]
    pub builtin: bool,
}

fn one() -> f32 {
    1.0
}

pub fn builtin_profiles() -> Vec<TranscodeProfile> {
    [
        ("ProRes 422 LT (edit friendly)", "Apple ProRes 422 LT"),
        ("ProRes 422 HQ", "Apple ProRes 422 HQ"),
        ("DNxHR SQ (edit friendly)", "Avid DNxHR SQ"),
        ("DNxHR HQ", "Avid DNxHR HQ"),
        ("APV 422-10", "APV 422-10"),
        ("WAV 24-bit (audio)", "Waveform Audio 48 kHz 24-bit"),
    ]
    .into_iter()
    .map(|(n, pr)| TranscodeProfile { name: n.into(), preset: pr.into(), scale: 1.0, builtin: true })
    .collect()
}

fn data_dir(s: &Session) -> Option<PathBuf> {
    s.export_presets.dir().map(Path::to_path_buf).or_else(|| s.prefs_path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf))
}

fn read_json<T: for<'de> Deserialize<'de>>(s: &Session, file: &str) -> Vec<T> {
    data_dir(s).and_then(|d| std::fs::read(d.join(file)).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_json<T: Serialize>(s: &Session, file: &str, v: &[T]) -> Result<()> {
    let Some(d) = data_dir(s) else { return Ok(()) };
    std::fs::create_dir_all(&d).map_err(|e| EngineError::Other(format!("{}: {e}", d.display())))?;
    let bytes = serde_json::to_vec_pretty(v).map_err(|e| EngineError::Other(e.to_string()))?;
    redrob_film_format::atomic_write(&d.join(file), &bytes).map_err(|e| EngineError::Other(e.to_string()))
}

const PROFILES_FILE: &str = "transcode-profiles.json";
const JOBS_FILE: &str = "clip-jobs.json";

/// Built-in profiles and the user's (a user profile shadows a built-in of the same name).
pub fn profiles(s: &Session) -> Vec<TranscodeProfile> {
    let user: Vec<TranscodeProfile> = read_json(s, PROFILES_FILE);
    let mut v: Vec<TranscodeProfile> = builtin_profiles().into_iter().filter(|b| !user.iter().any(|u| u.name == b.name)).collect();
    v.extend(user);
    v
}

fn profiles_list(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(Value::Array(profiles(s).iter().map(|p| json!({"name": p.name, "preset": p.preset, "scale": p.scale, "builtin": p.builtin})).collect()))
}

fn profiles_save(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "transcode.profiles.save";
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "need `name`"))?.to_string();
    let preset = str_p(p, "preset").ok_or_else(|| bad(cmd, "need `preset` (an export preset name)"))?.to_string();
    if s.export_presets.find(&preset).is_none() {
        return Err(bad(cmd, format!("no export preset named `{preset}`")));
    }
    let scale = f64_p(p, "scale").unwrap_or(1.0) as f32;
    if !(scale.is_finite() && scale > 0.0 && scale <= 4.0) {
        return Err(bad(cmd, "scale: 0 to 4"));
    }
    let mut user: Vec<TranscodeProfile> = read_json(s, PROFILES_FILE);
    user.retain(|u| u.name != name);
    user.push(TranscodeProfile { name: name.clone(), preset, scale, builtin: false });
    write_json(s, PROFILES_FILE, &user)?;
    Ok(json!({"saved": name}))
}

fn profiles_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "name").ok_or_else(|| bad("transcode.profiles.delete", "need `name`"))?.to_string();
    let mut user: Vec<TranscodeProfile> = read_json(s, PROFILES_FILE);
    let n = user.len();
    user.retain(|u| u.name != name);
    if user.len() == n {
        return Err(bad("transcode.profiles.delete", format!("no user profile named `{name}` (built-in profiles cannot be deleted)")));
    }
    write_json(s, PROFILES_FILE, &user)?;
    Ok(json!({"deleted": name}))
}

fn transcode(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clip.transcode";
    let pname = str_p(p, "profile").unwrap_or("ProRes 422 LT (edit friendly)");
    let profile = profiles(s).into_iter().find(|x| x.name == pname).ok_or_else(|| bad(cmd, format!("no transcode profile named `{pname}` (see transcode.profiles.list)")))?;
    let preset = s.export_presets.find(&profile.preset).ok_or_else(|| bad(cmd, format!("the profile's export preset `{}` is gone", profile.preset)))?;
    let replace = bool_p(p, "replace").unwrap_or(false);
    let add = bool_p(p, "addToProject").unwrap_or(true);
    let mut tasks = Vec::new();
    let mut taken = Vec::new();
    for item in items_p(s, p) {
        let Some((m, path, _)) = file_of(&s.project, item) else { continue };
        let (proj, seq, _) = clip_project(&s.project, item, &|_: &mut redrob_film_project::TrackItem, _: TrackKind| {}).ok_or_else(|| bad(cmd, "not a media clip"))?;
        let mut st = if m.info.video.is_some() && !preset.settings.format.is_audio_only() { preset.settings.clone() } else { intermediate(false) };
        st.frame_size = None;
        st.frame_rate = None;
        st.range = None;
        st.scale = profile.scale;
        st.path = out_path(&dest_dir(p, &path), &stem(&path), "_transcoded", st.extension(), &taken);
        taken.push(st.path.clone());
        tasks.push((item, Arc::new(proj), seq, st));
    }
    if tasks.is_empty() {
        return Err(bad(cmd, "select media clips in the Project panel"));
    }
    let outs: Vec<String> = tasks.iter().map(|t| t.3.path.clone()).collect();
    let job = start_render(s, format!("Transcode ({} clip(s), {})", tasks.len(), profile.name), tasks, add, replace, bool_p(p, "wait").unwrap_or(false))?;
    Ok(json!({"job": job, "outputs": outs, "profile": profile.name}))
}

// ---------------------------------------------------------------------------------------------
// custom clip jobs
// ---------------------------------------------------------------------------------------------

/// A custom clip job: an external program run on the clip's file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomJob {
    pub name: String,
    pub program: String,
    /// Arguments; `{input}` and `{output}` are replaced by the paths.
    pub args: Vec<String>,
    /// Extension of the output file.
    pub extension: String,
    /// Suffix added to the clip's file name for the output.
    #[serde(default)]
    pub suffix: String,
    #[serde(default = "yes")]
    pub add_to_project: bool,
}

fn yes() -> bool {
    true
}

fn jobs_list(s: &mut Session, _: &Value) -> Result<Value> {
    let v: Vec<CustomJob> = read_json(s, JOBS_FILE);
    Ok(serde_json::to_value(v).unwrap_or_default())
}

fn jobs_save(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clipJobs.save";
    let mut job: CustomJob = serde_json::from_value(p.clone()).map_err(|e| bad(cmd, format!("{{name, program, args:[str], extension, suffix?, addToProject?}}: {e}")))?;
    job.name = job.name.trim().to_string();
    job.extension = job.extension.trim().trim_start_matches('.').to_string();
    if job.name.is_empty() || job.program.trim().is_empty() || job.extension.is_empty() {
        return Err(bad(cmd, "need `name`, `program` and `extension`"));
    }
    if !job.args.iter().any(|a| a.contains("{input}")) || !job.args.iter().any(|a| a.contains("{output}")) {
        return Err(bad(cmd, "`args` must contain {input} and {output}"));
    }
    let mut v: Vec<CustomJob> = read_json(s, JOBS_FILE);
    v.retain(|j| j.name != job.name);
    let name = job.name.clone();
    v.push(job);
    write_json(s, JOBS_FILE, &v)?;
    Ok(json!({"saved": name}))
}

fn jobs_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "name").ok_or_else(|| bad("clipJobs.delete", "need `name`"))?.to_string();
    let mut v: Vec<CustomJob> = read_json(s, JOBS_FILE);
    let n = v.len();
    v.retain(|j| j.name != name);
    if v.len() == n {
        return Err(bad("clipJobs.delete", format!("no clip job named `{name}`")));
    }
    write_json(s, JOBS_FILE, &v)?;
    Ok(json!({"deleted": name}))
}

fn jobs_run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clipJobs.run";
    let name = str_p(p, "name").ok_or_else(|| bad(cmd, "need `name` (see clipJobs.list)"))?;
    let jobs: Vec<CustomJob> = read_json(s, JOBS_FILE);
    let cj = jobs.into_iter().find(|j| j.name == name).ok_or_else(|| bad(cmd, format!("no clip job named `{name}`")))?;
    if cfg!(target_arch = "wasm32") {
        return Err(EngineError::Other("custom clip jobs run external programs: desktop only".into()));
    }
    let mut work = Vec::new();
    let mut taken = Vec::new();
    for item in items_p(s, p) {
        let Some((_, path, _)) = file_of(&s.project, item) else { continue };
        let suffix = if cj.suffix.is_empty() { format!("_{}", crate::render_jobs::safe_name(&cj.name).replace(' ', "_")) } else { cj.suffix.clone() };
        let out = out_path(&dest_dir(p, &path), &stem(&path), &suffix, &cj.extension, &taken);
        taken.push(out.clone());
        work.push((item, path, out));
    }
    if work.is_empty() {
        return Err(bad(cmd, "select media clips in the Project panel"));
    }
    let id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let job = crate::Job { id, label: format!("{} ({} clip(s))", cj.name, work.len()), progress: Default::default(), result: Default::default() };
    job.progress.total.store(work.len() as u64, Ordering::Relaxed);
    let outputs: Arc<Mutex<Vec<(ItemId, String)>>> = Arc::default();
    let (prog, res, outs) = (job.progress.clone(), job.result.clone(), outputs.clone());
    let outs_json: Vec<String> = work.iter().map(|w| w.2.clone()).collect();
    let add = cj.add_to_project;
    let run = move || {
        let t0 = web_time::Instant::now();
        let mut err = None;
        let mut last = String::new();
        for (item, input, output) in work {
            if prog.cancel.load(Ordering::Relaxed) {
                err = Some("cancelled".to_string());
                break;
            }
            *prog.status.lock().unwrap_or_else(|e| e.into_inner()) = format!("{} {}", cj.program, Path::new(&input).file_name().map(|f| f.to_string_lossy()).unwrap_or_default());
            let args: Vec<String> = cj.args.iter().map(|a| a.replace("{input}", &input).replace("{output}", &output)).collect();
            match std::process::Command::new(cj.program.trim()).args(&args).stdin(std::process::Stdio::null()).output() {
                Ok(o) if o.status.success() && Path::new(&output).exists() => {
                    last = output.clone();
                    outs.lock().unwrap_or_else(|e| e.into_inner()).push((item, output));
                }
                Ok(o) => {
                    let tail = String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("").to_string();
                    err = Some(format!("{} failed ({}): {tail}", cj.program, o.status));
                    break;
                }
                Err(e) => {
                    err = Some(format!("{}: {e}", cj.program));
                    break;
                }
            }
            prog.done.fetch_add(1, Ordering::Relaxed);
        }
        let secs = t0.elapsed().as_secs_f64();
        let r = match err {
            Some(e) => {
                *prog.error.lock().unwrap_or_else(|x| x.into_inner()) = Some(e.clone());
                Err(e)
            }
            None => Ok(redrob_film_export::Report { path: last, frames: 0, seconds: secs, bytes: 0, render_fps: 0.0, extra_files: Vec::new() }),
        };
        *prog.status.lock().unwrap_or_else(|e| e.into_inner()) = match &r {
            Ok(_) => format!("Done in {secs:.1}s"),
            Err(e) => e.clone(),
        };
        prog.finished.store(true, Ordering::Relaxed);
        *res.lock().unwrap_or_else(|x| x.into_inner()) = Some(r);
    };
    let run = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), run);
    s.jobs.push(job);
    s.clip_jobs.push(PendingClipJob { job: id, outputs, add_to_project: add, replace: false });
    if bool_p(p, "wait").unwrap_or(false) {
        run();
        poll(s);
    } else {
        std::thread::Builder::new().name("redrob-film-custom-job".into()).spawn(run).map_err(|e| EngineError::Other(e.to_string()))?;
    }
    Ok(json!({"job": id, "outputs": outs_json}))
}

// ---------------------------------------------------------------------------------------------
// bin ↔ timeline
// ---------------------------------------------------------------------------------------------

/// Every use of `item` in the project's sequences: (sequence, track kind, track index, clip, start).
pub fn instances(p: &Project, item: ItemId) -> Vec<(ItemId, TrackKind, usize, redrob_film_project::ClipId, Tick)> {
    let mut v = Vec::new();
    for q in p.sequences().filter_map(|i| p.sequence(i.id)) {
        for (kind, tracks) in [(TrackKind::Video, &q.video_tracks), (TrackKind::Audio, &q.audio_tracks)] {
            for (ti, t) in tracks.iter().enumerate() {
                for x in &t.items {
                    if x.item == item {
                        v.push((q.id, kind, ti, x.id, x.start));
                    }
                }
            }
        }
    }
    v.sort_by_key(|x| (x.0.0, x.4.0));
    v
}

fn instances_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let item = u64_p(p, "item").map(ItemId).or_else(|| s.state.project_selection.first().copied()).ok_or_else(|| bad("clip.instances", "need `item`"))?;
    let list: Vec<Value> = instances(&s.project, item)
        .into_iter()
        .map(|(q, k, t, c, start)| {
            let rate = s.project.sequence(q).map(|x| x.settings.frame_rate).unwrap_or_default();
            let name = s.project.item(q).map(|i| i.name.clone()).unwrap_or_default();
            json!({"sequence": q.0, "sequenceName": name, "track": format!("{}{}", if k == TrackKind::Video { "V" } else { "A" }, t + 1), "clip": c.0, "start": start.0, "frame": rate.frame_at(start)})
        })
        .collect();
    Ok(json!({"item": item.0, "instances": list}))
}

fn go_to_instance(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clip.goToInstance";
    let clip = u64_p(p, "clip").map(redrob_film_project::ClipId).ok_or_else(|| bad(cmd, "need `clip` (see clip.instances)"))?;
    let (seq, start) = s
        .project
        .sequences()
        .filter_map(|i| s.project.sequence(i.id))
        .find_map(|q| q.find_item(clip).map(|(_, it)| (q.id, it.start)))
        .ok_or_else(|| bad(cmd, "no such timeline clip"))?;
    s.state.active_sequence = Some(seq);
    if !s.state.open_sequences.contains(&seq) {
        s.state.open_sequences.push(seq);
    }
    s.set_playhead(start);
    s.state.selection = vec![clip];
    s.events.push(Event::OpenSequence(seq));
    Ok(json!({"sequence": seq.0, "time": start.0}))
}

fn reveal(s: &mut Session, p: &Value) -> Result<Value> {
    let item = u64_p(p, "item").map(ItemId).or_else(|| s.state.project_selection.first().copied()).ok_or_else(|| bad("clip.reveal", "need `item`"))?;
    let (_, path, _) = file_of(&s.project, item).ok_or_else(|| bad("clip.reveal", "the item is not a file"))?;
    s.events.push(Event::OpenPath { path: path.clone(), reveal: true });
    Ok(json!({"path": path}))
}

/// Replace only the audio or the video of a clip's timeline uses with another file: the file
/// is imported, and the uses of that kind switch to it (timing and effects kept).
fn replace_part(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clip.replacePart";
    let item = u64_p(p, "item").map(ItemId).or_else(|| s.state.project_selection.first().copied()).ok_or_else(|| bad(cmd, "need `item`"))?;
    let path = str_p(p, "path").ok_or_else(|| bad(cmd, "need `path`"))?.to_string();
    let kind = match str_p(p, "part").unwrap_or("audio") {
        "audio" => TrackKind::Audio,
        "video" => TrackKind::Video,
        x => return Err(bad(cmd, format!("part `{x}`: audio | video"))),
    };
    let bin = s.project.root.parent_of(item).filter(|b| *b != s.project.root.id).map(|b| b.0);
    let v = s.execute("file.import", json!({"paths": [path], "bin": bin}))?;
    let new = v["items"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).map(ItemId).ok_or_else(|| EngineError::Other(format!("{path} could not be imported")))?;
    let ok = s.project.item(new).and_then(|i| i.as_media()).is_some_and(|m| if kind == TrackKind::Audio { m.info.has_audio() } else { m.info.video.is_some() });
    if !ok {
        return Err(bad(cmd, format!("{path} has no {}", if kind == TrackKind::Audio { "audio" } else { "video" })));
    }
    let n = s.edit(if kind == TrackKind::Audio { "Replace Audio" } else { "Replace Video" }, |pr, _| {
        let mut n = 0;
        let ids: Vec<ItemId> = pr.sequences().map(|i| i.id).collect();
        for q in ids {
            let Some(q) = pr.sequence_mut(q) else { continue };
            let tracks = if kind == TrackKind::Audio { &mut q.audio_tracks } else { &mut q.video_tracks };
            for t in tracks.iter_mut() {
                for x in t.items.iter_mut().filter(|x| x.item == item) {
                    x.item = new;
                    x.audio_stream = 0;
                    n += 1;
                }
            }
        }
        Ok(n)
    })?;
    Ok(json!({"item": new.0, "replaced": n}))
}

/// Adjust the active sequence's frame size, rate and pixel aspect to a clip's.
fn match_clip(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sequence.matchClip";
    let item = u64_p(p, "item").map(ItemId).or_else(|| s.state.project_selection.first().copied()).ok_or_else(|| bad(cmd, "need `item`"))?;
    let m = s.project.item(item).and_then(|i| i.as_media()).cloned().ok_or_else(|| bad(cmd, "not a media clip"))?;
    let v = m.info.video.as_ref().ok_or_else(|| bad(cmd, "the clip has no picture"))?;
    let (w, h, rate, par) = (v.width, v.height, m.frame_rate(), m.pixel_aspect());
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    s.edit("Adjust Profile to Clip", |pr, _| {
        let q = pr.sequence_mut(seq).ok_or(EngineError::NoSequence)?;
        q.settings.width = w;
        q.settings.height = h;
        q.settings.frame_rate = rate;
        q.settings.par = par;
        q.settings.validate().map_err(|e| bad(cmd, e))?;
        Ok(())
    })?;
    Ok(json!({"width": w, "height": h, "fps": rate.as_f64()}))
}

/// Insert a clip at the sequence time its own start timecode names.
fn insert_at_timecode(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clip.insertAtTimecode";
    let item = u64_p(p, "item").map(ItemId).or_else(|| s.state.project_selection.first().copied()).ok_or_else(|| bad(cmd, "need `item`"))?;
    let m = s.project.item(item).and_then(|i| i.as_media()).cloned().ok_or_else(|| bad(cmd, "not a media clip"))?;
    let tc = m.info.start_timecode.ok_or_else(|| bad(cmd, "the clip carries no timecode"))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let seq_rate = q.settings.frame_rate;
    let at = m.frame_rate().tick_of(tc) - seq_rate.tick_of(q.start_timecode);
    if at < Tick::ZERO {
        return Err(bad(cmd, "the clip's timecode is before the sequence's start timecode"));
    }
    let at = seq_rate.snap(at);
    let range = TimeRange { start: Tick::ZERO, duration: m.info.duration };
    let tg = s.targeting();
    let insert = bool_p(p, "insert").unwrap_or(false);
    let ids = crate::commands::place_item(s, item, range, at, tg.video_dest, tg.audio_dest, insert, "Insert at Timecode", None)?;
    Ok(json!({"time": at.0, "clips": ids.iter().map(|c| c.0).collect::<Vec<_>>()}))
}

fn has_items(s: &Session) -> std::result::Result<(), String> {
    if s.state.project_selection.is_empty() { Err("select clips in the Project panel".into()) } else { Ok(()) }
}

type Run = fn(&mut Session, &Value) -> Result<Value>;

fn spec(id: &'static str, label: &'static str, params: &'static str, enabled: fn(&Session) -> std::result::Result<(), String>, run: Run, journal: bool) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled, run, journal }
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        spec("clip.stabilizeJob", "Stabilize Clip", r#"{"items":[id]?,"smoothness":percent=50,"destination":dir?,"wait":bool?}"#, always, stabilize, true),
        spec(
            "clip.speedJob",
            "Duplicate Clip with Speed Change",
            r#"{"items":[id]?,"speed":percent=100,"reverse":bool=false,"pitchCompensation":bool=true,"destination":dir?,"wait":bool?}"#,
            always,
            speed_job,
            true,
        ),
        spec(
            "clip.transcode",
            "Transcode Clips",
            r#"{"items":[id]?,"profile":str?,"destination":dir?,"addToProject":bool=true,"replace":bool=false,"wait":bool?}"#,
            always,
            transcode,
            true,
        ),
        spec("transcode.profiles.list", "List Transcode Profiles", "{}", always, profiles_list, false),
        spec("transcode.profiles.save", "Save Transcode Profile", r#"{"name":str,"preset":exportPresetName,"scale":f32=1}"#, always, profiles_save, true),
        spec("transcode.profiles.delete", "Delete Transcode Profile", r#"{"name":str}"#, always, profiles_delete, true),
        spec("clipJobs.list", "List Clip Jobs", "{}", always, jobs_list, false),
        spec(
            "clipJobs.save",
            "Save Clip Job",
            r#"{"name":str,"program":str,"args":[str with {input} {output}],"extension":str,"suffix":str?,"addToProject":bool=true}"#,
            always,
            jobs_save,
            true,
        ),
        spec("clipJobs.delete", "Delete Clip Job", r#"{"name":str}"#, always, jobs_delete, true),
        spec("clipJobs.run", "Run Clip Job", r#"{"name":str,"items":[id]?,"destination":dir?,"wait":bool?}"#, always, jobs_run, true),
        spec("clip.instances", "Clip in Timeline", r#"{"item":id?}"#, always, instances_cmd, false),
        spec("clip.goToInstance", "Go to Clip in Timeline", r#"{"clip":clipId}"#, always, go_to_instance, false),
        spec("clip.reveal", "Reveal in File Manager", r#"{"item":id?}"#, has_items, reveal, false),
        spec("clip.replacePart", "Replace Audio or Video Part", r#"{"item":id?,"path":str,"part":"audio|video"}"#, always, replace_part, true),
        spec("sequence.matchClip", "Adjust Profile to Current Clip", r#"{"item":id?}"#, has_seq, match_clip, true),
        spec("clip.insertAtTimecode", "Insert at Timecode", r#"{"item":id?,"insert":bool=false}"#, has_seq, insert_at_timecode, true),
    ]
}
