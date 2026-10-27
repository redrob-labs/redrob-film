//! Effect stack extras (Kdenlive parity): favourite effects, the Effects panel's filters and
//! description pane, Sequence ▸ Add Effect…, the clip menu's Insert an Effect / Effects Enabled,
//! the keyframe clipboard (Ctrl+C / Ctrl+V in Effect Controls), Import Keyframes…, automatic
//! keyframes while playing, and the expression evaluation the value fields use.
//!
//! Automation ids: `addEffect.search`, `addEffect.item.<effect>`, `addEffect.apply`,
//! `importKeyframes.text`, `importKeyframes.ok`, `timeline.clipMenu.insertEffect.<effect>`,
//! `timeline.clipMenu.effectsEnabled`.

use redrob_film_project::{EffectKind, Keyframe, TrackKind};
use redrob_film_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::FilmApp;
use crate::dock::PanelKind;

/// Saved with the interface state.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FxStackUi {
    /// Favourite effects (ids), in the order they were added.
    pub favorites: Vec<String>,
    /// Effects panel filters: only GPU-accelerated / 32-bit (high bit depth) / YUV effects.
    pub only_accelerated: bool,
    pub only_float32: bool,
    pub only_yuv: bool,
    /// Effects panel: the description pane under the list, and the effect it describes.
    pub show_description: bool,
    pub described: Option<String>,
    /// Timeline: draw the keyframes of a clip's effects on the clip.
    pub show_clip_keyframes: bool,
    /// Automatic keyframes: a value changed while playing writes a keyframe at the playhead.
    pub auto_keyframes: bool,
    /// Automatic keyframes at most every [`AUTO_KEYFRAME_FRAMES`] frames.
    pub limit_auto_keyframes: bool,
    /// Program monitor Comparison View: compare the frame with and without the selected clip's
    /// effects instead of with another frame.
    pub compare_effects: bool,
    /// Sequence ▸ Add Effect… dialog (open when Some): its search text.
    #[serde(skip)]
    pub add_effect: Option<String>,
    /// Import Keyframes… dialog (open when Some).
    #[serde(skip)]
    pub import: Option<ImportDraft>,
    /// Keyframes copied in Effect Controls (Ctrl+C), pasted at the playhead with Ctrl+V.
    #[serde(skip)]
    pub clipboard: Vec<CopiedKeyframes>,
    /// The text of the last keyframe export, offered by Import Keyframes….
    #[serde(skip)]
    pub last_export: String,
    /// Automatic keyframes: the last one written per parameter (`clip:effect:param` → timeline ticks).
    #[serde(skip)]
    pub last_auto: Vec<(String, i64)>,
    /// Graphic layers copied on the Program monitor / in Essential Graphics (`graphics.copyLayers`)
    /// and how many times they were pasted (each paste lands further down and right).
    #[serde(skip)]
    pub layer_clipboard: Option<Value>,
    #[serde(skip)]
    pub layer_pastes: u32,
    /// Graphic patterns (Kdenlive's title patterns): named groups of layers saved from a graphic
    /// and inserted into others (Graphics and Titles ▸ Patterns…).
    pub patterns: Vec<(String, Value)>,
    /// Patterns… dialog (open when Some): the name for a new pattern.
    #[serde(skip)]
    pub patterns_dialog: Option<String>,
    /// Graphics and Titles ▸ Insert Special Character… (open when Some): the code point typed.
    #[serde(skip)]
    pub char_picker: Option<String>,
    /// View ▸ Grid… dialog open.
    #[serde(skip)]
    pub grid_dialog: bool,
}

impl Default for FxStackUi {
    fn default() -> Self {
        Self {
            favorites: Vec::new(),
            only_accelerated: false,
            only_float32: false,
            only_yuv: false,
            show_description: false,
            described: None,
            show_clip_keyframes: false,
            auto_keyframes: false,
            limit_auto_keyframes: true,
            compare_effects: false,
            add_effect: None,
            import: None,
            clipboard: Vec::new(),
            last_export: String::new(),
            last_auto: Vec::new(),
            layer_clipboard: None,
            layer_pastes: 0,
            char_picker: None,
            grid_dialog: false,
            patterns: Vec::new(),
            patterns_dialog: None,
        }
    }
}

/// With the limit on, automatic keyframes are this many frames apart at least.
pub const AUTO_KEYFRAME_FRAMES: i64 = 10;

/// One parameter's copied keyframes; times relative to the earliest copied keyframe.
#[derive(Clone, Debug, PartialEq)]
pub struct CopiedKeyframes {
    pub effect: String,
    pub param: String,
    pub mask: Option<usize>,
    pub keyframes: Vec<Keyframe>,
}

/// Import Keyframes… (Effect Controls ▸ right-click a keyframe): the pasted data and its mapping.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportDraft {
    pub clip: u64,
    pub effect: usize,
    pub param: String,
    pub mask: Option<usize>,
    pub text: String,
    /// At the clip's own times (else the first keyframe lands on the playhead).
    pub clip_times: bool,
    pub offset_frames: i64,
    pub value_offset: f64,
    /// Map a point's Y (else X) onto a scalar, or a scalar onto a point's Y.
    pub component_y: bool,
    pub error: String,
}

