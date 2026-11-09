//! The MCP server. Tool names are snake_case.

use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine as _;
use redrob_film_engine::Session;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock as Content};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AutomationError, BridgeClient, png_rgba};

pub enum Backend {
    Headless(Arc<Mutex<Session>>),
    Bridge(Arc<BridgeClient>),
}

/// The mode commands on the headless backend, as the app's shell answers them (apps/redrob-film-app/
/// src/shell.rs `service_control`): Effects commands, the two hand-offs, the mode, and `command.list`
/// with the mode commands appended. The Effects session starts on first use.
fn mode_command(
    edit: &mut Session,
    effects: &mut Option<redrob_effects_engine::Session>,
    mode: &mut &'static str,
    id: &str,
    params: Value,
) -> Result<Value, AutomationError> {
    use redrob_film_handoff as h;
    let other = AutomationError::Other;
    match id {
        "command.list" => {
            let list = edit.execute("command.list", params).map_err(AutomationError::from)?;
            let mut all = list.as_array().cloned().unwrap_or_default();
            all.extend(h::catalogue(h::can_send_selection(edit), h::can_place_composition(edit, effects.as_ref())));
            Ok(Value::Array(all))
        }
        h::SWITCH_MODE => {
            let want = params.get("mode").and_then(Value::as_str).unwrap_or("").trim().to_ascii_lowercase();
            *mode = match want.as_str() {
                "edit" | "editor" | "timeline" => "edit",
                "effects" | "effect" | "fx" | "composition" => {
                    effects.get_or_insert_with(redrob_effects_host::session);
                    "effects"
                }
                _ => return Err(other("mode: \"edit\" or \"effects\"".into())),
            };
            Ok(json!({"mode": *mode}))
        }
        h::EFFECTS_RUN => {
            let cmd = params
                .get("id")
                .or(params.get("command"))
                .and_then(Value::as_str)
                .ok_or_else(|| other("need `id` (an Effects command id)".into()))?
                .to_string();
            let p = params.get("params").cloned().unwrap_or(json!({}));
            let session = effects.get_or_insert_with(redrob_effects_host::session);
            session.execute(&cmd, p).map_err(|e| other(e.to_string()))
        }
        h::SEND_SELECTION_TO_EFFECTS => {
            let session = effects.get_or_insert_with(redrob_effects_host::session);
            let line = h::timeline_to_composition(edit, session, &params).map_err(other)?;
            *mode = "effects";
            Ok(json!({"ok": true, "message": line, "mode": "effects", "comp": session.active_comp_id().map(|c| c.0)}))
        }
        h::PLACE_COMPOSITION_ON_TIMELINE => {
            let session = effects.as_ref().ok_or_else(|| other("no composition: build one with effects.run first".into()))?;
            let line = h::composition_to_timeline(edit, session, &params).map_err(other)?;
            *mode = "edit";
            Ok(json!({"ok": true, "message": line, "mode": "edit"}))
        }
        h::STATUS => Ok(json!({
            "mode": *mode,
            "editors": if effects.is_some() { json!(["edit", "effects"]) } else { json!(["edit"]) },
            "activeComposition": effects.as_ref().and_then(|e| e.active_comp_id()).map(|c| c.0),
            "canSendSelection": h::can_send_selection(edit),
            "canPlaceComposition": h::can_place_composition(edit, effects.as_ref()),
        })),
        _ => Err(other(format!("unknown mode command {id}"))),
    }
}

/// `scene_build` on the headless backend: make the composition (from the Edit selection, or empty),
/// run [`crate::scene::plan`]'s batch in it as one undo step, and place it on the timeline when asked.
fn build_scene(edit: &mut Session, effects: &mut Option<redrob_effects_engine::Session>, spec: &Value) -> Result<Value, AutomationError> {
    use redrob_film_handoff as h;
    let other = AutomationError::Other;
    let session = effects.get_or_insert_with(redrob_effects_host::session);
    let name = spec.get("name").and_then(Value::as_str).unwrap_or("Scene").to_string();
    let from_selection = spec.get("footage").and_then(Value::as_str) == Some("selection");
    // where the footage sat on the timeline, to put the scene back over it
    let mut origin: Option<(i64, Value)> = None;
    let mut frame = crate::scene::Frame::of(spec);
    if from_selection {
        let seq = edit.active_sequence().ok_or_else(|| other("open a sequence and select the clips to build on".into()))?;
        let sel = edit.state.selection.clone();
        for (ti, track) in seq.video_tracks.iter().enumerate() {
            for c in track.items.iter().filter(|c| sel.contains(&c.id)) {
                if origin.as_ref().is_none_or(|(t, _)| c.start.0 < *t) {
                    origin = Some((c.start.0, json!(format!("V{}", ti + 1))));
                }
            }
        }
        if origin.is_none() {
            return Err(other("footage \"selection\": select the clips to build on first (timeline.select)".into()));
        }
        h::timeline_to_composition(edit, session, &json!({"name": name})).map_err(other)?;
        let info = session.execute("comp.info", json!({})).map_err(|e| other(e.to_string()))?;
        frame.width = info["width"].as_u64().unwrap_or(1080) as u32;
        frame.height = info["height"].as_u64().unwrap_or(1920) as u32;
        frame.fps = info["frameRate"].as_f64().unwrap_or(30.0);
        frame.duration = info["duration"].as_f64().unwrap_or(frame.duration);
    } else {
        let mut bg = json!({"name": name, "width": frame.width, "height": frame.height, "frameRate": frame.fps, "duration": frame.duration, "open": true});
        if let Some(c) = spec.get("background").and_then(Value::as_str) {
            bg["background"] = json!(c);
        }
        session.execute("comp.new", bg).map_err(|e| other(e.to_string()))?;
    }
    // the footage layers and their current scale (the framing the hand-off kept)
    let info = session.execute("comp.info", json!({})).map_err(|e| other(e.to_string()))?;
    let mut footage = Vec::new();
    for l in info["layers"].as_array().cloned().unwrap_or_default() {
        if l["type"] != "Footage" {
            continue;
        }
        let Some(id) = l["id"].as_u64() else { continue };
        let scale = session
            .execute("prop.get", json!({"layer": id, "path": "transform/scale"}))
            .ok()
            .and_then(|v| v.get("value").cloned().or(Some(v)))
            .and_then(|v| v.get(0).and_then(Value::as_f64))
            .unwrap_or(100.0);
        footage.push((id, scale));
    }
    let (steps, cues) = crate::scene::plan(spec, &frame, &footage).map_err(AutomationError::BadRequest)?;
    let n = steps.len();
    session
        .execute("engine.batch", json!({"steps": steps, "label": format!("Build Scene {name}"), "atomic": true}))
        .map_err(|e| other(format!("scene_build: {e}")))?;
    let comp = session.active_comp_id().map(|c| c.0);
    let mut out = json!({"comp": comp, "name": name, "steps": n, "width": frame.width, "height": frame.height, "duration": frame.duration, "footageLayers": footage.len()});
    match spec.get("place") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {
            out["cues"] = json!(cues.iter().map(|c| json!({"kind": c.kind, "compSeconds": c.seconds})).collect::<Vec<_>>());
            out["next"] = json!("not placed: run mode.placeCompositionOnTimeline, or scene_build with place: true");
        }
        Some(p) => {
            let (at, track) = match (p.get("seconds").and_then(Value::as_f64), &origin) {
                (Some(sec), _) => (redrob_film_time::Tick::from_seconds_f64(sec).0, p.get("track").cloned().unwrap_or(json!("V1"))),
                (None, Some((t, tr))) => (*t, p.get("track").cloned().unwrap_or(tr.clone())),
                (None, None) => (edit.playhead().0, p.get("track").cloned().unwrap_or(json!("V1"))),
            };
            edit.execute("playhead.set", json!({"time": at})).map_err(AutomationError::from)?;
            let line = h::composition_to_timeline(edit, session, &json!({"track": track})).map_err(other)?;
            out["placed"] = json!(line);
            let base = redrob_film_time::Tick(at).seconds();
            out["cues"] = json!(cues.iter().map(|c| json!({"kind": c.kind, "seconds": base + c.seconds})).collect::<Vec<_>>());
            out["next"] = json!("render_contact_sheet over the scene's span to check it; sfx_add with `cues` for sound");
        }
    }
    Ok(out)
}

/// A panic payload as text.
fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into())
}

/// A failed blocking task: a panic is reported as an internal error and the session stays usable.
fn join_error(e: tokio::task::JoinError) -> AutomationError {
    match e.try_into_panic() {
        Ok(p) => AutomationError::Other(format!("internal error: {}", panic_message(p))),
        Err(e) => AutomationError::Other(e.to_string()),
    }
}

