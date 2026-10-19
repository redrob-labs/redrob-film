//! Workflow acceptance over the real-world media corpus (docs/corpus.md, gaps.md G6).
//!
//! Every file of `tests/corpus/manifest.toml` that is present in the corpus directory
//! (`$REDROB_FILM_CORPUS`, else `<workspace>/target/corpus`) goes through one editing job driven
//! by the engine's commands: import, sequence from clip, razor in the middle, move the second
//! piece later (leaving a gap), apply an effect, export the first seconds. ffprobe / ffmpeg (an
//! external oracle only) then check the output: duration, decoded frame count, an audio stream
//! when the source had sound, and a clean full decode.
//!
//! Expectations come from the manifest: `pass` must succeed, `error` must fail with an error,
//! `report` only prints its outcome. A panic fails every expectation. Absent files are skipped
//! by name; without a corpus directory the test skips. The small tier is made by
//! `scripts/corpus.py generate --out target/corpus` and runs nightly
//! (`.github/workflows/nightly-corpus.yml`).
//!
//! Environment: `REDROB_FILM_CORPUS` (directory), `REDROB_FILM_CORPUS_SECONDS` (exported length,
//! default 3), `REDROB_FILM_CORPUS_ONLY` (comma-separated ids), `REDROB_FILM_CORPUS_KEEP=1` (keep
//! the exported files under `target/corpus-out`).

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use redrob_film_project::{ClipId, ItemId};
use redrob_film_time::Tick;
use serde_json::{Value, json};

use crate::Session;

/// Frames the second piece is moved later by.
const GAP_FRAMES: i64 = 10;
/// Default length of the exported range, seconds.
const DEFAULT_SECONDS: f64 = 3.0;

// ---------------------------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------------------------

/// A value of the manifest's TOML subset.
#[derive(Clone, Debug, PartialEq)]
enum TomlValue {
    Str(String),
    Int(i64),
    Bool(bool),
}

/// Parse the subset of TOML the manifest uses: `[[media]]` headers, `key = "basic string"`,
/// integers, booleans and `#` comments. Anything else is an error, never a panic.
fn parse_tables(text: &str) -> Result<Vec<HashMap<String, TomlValue>>, String> {
    let mut tables: Vec<HashMap<String, TomlValue>> = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let ln = n + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[media]]" {
            tables.push(HashMap::new());
            continue;
        }
        if line.starts_with('[') {
            return Err(format!("line {ln}: only [[media]] tables are read"));
        }
        let Some((key, rest)) = line.split_once('=') else {
            return Err(format!("line {ln}: expected `key = value`"));
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(format!("line {ln}: bad key `{key}`"));
        }
        let value = parse_value(rest.trim()).map_err(|e| format!("line {ln}: {e}"))?;
        let Some(table) = tables.last_mut() else {
            return Err(format!("line {ln}: `{key}` before the first [[media]]"));
        };
        if table.insert(key.to_string(), value).is_some() {
            return Err(format!("line {ln}: `{key}` given twice"));
        }
    }
    Ok(tables)
}

fn parse_value(v: &str) -> Result<TomlValue, String> {
    if let Some(body) = v.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = body.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    let tail = chars.as_str().trim();
                    if !(tail.is_empty() || tail.starts_with('#')) {
                        return Err(format!("text after the string: `{tail}`"));
                    }
                    return Ok(TomlValue::Str(out));
                }
                '\\' => match chars.next() {
                    Some('\\') => out.push('\\'),
                    Some('"') => out.push('"'),
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => return Err(format!("unsupported escape `\\{other}`")),
                    None => return Err("unterminated string".into()),
                },
                c => out.push(c),
            }
        }
        return Err("unterminated string".into());
    }
    let v = v.split_once('#').map_or(v, |(a, _)| a).trim();
    match v {
        "true" => Ok(TomlValue::Bool(true)),
        "false" => Ok(TomlValue::Bool(false)),
        _ => v.replace('_', "").parse::<i64>().map(TomlValue::Int).map_err(|_| format!("unsupported value `{v}`")),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    Pass,
    Error,
    Report,
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    id: String,
    kind: String,
    tier: String,
    file: String,
    video: bool,
    audio: bool,
    expect: Expect,
    issue: String,
}