/// Whether `id` passes the Effects panel's filters.
pub fn passes_filters(app: &FilmApp, d: &redrob_film_project::EffectDef) -> bool {
    let f = &app.ui.fx;
    (!f.only_accelerated || d.accelerated) && (!f.only_float32 || d.float32) && (!f.only_yuv || d.yuv)
}

pub fn is_favorite(app: &FilmApp, id: &str) -> bool {
    app.ui.fx.favorites.iter().any(|f| f == id)
}

pub fn toggle_favorite(app: &mut FilmApp, id: &str) {
    if is_favorite(app, id) {
        app.ui.fx.favorites.retain(|f| f != id);
    } else {
        app.ui.fx.favorites.push(id.to_string());
    }
}

/// The Effects panel's right-click menu of an effect: favourite and description.
pub fn effect_menu(app: &mut FilmApp, resp: &egui::Response, id: &str) {
    resp.context_menu(|ui| {
        let fav = is_favorite(app, id);
        let b = ui.button(if fav { tl!("Remove from Favorites") } else { tl!("Add to Favorites") });
        app.auto.add(&format!("effects.menu.{id}.favorite"), b.rect, "Favorite");
        if b.clicked() {
            toggle_favorite(app, id);
            ui.close();
        }
        if ui.button(tl!("Show Description")).clicked() {
            app.ui.fx.show_description = true;
            app.ui.fx.described = Some(id.to_string());
            ui.close();
        }
    });
}

/// The Effects panel's description pane: what the selected effect is and what it controls.
pub fn description_pane(app: &mut FilmApp, ui: &mut egui::Ui, rect: egui::Rect) {
    let t = app.tokens;
    ui.painter().rect_filled(rect, 0.0, t.panel_bg);
    ui.painter().line_segment([rect.left_top(), rect.right_top()], egui::Stroke::new(1.0, t.separator));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(6.0)).id_salt("fx-description"));
    c.set_clip_rect(rect);
    let Some(d) = app.ui.fx.described.as_deref().and_then(redrob_film_project::find_effect) else {
        c.label(egui::RichText::new(tl!("Click an effect to see its description")).color(t.text_dim));
        return;
    };
    c.label(egui::RichText::new(crate::i18n::t(d.name)).strong());
    let folder: Vec<&str> = d.category.iter().map(|s| crate::i18n::t(s)).collect();
    c.label(egui::RichText::new(folder.join(" ▸ ")).color(t.text_dim).size(11.0));
    let mut tags = Vec::new();
    if d.accelerated {
        tags.push(tl!("GPU accelerated"));
    }
    if d.float32 {
        tags.push(tl!("32-bit color"));
    }
    if d.yuv {
        tags.push(tl!("YUV"));
    }
    if !tags.is_empty() {
        c.label(egui::RichText::new(tags.join(" · ")).size(11.0));
    }
    let names: Vec<&str> = d.params.iter().map(|p| crate::i18n::t(p.label)).collect();
    if !names.is_empty() {
        c.label(egui::RichText::new(tlf!("Controls: {list}", list = names.join(", "))).size(11.0));
    }
    let animatable = d.params.iter().filter(|p| p.animatable).count();
    if animatable > 0 {
        c.label(egui::RichText::new(tlf!("{n} keyframable parameters", n = animatable)).color(t.text_dim).size(11.0));
    }
}

