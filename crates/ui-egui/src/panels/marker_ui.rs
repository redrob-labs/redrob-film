//! Timeline markers, Kdenlive style: the ruler's right-click menu, double-click to add or edit,
//! dragging markers on the ruler, the Edit Marker dialog, the Markers panel (search, sort,
//! multi-selection, thumbnails, markers of every clip, add at an interval, delete, default
//! category, lock), Export Markers (chapter text / JSON, offset, copy to clipboard), Import
//! Markers, the project's marker categories, and dragging a clip's zone marker into the Timeline.
//!
//! Every action runs an engine command (`markers.*`). Automation ids: `rulerMenu.<key>`,
//! `editMarker.<position|name|comment|category|range|duration|ok|cancel>`,
//! `exportMarkers.<format|json|offset|category|copy|save|close>`, `markersPanel.<key>`,
//! `markerCategories.<n>.<name|delete>`, `markerCategories.<add|ok|cancel>`, `markers.row.<id>`.

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use redrob_film_project::{ItemId, Label, Marker};
use redrob_film_time::{Tick, TimeDisplay, format_time, parse_timecode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::FilmApp;
use crate::dock::PanelKind;
use crate::theme::Tokens;

/// How the Markers panel orders its rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortBy {
    #[default]
    Time,
    Category,
    Comment,
}

/// Saved with the interface state (`UiState::markers`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MarkerUi {
    pub search: String,
    pub sort: SortBy,
    pub descending: bool,
    /// List the markers of every bin clip below the sequence's own.
    pub all_clips: bool,
    pub thumbnails: bool,
    /// Selected rows (sequence marker ids, or clip marker ids).
    #[serde(skip)]
    pub selected: Vec<u64>,
    /// The row Shift+click extends from.
    #[serde(skip)]
    pub anchor: Option<u64>,
    #[serde(skip)]
    pub edit: Option<EditDraft>,
    #[serde(skip)]
    pub export: Option<ExportDraft>,
    #[serde(skip)]
    pub interval: Option<IntervalDraft>,
    #[serde(skip)]
    pub categories: Option<Vec<(Label, String)>>,
    /// A category being deleted while markers still use it: (category, reassign to, markers).
    #[serde(skip)]
    pub delete_category: Option<(Label, Option<Label>, usize)>,
    /// A ruler marker being dragged: (marker id, where it was).
    #[serde(skip)]
    pub drag: Option<(u64, Tick)>,
    /// A clip marker being dragged on a timeline clip: (bin item, marker id, timeline clip, resizing its range).
    #[serde(skip)]
    pub clip_drag: Option<(ItemId, u64, redrob_film_project::ClipId, bool)>,
    /// The Timeline time under the pointer (Align Playhead to Mouse Position).
    #[serde(skip)]
    pub hover_time: Option<Tick>,
}

/// The Edit Marker dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct EditDraft {
    pub marker: u64,
    /// The bin item of a clip marker; None = a sequence marker.
    pub item: Option<ItemId>,
    pub position: String,
    pub name: String,
    pub comment: String,
    pub color: Label,
    pub range: bool,
    pub duration: String,
}

/// The Export Markers dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportDraft {
    pub format: String,
    pub json: bool,
    pub offset: String,
    /// Only markers of this category.
    pub color: Option<Label>,
}

/// Markers panel ▸ Add at Interval.
#[derive(Clone, Debug, PartialEq)]
pub struct IntervalDraft {
    pub interval: String,
    pub count: u32,
    pub color: Label,
}

fn rgb(l: Label) -> Color32 {
    let c = l.marker_rgb();
    Color32::from_rgb(c[0], c[1], c[2])
}

fn tc(app: &FilmApp, t: Tick) -> String {
    let Some(q) = app.session.active_sequence() else { return String::new() };
    format_time(t, q.settings.frame_rate, q.settings.drop_frame, TimeDisplay::Timecode, 48_000)
}

/// Parse a timecode typed in a dialog (frames) for the active sequence.
fn parse(app: &FilmApp, s: &str) -> Option<i64> {
    let q = app.session.active_sequence()?;
    parse_timecode(s, q.settings.frame_rate, q.settings.drop_frame, 0).ok()
}

fn run(app: &mut FilmApp, ctx: &egui::Context, id: &str, p: Value) -> Option<Value> {
    match crate::menus::invoke(app, ctx, id, p) {
        Ok(v) => Some(v),
        Err(e) => {
            app.ui.status = e;
            None
        }
    }
}

/// Menu items routed here (they open a dialog when invoked without parameters).
pub fn route(app: &mut FilmApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let empty = params.as_object().is_none_or(|m| m.is_empty());
    match id {
        "markers.exportChapters" if empty => {
            app.ui.markers.export =
                Some(ExportDraft { format: redrob_film_engine::marker_tools::DEFAULT_CHAPTER_FORMAT.into(), json: false, offset: String::new(), color: None });
            Some(Ok(Value::Null))
        }
        "markers.import" if empty => {
            let paths = app.hooks.pick_files.as_mut().map(|f| f(&["json", "txt"])).unwrap_or_default();
            let Some(path) = paths.into_iter().next() else { return Some(Ok(Value::Null)) };
            Some(app.session.execute("markers.import", json!({"path": path})).map_err(|e| e.to_string()))
        }
        "markers.editDialog" => {
            let t = app.session.playhead();
            let m = app.session.active_sequence().and_then(|q| {
                let rate = q.settings.frame_rate;
                q.markers.iter().find(|m| rate.frame_at(m.start) == rate.frame_at(t)).map(|m| m.id.0)
            });
            Some(match m {
                Some(m) => {
                    open_edit(app, m);
                    Ok(Value::Null)
                }
                None => Err(tl!("There is no marker at the playhead").to_string()),
            })
        }
        "markers.categoriesDialog" => {
            open_categories(app);
            Some(Ok(Value::Null))
        }
        "playhead.alignToMouse" => {
            let Some(t) = app.ui.markers.hover_time else { return Some(Err(tl!("Point at the Timeline first").to_string())) };
            app.session.set_playhead(t.max(Tick::ZERO));
            Some(Ok(json!({"time": t.0})))
        }
        _ => None,
    }
}