fn entries(tables: &[HashMap<String, TomlValue>]) -> Result<Vec<Entry>, String> {
    let mut out: Vec<Entry> = Vec::with_capacity(tables.len());
    for (i, t) in tables.iter().enumerate() {
        let s = |k: &str| match t.get(k) {
            Some(TomlValue::Str(v)) => Some(v.clone()),
            _ => None,
        };
        let b = |k: &str| match t.get(k) {
            Some(TomlValue::Bool(v)) => Some(*v),
            _ => None,
        };
        let id = s("id").ok_or_else(|| format!("entry {i}: no `id`"))?;
        let need = |k: &str| s(k).ok_or_else(|| format!("{id}: no `{k}`"));
        let file = need("file")?;
        if file.is_empty() || Path::new(&file).components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(format!("{id}: `file` must be a relative path inside the corpus"));
        }
        let expect = match s("expect").as_deref() {
            Some("pass") => Expect::Pass,
            Some("error") => Expect::Error,
            Some("report") => Expect::Report,
            other => return Err(format!("{id}: `expect` is {other:?}, want pass | error | report")),
        };
        if out.iter().any(|e| e.id == id) {
            return Err(format!("{id}: duplicate id"));
        }
        out.push(Entry {
            kind: need("kind")?,
            tier: need("tier")?,
            video: b("video").ok_or_else(|| format!("{id}: no `video`"))?,
            audio: b("audio").ok_or_else(|| format!("{id}: no `audio`"))?,
            issue: s("issue").unwrap_or_default(),
            id,
            file,
            expect,
        });
    }
    Ok(out)
}

fn manifest_path() -> PathBuf {
    redrob_film_testkit::workspace_root().join("tests").join("corpus").join("manifest.toml")
}

fn load_manifest() -> Result<Vec<Entry>, String> {
    let path = manifest_path();
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    entries(&parse_tables(&text)?)
}

/// `$REDROB_FILM_CORPUS`, else `<workspace>/target/corpus` when it exists.
fn corpus_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("REDROB_FILM_CORPUS").filter(|v| !v.is_empty()) {
        let d = PathBuf::from(d);
        assert!(d.is_dir(), "REDROB_FILM_CORPUS={} is not a directory", d.display());
        return Some(d);
    }
    let d = redrob_film_testkit::workspace_root().join("target").join("corpus");
    d.is_dir().then_some(d)
}

// ---------------------------------------------------------------------------------------------
// The editing job
// ---------------------------------------------------------------------------------------------

struct Oracles {
    ffprobe: PathBuf,
    ffmpeg: Option<PathBuf>,
}

fn num(v: &Value) -> f64 {
    v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_f64()).unwrap_or(f64::NAN)
}

