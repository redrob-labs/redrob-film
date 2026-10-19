//! Adobe Premiere Pro project (`.prproj`) import.
//!
//! A `.prproj` is gzip-compressed XML (Premiere also reads it uncompressed). The root
//! `<PremiereData>` holds a flat pool of objects: each top-level element carries either an
//! `ObjectID` (an integer) or an `ObjectUID` (a GUID), and refers to other objects with
//! `ObjectRef` / `ObjectURef` attributes on empty elements. The format was learned only from
//! sample projects (project versions 22 to 45, Premiere CS6 to 2026) read as XML; see the crate
//! README for the clean-room note.
//!
//! What is read:
//!
//! - the Project panel: `RootProjectItem` → `BinProjectItem` (bins) / `ClipProjectItem` (items,
//!   with their label `BE.Prefs.LabelColors.N`, which is the index into [`Label::ALL`]);
//! - `MasterClip` → `VideoClip` / `AudioClip` → `VideoMediaSource` / `AudioMediaSource` → `Media`
//!   (file path, offline reason, start timecode, `VideoStream` / `AudioStream` characteristics),
//!   or → `VideoSequenceSource` → `Sequence` (a sequence item or a nested sequence); adjustment
//!   layers (`IsAdjustmentLayer`); Black Video / Color Matte / Bars and Tone / Transparent Video /
//!   Universal Counting Leader by title;
//! - `Sequence`: name, frame rate (`FrameRate` in ticks per frame), frame size (`FrameRect`), audio
//!   sample rate (ticks per sample), start time (`MZ.ZeroPoint`), markers, links, video and audio
//!   tracks (locked, output/mute);
//! - track items: `Start` / `End` on the timeline, `InPoint` in the source, `PlaybackSpeed` (a
//!   ratio, so 0.25 is 25 %; a negative ratio is read as reverse), disabled (`IsMuted` on the
//!   track item), clip gain, labels;
//! - transitions (`Start`, `End`, `Alignment` = the cut's offset from the start, the clips on
//!   each side) mapped to our nearest transition by name;
//! - intrinsic effects: Motion (position and anchor are stored as fractions of the sequence and
//!   source frame), Opacity, Volume (`Level` is linear gain where 1.0 is +15 dB) and Channel
//!   Volume, with keyframes (stored in source time, as ours are);
//! - markers (`DVAMarker` JSON: name, comment, start, duration, type, colour).
//!
//! Time needs no conversion: Premiere's tick is ours ([`redrob_film_time::TICKS_PER_SECOND`] =
//! 254 016 000 000 per second, the value every sample's frame rates divide).
//!
//! Everything else (other effects that have no match in our library, blend modes, caption
//! tracks, graphics and titles, variable speed, keyframe easing) is named in the [`Report`].

use std::collections::{HashMap, HashSet};
use std::io::Read as _;

use redrob_film_geom::Vec2;
use redrob_film_media::{Generator, MediaKind};
use redrob_film_project::{
    BinId, ClipId, EffectKind, Interpolation, ItemId, ItemKind, Keyframe, Label, Marker, MarkerId, MarkerKind, Param, ParamValue, Sequence, TrackItem,
    TrackKind, Transition, TransitionAlign, TransitionId, effect::find_effect_by_name,
};
use redrob_film_time::{FrameRate, TICKS_PER_SECOND, Tick};
use roxmltree::Node;
use serde_json::Value;

use crate::common::{Builder, MediaSpec, empty_sequence, gain_to_db, resolve_path, set_param, settings_for, transition_effect};
use crate::xml::{child, child_text, elements, text};
use crate::{Error, Format, ImportOptions, Imported, Report, Result};

/// Largest decompressed document we accept (a gzip bomb must not exhaust memory).
const MAX_XML_BYTES: u64 = 1 << 30;
/// Bin and nested-sequence depth limit.
const MAX_DEPTH: usize = 64;
/// Premiere's Volume `Level` (and Channel Volume) store linear gain scaled so 1.0 is +15 dB.
const VOLUME_HEADROOM_DB: f64 = 15.0;
/// Opacity ▸ Blend Mode value on clips left at Normal in every sample.
const BLEND_NORMAL: i64 = 18;

fn is_gzip(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// Whether `bytes` look like a Premiere project (gzip-compressed or plain XML).
pub(crate) fn sniff(bytes: &[u8]) -> bool {
    if is_gzip(bytes) {
        let mut head = Vec::new();
        // A truncated or corrupt stream still yields what decoded before the error.
        let _ = flate2::read::MultiGzDecoder::new(bytes).take(16 * 1024).read_to_end(&mut head);
        return contains(&head, b"<PremiereData");
    }
    let n = bytes.len().min(4096);
    bytes.get(..n).is_some_and(|h| contains(h, b"<PremiereData"))
}

/// The XML text of a `.prproj` (decompressed when gzipped).
pub fn decode_document(bytes: &[u8]) -> Result<String> {
    if !is_gzip(bytes) {
        return Ok(crate::decode_text(bytes));
    }
    let mut out = Vec::new();
    flate2::read::MultiGzDecoder::new(bytes)
        .take(MAX_XML_BYTES + 1)
        .read_to_end(&mut out)
        .map_err(|e| Error::parse(Format::Prproj, format!("the gzip stream is damaged: {e}")))?;
    if out.len() as u64 > MAX_XML_BYTES {
        return Err(Error::parse(Format::Prproj, "the project is larger than 1 GiB uncompressed"));
    }
    Ok(crate::decode_text(&out))
}

// ---------------------------------------------------------------------------------------------
// Numbers and small values
// ---------------------------------------------------------------------------------------------

fn int(s: &str) -> Option<i64> {
    let s = s.trim();
    s.parse::<i64>().ok().or_else(|| s.parse::<f64>().ok().filter(|f| f.is_finite()).map(|f| f as i64))
}

fn float(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|f| f.is_finite())
}

/// A tick count from the document, clamped to the range our time arithmetic is safe in.
fn tick(v: i64) -> Tick {
    Tick(v.clamp(Tick::MIN.0, Tick::MAX.0))
}

/// Frame rate from ticks per frame.
fn rate_of(ticks_per_frame: i64) -> Option<FrameRate> {
    (ticks_per_frame > 0 && ticks_per_frame <= TICKS_PER_SECOND).then(|| FrameRate::new(TICKS_PER_SECOND, ticks_per_frame))
}