/// Checkmark of Markers ▸ Timeline Markers Locked.
pub fn checked(app: &FilmApp, id: &str) -> Option<bool> {
    (id == "markers.lock").then(|| redrob_film_engine::marker_tools::markers_locked(&app.session))
}

/// Open the Edit Marker dialog on sequence marker `id`.
pub fn open_edit(app: &mut FilmApp, id: u64) {
    let Some(m) = app.session.active_sequence().and_then(|q| q.markers.iter().find(|m| m.id.0 == id)).cloned() else { return };
    app.ui.markers.edit = Some(draft_of(app, &m, None));
}

fn draft_of(app: &FilmApp, m: &Marker, item: Option<ItemId>) -> EditDraft {
    EditDraft {
        marker: m.id.0,
        item,
        position: tc(app, m.start),
        name: m.name.clone(),
        comment: m.comment.clone(),
        color: m.color,
        range: m.duration > Tick::ZERO,
        duration: tc(app, if m.duration > Tick::ZERO { m.duration } else { Tick::from_seconds_f64(1.0) }),
    }
}

fn open_categories(app: &mut FilmApp) {
    app.ui.markers.categories = Some(app.session.project.settings.marker_categories().into_iter().map(|c| (c.color, c.name)).collect());
}

// ------------------------------------------------------------------ ruler

/// A double-click on the ruler: on a marker, edit it; elsewhere, add one there.
pub fn ruler_double_click(app: &mut FilmApp, ctx: &egui::Context, marker: Option<u64>, t: Tick) {
    match marker {
        Some(m) => open_edit(app, m),
        None => {
            run(app, ctx, "markers.add", json!({"time": t.max(Tick::ZERO).0}));
        }
    }
}

/// Dragging a marker on the ruler: press on it starts, the pointer moves it, release commits
/// one `markers.edit` (locked markers do not drag). Returns true while a drag is in progress.
pub fn ruler_drag(app: &mut FilmApp, ctx: &egui::Context, press_marker: Option<u64>, pointer_t: Option<Tick>) -> bool {
    if let Some(m) = press_marker
        && app.ui.markers.drag.is_none()
        && !redrob_film_engine::marker_tools::markers_locked(&app.session)
        && let Some(start) = app.session.active_sequence().and_then(|q| q.markers.iter().find(|x| x.id.0 == m)).map(|x| x.start)
    {
        app.ui.markers.drag = Some((m, start));
    }
    let Some((m, from)) = app.ui.markers.drag else { return false };
    let released = ctx.input(|i| !i.pointer.primary_down());
    if released {
        app.ui.markers.drag = None;
        if let Some(t) = pointer_t
            && ctx.input(|i| i.pointer.press_origin().zip(i.pointer.interact_pos()).is_some_and(|(a, b)| (a - b).length() > 3.0))
            && t != from
        {
            run(app, ctx, "markers.edit", json!({"marker": m, "time": t.max(Tick::ZERO).0}));
            app.session.set_playhead(t.max(Tick::ZERO));
        }
    }
    true
}

/// The marker being dragged and where it would land (drawn as a ghost on the ruler).
pub fn drag_ghost(app: &FilmApp) -> Option<u64> {
    app.ui.markers.drag.map(|d| d.0)
}

