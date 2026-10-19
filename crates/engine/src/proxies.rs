//! Proxies and ingest (Clip ▸ Proxy ▸ Create / Attach / Reconnect Full Resolution, Toggle Proxies,
//! Project Settings ▸ Ingest Settings).
//!
//! - **Create.** A background job transcodes each clip with our own encoders (ProRes 422 Proxy /
//!   LT or H.264 at ¼ or ½ size, audio included) and attaches the results when it finishes
//!   ([`poll`] applies finished jobs; the frontend calls it every frame through
//!   `Session::poll_persistence`).
//! - **Attach.** A proxy must have the original's duration (±1 frame) and frame rate; its frame
//!   size and aspect may differ.
//! - **Toggle.** `media.toggleProxies` flips Preferences ▸ Media ▸ Enable proxies; the media pool
//!   then hands proxies to monitors and playback. Export always uses full resolution.
//! - **Ingest.** With ingest enabled, `file.import` copies (verified by fingerprint), transcodes
//!   and/or creates proxies.
//! - **Proxy Clips preferences** (Preferences ▸ Media, Kdenlive's Proxy Clips page): the
//!   encoding profile and the profile for clips with alpha ([`create`] picks per clip), the proxy
//!   width, custom profiles (`custom:<name>`, [`parse_profile`], only [`PROXY_PARAMS`] allowed),
//!   still-image proxies (PNG at the proxy image width) and, on import, automatic proxies above a
//!   width plus camera proxy files ([`auto_proxies`], [`camera_proxy`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use redrob_film_export::{ExportSettings, Format, Progress, Report};
use redrob_film_media::{MediaKind, SharedSource};
use redrob_film_project::{IngestAction, ItemId, ItemKind, MediaClip, MediaRef, Project, SequenceSettings, TrackKind};
use redrob_film_time::{Tick, TimeRange};
use serde::Serialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// A proxy / transcode preset.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    #[serde(skip)]
    pub format: Format,
    /// Frame size relative to the original.
    pub scale: f32,
    /// ProRes flavour (`proxy`, `lt`, `hq`) for ProRes presets.
    pub prores: &'static str,
    pub extension: &'static str,
    /// Offered for proxies (else only for transcodes).
    pub proxy: bool,
    /// Keeps the alpha channel (ProRes 4444): the profile for clips with alpha.
    pub alpha: bool,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "prores_proxy_quarter",
        label: "ProRes 422 Proxy, ¼ size",
        format: Format::ProRes,
        scale: 0.25,
        prores: "proxy",
        extension: "mov",
        proxy: true,
        alpha: false,
    },
    Preset {
        id: "prores_proxy_half",
        label: "ProRes 422 Proxy, ½ size",
        format: Format::ProRes,
        scale: 0.5,
        prores: "proxy",
        extension: "mov",
        proxy: true,
        alpha: false,
    },
    Preset {
        id: "prores_lt_half",
        label: "ProRes 422 LT, ½ size",
        format: Format::ProRes,
        scale: 0.5,
        prores: "lt",
        extension: "mov",
        proxy: true,
        alpha: false,
    },
    Preset { id: "h264_quarter", label: "H.264, ¼ size", format: Format::H264, scale: 0.25, prores: "", extension: "mp4", proxy: true, alpha: false },
    Preset { id: "h264_half", label: "H.264, ½ size", format: Format::H264, scale: 0.5, prores: "", extension: "mp4", proxy: true, alpha: false },
    Preset {
        id: "prores_lt",
        label: "ProRes 422 LT (full size)",
        format: Format::ProRes,
        scale: 1.0,
        prores: "lt",
        extension: "mov",
        proxy: false,
        alpha: false,
    },
    Preset {
        id: "prores_hq",
        label: "ProRes 422 HQ (full size)",
        format: Format::ProRes,
        scale: 1.0,
        prores: "hq",
        extension: "mov",
        proxy: false,
        alpha: false,
    },
    Preset { id: "h264", label: "H.264 (full size)", format: Format::H264, scale: 1.0, prores: "", extension: "mp4", proxy: false, alpha: false },
    Preset {
        id: "prores_4444_quarter",
        label: "ProRes 4444 with alpha, ¼ size",
        format: Format::ProRes,
        scale: 0.25,
        prores: "4444",
        extension: "mov",
        proxy: true,
        alpha: true,
    },
    Preset {
        id: "prores_4444_half",
        label: "ProRes 4444 with alpha, ½ size",
        format: Format::ProRes,
        scale: 0.5,
        prores: "4444",
        extension: "mov",
        proxy: true,
        alpha: true,
    },
];

pub const DEFAULT_PROXY_PRESET: &str = "prores_proxy_quarter";
pub const DEFAULT_TRANSCODE_PRESET: &str = "prores_lt";

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// What to do with a finished job's outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDone {
    /// Attach outputs as proxies.
    AttachProxies,
    /// Point the clips at the outputs (ingest transcode).
    ReplaceMedia,
    /// Nothing (the job's own result is the product, e.g. Project Manager).
    Nothing,
}

/// A media job whose outputs still have to be applied to the project.
pub struct PendingJob {
    pub job: u64,
    pub on_done: OnDone,
    /// (item, output path) for each finished output.
    pub outputs: Arc<Mutex<Vec<(ItemId, String)>>>,
    /// Every item the job works on (proxy progress on the bin thumbnails).
    pub items: Vec<ItemId>,
}

/// Transcode `range` (whole media when None) of one media clip to `out` with `preset`, blocking.
/// `progress.total` is the caller's; this adds one per frame to `progress.done`.
pub fn transcode(
    clip: &MediaClip,
    name: &str,
    src: SharedSource,
    range: Option<TimeRange>,
    out: &str,
    preset: &Preset,
    progress: &Progress,
) -> std::result::Result<Report, String> {
    transcode_with(clip, name, src, range, out, &preset.encoding(), progress)
}

/// What a proxy / transcode job writes: a built-in [`Preset`] or a custom profile
/// ([`parse_profile`]), plus the Proxy video resize width.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Encoding {
    /// The preset id or `custom:<name>`.
    pub id: String,
    #[serde(skip)]
    pub format: Format,
    /// Frame size relative to the original (used when `width` is None).
    pub scale: f32,
    /// Target width in pixels (never upscales).
    pub width: Option<u32>,
    pub prores: String,
    pub extension: &'static str,
    pub alpha: bool,
    pub quality: u8,
    /// Video bitrate (H.264); None = from the frame size.
    pub bitrate_kbps: Option<u32>,
    pub audio: bool,
}