#[derive(Clone)]
pub struct FilmMcp {
    backend: Arc<Backend>,
    /// Headless only: the Effects editor's session, created on the first mode command that needs
    /// it, and the mode an agent last switched to (there is no window to show it in).
    effects: Arc<Mutex<Option<redrob_effects_engine::Session>>>,
    mode: Arc<Mutex<&'static str>>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListParams {
    /// Only commands whose id or label contains this text (case-insensitive).
    #[serde(default)]
    pub filter: Option<String>,
    /// Only commands that can run right now.
    #[serde(default)]
    pub enabled_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunParams {
    /// Command id, e.g. `sequence.addEdit`, `timeline.trim`, `effects.apply`, `file.import`.
    pub id: String,
    /// Parameters object (see `params` in `command_list`).
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BatchStep {
    /// Command id.
    pub id: String,
    /// Parameters object.
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BatchParams {
    /// Commands to run in order.
    pub steps: Vec<BatchStep>,
    /// Stop at the first failing step (default true).
    #[serde(default)]
    pub stop_on_error: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RenderParams {
    /// Timeline time in seconds (default: the playhead).
    #[serde(default)]
    pub seconds: Option<f64>,
    /// Longest side of the PNG (default 960).
    #[serde(default)]
    pub max_side: Option<u32>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ShotParams {
    /// Panel to crop to (e.g. `Timeline`, `Program`); omit for the whole window.
    #[serde(default)]
    pub panel: Option<String>,
    /// Longest side of the PNG (default 1600).
    #[serde(default)]
    pub max_side: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClickParams {
    /// Element id from `ui_elements` (e.g. `timeline.track.V1.lock`, `tools.Razor`, `project.item.12`).
    #[serde(default)]
    pub id: Option<String>,
    /// Or screen coordinates in points.
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    /// left | right | middle
    #[serde(default)]
    pub button: Option<String>,
    /// 2 for double-click.
    #[serde(default)]
    pub count: Option<u32>,
    /// {"shift":bool,"alt":bool,"command":bool,"ctrl":bool}
    #[serde(default)]
    pub modifiers: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DragParams {
    /// Start: {"id":..} or {"x":..,"y":..} (optionally "fx"/"fy" fractions inside the element).
    pub from: Value,
    /// End point, same forms.
    pub to: Value,
    #[serde(default)]
    pub steps: Option<u32>,
    #[serde(default)]
    pub modifiers: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeyParams {
    /// Key with optional modifiers, e.g. `Space`, `Cmd+K`, `Shift+Delete`, `I`, `L`.
    pub key: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TypeParams {
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MediaImportParams {
    /// Absolute paths to media files (MP4/MOV, WAV/MP3/FLAC/AIFF/Ogg, PNG/JPEG/…).
    pub paths: Vec<String>,
    /// Bin ID to import into (optional).
    #[serde(default)]
    pub bin: Option<u64>,
    /// Import numbered stills as an image sequence (optional, default false).
    #[serde(default)]
    pub image_sequence: Option<bool>,
}

/// `file.import` params for a `media_import` call.
fn media_import_params(p: MediaImportParams) -> Value {
    let mut params = json!({"paths": p.paths});
    if let Some(bin) = p.bin {
        params["bin"] = json!(bin);
    }
    if let Some(seq) = p.image_sequence {
        params["imageSequence"] = json!(seq);
    }
    params
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ElementsParams {
    /// Only element ids starting with this prefix (e.g. `timeline.clip.`).
    #[serde(default)]
    pub prefix: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize, JsonSchema)]
pub struct SceneParams {
    /// Composition name.
    #[serde(default)]
    pub name: Option<String>,
    /// "selection": the selected Edit clips become the background footage (their framing kept);
    /// omit for a graphics-only scene.
    #[serde(default)]
    pub footage: Option<String>,
    /// Without footage: "#hex" or ["#top", "#bottom"] for a vertical gradient (default near-black).
    #[serde(default)]
    pub background: Option<Value>,
    /// Frame size and timing without footage (defaults 1080x1920, 30 fps, 4 s); footage scenes take
    /// the selection's.
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub fps: Option<f64>,
    #[serde(default)]
    pub duration: Option<f64>,
    /// Colours and font: {"text": "#ffffff", "hero": "#ffc46b", "font": "Inter"}.
    #[serde(default)]
    pub theme: Option<Value>,
    /// The finish: {"darken": 0..0.9 (wash over footage, default 0.28), "vignette": 0..100 (45),
    /// "grain": 0..100 (4), "kenBurns": push-in % over the scene (6)}.
    #[serde(default)]
    pub look: Option<Value>,
    /// Bottom to top. Each: {"type": "text"|"bar"|"box", "text", "role": "hero"|"title"|"body"|
    /// "label"|"cta", "y"/"x": 0..1 of the frame (y kept in the 9:16 safe band 0.14-0.65), "size",
    /// "color", "in"/"out" (s), "enter": "rise"|"drop"|"slide"|"pop"|"fade"|"grow"|"none" or a text
    /// preset ("trackingIn", "fadeUpCharacters", "blurIn", "typewriter", "bounceInWords",
    /// "scramble", "dropInLines"), "exit": "fade"|"fall"|"none", "exitDuration": s (default 0.3), "glow": bool (default: hero only),
    /// "width"/"height" for bar and box}.
    pub layers: Vec<Value>,
    /// Seconds of fade to black at the end of the scene (an end card that breathes out).
    #[serde(default, rename = "fadeOut")]
    pub fade_out: Option<f64>,
    /// Seconds between entrances (default 0.14) and the first entrance (default 0.15).
    #[serde(default)]
    pub stagger: Option<f64>,
    #[serde(default)]
    pub start: Option<f64>,
    /// Put the composition on the timeline: true (for footage scenes: over the clips it came from;
    /// otherwise at the playhead on V1) or {"seconds": f64, "track": "V2"}.
    #[serde(default)]
    pub place: Option<Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SfxCue {
    /// whoosh | hit | riser | shimmer | click | boom | reverse.
    pub kind: String,
    /// Sequence time in seconds where the sound's accent lands (a whoosh's loudest point, a riser's
    /// end, a hit's attack): the moment of the cut or of the title arriving.
    pub seconds: f64,
    /// Gain in dB (default -6; a music bed under it usually wants -4 to -8).
    #[serde(default)]
    pub gain_db: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SfxParams {
    pub cues: Vec<SfxCue>,
    /// Audio tracks to use, e.g. ["A2", "A3"] (default); overlapping sounds go to the next one.
    #[serde(default)]
    pub tracks: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ContactSheetParams {
    /// How many frames, evenly spaced (default 12, 2–30).
    #[serde(default)]
    pub count: Option<u32>,
    /// Longest side of each frame in the sheet (default 240).
    #[serde(default)]
    pub tile: Option<u32>,
    /// First time in seconds (default 0).
    #[serde(default)]
    pub from: Option<f64>,
    /// Last time in seconds (default: the end of the active sequence).
    #[serde(default)]
    pub to: Option<f64>,
}

/// The times of a contact sheet: `count` points from `from` to just before `to`, centred in their
/// slices so the first and last frames are not the black edges of the range.
fn sheet_times(from: f64, to: f64, count: u32) -> Vec<f64> {
    let n = count.clamp(2, 30);
    let span = (to - from).max(0.0);
    (0..n).map(|i| from + span * (i as f64 + 0.5) / n as f64).collect()
}

/// Black bars of a frame as fractions of its height / width: (top, bottom, left, right). A row or
/// column counts as bar while at most 2 % of its pixels are brighter than near-black, so dark scenes
/// with a few lights are not taken for bars; an all-black frame reports no bars.
fn black_bars(f: &image::RgbaImage) -> Option<(f32, f32, f32, f32)> {
    let (w, h) = f.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let lit = |x: u32, y: u32| {
        let p = f.get_pixel(x, y).0;
        u16::from(p[0]) + u16::from(p[1]) + u16::from(p[2]) > 3 * 24
    };
    let row_bar = |y: u32| (0..w).filter(|&x| lit(x, y)).count() as u32 * 50 <= w;
    let col_bar = |x: u32| (0..h).filter(|&y| lit(x, y)).count() as u32 * 50 <= h;
    let top = (0..h).take_while(|&y| row_bar(y)).count() as u32;
    if top == h {
        return None;
    }
    // A letterbox ends on a hard edge; a vignette or a dark scene fades out. Keep a bar only when
    // the picture just inside it is clearly brighter than the bar itself.
    let row_mean = |y: u32| (0..w).map(|x| f.get_pixel(x, y).0[..3].iter().map(|c| u32::from(*c)).sum::<u32>()).sum::<u32>() as f32 / (w * 3) as f32;
    let col_mean = |x: u32| (0..h).map(|y| f.get_pixel(x, y).0[..3].iter().map(|c| u32::from(*c)).sum::<u32>()).sum::<u32>() as f32 / (h * 3) as f32;
    let edge = |bar: u32, inside: f32, at_bar: f32| if bar > 0 && inside > at_bar * 2.0 + 12.0 { bar } else { 0 };
    let top = edge(top, row_mean((top + 2).min(h - 1)), row_mean(top.saturating_sub(1)));
    let bottom = (0..h).rev().take_while(|&y| row_bar(y)).count() as u32;
    let bottom = edge(bottom, row_mean(h.saturating_sub(bottom + 3)), row_mean(h - bottom.max(1)));
    let left = (0..w).take_while(|&x| col_bar(x)).count() as u32;
    let left = edge(left, col_mean((left + 2).min(w - 1)), col_mean(left.saturating_sub(1)));
    let right = (0..w).rev().take_while(|&x| col_bar(x)).count() as u32;
    let right = edge(right, col_mean(w.saturating_sub(right + 3)), col_mean(w - right.max(1)));
    Some((top as f32 / h as f32, bottom as f32 / h as f32, left as f32 / w as f32, right as f32 / w as f32))
}

/// One line on the bars the contact sheet found: the typical (median) bar on each side over the
/// frames that are not all black (`None`), so a fade does not hide letterboxing baked into the
/// footage, and frames without bars count as no bar.
fn bars_note(times: &[f64], bars: &[Option<(f32, f32, f32, f32)>]) -> String {
    let lit: Vec<&(f32, f32, f32, f32)> = bars.iter().flatten().collect();
    let median = |pick: fn(&(f32, f32, f32, f32)) -> f32| {
        let mut v: Vec<f32> = lit.iter().map(|b| pick(b)).collect();
        v.sort_by(f32::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let (t, b, l, r) = (median(|x| x.0), median(|x| x.1), median(|x| x.2), median(|x| x.3));
    if t.max(b).max(l).max(r) < 0.02 {
        return format!("black bars: none (checked {} frames)", times.len());
    }
    let picture_h = (1.0 - t - b).max(0.01);
    let picture_w = (1.0 - l - r).max(0.01);
    format!(
        "black bars (typical over {} frames): top {:.0}%, bottom {:.0}%, left {:.0}%, right {:.0}% - the picture fills only {:.0}% x {:.0}% of the frame; to fill the frame, scale up by at least {:.2}x on top of the current scale",
        lit.len(),
        t * 100.0,
        b * 100.0,
        l * 100.0,
        r * 100.0,
        picture_w * 100.0,
        picture_h * 100.0,
        (1.0 / picture_h).max(1.0 / picture_w)
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ControlParams {
    /// Any control-channel method (e.g. `ui.set`, `ui.scroll`, `ui.timeline.hit`, `ui.playback`).
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

fn ok_json(v: &Value) -> CallToolResult {
    CallToolResult::success(vec![Content::text(serde_json::to_string_pretty(v).unwrap_or_default())])
}
fn fail(e: impl std::fmt::Display) -> CallToolResult {
    CallToolResult::error(vec![Content::text(e.to_string())])
}
fn png_result(png: &[u8], note: &str) -> CallToolResult {
    CallToolResult::success(vec![Content::image(base64::engine::general_purpose::STANDARD.encode(png), "image/png"), Content::text(note.to_string())])
}
fn wrap(r: Result<Value, AutomationError>) -> Result<CallToolResult, McpError> {
    Ok(match r {
        Ok(v) => ok_json(&v),
        Err(e) => fail(e),
    })
}

impl FilmMcp {
    pub fn headless(session: Session) -> Self {
        Self {
            backend: Arc::new(Backend::Headless(Arc::new(Mutex::new(session)))),
            effects: Arc::default(),
            mode: Arc::new(Mutex::new("edit")),
            tool_router: Self::tool_router(),
        }
    }
    pub fn bridge(addr: &str) -> Result<Self, AutomationError> {
        Ok(Self {
            backend: Arc::new(Backend::Bridge(Arc::new(BridgeClient::new(addr)?))),
            effects: Arc::default(),
            mode: Arc::new(Mutex::new("edit")),
            tool_router: Self::tool_router(),
        })
    }

    pub async fn serve_stdio(self) -> Result<(), AutomationError> {
        self.serve_io(tokio::io::stdin(), tokio::io::stdout()).await
    }

    /// Serve MCP over any line stream. A line that isn't JSON gets a JSON-RPC parse error
    /// (`-32700`, id null) instead of being dropped silently; the rest go to the rmcp service. One
    /// writer task owns `output`, so replies never interleave.
    pub async fn serve_io(
        self,
        input: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        output: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
    ) -> Result<(), AutomationError> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (mut to_service, service_in) = tokio::io::duplex(1 << 20);
        let (service_out, from_service) = tokio::io::duplex(1 << 20);
        let (err_tx, mut err_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(input).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(_) => {
                        if to_service.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let reply = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}});
                        let _ = err_tx.send(reply.to_string());
                    }
                }
            }
            // EOF: closing the pipe ends the service
            let _ = to_service.shutdown().await;
        });
        let writer = tokio::spawn(async move {
            let mut output = output;
            let mut replies = BufReader::new(from_service).lines();
            loop {
                let line = tokio::select! {
                    l = replies.next_line() => match l { Ok(Some(l)) => l, _ => break },
                    Some(e) = err_rx.recv() => e,
                };
                if output.write_all(format!("{line}\n").as_bytes()).await.is_err() || output.flush().await.is_err() {
                    break;
                }
            }
        });
        let running = self.serve((service_in, service_out)).await.map_err(|e| AutomationError::Other(format!("MCP init: {e}")))?;
        let r = running.waiting().await.map_err(|e| AutomationError::Other(e.to_string()));
        reader.abort();
        let _ = writer.await;
        r.map(|_| ())
    }

    /// Run an engine command on whichever backend.
    pub async fn run(&self, id: &str, params: Value) -> Result<Value, AutomationError> {
        match &*self.backend {
            Backend::Headless(s) => {
                let (s, fx, mode) = (s.clone(), self.effects.clone(), self.mode.clone());
                let id = id.to_string();
                tokio::task::spawn_blocking(move || {
                    // A command that panicked earlier poisoned the lock; the session is still usable.
                    let mut g = s.lock().unwrap_or_else(PoisonError::into_inner);
                    if redrob_film_handoff::ALL.contains(&id.as_str()) || id == "command.list" {
                        // edit lock first, then effects: every path takes them in this order
                        let mut e = fx.lock().unwrap_or_else(PoisonError::into_inner);
                        return mode_command(&mut g, &mut e, &mut mode.lock().unwrap_or_else(PoisonError::into_inner), &id, params);
                    }
                    g.execute(&id, params).map_err(AutomationError::from)
                })
                .await
                .map_err(join_error)?
            }
            Backend::Bridge(b) => b.execute(id, params).await,
        }
    }

    /// The project tree plus the active sequence (null when there is none).
    async fn document(&self) -> Result<Value, AutomationError> {
        let project = self.run("project.inspect", json!({})).await?;
        let sequence = self.run("sequence.inspect", json!({})).await.ok();
        Ok(json!({"project": project, "sequence": sequence}))
    }

    fn bridge_client(&self) -> Option<Arc<BridgeClient>> {
        match &*self.backend {
            Backend::Bridge(b) => Some(b.clone()),
            Backend::Headless(_) => None,
        }
    }

    async fn ui(&self, method: &str, params: Value) -> Result<CallToolResult, McpError> {
        match self.bridge_client() {
            Some(b) => wrap(b.call(method, params).await),
            None => Ok(fail(
                "this tool drives the live app: run `redrob-film-cli mcp --bridge 127.0.0.1:<port>` with the app started as `redrob-film --control <port>`",
            )),
        }
    }
}

#[tool_router]
impl FilmMcp {
    #[tool(
        title = "List commands",
        description = "List every command (id, label, menu path, shortcut, parameter doc, enabled now). Menus, shortcuts, panels and timeline gestures all map to these ids.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn command_list(&self, Parameters(p): Parameters<ListParams>) -> Result<CallToolResult, McpError> {
        let r = self.run("command.list", json!({})).await.map(|v| {
            let f = p.filter.unwrap_or_default().to_ascii_lowercase();
            let only = p.enabled_only.unwrap_or(false);
            Value::Array(
                v.as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|c| {
                        let id = c["id"].as_str().unwrap_or("").to_ascii_lowercase();
                        let label = c["label"].as_str().unwrap_or("").to_ascii_lowercase();
                        (f.is_empty() || id.contains(&f) || label.contains(&f)) && (!only || c["enabled"].as_bool() == Some(true))
                    })
                    .collect(),
            )
        });
        wrap(r)
    }

    #[tool(
        title = "Run a command",
        description = "Run a command by id with JSON params, e.g. {\"id\":\"timeline.razor\",\"params\":{\"seconds\":3.5}} or {\"id\":\"effects.apply\",\"params\":{\"effect\":\"Gaussian Blur\"}}. Undoable edits land in History. Motion graphics (After Effects-style compositions: text layers with animation presets, shapes, keyframes, expressions, glow): select clips, run `mode.sendSelectionToEffects`, build with `effects.run` {id, params} (e.g. layer.newText, layer.applyTextPreset, layer.newShape, anim.applyBuiltinPreset, effect.apply, prop.set; `effects.run` {id:\"command.list\"} lists all), then `mode.placeCompositionOnTimeline` puts the result back at the playhead.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn command_run(&self, Parameters(p): Parameters<RunParams>) -> Result<CallToolResult, McpError> {
        wrap(self.run(&p.id, p.params.unwrap_or(json!({}))).await)
    }

    #[tool(
        title = "Run several commands",
        description = "Run several commands in order: {steps: [{id, params?}], stop_on_error?: true}. Returns {completed, failed, results: [{ok, result | error}]}; each edit is its own undo step.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn command_batch(&self, Parameters(p): Parameters<BatchParams>) -> Result<CallToolResult, McpError> {
        let stop = p.stop_on_error.unwrap_or(true);
        let (mut completed, mut failed, mut results) = (0u32, 0u32, Vec::new());
        for st in p.steps {
            match self.run(&st.id, st.params.unwrap_or(json!({}))).await {
                Ok(v) => {
                    completed += 1;
                    results.push(json!({"ok": true, "result": v}));
                }
                Err(e) => {
                    failed += 1;
                    results.push(json!({"ok": false, "error": e.to_string()}));
                    if stop {
                        break;
                    }
                }
            }
        }
        let v = json!({"completed": completed, "failed": failed, "results": results});
        Ok(if failed > 0 { CallToolResult::error(vec![Content::text(v.to_string())]) } else { ok_json(&v) })
    }

    #[tool(
        title = "Inspect the document",
        description = "The project as JSON: the tree (as project_inspect) plus the active sequence (as sequence_inspect; null when there is none).",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn doc_inspect(&self) -> Result<CallToolResult, McpError> {
        wrap(self.document().await)
    }

    #[tool(
        title = "Render a preview",
        description = "The active sequence's frame at `seconds` (default: the playhead) as PNG, same as render_frame (bridge mode: Program monitor screenshot).",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn render_preview(&self, p: Parameters<RenderParams>) -> Result<CallToolResult, McpError> {
        self.render_frame(p).await
    }

    #[tool(
        title = "Inspect the project tree",
        description = "Project tree (bins and items with ids, types, durations), active sequence, source clip.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn project_inspect(&self) -> Result<CallToolResult, McpError> {
        wrap(self.run("project.inspect", json!({})).await)
    }

    #[tool(
        title = "Inspect the active sequence",
        description = "The active sequence as JSON: settings, tracks, clips (ids, start/duration in ticks and frames, effects), transitions, markers, in/out, playhead, selection. 254016000000 ticks = 1 second.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn sequence_inspect(&self) -> Result<CallToolResult, McpError> {
        wrap(self.run("sequence.inspect", json!({})).await)
    }

    #[tool(
        title = "Import media",
        description = "Import media files by absolute path (MP4/MOV, WAV/MP3/FLAC/AIFF/Ogg, PNG/JPEG/…).",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn media_import(&self, Parameters(p): Parameters<MediaImportParams>) -> Result<CallToolResult, McpError> {
        wrap(self.run("file.import", media_import_params(p)).await)
    }

    #[tool(
        title = "Render a frame",
        description = "Render the active sequence's frame (at `seconds` or the playhead) and return it as PNG.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn render_frame(&self, Parameters(p): Parameters<RenderParams>) -> Result<CallToolResult, McpError> {
        let max = p.max_side.unwrap_or(960);
        match &*self.backend {
            Backend::Headless(s) => {
                let s = s.clone();
                let r = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, AutomationError> {
                    let g = s.lock().unwrap_or_else(PoisonError::into_inner);
                    // read-only: render at `seconds` without moving the playhead (#114)
                    let t = p.seconds.map_or_else(|| g.playhead(), redrob_film_time::Tick::from_seconds_f64);
                    let (w, h) =
                        g.active_sequence().map(|q| (q.settings.width, q.settings.height)).ok_or_else(|| AutomationError::Other("no sequence".into()))?;
                    let scale = render_scale(max, w, h);
                    let img = g.try_render_program_at(scale, t).map_err(|e| AutomationError::Other(e.to_string()))?;
                    png_rgba(img.w as u32, img.h as u32, img.over_black_rgba8(), max)
                })
                .await
                .unwrap_or_else(|e| Err(join_error(e)));
                Ok(match r {
                    Ok(png) => png_result(&png, "rendered frame"),
                    Err(e) => fail(e),
                })
            }
            Backend::Bridge(b) => {
                let Some(sec) = p.seconds else {
                    return self.screenshot(&b.clone(), Some("Program".into()), max).await;
                };
                // The Program monitor shows the playhead's frame, so the live playhead has to move
                // for the screenshot; put it (and the selection, which may follow it) back after,
                // so the tool stays read-only (#114).
                let before = match self.run("sequence.inspect", json!({})).await {
                    Ok(v) => v,
                    Err(e) => return Ok(fail(e)),
                };
                if let Err(e) = self.run("playhead.set", json!({"seconds": sec})).await {
                    return Ok(fail(e));
                }
                let shot = self.screenshot(&b.clone(), Some("Program".into()), max).await;
                if let Some(t) = before.get("playhead").and_then(Value::as_i64) {
                    let _ = self.run("playhead.set", json!({"time": t})).await;
                }
                // only when it changed: selecting also leaves trim mode
                let now = self.run("sequence.inspect", json!({})).await.ok();
                if let Some(sel) = before.get("selection").filter(|v| v.is_array())
                    && now.as_ref().and_then(|n| n.get("selection")) != Some(sel)
                {
                    let _ = self.run("timeline.select", json!({"clips": sel})).await;
                }
                shot
            }
        }
    }

    #[tool(
        title = "Add sound effects",
        description = "Place synthesised sound effects (no files or licences needed) so each accent lands on its cue: whoosh (cuts, moves), hit (a title landing, a beat), riser (into a payoff; ends on the cue), shimmer (a logo or end card), click (small elements), boom (the biggest moment, once), reverse (into a title). scene_build returns its entrance times as `cues` for this. Example: {\"cues\":[{\"kind\":\"riser\",\"seconds\":12.0},{\"kind\":\"hit\",\"seconds\":12.0,\"gain_db\":-3}]}",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn sfx_add(&self, Parameters(p): Parameters<SfxParams>) -> Result<CallToolResult, McpError> {
        let tracks = p.tracks.filter(|t| !t.is_empty()).unwrap_or_else(|| vec!["A2".into(), "A3".into()]);
        let dir = crate::sfx::cache_dir();
        let mut items: std::collections::HashMap<&'static str, u64> = std::collections::HashMap::new();
        // the end of the last sound on each track, to keep sounds from overwriting each other
        let mut busy = vec![f64::NEG_INFINITY; tracks.len()];
        let mut placed = Vec::new();
        let mut cues = p.cues;
        cues.sort_by(|a, b| a.seconds.total_cmp(&b.seconds));
        for cue in cues {
            let Some(kind) = crate::sfx::Kind::parse(&cue.kind) else {
                return Ok(fail(format!("unknown kind {:?}: whoosh, hit, riser, shimmer, click, boom or reverse", cue.kind)));
            };
            let item = match items.get(kind.name()) {
                Some(i) => *i,
                None => {
                    let path = match crate::sfx::file(&dir, kind) {
                        Ok(p) => p,
                        Err(e) => return Ok(fail(format!("writing the {} sound: {e}", kind.name()))),
                    };
                    let r = match self.run("file.import", json!({"paths": [path.to_string_lossy()]})).await {
                        Ok(r) => r,
                        Err(e) => return Ok(fail(e)),
                    };
                    let Some(i) = r["items"].get(0).and_then(Value::as_u64) else {
                        return Ok(fail(format!("importing the {} sound failed: {r}", kind.name())));
                    };
                    items.insert(kind.name(), i);
                    i
                }
            };
            let start = (cue.seconds - kind.accent()).max(0.0);
            let end = start + f64::from(kind.seconds());
            let slot =
                busy.iter().position(|b| *b <= start + 1e-6).unwrap_or_else(|| busy.iter().enumerate().fold(0, |m, (i, b)| if *b < busy[m] { i } else { m }));
            busy[slot] = end;
            let r = match self.run("timeline.place", json!({"item": item, "track": tracks[slot], "seconds": start, "video": false})).await {
                Ok(r) => r,
                Err(e) => return Ok(fail(format!("placing {} at {:.2}s: {e}", kind.name(), cue.seconds))),
            };
            let clip = r["clips"].get(0).and_then(Value::as_u64);
            let gain = cue.gain_db.unwrap_or(-6.0).clamp(-60.0, 12.0);
            if let Some(c) = clip
                && gain.abs() > 1e-9
            {
                let _ = self.run("clip.audioGain", json!({"clips": [c], "mode": "set", "db": gain})).await;
            }
            placed.push(json!({"kind": kind.name(), "cue": cue.seconds, "start": start, "track": tracks[slot], "clip": clip, "gainDb": gain}));
        }
        Ok(ok_json(&json!({"placed": placed})))
    }

    #[tool(
        title = "Motion guide",
        description = "The motion-design rules for titles, promos and shorts (easing, stagger, holds, layer stack, safe zones, a 30 s structure) and which Film tools apply them. Read it once before building motion graphics.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn motion_guide(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(crate::scene::GUIDE)]))
    }

    #[tool(
        title = "Build a scene",
        description = "Build a finished motion-graphics composition in ONE call from a description in design terms: background (the selected footage or a gradient), text by role (hero/title/body/label/cta) and bars, each with an entrance (eased, staggered) and an exit, plus the look (wash, vignette, grain, Ken Burns drift). The motion guide's rules are the defaults. `place` puts it on the timeline. Then look at it with render_contact_sheet and rebuild with changed values if needed. Example: {\"footage\":\"selection\",\"layers\":[{\"text\":\"SINTEL\",\"role\":\"hero\",\"enter\":\"trackingIn\"},{\"text\":\"잃어버린 친구를 찾아\",\"role\":\"body\"},{\"type\":\"bar\"}],\"place\":true}",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn scene_build(&self, Parameters(p): Parameters<SceneParams>) -> Result<CallToolResult, McpError> {
        let Backend::Headless(s) = &*self.backend else {
            return Ok(fail("scene_build needs the headless engine (redrob-film-cli mcp)"));
        };
        let (s, fx) = (s.clone(), self.effects.clone());
        let spec = serde_json::to_value(&p).unwrap_or_default();
        let r = tokio::task::spawn_blocking(move || -> Result<Value, AutomationError> {
            let mut edit = s.lock().unwrap_or_else(PoisonError::into_inner);
            let mut effects = fx.lock().unwrap_or_else(PoisonError::into_inner);
            build_scene(&mut edit, &mut effects, &spec)
        })
        .await
        .unwrap_or_else(|e| Err(join_error(e)));
        wrap(r)
    }

    #[tool(
        title = "Render a contact sheet",
        description = "Frames of the active sequence, evenly spaced from `from` to `to` (default: all of it), tiled in one PNG with their times listed. Look at one before choosing ranges or crops (what is in the footage, letterbox bars, where the subject is) and again after editing, before you describe the result.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn render_contact_sheet(&self, Parameters(p): Parameters<ContactSheetParams>) -> Result<CallToolResult, McpError> {
        let Backend::Headless(s) = &*self.backend else {
            return Ok(fail("render_contact_sheet needs the headless engine; call render_frame at several times instead"));
        };
        let s = s.clone();
        let tile = p.tile.unwrap_or(240).clamp(64, 640);
        #[allow(clippy::type_complexity)]
        let r = tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, Vec<f64>, Vec<Option<(f32, f32, f32, f32)>>), AutomationError> {
            let g = s.lock().unwrap_or_else(PoisonError::into_inner);
            let q = g.active_sequence().ok_or_else(|| AutomationError::Other("no sequence".into()))?;
            let (w, h) = (q.settings.width, q.settings.height);
            let end = q.duration().seconds();
            let from = p.from.unwrap_or(0.0).clamp(0.0, end);
            let to = p.to.unwrap_or(end).clamp(from, end);
            let times = sheet_times(from, to, p.count.unwrap_or(12));
            let scale = render_scale(tile, w, h);
            let mut frames = Vec::with_capacity(times.len());
            let mut bars = Vec::with_capacity(times.len());
            for &t in &times {
                let img = g.try_render_program_at(scale, redrob_film_time::Tick::from_seconds_f64(t)).map_err(|e| AutomationError::Other(e.to_string()))?;
                let f =
                    image::RgbaImage::from_raw(img.w as u32, img.h as u32, img.over_black_rgba8()).ok_or_else(|| AutomationError::Other("bad image".into()))?;
                bars.push(black_bars(&f));
                frames.push(f);
            }
            let (fw, fh) = frames.first().map_or((1, 1), |f| (f.width(), f.height()));
            let cols = (times.len() as f64).sqrt().ceil() as u32;
            let rows = (times.len() as u32).div_ceil(cols);
            let gap = 4;
            let mut sheet = image::RgbaImage::from_pixel(cols * (fw + gap) + gap, rows * (fh + gap) + gap, image::Rgba([40, 40, 40, 255]));
            for (i, f) in frames.iter().enumerate() {
                let (c, r) = (i as u32 % cols, i as u32 / cols);
                image::imageops::overlay(&mut sheet, f, i64::from(gap + c * (fw + gap)), i64::from(gap + r * (fh + gap)));
            }
            let png = png_rgba(sheet.width(), sheet.height(), sheet.into_raw(), 0)?;
            Ok((png, times, bars))
        })
        .await
        .unwrap_or_else(|e| Err(join_error(e)));
        Ok(match r {
            Ok((png, times, bars)) => {
                let list: Vec<String> = times.iter().map(|t| format!("{t:.1}s")).collect();
                png_result(&png, &format!("contact sheet, left to right then top to bottom: {}\n{}", list.join(", "), bars_note(&times, &bars)))
            }
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Live app: UI state",
        description = "Live app: UI state (tool, workspace, panels, timeline zoom, playback, selection, fps).",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn ui_inspect(&self) -> Result<CallToolResult, McpError> {
        self.ui("ui.inspect", json!({})).await
    }

    #[tool(
        title = "Live app: on-screen elements",
        description = "Live app: every on-screen interactive element with id, label and rect (points). Use ids with ui_click/ui_drag.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn ui_elements(&self, Parameters(p): Parameters<ElementsParams>) -> Result<CallToolResult, McpError> {
        self.ui("ui.elements", json!({"prefix": p.prefix.unwrap_or_default()})).await
    }

    #[tool(
        title = "Live app: click",
        description = "Live app: click an element by id or at x,y (points). Supports right/middle button, double-click (count=2) and modifiers.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn ui_click(&self, Parameters(p): Parameters<ClickParams>) -> Result<CallToolResult, McpError> {
        let mut v = json!({"button": p.button, "count": p.count, "modifiers": p.modifiers});
        if let Some(id) = p.id {
            v["id"] = json!(id);
        } else {
            v["x"] = json!(p.x);
            v["y"] = json!(p.y);
        }
        self.ui("ui.click", v).await
    }

