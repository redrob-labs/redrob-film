//! Window ▸ Workspaces, like Premiere: the built-in and the user's own workspaces in one
//! alphabetical list, then Reset to Saved Layout, Save Changes to this Workspace, Save as New
//! Workspace… and Edit Workspaces… (rename, delete). Saved layouts live in `workspaces.json` beside the
//! preferences file. A built-in workspace can be changed and reset but not deleted or renamed;
//! deleting its saved changes brings back the original. Edit Workspaces… also moves a workspace up
//! or down the list (the order is kept in `workspaces.json`) and exports a layout to a file or
//! imports one (`*.rfworkspace`, JSON).

use egui::Align2;
use serde_json::{Value, json};

use crate::FilmApp;
use crate::dock::{self, is_builtin, slug};
use crate::menus::MenuItem;
use crate::state::WorkspaceDialog;

/// Command ids under `window.workspace.` that are not workspace names.
const ACTIONS: [&str; 10] = ["reset", "saveChanges", "saveAs", "edit", "rename", "delete", "moveUp", "moveDown", "export", "import"];

/// Extension of exported workspace files.
pub const WORKSPACE_EXT: &str = "rfworkspace";

/// `window.workspace.*`: switch (`window.workspace.<name>`), `reset`, `saveChanges`,
/// `saveAs {name?}` (no name: open the dialog), `edit`, `rename {from, to}`, `delete {name}`,
/// `moveUp {name}` / `moveDown {name}` (the list's order), `export {name?, path?}` (default: the
/// current workspace; no path: a save dialog) and `import {path?}` (no path: an open dialog).
pub fn route(app: &mut FilmApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let ws = id.strip_prefix("window.workspace.")?;
    let arg = |k: &str| params.get(k).and_then(Value::as_str).map(str::to_string);
    let r = match ws {
        "reset" => {
            let n = app.ui.workspace.clone();
            app.set_workspace(&n);
            Ok(json!({"workspace": n}))
        }
        "saveChanges" => {
            let n = app.ui.workspace.clone();
            save(app, &n).map(|_| json!({"saved": n}))
        }
        "saveAs" => match arg("name") {
            Some(n) => save_new(app, &n),
            None => {
                app.ui.workspace_dialog = Some(WorkspaceDialog::SaveAs { name: tl!("Untitled Workspace").into() });
                Ok(json!({"dialog": "saveWorkspace"}))
            }
        },
        "edit" => {
            app.ui.workspace_dialog = Some(WorkspaceDialog::Edit { selected: None, name: String::new() });
            Ok(json!({"dialog": "editWorkspaces", "workspaces": dock::names(&app.workspaces)}))
        }
        "rename" => rename(app, &arg("from").unwrap_or_default(), &arg("to").unwrap_or_default()),
        "delete" => delete(app, &arg("name").unwrap_or_default()),
        "moveUp" | "moveDown" => move_in_list(app, &arg("name").unwrap_or_default(), ws == "moveUp"),
        "export" => {
            let name = arg("name").unwrap_or_else(|| app.ui.workspace.clone());
            export(app, &name, arg("path"))
        }
        "import" => import(app, arg("path")),
        key => match dock::names(&app.workspaces).into_iter().find(|n| slug(n) == key) {
            Some(n) => {
                app.set_workspace(&n);
                Ok(json!({"workspace": n}))
            }
            None => Err(format!("unknown workspace `{key}`")),
        },
    };
    Some(r)
}

/// Save the current layout as the saved layout of `name`.
fn save(app: &mut FilmApp, name: &str) -> Result<(), String> {
    let layout = serde_json::to_value(&app.ui.dock).map_err(|e| e.to_string())?;
    let mut next = app.workspaces.clone();
    let saved = &mut next.saved;
    match saved.iter_mut().find(|s| s.name == name) {
        Some(s) => s.layout = layout,
        None => saved.push(dock::SavedWorkspace { name: name.to_string(), layout }),
    }
    next.current = name.to_string();
    app.set_workspaces(next)
}

