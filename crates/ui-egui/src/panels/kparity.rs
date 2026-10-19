//! Kdenlive-parity extras that need the frontend: the command bar (Find Action…), Focus
//! Timecode, keyframe navigation commands on the selected clip and the keyboard editing of the
//! selected keyframes in Effect Controls.
//!
//! | Id | What | Kdenlive layout key |
//! |---|---|---|
//! | `window.commandBar` | Find Action…: search every menu command and run it | Ctrl+Alt+I |
//! | `window.focusTimecode` | Focus Timecode: type a time into the Program monitor's timecode | = |
//! | `keyframes.toggle` | Add/Remove Keyframe on the selected keyframe's parameter (else the clip's first animated one) | |
//! | `keyframes.next` / `keyframes.prev` | Go to the selected clip's next / previous keyframe | |
//!
//! Effect Controls with a selected keyframe: Left / Right move it one frame, Alt+Left / Alt+Right
//! go to the previous / next keyframe.
//!
//! Automation ids: `commandBar.search`, `commandBar.item.<n>`.

use redrob_film_project::TrackItem;
use redrob_film_time::{Tick, TimeDisplay, format_time};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::dock::PanelKind;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KpUi {
    /// The command bar (open when Some): its search text and highlighted row.
    #[serde(skip)]
    pub command_bar: Option<(String, usize)>,
    /// Text panel ▸ Captions: the edit window's text size (0 = the default).
    pub caption_font: f32,
    /// Text panel ▸ Transcript: the words' text size (0 = the default).
    pub transcript_font: f32,
    /// Effects whose keyframes the timeline clip does not show (`clip:effect index`; Effect
    /// Controls ▸ effect menu ▸ Show Keyframes in Timeline).
    pub hidden_timeline_keyframes: Vec<String>,
    /// A keyframe diamond being dragged on a timeline clip: (clip, the keyframes' media time).
    #[serde(skip)]
    pub kf_drag: Option<(u64, redrob_film_time::Tick)>,
    /// Import Captions options (open when Some): the file and the text encoding (`auto` = detected).
    #[serde(skip)]
    pub caption_import: Option<(String, String)>,
}

/// The caption edit window's text size (Text panel ▸ Captions zoom).
pub fn caption_font_size(app: &FilmcraftApp) -> f32 {
    if app.ui.kp.caption_font > 0.0 { app.ui.kp.caption_font } else { 12.5 }
}

/// The transcript's text size (Text panel ▸ Transcript zoom).
pub fn transcript_font_size(app: &FilmcraftApp) -> f32 {
    if app.ui.kp.transcript_font > 0.0 { app.ui.kp.transcript_font } else { 13.0 }
}

pub fn route(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, _params: &Value) -> Option<Result<Value, String>> {
    match id {
        "window.commandBar" => {
            app.ui.kp.command_bar = Some((String::new(), 0));
            Some(Ok(Value::Null))
        }
        "window.focusTimecode" => {
            let Some(q) = app.session.active_sequence() else { return Some(Err(tl!("Open a sequence first").to_string())) };
            let rate = q.settings.frame_rate;
            let off = rate.tick_of(q.start_timecode.max(0));
            let text = format_time(app.session.playhead() + off, rate, q.settings.drop_frame, TimeDisplay::Timecode, 48_000);
            // the field enters editing with its whole value selected (see `widgets::timecode_field`)
            let id = egui::Id::new(("program", "timecode"));
            ctx.data_mut(|d| {
                d.insert_temp(id.with("editing"), text);
                d.insert_temp(id.with("select-all"), true);
            });
            app.show_panel(PanelKind::Program);
            Some(Ok(Value::Null))
        }
        "keyframes.toggle" => Some(toggle_keyframe(app)),
        "keyframes.next" | "keyframes.prev" => Some(go_keyframe(app, id == "keyframes.next")),
        _ => None,
    }
}

/// The clip keyframe commands act on: the selected keyframe's clip, else the first selected clip.
fn target_clip(app: &FilmcraftApp) -> Option<(u64, TrackItem)> {
    let q = app.session.active_sequence()?;
    let clip = app.ui.keyframe_selection.first().map(|k| k.clip).or_else(|| app.session.state.selection.first().map(|c| c.0))?;
    let (_, it) = q.find_item(redrob_film_project::ClipId(clip))?;
    Some((clip, it.clone()))
}