    #[tool(
        title = "Live app: drag",
        description = "Live app: press-drag-release from one point/element to another (move clips, trim edges, scrub, resize panels, drop project items onto tracks).",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn ui_drag(&self, Parameters(p): Parameters<DragParams>) -> Result<CallToolResult, McpError> {
        self.ui("ui.drag", json!({"from": p.from, "to": p.to, "steps": p.steps, "modifiers": p.modifiers})).await
    }

    #[tool(
        title = "Live app: press a key",
        description = "Live app: press a key or shortcut (e.g. `Space`, `L`, `Cmd+K`, `Shift+Delete`, `Cmd+Z`).",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn ui_key(&self, Parameters(p): Parameters<KeyParams>) -> Result<CallToolResult, McpError> {
        self.ui("ui.key", json!({"key": p.key})).await
    }

    #[tool(
        title = "Live app: type text",
        description = "Live app: type text into the focused field.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn ui_type(&self, Parameters(p): Parameters<TypeParams>) -> Result<CallToolResult, McpError> {
        self.ui("ui.type", json!({"text": p.text})).await
    }

    #[tool(
        title = "Live app: screenshot",
        description = "Live app: screenshot of the window or one panel (PNG).",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn ui_screenshot(&self, Parameters(p): Parameters<ShotParams>) -> Result<CallToolResult, McpError> {
        match self.bridge_client() {
            Some(b) => self.screenshot(&b, p.panel, p.max_side.unwrap_or(1600)).await,
            None => Ok(fail("ui_screenshot needs bridge mode")),
        }
    }

    #[tool(
        title = "Live app: control-channel call",
        description = "Live app: call any control-channel method directly (ui.set, ui.scroll, ui.timeline.hit, ui.timeline.locate, ui.playback, ui.panel.show, ui.menu.list, …).",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn ui_control(&self, Parameters(p): Parameters<ControlParams>) -> Result<CallToolResult, McpError> {
        self.ui(&p.method, p.params.unwrap_or(json!({}))).await
    }
}