/// A trimmed new workspace name, or why it can't be used.
fn new_name(app: &FilmApp, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the workspace needs a name".into());
    }
    if ACTIONS.iter().any(|a| a.eq_ignore_ascii_case(&slug(name))) {
        return Err(format!("`{name}` can't be used as a workspace name"));
    }
    if let Some(n) = dock::find(&app.workspaces, name) {
        return Err(format!("a workspace named `{n}` already exists"));
    }
    Ok(name.to_string())
}

fn save_new(app: &mut FilmApp, name: &str) -> Result<Value, String> {
    let name = new_name(app, name)?;
    save(app, &name)?;
    app.ui.workspace = name.clone();
    Ok(json!({"saved": name, "workspace": name}))
}

fn rename(app: &mut FilmApp, from: &str, to: &str) -> Result<Value, String> {
    if is_builtin(from) {
        return Err(format!("`{from}` is a built-in workspace and keeps its name"));
    }
    if !app.workspaces.saved.iter().any(|s| s.name == from) {
        return Err(format!("unknown workspace `{from}`"));
    }
    let to = new_name(app, to)?;
    let mut next = app.workspaces.clone();
    for s in next.saved.iter_mut().filter(|s| s.name == from) {
        s.name = to.clone();
    }
    if next.current == from {
        next.current = to.clone();
    }
    app.set_workspaces(next)?;
    if app.ui.workspace == from {
        app.ui.workspace = to.clone();
    }
    Ok(json!({"renamed": from, "to": to}))
}

/// Delete a user workspace; on a built-in, drop its saved changes (back to the original).
fn delete(app: &mut FilmApp, name: &str) -> Result<Value, String> {
    let mut next = app.workspaces.clone();
    let before = next.saved.len();
    next.saved.retain(|s| s.name != name);
    if next.saved.len() == before {
        return Err(if is_builtin(name) { format!("`{name}` is a built-in workspace and can't be deleted") } else { format!("unknown workspace `{name}`") });
    }
    app.set_workspaces(next)?;
    if app.ui.workspace == name {
        app.set_workspace(if is_builtin(name) { name } else { "Editing" });
    }
    Ok(if is_builtin(name) { json!({"restored": name}) } else { json!({"deleted": name}) })
}

/// Move a workspace one place up or down the list; the whole list's order is stored from then on.
fn move_in_list(app: &mut FilmApp, name: &str, up: bool) -> Result<Value, String> {
    let mut order = dock::names(&app.workspaces);
    let i = order.iter().position(|n| n == name).ok_or_else(|| format!("unknown workspace `{name}`"))?;
    let j = if up { i.checked_sub(1) } else { (i + 1 < order.len()).then_some(i + 1) };
    if let Some(j) = j {
        order.swap(i, j);
        let mut next = app.workspaces.clone();
        next.order = order.clone();
        app.set_workspaces(next)?;
    }
    Ok(json!({"order": order}))
}

/// Write a workspace's saved layout (the current layout for the workspace in use) to a file.
fn export(app: &mut FilmApp, name: &str, path: Option<String>) -> Result<Value, String> {
    let name = dock::find(&app.workspaces, name).ok_or_else(|| format!("unknown workspace `{name}`"))?;
    let layout = if name == app.ui.workspace {
        serde_json::to_value(&app.ui.dock).map_err(|e| e.to_string())?
    } else {
        serde_json::to_value(dock::saved_layout(&app.workspaces, &name)).map_err(|e| e.to_string())?
    };
    let path = match path {
        Some(p) => p,
        None => {
            let suggested = format!("{name}.{WORKSPACE_EXT}");
            match app.hooks.pick_save_as.as_mut().and_then(|f| f(tl!("Workspace"), &[WORKSPACE_EXT], &suggested)) {
                Some(p) => p,
                None => return Ok(Value::Null),
            }
        }
    };
    let file = dock::WorkspaceFile { kind: dock::WORKSPACE_FILE_KIND.into(), name: name.clone(), layout };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| format!("writing {path}: {e}"))?;
    Ok(json!({"exported": name, "path": path}))
}

