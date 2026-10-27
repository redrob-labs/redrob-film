//! The Redrob Film shell: one window, a thin top bar, and two editors hosted as switchable modes:
//! Edit (`FilmApp`, the timeline editor, created at start-up with every service main.rs wires)
//! and Effects (`EffectsApp`, the compositor, created on the first switch and kept). The shell
//! also owns what crosses the modes: the Mode menu's hand-offs (`handoff`), the shared AI dock
//! (`ai`), the project bundle (`bundle`), the control channel the agent drives both through
//! ([`ShellApp::service_control`]), the window title, and the close check for the editor that is
//! not on screen.
//!
//! Fonts and styles: each editor installs its own fonts and style on the shared `egui::Context`
//! once (`theme::install` in `FilmApp::logic`, crates/ui-egui/src/lib.rs L1926, and in
//! `EffectsApp::logic`, editors/effects/crates/ui-egui/src/lib.rs L1872), replacing the
//! other's. A switch while one editor's definitions are live would make the other draw with
//! `FontFamily::Name(..)` families egui has not got, which panics. So the shell records each
//! editor's definitions as it installs them, installs the union ([`merge_fonts`]) and applies a
//! switch only once the union is live ([`ModeState::apply_pending`]); each editor's style is
//! snapshotted while it is active and restored when it comes back ([`StyleSnapshot`]). This is the
//! Redrob Canvas shell's pattern.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use redrob_effects_ui_egui::EffectsApp;
use redrob_film_ui_egui::FilmApp;
use redrob_film_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

use crate::ai::tools::{self, Capabilities};
use crate::ai::{self, AiDock};
use crate::bundle::{self, BundleEvent, BundleTracker};
use crate::handoff;

/// The product name, as the window title and the top bar show it.
pub const APP_NAME: &str = "Redrob Film";

/// Height of the shell's own top bar, in points.
pub const TOP_BAR_HEIGHT: f32 = 28.0;

/// How long a status line stays before it has faded out, in seconds.
pub const STATUS_SECS: f32 = 6.0;

/// Which editor the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    #[default]
    Edit,
    Effects,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Edit, Mode::Effects];

    /// The segmented button's label.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Edit => "Edit",
            Mode::Effects => "Effects",
        }
    }

    /// The name commands and the agent use.
    pub fn id(self) -> &'static str {
        match self {
            Mode::Edit => "edit",
            Mode::Effects => "effects",
        }
    }

    /// Ctrl+1 / Ctrl+2.
    pub fn shortcut(self) -> egui::KeyboardShortcut {
        let key = match self {
            Mode::Edit => egui::Key::Num1,
            Mode::Effects => egui::Key::Num2,
        };
        egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, key)
    }
}

/// The window title: `<project name>[*] - Redrob Film`, or the product name alone with no
/// document. `dirty` adds the `*`.
pub fn window_title(doc: Option<(&str, bool)>) -> String {
    match doc {
        Some((name, dirty)) => format!("{name}{} - {APP_NAME}", if dirty { "*" } else { "" }),
        None => APP_NAME.to_string(),
    }
}

/// The file name of `path`, or the path itself when it has none.
fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| path.to_string())
}

/// The shell's status line in the top bar: one line that fades out after [`STATUS_SECS`].
#[derive(Clone, Debug)]
pub struct Status {
    pub text: String,
    pub error: bool,
    since: Instant,
}

impl Status {
    pub fn new(text: impl Into<String>, error: bool) -> Self {
        Self { text: text.into(), error, since: Instant::now() }
    }

    /// The line's opacity at `now`: full, then fading through its last second; `None` once gone.
    pub fn alpha(&self, now: Instant) -> Option<f32> {
        let left = STATUS_SECS - now.saturating_duration_since(self.since).as_secs_f32();
        (left > 0.0).then_some(left.min(1.0))
    }
}

// ----------------------------------------------------------------------------------------------
// Fonts and styles across modes
// ----------------------------------------------------------------------------------------------

/// The fonts an editor installed, by name: which font files and which families they form. Cheap
/// to compare every frame, unlike `FontDefinitions` itself (that compares the font bytes).
type FontSignature = (BTreeSet<String>, BTreeMap<String, Vec<String>>);

fn font_signature(defs: &egui::FontDefinitions) -> FontSignature {
    let names = defs.font_data.keys().cloned().collect();
    let families = defs.families.iter().map(|(family, stack)| (format!("{family:?}"), stack.clone())).collect();
    (names, families)
}

/// The union of every editor's font definitions. Every editor's named families are present, so
/// whichever editor draws finds its fonts; a family both editors define (`Proportional`,
/// `Monospace`, a shared name) gets the editors' fallback stacks appended in [`Mode::ALL`] order,
/// each font once. The result does not depend on which editor is active, so a mode switch never
/// re-installs fonts: `Context::set_fonts` rebuilds every font face (copying each font's bytes,
/// parsing it, building its shaper and hinting state) and empties the glyph atlas, which took
/// seconds per switch with the editors' CJK and system fallback fonts.
pub fn merge_fonts(all: &HashMap<Mode, egui::FontDefinitions>) -> egui::FontDefinitions {
    let mut merged = egui::FontDefinitions::default();
    merged.font_data.clear();
    merged.families.clear();
    for mode in Mode::ALL {
        if let Some(defs) = all.get(&mode) {
            for (name, data) in &defs.font_data {
                merged.font_data.entry(name.clone()).or_insert_with(|| data.clone());
            }
            for (family, stack) in &defs.families {
                let merged_stack = merged.families.entry(family.clone()).or_default();
                for name in stack {
                    if !merged_stack.contains(name) {
                        merged_stack.push(name.clone());
                    }
                }
            }
        }
    }
    merged
}

