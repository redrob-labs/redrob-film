//! Hand-offs between the two editors (Edit, the timeline, and Effects, compositions): a stretch of the timeline becomes an effects composition,
//! and a composition becomes a clip on the timeline.
//!
//! Both editors count time in the same unit (254 016 000 000 ticks a second: crates/time/src/
//! lib.rs L20 and editors/effects/crates/time/src/lib.rs L17), so a tick value crosses over
//! unchanged; only the newtype differs (`redrob_film_time::Tick(i64)` / `redrob_effects_time::Tick(i64)`).
//!
//! Timeline → composition ([`timeline_to_composition`]): every selected video clip (else every
//! video clip in the in/out range, else the whole sequence) whose media is a file becomes a
//! footage item and a layer of a new composition the size and rate of the sequence. The geometry
//! is [`layer_geometry`]: After Effects layers and Premiere clips describe the same thing with
//! different numbers (clip `start/duration/source_in/speed`, crates/project/src/lib.rs L511; layer
//! `start_time/in_point/out_point/stretch`, editors/effects/crates/project/src/lib.rs L976-981).
//!
//! Composition → timeline ([`composition_to_timeline`]): the active composition is wrapped in a
//! [`CompSource`], a `redrob_film_media::MediaSource` (crates/media/src/lib.rs L158) that renders
//! frames with the effects renderer (`Renderer::comp_frame`, editors/effects/crates/render/src/
//! lib.rs L944) on demand, registered in the Edit engine's media pool for a placeholder media item
//! (`MediaPool::insert`, crates/engine/src/media_pool.rs L133: a source inserted without a key
//! stands for the item whatever its reference). No file is rendered and no export runs. The
//! source holds a snapshot of the effects project: edits made in Effects afterwards show on the
//! timeline after the hand-off is run again (it replaces the source of the same composition).
//!
//! Frame mapping of [`CompSource`]: size = the composition's width × height at the request's scale
//! (the renderer scales, so a half-resolution playback request renders at half size); frame rate =
//! the composition's; pixel aspect = square (a composition's `pixel_aspect` is folded into its
//! width by the effects renderer, so the timeline sees square pixels); duration = the composition's.
//! Colour: the effects renderer composites premultiplied f32 in the project's output space (sRGB
//! unless the effects project says otherwise, editors/effects/crates/render/src/color.rs L55)
//! and `Image::to_rgba8` un-premultiplies and quantises (editors/effects/crates/raster/src/
//! lib.rs L201), which is exactly `PixelData::Rgba8`, straight-alpha sRGB-encoded (crates/frame/
//! src/lib.rs L47), so the frame is tagged `ColorInfo::SRGB_FULL` with alpha. Audio: none (a
//! composition's audio mix is not rendered here; the clip is picture-only, `audio_streams` empty).

use std::collections::HashMap;
use std::sync::Arc;

use redrob_film_color::ColorInfo;
use redrob_film_frame::{AudioBuffer, VideoFrame};
use redrob_film_media::{FrameRequest, MediaError, MediaInfo, MediaKind, MediaSource, VideoStreamInfo};
use redrob_film_project::{ItemKind, Label, MediaClip, MediaRef};
use redrob_film_time::Tick;
use serde_json::{Value, json};

// ----------------------------------------------------------------------------------------------
// The mode commands (the app's shell and the headless MCP server answer the same ids)
// ----------------------------------------------------------------------------------------------

/// `mode.switch {mode: "edit"|"effects"}`: show the other editor.
pub const SWITCH_MODE: &str = "mode.switch";
/// `mode.sendSelectionToEffects {}`: the Edit selection (or in/out range) becomes a composition.
pub const SEND_SELECTION_TO_EFFECTS: &str = "mode.sendSelectionToEffects";
/// `mode.placeCompositionOnTimeline {}`: the active composition becomes a clip at the playhead.
pub const PLACE_COMPOSITION_ON_TIMELINE: &str = "mode.placeCompositionOnTimeline";
/// `mode.status {}`: which mode is active, which editors exist, what the hand-offs could do.
pub const STATUS: &str = "mode.status";
/// `effects.run {id, params?}`: run an Effects engine command (editors/effects/crates/engine/
/// src/commands/*.rs; `effects.run {id: "command.list"}` lists them) on the Effects session.
pub const EFFECTS_RUN: &str = "effects.run";

/// Every mode command id.
pub const ALL: [&str; 5] = [SWITCH_MODE, SEND_SELECTION_TO_EFFECTS, PLACE_COMPOSITION_ON_TIMELINE, STATUS, EFFECTS_RUN];

/// The mode commands' entries for `command.list`, shaped like the engine's (crates/engine/src/
/// commands.rs: id, label, menu, shortcut, shortcuts, defaultShortcut, params, enabled).
pub fn catalogue(can_send_selection: bool, can_place_composition: bool) -> Vec<Value> {
    let entry = |id: &str, label: &str, params: &str, enabled: bool| {
        json!({
            "id": id,
            "label": label,
            "menu": ["Mode"],
            "shortcut": Value::Null,
            "shortcuts": [],
            "defaultShortcut": Value::Null,
            "params": params,
            "enabled": enabled,
        })
    };
    vec![
        entry(SWITCH_MODE, "Switch Mode", r#"{"mode":"edit"|"effects"}"#, true),
        entry(SEND_SELECTION_TO_EFFECTS, "Send Selection to Effects", r#"{"name":str? (the new composition's name)}"#, can_send_selection),
        entry(
            PLACE_COMPOSITION_ON_TIMELINE,
            "Place Composition on Timeline",
            r#"{"track":"V1"|id?,"insert":bool? (default false: overwrite at the playhead)}"#,
            can_place_composition,
        ),
        entry(STATUS, "Mode Status", "{}", true),
        entry(
            EFFECTS_RUN,
            "Run Effects Command",
            r#"{"id":str (an Effects command id; "command.list" lists them, "command.list" with {"filter":str} narrows),"params":object?}"#,
            true,
        ),
    ]
}

/// The `path` of a placeholder media item that stands for an effects composition:
/// `effects://comp/<item id>`. Never a file; the media pool is handed the source directly.
pub const COMP_SCHEME: &str = "effects://comp/";

/// The path of the placeholder item for composition `id`.
pub fn comp_path(id: redrob_effects_project::ItemId) -> String {
    format!("{COMP_SCHEME}{}", id.0)
}

/// The composition id a placeholder path names, if it is one.
pub fn parse_comp_path(path: &str) -> Option<redrob_effects_project::ItemId> {
    path.strip_prefix(COMP_SCHEME)?.parse::<u64>().ok().map(redrob_effects_project::ItemId)
}

// ----------------------------------------------------------------------------------------------
// Geometry (pure; the unit tests drive this part)
// ----------------------------------------------------------------------------------------------

/// A timeline clip's timing, in ticks: `TrackItem::{start, duration, source_in, speed}`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipGeom {
    pub start: i64,
    pub duration: i64,
    pub source_in: i64,
    /// Playback speed (1.0 = 100 %).
    pub speed: f64,
}

/// A composition layer's timing, in ticks: `Layer::{start_time, in_point, out_point, stretch}`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerGeom {
    /// Comp time where the layer's own time 0 sits.
    pub start_time: i64,
    pub in_point: i64,
    pub out_point: i64,
    /// Time stretch in percent (100 = normal).
    pub stretch: f64,
}

