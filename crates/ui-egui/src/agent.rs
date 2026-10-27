//! The Agent panel: a chat with Redrob Code, which edits this project through the app's own MCP
//! tools (the same `command_*`, `sequence_inspect`, `ui_*` tools any MCP client gets).
//!
//! Redrob Code is the built-in agent and the one the panel starts; other agents (Claude Code,
//! Codex, ...) connect from outside through `redrob-film-cli mcp` as before (`docs/agents.md`).
//!
//! The UI crate stays platform-neutral (it also runs in the browser): the agent process and its
//! protocol live behind [`AgentHost`], supplied by the desktop app through
//! [`crate::FilmApp::agent_services`]. Without one (the web build) the panel says so.
//!
//! The agent's model calls are billed to the connected Redrob workspace, so the first message
//! passes [`crate::account::require`]: without a workspace the Connect dialog opens and nothing
//! is sent. Editing never goes through here.
//!
//! Automation ids: `agent.input`, `agent.send`, `agent.stop`, `agent.connect`, `agent.addToRedrobCode`,
//! `agent.allow.<n>` (the options of a permission question). Commands: `agent.status`,
//! `agent.send {text}`, `agent.stop`, `agent.addToRedrobCode`.

use egui::{Rect, RichText, vec2};
use serde_json::{Value, json};

use crate::FilmApp;

/// One choice in a permission request, as the agent offered it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once` or `reject_always`.
    pub kind: String,
}

/// What the agent process reports, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentEvent {
    /// The session is open; `name` is the agent's own name and version.
    Ready { name: String },
    /// A piece of the agent's reply.
    Text(String),
    /// A piece of the agent's visible reasoning.
    Thought(String),
    /// A tool call started or changed state.
    Tool { id: String, title: String, status: String },
    /// The agent asks before running a tool; answer with [`AgentHost::answer`].
    Permission { request: u64, title: String, options: Vec<PermissionOption> },
    /// The turn ended; `stop` is the protocol's stop reason (`end_turn`, `cancelled`, ...).
    TurnEnd { stop: String },
    /// Something the user should read that is not part of the reply.
    Notice(String),
    /// The turn or the session failed.
    Error(String),
}

/// A running agent session. Every call returns at once; results arrive through [`Self::poll`].
pub trait AgentHost {
    fn prompt(&mut self, text: &str);
    fn cancel(&mut self);
    /// Answer a [`AgentEvent::Permission`]; `None` declines.
    fn answer(&mut self, request: u64, option: Option<&str>);
    fn poll(&mut self) -> Vec<AgentEvent>;
}

/// Starts an agent session on first use.
pub type AgentFactory = Box<dyn FnMut() -> Result<Box<dyn AgentHost>, String>>;

/// What the desktop app supplies for the panel.
#[derive(Default)]
pub struct AgentServices {
    pub factory: Option<AgentFactory>,
    /// Register this app's MCP server in the user's Redrob Code config; `Ok` names the file.
    pub add_to_redrob_code: Option<Box<dyn FnMut() -> Result<String, String>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    User(String),
    Agent(String),
    Thought(String),
    Tool { id: String, title: String, status: String },
    Notice(String),
    Error(String),
}

#[derive(Default)]
pub struct AgentPanel {
    host: Option<Box<dyn AgentHost>>,
    pub entries: Vec<Entry>,
    pub input: String,
    pub busy: bool,
    pub agent_name: Option<String>,
    pub pending: Option<(u64, String, Vec<PermissionOption>)>,
    /// The account generation the running host was started with.
    generation: u64,
}