/// An editor's egui style, saved while it is active and restored when it is active again.
#[derive(Clone)]
struct StyleSnapshot {
    global: Arc<egui::Style>,
    dark: Arc<egui::Style>,
    light: Arc<egui::Style>,
    theme: egui::ThemePreference,
}

impl StyleSnapshot {
    fn take(ctx: &egui::Context) -> Self {
        Self {
            global: ctx.global_style(),
            dark: ctx.style_of(egui::Theme::Dark),
            light: ctx.style_of(egui::Theme::Light),
            theme: ctx.options(|o| o.theme_preference),
        }
    }

    fn restore(&self, ctx: &egui::Context) {
        ctx.set_global_style(self.global.clone());
        ctx.set_style_of(egui::Theme::Dark, self.dark.clone());
        ctx.set_style_of(egui::Theme::Light, self.light.clone());
        ctx.set_theme(self.theme);
    }
}

/// The mode switch state machine, apart from the editors (so it can be tested without them).
#[derive(Default)]
pub struct ModeState {
    mode: Mode,
    /// A switch asked for (button, shortcut, hand-off, agent) and applied at the next frame's
    /// start, once the shared fonts hold every editor's families.
    pending: Option<Mode>,
    /// Each editor's own font definitions, read back after it installed them.
    fonts: HashMap<Mode, egui::FontDefinitions>,
    /// The signature of the merged definitions the shell installed last; `None` until an editor
    /// installed fonts of its own.
    merged_fonts: Option<FontSignature>,
    /// Each editor's egui style, saved every frame it is active.
    styles: HashMap<Mode, StyleSnapshot>,
}

impl ModeState {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn pending(&self) -> Option<Mode> {
        self.pending
    }

    /// Ask for `mode`; applied at the start of the next frame ([`Self::apply_pending`]).
    pub fn set_mode(&mut self, mode: Mode) {
        self.pending = Some(mode);
    }

    /// Apply a requested switch. Not while an editor's own font definitions are the live ones
    /// (the other editor would draw with families they lack and egui would panic): the shell
    /// installs the merged set first ([`Self::sync_fonts`]) and the switch waits one frame.
    pub fn apply_pending(&mut self, ctx: &egui::Context) -> bool {
        let Some(next) = self.pending else { return false };
        if next == self.mode {
            self.pending = None;
            return false;
        }
        if !self.fonts_settled(ctx) {
            return false;
        }
        self.pending = None;
        self.styles.insert(self.mode, StyleSnapshot::take(ctx));
        self.mode = next;
        if let Some(style) = self.styles.get(&next) {
            style.restore(ctx);
        }
        // The merged set is the same for every mode (`merge_fonts`), so once it is live the
        // switch needs no `set_fonts`: a font rebuild here froze the window for seconds.
        true
    }

    /// Whether the live font definitions are egui's defaults or the shell's merged set.
    pub fn fonts_settled(&self, ctx: &egui::Context) -> bool {
        match &self.merged_fonts {
            None => self.fonts.is_empty(),
            Some(merged) => ctx.fonts(|f| font_signature(f.definitions()) == *merged),
        }
    }

    /// Each editor installs its own fonts with `Context::set_fonts`, replacing everyone else's.
    /// When the live definitions are not the shell's merged set, the active editor just did
    /// that: remember its set and install the union of all of them.
    pub fn sync_fonts(&mut self, ctx: &egui::Context) {
        let live = ctx.fonts(|f| font_signature(f.definitions()));
        if self.merged_fonts.as_ref() == Some(&live) {
            return;
        }
        if self.merged_fonts.is_none() && self.fonts.is_empty() && live == font_signature(&egui::FontDefinitions::default()) {
            // Still egui's defaults: no editor has installed anything yet.
            return;
        }
        let defs = ctx.fonts(|f| f.definitions().clone());
        self.fonts.insert(self.mode, defs);
        let merged = merge_fonts(&self.fonts);
        self.merged_fonts = Some(font_signature(&merged));
        ctx.set_fonts(merged);
    }

    /// Remember the active editor's style as this frame leaves it.
    pub fn snapshot_style(&mut self, ctx: &egui::Context) {
        self.styles.insert(self.mode, StyleSnapshot::take(ctx));
    }

    /// Modes whose fonts the shell has recorded (for tests).
    #[cfg(test)]
    pub fn recorded_fonts(&self) -> Vec<Mode> {
        let mut v: Vec<Mode> = self.fonts.keys().copied().collect();
        v.sort_by_key(|m| m.id());
        v
    }
}