impl FilmMcp {
    async fn screenshot(&self, b: &BridgeClient, panel: Option<String>, max: u32) -> Result<CallToolResult, McpError> {
        let path = std::env::temp_dir().join(format!("redrob-film-mcp-{}.png", std::process::id()));
        if let Err(e) = b.call("ui.screenshot", json!({"path": path.to_string_lossy(), "panel": panel})).await {
            return Ok(fail(e));
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return Ok(fail(format!("screenshot: {e}"))),
        };
        let _ = std::fs::remove_file(&path);
        let bytes = match image::load_from_memory(&bytes) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                png_rgba(w, h, rgba.into_raw(), max).unwrap_or(bytes)
            }
            Err(_) => bytes,
        };
        Ok(png_result(&bytes, "screenshot of the live Redrob Film window"))
    }
}

impl FilmMcp {
    /// The first argument `tool` doesn't take, as a message naming the ones it does.
    fn unknown_arg(&self, tool: &str, args: Option<&rmcp::model::JsonObject>) -> Option<String> {
        let def = self.tool_router.get(tool)?;
        let props = def.input_schema.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
        let bad = args?.keys().find(|k| !props.contains_key(*k))?;
        let accepted: Vec<&str> = props.keys().map(String::as_str).collect();
        let accepted = if accepted.is_empty() { "no arguments".to_string() } else { accepted.join(", ") };
        Some(format!("unknown argument \"{bad}\" for {tool}; expected: {accepted}"))
    }
}