/// The ruler's right-click menu (automation ids `rulerMenu.<key>`): `t` is the time clicked and
/// `marker` the marker under the pointer, if any.
pub fn ruler_menu(app: &mut FilmApp, ctx: &egui::Context, ui: &mut egui::Ui, t: Tick, marker: Option<u64>) {
    let t = t.max(Tick::ZERO);
    let item = |app: &mut FilmApp, ui: &mut egui::Ui, key: &str, label: &str, enabled: bool| -> bool {
        let r = ui.add_enabled(enabled, egui::Button::new(label));
        app.auto.add(&format!("rulerMenu.{key}"), r.rect, label);
        let c = r.clicked();
        if c {
            ui.close();
        }
        c
    };
    let rate = app.session.sequence_rate();
    let at_t =
        marker.or_else(|| app.session.active_sequence().and_then(|q| q.markers.iter().find(|m| rate.frame_at(m.start) == rate.frame_at(t)).map(|m| m.id.0)));
    if item(app, ui, "toggle", tl!("Add/Remove Timeline Marker"), true) {
        let at = at_t.and_then(|m| app.session.active_sequence().and_then(|q| q.markers.iter().find(|x| x.id.0 == m)).map(|x| x.start.0)).unwrap_or(t.0);
        run(app, ctx, "markers.toggleAtPlayhead", json!({"time": at}));
    }
    if item(app, ui, "edit", tl!("Edit Timeline Marker…"), at_t.is_some())
        && let Some(m) = at_t
    {
        open_edit(app, m);
    }
    let locked = redrob_film_engine::marker_tools::markers_locked(&app.session);
    let label = if locked { tl!("✓ Timeline Markers Locked") } else { tl!("Timeline Markers Locked") };
    if item(app, ui, "lock", label, true) {
        run(app, ctx, "markers.lock", json!({"on": !locked}));
    }
    if item(app, ui, "export", tl!("Export Markers…"), true) {
        run(app, ctx, "markers.exportChapters", json!({}));
    }
    let markers: Vec<(u64, Tick, String, Label)> = app
        .session
        .active_sequence()
        .map(|q| q.markers.iter().map(|m| (m.id.0, m.start, if m.name.is_empty() { m.comment.clone() } else { m.name.clone() }, m.color)).collect())
        .unwrap_or_default();
    ui.add_enabled_ui(!markers.is_empty(), |ui| {
        ui.menu_button(tl!("Go To Marker…"), |ui| {
            for (id, start, name, color) in markers.iter() {
                let text = format!("{}  {}", tc(app, *start), if name.is_empty() { tl!("Marker") } else { name.as_str() });
                let r = ui.button(egui::RichText::new(text).color(rgb(*color)));
                app.auto.add(&format!("rulerMenu.goTo.{id}"), r.rect, name);
                if r.clicked() {
                    app.session.set_playhead(*start);
                    ui.close();
                }
            }
        });
    });
    ui.separator();
    if item(app, ui, "zoneIn", tl!("Set Zone In"), true) {
        run(app, ctx, "markers.markIn", json!({"time": t.0}));
    }
    if item(app, ui, "zoneOut", tl!("Set Zone Out"), true) {
        run(app, ctx, "markers.markOut", json!({"time": t.0}));
    }
    let has_sel = !app.session.state.selection.is_empty();
    if item(app, ui, "zoneToSelection", tl!("Adjust Timeline Zone to Selection"), has_sel) {
        run(app, ctx, "markers.markSelection", json!({}));
    }
    let zone = app.session.active_sequence().is_some_and(|q| q.mark_in.is_some() && q.mark_out.is_some());
    if item(app, ui, "markerFromZone", tl!("Create Marker from Zone"), zone)
        && let Some(v) = run(app, ctx, "markers.addRangeInOut", json!({}))
        && let Some(m) = v.get("marker").and_then(Value::as_u64)
    {
        open_edit(app, m);
    }
    if item(app, ui, "markerFromZoneQuick", tl!("Create Marker from Zone Quickly"), zone) {
        run(app, ctx, "markers.addRangeInOut", json!({}));
    }
    ui.separator();
    if item(app, ui, "note", tl!("Add Project Note"), true) {
        let line = format!("{} ", tc(app, t));
        let mut notes = app.session.project.notes.clone();
        if !notes.is_empty() && !notes.ends_with('\n') {
            notes.push('\n');
        }
        notes.push_str(&line);
        run(app, ctx, "project.setNotes", json!({"text": notes}));
        app.show_panel(PanelKind::ProjectNotes);
    }
    let has_captions = app.session.active_sequence().is_some_and(|q| !q.caption_tracks.is_empty());
    if item(app, ui, "subtitle", tl!("Add Subtitle"), true) {
        if !has_captions {
            run(app, ctx, "captions.newTrack", json!({}));
        }
        run(app, ctx, "captions.add", json!({"time": t.0}));
    }
}

// ------------------------------------------------------------------ dialogs

pub fn show(app: &mut FilmApp, ctx: &egui::Context) {
    edit_dialog(app, ctx);
    export_dialog(app, ctx);
    interval_dialog(app, ctx);
    categories_dialog(app, ctx);
}

fn category_combo(app: &mut FilmApp, ui: &mut egui::Ui, id: &str, color: &mut Label) {
    let cats = app.session.project.settings.marker_categories();
    let name = app.session.project.settings.marker_category_name(*color);
    let r = egui::ComboBox::from_id_salt(id).selected_text(egui::RichText::new(name).color(rgb(*color))).show_ui(ui, |ui| {
        for c in &cats {
            ui.selectable_value(&mut *color, c.color, egui::RichText::new(&c.name).color(rgb(c.color)));
        }
    });
    app.auto.add(id, r.response.rect, "Category");
}

fn edit_dialog(app: &mut FilmApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.markers.edit.clone() else { return };
    let mut open = true;
    let mut ok = false;
    let mut cancel = false;
    egui::Window::new(tl!("Edit Marker")).id(egui::Id::new("Edit Marker dialog")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
        egui::Grid::new("edit-marker-grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(tl!("Position:"));
            let r = ui.add(egui::TextEdit::singleline(&mut d.position).desired_width(110.0).font(egui::TextStyle::Monospace));
            app.auto.add("editMarker.position", r.rect, "Position");
            ui.end_row();
            ui.label(tl!("Category:"));
            category_combo(app, ui, "editMarker.category", &mut d.color);
            ui.end_row();
            ui.label(tl!("Name:"));
            let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(220.0));
            app.auto.add("editMarker.name", r.rect, "Name");
            ui.end_row();
            ui.label(tl!("Comment:"));
            let r = ui.add(egui::TextEdit::multiline(&mut d.comment).desired_width(220.0).desired_rows(3));
            app.auto.add("editMarker.comment", r.rect, "Comment");
            ui.end_row();
            let r = ui.checkbox(&mut d.range, tl!("Range Marker"));
            app.auto.add("editMarker.range", r.rect, "Range Marker");
            ui.add_enabled_ui(d.range, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tl!("Duration:"));
                    let r = ui.add(egui::TextEdit::singleline(&mut d.duration).desired_width(110.0).font(egui::TextStyle::Monospace));
                    app.auto.add("editMarker.duration", r.rect, "Duration");
                });
            });
            ui.end_row();
        });
        ui.separator();
        ui.horizontal(|ui| {
            let r = ui.button(tl!("Cancel"));
            app.auto.add("editMarker.cancel", r.rect, "Cancel");
            cancel = r.clicked();
            let r = ui.button(tl!("OK"));
            app.auto.add("editMarker.ok", r.rect, "OK");
            ok = r.clicked();
        });
    });
    if ok {
        let Some(pos) = parse(app, &d.position) else {
            app.ui.status = tl!("The position is not a valid timecode").to_string();
            app.ui.markers.edit = Some(d);
            return;
        };
        let dur = if d.range { parse(app, &d.duration).unwrap_or(0).max(0) } else { 0 };
        let rate = app.session.sequence_rate();
        let r = match d.item {
            None => app.session.execute(
                "markers.edit",
                json!({"marker": d.marker, "frame": pos, "name": d.name, "comment": d.comment, "color": d.color.name(), "durationFrames": dur}),
            ),
            Some(item) => app.session.execute(
                "markers.moveClipMarker",
                json!({"item": item.0, "marker": d.marker, "time": rate.tick_of(pos).0, "duration": rate.tick_of(dur).0, "name": d.name, "comment": d.comment, "color": d.color.name()}),
            ),
        };
        match r {
            Ok(_) => app.ui.markers.edit = None,
            Err(e) => {
                app.ui.status = e.to_string();
                app.ui.markers.edit = Some(d);
            }
        }
        return;
    }
    app.ui.markers.edit = if !open || cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) { None } else { Some(d) };
}