// ----------------------------------------------------------------------------------------------
// The shell
// ----------------------------------------------------------------------------------------------

/// The eframe app: the shell plus its editors.
pub struct ShellApp {
    modes: ModeState,
    /// The timeline editor, with every service main.rs gives it.
    pub(crate) edit: FilmApp,
    /// The compositor, created on the first switch to Effects.
    pub(crate) effects: Option<EffectsApp>,
    /// The shared AI dock.
    pub ai: AiDock,
    /// What the hand-offs could do when this frame's logic ran.
    caps: Capabilities,
    /// The shell's status line (hand-off results), fading.
    status: Option<Status>,
    /// The control channel from the TCP server (main.rs); the shell answers its own requests and
    /// forwards the rest to the Edit editor over `edit_tx`.
    control_rx: Option<Receiver<ControlRequest>>,
    edit_tx: Sender<ControlRequest>,
    /// Watches the Edit session for opens and saves (the project bundle).
    bundle: BundleTracker,
    /// The window title the shell set last.
    last_title: Option<String>,
    /// The non-empty title the window reported last frame (see `sync_window_title`).
    last_reported_title: Option<String>,
    /// A close was approved while Effects had unsaved changes: the next request passes.
    close_approved: bool,
    /// The Redrob Code app bridge (`app_bridge.rs`), registered while the window lives.
    app_bridge: Option<crate::app_bridge::BridgeHandle>,
}

impl ShellApp {
    /// `edit` is the fully wired Edit editor; `control_rx` the control server's requests and
    /// `edit_tx` the sender whose receiver `edit` was given with `FilmApp::with_control`.
    pub fn new(edit: FilmApp, control_rx: Option<Receiver<ControlRequest>>, edit_tx: Sender<ControlRequest>) -> Self {
        // The Effects editor's first frame installs its fonts, and its CJK / script fallbacks come
        // from a scan of the system font folders (`redrob_effects_text::fonts::scan_system`, a
        // separate database from the Edit editor's). On the UI thread that scan was a large part
        // of the seconds the first switch to Effects took: start it now, in the background.
        redrob_effects_text::fonts::scan_system_in_background();
        Self {
            modes: ModeState::default(),
            edit,
            effects: None,
            ai: AiDock::default(),
            caps: Capabilities::default(),
            status: None,
            control_rx,
            edit_tx,
            bundle: BundleTracker::default(),
            last_title: None,
            last_reported_title: None,
            close_approved: false,
            app_bridge: None,
        }
    }

    /// Keep the Redrob Code app bridge for the window's lifetime; it unregisters on exit.
    pub fn with_app_bridge(mut self, bridge: crate::app_bridge::BridgeHandle) -> Self {
        self.app_bridge = Some(bridge);
        self
    }

    pub fn mode(&self) -> Mode {
        self.modes.mode()
    }

    /// Ask for `mode`: applied at the start of the next frame. Effects is created on its first
    /// visit and kept afterwards.
    pub fn set_mode(&mut self, mode: Mode) {
        self.modes.set_mode(mode);
    }

    pub fn pending_mode(&self) -> Option<Mode> {
        self.modes.pending()
    }

    /// Whether the Effects editor exists yet.
    pub fn effects_created(&self) -> bool {
        self.effects.is_some()
    }

    /// Show `text` on the status line (an error in the error colour).
    pub fn set_status(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        if error {
            log::warn!("{text}");
        } else {
            log::info!("{text}");
        }
        self.status = Some(Status::new(text, error));
    }

    /// The Effects editor, created on demand: `redrob_effects_host::session()` (editors/effects/
    /// crates/host/src/lib.rs L84: media, import, expressions, scripting, export) and
    /// `EffectsApp::new` (editors/effects/crates/ui-egui/src/lib.rs L367), with the file
    /// dialogs the original standalone Effects app gave it. Its control server and native menu are not used: the shell's channel and the
    /// one window's menus are.
    pub(crate) fn ensure_effects(&mut self) -> &mut EffectsApp {
        self.effects.get_or_insert_with(Self::build_effects)
    }

    fn build_effects() -> EffectsApp {
        let mut app = EffectsApp::new(redrob_effects_host::session());
        app.hooks.pick_files = Some(Box::new(|exts: &[&str]| {
            rfd::FileDialog::new().add_filter("Media", exts).pick_files().unwrap_or_default().into_iter().map(|p| p.to_string_lossy().to_string()).collect()
        }));
        app.hooks.pick_save = Some(Box::new(|name: &str| {
            rfd::FileDialog::new().add_filter("Effects Project", &["json", "rfx"]).set_file_name(name).save_file().map(|p| p.to_string_lossy().to_string())
        }));
        app.hooks.pick_open_project = Some(Box::new(|| {
            rfd::FileDialog::new()
                .add_filter("Effects Project", &["json", "rfx", "rfxx", "ecproj", "ecprojx"]) // legacy: pre-rename effects projects still open
                .pick_file()
                .map(|p| p.to_string_lossy().to_string())
        }));
        app.hooks.pick_folder = Some(Box::new(|| rfd::FileDialog::new().pick_folder().map(|p| p.to_string_lossy().to_string())));
        app.hooks.pick_save_file = Some(Box::new(|name: &str, ext: &str| {
            let path = Path::new(name);
            let mut d = if ext.is_empty() { rfd::FileDialog::new() } else { rfd::FileDialog::new().add_filter(ext, &[ext]) };
            if let Some(dir) = path.parent().filter(|d| d.is_dir()) {
                d = d.set_directory(dir);
            }
            let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            d.set_file_name(file).save_file().map(|p| p.to_string_lossy().to_string())
        }));
        // The same `Box<dyn Fn(&egui::Context) -> Option<egui::Theme>>` both UI crates declare
        // (crates/ui-egui/src/lib.rs L167, editors/effects/crates/ui-egui/src/lib.rs L106).
        app.hooks.system_theme = crate::appearance::service();
        app
    }