impl Preset {
    pub fn encoding(&self) -> Encoding {
        Encoding {
            id: self.id.to_string(),
            format: self.format,
            scale: self.scale,
            width: None,
            prores: self.prores.to_string(),
            extension: self.extension,
            alpha: self.alpha,
            quality: 85,
            bitrate_kbps: None,
            audio: true,
        }
    }
}

impl Encoding {
    /// The scale for a source `src_width` pixels wide.
    pub fn scale_for(&self, src_width: u32) -> f32 {
        match self.width {
            Some(w) if w > 0 && src_width > 0 => (w as f32 / src_width as f32).clamp(0.01, 1.0),
            _ => self.scale.clamp(0.01, 1.0),
        }
    }
}

/// The parameters a custom proxy profile may use (`key=value`, separated by spaces): anything
/// else is refused with a warning naming it.
pub const PROXY_PARAMS: &[(&str, &str)] = &[
    ("codec", "prores | h264"),
    ("profile", "ProRes flavour: proxy | lt | standard | hq | 4444"),
    ("width", "proxy width in pixels"),
    ("scale", "proxy size relative to the original, 0.05–1"),
    ("quality", "1–100"),
    ("bitrate", "H.264 video bitrate in kbit/s"),
    ("alpha", "0 | 1: keep the alpha channel (ProRes 4444)"),
    ("audio", "0 | 1: include the audio"),
];

/// Why a custom profile's parameters were refused.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ProfileProblems {
    /// Keys not in [`PROXY_PARAMS`].
    pub unknown: Vec<String>,
    /// Bad values or malformed tokens.
    pub errors: Vec<String>,
}

/// Parse a custom profile's `key=value` parameters into an [`Encoding`].
pub fn parse_profile(name: &str, params: &str) -> std::result::Result<Encoding, ProfileProblems> {
    let mut e = Encoding {
        id: format!("custom:{name}"),
        format: Format::ProRes,
        scale: 0.25,
        width: None,
        prores: "proxy".into(),
        extension: "mov",
        alpha: false,
        quality: 85,
        bitrate_kbps: None,
        audio: true,
    };
    let mut bad = ProfileProblems::default();
    let flag = |v: &str| matches!(v, "1" | "true" | "yes" | "on");
    for tok in params.split_whitespace() {
        let Some((k, v)) = tok.split_once('=') else {
            bad.errors.push(format!("`{tok}` is not key=value"));
            continue;
        };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim();
        match k.as_str() {
            "codec" => match v.to_ascii_lowercase().as_str() {
                "prores" => {
                    e.format = Format::ProRes;
                    e.extension = "mov";
                }
                "h264" | "h.264" | "avc" => {
                    e.format = Format::H264;
                    e.extension = "mp4";
                }
                _ => bad.errors.push(format!("codec `{v}`: use prores or h264")),
            },
            "profile" => match v.to_ascii_lowercase().as_str() {
                p @ ("proxy" | "lt" | "standard" | "hq" | "4444") => e.prores = p.to_string(),
                _ => bad.errors.push(format!("profile `{v}`: use proxy, lt, standard, hq or 4444")),
            },
            "width" => match v.parse::<u32>() {
                Ok(w) if (16..=16384).contains(&w) => e.width = Some(w),
                _ => bad.errors.push(format!("width `{v}`: 16–16384 pixels")),
            },
            "scale" => match v.parse::<f32>() {
                Ok(x) if (0.05..=1.0).contains(&x) => e.scale = x,
                _ => bad.errors.push(format!("scale `{v}`: 0.05–1")),
            },
            "quality" => match v.parse::<u8>() {
                Ok(q) if (1..=100).contains(&q) => e.quality = q,
                _ => bad.errors.push(format!("quality `{v}`: 1–100")),
            },
            "bitrate" => match v.parse::<u32>() {
                Ok(b) if (100..=500_000).contains(&b) => e.bitrate_kbps = Some(b),
                _ => bad.errors.push(format!("bitrate `{v}`: 100–500000 kbit/s")),
            },
            "alpha" => e.alpha = flag(v),
            "audio" => e.audio = flag(v),
            _ => bad.unknown.push(k),
        }
    }
    if e.alpha {
        if e.format != Format::ProRes {
            bad.errors.push("alpha needs codec=prores".into());
        }
        e.prores = "4444".into();
    }
    if bad.unknown.is_empty() && bad.errors.is_empty() { Ok(e) } else { Err(bad) }
}

/// A built-in preset id or `custom:<name>` (a profile saved in Preferences) as an encoding.
pub fn resolve_encoding(s: &Session, id: &str) -> Option<Encoding> {
    if let Some(name) = id.strip_prefix("custom:") {
        let p = s.prefs.media.proxy_profiles.iter().find(|p| p.name == name)?;
        return parse_profile(&p.name, &p.params).ok();
    }
    preset(id).map(Preset::encoding)
}

/// Default alpha proxy profile.
pub const DEFAULT_ALPHA_PROXY_PRESET: &str = "prores_4444_quarter";

/// Transcode with an [`Encoding`] (see [`transcode`]).
pub fn transcode_with(
    clip: &MediaClip,
    name: &str,
    src: SharedSource,
    range: Option<TimeRange>,
    out: &str,
    preset: &Encoding,
    progress: &Progress,
) -> std::result::Result<Report, String> {
    let info = &clip.info;
    let rate = clip.frame_rate();
    let mut st = SequenceSettings::default();
    if let Some(v) = &info.video {
        st.width = v.width;
        st.height = v.height;
        // pixels in = pixels out: the sequence has the clip's pixel aspect, so nothing is stretched
        st.par = clip.pixel_aspect();
    }
    st.frame_rate = rate;
    st.sample_rate = info.audio().map_or(48_000, |a| a.sample_rate.max(8000));
    let mut p = Project::new("transcode");
    let mut c = clip.clone();
    c.offline = false;
    c.proxy = None;
    let item = p.add_item(name, redrob_film_project::Label::Iris, ItemKind::Media(c), None);
    let has_v = info.video.is_some();
    let has_a = info.has_audio();
    let seq = p.new_sequence("transcode", st, usize::from(has_v), usize::from(has_a), None);
    let range = range.unwrap_or(TimeRange { start: Tick::ZERO, duration: info.duration });
    let mut placed = Vec::new();
    if has_v && let Some(ti) = p.make_track_item(item, TrackKind::Video, Tick::ZERO, range, rate) {
        placed.push((TrackKind::Video, ti));
    }
    if has_a && let Some(ti) = p.make_track_item(item, TrackKind::Audio, Tick::ZERO, range, rate) {
        placed.push((TrackKind::Audio, ti));
    }
    let q = p.sequence_mut(seq).ok_or("no sequence")?;
    for (k, ti) in placed {
        let t = if k == TrackKind::Video { &mut q.video_tracks[0] } else { &mut q.audio_tracks[0] };
        t.items.push(ti);
    }
    let format = if has_v { preset.format } else { Format::Wav };
    let scale = preset.scale_for(info.video.as_ref().map_or(0, |v| v.width));
    let settings = ExportSettings {
        format,
        path: out.to_string(),
        scale,
        include_audio: has_a && (preset.audio || !has_v),
        quality: preset.quality,
        bitrate_kbps: preset
            .bitrate_kbps
            .unwrap_or_else(|| ((info.video.as_ref().map_or(1920 * 1080, |v| v.width * v.height) as f64 * (scale * scale) as f64) / 200.0).max(800.0) as u32),
        part_of_batch: true,
        prores_profile: preset.prores.clone(),
        alpha: preset.alpha && has_v,
        ..Default::default()
    };
    let project = Arc::new(p);
    let provider = move |id: ItemId| (id == item).then(|| src.clone());
    redrob_film_export::export(&project, seq, &settings, &provider, progress).map_err(|e| e.to_string())
}