/// Clip context menu (Timeline): Insert an Effect ▸ favourites, and Effects Enabled.
pub fn timeline_menu(app: &mut FilmApp, ui: &mut egui::Ui, clips: &[(u64, TrackKind, bool, bool)]) {
    if clips.is_empty() {
        return;
    }
    ui.separator();
    let favorites = app.ui.fx.favorites.clone();
    ui.menu_button(tl!("Insert an Effect"), |ui| {
        let mut any = false;
        for id in &favorites {
            let Some(d) = redrob_film_project::find_effect(id) else { continue };
            let fits = clips.iter().any(|c| (c.1 == TrackKind::Audio) == (d.kind == EffectKind::Audio));
            if !fits {
                continue;
            }
            any = true;
            let b = ui.button(crate::i18n::t(d.name));
            app.auto.add(&format!("timeline.clipMenu.insertEffect.{id}"), b.rect, d.name);
            if b.clicked() {
                let ids: Vec<u64> = clips.iter().map(|c| c.0).collect();
                if let Err(e) = app.session.execute("effects.apply", json!({"clips": ids, "effect": id})) {
                    app.ui.status = e.to_string();
                }
                ui.close();
            }
        }
        if !any {
            ui.add_enabled(false, egui::Button::new(tl!("(no favorite effects)")));
        }
        ui.separator();
        if ui.button(tl!("Add Effect…")).clicked() {
            app.ui.fx.add_effect = Some(String::new());
            ui.close();
        }
    });
    // Insert a Transition: at the clip's start or end, over the tracks below when no clip touches
    // it there (Kdenlive's composition from the clip menu); the default one or a favourite
    let video: Vec<u64> = clips.iter().filter(|c| c.1 == TrackKind::Video).map(|c| c.0).collect();
    if let Some(&clip) = video.first() {
        ui.menu_button(tl!("Insert a Transition"), |ui| {
            let mut choices: Vec<(String, Option<&'static str>)> = vec![(tl!("Default Transition").to_string(), None)];
            for id in &favorites {
                if let Some(d) = redrob_film_project::find_effect(id).filter(|d| d.kind == EffectKind::VideoTransition) {
                    choices.push((crate::i18n::t(d.name).to_string(), Some(d.id)));
                }
            }
            for (label, effect) in choices {
                ui.menu_button(label, |ui| {
                    for (side, text) in [("in", tl!("At Clip Start")), ("out", tl!("At Clip End"))] {
                        let b = ui.button(text);
                        app.auto.add(&format!("timeline.clipMenu.insertTransition.{}.{side}", effect.unwrap_or("default")), b.rect, text);
                        if b.clicked() {
                            let mut p = json!({"clip": clip, "edge": side});
                            if let Some(e) = effect {
                                p["effect"] = json!(e);
                            }
                            if let Err(e) = app.session.execute("sequence.applyVideoTransition", p) {
                                app.ui.status = e.to_string();
                            }
                            ui.close();
                        }
                    }
                });
            }
        });
    }
    // (2: has standard effects, 3: one of them is on)
    let with_fx = clips.iter().any(|c| c.2);
    let on = clips.iter().any(|c| c.3);
    let label = if on { tl!("✓ Effects Enabled") } else { tl!("Effects Enabled") };
    let b = ui.add_enabled(with_fx, egui::Button::new(label));
    app.auto.add("timeline.clipMenu.effectsEnabled", b.rect, "Effects Enabled");
    if b.clicked() {
        let ids: Vec<u64> = clips.iter().map(|c| c.0).collect();
        if let Err(e) = app.session.execute("effects.toggleStack", json!({"clips": ids})) {
            app.ui.status = e.to_string();
        }
        ui.close();
    }
}

/// UI routes: Sequence ▸ Add Effect…, and Copy / Paste of keyframes while Effect Controls has
/// focus and keyframes are selected (otherwise Copy and Paste act on clips as usual).
pub fn route(app: &mut FilmApp, ctx: &egui::Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let plain = params.as_object().is_none_or(|m| m.is_empty());
    match id {
        "sequence.addEffectDialog" => {
            app.ui.fx.add_effect = Some(String::new());
            Some(Ok(json!({"dialog": "addEffect"})))
        }
        "graphics.patterns" => {
            app.ui.fx.patterns_dialog = Some(String::new());
            Some(Ok(json!({"dialog": "patterns"})))
        }
        "view.gridSettings" => {
            app.ui.fx.grid_dialog = true;
            Some(Ok(json!({"dialog": "grid"})))
        }
        "graphics.insertSpecialCharacter" => Some(match params.get("text").and_then(Value::as_str) {
            // agents name the text; the menu opens the picker
            Some(t) => insert_text(app, t).map(|_| Value::Null),
            None => {
                app.ui.fx.char_picker = Some(String::new());
                Ok(json!({"dialog": "specialCharacter"}))
            }
        }),
        "edit.copy" if plain && app.ui.focused == PanelKind::EffectControls && !app.ui.keyframe_selection.is_empty() => Some(copy_keyframes(app, ctx)),
        "edit.paste" if plain && app.ui.focused == PanelKind::EffectControls && !app.ui.fx.clipboard.is_empty() => Some(paste_keyframes(app)),
        // graphic layers (Kdenlive's title objects): Copy keeps them, each Paste puts a copy
        // further down and right
        "edit.copy" if plain && graphics_focus(app) && !app.session.state.graphic_layers.is_empty() => {
            Some(app.session.execute("graphics.copyLayers", json!({})).map_err(|e| e.to_string()).map(|v| {
                let n = v["layers"].as_array().map_or(0, Vec::len);
                app.ui.fx.layer_clipboard = Some(v["layers"].clone());
                app.ui.fx.layer_pastes = 0;
                json!({"layers": n})
            }))
        }
        "edit.paste" if plain && graphics_focus(app) && app.ui.fx.layer_clipboard.is_some() => {
            let k = f64::from(app.ui.fx.layer_pastes + 1) * 20.0;
            let layers = app.ui.fx.layer_clipboard.clone().unwrap_or_default();
            let r = app.session.execute("graphics.pasteLayers", json!({"layers": layers, "offset": [k, k]})).map_err(|e| e.to_string());
            if r.is_ok() {
                app.ui.fx.layer_pastes += 1;
            }
            Some(r)
        }
        _ => None,
    }
}

/// Whether Copy / Paste act on graphic layers: the Program monitor or Essential Graphics has focus.
fn graphics_focus(app: &FilmApp) -> bool {
    matches!(app.ui.focused, PanelKind::Program | PanelKind::EssentialGraphics)
}

/// Copy the selected keyframes (Ctrl+C in Effect Controls). The text form goes on the system
/// clipboard too, for Import Keyframes… in another project.
pub fn copy_keyframes(app: &mut FilmApp, ctx: &egui::Context) -> Result<Value, String> {
    let seq = app.session.active_sequence().ok_or("no sequence")?;
    let mut groups: Vec<CopiedKeyframes> = Vec::new();
    for k in &app.ui.keyframe_selection {
        let Some((_, it)) = seq.find_item(redrob_film_project::ClipId(k.clip)) else { continue };
        let Some(e) = it.effects.get(k.effect) else { continue };
        let prm = match k.mask {
            Some(m) => e.masks.get(m).and_then(|m| m.param(&k.param)),
            None => e.params.get(&k.param),
        };
        let Some(kf) = prm.and_then(|p| p.keyframes.iter().find(|x| x.time == k.time)) else { continue };
        match groups.iter_mut().find(|g| g.effect == e.effect && g.param == k.param && g.mask == k.mask) {
            Some(g) => g.keyframes.push(kf.clone()),
            None => groups.push(CopiedKeyframes { effect: e.effect.clone(), param: k.param.clone(), mask: k.mask, keyframes: vec![kf.clone()] }),
        }
    }
    let Some(first) = groups.iter().flat_map(|g| g.keyframes.iter().map(|k| k.time)).min() else { return Err("no keyframes selected".into()) };
    for g in &mut groups {
        g.keyframes.sort_by_key(|k| k.time);
        for k in &mut g.keyframes {
            k.time -= first;
        }
    }
    let n: usize = groups.iter().map(|g| g.keyframes.len()).sum();
    if let Some(g) = groups.first() {
        let text = json!({"format": "redrob-film-keyframes", "effect": g.effect, "param": g.param, "keyframes": g.keyframes}).to_string();
        ctx.copy_text(text.clone());
        app.ui.fx.last_export = text;
    }
    app.ui.fx.clipboard = groups;
    app.ui.status = tlf!("Copied {n} keyframes", n = n);
    Ok(json!({"keyframes": n}))
}

/// Paste the copied keyframes at the playhead onto the selected clip (Ctrl+V in Effect Controls):
/// the earliest lands on the playhead, the others keep their spacing; each parameter goes to the
/// clip's effect of the same kind.
pub fn paste_keyframes(app: &mut FilmApp) -> Result<Value, String> {
    let clip = app.session.state.selection.first().copied().ok_or("select a clip first")?;
    let seq = app.session.active_sequence().ok_or("no sequence")?;
    let (_, it) = seq.find_item(clip).ok_or("select a clip first")?;
    let ph = app.session.playhead();
    let mt = it.effect_time_at(ph.clamp(it.start, (it.end() - Tick(1)).max(it.start)));
    let offset = mt - it.source_in;
    let calls: Vec<Value> = app
        .ui
        .fx
        .clipboard
        .iter()
        .filter_map(|g| {
            let idx = it.effects.iter().position(|e| e.effect == g.effect)?;
            let mut p = json!({"clip": clip.0, "effect": idx, "param": g.param, "keyframes": g.keyframes, "anchor": "clip", "offset": offset.0});
            if let Some(m) = g.mask {
                p["mask"] = json!(m);
            }
            Some(p)
        })
        .collect();
    let mut n = 0u64;
    for p in calls {
        let r = app.session.execute("effects.pasteKeyframes", p).map_err(|e| e.to_string())?;
        n += r["pasted"].as_u64().unwrap_or(0);
    }
    if n == 0 {
        return Err("the selected clip has none of the copied keyframes' effects".into());
    }
    Ok(json!({"pasted": n}))
}

/// Copy a parameter's keyframes to the system clipboard (Copy Keyframes to Clipboard).
pub fn export_param(app: &mut FilmApp, ctx: &egui::Context, clip: u64, effect: usize, param: &str, mask: Option<usize>) {
    let mut p = json!({"clip": clip, "effect": effect, "param": param});
    if let Some(m) = mask {
        p["mask"] = json!(m);
    }
    match app.session.execute("effects.exportKeyframes", p) {
        Ok(v) => {
            let text = v.to_string();
            ctx.copy_text(text.clone());
            app.ui.fx.last_export = text;
            app.ui.status = tl!("Keyframes copied to the clipboard").to_string();
        }
        Err(e) => app.ui.status = e.to_string(),
    }
}

/// Open Import Keyframes… for a parameter, offering the last export.
pub fn open_import(app: &mut FilmApp, clip: u64, effect: usize, param: &str, mask: Option<usize>) {
    let text = app.ui.fx.last_export.clone();
    app.ui.fx.import = Some(ImportDraft { clip, effect, param: param.to_string(), mask, text, clip_times: true, ..Default::default() });
}

/// Automatic keyframes: rewrite a value change made while playing so it writes a keyframe at the
/// playhead (or, with the limit on, onto the automatic keyframe written less than
/// [`AUTO_KEYFRAME_FRAMES`] frames ago).
pub fn auto_keyframe(app: &mut FilmApp, p: &mut Value) {
    if !app.ui.fx.auto_keyframes || !app.playback.playing || p.get("mask").is_some_and(|m| !m.is_null()) {
        return;
    }
    let (Some(clip), Some(pid)) = (p["clip"].as_u64(), p["param"].as_str().map(str::to_string)) else { return };
    let Some(seq) = app.session.active_sequence() else { return };
    let Some((_, it)) = seq.find_item(redrob_film_project::ClipId(clip)) else { return };
    let e = match &p["effect"] {
        Value::Number(n) => n.as_u64().and_then(|i| it.effects.get(i as usize)),
        Value::String(s) => it.effects.iter().find(|e| &e.effect == s),
        _ => None,
    };
    let Some(e) = e else { return };
    if !e.def().and_then(|d| d.param(&pid)).is_some_and(|d| d.animatable) {
        return;
    }
    let key = format!("{clip}:{}:{pid}", e.effect);
    let rate = app.session.sequence_rate();
    let ph = app.session.playhead();
    let mut at = ph;
    if app.ui.fx.limit_auto_keyframes
        && let Some((_, last)) = app.ui.fx.last_auto.iter().find(|(k, _)| *k == key)
        && (ph.0 - last).abs() < rate.tick_of(AUTO_KEYFRAME_FRAMES).0
    {
        at = Tick(*last);
    }
    p["keyframe"] = json!(true);
    p["time"] = json!(at.0);
    app.ui.fx.last_auto.retain(|(k, _)| *k != key);
    app.ui.fx.last_auto.push((key, at.0));
}

/// Automatic keyframes for a `graphics.set` made while playing (a layer dragged on the Program
/// monitor): its animatable properties get keyframes at the playhead, with the same limit.
pub fn auto_keyframe_graphics(app: &mut FilmApp, p: &mut Value) {
    if !app.ui.fx.auto_keyframes || !app.playback.playing {
        return;
    }
    let key = format!("gfx:{}:{}", p["clip"], p["layer"]);
    let rate = app.session.sequence_rate();
    let ph = app.session.playhead();
    let mut at = ph;
    if app.ui.fx.limit_auto_keyframes
        && let Some((_, last)) = app.ui.fx.last_auto.iter().find(|(k, _)| *k == key)
        && (ph.0 - last).abs() < rate.tick_of(AUTO_KEYFRAME_FRAMES).0
    {
        at = Tick(*last);
    }
    p["keyframe"] = json!(true);
    p["time"] = json!(at.0);
    app.ui.fx.last_auto.retain(|(k, _)| *k != key);
    app.ui.fx.last_auto.push((key, at.0));
}

pub fn show(app: &mut FilmApp, ctx: &egui::Context) {
    add_effect_dialog(app, ctx);
    import_dialog(app, ctx);
    char_picker(app, ctx);
    grid_dialog(app, ctx);
    patterns_dialog(app, ctx);
}

/// Graphics and Titles ▸ Patterns…: save the selected graphic layers as a named pattern, insert a
/// pattern into the selected graphic (where it was drawn), delete patterns.
fn patterns_dialog(app: &mut FilmApp, ctx: &egui::Context) {
    let Some(mut name) = app.ui.fx.patterns_dialog.clone() else { return };
    let mut open = true;
    let mut save = false;
    let mut insert: Option<usize> = None;
    let mut delete: Option<usize> = None;
    egui::Window::new(tl!("Patterns")).id(egui::Id::new("Patterns dialog")).open(&mut open).collapsible(false).default_width(300.0).show(ctx, |ui| {
        if app.ui.fx.patterns.is_empty() {
            ui.label(egui::RichText::new(tl!("No patterns yet: select layers of a graphic and save them.")).color(app.tokens.text_dim));
        }
        for (i, (n, layers)) in app.ui.fx.patterns.iter().enumerate() {
            ui.horizontal(|ui| {
                let count = layers.as_array().map_or(0, Vec::len);
                ui.label(format!("{n} ({count})"));
                if ui.button(tl!("Insert")).clicked() {
                    insert = Some(i);
                }
                if ui.button(tl!("Delete")).clicked() {
                    delete = Some(i);
                }
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text(tl!("Pattern name")).desired_width(150.0));
            app.auto.add("patterns.name", r.rect, "Pattern name");
            let b = ui.add_enabled(!name.trim().is_empty() && !app.session.state.graphic_layers.is_empty(), egui::Button::new(tl!("Save Selected Layers")));
            app.auto.add("patterns.save", b.rect, "Save Selected Layers");
            save = b.clicked();
        });
    });
    if save {
        match app.session.execute("graphics.copyLayers", json!({})) {
            Ok(v) => {
                let n = name.trim().to_string();
                app.ui.fx.patterns.retain(|(x, _)| *x != n);
                app.ui.fx.patterns.push((n, v["layers"].clone()));
                name.clear();
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    if let Some(i) = insert
        && let Some((_, layers)) = app.ui.fx.patterns.get(i).cloned()
        && let Err(e) = app.session.execute("graphics.pasteLayers", json!({"layers": layers, "offset": [0.0, 0.0]}))
    {
        app.ui.status = e.to_string();
    }
    if let Some(i) = delete
        && i < app.ui.fx.patterns.len()
    {
        app.ui.fx.patterns.remove(i);
    }
    app.ui.fx.patterns_dialog = if open && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) { Some(name) } else { None };
}

/// View ▸ Grid…: lines over the Program monitor picture (count and colour); graphics snap to them.
fn grid_dialog(app: &mut FilmApp, ctx: &egui::Context) {
    if !app.ui.fx.grid_dialog {
        return;
    }
    let mut open = true;
    egui::Window::new(tl!("Grid")).id(egui::Id::new("Grid dialog")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
        let v = &mut app.ui.program;
        ui.horizontal(|ui| {
            ui.label(tl!("Lines:"));
            let r = ui.add(egui::Slider::new(&mut v.grid_lines, 0..=32));
            app.auto.add("grid.lines", r.rect, "Lines");
        });
        ui.horizontal(|ui| {
            ui.label(tl!("Color:"));
            let [r, g, b, a] = v.grid_color;
            let mut c = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
            if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::OnlyBlend).changed() {
                v.grid_color = c.to_srgba_unmultiplied();
            }
        });
        ui.checkbox(&mut v.snap, tl!("Snap to the grid and guides"));
        if ui.button(tl!("Close")).clicked() {
            app.ui.fx.grid_dialog = false;
        }
    });
    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.fx.grid_dialog = false;
    }
}