    // ------------------------------------------------------------------------------------------
    // Top bar
    // ------------------------------------------------------------------------------------------

    /// The shell's own top bar: brand, the mode buttons, the Mode menu, the status line, and the
    /// right slot (AI dock toggle, update status).
    pub fn top_bar(&mut self, ui: &mut egui::Ui) {
        let visuals = ui.visuals().clone();
        let frame = egui::Frame::NONE.fill(visuals.panel_fill).stroke(egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color));
        egui::Panel::top("redrob_film_top_bar").exact_size(TOP_BAR_HEIGHT).frame(frame).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(APP_NAME).strong());
                ui.add_space(12.0);
                ui.separator();
                let mut next = self.mode();
                for mode in Mode::ALL {
                    let hint = format!("{}  ({})", mode.label(), ui.ctx().format_shortcut(&mode.shortcut()));
                    let button = ui.selectable_label(self.mode() == mode, mode.label()).on_hover_text(hint);
                    if button.clicked() {
                        next = mode;
                    }
                }
                if next != self.mode() {
                    self.set_mode(next);
                }
                ui.separator();
                ui.menu_button("Mode", |ui| self.mode_menu(ui));
                self.status_line(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    self.right_slot(ui);
                });
            });
        });
    }

    /// The "Mode" menu: the hand-offs, each enabled when it has something to work on.
    fn mode_menu(&mut self, ui: &mut egui::Ui) {
        let caps = self.caps;
        let send = ui
            .add_enabled(caps.can_send_selection, egui::Button::new("Send Selection to Effects"))
            .on_hover_text("The selected clips (or the in/out range, or the whole sequence) become a new composition in Effects")
            .on_disabled_hover_text("open a sequence with clips to send first");
        if send.clicked() {
            ui.close();
            // The status line already carries the outcome.
            let _ = self.send_selection_to_effects(&json!({}));
        }
        let place = ui
            .add_enabled(caps.can_place_composition, egui::Button::new("Place Composition on Timeline"))
            .on_hover_text("The active composition becomes a clip at the playhead, rendered live from Effects")
            .on_disabled_hover_text("open a composition in Effects and a sequence in Edit first");
        if place.clicked() {
            ui.close();
            let _ = self.place_composition_on_timeline(&json!({}));
        }
        ui.separator();
        for mode in Mode::ALL {
            let label = format!("{} Mode", mode.label());
            if ui.add_enabled(self.mode() != mode, egui::Button::new(label).shortcut_text(ui.ctx().format_shortcut(&mode.shortcut()))).clicked() {
                ui.close();
                self.set_mode(mode);
            }
        }
        ui.separator();
        if ui.add(egui::Button::new(if self.ai.open { "Hide AI" } else { "Show AI" }).shortcut_text(ui.ctx().format_shortcut(&ai::TOGGLE_SHORTCUT))).clicked() {
            ui.close();
            self.ai.toggle();
        }
    }

    /// Runs the timeline → composition hand-off and reports on the status line.
    pub fn send_selection_to_effects(&mut self, params: &Value) -> Result<Value, String> {
        match handoff::timeline_to_composition(self, params) {
            Ok(line) => {
                self.set_status(line.clone(), false);
                Ok(json!({"ok": true, "message": line, "mode": Mode::Effects.id()}))
            }
            Err(e) => {
                self.set_status(format!("Send to Effects: {e}"), true);
                Err(e)
            }
        }
    }

    /// Runs the composition → timeline hand-off and reports on the status line.
    pub fn place_composition_on_timeline(&mut self, params: &Value) -> Result<Value, String> {
        match handoff::composition_to_timeline(self, params) {
            Ok(line) => {
                self.set_status(line.clone(), false);
                Ok(json!({"ok": true, "message": line, "mode": Mode::Edit.id()}))
            }
            Err(e) => {
                self.set_status(format!("Place on timeline: {e}"), true);
                Err(e)
            }
        }
    }

    /// The top bar's status line: the last hand-off result, fading out.
    fn status_line(&mut self, ui: &mut egui::Ui) {
        let Some(status) = self.status.clone() else { return };
        let Some(alpha) = status.alpha(Instant::now()) else {
            self.status = None;
            return;
        };
        ui.add_space(8.0);
        let color = if status.error { ui.visuals().error_fg_color } else { ui.visuals().text_color() };
        ui.label(egui::RichText::new(&status.text).color(color.gamma_multiply(alpha)));
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }

    /// The right side of the top bar: the AI dock toggle and the update status.
    pub fn right_slot(&mut self, ui: &mut egui::Ui) {
        ai::toggle(ui, &mut self.ai);
        update_status(ui);
    }

    /// The AI dock, drawn before the editor area so the editor gets what is left.
    fn ai_dock(&mut self, ui: &mut egui::Ui) {
        if !self.ai.open {
            return;
        }
        let visuals = ui.visuals().clone();
        let frame = egui::Frame::NONE.fill(visuals.panel_fill).inner_margin(8.0).stroke(egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color));
        let edit = &mut self.edit;
        let opened_dialog = egui::Panel::right("redrob_film_ai_dock")
            .default_size(ai::DEFAULT_WIDTH)
            .size_range(ai::MIN_WIDTH..=ai::MAX_WIDTH)
            .resizable(true)
            .frame(frame)
            .show(ui, |ui| ai::body(ui, edit))
            .inner;
        // The Connect dialog (the account gate) is drawn by the Edit editor alone: show it.
        if opened_dialog && self.mode() == Mode::Effects {
            self.set_status("Connect a Redrob workspace in Edit mode to use the agent", false);
            self.set_mode(Mode::Edit);
        }
    }

    // ------------------------------------------------------------------------------------------
    // Documents, title, close
    // ------------------------------------------------------------------------------------------

    /// The bundle's name and whether either editor has unsaved changes: the Edit project's file
    /// name (its `Session::path`), else its project name; dirty when the Edit session is
    /// (`Session::is_dirty`, crates/engine/src/lib.rs L650) or the effects session is
    /// (editors/effects/crates/engine/src/lib.rs L823).
    pub fn active_document(&self) -> Option<(String, bool)> {
        let s = &self.edit.session;
        let name = s.path.as_deref().map(file_name).or_else(|| Some(s.project.name.clone()).filter(|n| !n.is_empty()));
        let dirty = s.is_dirty() || self.effects.as_ref().is_some_and(|e| e.session.is_dirty());
        name.map(|n| (n, dirty))
    }

    /// The window title is the shell's ([`window_title`]). The Effects editor sends its own on a
    /// document change (editors/effects/crates/ui-egui/src/lib.rs L1663); the window reports
    /// it back the next frame and the shell sets its own again.
    ///
    /// Only a title the window actually reports counts: winit returns an empty title on X11 and
    /// Wayland, and treating that as "overridden" sent the title every frame. Every viewport
    /// command asks egui for another frame, so the window never went idle (125 fps and 6 cores
    /// on a software rasteriser with nothing happening, which made the timeline needle lag).
    /// A title is also re-sent only when the reported one changed, so two senders cannot loop.
    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let doc = self.active_document();
        let want = window_title(doc.as_ref().map(|(name, dirty)| (name.as_str(), *dirty)));
        let reported = ctx.input(|i| i.viewport().title.clone()).filter(|t| !t.is_empty());
        let fresh = reported != self.last_reported_title;
        self.last_reported_title = reported.clone();
        let overridden = fresh && reported.is_some_and(|t| t != want);
        if overridden || self.last_title.as_ref() != Some(&want) {
            self.last_title = Some(want.clone());
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(want));
        }
    }

    /// A close request while Effects is off screen with unsaved changes: cancel it, switch to
    /// Effects (its own Unsaved Changes prompt answers the next request) and say why. The Edit
    /// editor asks about its own changes itself (`intercept_quit` in its `logic`).
    fn check_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if std::mem::take(&mut self.close_approved) {
            return;
        }
        let effects_dirty = self.effects.as_ref().is_some_and(|e| e.session.is_dirty());
        if self.mode() == Mode::Edit && effects_dirty {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_approved = true;
            self.set_mode(Mode::Effects);
            self.set_status("Effects has unsaved changes: save the project (Ctrl+S) or close again to be asked", true);
        }
    }

    // ------------------------------------------------------------------------------------------
    // Control channel and the agent's tools
    // ------------------------------------------------------------------------------------------

    /// Requests from the control server, before the Edit editor sees them: the shell's own
    /// commands are answered here, `command.list` / `engine.commands` get the shell's entries
    /// appended, a `ui.*` request while Effects is on screen is refused (the Edit editor's widgets
    /// are not there to click), and everything else goes on to the Edit editor, which handles it
    /// in its `logic` this same frame (`drain_control`, crates/ui-egui/src/lib.rs L1464).
    fn service_control(&mut self) {
        let Some(rx) = self.control_rx.take() else { return };
        while let Ok(req) = rx.try_recv() {
            let command = tools::command_of(&req.method, &req.params);
            match command.as_deref() {
                Some(id) if tools::is_shell_command(id) => {
                    let params = req.params.get("params").cloned().unwrap_or(json!({}));
                    let answer = self.run_shell_command(id, &params);
                    let _ = req.reply.send(control_reply(answer));
                }
                Some("command.list") => {
                    let answer = self.command_list();
                    let _ = req.reply.send(control_reply(answer));
                }
                _ if req.method == "engine.commands" => {
                    let answer = self.command_list();
                    let _ = req.reply.send(control_reply(answer));
                }
                _ if req.method.starts_with("ui.") && self.mode() == Mode::Effects => {
                    let why = format!(
                        "Effects mode is on screen; `{}` drives the Edit editor: run `{}` with {{\"mode\":\"edit\"}} first",
                        req.method,
                        tools::SWITCH_MODE
                    );
                    let _ = req.reply.send(control_reply(Err(why)));
                }
                _ => {
                    if let Err(e) = self.edit_tx.send(req) {
                        let _ = e.0.reply.send(control_reply(Err("the Edit editor's control channel is closed".to_string())));
                    }
                }
            }
        }
        self.control_rx = Some(rx);
    }

    /// `command.list`: the engine's catalogue (crates/engine/src/commands.rs L2902) with the
    /// shell's commands appended.
    fn command_list(&mut self) -> Result<Value, String> {
        let caps = self.caps;
        self.edit.session.execute("command.list", json!({})).map(|v| tools::append_catalogue(v, caps)).map_err(|e| e.to_string())
    }

    /// One of the shell's commands ([`tools`]), from the control channel.
    pub fn run_shell_command(&mut self, id: &str, params: &Value) -> Result<Value, String> {
        match id {
            tools::SWITCH_MODE => {
                let mode = tools::parse_mode(params.get("mode").and_then(Value::as_str).unwrap_or("")).ok_or("mode must be \"edit\" or \"effects\"")?;
                self.set_mode(mode);
                Ok(json!({"mode": mode.id(), "applied": "next frame"}))
            }
            tools::SEND_SELECTION_TO_EFFECTS => self.send_selection_to_effects(params),
            tools::PLACE_COMPOSITION_ON_TIMELINE => self.place_composition_on_timeline(params),
            tools::EFFECTS_RUN => {
                let cmd = params.get("id").or(params.get("command")).and_then(Value::as_str).ok_or("need `id` (an Effects command id)")?.to_string();
                let p = params.get("params").cloned().unwrap_or(json!({}));
                self.ensure_effects().session.execute(&cmd, p).map_err(|e| e.to_string())
            }
            tools::STATUS => Ok(json!({
                "mode": self.mode().id(),
                "pending": self.pending_mode().map(Mode::id),
                "effectsCreated": self.effects_created(),
                "canSendSelection": self.caps.can_send_selection,
                "canPlaceComposition": self.caps.can_place_composition,
                "aiDockOpen": self.ai.open,
            })),
            other => Err(format!("unknown shell command `{other}`")),
        }
    }

    // ------------------------------------------------------------------------------------------
    // Per-frame services
    // ------------------------------------------------------------------------------------------

    /// Ctrl+1 / Ctrl+2 switch modes, Ctrl+Shift+A toggles the AI dock: consumed before the active
    /// editor sees the frame's input.
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        for mode in Mode::ALL {
            if ctx.input_mut(|i| i.consume_shortcut(&mode.shortcut())) {
                self.set_mode(mode);
            }
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ai::TOGGLE_SHORTCUT)) {
            self.ai.toggle();
        }
    }

    /// The project bundle: an Edit open loads the effects file beside the project, an Edit save
    /// writes it ([`bundle`]).
    fn service_bundle(&mut self) {
        let event = {
            let s = &self.edit.session;
            let pool = Arc::as_ptr(&s.media) as usize;
            self.bundle.observe(pool, s.path.as_deref(), s.saved_revision)
        };
        match event {
            None => {}
            Some(BundleEvent::Installed { path }) => {
                let has_file = path.as_deref().is_some_and(|p| Path::new(&bundle::effects_path(p)).is_file());
                if !has_file && self.effects.is_none() {
                    return;
                }
                self.ensure_effects();
                let Some(effects) = self.effects.as_mut() else { return };
                match path {
                    Some(p) => match bundle::load_effects(&p, &mut effects.session) {
                        Ok(true) => {
                            let bound = handoff::rebind_composition_sources(&self.edit.session, &effects.session);
                            let linked = if bound > 0 { format!(" ({bound} composition clip(s) linked)") } else { String::new() };
                            self.set_status(format!("Opened the effects of {}{linked}", file_name(&p)), false);
                        }
                        Ok(false) => {}
                        Err(e) => self.set_status(format!("Effects not opened: {e}"), true),
                    },
                    None => {
                        if !effects.session.is_dirty() {
                            effects.session.replace_project(redrob_effects_project::Project::default(), None);
                        }
                    }
                }
            }
            Some(BundleEvent::Saved { path }) => {
                let Some(effects) = self.effects.as_mut() else { return };
                match bundle::save_effects(&path, &mut effects.session) {
                    Ok(Some(written)) => log::info!("effects saved to {written}"),
                    Ok(None) => {}
                    Err(e) => self.set_status(format!("Effects not saved: {e}"), true),
                }
            }
        }
    }

    /// What the hand-offs could do, for the menu and the agent's catalogue.
    fn refresh_caps(&mut self) {
        self.caps =
            Capabilities { can_send_selection: handoff::can_send_selection(&self.edit.session), can_place_composition: handoff::can_place_composition(self) };
    }

    /// The editor area below the top bar: the active editor draws its own panels inside it.
    fn editor_area(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let rect = ui.available_rect_before_wrap();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
        match self.mode() {
            Mode::Edit => eframe::App::ui(&mut self.edit, &mut child, frame),
            Mode::Effects => eframe::App::ui(self.ensure_effects(), &mut child, frame),
        }
    }
}