impl AgentPanel {
    /// Fold one event into the transcript. Text and thought chunks extend the open entry.
    pub fn apply(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Ready { name } => self.agent_name = Some(name),
            AgentEvent::Text(s) => match self.entries.last_mut() {
                Some(Entry::Agent(t)) => t.push_str(&s),
                _ => self.entries.push(Entry::Agent(s)),
            },
            AgentEvent::Thought(s) => match self.entries.last_mut() {
                Some(Entry::Thought(t)) => t.push_str(&s),
                _ => self.entries.push(Entry::Thought(s)),
            },
            AgentEvent::Tool { id, title, status } => {
                let found = self.entries.iter_mut().rev().find_map(|e| match e {
                    Entry::Tool { id: i, title: t, status: s } if *i == id => Some((t, s)),
                    _ => None,
                });
                match found {
                    Some((t, s)) => {
                        if !title.is_empty() {
                            *t = title;
                        }
                        if !status.is_empty() {
                            *s = status;
                        }
                    }
                    None => self.entries.push(Entry::Tool { id, title, status }),
                }
            }
            AgentEvent::Permission { request, title, options } => self.pending = Some((request, title, options)),
            AgentEvent::TurnEnd { stop } => {
                self.busy = false;
                self.pending = None;
                if stop != "end_turn" {
                    self.entries.push(Entry::Notice(match stop.as_str() {
                        "cancelled" => tl!("Stopped.").to_string(),
                        "max_tokens" => tl!("The reply hit its length limit.").to_string(),
                        "refusal" => tl!("Redrob Code declined this request.").to_string(),
                        other => tlf!("The turn ended: {reason}.", reason = other),
                    }));
                }
            }
            AgentEvent::Notice(s) => self.entries.push(Entry::Notice(s)),
            AgentEvent::Error(s) => {
                self.busy = false;
                self.pending = None;
                self.entries.push(Entry::Error(s));
            }
        }
    }

    /// A different workspace key: the next message starts a fresh agent that uses it.
    pub fn account_changed(&mut self, generation: u64) {
        if generation == self.generation {
            return;
        }
        self.generation = generation;
        if self.host.take().is_some() {
            self.busy = false;
            self.pending = None;
            self.agent_name = None;
            self.entries.push(Entry::Notice(tl!("Workspace changed. The next message starts the agent with it.").to_string()));
        }
    }

    /// Whether an agent process is running.
    pub fn running(&self) -> bool {
        self.host.is_some()
    }

    /// Send the composer's text, starting the agent first if needed. The caller has already
    /// passed the account gate.
    fn send(&mut self, factory: &mut Option<AgentFactory>) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.busy {
            return;
        }
        if self.host.is_none() {
            let Some(f) = factory.as_mut() else {
                self.entries.push(Entry::Error(tl!("The agent runs in the desktop app only.").to_string()));
                return;
            };
            match f() {
                Ok(h) => self.host = Some(h),
                Err(e) => {
                    self.entries.push(Entry::Error(e));
                    return;
                }
            }
        }
        self.input.clear();
        self.entries.push(Entry::User(text.clone()));
        self.busy = true;
        if let Some(h) = self.host.as_mut() {
            h.prompt(&text);
        }
    }

    /// Drain the host. Returns true while a turn is running, so the caller keeps repainting.
    pub fn poll(&mut self) -> bool {
        if let Some(h) = self.host.as_mut() {
            for ev in h.poll() {
                self.apply(ev);
            }
        }
        self.busy || self.pending.is_some()
    }

    fn answer(&mut self, option: Option<String>) {
        if let (Some((req, _, _)), Some(h)) = (self.pending.take(), self.host.as_mut()) {
            h.answer(req, option.as_deref());
        }
    }

    fn cancel(&mut self) {
        if let Some(h) = self.host.as_mut() {
            h.cancel();
        }
    }
}

/// Send what is in the composer: the account gate first (it opens the Connect dialog and keeps
/// the text when there is no workspace), then the agent.
fn submit(app: &mut FilmApp) {
    if app.agent.input.trim().is_empty() || app.agent.busy {
        return;
    }
    if !app.agent.running() && app.agent_services.factory.is_some() && !crate::account::require(app, "Redrob Code") {
        return;
    }
    app.agent.send(&mut app.agent_services.factory);
}

fn add_to_redrob_code(app: &mut FilmApp) -> Result<Value, String> {
    let Some(f) = app.agent_services.add_to_redrob_code.as_mut() else {
        return Err(tl!("The agent runs in the desktop app only.").to_string());
    };
    let path = f()?;
    app.ui.status = tlf!("Added Redrob Film to Redrob Code ({path})", path);
    Ok(json!({"path": path}))
}