/// The composition's span for a set of clips: from the first clip's start to the last clip's end.
/// `None` without clips.
pub fn comp_range(clips: &[ClipGeom]) -> Option<(i64, i64)> {
    let start = clips.iter().map(|c| c.start).min()?;
    let end = clips.iter().map(|c| c.start.saturating_add(c.duration)).max()?;
    Some((start, end))
}

/// The layer that shows the same frames at the same times as `clip`, in a composition whose time
/// 0 is sequence time `comp_start`.
///
/// A clip shows media time `source_in + (t - start) * speed` at sequence time `t`
/// (crates/project/src/lib.rs L520-523). A layer shows layer time
/// `(t - start_time) * 100 / stretch` at comp time `t` (`Layer::layer_time`, editors/effects/
/// crates/project/src/lib.rs L1014). So `stretch = 100 / speed`, the visible span is the clip's
/// span shifted by `comp_start`, and `start_time` is where layer time 0 (media time 0) falls:
/// `in_point - source_in / speed`.
///
/// Reverse and stopped clips (`speed <= 0`, or not finite) are refused: a layer's negative stretch
/// reverses around a different anchor and a frozen frame has no finite stretch.
pub fn layer_geometry(clip: &ClipGeom, comp_start: i64) -> Result<LayerGeom, String> {
    if !clip.speed.is_finite() || clip.speed <= 0.0 {
        return Err(format!("speed {} is not supported (forward speeds only)", clip.speed));
    }
    if clip.duration <= 0 {
        return Err("the clip has no duration".into());
    }
    let in_point = clip.start.saturating_sub(comp_start);
    let out_point = in_point.saturating_add(clip.duration);
    let stretch = 100.0 / clip.speed;
    // media time -> comp time offset: source_in / speed, rounded to a tick
    let media_offset = (clip.source_in as f64 / clip.speed).round();
    if !media_offset.is_finite() || media_offset.abs() > i64::MAX as f64 / 2.0 {
        return Err("the clip's source offset is out of range".into());
    }
    let start_time = in_point.saturating_sub(media_offset as i64);
    Ok(LayerGeom { start_time, in_point, out_point, stretch })
}

// ----------------------------------------------------------------------------------------------
// Composition as a timeline source
// ----------------------------------------------------------------------------------------------

/// The media description of a composition as the timeline sees it (see the module docs).
pub fn comp_media_info(name: &str, comp: &redrob_effects_project::Comp) -> Result<MediaInfo, String> {
    if comp.width == 0 || comp.height == 0 {
        return Err(format!("composition \"{name}\" has no size"));
    }
    if comp.frame_rate.num <= 0 || comp.frame_rate.den <= 0 {
        return Err(format!("composition \"{name}\" has no frame rate"));
    }
    if comp.duration.0 <= 0 {
        return Err(format!("composition \"{name}\" has no duration"));
    }
    Ok(MediaInfo {
        name: name.to_string(),
        kind: MediaKind::Movie,
        duration: Tick(comp.duration.0),
        video: Some(VideoStreamInfo {
            width: comp.width,
            height: comp.height,
            frame_rate: redrob_film_time::FrameRate::new(comp.frame_rate.num, comp.frame_rate.den),
            par: (1, 1),
            codec: "Composition".into(),
            pixel_format: "rgba8".into(),
            color: ColorInfo::SRGB_FULL,
            has_alpha: true,
            bitrate: None,
            hdr: None,
        }),
        audio_streams: Vec::new(),
        container: "effects".into(),
        start_timecode: None,
        file_size: None,
    })
}

/// A `MediaSource` that renders an effects composition on demand.
pub struct CompSource {
    info: MediaInfo,
    project: Arc<redrob_effects_project::Project>,
    comp: redrob_effects_project::ItemId,
    footage: Arc<dyn redrob_effects_render::FootageSource>,
    /// Processed layers kept across this composition's frames, one cache per compositor (a
    /// buffer an effect made on the GPU is not the CPU's, and neither may stand in for the other).
    gpu_cache: redrob_effects_render::LayerCache,
    cpu_cache: redrob_effects_render::LayerCache,
    /// The compositor this composition's frames render on ([`CompSource::compositor`]).
    choice: std::sync::Mutex<Choice>,
    /// Frames rendering now ([`CompSource::render`] lets [`GPU_FRAMES`] / [`CPU_FRAMES`] in), and
    /// its condition; `decided` is `choice`'s.
    busy: std::sync::Mutex<usize>,
    changed: std::sync::Condvar,
    decided: std::sync::Condvar,
}

/// Frames of one composition rendering at once on the GPU compositor. An export asks for a frame
/// on every thread (16 on the PC); all of them on the GPU at once took 1.2 s each, and the CPU
/// conversions of 16 footage frames fought the GPU work for the cores. Two and four came out the
/// same on the RTX 4060 laptop; one was slower.
const GPU_FRAMES: usize = 4;
/// The same for the CPU compositor, which already renders each frame on every core.
const CPU_FRAMES: usize = 2;

