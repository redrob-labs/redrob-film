//! Libraries panel (Kdenlive's Library): items and folders of the library folder, shared by every
//! project. Add Timeline Selection to Library, Add Folder, Add Clip to Project (also a
//! double-click), Rename, Delete, and drag an item or folder onto a folder to move it there. The
//! engine side is `redrob_film_engine::library` (`library.*`).
//!
//! Automation ids: `library.addSelection`, `library.newFolder`, `library.addToProject`,
//! `library.rename`, `library.delete`, `library.entry.<path>`, `library.name` (the name field),
//! `library.ok`.

use egui::{Align2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::{Value, json};

use crate::FilmApp;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

#[derive(Clone, Debug, Default)]
struct Entry {
    path: String,
    name: String,
    folder: bool,
    depth: usize,
}

/// What the name field at the bottom is asking for.
#[derive(Clone, Debug, PartialEq)]
enum Prompt {
    AddSelection,
    NewFolder,
    Rename(String),
}

#[derive(Clone, Debug, Default)]
struct PanelState {
    selected: Option<String>,
    prompt: Option<Prompt>,
    text: String,
    dragging: Option<String>,
    /// The name field takes the keyboard focus on its first frame.
    focus: bool,
    /// (read at, entries)
    cache: Option<(f64, Vec<Entry>)>,
}

fn state_id() -> egui::Id {
    egui::Id::new("library-panel")
}

/// The library's entries, re-read at most every two seconds (files moved outside show up).
fn entries(app: &mut FilmApp, now: f64, st: &mut PanelState) -> Vec<Entry> {
    if let Some((at, v)) = &st.cache
        && now - at < 2.0
    {
        return v.clone();
    }
    let v: Vec<Entry> = app
        .session
        .execute("library.list", json!({}))
        .ok()
        .and_then(|r| r.get("entries").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .map(|e| Entry {
            path: e["path"].as_str().unwrap_or_default().to_string(),
            name: e["name"].as_str().unwrap_or_default().to_string(),
            folder: e["folder"].as_bool().unwrap_or(false),
            depth: e["depth"].as_u64().unwrap_or(0) as usize,
        })
        .collect();
    st.cache = Some((now, v.clone()));
    v
}

/// The folder an entry is in ("" = the top).
fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

pub fn show(app: &mut FilmApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let mut st: PanelState = ctx.data(|d| d.get_temp::<PanelState>(state_id())).unwrap_or_default();
    let list = entries(app, now, &mut st);
    let mut run: Vec<(String, Value)> = Vec::new();

    // toolbar
    let bar = Rect::from_min_size(rect.min + vec2(6.0, 4.0), vec2(rect.width() - 12.0, 24.0));
    let mut x = bar.min.x;
    let has_clips = !app.session.state.selection.is_empty();
    let sel_item = st.selected.clone().filter(|p| list.iter().any(|e| e.path == *p));
    let sel_is_item = sel_item.as_ref().is_some_and(|p| list.iter().any(|e| e.path == *p && !e.folder));
    for (id, icon, tip, enabled) in [
        ("library.addSelection", Icon::Marker, tl!("Add Timeline Selection to Library"), has_clips),
        ("library.newFolder", Icon::Folder, tl!("Add Folder"), true),
        ("library.addToProject", Icon::NewItem, tl!("Add Clip to Project"), sel_is_item),
        ("library.rename", Icon::Type, tl!("Rename Library Clip"), sel_item.is_some()),
        ("library.delete", Icon::Trash, tl!("Delete"), sel_item.is_some()),
    ] {
        let r = Rect::from_min_size(pos2(x, bar.min.y), vec2(24.0, 22.0));
        let resp = ui.interact(r, egui::Id::new(("library-btn", id)), if enabled { Sense::click() } else { Sense::hover() }).on_hover_text(tip);
        app.auto.add(id, r, tip);
        if enabled && resp.hovered() {
            ui.painter().rect_filled(r, 3.0, t.hover);
        }
        icons::paint(ui.painter(), r.shrink(5.0), icon, if enabled { t.icon } else { t.text_faint });
        if enabled && resp.clicked() {
            match id {
                "library.addSelection" => {
                    st.prompt = Some(Prompt::AddSelection);
                    st.text = tl!("Library Clip").to_string();
                    st.focus = true;
                }
                "library.newFolder" => {
                    st.prompt = Some(Prompt::NewFolder);
                    st.text = tl!("New Folder").to_string();
                    st.focus = true;
                }
                "library.addToProject" => {
                    if let Some(p) = &sel_item {
                        run.push(("library.addToProject".into(), json!({"path": p})));
                    }
                }
                "library.rename" => {
                    if let Some(p) = &sel_item {
                        st.text = list.iter().find(|e| e.path == *p).map(|e| e.name.clone()).unwrap_or_default();
                        st.prompt = Some(Prompt::Rename(p.clone()));
                        st.focus = true;
                    }
                }
                _ => {
                    if let Some(p) = &sel_item {
                        run.push(("library.delete".into(), json!({"path": p})));
                        st.selected = None;
                    }
                }
            }
        }
        x = r.max.x + 4.0;
    }

    // name field (Add Timeline Selection, Add Folder, Rename)
    let mut top = bar.max.y + 4.0;
    if let Some(prompt) = st.prompt.clone() {
        let row = Rect::from_min_size(pos2(rect.min.x + 6.0, top), vec2(rect.width() - 12.0, 22.0));
        let fr = Rect::from_min_size(row.min, vec2((row.width() - 70.0).max(60.0), 22.0));
        let resp = ui.put(fr, egui::TextEdit::singleline(&mut st.text).id(egui::Id::new("library-name")));
        app.auto.add("library.name", fr, "Name");
        let ok = Rect::from_min_size(pos2(fr.max.x + 6.0, row.min.y), vec2(56.0, 22.0));
        let okr = ui.put(ok, egui::Button::new(tl!("OK")));
        app.auto.add("library.ok", ok, "OK");
        if st.focus {
            resp.request_focus();
            st.focus = false;
        }
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            st.prompt = None;
        } else if (okr.clicked() || enter) && !st.text.trim().is_empty() {
            // a new folder or item goes into the selected folder (or the selected item's folder)
            let folder = sel_item.as_ref().map(|p| if list.iter().any(|e| e.path == *p && e.folder) { p.as_str() } else { parent_of(p) }).unwrap_or("");
            let name = st.text.trim().to_string();
            run.push(match prompt {
                Prompt::AddSelection => ("library.addSelection".into(), json!({"name": name, "folder": folder})),
                Prompt::NewFolder => ("library.newFolder".into(), json!({"name": name, "parent": folder})),
                Prompt::Rename(p) => ("library.rename".into(), json!({"path": p, "name": name})),
            });
            st.prompt = None;
        }
        top = row.max.y + 4.0;
    }

    // entries
    let body = Rect::from_min_max(pos2(rect.min.x + 4.0, top), rect.max - vec2(4.0, 4.0));
    ui.painter().rect_filled(body, 4.0, t.panel_bg.gamma_multiply(0.92));
    if list.is_empty() {
        ui.painter().text(
            body.center(),
            Align2::CENTER_CENTER,
            tl!("Select clips in the timeline and add them to the library"),
            Tokens::ui(11.0),
            t.text_faint,
        );
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body.shrink(2.0)).id_salt("library-body"));
    child.set_clip_rect(body.intersect(ui.clip_rect()));
    let mut drop_on: Option<String> = None;
    egui::ScrollArea::vertical().id_salt("library-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        for e in &list {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click_and_drag());
            let selected = st.selected.as_deref() == Some(e.path.as_str());
            if selected {
                ui.painter().rect_filled(r, 0.0, t.row_selected);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 0.0, t.hover);
            }
            let x0 = r.min.x + 6.0 + e.depth as f32 * 14.0;
            let icon = if e.folder { Icon::Folder } else { Icon::Sequence };
            icons::paint(ui.painter(), Rect::from_center_size(pos2(x0 + 7.0, r.center().y), vec2(14.0, 14.0)), icon, t.icon);
            ui.painter().text(pos2(x0 + 20.0, r.center().y), Align2::LEFT_CENTER, &e.name, Tokens::ui(12.0), t.text);
            app.auto.add(&format!("library.entry.{}", e.path), r.intersect(ui.clip_rect()), &e.name);
            if resp.clicked() {
                st.selected = Some(e.path.clone());
            }
            if resp.double_clicked() && !e.folder {
                run.push(("library.addToProject".into(), json!({"path": e.path})));
            }
            if resp.drag_started() {
                st.dragging = Some(e.path.clone());
            }
            // a dragged entry over a folder: outline it, drop moves it in
            if let Some(d) = &st.dragging
                && e.folder
                && *d != e.path
                && ui.rect_contains_pointer(r)
            {
                ui.painter().rect_stroke(r, 3.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
                if ui.input(|i| i.pointer.any_released()) {
                    drop_on = Some(e.path.clone());
                }
            }
        }
        // dropping below the entries moves to the top of the library
        let rest = ui.available_rect_before_wrap();
        let rest = Rect::from_min_size(rest.min, vec2(rest.width(), rest.height().max(30.0)));
        let bg = ui.allocate_rect(rest, Sense::click());
        if bg.clicked() {
            st.selected = None;
        }
        if st.dragging.is_some() && ui.rect_contains_pointer(rest) && ui.input(|i| i.pointer.any_released()) {
            drop_on = Some(String::new());
        }
    });
    if let (Some(d), Some(folder)) = (st.dragging.clone(), drop_on) {
        if parent_of(&d) != folder {
            run.push(("library.move".into(), json!({"path": d, "folder": folder})));
        }
        st.dragging = None;
    }
    if ui.input(|i| i.pointer.any_released()) {
        st.dragging = None;
    }
    if !run.is_empty() {
        st.cache = None;
    }
    ctx.data_mut(|d| d.insert_temp(state_id(), st));
    for (cmd, p) in run {
        match app.session.execute(&cmd, p) {
            Ok(r) => {
                if cmd == "library.addToProject" {
                    app.ui.status = tl!("Added to the project bin").into();
                } else if let Some(path) = r.get("path").and_then(Value::as_str) {
                    let path = path.to_string();
                    ctx.data_mut(|d| d.get_temp_mut_or_default::<PanelState>(state_id()).selected = Some(path));
                }
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
}