/// Sample rate from ticks per sample.
fn sample_rate_of(ticks_per_sample: i64) -> Option<u32> {
    if ticks_per_sample <= 0 {
        return None;
    }
    let sr = (TICKS_PER_SECOND as f64 / ticks_per_sample as f64).round();
    (1.0..=1_000_000.0).contains(&sr).then_some(sr as u32)
}

/// `left,top,right,bottom` → (width, height).
fn rect(s: &str) -> Option<(u32, u32)> {
    let v: Vec<i64> = s.split(',').filter_map(int).collect();
    let [l, t, r, b] = v.as_slice() else { return None };
    let w = u32::try_from(r.checked_sub(*l)?).ok().filter(|w| (1..=65_536).contains(w))?;
    let h = u32::try_from(b.checked_sub(*t)?).ok().filter(|h| (1..=65_536).contains(h))?;
    Some((w, h))
}

/// `BE.Prefs.LabelColors.N` → label N of Premiere's (and our) sixteen.
fn label_of(s: &str) -> Option<Label> {
    let n = s.trim().strip_prefix("BE.Prefs.LabelColors.")?.parse::<usize>().ok()?;
    Label::ALL.get(n).copied()
}

/// The label whose colour is nearest to an ARGB / RGB value.
fn label_near(argb: u64) -> Label {
    let (r, g, b) = (((argb >> 16) & 0xff) as i32, ((argb >> 8) & 0xff) as i32, (argb & 0xff) as i32);
    Label::ALL
        .into_iter()
        .min_by_key(|l| {
            let [lr, lg, lb] = l.rgb();
            let (dr, dg, db) = (i32::from(lr) - r, i32::from(lg) - g, i32::from(lb) - b);
            dr * dr + dg * dg + db * db
        })
        .unwrap_or(Label::Green)
}

/// Volume `Level` (linear, 1.0 = +15 dB) → dB.
fn level_db(v: f64) -> f64 {
    if v <= 0.0 { -287.5 } else { (20.0 * v.log10() + VOLUME_HEADROOM_DB).clamp(-287.5, VOLUME_HEADROOM_DB) }
}

fn looks_like_path(p: &str) -> bool {
    p.contains('/') || p.contains('\\')
}

fn ticks_json(v: &Value) -> Option<i64> {
    let t = v.get("ticks").unwrap_or(v);
    t.as_i64().or_else(|| t.as_str().and_then(int)).or_else(|| t.as_f64().filter(|f| f.is_finite()).map(|f| f as i64))
}

fn object_key(n: Node) -> String {
    match n.attribute("ObjectID").or_else(|| n.attribute("ObjectUID")) {
        Some(id) => id.to_string(),
        None => format!("#node{}", n.id().get()),
    }
}

// ---------------------------------------------------------------------------------------------
// The object graph
// ---------------------------------------------------------------------------------------------

struct Graph<'a, 'i> {
    objs: HashMap<&'a str, Node<'a, 'i>>,
}

impl<'a, 'i> Graph<'a, 'i> {
    fn new(root: Node<'a, 'i>) -> Self {
        let mut objs = HashMap::new();
        for n in elements(root) {
            if let Some(id) = n.attribute("ObjectID").or_else(|| n.attribute("ObjectUID")) {
                objs.entry(id).or_insert(n);
            }
        }
        Self { objs }
    }

    /// The object a reference element names; an element that is not a reference is itself.
    fn deref(&self, n: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
        match n.attribute("ObjectRef").or_else(|| n.attribute("ObjectURef")) {
            Some(r) => self.objs.get(r).copied(),
            None => Some(n),
        }
    }

    /// The element at `path` below `n`, following a reference at every step.
    fn at(&self, n: Node<'a, 'i>, path: &[&str]) -> Option<Node<'a, 'i>> {
        let mut cur = n;
        for p in path {
            cur = self.deref(child(cur, p)?)?;
        }
        Some(cur)
    }

    fn text_at(&self, n: Node<'a, 'i>, path: &[&str]) -> Option<&'a str> {
        self.at(n, path).map(text)
    }

    /// The (referenced) entries of the list element at `path`, in `Index` order.
    fn list(&self, n: Node<'a, 'i>, path: &[&str]) -> Vec<Node<'a, 'i>> {
        let Some(c) = self.at(n, path) else { return Vec::new() };
        let mut v: Vec<(i64, Node<'a, 'i>)> =
            elements(c).filter_map(|e| Some((e.attribute("Index").and_then(int).unwrap_or(i64::MAX), self.deref(e)?))).collect();
        v.sort_by_key(|(i, _)| *i);
        v.into_iter().map(|(_, n)| n).collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------------------------

/// Keyframe easing: `0` is linear in every sample; the other codes are read as Bézier.
fn interp_of(code: Option<&str>) -> Interpolation {
    match code.map(str::trim) {
        None | Some("") | Some("0") => Interpolation::Linear,
        Some(_) => Interpolation::Bezier,
    }
}

fn animated(p: Node) -> bool {
    child_text(p, "IsTimeVarying") != Some("false") && child_text(p, "Keyframes").is_some_and(|k| !k.trim().is_empty())
}

/// The keyframes of a parameter: `time,value…;time,value…;`, times shifted by `-shift`.
fn keyframe_fields<'s>(p: Node<'s, '_>) -> impl Iterator<Item = Vec<&'s str>> {
    let k = if animated(p) { child_text(p, "Keyframes").unwrap_or("") } else { "" };
    k.split(';').map(str::trim).filter(|s| !s.is_empty()).map(|s| s.split(',').collect::<Vec<_>>())
}

/// The static value fields (`StartKeyframe` without its time).
fn start_fields<'s>(p: Node<'s, '_>) -> Vec<&'s str> {
    child_text(p, "StartKeyframe").map(|s| s.split(',').skip(1).collect()).unwrap_or_default()
}

fn finish_param(mut pr: Param) -> Param {
    pr.keyframes.sort_by_key(|k| k.time);
    if let Some(k) = pr.keyframes.first() {
        pr.value = k.value.clone();
    }
    pr
}

fn float_param(p: Node, shift: i64, map: impl Fn(f64) -> f64) -> Option<Param> {
    let base = start_fields(p).first().copied().and_then(float).or_else(|| child_text(p, "CurrentValue").and_then(float))?;
    let mut pr = Param::new(ParamValue::Float(map(base)));
    for f in keyframe_fields(p) {
        let (Some(t), Some(v)) = (f.first().copied().and_then(int), f.get(1).copied().and_then(float)) else { continue };
        let mut k = Keyframe::new(tick(t.saturating_sub(shift)), ParamValue::Float(map(v)));
        k.interp = interp_of(f.get(2).copied());
        pr.keyframes.push(k);
    }
    Some(finish_param(pr))
}

fn point_param(p: Node, shift: i64, map: impl Fn(f64, f64) -> Vec2) -> Option<Param> {
    let s = start_fields(p);
    let (x, y) = (s.first().copied().and_then(float)?, s.get(1).copied().and_then(float)?);
    let mut pr = Param::new(ParamValue::Vec2(map(x, y)));
    for f in keyframe_fields(p) {
        let (Some(t), Some(kx), Some(ky)) = (f.first().copied().and_then(int), f.get(1).copied().and_then(float), f.get(2).copied().and_then(float)) else {
            continue;
        };
        pr.keyframes.push(Keyframe::new(tick(t.saturating_sub(shift)), ParamValue::Vec2(map(kx, ky))));
    }
    Some(finish_param(pr))
}

fn bool_param(p: Node) -> Option<Param> {
    let v = start_fields(p).first().copied().or_else(|| child_text(p, "CurrentValue"))?.trim().to_ascii_lowercase();
    Some(Param::new(ParamValue::Bool(v == "true" || v == "1")))
}

/// A point parameter differs from the frame centre (or is animated).
fn moved(pr: &Param, cx: f64, cy: f64) -> bool {
    pr.is_animated() || pr.value.as_vec2().is_some_and(|v| (v.x - cx).abs() > 1e-6 || (v.y - cy).abs() > 1e-6)
}

// ---------------------------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Placement {
    bin: Option<BinId>,
    name: String,
    label: Option<Label>,
}