/// How far a placed composition is in choosing its compositor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Undecided,
    /// One frame request is measuring both. The lock is not held meanwhile: export frames render
    /// on rayon, which may run another frame's request on the measuring thread while it waits
    /// inside the renderer, and that request must not wait for the measurement below it.
    Measuring,
    Decided(Compositor),
}

/// Bytes of processed layers a placed composition keeps between frames (per compositor; the one
/// not chosen holds only the frames it was measured on).
const COMP_CACHE_BYTES: usize = 256 << 20;

/// The compositor a placed composition renders on. All its frames render on one of them, never a
/// mix: the two differ slightly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Compositor {
    Gpu,
    Cpu,
}

impl Compositor {
    fn label(self) -> &'static str {
        match self {
            Compositor::Gpu => "GPU",
            Compositor::Cpu => "CPU",
        }
    }
}

/// Frames each compositor renders, timed, when a composition measures which is faster (after
/// one untimed warm-up frame each: layer cache misses, uploads, kernel builds). As upstream
/// EffectCraft's Render Queue (#3335, 4649c27e).
const PROBE_FRAMES: i64 = 2;
/// Compositions shorter than this many frames render on the GPU without measuring: the frames
/// the slower compositor renders for the measurement would cost more than they could save.
const PROBE_MIN_FRAMES: i64 = 12;

/// The faster compositor from measured milliseconds per frame (`None`: not measured, or the GPU
/// stopped). The CPU wins only when clearly faster (`AutoPick::MARGIN`): ties and noise stay on
/// the GPU. Upstream EffectCraft's `pick_renderer` (#3335).
fn pick_compositor(gpu_ms: Option<f64>, cpu_ms: Option<f64>) -> Compositor {
    match (gpu_ms, cpu_ms) {
        (Some(g), Some(c)) if g.is_finite() && c.is_finite() && c < g * redrob_effects_render::AutoPick::MARGIN => Compositor::Cpu,
        (None, Some(_)) => Compositor::Cpu,
        _ => Compositor::Gpu,
    }
}

/// `REDROB_FILM_EFFECTS_GPU`: `off`/`cpu` (the CPU reference renderer, byte-reproducible
/// everywhere), `on`/`gpu` (the GPU without measuring) or unset/`auto` (measure each composition).
fn effects_gpu_env() -> Option<Compositor> {
    let v = std::env::var("REDROB_FILM_EFFECTS_GPU").ok()?;
    match v.trim().to_ascii_lowercase().as_str() {
        "off" | "0" | "false" | "cpu" => Some(Compositor::Cpu),
        "on" | "1" | "true" | "gpu" => Some(Compositor::Gpu),
        _ => None,
    }
}

/// The Effects GPU compositor placed compositions may render on, made on first use; `None`
/// without a usable adapter, or with `REDROB_FILM_EFFECTS_GPU=off`.
fn effects_gpu() -> Option<&'static redrob_effects_gpu::Gpu> {
    static GPU: std::sync::OnceLock<Option<redrob_effects_gpu::Gpu>> = std::sync::OnceLock::new();
    GPU.get_or_init(|| {
        if effects_gpu_env() == Some(Compositor::Cpu) {
            return None;
        }
        match redrob_effects_gpu::Gpu::try_headless() {
            Ok(g) => {
                eprintln!("effects: compositions render on {}", redrob_effects_render::Accelerator::name(&g));
                Some(g)
            }
            Err(e) => {
                eprintln!("effects: compositions render on the CPU (no GPU: {e})");
                None
            }
        }
    })
    .as_ref()
}

/// The thread pool placed compositions render their frames on, apart from the global rayon pool
/// that export frames run on. Export frames take one of a few pooled GPU renderers and wait when
/// all are busy; a frame of a placed composition renders inside one of them. Rendered on the global
/// pool, the renderer's parallel work let a waiting worker pick up another export frame, which then
/// waited for a renderer beneath the one its own thread held: with every renderer held that way the
/// export stopped for good (seen on the Windows PC with a scene on the GPU). On this pool, reached
/// from a thread that blocks without taking work, nothing beneath a composition frame is an export
/// frame. `None` if the pool can't be made (the frame renders where it was asked for).
fn effects_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("effects-{i}"))
            .build()
            .map_err(|e| eprintln!("effects: no thread pool of its own ({e}); compositions render on the export's threads"))
            .ok()
    })
    .as_ref()
}

impl CompSource {
    pub fn new(
        name: &str,
        project: Arc<redrob_effects_project::Project>,
        comp: redrob_effects_project::ItemId,
        footage: Arc<dyn redrob_effects_render::FootageSource>,
    ) -> Result<Self, String> {
        let c = project.comp(comp).ok_or_else(|| format!("no composition #{}", comp.0))?;
        let info = comp_media_info(name, c)?;
        Ok(Self {
            info,
            project,
            comp,
            footage,
            gpu_cache: redrob_effects_render::LayerCache::new(COMP_CACHE_BYTES),
            cpu_cache: redrob_effects_render::LayerCache::new(COMP_CACHE_BYTES),
            choice: std::sync::Mutex::new(Choice::Undecided),
            busy: std::sync::Mutex::new(0),
            changed: std::sync::Condvar::new(),
            decided: std::sync::Condvar::new(),
        })
    }