fn export_dialog(app: &mut FilmApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.markers.export.clone() else { return };
    let mut open = true;
    let mut close = false;
    let offset = if d.offset.trim().is_empty() { Some(0) } else { parse(app, &d.offset) };
    let params = json!({"format": d.format, "json": d.json, "offsetFrames": offset.unwrap_or(0), "color": d.color.map(|c| c.name())});
    let (text, warnings) = redrob_film_engine::marker_tools::chapters(&app.session, &params).unwrap_or_default();
    egui::Window::new(tl!("Export markers as chapters description"))
        .id(egui::Id::new("Export Markers dialog"))
        .open(&mut open)
        .collapsible(false)
        .default_width(460.0)
        .show(ctx, |ui| {
            egui::Grid::new("export-markers-grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label(tl!("Marker Type:"));
                let cats = app.session.project.settings.marker_categories();
                let shown = d.color.map(|c| app.session.project.settings.marker_category_name(c)).unwrap_or_else(|| tl!("All Categories").to_string());
                let r = egui::ComboBox::from_id_salt("exportMarkers.category").selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.color, None, tl!("All Categories"));
                    for c in &cats {
                        ui.selectable_value(&mut d.color, Some(c.color), egui::RichText::new(&c.name).color(rgb(c.color)));
                    }
                });
                app.auto.add("exportMarkers.category", r.response.rect, "Marker Type");
                ui.end_row();
                ui.label(tl!("Save As:"));
                ui.horizontal(|ui| {
                    let a = ui.radio_value(&mut d.json, false, tl!("Text (YouTube)"));
                    let b = ui.radio_value(&mut d.json, true, tl!("JSON"));
                    app.auto.add("exportMarkers.text", a.rect, "Text");
                    app.auto.add("exportMarkers.json", b.rect, "JSON");
                });
                ui.end_row();
                ui.label(tl!("Offset:"));
                let r = ui.add(egui::TextEdit::singleline(&mut d.offset).hint_text("00:00:00:00").desired_width(110.0).font(egui::TextStyle::Monospace));
                app.auto.add("exportMarkers.offset", r.rect, "Offset");
                ui.end_row();
                ui.label(tl!("Format:"));
                ui.add_enabled_ui(!d.json, |ui| {
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::TextEdit::singleline(&mut d.format).desired_width(220.0));
                        app.auto.add("exportMarkers.format", r.rect, "Format");
                        ui.menu_button("i", |ui| {
                            for tok in ["{{timecode}}", "{{comment}}", "{{frame}}", "{{index}}", "{{nexttimecode}}", "{{realtimecode}}"] {
                                if ui.button(tok).clicked() {
                                    if !d.format.is_empty() && !d.format.ends_with(' ') {
                                        d.format.push(' ');
                                    }
                                    d.format.push_str(tok);
                                    ui.close();
                                }
                            }
                        });
                        if ui.button(tl!("Reset")).clicked() {
                            d.format = redrob_film_engine::marker_tools::DEFAULT_CHAPTER_FORMAT.into();
                        }
                    });
                });
                ui.end_row();
            });
            if offset.is_none() {
                ui.colored_label(app.tokens.hot_text, tl!("The offset is not a valid timecode"));
            }
            for w in &warnings {
                ui.colored_label(app.tokens.hot_text, crate::i18n::t(w));
            }
            ui.label(tl!("Exported:"));
            let mut shown = text.clone();
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut shown).desired_width(f32::INFINITY).desired_rows(8).font(egui::TextStyle::Monospace).interactive(false));
            });
            ui.horizontal(|ui| {
                let r = ui.button(tl!("Copy to Clipboard"));
                app.auto.add("exportMarkers.copy", r.rect, "Copy to Clipboard");
                if r.clicked() {
                    ctx.copy_text(text.clone());
                    app.ui.status = tl!("Markers copied to the clipboard").to_string();
                }
                let r = ui.button(tl!("Save…"));
                app.auto.add("exportMarkers.save", r.rect, "Save");
                if r.clicked() {
                    let name = if d.json { "markers.json" } else { "chapters.txt" };
                    if let Some(path) = app.hooks.pick_save.as_mut().and_then(|f| f(name)) {
                        let mut p = params.clone();
                        p["path"] = json!(path);
                        if let Err(e) = app.session.execute("markers.exportChapters", p) {
                            app.ui.status = e.to_string();
                        }
                    }
                }
                let r = ui.button(tl!("Close"));
                app.auto.add("exportMarkers.close", r.rect, "Close");
                close = r.clicked();
            });
        });
    app.ui.markers.export = if !open || close { None } else { Some(d) };
}