/// `agent.*` commands (menu, control channel). `None` for any other id.
pub fn route(app: &mut FilmApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    match id {
        "agent.status" => Some(Ok(json!({
            "available": app.agent_services.factory.is_some(),
            "running": app.agent.running(),
            "agent": app.agent.agent_name,
            "busy": app.agent.busy,
            "pending": app.agent.pending.as_ref().map(|(_, title, _)| title.clone()),
            "entries": app.agent.entries.len(),
        }))),
        "agent.send" => {
            let Some(text) = params.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) else {
                return Some(Err("agent.send needs `text`".into()));
            };
            app.show_panel(crate::dock::PanelKind::Agent);
            app.agent.input = text.to_string();
            submit(app);
            Some(Ok(json!({"sent": app.agent.input.is_empty(), "busy": app.agent.busy})))
        }
        "agent.stop" => {
            app.agent.cancel();
            Some(Ok(Value::Null))
        }
        "agent.addToRedrobCode" => Some(add_to_redrob_code(app)),
        _ => None,
    }
}

/// Side margin of the chat, in points.
const MARGIN: f32 = 10.0;
/// Gap between two messages.
const GAP: f32 = 8.0;
/// A user bubble takes at most this share of the log's width.
const BUBBLE_SHARE: f32 = 0.85;

/// The panel body.
pub fn show(app: &mut FilmApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    if let Some(g) = app.account.as_ref().map(|a| a.generation()) {
        app.agent.account_changed(g);
    }
    if app.agent.poll() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(MARGIN, 6.0))));
    let ui = &mut child;
    ui.horizontal(|ui| {
        // The button keeps its place on the right; the title and the agent's name take what is
        // left and elide ("Redrob Co…") instead of running under the dock's edge.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if app.agent_services.add_to_redrob_code.is_some() {
                let b = ui
                    .small_button(tl!("Add to Redrob Code"))
                    .on_hover_text(tl!("Lets Redrob Code in a terminal edit projects with this app's tools (adds it to ~/.config/redrob/redrob.json)."));
                app.auto.add("agent.addToRedrobCode", b.rect, "Add to Redrob Code");
                if b.clicked()
                    && let Err(e) = add_to_redrob_code(app)
                {
                    app.ui.status = e;
                }
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new("Redrob Code").strong().color(t.text)).truncate());
                if let Some(n) = &app.agent.agent_name {
                    ui.add(egui::Label::new(RichText::new(n).small().color(t.text_faint)).truncate());
                }
            });
        });
    });
    if app.agent_services.factory.is_some() && !crate::account::connected(app) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tl!("Redrob Code needs a Redrob workspace.")).small().color(t.text_dim));
            let b = ui.small_button(tl!("Connect…"));
            app.auto.add("agent.connect", b.rect, "Connect");
            if b.clicked() {
                crate::account::require(app, "Redrob Code");
            }
        });
    }
    ui.add_space(4.0);
    ui.separator();
    chat(app, ui);
}