    /// A frame at comp time `t` and `scale` on compositor `on`, with that compositor's layer cache,
    /// rendered on [`effects_pool`] from a thread of its own (see there), once fewer than
    /// [`GPU_FRAMES`] / [`CPU_FRAMES`] of this composition are rendering. Waiting here holds no
    /// export renderer: export frames read their sources before they take one
    /// (`FrameRenderer::ahead`).
    fn render(&self, on: Compositor, t: i64, scale: f64) -> (u32, u32, Vec<u8>) {
        let cap = if on == Compositor::Gpu { GPU_FRAMES } else { CPU_FRAMES };
        {
            let mut busy = self.busy.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            while *busy >= cap {
                busy = self.changed.wait(busy).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            *busy += 1;
        }
        struct Done<'a>(&'a CompSource);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                *self.0.busy.lock().unwrap_or_else(std::sync::PoisonError::into_inner) -= 1;
                self.0.changed.notify_all();
            }
        }
        let _done = Done(self);
        self.render_on_pool(on, t, scale)
    }

    fn render_on_pool(&self, on: Compositor, t: i64, scale: f64) -> (u32, u32, Vec<u8>) {
        let Some(pool) = effects_pool() else { return self.render_here(on, t, scale) };
        let frame = std::thread::scope(|s| {
            std::thread::Builder::new().name("effects-frame".into()).spawn_scoped(s, || pool.install(|| self.render_here(on, t, scale))).map(|h| h.join())
        });
        match frame {
            Ok(Ok(image)) => image,
            Ok(Err(panic)) => std::panic::resume_unwind(panic),
            // No thread to hop to: render here (as before the pool).
            Err(_) => self.render_here(on, t, scale),
        }
    }

    /// The frame as straight-alpha RGBA8 (`(width, height, bytes)`). On the GPU it is converted
    /// there and read back at 4 bytes a pixel (`Gpu::render_rgba8`); a frame the GPU declines, and
    /// every CPU frame, is converted from the float image (`Image::to_rgba8`).
    fn render_here(&self, on: Compositor, t: i64, scale: f64) -> (u32, u32, Vec<u8>) {
        let gpu = effects_gpu().filter(|_| on == Compositor::Gpu);
        let backend = if gpu.is_some() { redrob_effects_render::Backend::Gpu } else { redrob_effects_render::Backend::Cpu };
        let opts = redrob_effects_render::RenderOpts { scale, backend, ..Default::default() };
        let mut r = redrob_effects_render::Renderer::new(&self.project, self.footage.as_ref(), opts);
        r.cache = Some(if gpu.is_some() { &self.gpu_cache } else { &self.cpu_cache });
        r.accel = gpu.map(|g| g as &dyn redrob_effects_render::Accelerator);
        let t = redrob_effects_time::Tick(t);
        if let Some(g) = gpu
            && let Some(frame) = g.render_rgba8(&r, self.comp, t)
        {
            return frame;
        }
        let image = if gpu.is_some() { r.comp_frame_cpu(self.comp, t) } else { r.comp_frame(self.comp, t) };
        (image.width, image.height, image.to_rgba8())
    }

    /// The compositor this composition renders on: the CPU without a GPU, the one
    /// `REDROB_FILM_EFFECTS_GPU` names, else the faster of the two measured on the first frame
    /// requested and the next [`PROBE_FRAMES`] (upstream EffectCraft #3335). A footage-only scene
    /// renders faster on the CPU (the GPU pays uploads and readbacks every frame); Glow and other
    /// heavy effects faster on the GPU. Frames requested while another request measures wait for
    /// its choice: rendering meanwhile, they slowed the measurement down and came from a
    /// compositor that might not be chosen. (The wait holds no lock and no export renderer; the
    /// measuring request renders on [`effects_pool`], which an export frame never waits on.)
    fn compositor(&self, t: i64, scale: f64, frame: i64, last: i64) -> Compositor {
        let Some(gpu) = effects_gpu() else { return Compositor::Cpu };
        if let Some(c) = effects_gpu_env() {
            return c;
        }
        {
            let mut slot = self.choice.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                match *slot {
                    Choice::Decided(c) => return c,
                    Choice::Measuring => slot = self.decided.wait(slot).unwrap_or_else(std::sync::PoisonError::into_inner),
                    Choice::Undecided => break,
                }
            }
            *slot = Choice::Measuring;
        }
        // A measurement that panics leaves the GPU chosen rather than the other frames waiting.
        struct Settle<'a>(&'a CompSource);
        impl Drop for Settle<'_> {
            fn drop(&mut self) {
                let mut slot = self.0.choice.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if *slot == Choice::Measuring {
                    *slot = Choice::Decided(Compositor::Gpu);
                }
                self.0.decided.notify_all();
            }
        }
        let _settle = Settle(self);
        let frames = if frame > 0 { last / frame + 1 } else { 0 };
        let c = if frames < PROBE_MIN_FRAMES {
            Compositor::Gpu
        } else {
            // `lose`: the ms a frame at which this side has lost; it stops once its frames so far
            // average that over all PROBE_FRAMES (the rest could not bring it back), and reports
            // the average of the frames it rendered.
            let timed = |on: Compositor, lose: Option<f64>| {
                let _warm = self.render(on, t, scale);
                let t0 = std::time::Instant::now();
                let mut done = 0;
                for i in 1..=PROBE_FRAMES {
                    let at = t + i * frame;
                    // Near the end the measured frames are the ones before `t` instead.
                    let at = if at > last { t - i * frame } else { at };
                    let _ = self.render(on, at.clamp(0, last), scale);
                    done = i;
                    if lose.is_some_and(|l| t0.elapsed().as_secs_f64() * 1e3 / PROBE_FRAMES as f64 >= l) {
                        break;
                    }
                }
                t0.elapsed().as_secs_f64() * 1e3 / done.max(1) as f64
            };
            let gpu_ms = timed(Compositor::Gpu, None);
            // The CPU wins only below `gpu_ms * MARGIN` (`pick_compositor`).
            let cpu_ms = timed(Compositor::Cpu, Some(gpu_ms * redrob_effects_render::AutoPick::MARGIN));
            // A GPU that stopped meanwhile rendered its frames on the CPU: it is no choice.
            let gpu_ms = gpu.context().check_health().is_ok().then_some(gpu_ms);
            let c = pick_compositor(gpu_ms, Some(cpu_ms));
            let name = &self.info.name;
            match gpu_ms {
                Some(g) => eprintln!("effects: \"{name}\" renders on the {} (measured {g:.0} ms a frame on the GPU, {cpu_ms:.0} ms on the CPU)", c.label()),
                None => eprintln!("effects: \"{name}\" renders on the CPU (the GPU stopped)"),
            }
            // The cache of the compositor not chosen holds only the measured frames.
            match c {
                Compositor::Gpu => self.cpu_cache.clear(),
                Compositor::Cpu => self.gpu_cache.clear(),
            }
            c
        };
        *self.choice.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Choice::Decided(c);
        c
    }
}