fn interval_dialog(app: &mut FilmApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.markers.interval.clone() else { return };
    let mut open = true;
    let mut done = false;
    egui::Window::new(tl!("Add Markers")).id(egui::Id::new("Add Markers dialog")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
        egui::Grid::new("interval-grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(tl!("Category:"));
            category_combo(app, ui, "markersPanel.interval.category", &mut d.color);
            ui.end_row();
            ui.label(tl!("Number of markers:"));
            let r = ui.add(egui::DragValue::new(&mut d.count).range(1..=1000));
            app.auto.add("markersPanel.interval.count", r.rect, "Number of markers");
            ui.end_row();
            ui.label(tl!("Interval:"));
            let r = ui.add(egui::TextEdit::singleline(&mut d.interval).desired_width(110.0).font(egui::TextStyle::Monospace));
            app.auto.add("markersPanel.interval.interval", r.rect, "Interval");
            ui.end_row();
        });
        ui.label(egui::RichText::new(tl!("Markers start at the playhead.")).color(app.tokens.text_dim));
        ui.horizontal(|ui| {
            if ui.button(tl!("Cancel")).clicked() {
                done = true;
            }
            let r = ui.button(tl!("OK"));
            app.auto.add("markersPanel.interval.ok", r.rect, "OK");
            if r.clicked() {
                match parse(app, &d.interval).filter(|f| *f > 0) {
                    Some(f) => {
                        let count = d.count;
                        let color = d.color.name();
                        run(app, ctx, "markers.addInterval", json!({"intervalFrames": f, "count": count, "color": color}));
                        done = true;
                    }
                    None => app.ui.status = tl!("The interval is not a valid timecode").to_string(),
                }
            }
        });
    });
    app.ui.markers.interval = if !open || done { None } else { Some(d) };
}

fn categories_dialog(app: &mut FilmApp, ctx: &egui::Context) {
    let Some(mut cats) = app.ui.markers.categories.clone() else { return };
    let mut open = true;
    let mut done = false;
    let mut remove: Option<usize> = None;
    egui::Window::new(tl!("Marker Categories")).id(egui::Id::new("Marker Categories dialog")).open(&mut open).collapsible(false).resizable(false).show(
        ctx,
        |ui| {
            ui.label(egui::RichText::new(tl!("Categories of this project (Project Settings ▸ Markers)")).color(app.tokens.text_dim));
            for (i, (color, name)) in cats.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("{}", i + 1));
                    let picked = *color;
                    egui::ComboBox::from_id_salt(("cat-color", i)).selected_text(egui::RichText::new("■").color(rgb(picked))).width(40.0).show_ui(ui, |ui| {
                        for l in Label::ALL {
                            ui.selectable_value(&mut *color, l, egui::RichText::new(format!("■ {}", crate::i18n::t(l.name()))).color(rgb(l)));
                        }
                    });
                    let r = ui.add(egui::TextEdit::singleline(&mut *name).desired_width(160.0));
                    app.auto.add(&format!("markerCategories.{}.name", i + 1), r.rect, name.as_str());
                    let r = ui.button("✕");
                    app.auto.add(&format!("markerCategories.{}.delete", i + 1), r.rect, "Delete");
                    if r.on_hover_text(tl!("Delete category")).clicked() {
                        remove = Some(i);
                    }
                });
            }
            let r = ui.button(tl!("Add Category"));
            app.auto.add("markerCategories.add", r.rect, "Add Category");
            if r.clicked()
                && let Some(l) = Label::ALL.into_iter().find(|l| !cats.iter().any(|c| c.0 == *l))
            {
                cats.push((l, crate::i18n::t(l.name()).to_string()));
            }
            ui.separator();
            ui.horizontal(|ui| {
                let r = ui.button(tl!("Cancel"));
                app.auto.add("markerCategories.cancel", r.rect, "Cancel");
                if r.clicked() {
                    done = true;
                }
                let r = ui.button(tl!("OK"));
                app.auto.add("markerCategories.ok", r.rect, "OK");
                if r.clicked() {
                    let list: Vec<Value> = cats.iter().map(|(c, n)| json!({"color": c.name(), "name": n})).collect();
                    if let Some(e) = app.session.execute("markers.categories", json!({"categories": list})).err() {
                        app.ui.status = e.to_string();
                    } else {
                        done = true;
                    }
                }
            });
        },
    );
    if let Some(i) = remove {
        let color = cats[i].0;
        let used: usize =
            app.session.project.sequences().filter_map(|it| it.as_sequence()).map(|q| q.markers.iter().filter(|m| m.color == color).count()).sum();
        let saved = app.session.project.settings.marker_categories().iter().any(|c| c.color == color);
        if used > 0 && saved {
            let other = cats.iter().map(|c| c.0).find(|c| *c != color);
            app.ui.markers.delete_category = Some((color, other, used));
        } else {
            cats.remove(i);
        }
    }
    delete_category_dialog(app, ctx, &mut cats);
    app.ui.markers.categories = if !open || done { None } else { Some(cats) };
}