/// Frames a transcode of `clip` will write (for progress totals).
pub fn frame_count(clip: &MediaClip, range: Option<TimeRange>) -> u64 {
    let rate = clip.frame_rate();
    let d = range.map_or(clip.info.duration, |r| r.duration);
    if clip.info.video.is_none() { 1 } else { rate.frame_at(d).max(1) as u64 }
}

/// `<dir>/<stem><suffix>.<ext>`, numbered when the name is taken.
pub fn unique_path(dir: &Path, stem: &str, suffix: &str, ext: &str, taken: &[String]) -> PathBuf {
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{stem}{suffix}.{ext}") } else { format!("{stem}{suffix} {n}.{ext}") };
        let p = dir.join(name);
        if !p.exists() && !taken.contains(&p.to_string_lossy().into_owned()) {
            return p;
        }
        n += 1;
    }
}

fn media_path(p: &Project, item: ItemId) -> Option<(MediaClip, String, String)> {
    let it = p.item(item)?;
    let m = it.as_media()?;
    match &m.media {
        MediaRef::File { path } => Some((m.clone(), path.clone(), it.name.clone())),
        MediaRef::Generator(_) => None,
    }
}

fn items_param(s: &Session, p: &Value) -> Vec<ItemId> {
    match p.get("items").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_u64).map(ItemId).collect(),
        None => u64_p(p, "item").map(|i| vec![ItemId(i)]).unwrap_or_else(|| s.state.project_selection.clone()),
    }
}

/// What one output of a media job is made by.
#[derive(Clone, Debug)]
pub enum JobKind {
    /// Transcode the clip (proxy, ingest transcode).
    Transcode(Encoding),
    /// A still image's proxy: the picture scaled to `width` pixels, as PNG.
    Still { width: u32 },
}

/// Start a background transcode job over `work` (item, output path, preset). Returns the job id.
pub fn start_job(s: &mut Session, label: String, work: Vec<(ItemId, String, &'static Preset)>, on_done: OnDone, wait: bool) -> Result<u64> {
    let work = work.into_iter().map(|(i, o, p)| (i, o, JobKind::Transcode(p.encoding()))).collect();
    start_media_job(s, label, work, on_done, wait)
}

/// [`start_job`] with an encoding or still-image proxy per output.
pub fn start_media_job(s: &mut Session, label: String, work: Vec<(ItemId, String, JobKind)>, on_done: OnDone, wait: bool) -> Result<u64> {
    let mut tasks = Vec::new();
    let mut total = 0;
    let mut items = Vec::new();
    for (item, out, kind) in work {
        let (clip, _, name) = media_path(&s.project, item).ok_or_else(|| bad("media.createProxies", format!("item {} is not file media", item.0)))?;
        let src = s.media.full_res_source(&s.project, item, &*s.services).ok_or_else(|| EngineError::Other(format!("{name}: no source")))?;
        if s.media.offline_status(item).is_some() {
            return Err(EngineError::Other(format!("{name} is offline; link it first")));
        }
        total += match kind {
            JobKind::Still { .. } => 1,
            JobKind::Transcode(_) => frame_count(&clip, None),
        };
        items.push(item);
        tasks.push((item, clip, name, src, out, kind));
    }
    let id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let job = crate::Job { id, label, progress: Default::default(), result: Default::default() };
    job.progress.total.store(total, std::sync::atomic::Ordering::Relaxed);
    s.job_items.insert(id, tasks.iter().map(|t| t.0).collect());
    let outputs: Arc<Mutex<Vec<(ItemId, String)>>> = Arc::default();
    let (prog, res, outs) = (job.progress.clone(), job.result.clone(), outputs.clone());
    // Settings ▸ Media ▸ Proxy and transcode jobs at once: clips transcoded side by side
    let threads = if cfg!(target_arch = "wasm32") { 1 } else { s.prefs.media.proxy_job_threads.clamp(1, 16) as usize };
    let project = s.project.clone();
    let run = move || {
        let t0 = web_time::Instant::now();
        let n = tasks.len();
        let queue = Mutex::new(tasks.into_iter().enumerate().collect::<std::collections::VecDeque<_>>());
        let failed: Mutex<Option<String>> = Mutex::new(None);
        let bytes = std::sync::atomic::AtomicU64::new(0);
        let worker = || {
            loop {
                if prog.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    failed.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(|| "cancelled".to_string());
                    break;
                }
                if failed.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
                    break;
                }
                let Some((k, (item, clip, name, src, out, kind))) = queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front() else { break };
                *prog.status.lock().unwrap_or_else(|e| e.into_inner()) = format!("{name} ({} of {n})", k + 1);
                if let Some(d) = Path::new(&out).parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let r = match &kind {
                    JobKind::Transcode(enc) => transcode_with(&clip, &name, src, None, &out, enc, &prog).map(|r| r.bytes),
                    JobKind::Still { width } => {
                        let r = still_proxy(&project, item, &clip, src, &out, *width);
                        prog.done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        r
                    }
                };
                match r {
                    Ok(b) => {
                        bytes.fetch_add(b, std::sync::atomic::Ordering::Relaxed);
                        outs.lock().unwrap_or_else(|e| e.into_inner()).push((item, out));
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&out);
                        failed.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(|| format!("{name}: {e}"));
                        break;
                    }
                }
            }
        };
        if threads <= 1 || n <= 1 {
            worker();
        } else {
            std::thread::scope(|sc| {
                for _ in 0..threads.min(n) {
                    sc.spawn(worker);
                }
            });
        }
        let bytes = bytes.load(std::sync::atomic::Ordering::Relaxed);
        let err = failed.into_inner().unwrap_or_else(|e| e.into_inner());
        let secs = t0.elapsed().as_secs_f64();
        let frames = prog.done.load(std::sync::atomic::Ordering::Relaxed);
        let r = match err {
            Some(e) => {
                *prog.error.lock().unwrap_or_else(|x| x.into_inner()) = Some(e.clone());
                Err(e)
            }
            None => Ok(Report { path: String::new(), frames, seconds: secs, bytes, render_fps: frames as f64 / secs.max(1e-6), extra_files: Vec::new() }),
        };
        *prog.status.lock().unwrap_or_else(|e| e.into_inner()) = match &r {
            Ok(_) => format!("Done in {secs:.1}s"),
            Err(e) => e.clone(),
        };
        prog.finished.store(true, std::sync::atomic::Ordering::Relaxed);
        *res.lock().unwrap_or_else(|x| x.into_inner()) = Some(r);
    };
    let run = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), run);
    s.jobs.push(job);
    s.media_jobs.push(PendingJob { job: id, on_done, outputs, items });
    if wait || cfg!(target_arch = "wasm32") {
        run();
        poll(s);
    } else {
        std::thread::Builder::new().name("filmcraft-proxies".into()).spawn(run).map_err(|e| EngineError::Other(e.to_string()))?;
    }
    Ok(id)
}

