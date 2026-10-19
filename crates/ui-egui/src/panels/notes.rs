//! Project Notes panel (#620): free plain text kept in the project file, for to-dos,
//! instructions for whoever opens the project next, frame numbers and the like. Every change is
//! a `project.setNotes`; one typing session (from focusing the editor until it loses focus) is
//! one undo step.
//!
//! Timecodes written in the notes (`01:00:12:05`) are links to that moment of the active sequence
//! (`project.notesTimecodes`): they are listed under the editor, a click on one (or Ctrl/⌘+click
//! on it in the text) moves the playhead there. The toolbar writes the playhead's timecode at the
//! caret (`project.notesInsertTimecode`), makes sequence markers of the timecodes in the selected
//! text, or all of them (`project.notesMarkers`), and finds text in the notes (Ctrl/⌘+F, F3 next,
//! Shift+F3 previous).
//!
//! Automation ids: `notes.text` (the editor; its label is the start of the notes),
//! `notes.insertTimecode`, `notes.createMarkers`, `notes.find`, `notes.findPrevious`,
//! `notes.findNext`, `notes.timecode.<n>` (the links). Agents read and write the notes with
//! `project.notes` / `project.setNotes`.

use egui::text::{CCursor, CCursorRange};
use egui::{Rect, pos2, vec2};
use serde_json::json;

use crate::FilmcraftApp;

/// Longest automation label (characters): the full notes are `project.notes`.
const LABEL_CHARS: usize = 200;
const TOOLBAR_H: f32 = 26.0;
const LINKS_H: f32 = 24.0;

fn editor_id() -> egui::Id {
    egui::Id::new("project-notes-editor")
}

/// The editor's selection as sorted character offsets (caret = both equal), if it has one.
fn selection(ctx: &egui::Context) -> Option<(usize, usize)> {
    let st = egui::text_edit::TextEditState::load(ctx, editor_id())?;
    let [a, b] = st.cursor.char_range()?.sorted_cursors();
    Some((a.index.0, b.index.0))
}

/// Select characters `a..b` of the editor and give it the focus.
fn select(ctx: &egui::Context, a: usize, b: usize) {
    let mut st = egui::text_edit::TextEditState::load(ctx, editor_id()).unwrap_or_default();
    st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
    st.store(ctx, editor_id());
    ctx.memory_mut(|m| m.request_focus(editor_id()));
}

/// Character offsets of every case-insensitive occurrence of `query` in `text`.
fn find_all(text: &str, query: &str) -> Vec<(usize, usize)> {
    let low = |c: char| c.to_lowercase().next().unwrap_or(c);
    let hay: Vec<char> = text.chars().map(low).collect();
    let needle: Vec<char> = query.chars().map(low).collect();
    if needle.is_empty() || needle.len() > hay.len() {
        return Vec::new();
    }
    (0..=hay.len() - needle.len()).filter(|&i| hay[i..i + needle.len()] == needle[..]).map(|i| (i, i + needle.len())).collect()
}

