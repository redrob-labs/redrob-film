//! `scene_build`: one description in, one finished Effects composition out.
//!
//! An agent that builds motion graphics a command at a time spends its effort on plumbing (layer
//! ids, property paths, key times) and ships the generic look: linear moves, opacity-only fades,
//! everything entering at once, flat frames. The description here is in design terms instead (a
//! role per text, an entrance, a look), and [`plan`] turns it into one `engine.batch` of Effects
//! commands that applies the craft rules of [`GUIDE`] by default: entrances move two or three
//! properties with a strong ease-out, layers stagger, exits are short and eased in, the hero alone
//! glows, footage drifts (Ken Burns), and a grade layer adds vignette and grain on top. Text stays
//! inside the 9:16 safe zone (top 14 %, bottom 35 % are covered by platform UI).
//!
//! Layer stack, bottom to top: background (footage sent from Edit, or a solid / gradient), a dark
//! wash for contrast, the graphics in the order given, the grade.

use serde_json::{Value, json};

/// The craft guide `motion_guide` returns (and the defaults [`plan`] applies).
pub const GUIDE: &str = include_str!("guide.md");

/// Text roles: size, style, tracking (1/1000 em), default colour key and whether it glows.
struct Role {
    size: f64,
    style: &'static str,
    tracking: f64,
    hero: bool,
    caps: bool,
}

fn role(name: &str) -> Role {
    match name {
        "hero" => Role { size: 160.0, style: "Bold", tracking: -10.0, hero: true, caps: false },
        "title" => Role { size: 96.0, style: "Bold", tracking: -10.0, hero: false, caps: false },
        "label" => Role { size: 34.0, style: "Bold", tracking: 220.0, hero: false, caps: true },
        "cta" => Role { size: 56.0, style: "Bold", tracking: 40.0, hero: false, caps: false },
        _ => Role { size: 54.0, style: "Regular", tracking: 0.0, hero: false, caps: false },
    }
}

/// The text presets of the Effects engine (`layer.applyTextPreset`).
const TEXT_PRESETS: [&str; 8] = ["typewriter", "fadeUpCharacters", "bounceInWords", "trackingIn", "scramble", "blurIn", "jitter", "dropInLines"];

/// The 9:16 safe band for text, as fractions of the height.
const SAFE_TOP: f64 = 0.14;
const SAFE_BOTTOM: f64 = 0.65;

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite())
}
fn text(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

/// The scene's frame, timing and look, with the defaults filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
}

impl Frame {
    pub fn of(spec: &Value) -> Self {
        Self {
            width: num(spec, "width").map_or(1080, |w| w.clamp(16.0, 8192.0) as u32),
            height: num(spec, "height").map_or(1920, |h| h.clamp(16.0, 8192.0) as u32),
            fps: num(spec, "fps").map_or(30.0, |f| f.clamp(1.0, 120.0)),
            duration: num(spec, "duration").map_or(4.0, |d| d.clamp(0.5, 120.0)),
        }
    }
}