/// Write a still image's proxy: its picture `width` pixels wide (aspect kept) as a PNG.
/// Returns the bytes written.
fn still_proxy(project: &Project, item: ItemId, clip: &MediaClip, src: SharedSource, out: &str, width: u32) -> std::result::Result<u64, String> {
    let full_w = clip.info.video.as_ref().map_or(0, |v| v.width).max(1);
    let scale = (width.max(16) as f32 / full_w as f32).min(1.0);
    let provider = redrob_film_render::SourceMap(std::iter::once((item, src)).collect());
    let img = redrob_film_render::render_item(project, item, Tick::ZERO, scale, &provider).ok_or("the image could not be decoded")?;
    let png = redrob_film_export::encode_png(img.to_rgba8(), img.w as u32, img.h as u32).map_err(|e| e.to_string())?;
    std::fs::write(out, &png).map_err(|e| e.to_string())?;
    Ok(png.len() as u64)
}

/// Progress (0..1) of the running proxy job that will attach a proxy to `item`.
pub fn proxy_progress(s: &Session, item: ItemId) -> Option<f32> {
    use std::sync::atomic::Ordering;
    let pj = s.media_jobs.iter().find(|pj| pj.on_done == OnDone::AttachProxies && pj.items.contains(&item))?;
    let job = s.jobs.iter().find(|j| j.id == pj.job)?;
    if job.progress.finished.load(Ordering::Relaxed) {
        return None;
    }
    let total = job.progress.total.load(Ordering::Relaxed).max(1) as f32;
    Some((job.progress.done.load(Ordering::Relaxed) as f32 / total).clamp(0.0, 1.0))
}

/// Apply the outputs of finished media jobs (attach proxies, switch to transcodes).
pub fn poll(s: &mut Session) {
    use std::sync::atomic::Ordering;
    let finished: Vec<usize> = s
        .media_jobs
        .iter()
        .enumerate()
        .filter(|(_, pj)| s.jobs.iter().find(|j| j.id == pj.job).is_none_or(|j| j.progress.finished.load(Ordering::Relaxed)))
        .map(|(i, _)| i)
        .collect();
    for i in finished.into_iter().rev() {
        let pj = s.media_jobs.remove(i);
        let outs = std::mem::take(&mut *pj.outputs.lock().unwrap_or_else(|e| e.into_inner()));
        if outs.is_empty() {
            continue;
        }
        let r = match pj.on_done {
            OnDone::AttachProxies => attach(s, &outs, false).map(|_| ()),
            OnDone::ReplaceMedia => replace_media(s, &outs),
            OnDone::Nothing => Ok(()),
        };
        if let Err(e) = r {
            s.error_toast("job", e.to_string());
        }
    }
}

/// Check a proxy file against its clip: duration within one frame, same frame rate.
fn check_proxy(s: &Session, item: ItemId, path: &str) -> Result<()> {
    let (clip, _, name) = media_path(&s.project, item).ok_or_else(|| bad("media.attachProxies", format!("item {} is not file media", item.0)))?;
    let src = s.media.open_file(path, &*s.services).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let pi = src.info();
    let fd = clip.info.frame_rate().frame_duration();
    if (pi.duration - clip.info.duration).0.abs() > fd.0 {
        return Err(EngineError::Other(format!(
            "{}: the proxy is {:.3}s long but {name} is {:.3}s",
            Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            pi.duration.seconds(),
            clip.info.duration.seconds()
        )));
    }
    if let (Some(a), Some(b)) = (&pi.video, &clip.info.video)
        && a.frame_rate != b.frame_rate
    {
        return Err(EngineError::Other(format!("the proxy runs at {} fps but {name} at {} fps", a.frame_rate.label(), b.frame_rate.label())));
    }
    if pi.video.is_none() && clip.info.video.is_some() {
        return Err(EngineError::Other("the proxy has no video".into()));
    }
    Ok(())
}

/// Attach proxies (checked unless `force`). One undo step.
pub fn attach(s: &mut Session, pairs: &[(ItemId, String)], force: bool) -> Result<Value> {
    if !force {
        for (i, p) in pairs {
            check_proxy(s, *i, p)?;
        }
    }
    let pairs = pairs.to_vec();
    s.edit("Attach Proxies", |proj, _| {
        for (i, p) in &pairs {
            if let Some(m) = proj.item_mut(*i).and_then(|it| it.as_media_mut()) {
                m.proxy = Some(MediaRef::File { path: p.clone() });
            }
        }
        Ok(())
    })?;
    Ok(json!({"attached": pairs.iter().map(|(i, p)| json!({"item": i.0, "path": p})).collect::<Vec<_>>()}))
}