/// Add the workspace of a file written by `export` (a free name: `<name> 2`… when taken).
fn import(app: &mut FilmApp, path: Option<String>) -> Result<Value, String> {
    let path = match path {
        Some(p) => p,
        None => match app.hooks.pick_open_file.as_mut().and_then(|f| f(tl!("Workspace"), &[WORKSPACE_EXT, "json"])) {
            Some(p) => p,
            None => return Ok(Value::Null),
        },
    };
    let bytes = std::fs::read(&path).map_err(|e| format!("reading {path}: {e}"))?;
    let file: dock::WorkspaceFile = serde_json::from_slice(&bytes).map_err(|e| format!("{path} is not a workspace file: {e}"))?;
    if file.kind != dock::WORKSPACE_FILE_KIND {
        return Err(format!("{path} is not a workspace file"));
    }
    serde_json::from_value::<dock::DockNode>(file.layout.clone()).map_err(|e| format!("{path}: the layout can't be read: {e}"))?;
    let base = if file.name.trim().is_empty() { "Imported Workspace".to_string() } else { file.name.trim().to_string() };
    let mut name = base.clone();
    let mut n = 2;
    while new_name(app, &name).is_err() {
        name = format!("{base} {n}");
        n += 1;
        if n > 1000 {
            return Err("no free workspace name".into());
        }
    }
    let mut next = app.workspaces.clone();
    next.saved.push(dock::SavedWorkspace { name: name.clone(), layout: file.layout });
    app.set_workspaces(next)?;
    Ok(json!({"imported": name}))
}

/// Rebuild the Window ▸ Workspaces submenu: every workspace (the current one checked), then the
/// layout commands in table order.
pub fn menu(app: &FilmApp, items: &mut Vec<MenuItem>) {
    splice(items, &app.workspaces, &app.session, Some(&app.ui.workspace), |n| app.ui.language.tr(n).to_string());
}

/// The same submenu without a running app: the built-in workspaces in English, none checked
/// (`menus::default_menu_items`).
pub fn default_menu(items: &mut Vec<MenuItem>, session: &redrob_film_engine::Session) {
    splice(items, &dock::WorkspacePrefs::default(), session, None, str::to_string);
}

fn splice(items: &mut Vec<MenuItem>, prefs: &dock::WorkspacePrefs, session: &redrob_film_engine::Session, current: Option<&str>, tr: impl Fn(&str) -> String) {
    let is_ws = |it: &MenuItem| it.path.len() == 2 && it.path[0] == "Window" && it.path[1] == "Workspaces";
    let Some(at) = items.iter().position(is_ws) else { return };
    let actions: Vec<MenuItem> =
        items.iter().filter(|it| is_ws(it) && it.id.strip_prefix("window.workspace.").is_some_and(|k| ACTIONS.contains(&k))).cloned().collect();
    items.retain(|it| !is_ws(it));
    let mut sub: Vec<MenuItem> = dock::names(prefs)
        .into_iter()
        .map(|n| {
            let id = format!("window.workspace.{}", slug(&n));
            MenuItem {
                label: tr(&n),
                path: vec!["Window".into(), "Workspaces".into()],
                shortcut: session.shortcuts.primary(&id),
                enabled: true,
                checked: current.map(|c| n == c),
                id,
            }
        })
        .collect();
    sub.extend(actions);
    items.splice(at..at, sub);
}