fn chat(app: &mut FilmApp, ui: &mut egui::Ui) {
    let t = app.tokens;
    let error_color = egui::Color32::from_rgb(0xff, 0x8a, 0x80);
    let composer_h = 92.0 + if app.agent.pending.is_some() { 52.0 } else { 0.0 };
    let list_h = (ui.available_height() - composer_h).max(60.0);
    egui::ScrollArea::vertical().id_salt("agent_log").max_height(list_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        ui.add_space(GAP);
        let panel = &app.agent;
        if panel.entries.is_empty() {
            ui.add(egui::Label::new(RichText::new(tl!("Ask Redrob Code to cut, arrange, caption or check this edit.")).color(t.text_dim)).wrap());
        }
        let full = ui.available_width();
        for e in &panel.entries {
            match e {
                Entry::User(s) => {
                    // The user's own words sit on the right, in a bubble.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        egui::Frame::NONE.fill(t.control_surface).corner_radius(10).inner_margin(egui::Margin::symmetric(10, 7)).show(ui, |ui| {
                            ui.set_max_width(full * BUBBLE_SHARE);
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.add(egui::Label::new(RichText::new(s).color(t.text)).wrap());
                            });
                        });
                    });
                }
                Entry::Agent(s) => {
                    ui.add(egui::Label::new(RichText::new(s).color(t.text)).wrap().selectable(true));
                }
                Entry::Thought(s) => {
                    ui.add(egui::Label::new(RichText::new(s).italics().small().color(t.text_faint)).wrap());
                }
                Entry::Tool { title, status, .. } => {
                    let mark = match status.as_str() {
                        "completed" => "✓",
                        "failed" => "✗",
                        _ => "…",
                    };
                    egui::Frame::NONE
                        .fill(t.control_well)
                        .stroke(egui::Stroke::new(1.0, t.control_border))
                        .corner_radius(6)
                        .inner_margin(egui::Margin::symmetric(8, 3))
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(format!("{mark} {title}")).small().color(t.text_dim)).wrap());
                        });
                }
                Entry::Notice(s) => {
                    ui.add(egui::Label::new(RichText::new(s).small().color(t.text_dim)).wrap());
                }
                Entry::Error(s) => {
                    ui.add(egui::Label::new(RichText::new(s).small().color(error_color)).wrap());
                }
            }
            ui.add_space(GAP);
        }
    });
    let mut answer: Option<Option<String>> = None;
    if let Some((_, title, options)) = &app.agent.pending {
        ui.add(egui::Label::new(RichText::new(tlf!("Allow {tool}?", tool = title)).color(t.text)).wrap());
        let mut rects = Vec::new();
        ui.horizontal_wrapped(|ui| {
            for o in options {
                let b = ui.button(&o.name);
                rects.push((b.rect, o.name.clone()));
                if b.clicked() {
                    answer = Some(if o.kind.starts_with("reject") { None } else { Some(o.id.clone()) });
                }
            }
        });
        for (i, (r, name)) in rects.into_iter().enumerate() {
            app.auto.add(&format!("agent.allow.{i}"), r, &name);
        }
    }
    if let Some(a) = answer {
        app.agent.answer(a);
    }
    ui.add_space(6.0);
    let mut send = false;
    let mut stop = false;
    let busy = app.agent.busy;
    let mut input_rect = Rect::NOTHING;
    let mut button: Option<(Rect, &str)> = None;
    // The composer: one rounded box holding the text and the Send button, like a chat app.
    egui::Frame::NONE.fill(t.field_bg).stroke(egui::Stroke::new(1.0, t.field_border)).corner_radius(10).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        let edit = egui::TextEdit::multiline(&mut app.agent.input)
            .id_salt("agent-input")
            .hint_text(tl!("Message Redrob Code"))
            .frame(egui::Frame::NONE)
            .desired_rows(2)
            .desired_width(ui.available_width());
        let r = ui.add_enabled(!busy, edit);
        input_rect = r.rect;
        if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
            send = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if busy {
                let b = ui.add_sized(vec2(60.0, 22.0), egui::Button::new(tl!("Stop")));
                button = Some((b.rect, "agent.stop"));
                if b.clicked() {
                    stop = true;
                }
                ui.spinner();
            } else {
                let empty = app.agent.input.trim().is_empty();
                let b =
                    ui.add_enabled(!empty, egui::Button::new(RichText::new(tl!("Send")).color(egui::Color32::WHITE)).fill(t.accent).min_size(vec2(60.0, 22.0)));
                button = Some((b.rect, "agent.send"));
                if b.clicked() {
                    send = true;
                }
            }
        });
    });
    app.auto.add("agent.input", input_rect, "Message Redrob Code");
    if let Some((r, id)) = button {
        app.auto.add(id, r, if id == "agent.stop" { "Stop" } else { "Send" });
    }
    if send {
        // A trailing newline from Enter is not part of the message.
        let trimmed = app.agent.input.trim_end_matches('\n').to_string();
        app.agent.input = trimmed;
        submit(app);
    }
    if stop {
        app.agent.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn chunks_merge_and_tools_update_in_place() {
        let mut p = AgentPanel { busy: true, ..Default::default() };
        p.apply(AgentEvent::Text("Cutting ".into()));
        p.apply(AgentEvent::Text("the clip.".into()));
        p.apply(AgentEvent::Tool { id: "t1".into(), title: "command_run".into(), status: "pending".into() });
        p.apply(AgentEvent::Tool { id: "t1".into(), title: String::new(), status: "completed".into() });
        p.apply(AgentEvent::Text("Done.".into()));
        p.apply(AgentEvent::TurnEnd { stop: "end_turn".into() });
        assert_eq!(
            p.entries,
            vec![
                Entry::Agent("Cutting the clip.".into()),
                Entry::Tool { id: "t1".into(), title: "command_run".into(), status: "completed".into() },
                Entry::Agent("Done.".into()),
            ]
        );
        assert!(!p.busy);
    }

    #[test]
    fn cancelled_turn_leaves_a_notice_and_clears_a_pending_question() {
        let mut p = AgentPanel { busy: true, ..Default::default() };
        p.apply(AgentEvent::Permission { request: 3, title: "command_run".into(), options: vec![] });
        assert!(p.pending.is_some());
        p.apply(AgentEvent::TurnEnd { stop: "cancelled".into() });
        assert!(p.pending.is_none());
        assert_eq!(p.entries, vec![Entry::Notice("Stopped.".into())]);
    }

    #[derive(Default)]
    struct Echo(Rc<RefCell<Vec<String>>>);
    impl AgentHost for Echo {
        fn prompt(&mut self, text: &str) {
            self.0.borrow_mut().push(text.to_string());
        }
        fn cancel(&mut self) {}
        fn answer(&mut self, _: u64, _: Option<&str>) {}
        fn poll(&mut self) -> Vec<AgentEvent> {
            Vec::new()
        }
    }

    #[test]
    fn a_new_workspace_key_restarts_the_agent() {
        let mut p = AgentPanel { input: "hi".into(), ..Default::default() };
        let mut factory: Option<AgentFactory> = Some(Box::new(|| Ok(Box::new(Echo::default()) as Box<dyn AgentHost>)));
        p.send(&mut factory);
        assert!(p.running());
        p.account_changed(0);
        assert!(p.running(), "same generation keeps the agent");
        p.account_changed(1);
        assert!(!p.running() && !p.busy);
        assert_eq!(p.entries.last(), Some(&Entry::Notice("Workspace changed. The next message starts the agent with it.".into())));
    }

    #[test]
    fn without_a_host_factory_the_panel_says_so() {
        let mut p = AgentPanel { input: "cut here".into(), ..Default::default() };
        p.send(&mut None);
        assert_eq!(p.entries, vec![Entry::Error("The agent runs in the desktop app only.".into())]);
        assert!(!p.busy);
    }

    struct Account(bool);
    impl crate::account::AccountService for Account {
        fn account(&self) -> Option<crate::account::Account> {
            self.0.then(|| crate::account::Account { account_name: "W".into(), key_name: "k".into(), via_launcher: false })
        }
        fn state(&mut self) -> crate::account::ConnectState {
            crate::account::ConnectState::Idle
        }
        fn connect(&mut self) {}
        fn cancel(&mut self) {}
        fn disconnect(&mut self) {}
        fn generation(&self) -> u64 {
            0
        }
    }

    fn app_with(connected: bool, sent: Rc<RefCell<Vec<String>>>) -> FilmApp {
        let mut app = FilmApp::new(redrob_film_engine::Session::default());
        app.account = Some(Box::new(Account(connected)));
        app.agent_services.factory = Some(Box::new(move || Ok(Box::new(Echo(sent.clone())) as Box<dyn AgentHost>)));
        app
    }

    #[test]
    fn the_first_message_waits_for_a_workspace() {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let mut app = app_with(false, sent.clone());
        let r = route(&mut app, "agent.send", &json!({"text": "trim the gaps"})).unwrap().unwrap();
        assert_eq!(r["sent"], false);
        assert_eq!(app.dialog, Some(crate::Dialog::RedrobConnect), "the Connect dialog opens instead");
        assert_eq!(app.agent.input, "trim the gaps", "the text is kept for after connecting");
        assert!(sent.borrow().is_empty() && !app.agent.running());
    }

    #[test]
    fn a_connected_workspace_starts_redrob_code() {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let mut app = app_with(true, sent.clone());
        let r = route(&mut app, "agent.send", &json!({"text": "trim the gaps"})).unwrap().unwrap();
        assert_eq!(r, json!({"sent": true, "busy": true}));
        assert_eq!(*sent.borrow(), vec!["trim the gaps".to_string()]);
        assert!(app.ui.dock.contains(crate::dock::PanelKind::Agent));
        let s = route(&mut app, "agent.status", &json!({})).unwrap().unwrap();
        assert_eq!(s["running"], true);
        assert_eq!(s["available"], true);
        assert!(route(&mut app, "agent.nope", &json!({})).is_none());
    }

    #[test]
    fn add_to_redrob_code_needs_the_desktop() {
        let mut app = FilmApp::new(redrob_film_engine::Session::default());
        assert!(route(&mut app, "agent.addToRedrobCode", &json!({})).unwrap().is_err());
        app.agent_services.add_to_redrob_code = Some(Box::new(|| Ok("/x/redrob.json".into())));
        assert_eq!(route(&mut app, "agent.addToRedrobCode", &json!({})).unwrap(), Ok(json!({"path": "/x/redrob.json"})));
    }
}