fn replace_media(s: &mut Session, outs: &[(ItemId, String)]) -> Result<()> {
    for (i, p) in outs {
        let q = json!({"item": i.0, "path": p, "force": true, "relinkOthers": false});
        crate::relink::relink(s, &q)?;
    }
    Ok(())
}

/// Where proxies of `media` go: the chosen folder, else `<media dir>/Proxies`.
pub fn proxy_dir(media: &str, dest: Option<&str>) -> PathBuf {
    match dest {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => Path::new(media).parent().unwrap_or(Path::new(".")).join("Proxies"),
    }
}

/// After a project opens: proxies whose files are gone (but whose originals are there) are made
/// again at the same paths, in the background (`wait` blocks); returns the job when one started.
/// The preset follows the proxy file's extension (`.mp4` = H.264, else ProRes Proxy) at ¼ size.
pub fn recreate_missing(s: &mut Session, wait: bool) -> Option<u64> {
    let mut work = Vec::new();
    for (id, it) in s.project.items.iter() {
        let Some(m) = it.as_media() else { continue };
        let (Some(MediaRef::File { path: proxy }), MediaRef::File { path: original }) = (&m.proxy, &m.media) else { continue };
        if m.offline || s.services.file_size(proxy).is_ok() || s.services.file_size(original).is_err() {
            continue;
        }
        let pr = if proxy.to_ascii_lowercase().ends_with(".mp4") { preset("h264_quarter") } else { preset(DEFAULT_PROXY_PRESET) };
        if let Some(pr) = pr {
            work.push((*id, proxy.clone(), pr));
        }
    }
    if work.is_empty() {
        return None;
    }
    let n = work.len();
    match start_job(s, format!("Recreate Missing Proxies ({n})"), work, OnDone::AttachProxies, wait) {
        Ok(job) => {
            s.toast(format!("{n} proxy file(s) were missing; recreating them"));
            Some(job)
        }
        Err(e) => {
            s.log.push(crate::panels::Level::Warning, "proxies", format!("recreating missing proxies: {e}"));
            None
        }
    }
}

/// `media.createProxies {items?, preset?, destination?, wait?}`.
pub fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let unknown = || bad("media.createProxies", "unknown preset (see media.proxyPresets / media.proxyProfiles)");
    // an explicit preset is used for every clip; otherwise Preferences ▸ Media ▸ Proxy Clips: the
    // encoding profile, and the alpha profile for clips with an alpha channel
    let explicit = str_p(p, "preset").map(|id| resolve_encoding(s, id).ok_or_else(unknown)).transpose()?;
    let m = &s.prefs.media;
    let normal = match &explicit {
        Some(e) => e.clone(),
        None => resolve_encoding(s, if m.proxy_profile.is_empty() { DEFAULT_PROXY_PRESET } else { m.proxy_profile.as_str() })
            .or_else(|| preset(DEFAULT_PROXY_PRESET).map(Preset::encoding))
            .ok_or_else(unknown)?,
    };
    let alpha = match &explicit {
        Some(e) => e.clone(),
        None => resolve_encoding(s, if m.proxy_alpha_profile.is_empty() { DEFAULT_ALPHA_PROXY_PRESET } else { m.proxy_alpha_profile.as_str() })
            .or_else(|| preset(DEFAULT_ALPHA_PROXY_PRESET).map(Preset::encoding))
            .ok_or_else(unknown)?,
    };
    // Proxy video resize (width): the request's, else the profile's own, else the preference
    let width = u64_p(p, "width").map(|w| w as u32).or((m.proxy_video_width > 0).then_some(m.proxy_video_width));
    let image_width = u64_p(p, "imageWidth").map(|w| w as u32).unwrap_or(m.proxy_image_width).max(16);
    let with_width = |mut e: Encoding| {
        if u64_p(p, "width").is_some() || e.width.is_none() {
            e.width = width.or(e.width);
        }
        e
    };
    let (normal, alpha) = (with_width(normal), with_width(alpha));
    let items = items_param(s, p);
    // default: Project Settings ▸ Scratch Disks ▸ Captured and Generated, else next to the media
    let dest = str_p(p, "destination")
        .map(str::to_string)
        .or_else(|| s.project.settings.scratch.captured.clone().filter(|d| !d.is_empty()).map(|d| format!("{d}/Proxies")));
    let mut work = Vec::new();
    let mut taken = Vec::new();
    let mut skipped = Vec::new();
    for i in items {
        let Some((clip, path, name)) = media_path(&s.project, i) else {
            skipped.push(json!({"item": i.0, "reason": "not file media"}));
            continue;
        };
        let Some(v) = clip.info.video.as_ref() else {
            skipped.push(json!({"item": i.0, "reason": format!("{name} has no video to proxy")}));
            continue;
        };
        let stem = Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name.clone());
        let dir = proxy_dir(&path, dest.as_deref());
        let (kind, ext) = match clip.info.kind {
            MediaKind::Movie => {
                let e = if v.has_alpha { alpha.clone() } else { normal.clone() };
                let ext = e.extension;
                (JobKind::Transcode(e), ext)
            }
            // Proxy Clips ▸ image proxies: stills wider than the proxy image width
            MediaKind::Still if v.width > image_width => (JobKind::Still { width: image_width }, "png"),
            MediaKind::Still => {
                skipped.push(json!({"item": i.0, "reason": format!("{name} is already {} px wide or narrower", image_width)}));
                continue;
            }
            _ => {
                skipped.push(json!({"item": i.0, "reason": format!("{name} has no video to proxy")}));
                continue;
            }
        };
        let out = unique_path(&dir, &stem, "_Proxy", ext, &taken).to_string_lossy().into_owned();
        taken.push(out.clone());
        work.push((i, out, kind));
    }
    if work.is_empty() {
        return Err(EngineError::Other(format!("nothing to proxy: {}", skipped.iter().filter_map(|v| v["reason"].as_str()).collect::<Vec<_>>().join("; "))));
    }
    let outputs: Vec<Value> = work.iter().map(|(i, o, _)| json!({"item": i.0, "path": o})).collect();
    let n = work.len();
    let label = format!("Create Proxies ({n} clip{})", if n == 1 { "" } else { "s" });
    let attach = bool_p(p, "attach").unwrap_or(true);
    let job = start_media_job(s, label, work, if attach { OnDone::AttachProxies } else { OnDone::Nothing }, bool_p(p, "wait").unwrap_or(false))?;
    Ok(json!({"job": job, "preset": normal.id, "alphaPreset": alpha.id, "outputs": outputs, "skipped": skipped}))
}