/// Select the next (or previous) match after (before) the caret, wrapping around.
fn find_step(app: &mut FilmcraftApp, ctx: &egui::Context, query: &str, forward: bool) {
    let hits = find_all(&app.session.project.notes, query);
    if hits.is_empty() {
        app.ui.status = tl!("Not found in the project notes").into();
        return;
    }
    let (a, b) = selection(ctx).unwrap_or((0, 0));
    let hit = if forward {
        hits.iter().find(|h| h.0 > a || (h.0 == a && b == a)).or(hits.first())
    } else {
        hits.iter().rev().find(|h| h.0 < a).or(hits.last())
    };
    if let Some(&(x, y)) = hit {
        select(ctx, x, y);
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let inner = rect.shrink2(vec2(8.0, 6.0));
    let timecodes = redrob_film_engine::panels::active_note_timecodes(&app.session);
    let links_h = if timecodes.is_empty() { 0.0 } else { LINKS_H };
    let bar = Rect::from_min_size(inner.min, vec2(inner.width(), TOOLBAR_H));
    let edit_rect = Rect::from_min_max(pos2(inner.min.x, bar.max.y + 4.0), pos2(inner.max.x, inner.max.y - links_h));
    let find_id = egui::Id::new("project-notes-find");
    let mut query: String = ctx.data(|d| d.get_temp(find_id)).unwrap_or_default();

    // ---- toolbar: Insert Timecode | Create Markers | find field ◀ ▶
    let mut action: Option<&'static str> = None;
    {
        let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let has_seq = app.session.active_sequence().is_some();
        let r = bui.add_enabled(has_seq, egui::Button::new(tl!("Insert Timecode"))).on_hover_text(tl!("Write the playhead's timecode at the caret"));
        app.auto.add("notes.insertTimecode", r.rect, "Insert Timecode");
        if r.clicked() {
            action = Some("insert");
        }
        let r = bui
            .add_enabled(has_seq && !timecodes.is_empty(), egui::Button::new(tl!("Create Markers")))
            .on_hover_text(tl!("Add a sequence marker at each timecode of the selected text (or of all the notes)"));
        app.auto.add("notes.createMarkers", r.rect, "Create Markers");
        if r.clicked() {
            action = Some("markers");
        }
        bui.add_space(8.0);
        let fw = (bui.available_width() - 64.0).clamp(60.0, 240.0);
        let fr = bui.add(egui::TextEdit::singleline(&mut query).id(find_id.with("field")).hint_text(tl!("Find in notes")).desired_width(fw));
        app.auto.add("notes.find", fr.rect, &query);
        if fr.lost_focus() && bui.input(|i| i.key_pressed(egui::Key::Enter)) {
            action = Some(if bui.input(|i| i.modifiers.shift) { "prev" } else { "next" });
        }
        let r = bui.add_enabled(!query.is_empty(), egui::Button::new("◀")).on_hover_text(tl!("Find Previous (Shift+F3)"));
        app.auto.add("notes.findPrevious", r.rect, "Find Previous");
        if r.clicked() {
            action = Some("prev");
        }
        let r = bui.add_enabled(!query.is_empty(), egui::Button::new("▶")).on_hover_text(tl!("Find Next (F3)"));
        app.auto.add("notes.findNext", r.rect, "Find Next");
        if r.clicked() {
            action = Some("next");
        }
        // Ctrl/⌘+F, F3 and Shift+F3 while the notes or their find field have the keyboard
        let mine = ctx.memory(|m| m.focused()).is_some_and(|f| f == editor_id() || f == find_id.with("field"));
        if mine {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
                ctx.memory_mut(|m| m.request_focus(find_id.with("field")));
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F3)) {
                action = Some("prev");
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F3)) {
                action = Some("next");
            }
        }
    }
    ctx.data_mut(|d| d.insert_temp(find_id, query.clone()));

    // ---- the editor
    let mut text = app.session.project.notes.clone();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect));
    let eui = &mut child;
    let height = eui.available_height();
    let r = egui::ScrollArea::vertical()
        .id_salt("project-notes-scroll")
        .auto_shrink([false, false])
        .show(eui, |eui| {
            eui.add(
                egui::TextEdit::multiline(&mut text)
                    .id(editor_id())
                    .hint_text(tl!("Notes for this project: to-dos, instructions, frame numbers… Saved with the project."))
                    .text_color(t.text)
                    .desired_width(f32::INFINITY)
                    .min_size(vec2(0.0, height)),
            )
        })
        .inner;
    // each focus of the editor starts a new undo step; the edits while it keeps focus join it
    let typing = egui::Id::new("project-notes-typing");
    if r.gained_focus() {
        eui.data_mut(|d| {
            let n = d.get_temp::<u64>(typing).unwrap_or(0).wrapping_add(1);
            d.insert_temp(typing, n);
        });
    }
    if r.changed() {
        let key = eui.data(|d| d.get_temp::<u64>(typing)).unwrap_or(0);
        if let Err(e) = app.session.execute("project.setNotes", json!({"text": text, "merge": key.to_string()})) {
            app.ui.status = e.to_string();
        }
    }
    // Ctrl/⌘+click on a timecode in the text follows the link
    if r.clicked() && eui.input(|i| i.modifiers.command) && !r.changed() {
        let caret = selection(&ctx).map(|(a, _)| a);
        if let Some(tc) = caret.and_then(|c| timecodes.iter().find(|tc| c >= tc.start && c <= tc.end)) {
            app.seek_program(tc.time);
        }
    }
    let label: String = app.session.project.notes.chars().take(LABEL_CHARS).collect();
    app.auto.add("notes.text", r.rect, &label);

    // ---- the links: every timecode of the notes, a click moves the playhead there
    if !timecodes.is_empty() {
        let lr = Rect::from_min_max(pos2(inner.min.x, inner.max.y - LINKS_H + 2.0), inner.max);
        let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(lr));
        egui::ScrollArea::horizontal().id_salt("project-notes-links").auto_shrink([false, true]).show(&mut lui, |lui| {
            lui.horizontal(|lui| {
                lui.label(egui::RichText::new(tl!("Timecodes:")).color(t.text_dim));
                for (n, tc) in timecodes.iter().enumerate() {
                    let tip = if tc.rest.is_empty() { tc.text.clone() } else { format!("{} {}", tc.text, tc.rest) };
                    let lr = lui.link(egui::RichText::new(&tc.text).color(t.accent)).on_hover_text(tip);
                    app.auto.add(&format!("notes.timecode.{n}"), lr.rect, &tc.text);
                    if lr.clicked() {
                        app.seek_program(tc.time);
                    }
                }
            });
        });
    }

    // ---- toolbar actions (after the editor, so its selection of this frame is known)
    match action {
        Some("insert") => {
            let at = selection(&ctx).map(|(_, b)| b);
            let p = match at {
                Some(a) => json!({"at": a}),
                None => json!({}),
            };
            match app.session.execute("project.notesInsertTimecode", p) {
                Ok(v) => {
                    let c = v.get("cursor").and_then(serde_json::Value::as_u64).unwrap_or(0) as usize;
                    select(&ctx, c, c);
                }
                Err(e) => app.ui.status = e.to_string(),
            }
        }
        Some("markers") => {
            let p = match selection(&ctx) {
                Some((a, b)) if a < b => json!({"from": a, "to": b}),
                _ => json!({}),
            };
            match app.session.execute("project.notesMarkers", p) {
                Ok(v) => {
                    let n = v.get("markers").and_then(serde_json::Value::as_u64).unwrap_or(0).to_string();
                    app.ui.status = tlf!("{n} marker(s) created from the notes", n);
                }
                Err(e) => app.ui.status = e.to_string(),
            }
        }
        Some(dir @ ("next" | "prev")) => find_step(app, &ctx, &query, dir == "next"),
        _ => {}
    }
}