fn timeline_time_of(it: &TrackItem, m: Tick) -> Tick {
    it.start + Tick(((m - it.source_in).0 as f64 / it.speed.abs().max(1e-6)) as i64)
}

fn go_keyframe(app: &mut FilmcraftApp, next: bool) -> Result<Value, String> {
    let (_, it) = target_clip(app).ok_or_else(|| tl!("Select a clip first").to_string())?;
    let mut times: Vec<Tick> = it
        .effects
        .iter()
        .filter(|e| e.enabled)
        .flat_map(|e| e.params.values().flat_map(|p| p.keyframes.iter().map(|k| timeline_time_of(&it, k.time))))
        .collect();
    times.sort();
    times.dedup();
    let ph = app.session.playhead();
    let to = if next { times.into_iter().find(|t| *t > ph) } else { times.into_iter().rev().find(|t| *t < ph) };
    match to {
        Some(t) => {
            app.session.set_playhead(t);
            Ok(json!({"time": t.0}))
        }
        None => Ok(Value::Null),
    }
}

fn toggle_keyframe(app: &mut FilmcraftApp) -> Result<Value, String> {
    let (clip, it) = target_clip(app).ok_or_else(|| tl!("Select a clip first").to_string())?;
    let (effect, param, mask) = match app.ui.keyframe_selection.first() {
        Some(k) => (k.effect, k.param.clone(), k.mask),
        None => {
            let found = it.effects.iter().enumerate().find_map(|(i, e)| e.params.iter().find(|(_, p)| p.is_animated()).map(|(id, _)| (i, id.to_string())));
            let (i, p) = found.ok_or_else(|| tl!("The clip has no animated parameter: turn on animation in Effect Controls first").to_string())?;
            (i, p, None)
        }
    };
    let mut p = json!({"clip": clip, "effect": effect, "param": param});
    if let Some(m) = mask {
        p["mask"] = json!(m);
    }
    app.session.execute("effects.addKeyframe", p).map_err(|e| e.to_string())
}

/// Keys of the focused Effect Controls panel with keyframes selected: Left / Right move them one
/// frame, Alt+Left / Alt+Right go to the previous / next keyframe. Returns true when a key was used.
pub fn effect_controls_keys(app: &mut FilmcraftApp, ctx: &egui::Context) -> bool {
    if app.ui.focused != PanelKind::EffectControls || app.ui.keyframe_selection.is_empty() || ctx.egui_wants_keyboard_input() {
        return false;
    }
    let (left, right, alt_left, alt_right) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight),
            i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft),
            i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowRight),
        )
    });
    if alt_left || alt_right {
        if let Err(e) = go_keyframe(app, alt_right) {
            app.ui.status = e;
        }
        return true;
    }
    if !(left || right) {
        return false;
    }
    let Some((clip, it)) = target_clip(app) else { return true };
    let frame = app.session.sequence_rate().frame_duration();
    // one sequence frame in media time
    let step = (frame.0 as f64 * it.speed.abs().max(1e-6)) as i64;
    let delta = Tick(if left { -step } else { step });
    let refs: Vec<Value> = app
        .ui
        .keyframe_selection
        .iter()
        .filter(|k| k.clip == clip)
        .map(|k| {
            let mut v = json!({"effect": k.effect, "param": k.param, "mediaTime": k.time.0});
            if let Some(m) = k.mask {
                v["mask"] = json!(m);
            }
            v
        })
        .collect();
    match app.session.execute("effects.shiftKeyframes", json!({"clip": clip, "keyframes": refs, "delta": delta.0})) {
        Ok(_) => {
            for k in app.ui.keyframe_selection.iter_mut().filter(|k| k.clip == clip) {
                k.time += delta;
            }
        }
        Err(e) => app.ui.status = e.to_string(),
    }
    true
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    command_bar(app, ctx);
    caption_import(app, ctx);
}