/// `{"ok": true, "result": ..}` / `{"ok": false, "error": ..}`, as `control::handle` answers
/// (crates/ui-egui/src/control.rs L60-L65).
fn control_reply(r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => json!({"ok": true, "result": v}),
        Err(e) => json!({"ok": false, "error": e}),
    }
}

/// The update status in the top bar's right slot: the Updates menu and, when a newer release
/// exists, the "Update to vX.Y.Z" / "Install and restart" button (`update.rs`). Under Redrob
/// Launcher, which owns updates, only the menu is drawn and it says so.
fn update_status(ui: &mut egui::Ui) {
    crate::update::top_bar(ui);
}

impl eframe::App for ShellApp {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.handle_shortcuts(ctx);
        self.check_close(ctx);
        self.modes.sync_fonts(ctx);
        self.modes.apply_pending(ctx);
        if self.modes.pending().is_some() {
            // The switch waits for the merged fonts: keep the frames coming.
            ctx.request_repaint();
        }
        self.service_control();
        // Once, in every mode: a window bigger than its monitor's work area is maximized into it,
        // so its title bar can be reached.
        redrob_effects_ui_egui::fit_window(ctx);
        // The Edit editor's logic runs every frame, whichever mode is on screen: it drains the
        // control channel (the agent's commands), polls the agent and keeps auto-save going
        // (crates/ui-egui/src/lib.rs L1924). Its `ui` runs only while it is on screen.
        eframe::App::logic(&mut self.edit, ctx, frame);
        if self.mode() == Mode::Effects {
            eframe::App::logic(self.ensure_effects(), ctx, frame);
        }
        self.service_bundle();
        self.refresh_caps();
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Only the active editor sees the raw input (synthetic events, pointer wrap).
        match self.mode() {
            Mode::Edit => eframe::App::raw_input_hook(&mut self.edit, ctx, raw),
            Mode::Effects => {
                if let Some(app) = self.effects.as_mut() {
                    eframe::App::raw_input_hook(app, ctx, raw);
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.sync_window_title(ui.ctx());
        self.top_bar(ui);
        self.ai_dock(ui);
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.editor_area(ui, frame));
        self.modes.snapshot_style(ui.ctx());
    }

    fn on_exit(&mut self) {
        log::info!("closing in {} mode (pending {:?}); effects created: {}", self.mode().label(), self.pending_mode(), self.effects_created());
        // Unregister from Redrob Code first, so the agent stops seeing tools the window is closing.
        if let Some(bridge) = self.app_bridge.take() {
            bridge.stop();
        }
        eframe::App::on_exit(&mut self.edit);
        if let Some(app) = self.effects.as_mut() {
            eframe::App::on_exit(app);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn modes_switch_and_each_has_a_distinct_shortcut() {
        let ctx = egui::Context::default();
        let mut modes = ModeState::default();
        assert_eq!(modes.mode(), Mode::Edit);
        for mode in [Mode::Effects, Mode::Edit, Mode::Effects] {
            modes.set_mode(mode);
            assert_eq!(modes.pending(), Some(mode));
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                modes.apply_pending(ui.ctx());
            });
            out.textures_delta.clear();
            assert_eq!((modes.mode(), modes.pending()), (mode, None));
        }
        // asking for the active mode is a no-op
        modes.set_mode(Mode::Effects);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(!modes.apply_pending(ui.ctx()));
        });
        out.textures_delta.clear();
        assert_eq!(modes.pending(), None);
        let keys: BTreeSet<String> = Mode::ALL.iter().map(|m| format!("{:?}", m.shortcut().logical_key)).collect();
        assert_eq!(keys.len(), 2);
        assert!(Mode::ALL.iter().all(|m| m.shortcut().modifiers == egui::Modifiers::COMMAND));
        assert_eq!((Mode::Edit.id(), Mode::Effects.id()), ("edit", "effects"));
    }

    #[test]
    fn a_switch_waits_until_the_merged_fonts_are_live() {
        let ctx = egui::Context::default();
        let mut modes = ModeState::default();
        let frame = |modes: &mut ModeState, f: &dyn Fn(&egui::Context)| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                f(ui.ctx());
                modes.sync_fonts(ui.ctx());
                modes.apply_pending(ui.ctx());
            });
            out.textures_delta.clear();
        };
        let named = |name: &str| {
            let mut defs = egui::FontDefinitions::default();
            defs.families.insert(egui::FontFamily::Name(name.into()), vec!["Hack".into()]);
            defs
        };
        // Frame 1: the Edit editor installs fonts of its own, with a named family.
        frame(&mut modes, &|ctx| ctx.set_fonts(named("film-ui")));
        assert!(modes.recorded_fonts().is_empty(), "set_fonts applies at the next frame");
        // Frame 2: the shell records them; one editor's set is the merged set, so a switch asked
        // for now goes through at once.
        modes.set_mode(Mode::Effects);
        frame(&mut modes, &|_| {});
        assert_eq!(modes.recorded_fonts(), vec![Mode::Edit]);
        assert_eq!((modes.mode(), modes.pending()), (Mode::Effects, None));
        // Frame 3: the Effects editor installs its own set, dropping Edit's family.
        frame(&mut modes, &|ctx| ctx.set_fonts(named("effects-ui")));
        // Frame 4: Effects' set is live and is not the merged one: a switch back must wait...
        modes.set_mode(Mode::Edit);
        frame(&mut modes, &|_| {});
        assert_eq!(modes.recorded_fonts(), vec![Mode::Edit, Mode::Effects]);
        assert_eq!((modes.mode(), modes.pending()), (Mode::Effects, Some(Mode::Edit)));
        // Frame 5: ...until the union is live: both named families present.
        frame(&mut modes, &|_| {});
        let live = ctx.fonts(|f| f.definitions().clone());
        assert!(live.families.contains_key(&egui::FontFamily::Name("film-ui".into())));
        assert!(live.families.contains_key(&egui::FontFamily::Name("effects-ui".into())));
        assert_eq!((modes.mode(), modes.pending()), (Mode::Edit, None));
        assert!(modes.fonts_settled(&ctx));
    }

    #[test]
    fn merged_fonts_are_the_union_and_do_not_depend_on_the_active_editor() {
        let mut a = egui::FontDefinitions::default();
        a.families.insert(egui::FontFamily::Proportional, vec!["A".into()]);
        a.families.insert(egui::FontFamily::Name("shared".into()), vec!["A".into()]);
        a.families.insert(egui::FontFamily::Name("only-a".into()), vec!["A".into()]);
        let mut b = egui::FontDefinitions::default();
        b.families.insert(egui::FontFamily::Proportional, vec!["B".into()]);
        b.families.insert(egui::FontFamily::Name("shared".into()), vec!["B".into()]);
        b.families.insert(egui::FontFamily::Name("only-b".into()), vec!["B".into()]);
        let all = HashMap::from([(Mode::Edit, a), (Mode::Effects, b)]);
        let merged = merge_fonts(&all);
        assert_eq!(merged.families[&egui::FontFamily::Proportional], vec!["A".to_string(), "B".to_string()]);
        assert_eq!(merged.families[&egui::FontFamily::Name("shared".into())], vec!["A".to_string(), "B".to_string()]);
        assert!(merged.families.contains_key(&egui::FontFamily::Name("only-a".into())));
        assert!(merged.families.contains_key(&egui::FontFamily::Name("only-b".into())));
        // font files are the union too (both carry egui's defaults here)
        assert_eq!(merged.font_data.len(), egui::FontDefinitions::default().font_data.len());
        // one editor alone is itself
        let one = HashMap::from([(Mode::Edit, all[&Mode::Edit].clone())]);
        assert_eq!(font_signature(&merge_fonts(&one)), font_signature(&all[&Mode::Edit]));
    }

    #[test]
    fn the_window_title_names_the_bundle_and_marks_unsaved_changes() {
        assert_eq!(window_title(None), "Redrob Film");
        assert_eq!(window_title(Some(("cut.rfilm", false))), "cut.rfilm - Redrob Film");
        assert_eq!(window_title(Some(("Untitled", true))), "Untitled* - Redrob Film");
        assert_eq!(file_name("/work/cut.rfilm"), "cut.rfilm");
        assert_eq!(file_name(""), "");
    }

    #[test]
    fn the_status_line_fades_after_its_lifetime() {
        let status = Status::new("done", false);
        let now = Instant::now();
        assert_eq!(status.alpha(now), Some(1.0));
        let late = now + Duration::from_secs_f32(STATUS_SECS - 0.5);
        let alpha = status.alpha(late).unwrap_or(0.0);
        assert!(alpha > 0.0 && alpha < 1.0, "{alpha}");
        assert_eq!(status.alpha(now + Duration::from_secs(60)), None);
    }

    #[test]
    fn control_replies_have_the_editors_shape() {
        assert_eq!(control_reply(Ok(json!(1))), json!({"ok": true, "result": 1}));
        assert_eq!(control_reply(Err("no".into())), json!({"ok": false, "error": "no"}));
    }

    #[test]
    fn ctrl_digits_are_consumed_before_the_editor_sees_them() {
        // The shortcut layer alone (no editors): the key event is consumed from the context.
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::Key { key: egui::Key::Num2, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND });
        let mut switched = None;
        let mut out = ctx.run_ui(raw, |ui| {
            for mode in Mode::ALL {
                if ui.ctx().input_mut(|i| i.consume_shortcut(&mode.shortcut())) {
                    switched = Some(mode);
                }
            }
            assert!(!ui.ctx().input_mut(|i| i.consume_shortcut(&Mode::Effects.shortcut())), "consumed once");
        });
        out.textures_delta.clear();
        assert_eq!(switched, Some(Mode::Effects));
    }
}