const INSTRUCTIONS: &str = "Redrob Film video editor (Premiere Pro-class). Every edit is an engine command: `command_list` to discover ids/params, `command_run` to execute (undoable; `command_batch` runs several). `doc_inspect` (or `project_inspect`/`sequence_inspect`) returns ids you can pass to commands; `render_preview` shows the result. Look before you cut and before you report: `render_contact_sheet` over the source shows what each part holds (titles, letterbox bars, where the subject sits), and over the edit afterwards shows what you made; never describe a frame, crop or caption you have not seen. To fill a vertical (9:16) frame, select the clip and run `clip.fillFrame`: it crops black bars baked into the footage, and `focusX`/`focusY` (0..1) keep an off-centre subject in frame. Captions come from `transcript.generate` (Whisper) on the dialogue, not from guesses. Export short-form vertical video with `file.exportMedia` {\"preset\": \"Social Vertical 1080×1920\"} (12 Mbps, scaled to fill) rather than the 20 Mbps default. In bridge mode the `ui_*` tools drive the live app: `ui_elements` lists clickable ids, `ui_click`/`ui_drag`/`ui_key` operate it, `ui_screenshot` shows it. Time is in ticks: 254016000000 per second (commands also accept `seconds`, `frame` or `timecode`).";

/// Resources: the project (as `doc_inspect`) and the command catalog (as `command_list`).
const DOCUMENT_URI: &str = "redrob-film://document";
const COMMANDS_URI: &str = "redrob-film://commands";
// legacy: the URIs the resources had before the rename still read
const LEGACY_DOCUMENT_URI: &str = "filmcraft://document"; // legacy:
const LEGACY_COMMANDS_URI: &str = "filmcraft://commands"; // legacy:

/// MCP 2026-07-28 clients (current Claude Code) require `ttlMs` and `cacheScope` on list and read
/// results; rmcp fills `resultType` and leaves the cache hints to the server, as the generated
/// `tools/list` does. The resource list never changes; the document does, so reads aren't cached.
fn cache_hints(context: &rmcp::service::RequestContext<rmcp::RoleServer>, ttl_ms: u64) -> Option<(u64, rmcp::model::CacheScope)> {
    context.protocol_version().is_some_and(|v| v >= rmcp::model::ProtocolVersion::V_2026_07_28).then_some((ttl_ms, rmcp::model::CacheScope::Private))
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FilmMcp {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(rmcp::model::ServerCapabilities::builder().enable_tools().enable_resources().build())
            .with_server_info(rmcp::model::Implementation::new("redrob-film", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS.to_string())
    }

    /// Strict arguments and long exports in front of the tool router.
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, McpError> {
        if let Some(m) = self.unknown_arg(&request.name, request.arguments.as_ref()) {
            return Err(McpError::invalid_params(m, None));
        }
        // A blocking export reports progress and can be cancelled (docs/agents.md § Long exports).
        // Bridge mode too: the app runs it as a job, so it stays responsive (#91, #92).
        if request.name == "command_run"
            && let Some(a) = &request.arguments
            && let Some(id) = a.get("id").and_then(Value::as_str)
        {
            let params = a.get("params").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
            if crate::long_job::is_long(id, &params) {
                return Ok(self.run_long(id, params, &context).await.into());
            }
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListResourcesResult, McpError> {
        use rmcp::model::Resource;
        let mut r = rmcp::model::ListResourcesResult::with_all_items(vec![
            Resource::new(DOCUMENT_URI, "document")
                .with_title("Project")
                .with_description("The project tree and the active sequence (same as doc_inspect).")
                .with_mime_type("application/json"),
            Resource::new(COMMANDS_URI, "commands")
                .with_title("Command catalog")
                .with_description("Every command with id, label, menu path, shortcut, params and enabled state (same as command_list).")
                .with_mime_type("application/json"),
        ]);
        if let Some((ttl, scope)) = cache_hints(&context, 600_000) {
            r = r.with_ttl_ms(ttl).with_cache_scope(scope);
        }
        Ok(r)
    }

    async fn read_resource(
        &self,
        request: rmcp::model::ReadResourceRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, McpError> {
        let v = match request.uri.as_str() {
            DOCUMENT_URI | LEGACY_DOCUMENT_URI => self.document().await,
            COMMANDS_URI | LEGACY_COMMANDS_URI => self.run("command.list", json!({})).await,
            other => return Err(McpError::resource_not_found(format!("resource not found: {other}"), None)),
        }
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let text = serde_json::to_string_pretty(&v).map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let mut r = rmcp::model::ReadResourceResult::new(vec![rmcp::model::ResourceContents::text(text, request.uri).with_mime_type("application/json")]);
        if let Some((ttl, scope)) = cache_hints(&context, 0) {
            r = r.with_ttl_ms(ttl).with_cache_scope(scope);
        }
        Ok(r.into())
    }
}

/// Render scale that fits the longest side of a `w` x `h` sequence into `max_side` (never upscales; 0 = no limit, as in `png_rgba`).
fn render_scale(max_side: u32, w: u32, h: u32) -> f32 {
    if max_side == 0 {
        return 1.0;
    }
    (max_side as f32 / w.max(h).max(1) as f32).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_scale_zero_means_no_limit() {
        assert_eq!(render_scale(0, 1920, 1080), 1.0);
        assert_eq!(render_scale(960, 1920, 1080), 0.5);
        assert_eq!(render_scale(960, 640, 360), 1.0);
        assert_eq!(render_scale(960, 0, 0), 1.0);
    }

    #[test]
    fn black_bars_measure_letterbox_and_ignore_dark_scenes() {
        // 100x100: rows 0..12 and 88..100 black, a lit picture between
        let mut f = image::RgbaImage::from_pixel(100, 100, image::Rgba([0, 0, 0, 255]));
        for y in 12..88 {
            for x in 0..100 {
                f.put_pixel(x, y, image::Rgba([120, 90, 60, 255]));
            }
        }
        let (t, b, l, r) = black_bars(&f).unwrap();
        assert!((t - 0.12).abs() < 0.011 && (b - 0.12).abs() < 0.011 && l == 0.0 && r == 0.0, "{t} {b} {l} {r}");
        // all black (a fade) reports no bars, and the note ignores it
        // a vignette: edges fade to near-black, no hard edge, so no bars
        let vig = image::RgbaImage::from_fn(100, 100, |x, y| {
            let d = ((x as f32 - 50.0).powi(2) + (y as f32 - 50.0).powi(2)).sqrt() / 70.0;
            let v = (60.0 * (1.0 - d).max(0.0)) as u8;
            image::Rgba([v, v, v, 255])
        });
        assert_eq!(black_bars(&vig), Some((0.0, 0.0, 0.0, 0.0)));
        let dark = image::RgbaImage::from_pixel(50, 50, image::Rgba([0, 0, 0, 255]));
        assert_eq!(black_bars(&dark), None);
        let note = bars_note(&[0.0, 1.0, 2.0], &[None, Some((0.12, 0.12, 0.0, 0.0)), Some((0.12, 0.12, 0.0, 0.0))]);
        assert!(note.contains("top 12%") && note.contains("1.32x"), "{note}");
        assert!(bars_note(&[0.0], &[None]).contains("none"));
        // two of three frames without bars: the typical frame has none
        assert!(bars_note(&[0.0, 1.0, 2.0], &[Some((0.0, 0.0, 0.0, 0.0)), Some((0.0, 0.0, 0.0, 0.0)), Some((0.12, 0.12, 0.0, 0.0))]).contains("none"));
    }

    #[test]
    fn sheet_times_are_centred_in_their_slices() {
        assert_eq!(sheet_times(0.0, 10.0, 2), vec![2.5, 7.5]);
        assert_eq!(sheet_times(0.0, 10.0, 100).len(), 30);
    }

    /// The headless server reaches Effects the way the app's AI dock does: a clip goes to a
    /// composition, an Effects command adds a text layer, and the composition comes back to the
    /// timeline, picture and all.
    #[tokio::test]
    async fn headless_mode_commands_round_trip_through_effects() {
        // a still on disk: the hand-off sends clips with a media file behind them
        let path = std::env::temp_dir().join(format!("film-mcp-effects-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(320, 180, image::Rgba([30, 90, 160, 255])).save(&path).unwrap();
        let mut s = Session::default();
        let item = s.execute("file.import", json!({"paths": [path.to_string_lossy()]})).unwrap()["items"][0].as_u64().unwrap();
        s.execute("file.newSequence", json!({"name": "v", "width": 320, "height": 180})).unwrap();
        s.execute("timeline.place", json!({"item": item, "track": "V1", "seconds": 0.0})).unwrap();
        let m = FilmMcp::headless(s);
        let list = m.run("command.list", json!({})).await.unwrap();
        let ids: Vec<&str> = list.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
        for id in redrob_film_handoff::ALL {
            assert!(ids.contains(&id), "{id} listed");
        }
        assert_eq!(m.run("mode.status", json!({})).await.unwrap()["editors"], json!(["edit"]));
        let seq = m.run("sequence.inspect", json!({})).await.unwrap();
        let clip = seq["video"][0]["items"][0]["clip"].as_u64().unwrap();
        m.run("timeline.select", json!({"clips": [clip]})).await.unwrap();
        let sent = m.run("mode.sendSelectionToEffects", json!({"name": "Titles"})).await.unwrap();
        assert_eq!(sent["mode"], "effects", "{sent}");
        let text = m.run("effects.run", json!({"id": "layer.newText", "params": {"text": "Hello", "size": 120}})).await.unwrap();
        assert!(text["layer"].as_u64().is_some(), "{text}");
        let info = m.run("effects.run", json!({"id": "comp.info"})).await.unwrap();
        assert_eq!(info["layers"].as_array().unwrap().len(), 2, "{info}");
        m.run("playhead.set", json!({"seconds": 0.0})).await.unwrap();
        let placed = m.run("mode.placeCompositionOnTimeline", json!({"track": "V1"})).await.unwrap();
        assert_eq!(placed["mode"], "edit", "{placed}");
        let r = m.render_frame(Parameters(RenderParams { seconds: Some(0.5), max_side: Some(96) })).await.unwrap();
        assert_ne!(r.is_error, Some(true), "{r:?}");
        assert!(m.run("effects.run", json!({"id": "no.such.command"})).await.is_err());
        assert!(m.run("mode.switch", json!({"mode": "photo"})).await.is_err());
    }

    #[tokio::test]
    async fn contact_sheet_renders_the_sequence_in_one_png() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let m = FilmMcp::headless(s);
        let r = m.render_contact_sheet(Parameters(ContactSheetParams { count: Some(4), tile: Some(96), from: None, to: None })).await.unwrap();
        assert_ne!(r.is_error, Some(true), "{r:?}");
        let text = format!("{:?}", r.content);
        assert!(text.contains("image/png") && text.contains("contact sheet") && text.contains("black bars"), "{text}");
    }

    #[tokio::test]
    async fn headless_commands_and_render() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let m = FilmMcp::headless(s);
        let v = m.run("sequence.addEditAllTracks", json!({"seconds": 2.0})).await.unwrap();
        assert!(v["cuts"].as_u64().unwrap() >= 2);
        let r = m.render_frame(Parameters(RenderParams { seconds: Some(1.0), max_side: Some(320) })).await.unwrap();
        assert_ne!(r.is_error, Some(true));
    }

    /// `render_frame` / `render_preview` are annotated read-only (#114): rendering at `seconds`
    /// leaves the playhead, and the selection that follows it, where they were, and still renders
    /// the frame at `seconds`.
    #[tokio::test]
    async fn rendering_at_a_time_does_not_move_the_playhead() {
        let m = FilmMcp::headless(demo());
        m.run("sequence.selectionFollowsPlayhead", json!({"on": true})).await.unwrap();
        m.run("playhead.set", json!({"seconds": 1.0})).await.unwrap();
        let before = m.run("sequence.inspect", json!({})).await.unwrap();
        let png = |r: CallToolResult| serde_json::to_string(&r.content).unwrap();
        let at_playhead = png(m.render_frame(Parameters(RenderParams { seconds: None, max_side: Some(64) })).await.unwrap());
        for seconds in [5.0, 8.0] {
            let r = m.render_frame(Parameters(RenderParams { seconds: Some(seconds), max_side: Some(64) })).await.unwrap();
            assert_ne!(png(r), at_playhead, "renders the frame at {seconds} s");
            m.render_preview(Parameters(RenderParams { seconds: Some(seconds), max_side: Some(64) })).await.unwrap();
        }
        let after = m.run("sequence.inspect", json!({})).await.unwrap();
        assert_eq!(after["playhead"], before["playhead"]);
        assert_eq!(after["selection"], before["selection"]);
    }

    /// One request/response exchange over `serve_io`, as an MCP client sees it.
    struct Client {
        tx: tokio::io::DuplexStream,
        rx: tokio::io::Lines<tokio::io::BufReader<tokio::io::DuplexStream>>,
    }

    impl Client {
        fn start(s: Session) -> Self {
            use tokio::io::AsyncBufReadExt;
            let (tx, server_in) = tokio::io::duplex(1 << 22);
            let (server_out, rx) = tokio::io::duplex(1 << 22);
            tokio::spawn(FilmMcp::headless(s).serve_io(server_in, server_out));
            Self { tx, rx: tokio::io::BufReader::new(rx).lines() }
        }
        async fn send(&mut self, line: &str) {
            use tokio::io::AsyncWriteExt;
            self.tx.write_all(format!("{line}\n").as_bytes()).await.unwrap();
        }
        async fn next(&mut self) -> Value {
            serde_json::from_str(&self.rx.next_line().await.unwrap().unwrap()).unwrap()
        }
        async fn ask(&mut self, msg: Value) -> Value {
            self.send(&msg.to_string()).await;
            self.next().await
        }
        async fn init(&mut self) -> Value {
            let r = self.ask(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}})).await;
            self.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string()).await;
            r
        }
        async fn call(&mut self, id: u64, name: &str, arguments: Value) -> Value {
            self.ask(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})).await
        }
    }

    /// #499: `media_import` takes `paths` (a required array), and forwards `bin` / `image_sequence` to `file.import`.
    #[test]
    fn media_import_schema_and_params() {
        let m = FilmMcp::headless(Session::default());
        let def = m.tool_router.get("media_import").unwrap();
        let schema = Value::Object((*def.input_schema).clone());
        assert_eq!(schema["properties"]["paths"]["type"], "array", "{schema}");
        assert!(schema["required"].as_array().unwrap().iter().any(|r| r == "paths"), "{schema}");
        let p: MediaImportParams = serde_json::from_value(json!({"paths": ["/a.png"], "bin": 7, "image_sequence": true})).unwrap();
        assert_eq!(media_import_params(p), json!({"paths": ["/a.png"], "bin": 7, "imageSequence": true}));
        let p: MediaImportParams = serde_json::from_value(json!({"paths": ["/a.wav"]})).unwrap();
        assert_eq!(media_import_params(p), json!({"paths": ["/a.wav"]}));
        assert!(serde_json::from_value::<MediaImportParams>(json!({"text": "/a.wav"})).is_err());
    }

    fn demo() -> Session {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s
    }

    /// Every listed tool has a title and the four hints, and the core tools are listed.
    #[tokio::test(flavor = "multi_thread")]
    async fn tools_are_titled_and_annotated() {
        let mut c = Client::start(demo());
        let init = c.init().await;
        assert_eq!(init["result"]["serverInfo"]["name"], "redrob-film");
        let tools = c.ask(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).await["result"]["tools"].as_array().cloned().unwrap();
        for t in &tools {
            assert!(t["title"].is_string(), "{} has no title", t["name"]);
            for h in ["readOnlyHint", "destructiveHint", "idempotentHint", "openWorldHint"] {
                assert!(t["annotations"][h].is_boolean(), "{} lacks {h}", t["name"]);
            }
        }
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        for want in ["command_list", "command_run", "command_batch", "doc_inspect", "render_preview", "project_inspect", "render_frame", "ui_click"] {
            assert!(names.contains(&want), "{want} not listed");
        }
        let ro = |n: &str| tools.iter().find(|t| t["name"] == n).map(|t| t["annotations"]["readOnlyHint"].clone());
        assert_eq!((ro("command_list"), ro("command_run")), (Some(json!(true)), Some(json!(false))));
    }

    /// Strict arguments, malformed JSON, batch, doc_inspect and render_preview over the raw line
    /// protocol.
    #[tokio::test(flavor = "multi_thread")]
    async fn strict_arguments_and_parse_errors() {
        let mut c = Client::start(demo());
        c.init().await;
        let r = c.call(1, "command_list", json!({"filter": "razor", "enabled_only": true})).await;
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("timeline.razor"), "{r}");
        // an unknown argument names itself and the accepted ones
        let r = c.call(2, "command_list", json!({"filtr": "x"})).await;
        assert_eq!(r["error"]["code"], -32602, "{r}");
        let m = r["error"]["message"].as_str().unwrap();
        assert!(m.contains("\"filtr\"") && m.contains("filter") && m.contains("enabled_only"), "{m}");
        let r = c.call(3, "doc_inspect", json!({"verbose": true})).await;
        assert_eq!(r["error"]["code"], -32602, "{r}");
        // command params stay free-form
        let r = c.call(4, "command_run", json!({"id": "playhead.set", "params": {"seconds": 0.5}})).await;
        assert_eq!(r["result"]["isError"], false, "{r}");
        // malformed JSON: a parse error, and the server keeps serving
        c.send("{\"jsonrpc\": \"2.0\", \"id\": 5, BROKEN").await;
        let r = c.next().await;
        assert_eq!((r["error"]["code"].as_i64(), r["id"].is_null()), (Some(-32700), true), "{r}");
        let r = c.ask(json!({"jsonrpc":"2.0","id":6,"method":"ping"})).await;
        assert!(r["result"].is_object(), "{r}");
        // batch: stops at the first failure and reports both
        let r =
            c.call(7, "command_batch", json!({"steps": [{"id": "playhead.set", "params": {"seconds": 0.5}}, {"id": "no.such"}, {"id": "playhead.set"}]})).await;
        assert_eq!(r["result"]["isError"], true, "{r}");
        let v: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!((v["completed"].as_u64(), v["failed"].as_u64(), v["results"].as_array().map(Vec::len)), (Some(1), Some(1), Some(2)), "{v}");
        let r = c.call(8, "doc_inspect", json!({})).await;
        let v: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(v["project"].is_object() && v["sequence"].is_object(), "{v}");
        let r = c.call(9, "render_preview", json!({"max_side": 64})).await;
        assert_eq!(r["result"]["content"][0]["type"], "image", "{r}");
    }

    /// `redrob-film://document` and `redrob-film://commands` are listed and readable as JSON.
    #[tokio::test(flavor = "multi_thread")]
    async fn resources_document_and_commands() {
        let mut c = Client::start(demo());
        let init = c.init().await;
        assert!(init["result"]["capabilities"]["resources"].is_object(), "{init}");
        assert!(init["result"]["instructions"].as_str().unwrap().contains("doc_inspect"), "{init}");
        let r = c.ask(json!({"jsonrpc":"2.0","id":1,"method":"resources/list"})).await;
        let uris: Vec<&str> = r["result"]["resources"].as_array().unwrap().iter().filter_map(|x| x["uri"].as_str()).collect();
        assert_eq!(uris, [DOCUMENT_URI, COMMANDS_URI], "{r}");
        let r = c.ask(json!({"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":DOCUMENT_URI}})).await;
        assert_eq!(r["result"]["contents"][0]["mimeType"], "application/json", "{r}");
        let v: Value = serde_json::from_str(r["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
        assert!(v["project"].is_object() && v["sequence"].is_object(), "{v}");
        let r = c.ask(json!({"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":COMMANDS_URI}})).await;
        assert!(r["result"]["contents"][0]["text"].as_str().unwrap().contains("timeline.razor"), "{r}");
        let r = c.ask(json!({"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"redrob-film://nope"}})).await;
        assert!(r["error"]["code"].is_i64(), "{r}");
    }

    /// MCP 2026-07-28 clients (current Claude Code) negotiate per request and reject list and read
    /// results without `resultType`, `ttlMs` and `cacheScope`; sessions that negotiated an older
    /// revision through `initialize` get the old shape.
    #[tokio::test(flavor = "multi_thread")]
    async fn modern_clients_get_result_type_and_cache_hints() {
        let mut c = Client::start(Session::default());
        let meta = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {},
        });
        let r = c.ask(json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":meta}})).await;
        assert!(r["result"]["supportedVersions"].as_array().unwrap().contains(&json!("2026-07-28")), "{r}");
        for (id, method, params) in [
            (2, "tools/list", json!({"_meta": meta})),
            (3, "resources/list", json!({"_meta": meta})),
            (4, "resources/read", json!({"_meta": meta, "uri": DOCUMENT_URI})),
        ] {
            let r = c.ask(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).await;
            assert_eq!(r["result"]["resultType"], "complete", "{method}: {r}");
            assert!(r["result"]["ttlMs"].is_u64(), "{method}: {r}");
            assert!(matches!(r["result"]["cacheScope"].as_str(), Some("public" | "private")), "{method}: {r}");
        }
        let mut c = Client::start(Session::default());
        c.init().await;
        for (id, method, params) in [(2, "tools/list", json!({})), (3, "resources/list", json!({})), (4, "resources/read", json!({"uri": DOCUMENT_URI}))] {
            let r = c.ask(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).await;
            assert!(r["result"].is_object() && r["result"].get("resultType").is_none() && r["result"].get("ttlMs").is_none(), "{method}: {r}");
        }
    }

    /// docs/agents.md § Long exports: a blocking export reports `notifications/progress` for its
    /// token (none without one), answers `ping` meanwhile, and `notifications/cancelled` stops
    /// it, deletes the partial file and sends no response.
    #[tokio::test(flavor = "multi_thread")]
    async fn export_progress_and_cancel() {
        let dir = std::env::temp_dir().join(format!("redrob-film-mcp-progress-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = Client::start(demo());
        c.init().await;
        let export = |id: u64, name: &str, token: Option<Value>, seconds: f64| {
            let path = dir.join(name).to_string_lossy().to_string();
            let params = json!({"path": path, "format": "h264", "width": 640, "height": 360, "audio": false, "range": "custom", "startSeconds": 0, "endSeconds": seconds, "wait": true});
            let mut call =
                json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"command_run","arguments":{"id":"file.exportMedia","params":params}}});
            if let Some(t) = token {
                call["params"]["_meta"] = json!({"progressToken": t});
            }
            call.to_string()
        };
        // no token: no notifications, the same response as before
        c.send(&export(1, "quiet.mp4", None, 0.5)).await;
        let m = c.next().await;
        assert_eq!((m["id"].as_u64(), &m["result"]["isError"]), (Some(1), &json!(false)), "{m}");
        // progress, strictly increasing, with a total; ping answered while the export runs. The
        // job is polled every 100 ms, so a fast machine finishes a short export within one poll:
        // a longer one is tried until the export outlasts a few polls.
        let (mut notes, mut pong) = (0, false);
        for (attempt, seconds) in [1.0, 4.0, 16.0].into_iter().enumerate() {
            let (id, ping, name) = (20 + attempt as u64, 30 + attempt as u64, format!("a{attempt}.mp4"));
            c.send(&export(id, &name, Some(json!("tok")), seconds)).await;
            let (mut last, mut pinged) = (-1.0, false);
            (notes, pong) = (0, false);
            let done = loop {
                let m = c.next().await;
                if m["method"] == "notifications/progress" {
                    assert_eq!(m["params"]["progressToken"], "tok", "{m}");
                    let p = m["params"]["progress"].as_f64().unwrap();
                    assert!(p > last && m["params"]["total"].as_f64().unwrap() >= p, "{m}");
                    last = p;
                    notes += 1;
                    if !pinged {
                        pinged = true;
                        c.send(&json!({"jsonrpc": "2.0", "id": ping, "method": "ping"}).to_string()).await;
                    }
                } else if m["id"] == ping {
                    pong = true;
                } else if m["id"] == id {
                    break m;
                }
            };
            assert_eq!(done["result"]["isError"], false, "{done}");
            assert!(notes >= 1, "a finished export reports its progress at least once");
            assert!(dir.join(&name).exists());
            if notes >= 2 && pong {
                break;
            }
            // the ping's answer is still on its way: read it so it cannot be taken for a later reply
            while pinged && !pong {
                pong = c.next().await["id"] == ping;
            }
            pong = false;
        }
        assert!(notes >= 2 && pong, "{notes} notifications, ping answered during the export: {pong}");
        // cancel after the first notification: no response, no partial file
        c.send(&export(4, "b.mp4", Some(json!(7)), 20.0)).await;
        loop {
            let m = c.next().await;
            assert_ne!(m["id"], 4, "finished before the cancel: {m}");
            if m["method"] == "notifications/progress" {
                break;
            }
        }
        c.send(r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":4}}"#).await;
        let t0 = std::time::Instant::now();
        while dir.join("b.mp4").exists() || t0.elapsed() < std::time::Duration::from_millis(500) {
            assert!(t0.elapsed().as_secs() < 30, "partial file still there");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        c.send(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"command_run","arguments":{"id":"jobs.list"}}}"#).await;
        let m = loop {
            let m = c.next().await;
            assert_ne!(m["id"], 4, "no response for a cancelled request: {m}");
            if m["id"] == 5 {
                break m;
            }
        };
        let jobs: Value = serde_json::from_str(m["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let last = jobs.as_array().and_then(|a| a.last()).cloned().unwrap_or_default();
        assert_eq!(last["result"]["error"], "cancelled", "{jobs}");
        assert!(!dir.join("b.mp4").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A command that panicked used to poison the session lock, and every later call failed with
    /// "session lock poisoned" until the server was restarted.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_poisoned_session_lock_keeps_serving() {
        let m = FilmMcp::headless(demo());
        let Backend::Headless(s) = &*m.backend else { panic!("headless backend") };
        let s = s.clone();
        let _ = std::thread::spawn(move || {
            let _g = s.lock().unwrap();
            panic!("a command panicked");
        })
        .join();
        let v = m.run("sequence.inspect", json!({})).await.unwrap();
        assert!(v.is_object(), "{v}");
        let r = m.render_frame(Parameters(RenderParams { seconds: Some(0.5), max_side: Some(64) })).await.unwrap();
        assert_ne!(r.is_error, Some(true));
    }
}