/// Deleting a category its markers still use: reassign them to another category, or delete them.
fn delete_category_dialog(app: &mut FilmApp, ctx: &egui::Context, cats: &mut Vec<(Label, String)>) {
    let Some((color, mut to, used)) = app.ui.markers.delete_category else { return };
    let mut close = false;
    egui::Window::new(tl!("Delete Category")).id(egui::Id::new("Delete Marker Category dialog")).collapsible(false).resizable(false).show(ctx, |ui| {
        ui.label(tlf!("{n} markers use this category.", n = used));
        ui.horizontal(|ui| {
            ui.label(tl!("Reassign them to:"));
            let others: Vec<(Label, String)> = cats.iter().filter(|c| c.0 != color).cloned().collect();
            let shown = to.and_then(|t| others.iter().find(|c| c.0 == t).map(|c| c.1.clone())).unwrap_or_default();
            egui::ComboBox::from_id_salt("delete-category-to").selected_text(shown).show_ui(ui, |ui| {
                for (l, n) in &others {
                    ui.selectable_value(&mut to, Some(*l), egui::RichText::new(n).color(rgb(*l)));
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.button(tl!("Cancel")).clicked() {
                close = true;
            }
            if ui.add_enabled(to.is_some(), egui::Button::new(tl!("Reassign and Delete"))).clicked() {
                let r = app.session.execute("markers.deleteCategory", json!({"color": color.name(), "reassign": to.map(|l| l.name())}));
                finish_delete(app, cats, color, r);
                close = true;
            }
            if ui.button(tl!("Delete Markers Too")).clicked() {
                let r = app.session.execute("markers.deleteCategory", json!({"color": color.name(), "deleteMarkers": true}));
                finish_delete(app, cats, color, r);
                close = true;
            }
        });
    });
    app.ui.markers.delete_category = if close { None } else { Some((color, to, used)) };
}

fn finish_delete<E: std::fmt::Display>(app: &mut FilmApp, cats: &mut Vec<(Label, String)>, color: Label, r: Result<Value, E>) {
    match r {
        Ok(_) => cats.retain(|c| c.0 != color),
        Err(e) => app.ui.status = e.to_string(),
    }
}

// ------------------------------------------------------------------ panel

/// One row of the Markers panel.
#[derive(Clone)]
struct Row {
    /// Bin item of a clip marker (None = the sequence's own marker).
    item: Option<ItemId>,
    item_name: String,
    m: Marker,
}

fn rows(app: &FilmApp) -> Vec<Row> {
    let mu = &app.ui.markers;
    let hidden = &app.session.state.hidden_marker_colors;
    let q = mu.search.to_lowercase();
    let mut out: Vec<Row> = Vec::new();
    if let Some(seq) = app.session.active_sequence() {
        out.extend(seq.markers.iter().map(|m| Row { item: None, item_name: String::new(), m: m.clone() }));
    }
    if mu.all_clips {
        for it in app.session.project.items.values() {
            if let redrob_film_project::ItemKind::Media(mc) = &it.kind {
                out.extend(mc.markers.iter().map(|m| Row { item: Some(it.id), item_name: it.name.clone(), m: m.clone() }));
            }
        }
    }
    out.retain(|r| !hidden.iter().any(|h| h.marker_rgb() == r.m.color.marker_rgb()));
    if !q.is_empty() {
        out.retain(|r| r.m.name.to_lowercase().contains(&q) || r.m.comment.to_lowercase().contains(&q) || r.item_name.to_lowercase().contains(&q));
    }
    let cats = app.session.project.settings.marker_categories();
    let cat_index = |l: Label| cats.iter().position(|c| c.color == l).unwrap_or(usize::MAX);
    match mu.sort {
        SortBy::Time => out.sort_by_key(|r| (r.item.map(|i| i.0).unwrap_or(0), r.m.start)),
        SortBy::Category => out.sort_by_key(|r| (cat_index(r.m.color), r.m.start)),
        SortBy::Comment => out.sort_by_key(|r| (if r.m.name.is_empty() { r.m.comment.to_lowercase() } else { r.m.name.to_lowercase() }, r.m.start)),
    }
    if mu.descending {
        out.reverse();
    }
    out
}

/// Keys of the focused Markers panel: F2 edits the selected marker, Delete removes the selection,
/// Cmd+A selects every row, Up / Down move the selection. Returns true when a key was used.
pub fn panel_keys(app: &mut FilmApp, ctx: &egui::Context) -> bool {
    if app.ui.focused != PanelKind::Markers || ctx.egui_wants_keyboard_input() {
        return false;
    }
    let (f2, del, all, up, down) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::F2),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace),
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
        )
    });
    let list = rows(app);
    if all {
        app.ui.markers.selected = list.iter().map(|r| r.m.id.0).collect();
    }
    if up || down {
        let cur = app.ui.markers.selected.last().and_then(|s| list.iter().position(|r| r.m.id.0 == *s));
        let next = match cur {
            None => 0,
            Some(i) if up => i.saturating_sub(1),
            Some(i) => (i + 1).min(list.len().saturating_sub(1)),
        };
        if let Some(r) = list.get(next) {
            select_row(app, r, false, false, &list);
        }
    }
    if f2 && let Some(r) = app.ui.markers.selected.last().and_then(|s| list.iter().find(|r| r.m.id.0 == *s)).cloned() {
        app.ui.markers.edit = Some(draft_of(app, &r.m, r.item));
    }
    if del {
        delete_selected(app, ctx, &list);
    }
    f2 || del || all || up || down
}

fn delete_selected(app: &mut FilmApp, ctx: &egui::Context, list: &[Row]) {
    let sel = app.ui.markers.selected.clone();
    let seq: Vec<u64> = list.iter().filter(|r| r.item.is_none() && sel.contains(&r.m.id.0)).map(|r| r.m.id.0).collect();
    if !seq.is_empty() {
        run(app, ctx, "markers.delete", json!({"markers": seq}));
    }
    // clip markers: on the bin clip that holds them
    for r in list.iter().filter(|r| sel.contains(&r.m.id.0)) {
        if let Some(item) = r.item {
            let _ = app.session.execute("markers.deleteClipMarker", json!({"item": item.0, "marker": r.m.id.0}));
        }
    }
    app.ui.markers.selected.clear();
}