/// Proxy Clips on import (Preferences ▸ Media): attach camera proxy files (External proxy clips),
/// then create proxies of videos and stills wider than the thresholds. Needs Enable proxies;
/// clips that already have a proxy, or that Ingest is about to proxy, are left alone.
pub fn auto_proxies(s: &mut Session, items: &[ItemId]) -> Result<Value> {
    let m = s.prefs.media.clone();
    if !m.enable_proxies || items.is_empty() || !(m.proxy_auto_video || m.proxy_auto_image || m.proxy_external) {
        return Ok(Value::Null);
    }
    let ing = &s.project.settings.ingest;
    if ing.enabled && matches!(ing.action, IngestAction::CreateProxies | IngestAction::CopyAndCreateProxies) {
        return Ok(Value::Null);
    }
    let fresh: Vec<ItemId> = items.iter().copied().filter(|i| s.project.item(*i).and_then(|it| it.as_media()).is_some_and(|c| c.proxy.is_none())).collect();
    let mut external = Vec::new();
    if m.proxy_external {
        let pairs: Vec<(ItemId, String)> = fresh
            .iter()
            .filter_map(|i| {
                let (clip, path, _) = media_path(&s.project, *i)?;
                (clip.info.kind == MediaKind::Movie).then_some(())?;
                camera_proxy(&path).map(|p| (*i, p))
            })
            .collect();
        for pair in pairs {
            // a camera file that does not match the clip (duration, rate) is skipped, not fatal
            if check_proxy(s, pair.0, &pair.1).is_ok() {
                external.push(pair);
            }
        }
        if !external.is_empty() {
            attach(s, &external, true)?;
        }
    }
    let wanted: Vec<u64> = fresh
        .iter()
        .filter(|i| !external.iter().any(|e| e.0 == **i))
        .filter(|i| {
            let Some(c) = s.project.item(**i).and_then(|it| it.as_media()) else { return false };
            let Some(v) = c.info.video.as_ref() else { return false };
            match c.info.kind {
                MediaKind::Movie => m.proxy_auto_video && v.width > m.proxy_auto_video_width,
                MediaKind::Still => m.proxy_auto_image && v.width > m.proxy_auto_image_width,
                _ => false,
            }
        })
        .map(|i| i.0)
        .collect();
    let job = if wanted.is_empty() { Value::Null } else { create(s, &json!({"items": wanted}))? };
    Ok(json!({"external": external.iter().map(|(i, p)| json!({"item": i.0, "path": p})).collect::<Vec<_>>(), "created": job}))
}

/// The low-resolution file a camera records beside a clip, when it exists: GoPro `GX010123.MP4`
/// → `GL010123.LRV` (or the same name with `.LRV`), DJI `DJI_0001.MP4` → `DJI_0001.LRF`, Sony
/// `…/CLIP/C0001.MP4` → `…/SUB/C0001S03.MP4`.
pub fn camera_proxy(path: &str) -> Option<String> {
    let p = Path::new(path);
    let dir = p.parent()?;
    let stem = p.file_stem()?.to_string_lossy().into_owned();
    let exists = |q: &Path| q.is_file();
    let mut cands: Vec<PathBuf> = Vec::new();
    for ext in ["LRV", "lrv", "LRF", "lrf"] {
        cands.push(dir.join(format!("{stem}.{ext}")));
    }
    // GoPro: the second letter of the chaptered name becomes L (GX/GH/GP → GL)
    if stem.len() > 2 && stem.is_char_boundary(2) && stem.to_ascii_uppercase().starts_with('G') {
        let low = format!("GL{}", &stem[2..]);
        for ext in ["LRV", "lrv"] {
            cands.push(dir.join(format!("{low}.{ext}")));
        }
    }
    // Sony XAVC: PRIVATE/M4ROOT/CLIP/C0001.MP4 + PRIVATE/M4ROOT/SUB/C0001S03.MP4
    if dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case("CLIP"))
        && let Some(root) = dir.parent()
    {
        for sub in ["SUB", "sub"] {
            for ext in ["MP4", "mp4"] {
                cands.push(root.join(sub).join(format!("{stem}S03.{ext}")));
            }
        }
    }
    cands.into_iter().find(|c| exists(c) && c.as_path() != p).map(|c| c.to_string_lossy().into_owned())
}