/// The characters Insert Special Character… offers in its grid.
const SPECIAL_CHARACTERS: &str = "©®™℗°±×÷≈≠≤≥∞√∑µπΩαβγδλσφ€£¥¢₹₩§¶†‡•·…–—‘’“”«»‹›¿¡←↑→↓↔⇒★☆♥♦♣♠✓✗♪♫";

/// Insert `s` into the text layer being edited (at the caret, over the selection), or at the end of
/// the selected text layer's text.
pub fn insert_text(app: &mut FilmApp, s: &str) -> Result<(), String> {
    let seq = app.session.active_sequence().ok_or("no sequence")?;
    let (clip, layer, range) = match &app.ui.gfx_edit {
        Some(ed) => (ed.clip, ed.layer, Some((ed.caret.min(ed.anchor), ed.caret.max(ed.anchor)))),
        None => {
            let clip = app.session.state.selection.first().ok_or("select a text layer first")?.0;
            (clip, app.session.state.graphic_layers.first().copied().ok_or("select a text layer first")?, None)
        }
    };
    let (_, it) = seq.find_item(redrob_film_project::ClipId(clip)).ok_or("select a text layer first")?;
    let idx = redrob_film_project::graphic::layer_indices(&it.effects);
    let e = idx.get(layer).map(|&i| &it.effects[i]).filter(|e| e.effect == redrob_film_project::graphic::TEXT_LAYER).ok_or("select a text layer first")?;
    let mt = it.source_time_at(app.session.playhead().clamp(it.start, (it.end() - Tick(1)).max(it.start)));
    let mut text = match e.param("text").map(|p| p.value_at(mt)) {
        Some(redrob_film_project::ParamValue::Text(t)) => t,
        _ => String::new(),
    };
    let (a, b) = range.unwrap_or((text.len(), text.len()));
    let fix = |i: usize| {
        let mut i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let (a, b) = (fix(a), fix(b));
    text.replace_range(a..b, s);
    app.session.execute("graphics.setText", json!({"clip": clip, "layer": layer, "text": text})).map_err(|e| e.to_string())?;
    if let Some(ed) = app.ui.gfx_edit.as_mut() {
        ed.caret = a + s.len();
        ed.anchor = ed.caret;
    }
    Ok(())
}

/// Insert Special Character…: a grid of symbols and a field for any Unicode code point (hex).
fn char_picker(app: &mut FilmApp, ctx: &egui::Context) {
    let Some(mut code) = app.ui.fx.char_picker.clone() else { return };
    let mut open = true;
    let mut pick: Option<String> = None;
    egui::Window::new(tl!("Insert Special Character")).id(egui::Id::new("Insert Special Character")).open(&mut open).collapsible(false).resizable(false).show(
        ctx,
        |ui| {
            egui::Grid::new("special-chars").spacing([2.0, 2.0]).show(ui, |ui| {
                for (i, c) in SPECIAL_CHARACTERS.chars().enumerate() {
                    let b = ui.add_sized([26.0, 26.0], egui::Button::new(egui::RichText::new(c.to_string()).size(15.0)));
                    app.auto.add(&format!("specialCharacter.{:04X}", c as u32), b.rect, &c.to_string());
                    if b.on_hover_text(format!("U+{:04X}", c as u32)).clicked() {
                        pick = Some(c.to_string());
                    }
                    if i % 12 == 11 {
                        ui.end_row();
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(tl!("Unicode (hex):"));
                let r = ui.add(egui::TextEdit::singleline(&mut code).desired_width(80.0).hint_text("2764"));
                app.auto.add("specialCharacter.code", r.rect, "Unicode");
                let typed = u32::from_str_radix(code.trim().trim_start_matches("U+").trim_start_matches("u+"), 16)
                    .ok()
                    .and_then(char::from_u32)
                    .filter(|c| !c.is_control());
                ui.label(egui::RichText::new(typed.map(String::from).unwrap_or_default()).size(18.0));
                let b = ui.add_enabled(typed.is_some(), egui::Button::new(tl!("Insert")));
                app.auto.add("specialCharacter.insert", b.rect, "Insert");
                if b.clicked()
                    && let Some(c) = typed
                {
                    pick = Some(c.to_string());
                }
            });
        },
    );
    if let Some(s) = pick
        && let Err(e) = insert_text(app, &s)
    {
        app.ui.status = e;
    }
    app.ui.fx.char_picker = if open && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) { Some(code) } else { None };
}

/// Sequence ▸ Add Effect…: effects by folder (favourites first), searchable; applies to the
/// selected clips.
fn add_effect_dialog(app: &mut FilmApp, ctx: &egui::Context) {
    let Some(mut q) = app.ui.fx.add_effect.clone() else { return };
    let mut open = true;
    let mut apply: Option<String> = None;
    egui::Window::new(tl!("Add Effect")).id(egui::Id::new("Add Effect")).open(&mut open).collapsible(false).default_size([340.0, 420.0]).show(ctx, |ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut q).hint_text(tl!("Search effects")).desired_width(f32::INFINITY));
        app.auto.add("addEffect.search", r.rect, "Search effects");
        let filter = q.to_lowercase();
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            let defs = redrob_film_project::effect_defs();
            let usable = |d: &redrob_film_project::EffectDef| {
                matches!(d.kind, EffectKind::Video | EffectKind::Audio)
                    && !d.intrinsic
                    && !redrob_film_project::graphic::is_layer_id(d.id)
                    && crate::i18n::matches_query(d.name, &filter)
            };
            let favorites: Vec<&redrob_film_project::EffectDef> =
                app.ui.fx.favorites.iter().filter_map(|id| redrob_film_project::find_effect(id)).filter(|d| usable(d)).collect();
            if !favorites.is_empty() {
                ui.label(egui::RichText::new(tl!("Favorites")).strong());
                for d in favorites {
                    let b = ui.selectable_label(false, crate::i18n::t(d.name));
                    if b.double_clicked() || b.clicked() {
                        apply = Some(d.id.to_string());
                    }
                }
                ui.separator();
            }
            let mut folders: Vec<String> = Vec::new();
            for d in defs.iter().filter(|d| usable(d)) {
                let f = d.category.join("/");
                if !folders.contains(&f) {
                    folders.push(f);
                }
            }
            for f in folders {
                let title: Vec<&str> = f.split('/').map(crate::i18n::t).collect();
                egui::CollapsingHeader::new(title.join(" ▸ ")).id_salt(("add-effect", &f)).default_open(!filter.is_empty()).show(ui, |ui| {
                    for d in defs.iter().filter(|d| usable(d) && d.category.join("/") == f) {
                        let b = ui.selectable_label(false, crate::i18n::t(d.name));
                        app.auto.add(&format!("addEffect.item.{}", d.id), b.rect, d.name);
                        if b.clicked() {
                            apply = Some(d.id.to_string());
                        }
                    }
                });
            }
        });
    });
    if let Some(id) = apply {
        match app.session.execute("effects.apply", json!({"effect": id})) {
            Ok(_) => {
                app.ui.fx.add_effect = None;
                return;
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    app.ui.fx.add_effect = if open && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) { Some(q) } else { None };
}

/// Import Keyframes…: paste keyframe data (an export from Copy Keyframes to Clipboard or Ctrl+C),
/// choose where it goes and how it maps, then OK writes it onto the parameter.
fn import_dialog(app: &mut FilmApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.fx.import.clone() else { return };
    let mut open = true;
    let mut ok = false;
    let mut cancel = false;
    egui::Window::new(tl!("Import Keyframes")).id(egui::Id::new("Import Keyframes")).open(&mut open).collapsible(false).default_width(380.0).show(ctx, |ui| {
        ui.label(tlf!("Into: {param}", param = d.param));
        ui.label(egui::RichText::new(tl!("Keyframe data (paste with Ctrl+V):")).size(11.0));
        let r = ui.add(egui::TextEdit::multiline(&mut d.text).desired_rows(6).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
        app.auto.add("importKeyframes.text", r.rect, "Keyframe data");
        ui.checkbox(&mut d.clip_times, tl!("Keep the clip's keyframe times (else start at the playhead)"));
        ui.horizontal(|ui| {
            ui.label(tl!("Time offset (frames):"));
            ui.add(egui::DragValue::new(&mut d.offset_frames));
        });
        ui.horizontal(|ui| {
            ui.label(tl!("Value offset:"));
            ui.add(egui::DragValue::new(&mut d.value_offset).speed(0.1));
        });
        ui.horizontal(|ui| {
            ui.label(tl!("Point component:"));
            ui.radio_value(&mut d.component_y, false, "X");
            ui.radio_value(&mut d.component_y, true, "Y");
        });
        if !d.error.is_empty() {
            ui.colored_label(app.tokens.render_red, &d.error);
        }
        ui.horizontal(|ui| {
            cancel = ui.button(tl!("Cancel")).clicked();
            let b = ui.add_enabled(!d.text.trim().is_empty(), egui::Button::new(tl!("OK")));
            app.auto.add("importKeyframes.ok", b.rect, "OK");
            ok = b.clicked();
        });
    });
    if ok {
        let rate = app.session.sequence_rate();
        let mut p = json!({
            "clip": d.clip,
            "effect": d.effect,
            "param": d.param,
            "data": d.text,
            "anchor": if d.clip_times { "clip" } else { "playhead" },
            "offset": rate.tick_of(d.offset_frames).0,
            "valueOffset": d.value_offset,
            "component": if d.component_y { "y" } else { "x" },
        });
        if let Some(m) = d.mask {
            p["mask"] = json!(m);
        }
        match app.session.execute("effects.pasteKeyframes", p) {
            Ok(r) => {
                app.ui.status = tlf!("Imported {n} keyframes", n = r["pasted"].as_u64().unwrap_or(0));
                app.ui.fx.import = None;
            }
            Err(e) => {
                d.error = e.to_string();
                app.ui.fx.import = Some(d);
            }
        }
        return;
    }
    app.ui.fx.import = if open && !cancel { Some(d) } else { None };
}

/// Evaluate a typed value: a number or a basic math expression (`+ - * / ( )`, usual precedence)
/// that may name other fields (`%x`, `%y`, `%w`, `%h`, `%s`, `%o`, `%1`…, from `refs`).
pub fn eval_expr(text: &str, refs: &[(String, f64)]) -> Option<f64> {
    struct P<'a> {
        s: &'a [u8],
        i: usize,
        refs: &'a [(String, f64)],
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
                self.i += 1;
            }
        }
        fn expr(&mut self) -> Option<f64> {
            let mut v = self.term()?;
            loop {
                self.ws();
                match self.s.get(self.i) {
                    Some(b'+') => {
                        self.i += 1;
                        v += self.term()?;
                    }
                    Some(b'-') => {
                        self.i += 1;
                        v -= self.term()?;
                    }
                    _ => return Some(v),
                }
            }
        }
        fn term(&mut self) -> Option<f64> {
            let mut v = self.factor()?;
            loop {
                self.ws();
                match self.s.get(self.i) {
                    Some(b'*') => {
                        self.i += 1;
                        v *= self.factor()?;
                    }
                    Some(b'/') => {
                        self.i += 1;
                        let d = self.factor()?;
                        if d == 0.0 {
                            return None;
                        }
                        v /= d;
                    }
                    _ => return Some(v),
                }
            }
        }
        fn factor(&mut self) -> Option<f64> {
            self.ws();
            match *self.s.get(self.i)? {
                b'-' => {
                    self.i += 1;
                    Some(-self.factor()?)
                }
                b'+' => {
                    self.i += 1;
                    self.factor()
                }
                b'(' => {
                    self.i += 1;
                    let v = self.expr()?;
                    self.ws();
                    (self.s.get(self.i) == Some(&b')')).then(|| {
                        self.i += 1;
                        v
                    })
                }
                b'%' => {
                    self.i += 1;
                    let start = self.i;
                    while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphanumeric()) {
                        self.i += 1;
                    }
                    let name = std::str::from_utf8(&self.s[start..self.i]).ok()?.to_ascii_lowercase();
                    self.refs.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
                }
                _ => {
                    let start = self.i;
                    while self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.' || *c == b',') {
                        self.i += 1;
                    }
                    std::str::from_utf8(&self.s[start..self.i]).ok()?.replace(',', ".").parse().ok()
                }
            }
        }
    }
    let mut p = P { s: text.trim().as_bytes(), i: 0, refs };
    let v = p.expr()?;
    p.ws();
    (p.i == p.s.len() && v.is_finite()).then_some(v)
}