fn select_row(app: &mut FilmApp, r: &Row, ctrl: bool, shift: bool, list: &[Row]) {
    let id = r.m.id.0;
    let mu = &mut app.ui.markers;
    if shift && let Some(a) = mu.anchor {
        let ia = list.iter().position(|x| x.m.id.0 == a);
        let ib = list.iter().position(|x| x.m.id.0 == id);
        if let (Some(ia), Some(ib)) = (ia, ib) {
            let (lo, hi) = (ia.min(ib), ia.max(ib));
            mu.selected = list[lo..=hi].iter().map(|x| x.m.id.0).collect();
        }
    } else if ctrl {
        if let Some(p) = mu.selected.iter().position(|x| *x == id) {
            mu.selected.remove(p);
        } else {
            mu.selected.push(id);
        }
        mu.anchor = Some(id);
    } else {
        mu.selected = vec![id];
        mu.anchor = Some(id);
    }
    // the playhead goes to the marker (a clip marker opens its clip in the Source monitor)
    match r.item {
        None => app.session.set_playhead(r.m.start),
        Some(item) => {
            if app.session.state.source_item != Some(item) {
                let _ = app.session.execute("source.open", json!({"item": item.0}));
            }
            let _ = app.session.execute("source.setPlayhead", json!({"time": r.m.start.0}));
        }
    }
}

