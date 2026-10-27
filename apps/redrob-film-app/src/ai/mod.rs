//! The shared AI dock: one chat with the built-in agent (Redrob Code over ACP), shown beside
//! whichever editor is active.
//!
//! The Edit editor already owns the agent: `FilmApp::agent` (`AgentPanel`, crates/ui-egui/
//! src/agent.rs L85) holds the transcript and the running `AgentHost`, `FilmApp::
//! agent_services.factory` (main.rs) starts Redrob Code with this app's MCP server, and the panel
//! body is the free function `redrob_film_ui_egui::agent::show(app, ui, rect)` (agent.rs L276),
//! which only touches `app.agent`, `app.account`, `app.agent_services`, `app.auto`, `app.tokens`
//! and `app.ui.status`. So the dock does not lift the agent out of `FilmApp`: it draws that
//! same function into a right panel, in both modes, with the Edit editor's state. The Edit dock's
//! own Agent panel keeps working too (same `AgentPanel`, so the two views are one conversation).
//!
//! What the agent can do with the active mode goes through the control channel the shell
//! interposes ([`tools`]): `mode.switch`, `mode.sendSelectionToEffects`,
//! `mode.placeCompositionOnTimeline`, `mode.status`, next to every editor command.
//!
//! One seam: the account gate (`redrob_film_ui_egui::account::require`, called from the panel's Send)
//! opens the Connect dialog as `FilmApp::dialog`, which only the Edit editor draws. The shell
//! switches to Edit when that dialog appears while Effects is on screen
//! ([`crate::shell::ShellApp::ai_dock`]).

pub mod tools;

use redrob_film_ui_egui::FilmApp;

/// The dock's width in points when first opened.
pub const DEFAULT_WIDTH: f32 = 320.0;
/// Narrowest the dock can be dragged.
pub const MIN_WIDTH: f32 = 240.0;
/// Widest the dock can be dragged.
pub const MAX_WIDTH: f32 = 640.0;

/// Ctrl+Shift+A toggles the dock.
pub const TOGGLE_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT), egui::Key::A);

/// The dock's own state (the conversation lives in `FilmApp::agent`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AiDock {
    pub open: bool,
}

impl AiDock {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }
}

/// The top bar's AI button (right slot): selected while the dock is open.
pub fn toggle(ui: &mut egui::Ui, dock: &mut AiDock) {
    let hint = format!("Redrob Code  ({})", ui.ctx().format_shortcut(&TOGGLE_SHORTCUT));
    if ui.selectable_label(dock.open, "AI").on_hover_text(hint).clicked() {
        dock.toggle();
    }
}

/// The dock's body: the Edit editor's agent panel, drawn into `ui`'s whole rect. Returns whether
/// the panel opened a dialog on the Edit editor (the Connect dialog), which only Edit mode shows.
pub fn body(ui: &mut egui::Ui, edit: &mut FilmApp) -> bool {
    let had_dialog = edit.dialog.is_some();
    let rect = ui.available_rect_before_wrap();
    // The panel measures its frame by what the contents take, and stores that as the width to
    // show next frame. `agent::show` paints into a child `Ui` without allocating anything here,
    // so without this the frame hugs the panel's minimum width at the left of the dock, the
    // header is clipped to it, and every drag of the handle snaps back to the minimum.
    ui.set_min_size(rect.size());
    redrob_film_ui_egui::agent::show(edit, ui, rect);
    !had_dialog && edit.dialog.is_some()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_dock_toggles_and_starts_closed() {
        let mut dock = AiDock::default();
        assert!(!dock.open);
        dock.toggle();
        assert!(dock.open);
        dock.toggle();
        assert!(!dock.open);
        const { assert!(TOGGLE_SHORTCUT.modifiers.shift && TOGGLE_SHORTCUT.modifiers.command) };
        const { assert!(MIN_WIDTH <= DEFAULT_WIDTH && DEFAULT_WIDTH <= MAX_WIDTH) };
    }
}