impl MediaSource for CompSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }

    fn video_frame(&self, req: FrameRequest) -> redrob_film_media::Result<Arc<VideoFrame>> {
        let v = self.info.video.as_ref().ok_or(MediaError::NoStream("video"))?;
        // Past the end the last frame holds (a clip trimmed past its media shows the last frame).
        let last = self.info.duration.0.saturating_sub(v.frame_rate.frame_duration().0).max(0);
        let t = req.time.0.clamp(0, last);
        let scale = if req.scale.is_finite() { f64::from(req.scale.clamp(0.1, 1.0)) } else { 1.0 };
        // On the Effects GPU compositor or the CPU one, whichever this composition renders
        // faster on (see `compositor`). A scene over 1080×1920 footage spent 178 ms a frame on
        // the CPU, 74 of them in the hero title's Glow; footage alone renders faster on the CPU.
        // The layer cache keeps still layers (washes, settled titles) between frames, which also
        // spares their upload to the GPU.
        let on = self.compositor(t, scale, v.frame_rate.frame_duration().0, last);
        let (w, h, data) = self.render(on, t, scale);
        let expected = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4));
        if w == 0 || h == 0 || expected != Some(data.len()) {
            return Err(MediaError::Decode(format!("composition frame has an inconsistent size ({w}×{h}, {} bytes)", data.len())));
        }
        Ok(Arc::new(VideoFrame::rgba8(w, h, data).with_pts(req.time)))
    }

    fn audio(&self, _start: i64, _frames: usize, _sample_rate: u32) -> redrob_film_media::Result<AudioBuffer> {
        Err(MediaError::NoStream("audio"))
    }
}

// ----------------------------------------------------------------------------------------------
// Timeline → composition
// ----------------------------------------------------------------------------------------------

/// One clip to send: its timing and the media file behind it.
struct Outgoing {
    name: String,
    path: String,
    geom: ClipGeom,
    framing: Framing,
}

/// A clip's static Motion, in Edit's units (sequence pixels, source pixels for the anchor, percent,
/// degrees); `None` where the clip has no value of its own (automatic position or anchor).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Framing {
    pub scale: Option<f64>,
    pub position: Option<(f64, f64)>,
    pub anchor: Option<(f64, f64)>,
    pub rotation: Option<f64>,
}

impl Framing {
    /// The static Motion of `clip` (an animated parameter is left to the layer's default: its
    /// keyframes do not carry over).
    pub fn of(clip: &redrob_film_project::TrackItem) -> Self {
        let Some(m) = clip.effect("motion") else { return Self::default() };
        let still = |k: &str| m.param(k).filter(|p| p.keyframes.is_empty()).map(|p| p.value.clone());
        let point = |k: &str| still(k).and_then(|v| v.as_vec2()).filter(|v| v.x.is_finite() && v.y.is_finite()).map(|v| (v.x, v.y));
        Self {
            scale: still("scale").and_then(|v| v.as_f64()).filter(|v| v.is_finite()),
            position: point("position"),
            anchor: point("anchor"),
            rotation: still("rotation").and_then(|v| v.as_f64()).filter(|v| v.is_finite()),
        }
    }

    /// The layer property writes (`prop.set` path and value) that give a layer this framing.
    pub fn layer_props(&self) -> Vec<(&'static str, Value)> {
        let mut out = Vec::new();
        if let Some(a) = self.anchor {
            out.push(("transform/anchor", json!([a.0, a.1, 0.0])));
        }
        if let Some(p) = self.position {
            out.push(("transform/position", json!([p.0, p.1, 0.0])));
        }
        if let Some(sc) = self.scale.filter(|v| (*v - 100.0).abs() > 1e-9) {
            out.push(("transform/scale", json!([sc, sc, 100.0])));
        }
        if let Some(r) = self.rotation.filter(|v| v.abs() > 1e-9) {
            out.push(("transform/rotation", json!(r)));
        }
        out
    }
}

/// The video clips to send from the Edit editor's active sequence: the selection, else the in/out
/// range, else the whole sequence. `skipped` counts clips without a media file behind them
/// (generators, graphics, nested sequences, offline media).
fn outgoing_clips(session: &redrob_film_engine::Session) -> Result<(Vec<Outgoing>, usize), String> {
    let seq = session.active_sequence().ok_or("open a sequence first")?;
    let project = &session.project;
    let selected: Vec<redrob_film_project::ClipId> = session.state.selection.clone();
    let range = match (seq.mark_in, seq.mark_out) {
        (Some(i), Some(o)) if o > i => Some((i, o)),
        _ => None,
    };
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for track in &seq.video_tracks {
        for clip in &track.items {
            let wanted = if !selected.is_empty() {
                selected.contains(&clip.id)
            } else if let Some((i, o)) = range {
                clip.end() > i && clip.start < o
            } else {
                true
            };
            if !wanted || !clip.enabled {
                continue;
            }
            let path = match project.resolve_media(clip.item) {
                Some((_, m, _)) if !m.offline => match &m.media {
                    MediaRef::File { path } if parse_comp_path(path).is_none() => Some(path.clone()),
                    _ => None,
                },
                _ => None,
            };
            let Some(path) = path else {
                skipped = skipped.saturating_add(1);
                continue;
            };
            let speed = if clip.reverse { -clip.speed.abs() } else { clip.speed };
            out.push(Outgoing {
                name: clip.name.clone(),
                path,
                geom: ClipGeom { start: clip.start.0, duration: clip.duration.0, source_in: clip.source_in.0, speed },
                framing: Framing::of(clip),
            });
        }
    }
    Ok((out, skipped))
}

/// What `timeline_to_composition` would send right now (for the Mode menu's enablement).
pub fn can_send_selection(session: &redrob_film_engine::Session) -> bool {
    outgoing_clips(session).is_ok_and(|(clips, _)| !clips.is_empty())
}