/// Import Captions: the text encoding of the file (Automatic detects UTF-8, UTF-16 with a byte
/// order mark, else Windows-1252).
fn caption_import(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some((path, mut enc)) = app.ui.kp.caption_import.clone() else { return };
    let mut open = true;
    let mut done = false;
    egui::Window::new(tl!("Import Subtitle File")).id(egui::Id::new("Import Subtitle dialog")).open(&mut open).collapsible(false).resizable(false).show(
        ctx,
        |ui| {
            ui.label(egui::RichText::new(&path).color(app.tokens.text_dim));
            ui.horizontal(|ui| {
                ui.label(tl!("Encoding:"));
                let shown = if enc == "auto" { tl!("Automatic").to_string() } else { enc.clone() };
                let r = egui::ComboBox::from_id_salt("importSubtitle.encoding").selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut enc, "auto".to_string(), tl!("Automatic"));
                    for e in ["utf-8", "utf-16le", "utf-16be", "windows-1252", "latin-1"] {
                        ui.selectable_value(&mut enc, e.to_string(), e);
                    }
                });
                app.auto.add("importSubtitle.encoding", r.response.rect, "Encoding");
            });
            ui.horizontal(|ui| {
                if ui.button(tl!("Cancel")).clicked() {
                    done = true;
                }
                let r = ui.button(tl!("Import"));
                app.auto.add("importSubtitle.ok", r.rect, "Import");
                if r.clicked() {
                    if let Err(e) = app.session.execute("captions.import", json!({"path": path, "encoding": enc})) {
                        app.ui.status = e.to_string();
                    }
                    done = true;
                }
            });
        },
    );
    app.ui.kp.caption_import = if !open || done { None } else { Some((path, enc)) };
}

/// Find Action…: every menu command (and every frontend command) by its label, menu path or id;
/// Enter or a click runs it.
fn command_bar(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some((mut q, mut hi)) = app.ui.kp.command_bar.clone() else { return };
    let mut entries: Vec<(String, String, Option<String>)> = crate::menus::menu_items(app)
        .into_iter()
        .filter(|m| m.enabled)
        .map(|m| (m.id, if m.path.is_empty() { m.label } else { format!("{} ▸ {}", m.path.join(" ▸ "), m.label) }, m.shortcut))
        .collect();
    for c in crate::menus::UI_COMMANDS.iter().chain(crate::panels::keyboard::COMMANDS) {
        if !entries.iter().any(|e| e.0 == c.id) {
            entries.push((c.id.to_string(), crate::i18n::t(c.label).to_string(), None));
        }
    }
    let words: Vec<String> = q.to_lowercase().split_whitespace().map(str::to_string).collect();
    let shown: Vec<&(String, String, Option<String>)> = entries
        .iter()
        .filter(|(id, label, _)| words.iter().all(|w| crate::i18n::matches_query(label, w) || id.to_lowercase().contains(w.as_str())))
        .take(40)
        .collect();
    hi = hi.min(shown.len().saturating_sub(1));
    let mut open = true;
    let mut run: Option<String> = None;
    let (down, up, enter, esc) = ctx
        .input(|i| (i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    if down {
        hi = (hi + 1).min(shown.len().saturating_sub(1));
    }
    if up {
        hi = hi.saturating_sub(1);
    }
    egui::Window::new(tl!("Find Action"))
        .id(egui::Id::new("Find Action dialog"))
        .open(&mut open)
        .collapsible(false)
        .title_bar(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 80.0])
        .default_width(460.0)
        .show(ctx, |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut q).hint_text(tl!("Search for an action")).desired_width(f32::INFINITY));
            app.auto.add("commandBar.search", r.rect, "Search for an action");
            if !r.has_focus() {
                r.request_focus();
            }
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (i, (id, label, key)) in shown.iter().enumerate() {
                    let text = match key {
                        Some(k) => format!("{label}    {k}"),
                        None => label.clone(),
                    };
                    let b = ui.selectable_label(i == hi, text);
                    app.auto.add(&format!("commandBar.item.{i}"), b.rect, label);
                    if i == hi && (down || up) {
                        b.scroll_to_me(None);
                    }
                    if b.clicked() {
                        run = Some(id.clone());
                    }
                }
                if shown.is_empty() {
                    ui.label(egui::RichText::new(tl!("No matching action")).color(app.tokens.text_faint));
                }
            });
        });
    if enter && let Some(e) = shown.get(hi) {
        run = Some(e.0.clone());
    }
    if let Some(id) = run {
        app.ui.kp.command_bar = None;
        if let Err(e) = crate::menus::invoke(app, ctx, &id, json!({})) {
            app.ui.status = e;
        }
        return;
    }
    app.ui.kp.command_bar = if !open || esc { None } else { Some((q, hi)) };
}
