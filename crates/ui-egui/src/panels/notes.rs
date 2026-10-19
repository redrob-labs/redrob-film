//! Project Notes panel (#620): free plain text kept in the project file, for to-dos,
//! instructions for whoever opens the project next, frame numbers and the like. Every change is
//! a `project.setNotes`; one typing session (from focusing the editor until it loses focus) is
//! one undo step.
//!
//! Automation ids: `notes.text` (the editor; its label is the start of the notes). Agents read
//! and write the notes with `project.notes` / `project.setNotes`.

use egui::{Rect, vec2};
use serde_json::json;

use crate::FilmcraftApp;

/// Longest automation label (characters): the full notes are `project.notes`.
const LABEL_CHARS: usize = 200;

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 6.0))));
    let ui = &mut child;
    let mut text = app.session.project.notes.clone();
    // Kdenlive's Add Project Note: a line starting with the Program playhead's timecode; every
    // timecode in the notes is a link that moves the playhead to its frame
    let stamp = program_timecode(app);
    let links = timecodes_in(&text);
    let mut add = false;
    let mut go: Option<String> = None;
    ui.horizontal_wrapped(|ui| {
        let r = ui.add_enabled(stamp.is_some(), egui::Button::new(tl!("Add Project Note")));
        app.auto.add("notes.addNote", r.rect, "Add Project Note");
        add = r.clicked();
        for (k, tc) in links.iter().enumerate() {
            let r = ui.link(tc.as_str()).on_hover_text(tl!("Go to this frame"));
            app.auto.add(&format!("notes.link.{k}"), r.rect, tc);
            if r.clicked() {
                go = Some(tc.clone());
            }
        }
    });
    if add && let Some(tc) = &stamp {
        let sep = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
        let next = format!("{text}{sep}{tc} ");
        if let Err(e) = app.session.execute("project.setNotes", json!({"text": next})) {
            app.ui.status = e.to_string();
        }
        text = app.session.project.notes.clone();
    }
    if let Some(tc) = go
        && let Some(q) = app.session.active_sequence()
    {
        let rate = q.settings.frame_rate;
        // the notes count from the sequence's start timecode, as its monitor does
        let res = redrob_film_time::parse_timecode(&tc, rate, q.settings.drop_frame, 0)
            .map_err(|e| e.to_string())
            .map(|f| rate.tick_of((f - q.start_timecode.max(0)).max(0)));
        match res {
            Ok(t) => {
                app.stop();
                if let Err(e) = app.session.execute("playhead.set", json!({"time": t.0})) {
                    app.ui.status = e.to_string();
                }
            }
            Err(e) => app.ui.status = e,
        }
    }
    let height = ui.available_height();
    let r = egui::ScrollArea::vertical()
        .id_salt("project-notes-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .id_salt("project-notes")
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
        ui.data_mut(|d| {
            let n = d.get_temp::<u64>(typing).unwrap_or(0).wrapping_add(1);
            d.insert_temp(typing, n);
        });
    }
    if r.changed() {
        let key = ui.data(|d| d.get_temp::<u64>(typing)).unwrap_or(0);
        if let Err(e) = app.session.execute("project.setNotes", json!({"text": text, "merge": key.to_string()})) {
            app.ui.status = e.to_string();
        }
    }
    let label: String = app.session.project.notes.chars().take(LABEL_CHARS).collect();
    app.auto.add("notes.text", r.rect, &label);
}

/// The Program playhead as the sequence shows it (from its start timecode).
fn program_timecode(app: &FilmcraftApp) -> Option<String> {
    let q = app.session.active_sequence()?;
    let rate = q.settings.frame_rate;
    let ofs = rate.tick_of(q.start_timecode.max(0));
    Some(redrob_film_time::format_time(app.session.playhead() + ofs, rate, q.settings.drop_frame, redrob_film_time::TimeDisplay::Timecode, 48000))
}

/// The timecodes (`hh:mm:ss:ff`, `;` before the frames for drop frame) written in `text`, in order,
/// each once.
fn timecodes_in(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i + 11 <= b.len() {
        let w = &b[i..i + 11];
        let digits = [0, 1, 3, 4, 6, 7, 9, 10].iter().all(|&k| w[k].is_ascii_digit());
        let seps = w[2] == b':' && w[5] == b':' && (w[8] == b':' || w[8] == b';');
        let bounded = (i == 0 || !b[i - 1].is_ascii_digit()) && b.get(i + 11).is_none_or(|c| !c.is_ascii_digit());
        if digits && seps && bounded {
            let tc = String::from_utf8_lossy(w).into_owned();
            if !out.contains(&tc) {
                out.push(tc);
            }
            i += 11;
        } else {
            i += 1;
        }
    }
    out
}