/// Send the Edit selection (or in/out range, or sequence) to Effects as a new composition and
/// open it there. Returns the status line.
pub fn timeline_to_composition(edit: &redrob_film_engine::Session, session: &mut redrob_effects_engine::Session, params: &Value) -> Result<String, String> {
    let (clips, skipped) = outgoing_clips(edit)?;
    if clips.is_empty() {
        return Err(if skipped > 0 {
            format!("nothing to send: {skipped} clip(s) have no media file behind them (generators, graphics and nested sequences stay in Edit)")
        } else {
            "nothing to send: select clips or set an in/out range".into()
        });
    }
    let (seq_name, settings) = {
        let s = edit;
        let id = s.state.active_sequence.ok_or("open a sequence first")?;
        let item = s.project.item(id).ok_or("the active sequence is gone")?;
        let seq = s.project.sequence(id).ok_or("the active sequence is gone")?;
        (item.name.clone(), seq.settings.clone())
    };
    let geoms: Vec<ClipGeom> = clips.iter().map(|c| c.geom).collect();
    let (comp_start, comp_end) = comp_range(&geoms).ok_or("nothing to send")?;
    let duration = comp_end.saturating_sub(comp_start);
    if duration <= 0 {
        return Err("the clips to send have no duration".into());
    }
    let wanted_name = params.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty());
    let name = wanted_name.map(str::to_string).unwrap_or_else(|| format!("{seq_name} (Effects)"));
    let rate = redrob_effects_time::FrameRate::new(settings.frame_rate.num, settings.frame_rate.den);

    // editors/effects/crates/engine/src/commands/comp.rs L99: `comp.new` returns {"comp": id}.
    let created = session
        .execute(
            "comp.new",
            json!({
                "name": name.as_str(),
                "width": settings.width,
                "height": settings.height,
                "frameRate": rate.as_f64(),
                "duration": redrob_effects_time::Tick(duration).seconds(),
                "open": false,
            }),
        )
        .map_err(|e| e.to_string())?;
    let cid = created.get("comp").and_then(Value::as_u64).map(redrob_effects_project::ItemId).ok_or("comp.new returned no composition id")?;
    // The exact rate and length (the command takes seconds and an fps float).
    session
        .edit("Composition from Timeline", None, |proj, _| {
            let c = proj.comp_mut(cid).ok_or(redrob_effects_engine::EngineError::NoComp)?;
            c.frame_rate = rate;
            c.duration = redrob_effects_time::Tick(duration);
            c.work_area = (redrob_effects_time::Tick(0), redrob_effects_time::Tick(duration));
            Ok(())
        })
        .map_err(|e| e.to_string())?;

    // open first: the layer property writes below act on the open composition
    session.execute("comp.open", json!({"comp": cid.0})).map_err(|e| e.to_string())?;
    let mut imported: HashMap<String, redrob_effects_project::ItemId> = HashMap::new();
    let mut placed = 0usize;
    let mut problems: Vec<String> = Vec::new();
    for clip in &clips {
        let geom = match layer_geometry(&clip.geom, comp_start) {
            Ok(g) => g,
            Err(e) => {
                problems.push(format!("{}: {e}", clip.name));
                continue;
            }
        };
        let fid = match imported.get(&clip.path) {
            Some(id) => *id,
            None => {
                // editors/effects/crates/engine/src/commands/file.rs L234 / L500: {"items": [ids], "errors": [..]}
                let r = session.execute("file.import", json!({"paths": [clip.path.as_str()], "addToComp": false})).map_err(|e| e.to_string())?;
                let Some(id) = r.get("items").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_u64).map(redrob_effects_project::ItemId)
                else {
                    let why = r.get("errors").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str).unwrap_or("not importable");
                    problems.push(format!("{}: {why}", clip.name));
                    continue;
                };
                imported.insert(clip.path.clone(), id);
                id
            }
        };
        // editors/effects/crates/engine/src/commands/layer.rs L253: `layer.addItem` -> {"layer": id}
        let r = session.execute("layer.addItem", json!({"comp": cid.0, "item": fid.0, "time": 0.0})).map_err(|e| e.to_string())?;
        let Some(lid) = r.get("layer").and_then(Value::as_u64).map(redrob_effects_project::LayerId) else {
            problems.push(format!("{}: no layer was added", clip.name));
            continue;
        };
        let layer_name = clip.name.clone();
        session
            .edit("Place Clip", None, |proj, _| {
                let c = proj.comp_mut(cid).ok_or(redrob_effects_engine::EngineError::NoComp)?;
                let l = c.layers.iter_mut().find(|l| l.id == lid).ok_or_else(|| redrob_effects_engine::EngineError::Other("the new layer is gone".into()))?;
                l.name = layer_name;
                l.start_time = redrob_effects_time::Tick(geom.start_time);
                l.in_point = redrob_effects_time::Tick(geom.in_point);
                l.out_point = redrob_effects_time::Tick(geom.out_point);
                l.stretch = geom.stretch;
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        // the clip's framing (a Fill Frame, a reframe) on the layer
        for (path, value) in clip.framing.layer_props() {
            if let Err(e) = session.execute("prop.set", json!({"layer": lid.0, "path": path, "value": value})) {
                problems.push(format!("{}: {path} not kept ({e})", clip.name));
            }
        }
        placed = placed.saturating_add(1);
    }
    session.execute("comp.open", json!({"comp": cid.0})).map_err(|e| e.to_string())?;
    let mut line = format!("Sent {placed} clip(s) to Effects as \"{name}\"");
    if skipped > 0 {
        line.push_str(&format!("; {skipped} without a media file left out"));
    }
    if !problems.is_empty() {
        line.push_str(&format!("; {} not placed: {}", problems.len(), problems.join(", ")));
    }
    Ok(line)
}

// ----------------------------------------------------------------------------------------------
// Composition → timeline
// ----------------------------------------------------------------------------------------------

/// Whether `composition_to_timeline` has something to do: an active composition and a sequence.
pub fn can_place_composition(edit: &redrob_film_engine::Session, effects: Option<&redrob_effects_engine::Session>) -> bool {
    effects.is_some_and(|e| e.active_comp_id().is_some()) && edit.active_sequence().is_some()
}