/// A relative time typed into a timecode field in Kdenlive's short forms: `+` or `-` then `1::`
/// (an hour), `2:` (minutes), `12.` (seconds), `2:.9` (minutes and frames) — a `:` ends an hours
/// or minutes field, a `.` the seconds. Returns the signed frame count; None for anything else
/// (`+9`, `+00:00:01:05` and absolute times are `parse_timecode`'s).
pub fn relative_frames(text: &str, timebase: i64) -> Option<i64> {
    let t = text.trim();
    let (sign, rest) = match t.as_bytes().first()? {
        b'+' => (1, &t[1..]),
        b'-' => (-1, &t[1..]),
        _ => return None,
    };
    let rest = rest.trim();
    if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit() || c == ':' || c == '.' || c == ';') {
        return None;
    }
    if !(rest.contains('.') || rest.ends_with(':')) {
        return None;
    }
    let rest = rest.replace(';', ":");
    let num = |s: &str| -> Option<i64> { if s.is_empty() { Some(0) } else { s.parse().ok() } };
    // hours / minutes / seconds, right-aligned on the seconds
    let hms = |s: &str| -> Option<i64> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() > 3 {
            return None;
        }
        let mut secs = 0;
        for (i, p) in parts.iter().rev().enumerate() {
            secs += num(p)? * [1, 60, 3600][i];
        }
        Some(secs)
    };
    let frames = if let Some((left, f)) = rest.split_once('.') {
        hms(left)? * timebase + num(f)?
    } else {
        // a trailing `:`: hours / minutes
        hms(&rest)? * timebase
    };
    Some(sign * frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions_and_field_references() {
        let refs = vec![("x".to_string(), 100.0), ("y".to_string(), 50.0), ("1".to_string(), 3.0)];
        assert_eq!(eval_expr("12.5", &[]), Some(12.5));
        assert_eq!(eval_expr("2+3*4", &[]), Some(14.0));
        assert_eq!(eval_expr("(2+3)*4", &[]), Some(20.0));
        assert_eq!(eval_expr("-4/2", &[]), Some(-2.0));
        assert_eq!(eval_expr("%y*2", &refs), Some(100.0));
        assert_eq!(eval_expr("%x - %1", &refs), Some(97.0));
        assert_eq!(eval_expr("1/0", &[]), None);
        assert_eq!(eval_expr("%w", &refs), None);
        assert_eq!(eval_expr("3 +", &[]), None);
    }

    #[test]
    fn relative_timecode_short_forms() {
        let tb = 25;
        assert_eq!(relative_frames("+1::", tb), Some(3600 * 25));
        assert_eq!(relative_frames("-2:", tb), Some(-2 * 60 * 25));
        assert_eq!(relative_frames("+12.", tb), Some(12 * 25));
        assert_eq!(relative_frames("+2:.9", tb), Some(2 * 60 * 25 + 9));
        assert_eq!(relative_frames("+1.05", tb), Some(30));
        // left to parse_timecode
        assert_eq!(relative_frames("+9", tb), None);
        assert_eq!(relative_frames("+00:00:01:05", tb), None);
        assert_eq!(relative_frames("00:00:01:05", tb), None);
    }
}