struct Imp<'a, 'i, 'r> {
    g: Graph<'a, 'i>,
    b: Builder,
    base_dir: Option<String>,
    report: &'r mut Report,
    /// Where the Project panel files each master clip (by object key), in panel order.
    placement: HashMap<String, Placement>,
    order: Vec<String>,
    /// Master clip → item (`None` while in progress or when it cannot be imported).
    masters: HashMap<String, Option<ItemId>>,
    seqs: HashMap<String, ItemId>,
    seq_order: Vec<ItemId>,
    in_progress: HashSet<String>,
    nested: HashSet<ItemId>,
}

pub(crate) fn import(bytes: &[u8], opts: &ImportOptions, report: &mut Report) -> Result<Imported> {
    let text = decode_document(bytes)?;
    let doc = crate::xml::parse(&text).map_err(|e| Error::parse(Format::Prproj, e))?;
    let root = doc.root_element();
    if root.tag_name().name() != "PremiereData" {
        return Err(Error::parse(Format::Prproj, "root element is not <PremiereData>"));
    }
    let g = Graph::new(root);
    let project = elements(root).find(|n| n.has_tag_name("Project")).and_then(|n| g.deref(n));
    let name = opts.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| "Premiere Project".to_string());
    let mut imp = Imp {
        g,
        b: Builder::new(&name),
        base_dir: opts.base_dir.clone(),
        report,
        placement: HashMap::new(),
        order: Vec::new(),
        masters: HashMap::new(),
        seqs: HashMap::new(),
        seq_order: Vec::new(),
        in_progress: HashSet::new(),
        nested: HashSet::new(),
    };
    // 1. The Project panel: bins, and where each master clip lives.
    let root_item = project
        .and_then(|p| p.descendants().find(|d| d.has_tag_name("RootProjectItem") && (d.has_attribute("ObjectRef") || d.has_attribute("ObjectURef"))))
        .and_then(|r| imp.g.deref(r))
        .or_else(|| elements(root).find(|n| n.has_tag_name("RootProjectItem")));
    if let Some(r) = root_item {
        let mut seen = HashSet::new();
        imp.walk_bin(r, None, 0, &mut seen);
    }
    // 2. Every master clip in the panel, in panel order (sequences get built on the way).
    for key in imp.order.clone() {
        if let Some(mc) = imp.g.objs.get(key.as_str()).copied() {
            imp.master(mc);
        }
    }
    // 3. Every sequence, also those no bin lists.
    let seq_nodes: Vec<Node> = elements(root).filter(|n| n.has_tag_name("Sequence") && (n.has_attribute("ObjectID") || n.has_attribute("ObjectUID"))).collect();
    for s in seq_nodes {
        imp.sequence(s);
    }
    let top: Vec<ItemId> = imp.seq_order.iter().copied().filter(|s| !imp.nested.contains(s)).collect();
    imp.b.top = if top.is_empty() { imp.seq_order.clone() } else { top };
    if imp.b.p.items.is_empty() {
        return Err(Error::Empty);
    }
    Ok(imp.b.finish())
}