/// The placeholder media clip for a composition.
fn placeholder_clip(info: MediaInfo, comp: redrob_effects_project::ItemId) -> MediaClip {
    MediaClip {
        media: MediaRef::File { path: comp_path(comp) },
        info,
        interpret: Default::default(),
        mark_in: None,
        mark_out: None,
        markers: Vec::new(),
        offline: false,
        proxy: None,
        identity: None,
    }
}

/// Place the active composition on the Edit timeline at the playhead (a picture-only clip rendered
/// live by the effects renderer). `params`: `track` ("V1" | id) and `insert`
/// as `timeline.place` takes them (crates/engine/src/commands.rs L2183). Returns the status line.
pub fn composition_to_timeline(edit: &mut redrob_film_engine::Session, effects: &redrob_effects_engine::Session, params: &Value) -> Result<String, String> {
    let cid = effects.active_comp_id().ok_or("no active composition in Effects")?;
    let project = effects.project.clone();
    let footage = effects.footage.clone();
    let name = project.item(cid).map(|i| i.name.clone()).ok_or("the active composition is gone")?;
    let comp = project.comp(cid).ok_or("the active composition is gone")?;
    let info = comp_media_info(&name, comp)?;
    if edit.active_sequence().is_none() {
        return Err("open a sequence in Edit first".into());
    }
    let source = Arc::new(CompSource::new(&name, project, cid, footage)?);
    // One placeholder per composition: placing the same comp again reuses (and refreshes) it.
    let placeholder = MediaRef::File { path: comp_path(cid) };
    let existing = edit.project.items.values().find(|it| matches!(&it.kind, ItemKind::Media(m) if m.media == placeholder)).map(|it| it.id);
    let item = match existing {
        Some(id) => {
            let fresh = info.clone();
            edit.edit("Refresh Composition", |proj, _| {
                if let Some(ItemKind::Media(m)) = proj.item_mut(id).map(|it| &mut it.kind) {
                    m.info = fresh;
                }
                Ok(id)
            })
            .map_err(|e| e.to_string())?
        }
        None => {
            let clip = placeholder_clip(info, cid);
            let label = name.clone();
            edit.edit("Place Composition", |proj, _| Ok(proj.add_item(&label, Label::Iris, ItemKind::Media(clip), None))).map_err(|e| e.to_string())?
        }
    };
    edit.media.insert(item, source);
    let mut place = json!({"item": item.0, "time": edit.playhead().0, "audio": false});
    if let Some(track) = params.get("track") {
        place["track"] = track.clone();
    }
    if let Some(insert) = params.get("insert").and_then(Value::as_bool) {
        place["insert"] = json!(insert);
    }
    let r = edit.execute("timeline.place", place).map_err(|e| e.to_string())?;
    let n = r.get("clips").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
    Ok(format!("Placed \"{name}\" on the timeline ({n} clip(s); the picture renders live from Effects)"))
}

