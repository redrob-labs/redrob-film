//! The shell's own commands, as the agent and the control channel see them.
//!
//! The built-in agent reaches this app through the MCP server's `command_list` / `command_run`
//! tools (crates/automation/src/server.rs L311 / L336), which the bridge turns into control
//! requests `engine.execute {command, params}` (crates/automation/src/bridge.rs L49). The shell
//! sits on that channel before the Edit editor does ([`crate::shell::ShellApp::service_control`]):
//! these `mode.*` ids are answered by the shell, `command.list` gets them appended, everything else
//! goes on to `FilmApp` untouched (crates/ui-egui/src/control.rs L99).
//!
//! Tool ids are commands, so the agent discovers them the same way as every editor command: no
//! second tool catalogue to keep in step.

use serde_json::{Value, json};

use crate::shell::Mode;

/// `mode.switch {mode: "edit"|"effects"}`: show the other editor.
pub const SWITCH_MODE: &str = "mode.switch";
/// `mode.sendSelectionToEffects {}`: the Edit selection (or in/out range) becomes a composition.
pub const SEND_SELECTION_TO_EFFECTS: &str = "mode.sendSelectionToEffects";
/// `mode.placeCompositionOnTimeline {}`: the active composition becomes a clip at the playhead.
pub const PLACE_COMPOSITION_ON_TIMELINE: &str = "mode.placeCompositionOnTimeline";
/// `mode.status {}`: which mode is active, which editors exist, what the hand-offs could do.
pub const STATUS: &str = "mode.status";
/// `effects.run {id, params?}`: run an Effects engine command (editors/effects/crates/engine/
/// src/commands/*.rs; `effects.run {id: "command.list"}` lists them) on the Effects session.
pub const EFFECTS_RUN: &str = "effects.run";

/// Every shell command id.
pub const ALL: [&str; 5] = [SWITCH_MODE, SEND_SELECTION_TO_EFFECTS, PLACE_COMPOSITION_ON_TIMELINE, STATUS, EFFECTS_RUN];

/// Whether `id` is one of the shell's commands (answered before the Edit editor sees it).
pub fn is_shell_command(id: &str) -> bool {
    ALL.contains(&id)
}

/// `edit` / `effects` (any case, with or without the `mode.` the segmented control shows).
pub fn parse_mode(s: &str) -> Option<Mode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "edit" | "editor" | "timeline" => Some(Mode::Edit),
        "effects" | "effect" | "fx" | "composition" => Some(Mode::Effects),
        _ => None,
    }
}

/// What the shell can do right now; decides the `enabled` flag of each command's catalogue entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Edit mode has an open sequence with a selection or an in/out range to send.
    pub can_send_selection: bool,
    /// Effects mode has an active composition and Edit has a sequence to place it in.
    pub can_place_composition: bool,
}

/// The shell's entries for `command.list`, shaped like the engine's (crates/engine/src/
/// commands.rs L2908: id, label, menu, shortcut, shortcuts, defaultShortcut, params, enabled).
pub fn catalogue(caps: Capabilities) -> Vec<Value> {
    let entry = |id: &str, label: &str, params: &str, enabled: bool| {
        json!({
            "id": id,
            "label": label,
            "menu": ["Mode"],
            "shortcut": Value::Null,
            "shortcuts": [],
            "defaultShortcut": Value::Null,
            "params": params,
            "enabled": enabled,
        })
    };
    vec![
        entry(SWITCH_MODE, "Switch Mode", r#"{"mode":"edit"|"effects"}"#, true),
        entry(SEND_SELECTION_TO_EFFECTS, "Send Selection to Effects", r#"{"name":str? (the new composition's name)}"#, caps.can_send_selection),
        entry(
            PLACE_COMPOSITION_ON_TIMELINE,
            "Place Composition on Timeline",
            r#"{"track":"V1"|id?,"insert":bool? (default false: overwrite at the playhead)}"#,
            caps.can_place_composition,
        ),
        entry(STATUS, "Mode Status", "{}", true),
        entry(EFFECTS_RUN, "Run Effects Command", r#"{"id":str (an Effects command id; "command.list" lists them),"params":object?}"#, true),
    ]
}

/// `command.list` with the shell's commands appended (the engine's list first, unchanged).
pub fn append_catalogue(engine_list: Value, caps: Capabilities) -> Value {
    let mut all = engine_list.as_array().cloned().unwrap_or_default();
    all.extend(catalogue(caps));
    Value::Array(all)
}

/// Whether a control request is `engine.execute` / `ui.menu.invoke` for `command`; the command id
/// comes from `command` or `id` as `control::handle` reads it (crates/ui-egui/src/control.rs L104).
pub fn command_of(method: &str, params: &Value) -> Option<String> {
    if method != "engine.execute" && method != "ui.menu.invoke" {
        return None;
    }
    params.get("command").or(params.get("id")).and_then(Value::as_str).map(str::to_string)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn mode_names_parse_loosely_and_unknown_ones_do_not() {
        assert_eq!(parse_mode("edit"), Some(Mode::Edit));
        assert_eq!(parse_mode(" Effects "), Some(Mode::Effects));
        assert_eq!(parse_mode("FX"), Some(Mode::Effects));
        assert_eq!(parse_mode("photo"), None);
        assert_eq!(parse_mode(""), None);
    }

    #[test]
    fn the_catalogue_lists_every_shell_command_with_the_engines_shape() {
        let caps = Capabilities { can_send_selection: true, can_place_composition: false };
        let list = catalogue(caps);
        assert_eq!(list.len(), ALL.len());
        for (entry, id) in list.iter().zip(ALL) {
            assert_eq!(entry["id"], id);
            for key in ["label", "menu", "shortcut", "shortcuts", "defaultShortcut", "params", "enabled"] {
                assert!(entry.get(key).is_some(), "{id} lacks {key}");
            }
        }
        assert_eq!(list[1]["enabled"], true);
        assert_eq!(list[2]["enabled"], false);
        let merged = append_catalogue(json!([{"id": "timeline.razor"}]), caps);
        let ids: Vec<&str> = merged.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
        assert_eq!(ids[0], "timeline.razor");
        assert!(ids.contains(&SWITCH_MODE));
    }

    #[test]
    fn only_execute_requests_name_a_command() {
        assert_eq!(command_of("engine.execute", &json!({"command": "mode.switch"})).as_deref(), Some("mode.switch"));
        assert_eq!(command_of("ui.menu.invoke", &json!({"id": "file.save"})).as_deref(), Some("file.save"));
        assert_eq!(command_of("ui.click", &json!({"command": "mode.switch"})), None);
        assert_eq!(command_of("engine.execute", &json!({})), None);
        assert!(is_shell_command(SWITCH_MODE));
        assert!(!is_shell_command("timeline.place"));
    }
}