/// The Markers panel.
pub fn panel(app: &mut FilmApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    if app.session.active_sequence().is_none() && !app.ui.markers.all_clips {
        crate::dock::placeholder(ui, rect, &t, tl!("(no sequence)"));
        return;
    }
    let ctx = ui.ctx().clone();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(6.0)));
    let ui = &mut child;
    let mut elems: Vec<(String, Rect, String)> = Vec::new();
    // ---- search, sort, view toggles
    ui.horizontal(|ui| {
        let w = (ui.available_width() * 0.4).clamp(80.0, 200.0);
        let r = crate::widgets::search_field(ui, &mut app.ui.markers.search, tl!("Search markers"), w, &t);
        elems.push(("markersPanel.search".into(), r.rect, "Search markers".into()));
        let mu = &mut app.ui.markers;
        let label = match mu.sort {
            SortBy::Time => tl!("Sort by Time"),
            SortBy::Category => tl!("Sort by Category"),
            SortBy::Comment => tl!("Sort by Comment"),
        };
        let r = egui::ComboBox::from_id_salt("markersPanel.sort").selected_text(label).show_ui(ui, |ui| {
            ui.selectable_value(&mut mu.sort, SortBy::Time, tl!("Sort by Time"));
            ui.selectable_value(&mut mu.sort, SortBy::Category, tl!("Sort by Category"));
            ui.selectable_value(&mut mu.sort, SortBy::Comment, tl!("Sort by Comment"));
        });
        elems.push(("markersPanel.sort".into(), r.response.rect, "Sort".into()));
        let r = ui.button(if mu.descending { "↓" } else { "↑" }).on_hover_text(tl!("Ascending / descending"));
        if r.clicked() {
            mu.descending = !mu.descending;
        }
        elems.push(("markersPanel.order".into(), r.rect, "Sort order".into()));
        let r = ui.checkbox(&mut mu.all_clips, tl!("All Clips")).on_hover_text(tl!("Show the markers of every clip in the project"));
        elems.push(("markersPanel.allClips".into(), r.rect, "All Clips".into()));
        let r = ui.checkbox(&mut mu.thumbnails, tl!("Thumbnails"));
        elems.push(("markersPanel.thumbnails".into(), r.rect, "Thumbnails".into()));
    });
    // ---- category filter and default category
    let hidden: Vec<[u8; 3]> = app.session.state.hidden_marker_colors.iter().map(|l| l.marker_rgb()).collect();
    let mut toggle = None;
    let cats = app.session.project.settings.marker_categories();
    ui.horizontal(|ui| {
        for c in &cats {
            let l = c.color;
            let on = !hidden.contains(&l.marker_rgb());
            let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
            if on {
                ui.painter().rect_filled(r.shrink(2.0), 3.0, rgb(l));
            } else {
                ui.painter().rect_stroke(r.shrink(2.5), 3.0, egui::Stroke::new(1.0, rgb(l)), egui::StrokeKind::Inside);
            }
            elems.push((format!("markers.filter.{}", l.name()), r, format!("{} markers {}", c.name, if on { "shown" } else { "hidden" })));
            if resp.on_hover_text(tlf!("Show or hide {color} markers", color = &c.name)).clicked() {
                toggle = Some((l, !on));
            }
        }
        ui.separator();
        ui.label(tl!("Default:"));
        let mut d = app.session.project.settings.default_marker_color();
        let before = d;
        category_combo(app, ui, "markersPanel.defaultCategory", &mut d);
        if d != before
            && let Err(e) = app.session.execute("markers.categories", json!({"default": d.name()}))
        {
            app.ui.status = e.to_string();
        }
    });
    // ---- actions
    let list = rows(app);
    let sel = app.ui.markers.selected.clone();
    let locked = redrob_film_engine::marker_tools::markers_locked(&app.session);
    ui.horizontal(|ui| {
        let mut b = |ui: &mut egui::Ui, key: &str, label: &str, tip: &str, enabled: bool| -> bool {
            let r = ui.add_enabled(enabled, egui::Button::new(label).small()).on_hover_text(tip);
            elems.push((format!("markersPanel.{key}"), r.rect, tip.to_string()));
            r.clicked()
        };
        let has_seq = app.session.active_sequence().is_some();
        if b(ui, "add", "+", tl!("Add Marker"), has_seq) {
            run(app, &ctx, "markers.add", json!({}));
        }
        if b(ui, "addInterval", "+…", tl!("Add Markers at Interval…"), has_seq) {
            let one_sec = tc(app, Tick::from_seconds_f64(10.0));
            let color = app.session.project.settings.default_marker_color();
            app.ui.markers.interval = Some(IntervalDraft { interval: one_sec, count: 5, color });
        }
        if b(ui, "edit", "✎", tl!("Edit Marker"), sel.len() == 1)
            && let Some(r) = list.iter().find(|r| sel.contains(&r.m.id.0))
        {
            app.ui.markers.edit = Some(draft_of(app, &r.m, r.item));
        }
        if b(ui, "delete", "🗑", tl!("Delete Selected Markers"), !sel.is_empty()) {
            delete_selected(app, &ctx, &list);
        }
        let lock_tip = if locked { tl!("Timeline markers are locked") } else { tl!("Lock timeline markers") };
        if b(ui, "lock", if locked { "🔒" } else { "🔓" }, lock_tip, has_seq) {
            run(app, &ctx, "markers.lock", json!({"on": !locked}));
        }
        ui.menu_button("☰", |ui| {
            if ui.button(tl!("Import Markers…")).clicked() {
                run(app, &ctx, "markers.import", json!({}));
                ui.close();
            }
            if ui.button(tl!("Export Markers…")).clicked() {
                run(app, &ctx, "markers.exportChapters", json!({}));
                ui.close();
            }
            if ui.button(tl!("Configure Categories…")).clicked() {
                open_categories(app);
                ui.close();
            }
        });
    });
    ui.add_space(4.0);
    // ---- the list
    let thumbs = app.ui.markers.thumbnails;
    let row_h = if thumbs { 52.0 } else { 38.0 };
    let seq_item = app.session.state.active_sequence;
    let mut clicked: Option<(Row, bool, bool)> = None;
    let mut double: Option<Row> = None;
    let mut drag: Option<Row> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for r in &list {
            let (rr, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
            let m = &r.m;
            if sel.contains(&m.id.0) {
                ui.painter().rect_filled(rr, 3.0, t.row_selected);
            } else if resp.hovered() {
                ui.painter().rect_filled(rr, 3.0, t.hover);
            }
            ui.painter().rect_filled(Rect::from_min_size(rr.min + vec2(4.0, 6.0), vec2(4.0, row_h - 12.0)), 2.0, rgb(m.color));
            let mut x = rr.min.x + 16.0;
            if thumbs
                && let Some(item) = r.item.or(seq_item)
                && let Some((tex, size)) = app.thumbnail(&ctx, item, m.start, 80)
            {
                let h = row_h - 8.0;
                let w = (size.x / size.y.max(1.0) * h).min(80.0);
                ui.painter().image(
                    tex,
                    Rect::from_min_size(pos2(x, rr.min.y + 4.0), vec2(w, h)),
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                x += w + 8.0;
            }
            let name = if !m.name.is_empty() {
                m.name.clone()
            } else if !m.comment.is_empty() {
                m.comment.clone()
            } else {
                tl!("Marker").to_string()
            };
            let title = if r.item.is_some() { format!("{}  ·  {}", r.item_name, name) } else { name.clone() };
            ui.painter().text(pos2(x, rr.min.y + row_h * 0.32), Align2::LEFT_CENTER, title, Tokens::ui(12.0), t.text);
            let mut line = tc(app, m.start);
            if m.duration > Tick::ZERO {
                line = format!("{line}  ({})", tc(app, m.duration));
            }
            line = format!("{line}  {}", app.session.project.settings.marker_category_name(m.color));
            ui.painter().text(pos2(x, rr.min.y + row_h * 0.7), Align2::LEFT_CENTER, line, Tokens::mono(11.0), t.hot_text);
            elems.push((format!("markers.row.{}", m.id.0), rr, name));
            if resp.double_clicked() {
                double = Some(r.clone());
            } else if resp.clicked() {
                let mods = ui.input(|i| i.modifiers);
                clicked = Some((r.clone(), mods.command, mods.shift));
            }
            // a clip's zone marker drags into the Timeline as that part of the clip
            if resp.drag_started() && r.item.is_some() && m.duration > Tick::ZERO {
                drag = Some(r.clone());
            }
        }
        if list.is_empty() {
            let text = if app.session.active_sequence().is_some_and(|q| q.markers.is_empty()) && app.ui.markers.search.is_empty() {
                tl!("No markers. Press M to add one.")
            } else {
                tl!("No marker matches the search or the category filter.")
            };
            ui.label(egui::RichText::new(text).color(t.text_faint));
        }
    });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if let Some((l, visible)) = toggle
        && let Err(e) = app.session.execute("markers.filterColors", json!({"color": l.name(), "visible": visible}))
    {
        app.ui.status = e.to_string();
    }
    if let Some((r, ctrl, shift)) = clicked {
        select_row(app, &r, ctrl, shift, &list);
    }
    if let Some(r) = double {
        app.ui.markers.selected = vec![r.m.id.0];
        app.ui.markers.edit = Some(draft_of(app, &r.m, r.item));
    }
    if let Some(r) = drag
        && let Some(item) = r.item
        && let Some(pi) = app.session.project.item(item)
    {
        let (video, audio) = (pi.has_video(), pi.has_audio());
        crate::panels::start_drag_source(
            ui,
            crate::panels::source_drag::SourceDrag { item, range: redrob_film_time::TimeRange { start: r.m.start, duration: r.m.duration }, video, audio },
        );
    }
}