/// After a project opens: give every placeholder item (`effects://comp/<id>`) its live source
/// again, from the effects project of the same bundle. Items whose composition is gone stay as
/// they are (the engine shows them offline). Returns how many were bound.
pub fn rebind_composition_sources(edit: &redrob_film_engine::Session, effects: &redrob_effects_engine::Session) -> usize {
    let mut bound = 0usize;
    for it in edit.project.items.values() {
        let ItemKind::Media(m) = &it.kind else { continue };
        let MediaRef::File { path } = &m.media else { continue };
        let Some(cid) = parse_comp_path(path) else { continue };
        match CompSource::new(&it.name, effects.project.clone(), cid, effects.footage.clone()) {
            Ok(src) => {
                edit.media.insert(it.id, Arc::new(src));
                bound = bound.saturating_add(1);
            }
            Err(e) => eprintln!("composition clip \"{}\" not bound: {e}", it.name),
        }
    }
    bound
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const SEC: i64 = redrob_film_time::TICKS_PER_SECOND;

    /// A 3-clip sequence: A at 0 s for 2 s (from media 1 s), B at 2 s for 1 s at 200 %, C at 4 s
    /// for 2 s after a 1 s gap. The composition starts with A and ends with C.
    fn three_clips() -> [ClipGeom; 3] {
        [
            ClipGeom { start: 0, duration: 2 * SEC, source_in: SEC, speed: 1.0 },
            ClipGeom { start: 2 * SEC, duration: SEC, source_in: 0, speed: 2.0 },
            ClipGeom { start: 4 * SEC, duration: 2 * SEC, source_in: 3 * SEC, speed: 1.0 },
        ]
    }

    #[test]
    fn the_composition_spans_the_first_start_to_the_last_end() {
        let clips = three_clips();
        assert_eq!(comp_range(&clips), Some((0, 6 * SEC)));
        // a selection that does not start at 0 moves the comp's origin
        let later = &clips[1..];
        assert_eq!(comp_range(later), Some((2 * SEC, 6 * SEC)));
        assert_eq!(comp_range(&[]), None);
    }

    #[test]
    fn clip_timing_maps_to_layer_timing_on_a_three_clip_sequence() {
        let clips = three_clips();
        let (comp_start, _) = comp_range(&clips).unwrap();
        // A: in 0..2 s, media starts 1 s in, so layer time 0 sits 1 s before the in point.
        let a = layer_geometry(&clips[0], comp_start).unwrap();
        assert_eq!((a.in_point, a.out_point), (0, 2 * SEC));
        assert_eq!(a.start_time, -SEC);
        assert_eq!(a.stretch, 100.0);
        // B: 200 % = 50 % stretch; media 0 at the in point.
        let b = layer_geometry(&clips[1], comp_start).unwrap();
        assert_eq!((b.in_point, b.out_point), (2 * SEC, 3 * SEC));
        assert_eq!(b.start_time, 2 * SEC);
        assert_eq!(b.stretch, 50.0);
        // C: after the gap; media 3 s in.
        let c = layer_geometry(&clips[2], comp_start).unwrap();
        assert_eq!((c.in_point, c.out_point), (4 * SEC, 6 * SEC));
        assert_eq!(c.start_time, SEC);
        // Every layer shows the clip's first media frame at its in point (Layer::layer_time).
        for (clip, layer) in clips.iter().zip([a, b, c]) {
            let layer_time = ((layer.in_point - layer.start_time) as f64 * 100.0 / layer.stretch).round() as i64;
            assert_eq!(layer_time, clip.source_in, "{clip:?}");
            // ...and the last frame at the out point.
            let last = ((layer.out_point - layer.start_time) as f64 * 100.0 / layer.stretch).round() as i64;
            let media_end = clip.source_in + (clip.duration as f64 * clip.speed).round() as i64;
            assert_eq!(last, media_end, "{clip:?}");
        }
    }

    #[test]
    fn a_later_origin_shifts_every_layer() {
        let clips = three_clips();
        let later = &clips[1..];
        let (comp_start, _) = comp_range(later).unwrap();
        let b = layer_geometry(&later[0], comp_start).unwrap();
        assert_eq!((b.in_point, b.out_point), (0, SEC));
        let c = layer_geometry(&later[1], comp_start).unwrap();
        assert_eq!((c.in_point, c.out_point), (2 * SEC, 4 * SEC));
    }

    #[test]
    fn reverse_frozen_and_empty_clips_are_refused() {
        let base = ClipGeom { start: 0, duration: SEC, source_in: 0, speed: 1.0 };
        assert!(layer_geometry(&ClipGeom { speed: -1.0, ..base }, 0).is_err());
        assert!(layer_geometry(&ClipGeom { speed: 0.0, ..base }, 0).is_err());
        assert!(layer_geometry(&ClipGeom { speed: f64::NAN, ..base }, 0).is_err());
        assert!(layer_geometry(&ClipGeom { duration: 0, ..base }, 0).is_err());
        // hostile extremes do not panic
        let huge = ClipGeom { start: i64::MAX, duration: i64::MAX, source_in: i64::MAX, speed: 1e-300 };
        let _ = layer_geometry(&huge, i64::MIN);
    }

    #[test]
    fn a_composition_describes_itself_as_square_pixel_rgba_movie_without_audio() {
        let rate = redrob_effects_time::FrameRate::FPS_23_976;
        let comp = redrob_effects_project::Comp::new(1920, 1080, rate, redrob_effects_time::Tick(10 * SEC));
        let info = comp_media_info("Title", &comp).unwrap();
        assert_eq!(info.name, "Title");
        assert_eq!(info.kind, MediaKind::Movie);
        assert_eq!(info.duration, Tick(10 * SEC));
        let v = info.video.as_ref().unwrap();
        assert_eq!((v.width, v.height), (1920, 1080));
        assert_eq!((v.frame_rate.num, v.frame_rate.den), (24000, 1001));
        assert_eq!(v.par, (1, 1));
        assert!(v.has_alpha);
        assert_eq!(v.color, ColorInfo::SRGB_FULL);
        assert!(info.audio_streams.is_empty());
        assert!(!info.has_audio());
        // degenerate compositions are refused, not panicked on
        let empty = redrob_effects_project::Comp::new(0, 1080, rate, redrob_effects_time::Tick(SEC));
        assert!(comp_media_info("x", &empty).is_err());
        let zero_len = redrob_effects_project::Comp::new(16, 16, rate, redrob_effects_time::Tick(0));
        assert!(comp_media_info("x", &zero_len).is_err());
    }

    #[test]
    fn the_cpu_renders_a_composition_only_when_clearly_faster() {
        assert_eq!(pick_compositor(Some(150.0), Some(40.0)), Compositor::Cpu);
        assert_eq!(pick_compositor(Some(250.0), Some(520.0)), Compositor::Gpu);
        // Ties and noise stay on the GPU.
        assert_eq!(pick_compositor(Some(100.0), Some(90.0)), Compositor::Gpu);
        // A GPU that stopped is no choice; nothing measured stays on the GPU.
        assert_eq!(pick_compositor(None, Some(500.0)), Compositor::Cpu);
        assert_eq!(pick_compositor(None, None), Compositor::Gpu);
        assert_eq!(pick_compositor(Some(f64::NAN), Some(1.0)), Compositor::Gpu);
    }

    #[test]
    fn a_composition_source_renders_frames_of_its_size_and_clamps_time() {
        let rate = redrob_effects_time::FrameRate::FPS_30;
        let mut project = redrob_effects_project::Project::default();
        let comp = redrob_effects_project::Comp::new(64, 32, rate, redrob_effects_time::Tick(2 * SEC));
        let cid = project.add_item("C", redrob_effects_color::Label::Sandstone, None, redrob_effects_project::ItemKind::Comp(comp.into()));
        let src = CompSource::new("C", Arc::new(project), cid, Arc::new(redrob_effects_render::NoFootage)).unwrap();
        let f = src.video_frame(FrameRequest::full(Tick(SEC))).unwrap();
        assert_eq!((f.width, f.height), (64, 32));
        assert_eq!(f.pts, Tick(SEC));
        assert!(matches!(f.data, redrob_film_frame::PixelData::Rgba8(_)));
        // half scale renders half size; a time past the end and a negative one are clamped
        let half = src.video_frame(FrameRequest { time: Tick(99 * SEC), scale: 0.5 }).unwrap();
        assert_eq!((half.width, half.height), (32, 16));
        assert!(src.video_frame(FrameRequest { time: Tick(-SEC), scale: f32::NAN }).is_ok());
        assert!(matches!(src.audio(0, 10, 48_000), Err(MediaError::NoStream("audio"))));
        assert!(src.info().has_video());
    }

    #[test]
    fn placeholder_paths_round_trip_and_reject_files() {
        let id = redrob_effects_project::ItemId(42);
        assert_eq!(parse_comp_path(&comp_path(id)), Some(id));
        assert_eq!(parse_comp_path("/media/a.mov"), None);
        assert_eq!(parse_comp_path("effects://comp/x"), None);
        let comp = redrob_effects_project::Comp::new(8, 8, redrob_effects_time::FrameRate::FPS_25, redrob_effects_time::Tick(SEC));
        let clip = placeholder_clip(comp_media_info("C", &comp).unwrap(), id);
        assert!(!clip.offline);
        assert_eq!(clip.media, MediaRef::File { path: "effects://comp/42".into() });
    }
}