/// Ingest freshly imported items per the project's ingest settings.
pub fn ingest(s: &mut Session, items: &[ItemId]) -> Result<Value> {
    let ing = s.project.settings.ingest.clone();
    if !ing.enabled || items.is_empty() {
        return Ok(Value::Null);
    }
    let mut copied = Vec::new();
    let copy = matches!(ing.action, IngestAction::Copy | IngestAction::CopyAndCreateProxies);
    if copy {
        for &i in items {
            let Some((_, path, _)) = media_path(&s.project, i) else { continue };
            let dir = match &ing.destination {
                Some(d) if !d.is_empty() => PathBuf::from(d),
                _ => Path::new(&path).parent().unwrap_or(Path::new(".")).join("Ingested Media"),
            };
            let fname = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let out = dir.join(&fname);
            if Path::new(&path) == out {
                continue;
            }
            std::fs::create_dir_all(&dir).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
            std::fs::copy(&path, &out).map_err(|e| EngineError::Other(format!("copying {fname}: {e}")))?;
            let out_s = out.to_string_lossy().into_owned();
            // verify the copy before switching to it
            let a = crate::relink::identity_of(&*s.services, &path).map_err(|e| EngineError::Other(e.to_string()))?;
            let b = crate::relink::identity_of(&*s.services, &out_s).map_err(|e| EngineError::Other(e.to_string()))?;
            if a != b {
                return Err(EngineError::Other(format!("the copy of {fname} doesn't match the original")));
            }
            crate::relink::relink(s, &json!({"item": i.0, "path": out_s, "relinkOthers": false}))?;
            copied.push(json!({"item": i.0, "path": out_s}));
        }
    }
    let mut job = Value::Null;
    match ing.action {
        IngestAction::CreateProxies | IngestAction::CopyAndCreateProxies => {
            // only videos wider than the project's threshold (Ingest Settings ▸ Only for videos wider than)
            let wide = |i: &ItemId| {
                s.project
                    .item(*i)
                    .and_then(|x| x.as_media())
                    .is_some_and(|m| m.info.kind == MediaKind::Movie && m.info.video.as_ref().is_some_and(|v| v.width > ing.min_proxy_width))
            };
            let picked: Vec<u64> = items.iter().filter(|i| wide(i)).map(|i| i.0).collect();
            if !picked.is_empty() {
                let p = json!({"items": picked, "preset": if ing.preset.is_empty() { DEFAULT_PROXY_PRESET } else { ing.preset.as_str() }, "destination": ing.destination});
                job = create(s, &p)?;
            }
        }
        IngestAction::Transcode => {
            let pr =
                preset(if ing.preset.is_empty() { DEFAULT_TRANSCODE_PRESET } else { ing.preset.as_str() }).ok_or_else(|| bad("ingest", "unknown preset"))?;
            let mut work = Vec::new();
            for &i in items {
                let Some((clip, path, name)) = media_path(&s.project, i) else { continue };
                if clip.info.video.is_none() {
                    continue;
                }
                let dir = match &ing.destination {
                    Some(d) if !d.is_empty() => PathBuf::from(d),
                    _ => Path::new(&path).parent().unwrap_or(Path::new(".")).join("Ingested Media"),
                };
                let stem = Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
                work.push((i, unique_path(&dir, &stem, "", pr.extension, &[]).to_string_lossy().into_owned(), pr));
            }
            if !work.is_empty() {
                let id = start_job(s, format!("Ingest: transcode {} clip(s)", work.len()), work, OnDone::ReplaceMedia, false)?;
                job = json!({"job": id});
            }
        }
        IngestAction::Copy => {}
    }
    Ok(json!({"copied": copied, "job": job}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "media.createProxies",
            label: "Create Proxies…",
            menu: &["Clip", "Proxy"],
            shortcut: None,
            params: r#"{"items":[id]?,"preset":"prores_proxy_quarter|prores_proxy_half|prores_lt_half|h264_quarter|h264_half|prores_4444_quarter|prores_4444_half|custom:<name>"?,"width":int?,"imageWidth":int?,"destination":str?,"attach":bool=true,"wait":bool=false}"#,
            enabled: has_file_media,
            run: create,
            journal: true,
        },
        CommandSpec {
            id: "media.attachProxies",
            label: "Attach Proxies…",
            menu: &["Clip", "Proxy"],
            shortcut: None,
            params: r#"{"item":id,"path":str}|{"items":[id],"paths":[str]},"force":bool=false"#,
            enabled: has_file_media,
            run: |s, p| {
                let pairs: Vec<(ItemId, String)> = match (p.get("items").and_then(Value::as_array), p.get("paths").and_then(Value::as_array)) {
                    (Some(i), Some(ps)) => i.iter().zip(ps).filter_map(|(i, p)| Some((ItemId(i.as_u64()?), p.as_str()?.to_string()))).collect(),
                    _ => vec![(
                        u64_p(p, "item").map(ItemId).ok_or_else(|| bad("media.attachProxies", "need `item`"))?,
                        str_p(p, "path").ok_or_else(|| bad("media.attachProxies", "need `path`"))?.to_string(),
                    )],
                };
                attach(s, &pairs, bool_p(p, "force").unwrap_or(false))
            },
            journal: true,
        },
        CommandSpec {
            id: "media.detachProxies",
            label: "Detach Proxies",
            menu: &["Clip", "Proxy"],
            shortcut: None,
            params: r#"{"items":[id]?,"deleteFiles":bool=false}"#,
            enabled: has_file_media,
            run: |s, p| {
                let items = items_param(s, p);
                let delete = p.get("deleteFiles").and_then(Value::as_bool).unwrap_or(false);
                let mut paths: Vec<String> = Vec::new();
                let n = s.edit(if delete { "Delete Proxies" } else { "Detach Proxies" }, |proj, _| {
                    let mut n = 0;
                    for i in &items {
                        if let Some(m) = proj.item_mut(*i).and_then(|it| it.as_media_mut())
                            && let Some(px) = m.proxy.take()
                        {
                            if let MediaRef::File { path } = px {
                                paths.push(path);
                            }
                            n += 1;
                        }
                    }
                    Ok(n)
                })?;
                // Clip Properties ▸ Delete proxy: the file goes too, unless something else still uses it
                let mut deleted = Vec::new();
                if delete && !cfg!(target_arch = "wasm32") {
                    for path in paths {
                        let used = s.project.items.values().filter_map(|it| it.as_media()).any(|m| {
                            matches!(&m.proxy, Some(MediaRef::File { path: q }) if *q == path) || matches!(&m.media, MediaRef::File { path: q } if *q == path)
                        });
                        if !used && std::fs::remove_file(&path).is_ok() {
                            deleted.push(path);
                        }
                    }
                }
                Ok(json!({"detached": n, "deleted": deleted}))
            },
            journal: true,
        },
        CommandSpec {
            id: "media.reconnectFullRes",
            label: "Reconnect Full Resolution Media…",
            menu: &["Clip", "Proxy"],
            shortcut: None,
            params: r#"{"item":id,"path":str}"#,
            enabled: has_file_media,
            run: |s, p| {
                let item = u64_p(p, "item").map(ItemId).ok_or_else(|| bad("media.reconnectFullRes", "need `item`"))?;
                if s.project.item(item).and_then(|i| i.as_media()).is_none_or(|m| m.proxy.is_none()) {
                    return Err(EngineError::Other("the clip has no proxy attached".into()));
                }
                let mut q = p.clone();
                q["match"] = json!({"fileName": false, "extension": false});
                q["relinkOthers"] = json!(false);
                crate::relink::relink(s, &q)
            },
            journal: true,
        },
        CommandSpec {
            id: "media.toggleProxies",
            label: "Toggle Proxies",
            menu: &["View"],
            shortcut: None,
            params: r#"{"enabled":bool?}"#,
            enabled: always,
            run: |s, p| {
                let on = bool_p(p, "enabled").unwrap_or(!s.prefs.media.enable_proxies);
                let mut next = s.prefs.clone();
                next.media.enable_proxies = on;
                s.set_prefs(next).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
                let attached = s.project.items.values().filter(|i| i.as_media().is_some_and(|m| m.proxy.is_some())).count();
                Ok(json!({"enabled": on, "clipsWithProxies": attached}))
            },
            journal: false,
        },
        CommandSpec {
            id: "media.proxyPresets",
            label: "Proxy Presets",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: always,
            run: |_, _| Ok(serde_json::to_value(PRESETS).unwrap_or_default()),
            journal: false,
        },
        CommandSpec {
            id: "media.proxyProfiles",
            label: "Proxy Profiles",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: always,
            run: |s, _| Ok(profiles_json(s)),
            journal: false,
        },
        CommandSpec {
            id: "media.saveProxyProfile",
            label: "Save Proxy Profile",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str,"params":str,"previous":str?}"#,
            enabled: always,
            run: save_profile,
            journal: false,
        },
        CommandSpec {
            id: "media.deleteProxyProfile",
            label: "Delete Proxy Profile",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str}"#,
            enabled: always,
            run: |s, p| {
                let name = str_p(p, "name").ok_or_else(|| bad("media.deleteProxyProfile", "need `name`"))?.to_string();
                let mut next = s.prefs.clone();
                let before = next.media.proxy_profiles.len();
                next.media.proxy_profiles.retain(|x| x.name != name);
                if next.media.proxy_profiles.len() == before {
                    return Err(EngineError::Other(format!("no proxy profile named {name}")));
                }
                let id = format!("custom:{name}");
                if next.media.proxy_profile == id {
                    next.media.proxy_profile.clear();
                }
                if next.media.proxy_alpha_profile == id {
                    next.media.proxy_alpha_profile.clear();
                }
                s.set_prefs(next).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
                Ok(profiles_json(s))
            },
            journal: false,
        },
        CommandSpec {
            id: "project.ingestSettings",
            label: "Ingest Settings…",
            menu: &[],
            shortcut: None,
            params: r#"{"enabled":bool?,"action":"copy|transcode|createProxies|copyAndCreateProxies"?,"destination":str?,"preset":str?,"minProxyWidth":u32?}"#,
            enabled: always,
            run: |s, p| {
                let mut ing = s.project.settings.ingest.clone();
                if let Some(b) = bool_p(p, "enabled") {
                    ing.enabled = b;
                }
                if let Some(a) = p.get("action") {
                    ing.action = serde_json::from_value(a.clone()).map_err(|e| bad("project.ingestSettings", e.to_string()))?;
                }
                if let Some(d) = p.get("destination") {
                    ing.destination = d.as_str().filter(|d| !d.is_empty()).map(str::to_string);
                }
                if let Some(pr) = str_p(p, "preset") {
                    if !pr.is_empty() && preset(pr).is_none() {
                        return Err(bad("project.ingestSettings", format!("unknown preset `{pr}`")));
                    }
                    ing.preset = pr.to_string();
                }
                if let Some(w) = u64_p(p, "minProxyWidth") {
                    ing.min_proxy_width = w.min(u64::from(redrob_film_project::MAX_FRAME_SIDE)) as u32;
                }
                if ing != s.project.settings.ingest {
                    let v = ing.clone();
                    s.edit("Ingest Settings", |proj, _| {
                        proj.settings.ingest = v;
                        Ok(())
                    })?;
                }
                Ok(serde_json::to_value(&s.project.settings.ingest).unwrap_or_default())
            },
            journal: true,
        },
    ]
}