impl<'a, 'i> Imp<'a, 'i, '_> {
    fn walk_bin(&mut self, n: Node<'a, 'i>, bin: Option<BinId>, depth: usize, seen: &mut HashSet<String>) {
        for item in self.g.list(n, &["ProjectItemContainer", "Items"]) {
            if !seen.insert(object_key(item)) {
                continue;
            }
            let name = self.g.text_at(item, &["ProjectItem", "Name"]).unwrap_or("").to_string();
            let label = self.g.text_at(item, &["ProjectItem", "Node", "Properties", "Column.PropertyText.Label"]).and_then(label_of);
            match item.tag_name().name() {
                "BinProjectItem" => {
                    if depth >= MAX_DEPTH {
                        // Clips used in a sequence are still imported, at the top level.
                        self.report.warn(format!("bin \"{name}\" is nested more than {MAX_DEPTH} deep; its contents were not listed"));
                        continue;
                    }
                    let b = self.b.bin(if name.is_empty() { "Bin" } else { &name }, bin);
                    if label.is_some() {
                        self.report.info("bin labels are not kept (bins have no label here)");
                    }
                    self.walk_bin(item, Some(b), depth + 1, seen);
                }
                "ClipProjectItem" => {
                    if let Some(mc) = self.g.at(item, &["MasterClip"]) {
                        let key = object_key(mc);
                        if !self.placement.contains_key(&key) {
                            self.order.push(key.clone());
                            self.placement.insert(key, Placement { bin, name, label });
                        }
                    } else {
                        self.report.warn(format!("project item \"{name}\" has no master clip; skipped"));
                    }
                }
                other => self.report.warn(format!("project item \"{name}\" ({other}) is not supported; skipped")),
            }
        }
    }

    /// The sequence a clip plays, if its source is a sequence.
    fn clip_sequence(&self, clip: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
        let src = self.g.at(clip, &["Clip", "Source"])?;
        if !src.tag_name().name().ends_with("SequenceSource") {
            return None;
        }
        self.g.at(src, &["SequenceSource", "Sequence"])
    }

    /// The `Media` object a clip plays, and its source's `OriginalDuration`.
    fn clip_media(&self, clip: Node<'a, 'i>) -> Option<(Node<'a, 'i>, Option<i64>)> {
        let src = self.g.at(clip, &["Clip", "Source"])?;
        if !src.tag_name().name().ends_with("MediaSource") {
            return None;
        }
        Some((self.g.at(src, &["MediaSource", "Media"])?, child_text(src, "OriginalDuration").and_then(int)))
    }

    /// The project item for a master clip: media, a sequence, or an adjustment layer.
    fn master(&mut self, mc: Node<'a, 'i>) -> Option<ItemId> {
        let key = object_key(mc);
        if let Some(id) = self.masters.get(&key) {
            return *id;
        }
        self.masters.insert(key.clone(), None);
        let place = self.placement.get(&key).cloned();
        let name = place
            .as_ref()
            .map(|p| p.name.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| child_text(mc, "Name").filter(|n| !n.is_empty()).map(str::to_string))
            .unwrap_or_else(|| "Clip".to_string());
        let bin = place.as_ref().and_then(|p| p.bin);
        let label = place.as_ref().and_then(|p| p.label);
        let clips = self.g.list(mc, &["Clips"]);
        let id = if let Some(s) = clips.iter().find_map(|c| self.clip_sequence(*c)) {
            let id = self.sequence(s);
            if let Some(id) = id
                && bin.is_some()
            {
                self.b.p.move_to_bin(&[id], bin);
            }
            id
        } else if child_text(mc, "IsAdjustmentLayer") == Some("true") {
            let media = clips.iter().find_map(|c| self.clip_media(*c));
            let stream = media.and_then(|(m, _)| self.g.at(m, &["VideoStream"]));
            let (w, h) = stream.and_then(|v| child_text(v, "FrameRect")).and_then(rect).unwrap_or((1920, 1080));
            let rate = stream.and_then(|v| child_text(v, "FrameRate")).and_then(int).and_then(rate_of).unwrap_or_default();
            let duration = media.and_then(|(_, d)| d).filter(|d| *d > 0).map(tick).unwrap_or(Tick(TICKS_PER_SECOND * 3600));
            Some(self.b.p.add_item(&name, Label::Lavender, ItemKind::AdjustmentLayer { width: w, height: h, rate, duration }, bin))
        } else {
            self.media(&key, &clips, &name, bin)
        };
        if let (Some(id), Some(l)) = (id, label)
            && let Some(it) = self.b.p.item_mut(id)
        {
            it.label = l;
        }
        // Master clip markers: on the media item, in source time.
        if let Some(id) = id
            && self.b.p.item(id).and_then(|i| i.as_media()).is_some()
        {
            let shift = self.still_shift(id, &clips);
            let mk = clips.iter().find_map(|c| self.g.at(*c, &["Clip", "MarkerOwner", "Markers"]));
            let markers = self.markers(mk, shift);
            if let Some(m) = self.b.p.item_mut(id).and_then(|i| i.as_media_mut()) {
                m.markers = markers;
            }
        }
        self.masters.insert(key, id);
        id
    }

    /// A still's clips carry an arbitrary source in-point; ours start at 0. The offset to subtract.
    fn still_shift(&self, item: ItemId, clips: &[Node<'a, 'i>]) -> i64 {
        if !self.is_still(item) {
            return 0;
        }
        clips.iter().find_map(|c| self.g.text_at(*c, &["Clip", "InPoint"]).and_then(int)).unwrap_or(0)
    }

    fn is_still(&self, item: ItemId) -> bool {
        self.b.p.item(item).and_then(|i| i.as_media()).is_some_and(|m| m.info.kind == MediaKind::Still)
    }

    fn media(&mut self, key: &str, clips: &[Node<'a, 'i>], name: &str, bin: Option<BinId>) -> Option<ItemId> {
        let Some((m, orig)) = clips.iter().find_map(|c| self.clip_media(*c)) else {
            if clips.iter().any(|c| self.g.at(*c, &["Clip", "Source"]).is_some_and(|s| s.has_tag_name("DataMediaSource"))) {
                self.report.warn(format!("caption / data clip \"{name}\" is not supported; skipped"));
                return None;
            }
            self.report.warn(format!("clip \"{name}\" has no media; imported offline"));
            return Some(self.offline(&format!("mc:{key}"), name, None, bin));
        };
        let title = child_text(m, "Title").filter(|t| !t.is_empty()).unwrap_or(name).to_string();
        let video = self.g.at(m, &["VideoStream"]);
        let audio = self.g.at(m, &["AudioStream"]);
        let still = video.and_then(|v| child_text(v, "IsStill")) == Some("true") || child_text(m, "Infinite") == Some("true");
        let vrate = video.and_then(|v| child_text(v, "FrameRate")).and_then(int).and_then(rate_of);
        let mut spec = MediaSpec::default();
        if let Some(v) = video {
            let (w, h) = child_text(v, "FrameRect").and_then(rect).unwrap_or((1920, 1080));
            spec.video = Some((w, h, vrate.unwrap_or_default()));
        }
        if let Some(a) = audio {
            let sr = child_text(a, "FrameRate").and_then(int).and_then(sample_rate_of).unwrap_or(48_000);
            let ch = child_text(a, "AudioChannelLayout").map(|l| l.matches("channellabel").count()).filter(|c| *c > 0).unwrap_or(2);
            spec.audio = Some((sr, u32::try_from(ch.min(64)).unwrap_or(2)));
        }
        if still {
            spec.kind = Some(MediaKind::Still);
        } else {
            let durs = [video, audio].into_iter().flatten().filter_map(|s| child_text(s, "Duration").and_then(int));
            spec.duration = durs.chain(orig).filter(|d| *d > 0 && *d < Tick::MAX.0).max().map(tick);
            if let (Some(start), Some(rate)) = (child_text(m, "Start").and_then(int).filter(|s| *s > 0), vrate) {
                spec.start_tc = Some(rate.frame_at(tick(start)));
            }
        }
        let path = ["ActualMediaFilePath", "FilePath"].into_iter().find_map(|t| child_text(m, t).filter(|p| looks_like_path(p)));
        let mkey = format!("mc:{key}");
        let Some(path) = path else {
            // Synthetic media: Premiere's generators by title, anything else (graphics, titles) offline.
            let g = match title.to_ascii_lowercase().as_str() {
                "black video" => Some(Generator::BlackVideo),
                "transparent video" => Some(Generator::TransparentVideo),
                "color matte" | "colour matte" => Some(Generator::ColorMatte { color: [0.0, 0.0, 0.0, 1.0] }),
                "universal counting leader" => Some(Generator::CountingLeader),
                t if t.contains("bars and tone") => Some(Generator::BarsAndTone),
                _ => None,
            };
            if let Some(g) = g {
                if matches!(g, Generator::ColorMatte { .. }) {
                    self.report.warn(format!("colour matte \"{name}\": its colour is not imported (black)"));
                }
                return Some(self.b.generator_media(&mkey, name, g, &spec, bin));
            }
            self.report.warn(format!("graphic / synthetic clip \"{name}\" (titles, captions, Essential Graphics) is not supported; imported offline"));
            return Some(self.offline(&mkey, name, Some(&spec), bin));
        };
        let p = resolve_path(path, self.base_dir.as_deref());
        let id = self.b.file_media(&mkey, name, &p, &spec, bin);
        if child_text(m, "OfflineReason").and_then(int).is_some_and(|r| r != 0)
            && let Some(mc) = self.b.p.item_mut(id).and_then(|i| i.as_media_mut())
        {
            mc.offline = true;
        }
        Some(id)
    }

    /// An offline placeholder media item.
    fn offline(&mut self, key: &str, name: &str, spec: Option<&MediaSpec>, bin: Option<BinId>) -> ItemId {
        let spec = spec.cloned().unwrap_or_else(|| MediaSpec { video: Some((1920, 1080, FrameRate::default())), audio: Some((48_000, 2)), ..Default::default() });
        let p = resolve_path(name, self.base_dir.as_deref());
        let id = self.b.file_media(key, name, &p, &spec, bin);
        if let Some(m) = self.b.p.item_mut(id).and_then(|i| i.as_media_mut()) {
            m.offline = true;
        }
        id
    }

    fn markers(&mut self, markers: Option<Node<'a, 'i>>, shift: i64) -> Vec<Marker> {
        let Some(m) = markers else { return Vec::new() };
        let mut out = Vec::new();
        for entry in self.g.list(m, &["Markers"]) {
            let Some(obj) = self.g.at(entry, &["Second"]) else { continue };
            let parsed = child_text(obj, "DVAMarker").and_then(|j| serde_json::from_str::<Value>(j).ok());
            let Some(d) = parsed.as_ref().and_then(|v| v.get("DVAMarker")) else {
                self.report.warn("a marker in an unknown format was not imported");
                continue;
            };
            let Some(start) = d.get("mStartTime").and_then(ticks_json) else {
                self.report.warn("a marker without a time was not imported");
                continue;
            };
            let duration = d.get("mDuration").and_then(ticks_json).filter(|t| *t > 0).map(tick).unwrap_or(Tick::ZERO);
            let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let kind = match d.get("mType").and_then(Value::as_str).unwrap_or("Comment") {
                "Comment" => MarkerKind::Comment,
                "Chapter" => MarkerKind::Chapter,
                "Segmentation" => MarkerKind::Segmentation,
                "WebLink" => MarkerKind::WebLink,
                "Cue" | "FlashCuePoint" => MarkerKind::FlashCue,
                other => {
                    self.report.info(format!("marker type \"{other}\" imported as a comment marker"));
                    MarkerKind::Comment
                }
            };
            let color = d
                .get("mCuePointList")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|c| c.get("mValue").and_then(Value::as_str).and_then(|v| serde_json::from_str::<Value>(v).ok()))
                .find_map(|v| v.get("color").and_then(Value::as_u64))
                .map(label_near)
                .unwrap_or(Label::Green);
            out.push(Marker {
                id: MarkerId(self.b.alloc()),
                start: tick(start.saturating_sub(shift)),
                duration,
                name: s("mName"),
                comment: s("mComment"),
                kind,
                color,
            });
        }
        out.sort_by_key(|m| m.start);
        out
    }

    fn sequence(&mut self, n: Node<'a, 'i>) -> Option<ItemId> {
        let key = object_key(n);
        if let Some(id) = self.seqs.get(&key) {
            return Some(*id);
        }
        let name = child_text(n, "Name").filter(|s| !s.is_empty()).unwrap_or("Sequence").to_string();
        if self.in_progress.contains(&key) {
            self.report.warn(format!("sequence \"{name}\" contains itself; the recursive clip was skipped"));
            return None;
        }
        if self.in_progress.len() >= MAX_DEPTH {
            self.report.warn(format!("sequences are nested more than {MAX_DEPTH} deep; \"{name}\" was skipped"));
            return None;
        }
        self.in_progress.insert(key.clone());
        let groups: Vec<Node<'a, 'i>> = self.g.list(n, &["TrackGroups"]).into_iter().filter_map(|tg| self.g.at(tg, &["Second"])).collect();
        let group = |tag: &str| groups.iter().copied().find(|g| g.has_tag_name(tag));
        let (vg, ag, dg) = (group("VideoTrackGroup"), group("AudioTrackGroup"), group("DataTrackGroup"));
        let rate = vg.and_then(|g| self.g.text_at(g, &["TrackGroup", "FrameRate"])).and_then(int).and_then(rate_of);
        if rate.is_none() {
            self.report.warn(format!("sequence \"{name}\" has no readable frame rate; {} was assumed", FrameRate::default().label()));
        }
        let rate = rate.unwrap_or_default();
        let (w, h) = vg.and_then(|g| child_text(g, "FrameRect")).and_then(rect).unwrap_or((1920, 1080));
        let mut settings = settings_for(rate, w, h, false);
        if let Some(sr) = ag.and_then(|g| self.g.text_at(g, &["TrackGroup", "FrameRate"])).and_then(int).and_then(sample_rate_of) {
            settings.sample_rate = sr;
        }
        let id = self.b.reserve_sequence(&name, settings.clone(), None);
        self.seqs.insert(key.clone(), id);
        self.seq_order.push(id);
        let mut seq = empty_sequence(settings);
        if let Some(z) = self.g.text_at(n, &["Node", "Properties", "MZ.ZeroPoint"]).and_then(int).filter(|z| *z > 0) {
            seq.start_timecode = rate.frame_at(tick(z));
        }
        seq.markers = {
            let mk = self.g.at(n, &["MarkerOwner", "Markers"]);
            self.markers(mk, 0)
        };
        let mut clip_ids: HashMap<String, ClipId> = HashMap::new();
        for (kind, grp) in [(TrackKind::Video, vg), (TrackKind::Audio, ag)] {
            let Some(grp) = grp else { continue };
            for (ti, tn) in self.g.list(grp, &["TrackGroup", "Tracks"]).into_iter().enumerate() {
                self.b.ensure_tracks(&mut seq, kind, ti + 1);
                self.track(&mut seq, kind, ti, tn, (w, h), &mut clip_ids);
            }
        }
        if let Some(dg) = dg {
            let captions: usize = self.g.list(dg, &["TrackGroup", "Tracks"]).into_iter().map(|t| self.g.list(t, &["ClipTrack", "ClipItems", "TrackItems"]).len()).sum();
            if captions > 0 {
                self.report.warn(format!("{captions} caption clip(s) in sequence \"{name}\" were not imported"));
            }
        }
        if seq.video_tracks.is_empty() {
            self.b.ensure_tracks(&mut seq, TrackKind::Video, 1);
        }
        if seq.audio_tracks.is_empty() {
            self.b.ensure_tracks(&mut seq, TrackKind::Audio, 1);
        }
        // Linked clips (video with its audio).
        for link in self.g.list(n, &["PersistentGroupContainer", "LinkContainer", "Links"]) {
            let members: Vec<ClipId> = self.g.list(link, &["TrackItemGroup", "TrackItems"]).into_iter().filter_map(|t| clip_ids.get(&object_key(t)).copied()).collect();
            if members.len() < 2 {
                continue;
            }
            let l = self.b.link_id();
            for c in members {
                if let Some((_, it)) = seq.find_item_mut(c) {
                    it.link = Some(l);
                }
            }
        }
        self.b.put_sequence(id, seq);
        self.in_progress.remove(&key);
        Some(id)
    }

    fn track(&mut self, seq: &mut Sequence, kind: TrackKind, ti: usize, tn: Node<'a, 'i>, frame: (u32, u32), clip_ids: &mut HashMap<String, ClipId>) {
        if let Some(t) = seq.tracks_mut(kind).get_mut(ti) {
            t.locked = self.g.text_at(tn, &["ClipTrack", "Track", "IsLocked"]) == Some("true");
            t.enabled = self.g.text_at(tn, &["ClipTrack", "Track", "IsMuted"]) != Some("true");
        }
        for it in self.g.list(tn, &["ClipTrack", "ClipItems", "TrackItems"]) {
            let Some(item) = self.track_item(it, kind, frame) else { continue };
            clip_ids.insert(object_key(it), item.id);
            let overlaps = seq.tracks(kind).get(ti).is_some_and(|t| t.items.iter().any(|o| o.start < item.end() && item.start < o.end()));
            if overlaps {
                self.report.warn("overlapping clips on one track; the later one was moved to a new track");
                self.b.place(seq, kind, ti + 1, item);
            } else if let Some(t) = seq.tracks_mut(kind).get_mut(ti) {
                t.items.push(item);
            }
        }
        if let Some(t) = seq.tracks_mut(kind).get_mut(ti) {
            t.sort();
        }
        for tr in self.g.list(tn, &["ClipTrack", "TransitionItems", "TrackItems"]) {
            self.transition(seq, kind, ti, tr);
        }
    }

    fn track_item(&mut self, n: Node<'a, 'i>, kind: TrackKind, frame: (u32, u32)) -> Option<TrackItem> {
        let sub = self.g.at(n, &["ClipTrackItem", "SubClip"]);
        let name = sub.and_then(|s| child_text(s, "Name")).filter(|s| !s.is_empty()).unwrap_or("Clip").to_string();
        let start = self.g.text_at(n, &["ClipTrackItem", "TrackItem", "Start"]).and_then(int).unwrap_or(0);
        let Some(end) = self.g.text_at(n, &["ClipTrackItem", "TrackItem", "End"]).and_then(int) else {
            self.report.warn(format!("clip \"{name}\" has no end time; skipped"));
            return None;
        };
        let (start, end) = (tick(start), tick(end));
        if end <= start {
            self.report.warn(format!("clip \"{name}\" has no duration; skipped"));
            return None;
        }
        let clip = sub.and_then(|s| self.g.at(s, &["Clip"]));
        let master = sub.and_then(|s| self.g.at(s, &["MasterClip"]));
        let resolved = match master {
            Some(m) => self.master(m),
            None => clip.and_then(|c| self.clip_sequence(c)).and_then(|s| self.sequence(s)),
        };
        let item = match resolved {
            Some(i) => i,
            None => {
                self.report.warn(format!("clip \"{name}\" refers to media the project does not define; imported offline"));
                self.offline(&format!("offline:{name}"), &name, None, None)
            }
        };
        if self.b.p.item(item).is_some_and(|i| i.as_sequence().is_some()) {
            self.nested.insert(item);
        }
        let ctext = |path: &[&str]| clip.and_then(|c| self.g.text_at(c, path));
        let in_pt = ctext(&["Clip", "InPoint"]).and_then(int).unwrap_or(0);
        let raw_speed = ctext(&["Clip", "PlaybackSpeed"]).and_then(float).unwrap_or(1.0);
        let label = ctext(&["Clip", "Node", "Properties", "asl.clip.label.name"]).and_then(label_of);
        let remap = clip.and_then(|c| self.g.at(c, &["Clip", "TimeRemapping"])).and_then(|r| self.g.at(r, &["Keyframes"]));
        let gain = clip.and_then(|c| child_text(c, "Gain")).and_then(float);
        let shift = if self.is_still(item) { in_pt } else { 0 };
        let source_in = tick(in_pt.saturating_sub(shift).max(0));
        let mut ti = self.b.clip(item, kind, &name, start, end - start, source_in);
        if raw_speed != 0.0 {
            ti.speed = raw_speed.abs().min(10_000.0);
            ti.reverse = raw_speed < 0.0;
        }
        ti.enabled = self.g.text_at(n, &["ClipTrackItem", "IsMuted"]) != Some("true");
        if let Some(l) = label {
            ti.label = l;
        }
        if remap.is_some_and(animated) {
            self.report.warn(format!("variable speed (Time Remapping) on clip \"{name}\" is not supported; a constant speed was used"));
        }
        if kind == TrackKind::Audio
            && let Some(g) = gain.filter(|g| (g - 1.0).abs() > 1e-9)
        {
            ti.gain_db = gain_to_db(g);
        }
        let src_frame = self.source_frame(item).unwrap_or(frame);
        let chain = self.g.at(n, &["ClipTrackItem", "ComponentOwner", "Components"]);
        let comps = chain.map(|c| self.g.list(c, &["ComponentChain", "Components"])).unwrap_or_default();
        for c in comps {
            self.component(&mut ti, c, frame, src_frame, shift);
        }
        Some(ti)
    }

    fn source_frame(&self, item: ItemId) -> Option<(u32, u32)> {
        match &self.b.p.item(item)?.kind {
            ItemKind::Media(m) => m.info.video.as_ref().map(|v| (v.width, v.height)),
            ItemKind::Sequence(s) => Some((s.settings.width, s.settings.height)),
            ItemKind::AdjustmentLayer { width, height, .. } => Some((*width, *height)),
            _ => None,
        }
    }

    fn component(&mut self, ti: &mut TrackItem, c: Node<'a, 'i>, frame: (u32, u32), src: (u32, u32), shift: i64) {
        let video = c.has_tag_name("VideoFilterComponent");
        let comp = if video { self.g.at(c, &["Component"]) } else { self.g.at(c, &["AudioComponent", "Component"]) };
        let Some(comp) = comp else { return };
        let match_name = child_text(c, "MatchName").or_else(|| child_text(c, "FilterMatchName")).unwrap_or("").to_string();
        let display = child_text(comp, "DisplayName").filter(|d| !d.is_empty()).map(str::to_string).unwrap_or_else(|| match_name.clone());
        let enabled = child_text(comp, "Bypass") != Some("true");
        let params = self.g.list(comp, &["Params"]);
        let find = |id: i64, name: &str| {
            params
                .iter()
                .copied()
                .find(|p| id > 0 && child_text(*p, "ParameterID").and_then(int) == Some(id))
                .or_else(|| params.iter().copied().find(|p| child_text(*p, "Name").is_some_and(|n| n.trim().eq_ignore_ascii_case(name))))
        };
        let (fw, fh) = (f64::from(frame.0), f64::from(frame.1));
        let (sw, sh) = (f64::from(src.0), f64::from(src.1));
        match match_name.as_str() {
            "AE.ADBE Motion" => {
                if let Some(pr) = find(1, "Position").and_then(|p| point_param(p, shift, |x, y| Vec2::new(x * fw, y * fh)))
                    && moved(&pr, fw / 2.0, fh / 2.0)
                {
                    set_param(ti, "motion", "position", pr);
                }
                if let Some(pr) = find(2, "Scale").and_then(|p| float_param(p, shift, |v| v)) {
                    set_param(ti, "motion", "scale", pr);
                }
                if let Some(pr) = find(3, "Scale Width").and_then(|p| float_param(p, shift, |v| v)) {
                    set_param(ti, "motion", "scale_width", pr);
                }
                if let Some(pr) = find(4, "Uniform Scale").and_then(bool_param) {
                    set_param(ti, "motion", "uniform_scale", pr);
                }
                if let Some(pr) = find(5, "Rotation").and_then(|p| float_param(p, shift, |v| v)) {
                    set_param(ti, "motion", "rotation", pr);
                }
                if let Some(pr) = find(6, "Anchor Point").and_then(|p| point_param(p, shift, |x, y| Vec2::new(x * sw, y * sh)))
                    && moved(&pr, sw / 2.0, sh / 2.0)
                {
                    set_param(ti, "motion", "anchor", pr);
                }
                if let Some(pr) = find(7, "Anti-flicker Filter").and_then(|p| float_param(p, shift, |v| v)) {
                    set_param(ti, "motion", "anti_flicker", pr);
                }
                if let Some(m) = ti.effect_mut("motion") {
                    m.enabled = enabled;
                }
            }
            "AE.ADBE Opacity" => {
                if let Some(pr) = find(1, "Opacity").and_then(|p| float_param(p, shift, |v| v)) {
                    set_param(ti, "opacity", "opacity", pr);
                }
                let modes: Vec<i64> = params
                    .iter()
                    .filter(|p| child_text(**p, "Name").is_some_and(|n| n.trim().eq_ignore_ascii_case("Blend Mode")))
                    .filter_map(|p| start_fields(*p).first().copied().and_then(int))
                    .collect();
                if modes.first().is_some_and(|m| *m != BLEND_NORMAL) || modes.get(1).is_some_and(|m| *m != 0) {
                    self.report.warn(format!("blend mode on clip \"{}\" is not imported (Normal was used)", ti.name));
                }
                if let Some(m) = ti.effect_mut("opacity") {
                    m.enabled = enabled;
                }
            }
            m if m.starts_with("Internal Volume") => {
                if let Some(pr) = find(0, "Level").or_else(|| find(0, "Volume")).and_then(|p| float_param(p, shift, level_db)) {
                    set_param(ti, "volume", "level", pr);
                }
                if let Some(e) = ti.effect_mut("volume") {
                    e.enabled = enabled;
                }
            }
            m if m.starts_with("Internal Channel Volume") => {
                for (pname, id) in [("Left", "left"), ("Right", "right")] {
                    if let Some(pr) = find(0, pname).and_then(|p| float_param(p, shift, level_db))
                        && (pr.is_animated() || pr.value.as_f64().is_some_and(|v| v.abs() > 1e-6))
                    {
                        set_param(ti, "channel_volume", id, pr);
                    }
                }
                if let Some(e) = ti.effect_mut("channel_volume") {
                    e.enabled = enabled;
                }
            }
            "AE.ADBE MPEG.SourceSettings" => {
                self.report.info("MPEG Source Settings are not used (the source is decoded directly)");
            }
            _ => {
                let want = if video { EffectKind::Video } else { EffectKind::Audio };
                let Some(def) = find_effect_by_name(display.trim()).filter(|d| !d.intrinsic && d.kind == want) else {
                    self.report.warn(format!("effect \"{display}\" is not supported and was skipped"));
                    return;
                };
                let mut inst = def.instance();
                inst.enabled = enabled;
                let mut unmapped = false;
                for pd in &def.params {
                    let Some(p) = find(0, pd.label) else { continue };
                    let pr = match &pd.default {
                        ParamValue::Float(_) => float_param(p, shift, |v| v),
                        ParamValue::Bool(_) => bool_param(p),
                        _ => {
                            unmapped = true;
                            None
                        }
                    };
                    if let Some(pr) = pr {
                        inst.params.insert(pd.id.to_string(), pr);
                    }
                }
                if unmapped {
                    self.report.info(format!("effect \"{display}\": some parameters kept their defaults"));
                }
                ti.effects.push(inst);
            }
        }
    }

    fn transition(&mut self, seq: &mut Sequence, kind: TrackKind, ti: usize, n: Node<'a, 'i>) {
        let Some(tt) = self.g.at(n, &["TransitionTrackItem"]) else { return };
        let s = self.g.text_at(tt, &["TrackItem", "Start"]).and_then(int).map(tick);
        let e = self.g.text_at(tt, &["TrackItem", "End"]).and_then(int).map(tick);
        let raw = child_text(tt, "DisplayName").filter(|d| !d.is_empty()).or_else(|| child_text(tt, "MatchName")).unwrap_or("Cross Dissolve");
        let (Some(s), Some(e)) = (s, e) else {
            self.report.warn(format!("transition \"{raw}\" has no time range; skipped"));
            return;
        };
        if e <= s {
            self.report.warn(format!("transition \"{raw}\" has no duration; skipped"));
            return;
        }
        let name = raw.trim().trim_end_matches("(Legacy)").trim();
        let effect = transition_effect(name, kind == TrackKind::Audio, self.report);
        let len = e.0 - s.0;
        let cut = s.0 + child_text(tt, "Alignment").and_then(int).unwrap_or(len / 2).clamp(0, len);
        let has_out = child_text(tt, "HasOutgoingClip") != Some("false");
        let has_in = child_text(tt, "HasIncomingClip") != Some("false");
        let Some(track) = seq.tracks_mut(kind).get_mut(ti) else { return };
        let near = |t: Tick| t.0.saturating_sub(cut).saturating_abs();
        let from = if has_out { track.items.iter().filter(|i| i.end() >= s && i.end() <= e).min_by_key(|i| near(i.end())).map(|i| (i.id, i.end())) } else { None };
        let to = if has_in { track.items.iter().filter(|i| i.start >= s && i.start <= e).min_by_key(|i| near(i.start)).map(|i| (i.id, i.start)) } else { None };
        if from.is_none() && to.is_none() {
            self.report.warn(format!("transition \"{raw}\" has no adjacent clip; skipped"));
            return;
        }
        let at = from.map(|f| f.1).or(to.map(|t| t.1)).unwrap_or(Tick(cut));
        let align = if at <= s {
            TransitionAlign::StartAtCut
        } else if at >= e {
            TransitionAlign::EndAtCut
        } else {
            TransitionAlign::CenterAtCut
        };
        let id = TransitionId(self.b.alloc());
        track.transitions.push(Transition { id, effect, start: s, duration: e - s, from: from.map(|f| f.0), to: to.map(|t| t.0), align, reverse: false });
        track.sort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_values() {
        assert_eq!(rate_of(8_475_667_200), Some(FrameRate::FPS_29_97));
        assert_eq!(rate_of(10_594_584_000), Some(FrameRate::FPS_23_976));
        assert_eq!(rate_of(10_160_640_000), Some(FrameRate::FPS_25));
        assert_eq!(rate_of(0), None);
        assert_eq!(rate_of(-5), None);
        assert_eq!(sample_rate_of(5_292_000), Some(48_000));
        assert_eq!(sample_rate_of(5_760_000), Some(44_100));
        assert_eq!(sample_rate_of(0), None);
        assert_eq!(rect("0,0,1280,720"), Some((1280, 720)));
        assert_eq!(rect("0,0,-5,720"), None);
        assert_eq!(rect("garbage"), None);
        assert_eq!(label_of("BE.Prefs.LabelColors.5"), Some(Label::Forest));
        assert_eq!(label_of("BE.Prefs.LabelColors.99"), None);
        assert!((level_db(0.177_827_939_391) - 0.0).abs() < 1e-6);
        assert!((level_db(1.0) - 15.0).abs() < 1e-9);
        assert_eq!(level_db(0.0), -287.5);
        assert_eq!(int("100."), Some(100));
        assert_eq!(int("9223372036854775807"), Some(i64::MAX));
        assert_eq!(tick(i64::MAX), Tick::MAX);
    }

    #[test]
    fn sniffs_plain_and_gzip() {
        let xml = b"<?xml version=\"1.0\"?>\n<PremiereData Version=\"3\"></PremiereData>";
        assert!(sniff(xml));
        let mut gz = Vec::new();
        {
            use std::io::Write as _;
            let mut enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::fast());
            enc.write_all(xml).unwrap();
            enc.finish().unwrap();
        }
        assert!(sniff(&gz));
        assert_eq!(decode_document(&gz).unwrap(), String::from_utf8_lossy(xml));
        assert!(!sniff(b"<xmeml version=\"4\"/>"));
        // a truncated gzip stream is an error, not a panic
        assert!(decode_document(&gz[..gz.len() / 2]).is_err());
    }
}