fn ffprobe_json(ffprobe: &Path, path: &Path) -> Result<Value, String> {
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-count_frames", "-of", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()
        .map_err(|e| format!("run ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(format!("ffprobe cannot read the output: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe JSON: {e}"))
}

/// Import → sequence from clip → razor → move → effect → export → check. `Ok` carries a short
/// description of the output; `Err` names the step that failed.
fn round_trip(e: &Entry, src: &Path, out_dir: &Path, seconds: f64, oracles: &Oracles) -> Result<String, String> {
    let mut s = Session::default();
    let r = s.execute("file.import", json!({"paths": [src.to_string_lossy().to_string()]})).map_err(|err| format!("import: {err}"))?;
    let item = r["items"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).ok_or_else(|| format!("import: no item ({r})"))?;
    let info = s.project.item(ItemId(item)).and_then(|it| it.as_media()).map(|m| m.info.clone()).ok_or("import: not a media item")?;
    let has_video = info.video.is_some();
    let has_audio = info.audio().is_some();
    if e.video && !has_video {
        return Err("import: no picture stream".into());
    }
    if e.audio && !has_audio {
        return Err(format!("import: no sound stream ({} audio streams)", info.audio_streams.len()));
    }

    s.execute("file.newSequenceFromClip", json!({"items": [item]})).map_err(|err| format!("sequence from clip: {err}"))?;
    let rate = s.sequence_rate();
    let (track, first) = {
        let q = s.active_sequence().ok_or("sequence from clip: no active sequence")?;
        let v = q.video_tracks.first().and_then(|t| t.items.first());
        let a = q.audio_tracks.first().and_then(|t| t.items.first());
        match (v, a) {
            (Some(v), _) => ("V1", v.clone()),
            (None, Some(a)) => ("A1", a.clone()),
            _ => return Err("sequence from clip: nothing placed".into()),
        }
    };

    // cut in the middle (at most a second in) and move the second piece later
    let frames = rate.frame_at(first.duration);
    if frames >= 2 {
        let cut_frame = (frames / 2).min(rate.frame_at(Tick::from_seconds_f64(1.0))).max(1);
        let cut = first.start + rate.tick_of(cut_frame);
        s.execute("timeline.razor", json!({"time": cut.0, "clip": first.id.0})).map_err(|err| format!("razor: {err}"))?;
        let right: ClipId = s
            .active_sequence()
            .and_then(|q| if track == "V1" { q.video_tracks.first() } else { q.audio_tracks.first() })
            .and_then(|t| t.items.iter().find(|i| i.start == cut))
            .map(|i| i.id)
            .ok_or("razor: no piece starts at the cut")?;
        let to = cut + rate.tick_of(GAP_FRAMES);
        s.execute("timeline.move", json!({"moves": [{"clip": right.0, "track": track, "time": to.0}], "linked": true}))
            .map_err(|err| format!("move: {err}"))?;
        let landed = s.active_sequence().and_then(|q| q.find_item(right)).map(|(_, it)| it.start);
        if landed != Some(to) {
            return Err(format!("move: the piece starts at {landed:?}, wanted {to:?}"));
        }
    }

    let effect = if track == "V1" { "gaussian_blur" } else { "volume_a" };
    s.execute("effects.apply", json!({"clips": [first.id.0], "effect": effect})).map_err(|err| format!("effect: {err}"))?;
    let applied = s.active_sequence().and_then(|q| q.find_item(first.id)).is_some_and(|(_, it)| it.effects.iter().any(|x| x.effect == effect));
    if !applied {
        return Err(format!("effect: {effect} is not on the clip"));
    }

    // export the first `seconds` of the sequence
    let duration = s.active_sequence().map(|q| q.duration()).ok_or("export: no sequence")?;
    let end = duration.seconds().min(seconds);
    let (format, ext) = if has_video { ("h264", "mp4") } else { ("wav", "wav") };
    let out = out_dir.join(format!("{}.{ext}", e.id));
    let params = json!({
        "format": format,
        "path": out.to_string_lossy().to_string(),
        "range": "custom",
        "startSeconds": 0.0,
        "endSeconds": end,
        "wait": true,
    });
    let want_frames = s.execute("export.resolve", params.clone()).ok().and_then(|r| r["frames"].as_i64());
    let r = s.execute("file.exportMedia", params).map_err(|err| format!("export: {err}"))?;
    let out = r["path"].as_str().map(PathBuf::from).unwrap_or(out);
    if !out.is_file() {
        return Err(format!("export: no file at {}", out.display()));
    }

    // check the output against the oracle
    let probe = ffprobe_json(&oracles.ffprobe, &out)?;
    let streams = probe["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|x| x["codec_type"] == "video");
    let audio = streams.iter().find(|x| x["codec_type"] == "audio");
    let fps = rate.num as f64 / rate.den.max(1) as f64;
    let frame_secs = if fps > 0.0 { 1.0 / fps } else { 0.04 };
    let mut desc = String::new();
    if has_video {
        let v = video.ok_or("output: no video stream")?;
        let got = num(&v["nb_read_frames"]);
        let want = want_frames.map_or((end * fps).round(), |f| f as f64);
        if !got.is_finite() || (got - want).abs() > 1.0 {
            return Err(format!("output: {got} decoded frames, want {want}"));
        }
        let secs = if num(&v["duration"]).is_finite() { num(&v["duration"]) } else { num(&probe["format"]["duration"]) };
        if !secs.is_finite() || (secs - end).abs() > frame_secs + 0.05 {
            return Err(format!("output: video lasts {secs:.3} s, want {end:.3} s"));
        }
        desc.push_str(&format!("{got} frames, {secs:.3} s"));
    } else {
        let secs = num(&probe["format"]["duration"]);
        if !secs.is_finite() || (secs - end).abs() > 0.02 {
            return Err(format!("output: audio lasts {secs:.3} s, want {end:.3} s"));
        }
        desc.push_str(&format!("{secs:.3} s"));
    }
    if has_audio {
        let a = audio.ok_or("output: the source has sound but the export has no audio stream")?;
        desc.push_str(&format!(", audio {} {} ch", a["codec_name"].as_str().unwrap_or("?"), a["channels"]));
    }
    if let Some(ffmpeg) = &oracles.ffmpeg {
        let d = Command::new(ffmpeg).args(["-v", "error", "-i"]).arg(&out).args(["-f", "null", "-"]).output().map_err(|e| format!("run ffmpeg: {e}"))?;
        let errs = String::from_utf8_lossy(&d.stderr);
        if !d.status.success() || !errs.trim().is_empty() {
            return Err(format!("output does not decode cleanly: {}", errs.trim()));
        }
    }
    Ok(desc)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Pass,
    Fail,
    XFail,
    XPass,
}

/// Run one entry under `catch_unwind` and judge the outcome against its expectation.
fn judge(e: &Entry, src: &Path, out_dir: &Path, seconds: f64, oracles: &Oracles) -> (Verdict, String) {
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| round_trip(e, src, out_dir, seconds, oracles)));
    match (e.expect, run) {
        (_, Err(p)) => {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            (Verdict::Fail, format!("PANIC {msg}"))
        }
        (Expect::Pass, Ok(Ok(d))) => (Verdict::Pass, d),
        (Expect::Pass, Ok(Err(err))) => (Verdict::Fail, err),
        (Expect::Error, Ok(Err(err))) => (Verdict::Pass, format!("refused: {err}")),
        (Expect::Error, Ok(Ok(d))) => (Verdict::Fail, format!("expected an error, got {d}")),
        (Expect::Report, Ok(Ok(d))) => (Verdict::XPass, d),
        (Expect::Report, Ok(Err(err))) => (Verdict::XFail, err),
    }
}

#[test]
fn corpus_round_trip() {
    let entries = load_manifest().unwrap();
    let Some(dir) = corpus_dir() else {
        eprintln!("SKIPPED (corpus_round_trip): no corpus; set REDROB_FILM_CORPUS or run scripts/corpus.py generate --out target/corpus");
        return;
    };
    let Some(ffprobe) = redrob_film_testkit::ffprobe_or_skip("corpus_round_trip") else { return };
    let oracles = Oracles { ffprobe, ffmpeg: redrob_film_testkit::ffmpeg() };
    let seconds = std::env::var("REDROB_FILM_CORPUS_SECONDS").ok().and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(DEFAULT_SECONDS);
    let only: Vec<String> = std::env::var("REDROB_FILM_CORPUS_ONLY").map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    let keep = std::env::var("REDROB_FILM_CORPUS_KEEP").is_ok_and(|v| v == "1");
    let out_root = redrob_film_testkit::workspace_root().join("target").join("corpus-out").join(format!("run-{}", std::process::id()));
    std::fs::create_dir_all(&out_root).unwrap();

    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut failures = Vec::new();
    for e in &entries {
        if !only.is_empty() && !only.contains(&e.id) {
            continue;
        }
        let src = dir.join(&e.file);
        if !src.is_file() {
            *counts.entry("SKIP").or_default() += 1;
            continue;
        }
        let out_dir = out_root.join(&e.id);
        std::fs::create_dir_all(&out_dir).unwrap();
        let (verdict, detail) = judge(e, &src, &out_dir, seconds, &oracles);
        let tag = match verdict {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::XFail => "XFAIL",
            Verdict::XPass => "XPASS",
        };
        *counts.entry(tag).or_default() += 1;
        let issue = if e.issue.is_empty() { String::new() } else { format!(" [{}]", e.issue) };
        println!("{tag:<5} {:<36} {:<10} {:<5} {detail}{issue}", e.id, e.kind, e.tier);
        if verdict == Verdict::Fail {
            failures.push(format!("{}: {detail}", e.id));
        }
        if !keep {
            let _ = std::fs::remove_dir_all(&out_dir);
        }
    }
    if !keep {
        let _ = std::fs::remove_dir_all(&out_root);
    }
    let c = |k: &str| counts.get(k).copied().unwrap_or(0);
    println!(
        "corpus {}: {} pass, {} fail, {} xfail, {} xpass, {} absent (of {} entries)",
        dir.display(),
        c("PASS"),
        c("FAIL"),
        c("XFAIL"),
        c("XPASS"),
        c("SKIP"),
        entries.len()
    );
    assert!(failures.is_empty(), "{} corpus files failed:\n{}", failures.len(), failures.join("\n"));
}

// ---------------------------------------------------------------------------------------------
// Manifest tests (run everywhere, no media needed)
// ---------------------------------------------------------------------------------------------

#[test]
fn corpus_manifest_is_valid_and_large_enough() {
    let entries = load_manifest().unwrap();
    assert!(entries.len() >= 50, "{} entries", entries.len());
    for kind in ["phone", "camera", "cinema", "screen", "audio", "container", "multitrack", "damaged"] {
        assert!(entries.iter().any(|e| e.kind == kind), "no {kind} entry");
    }
    for e in &entries {
        assert!(matches!(e.tier.as_str(), "small" | "large" | "real"), "{}: tier {}", e.id, e.tier);
        assert!(e.video || e.audio || e.kind == "damaged", "{}: neither picture nor sound", e.id);
    }
    let mut files: Vec<&str> = entries.iter().map(|e| e.file.as_str()).collect();
    files.sort_unstable();
    let n = files.len();
    files.dedup();
    assert_eq!(files.len(), n, "two entries share a file");
}

#[test]
fn corpus_manifest_parser_reads_the_subset() {
    let t = parse_tables("# c\n[[media]]\nid = \"a\" # note\nn = 1_000\nok = true\ns = \"x\\\\,y \\\"q\\\"\"\n\n[[media]]\nid = \"b\"\n").unwrap();
    assert_eq!(t.len(), 2);
    assert_eq!(t[0]["id"], TomlValue::Str("a".into()));
    assert_eq!(t[0]["n"], TomlValue::Int(1000));
    assert_eq!(t[0]["ok"], TomlValue::Bool(true));
    assert_eq!(t[0]["s"], TomlValue::Str("x\\,y \"q\"".into()));
    assert_eq!(t[1]["id"], TomlValue::Str("b".into()));
}

#[test]
fn corpus_manifest_parser_rejects_hostile_input_without_panicking() {
    for text in [
        "id = \"before any table\"",
        "[[media]]\nid = \"unterminated",
        "[[media]]\nid = \"a\" trailing",
        "[[media]]\nid = \"bad \\q escape\"",
        "[[media]]\nid = \"ends in backslash\\",
        "[[media]]\nn = 99999999999999999999999",
        "[[media]]\nlist = [1, 2]",
        "[[media]]\n= \"no key\"",
        "[[media]]\nno equals sign",
        "[[media]]\nid = \"a\"\nid = \"b\"",
        "[table]\nid = \"a\"",
        "[[media]]\nid = \"\u{e9}\u{301}\"\u{1f600}",
    ] {
        let r = std::panic::catch_unwind(|| parse_tables(text));
        assert!(r.is_ok(), "panicked on {text:?}");
        assert!(r.unwrap().is_err(), "accepted {text:?}");
    }
    let base = "[[media]]\nid = \"a\"\nkind = \"phone\"\ntier = \"small\"\nvideo = true\naudio = true\nexpect = \"pass\"\n";
    for file in ["/etc/passwd", "../x.mp4", "a/../../x", ""] {
        let t = parse_tables(&format!("{base}file = \"{file}\"\n")).unwrap();
        assert!(entries(&t).is_err(), "accepted file {file:?}");
    }
    let t = parse_tables(&format!("{base}file = \"phone/a.mov\"\n")).unwrap();
    assert_eq!(entries(&t).unwrap()[0].file, "phone/a.mov");
    let t = parse_tables(&format!("{}file = \"x\"\n", base.replace("pass", "maybe"))).unwrap();
    assert!(entries(&t).is_err());
}