/// Batch steps that build the scene into the open composition, whose footage layers (already in
/// it, bottom of the stack) are `footage` with their current uniform scale. Each step is
/// `{command, params}`; `"$N.layer"` refers to step N's new layer (engine.batch, 1-based).
pub fn plan(spec: &Value, frame: &Frame, footage: &[(u64, f64)]) -> Result<Vec<Value>, String> {
    let mut steps: Vec<Value> = Vec::new();
    let push = |steps: &mut Vec<Value>, command: &str, params: Value| -> String {
        steps.push(json!({"command": command, "params": params}));
        format!("${}.layer", steps.len())
    };
    let (w, h, dur) = (f64::from(frame.width), f64::from(frame.height), frame.duration);
    let look = spec.get("look").cloned().unwrap_or(json!({}));
    let theme = spec.get("theme").cloned().unwrap_or(json!({}));
    let ink = text(&theme, "text").unwrap_or_else(|| "#ffffff".into());
    let hero_color = text(&theme, "hero").unwrap_or_else(|| "#ffc46b".into());
    let font = text(&theme, "font");

    // 1. background: footage drifts; without footage a solid or a vertical gradient
    let drift = num(&look, "kenBurns").unwrap_or(6.0).clamp(0.0, 30.0) / 100.0;
    for (layer, scale) in footage {
        if drift > 0.0 {
            for (t, s) in [(0.0, *scale), (dur, scale * (1.0 + drift))] {
                steps.push(json!({"command": "prop.addKey", "params": {"layer": layer, "path": "transform/scale", "time": t, "timeBase": "comp", "value": [s, s, 100.0]}}));
            }
            steps.push(json!({"command": "keys.setEase", "params": {"layer": layer, "path": "transform/scale", "time": 0.0, "side": "out", "influence": 40}}));
        }
    }
    if footage.is_empty() {
        match spec.get("background") {
            Some(Value::Array(c)) if c.len() >= 2 => {
                let bg = push(&mut steps, "layer.newSolid", json!({"name": "Background", "color": "#000000", "width": frame.width, "height": frame.height}));
                steps.push(json!({"command": "effect.apply", "params": {"effect": "Gradient Ramp", "layers": [bg]}}));
                for (k, v) in [
                    ("effects/#1/start", json!([w / 2.0, 0.0])),
                    ("effects/#1/startColor", json!(c[0])),
                    ("effects/#1/end", json!([w / 2.0, h])),
                    ("effects/#1/endColor", json!(c[1])),
                ] {
                    steps.push(json!({"command": "prop.set", "params": {"layer": bg, "path": k, "value": v}}));
                }
            }
            other => {
                let c = other.and_then(Value::as_str).unwrap_or("#0b0b10");
                push(&mut steps, "layer.newSolid", json!({"name": "Background", "color": c, "width": frame.width, "height": frame.height}));
            }
        }
    }
    // 2. a dark wash over footage so type reads
    let darken = num(&look, "darken").unwrap_or(if footage.is_empty() { 0.0 } else { 0.28 }).clamp(0.0, 0.9);
    if darken > 0.0 {
        let wash = push(&mut steps, "layer.newSolid", json!({"name": "Wash", "color": "#000000", "width": frame.width, "height": frame.height}));
        steps.push(json!({"command": "prop.set", "params": {"layer": wash, "path": "transform/opacity", "value": darken * 100.0}}));
    }

    // 3. graphics, staggered
    let layers = spec.get("layers").and_then(Value::as_array).cloned().unwrap_or_default();
    if layers.is_empty() {
        return Err("layers: give at least one layer, e.g. {\"type\":\"text\",\"text\":\"Title\",\"role\":\"hero\"}".into());
    }
    let stagger = num(spec, "stagger").unwrap_or(0.14).clamp(0.0, 2.0);
    let first = num(spec, "start").unwrap_or(0.15).clamp(0.0, dur);
    let n = layers.len() as f64;
    for (i, l) in layers.iter().enumerate() {
        let kind = text(l, "type").unwrap_or_else(|| "text".into());
        let r = role(&text(l, "role").unwrap_or_else(|| if i == 0 { "hero".into() } else { "body".into() }));
        // default rows: centred on 0.42 of the height, one row per layer
        let row = 0.42 + (i as f64 - (n - 1.0) / 2.0) * 0.065;
        let y = (num(l, "y").unwrap_or(row)).clamp(SAFE_TOP, SAFE_BOTTOM) * h;
        let x = num(l, "x").unwrap_or(0.5).clamp(0.06, 0.94) * w;
        let t_in = num(l, "in").unwrap_or(first + i as f64 * stagger).clamp(0.0, dur);
        let t_out = num(l, "out").unwrap_or(dur).clamp(t_in + 0.1, dur);
        let color = text(l, "color").unwrap_or_else(|| if r.caps || kind != "text" { hero_color.clone() } else { ink.clone() });
        let id = match kind.as_str() {
            "bar" | "box" => {
                let size =
                    [num(l, "width").unwrap_or(if kind == "bar" { 240.0 } else { 520.0 }), num(l, "height").unwrap_or(if kind == "bar" { 8.0 } else { 110.0 })];
                push(
                    &mut steps,
                    "layer.newShape",
                    json!({"kind": if kind == "bar" { "rect" } else { "rounded" }, "size": size, "fill": color, "position": [x, y]}),
                )
            }
            _ => {
                let mut content = text(l, "text").unwrap_or_default();
                if r.caps {
                    content = content.to_uppercase();
                }
                let mut p = json!({
                    "text": content,
                    "size": num(l, "size").unwrap_or(r.size),
                    "style": text(l, "style").unwrap_or_else(|| r.style.into()),
                    "tracking": num(l, "tracking").unwrap_or(r.tracking),
                    "fill": color,
                    "justify": "center",
                    "position": [x, y],
                });
                if let Some(f) = &font {
                    p["font"] = json!(f);
                }
                push(&mut steps, "layer.newText", p)
            }
        };
        steps.push(json!({"command": "layer.timing", "params": {"layers": [id], "start": t_in, "in": t_in, "out": t_out}}));
        let enter = text(l, "enter").unwrap_or_else(|| if kind == "text" { "rise".into() } else { "grow".into() });
        let len = num(l, "enterDuration").unwrap_or(0.55).clamp(0.1, 3.0);
        let key = |steps: &mut Vec<Value>, path: &str, t: f64, v: Value| {
            steps.push(json!({"command": "prop.addKey", "params": {"layer": id, "path": path, "time": t, "value": v}}));
        };
        let ease = |steps: &mut Vec<Value>, path: &str, t: f64, side: &str, influence: f64| {
            steps.push(json!({"command": "keys.setEase", "params": {"layer": id, "path": path, "time": t, "side": side, "influence": influence}}));
        };
        match enter.as_str() {
            "none" => {}
            p if TEXT_PRESETS.contains(&p) && kind == "text" => {
                steps.push(json!({"command": "layer.applyTextPreset", "params": {"layer": id, "preset": p}}));
            }
            "grow" => {
                key(&mut steps, "transform/scale", 0.0, json!([0.0, 100.0, 100.0]));
                key(&mut steps, "transform/scale", len, json!([100.0, 100.0, 100.0]));
                ease(&mut steps, "transform/scale", 0.0, "out", 85.0);
                ease(&mut steps, "transform/scale", len, "in", 90.0);
            }
            other => {
                // rise / drop / slide / pop / fade: two or three properties, strong ease-out
                let (dx, dy, s0) = match other {
                    "drop" => (0.0, -70.0, 96.0),
                    "slide" => (-90.0, 0.0, 100.0),
                    "pop" => (0.0, 0.0, 70.0),
                    "fade" => (0.0, 0.0, 100.0),
                    _ => (0.0, 70.0, 94.0),
                };
                key(&mut steps, "transform/opacity", 0.0, json!(0.0));
                key(&mut steps, "transform/opacity", len * 0.7, json!(100.0));
                if dx != 0.0 || dy != 0.0 {
                    key(&mut steps, "transform/position", 0.0, json!([x + dx, y + dy, 0.0]));
                    key(&mut steps, "transform/position", len, json!([x, y, 0.0]));
                    ease(&mut steps, "transform/position", 0.0, "out", 85.0);
                    ease(&mut steps, "transform/position", len, "in", 90.0);
                }
                if (s0 - 100.0f64).abs() > f64::EPSILON {
                    key(&mut steps, "transform/scale", 0.0, json!([s0, s0, 100.0]));
                    key(&mut steps, "transform/scale", len, json!([100.0, 100.0, 100.0]));
                    ease(&mut steps, "transform/scale", 0.0, "out", 85.0);
                    ease(&mut steps, "transform/scale", len, "in", if other == "pop" { 40.0 } else { 90.0 });
                }
            }
        }
        // exits are shorter than entrances and ease in
        let exit = text(l, "exit").unwrap_or_else(|| "fade".into());
        let life = t_out - t_in;
        if exit != "none" && t_out >= dur - 1e-6 && life > len + 0.5 {
            let (a, b) = (life - 0.3, life);
            key(&mut steps, "transform/opacity", a, json!(100.0));
            key(&mut steps, "transform/opacity", b, json!(0.0));
            ease(&mut steps, "transform/opacity", a, "out", 70.0);
            if exit == "fall" && kind == "text" {
                key(&mut steps, "transform/position", a, json!([x, y, 0.0]));
                key(&mut steps, "transform/position", b, json!([x, y + 30.0, 0.0]));
            }
        }
        // the hero alone glows
        let glow = l.get("glow").and_then(Value::as_bool).unwrap_or(r.hero && kind == "text");
        if glow {
            steps.push(json!({"command": "effect.apply", "params": {"effect": "Glow", "layers": [id]}}));
            for (k, v) in [("effects/#1/threshold", json!(45.0)), ("effects/#1/radius", json!(36.0)), ("effects/#1/intensity", json!(0.7))] {
                steps.push(json!({"command": "prop.set", "params": {"layer": id, "path": k, "value": v}}));
            }
        }
    }

    // 4. the grade on top: vignette and grain
    let vignette = num(&look, "vignette").unwrap_or(45.0).clamp(0.0, 100.0);
    let grain = num(&look, "grain").unwrap_or(4.0).clamp(0.0, 100.0);
    if vignette > 0.0 || grain > 0.0 {
        let grade = push(&mut steps, "layer.newAdjustment", json!({"name": "Grade"}));
        let mut slot = 0;
        if vignette > 0.0 {
            slot += 1;
            steps.push(json!({"command": "effect.apply", "params": {"effect": "CC Vignette", "layers": [grade]}}));
            steps.push(json!({"command": "prop.set", "params": {"layer": grade, "path": format!("effects/#{slot}/amount"), "value": vignette}}));
        }
        if grain > 0.0 {
            slot += 1;
            steps.push(json!({"command": "effect.apply", "params": {"effect": "Noise", "layers": [grade]}}));
            steps.push(json!({"command": "prop.set", "params": {"layer": grade, "path": format!("effects/#{slot}/amount"), "value": grain}}));
        }
    }
    Ok(steps)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn cmds(steps: &[Value]) -> Vec<&str> {
        steps.iter().map(|s| s["command"].as_str().unwrap()).collect()
    }

    #[test]
    fn a_title_card_follows_the_craft_rules() {
        let spec = json!({"background": ["#14121f", "#000000"], "layers": [
            {"text": "SINTEL", "role": "hero"},
            {"text": "a short film", "role": "body"},
            {"type": "bar"},
        ]});
        let steps = plan(&spec, &Frame::of(&spec), &[]).unwrap();
        let c = cmds(&steps);
        assert_eq!(c[0], "layer.newSolid", "background first");
        assert!(c.contains(&"effect.apply"), "gradient, glow, grade");
        assert_eq!(c.iter().filter(|x| **x == "layer.newText").count(), 2);
        assert_eq!(*c.last().unwrap(), "prop.set", "the grade's settings end the batch");
        assert!(c.iter().rposition(|x| *x == "layer.newAdjustment").unwrap() > c.iter().rposition(|x| *x == "layer.newShape").unwrap(), "grade on top");
        // staggered entrances: each layer starts later than the one before
        let starts: Vec<f64> = steps.iter().filter(|s| s["command"] == "layer.timing").map(|s| s["params"]["start"].as_f64().unwrap()).collect();
        assert!(starts.windows(2).all(|w| w[1] > w[0]), "{starts:?}");
        // no linear moves: every position key pair gets an ease
        let eases = steps.iter().filter(|s| s["command"] == "keys.setEase").count();
        assert!(eases >= 4, "{eases}");
        // only the hero glows
        let glows = steps.iter().filter(|s| s["command"] == "effect.apply" && s["params"]["effect"] == "Glow").count();
        assert_eq!(glows, 1);
    }

    #[test]
    fn text_stays_in_the_safe_band_and_footage_drifts() {
        let spec = json!({"duration": 3, "layers": [{"text": "TOP", "y": 0.0}, {"text": "LOW", "y": 0.99}]});
        let steps = plan(&spec, &Frame::of(&spec), &[(7, 234.0)]).unwrap();
        let ys: Vec<f64> = steps.iter().filter(|s| s["command"] == "layer.newText").map(|s| s["params"]["position"][1].as_f64().unwrap()).collect();
        assert!((ys[0] - 0.14 * 1920.0).abs() < 1e-6 && (ys[1] - 0.65 * 1920.0).abs() < 1e-6, "{ys:?}");
        let drift: Vec<&Value> = steps.iter().filter(|s| s["command"] == "prop.addKey" && s["params"]["layer"] == 7).collect();
        assert_eq!(drift.len(), 2);
        assert!(drift[1]["params"]["value"][0].as_f64().unwrap() > 234.0);
        assert!(cmds(&steps).contains(&"layer.newSolid"), "the wash over footage");
    }

    #[test]
    fn presets_are_used_by_name_and_empty_scenes_are_refused() {
        let spec = json!({"layers": [{"text": "Hi", "enter": "trackingIn"}]});
        let steps = plan(&spec, &Frame::of(&spec), &[]).unwrap();
        assert!(steps.iter().any(|s| s["command"] == "layer.applyTextPreset" && s["params"]["preset"] == "trackingIn"));
        assert!(plan(&json!({}), &Frame::of(&json!({})), &[]).is_err());
        assert!(GUIDE.contains("ease"), "the guide ships");
    }
}