/// `media.proxyProfiles`: the built-in proxy presets, the custom profiles (with their parsed
/// encoding or problems), the chosen profiles and the allowed parameters.
pub fn profiles_json(s: &Session) -> Value {
    let builtin: Vec<Value> = PRESETS.iter().filter(|p| p.proxy).map(|p| json!({"id": p.id, "label": p.label, "alpha": p.alpha, "builtin": true})).collect();
    let custom: Vec<Value> = s
        .prefs
        .media
        .proxy_profiles
        .iter()
        .map(|p| {
            let parsed = parse_profile(&p.name, &p.params);
            json!({
                "id": format!("custom:{}", p.name),
                "label": p.name,
                "params": p.params,
                "alpha": parsed.as_ref().is_ok_and(|e| e.alpha),
                "builtin": false,
                "problems": parsed.err(),
            })
        })
        .collect();
    let m = &s.prefs.media;
    json!({
        "builtin": builtin,
        "custom": custom,
        "profile": if m.proxy_profile.is_empty() { DEFAULT_PROXY_PRESET } else { m.proxy_profile.as_str() },
        "alphaProfile": if m.proxy_alpha_profile.is_empty() { DEFAULT_ALPHA_PROXY_PRESET } else { m.proxy_alpha_profile.as_str() },
        "allowedParams": PROXY_PARAMS.iter().map(|(k, d)| json!({"key": k, "values": d})).collect::<Vec<_>>(),
    })
}

/// `media.saveProxyProfile {name, params, previous?}`: add a profile, or replace `previous`
/// (renaming it). Parameters outside [`PROXY_PARAMS`] are refused, naming them.
fn save_profile(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("media.saveProxyProfile", "need a `name`"))?.to_string();
    if name.contains(':') || name.len() > 80 {
        return Err(bad("media.saveProxyProfile", "a profile name has at most 80 characters and no ':'"));
    }
    let params = str_p(p, "params").unwrap_or("").trim().to_string();
    if let Err(e) = parse_profile(&name, &params) {
        let mut why = Vec::new();
        if !e.unknown.is_empty() {
            why.push(format!(
                "parameters not allowed: {} (allowed: {})",
                e.unknown.join(", "),
                PROXY_PARAMS.iter().map(|x| x.0).collect::<Vec<_>>().join(", ")
            ));
        }
        why.extend(e.errors);
        return Err(EngineError::Other(why.join("; ")));
    }
    let previous = str_p(p, "previous").map(str::to_string);
    let mut next = s.prefs.clone();
    let list = &mut next.media.proxy_profiles;
    if list.iter().any(|x| x.name == name && previous.as_deref() != Some(name.as_str())) {
        return Err(EngineError::Other(format!("a proxy profile named {name} already exists")));
    }
    let profile = crate::autosave::ProxyProfile { name: name.clone(), params };
    let pos = previous.as_deref().and_then(|old| list.iter().position(|x| x.name == old));
    match pos {
        Some(i) => list[i] = profile,
        None => list.push(profile),
    }
    // a renamed profile stays the chosen one
    if let Some(old) = previous.filter(|o| *o != name) {
        let (o, n) = (format!("custom:{old}"), format!("custom:{name}"));
        if next.media.proxy_profile == o {
            next.media.proxy_profile = n.clone();
        }
        if next.media.proxy_alpha_profile == o {
            next.media.proxy_alpha_profile = n;
        }
    }
    s.set_prefs(next).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
    Ok(profiles_json(s))
}

fn has_file_media(s: &Session) -> std::result::Result<(), String> {
    if s.project.items.values().any(|i| i.as_media().is_some_and(|m| matches!(m.media, MediaRef::File { .. }))) {
        Ok(())
    } else {
        Err("the project has no file-based media".into())
    }
}