/// Save as New Workspace… and Edit Workspaces… dialogs.
pub fn dialogs(app: &mut FilmApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.workspace_dialog.clone() else { return };
    let mut elems: Vec<(String, egui::Rect, String)> = Vec::new();
    let mut push = |id: &str, r: &egui::Response, label: &str| elems.push((id.to_string(), r.rect, label.to_string()));
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let mut act: Option<(&str, Value)> = None;
    let names = dock::names(&app.workspaces);
    let changed: Vec<String> = app.workspaces.saved.iter().map(|s| s.name.clone()).collect();
    let (title, shown) = match &d {
        WorkspaceDialog::SaveAs { .. } => ("New Workspace", tl!("New Workspace")),
        WorkspaceDialog::Edit { .. } => ("Edit Workspaces", tl!("Edit Workspaces")),
    };
    egui::Window::new(shown).id(egui::Id::new(title)).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(
        ctx,
        |ui| match &mut d {
            WorkspaceDialog::SaveAs { name } => {
                ui.horizontal(|ui| {
                    ui.label(tl!("Name:"));
                    let r = ui.text_edit_singleline(name);
                    push("workspaces.save.name", &r, "Name");
                });
                ui.horizontal(|ui| {
                    let r = ui.button(tl!("Cancel"));
                    push("workspaces.save.cancel", &r, "Cancel");
                    close |= r.clicked();
                    let r = ui.button(tl!("OK"));
                    push("workspaces.save.ok", &r, "OK");
                    if r.clicked() {
                        act = Some(("window.workspace.saveAs", json!({"name": name.clone()})));
                    }
                });
            }
            WorkspaceDialog::Edit { selected, name } => {
                for (i, n) in names.iter().enumerate() {
                    let r = ui.selectable_label(selected.as_ref() == Some(n), crate::i18n::t(n));
                    push(&format!("workspaces.edit.row.{i}"), &r, n);
                    if r.clicked() {
                        *selected = Some(n.clone());
                        *name = n.clone();
                    }
                }
                ui.separator();
                let sel = selected.clone().filter(|s| names.contains(s));
                let own = sel.as_deref().is_some_and(|s| !is_builtin(s));
                ui.horizontal(|ui| {
                    ui.label(tl!("Name:"));
                    let r = ui.add_enabled(own, egui::TextEdit::singleline(name));
                    push("workspaces.edit.name", &r, "Name");
                    let r = ui.add_enabled(own && sel.as_deref() != Some(name.trim()), egui::Button::new(tl!("Rename")));
                    push("workspaces.edit.rename", &r, "Rename");
                    if r.clicked()
                        && let Some(s) = &sel
                    {
                        act = Some(("window.workspace.rename", json!({"from": s, "to": name.clone()})));
                        *selected = Some(name.trim().to_string());
                    }
                });
                ui.horizontal(|ui| {
                    // a built-in workspace can't be deleted; its saved changes can
                    let restore = sel.as_deref().is_some_and(|s| is_builtin(s) && changed.iter().any(|c| c == s));
                    let label = if restore { "Restore Original" } else { "Delete" };
                    let r = ui.add_enabled(own || restore, egui::Button::new(if restore { tl!("Restore Original") } else { tl!("Delete") }));
                    push("workspaces.edit.delete", &r, label);
                    if r.clicked()
                        && let Some(s) = &sel
                    {
                        act = Some(("window.workspace.delete", json!({"name": s})));
                        if own {
                            *selected = None;
                            name.clear();
                        }
                    }
                    let r = ui.add_enabled(sel.is_some(), egui::Button::new(tl!("Move Up")));
                    push("workspaces.edit.moveUp", &r, "Move Up");
                    if r.clicked()
                        && let Some(s) = &sel
                    {
                        act = Some(("window.workspace.moveUp", json!({"name": s})));
                    }
                    let r = ui.add_enabled(sel.is_some(), egui::Button::new(tl!("Move Down")));
                    push("workspaces.edit.moveDown", &r, "Move Down");
                    if r.clicked()
                        && let Some(s) = &sel
                    {
                        act = Some(("window.workspace.moveDown", json!({"name": s})));
                    }
                });
                ui.horizontal(|ui| {
                    let r = ui.add_enabled(sel.is_some(), egui::Button::new(tl!("Export…")));
                    push("workspaces.edit.export", &r, "Export…");
                    if r.clicked()
                        && let Some(s) = &sel
                    {
                        act = Some(("window.workspace.export", json!({"name": s})));
                    }
                    let r = ui.button(tl!("Import…"));
                    push("workspaces.edit.import", &r, "Import…");
                    if r.clicked() {
                        act = Some(("window.workspace.import", json!({})));
                    }
                    let r = ui.button(tl!("Close"));
                    push("workspaces.edit.close", &r, "Close");
                    close |= r.clicked();
                });
            }
        },
    );
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    let keep_open = matches!(d, WorkspaceDialog::Edit { .. });
    app.ui.workspace_dialog = Some(d);
    if let Some((id, p)) = act {
        match route(app, id, &p) {
            Some(Err(e)) => app.ui.status = e,
            _ => close |= !keep_open,
        }
    }
    if close {
        app.ui.workspace_dialog = None;
    }
}
