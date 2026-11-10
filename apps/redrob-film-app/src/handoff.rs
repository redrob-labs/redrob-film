//! Hand-offs between the two editors, on the app's shell: the work is in `redrob_film_handoff`
//! (crates/handoff, shared with the headless MCP server); these wrappers pick the shell's two
//! sessions and switch to the editor the hand-off ends in.

pub use redrob_film_handoff::*;
use serde_json::Value;

use crate::shell::{Mode, ShellApp};

/// [`redrob_film_handoff::timeline_to_composition`] from the shell, then shows Effects.
pub fn timeline_to_composition(shell: &mut ShellApp, params: &Value) -> Result<String, String> {
    shell.ensure_effects();
    let effects = shell.effects.as_mut().ok_or("the Effects editor did not start")?;
    let line = redrob_film_handoff::timeline_to_composition(&shell.edit.session, &mut effects.session, params)?;
    shell.set_mode(Mode::Effects);
    Ok(line)
}

/// [`redrob_film_handoff::can_place_composition`] for the shell's sessions.
pub fn can_place_composition(shell: &ShellApp) -> bool {
    redrob_film_handoff::can_place_composition(&shell.edit.session, shell.effects.as_ref().map(|e| &e.session))
}

/// [`redrob_film_handoff::composition_to_timeline`] from the shell, then shows Edit.
pub fn composition_to_timeline(shell: &mut ShellApp, params: &Value) -> Result<String, String> {
    let effects = shell.effects.as_ref().ok_or("no composition: open one in Effects first")?;
    let line = redrob_film_handoff::composition_to_timeline(&mut shell.edit.session, &effects.session, params)?;
    shell.set_mode(Mode::Edit);
    Ok(line)
}
