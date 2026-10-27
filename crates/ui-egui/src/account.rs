//! The Redrob workspace this app is connected to.
//!
//! Editing never needs an account. The AI features do: the built-in agent's model calls are billed
//! to a Redrob workspace. Connecting uses the console's device flow, so nobody copies a key: the
//! app shows a short code, the person approves it on console.redrob.ai, and the console hands this
//! machine a workspace key. The flow and the stored key live behind [`AccountService`], supplied
//! by the desktop app ([`crate::FilmApp::account`]); the UI only ever sees names, never the
//! key.
//!
//! Nothing here runs on its own. [`require`] is the one gate: an AI feature calls it right before
//! it needs the key, and gets `false` plus the connect dialog when there is no account yet.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::FilmApp;

/// What may be shown about the connected workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub account_name: String,
    pub key_name: String,
    /// The key is the computer's shared one, from Redrob Launcher: signing out happens there.
    pub via_launcher: bool,
}

/// A connection the person is approving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingConnect {
    pub user_code: String,
    /// The approval page with the code filled in.
    pub url: String,
    /// Seconds left before the code expires.
    pub seconds_left: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectState {
    Idle,
    Starting,
    Waiting(PendingConnect),
    /// The attempt ended without a key; the text says why in plain words.
    Failed(String),
}

/// The desktop app's account store and device-flow client. Every call returns at once.
pub trait AccountService {
    fn account(&self) -> Option<Account>;
    fn state(&mut self) -> ConnectState;
    fn connect(&mut self);
    fn cancel(&mut self);
    fn disconnect(&mut self);
    /// Bumped whenever the stored key changes, so a running agent can be restarted with it.
    fn generation(&self) -> u64;
}

/// Whether a Redrob workspace is connected. Without a service (web, tests) there never is.
pub fn connected(app: &FilmApp) -> bool {
    app.account.as_ref().is_some_and(|s| s.account().is_some())
}

/// The gate an AI feature passes right before it needs the workspace key: `true` when connected;
/// otherwise the connect dialog opens (remembering `feature` for its title) and this is `false`.
/// Editing commands never call this.
pub fn require(app: &mut FilmApp, feature: &str) -> bool {
    if connected(app) {
        return true;
    }
    app.ui.account_feature = Some(feature.to_string());
    app.dialog = Some(crate::Dialog::RedrobConnect);
    false
}

/// `account.*` commands (menu, control channel, agents). `None` for any other id.
pub fn route(app: &mut FilmApp, id: &str) -> Option<Result<Value, String>> {
    match id {
        "account.status" => Some(Ok(status(app))),
        "account.connect" => {
            if connected(app) {
                return Some(Ok(status(app)));
            }
            app.ui.account_feature = None;
            app.dialog = Some(crate::Dialog::RedrobConnect);
            if let Some(s) = app.account.as_mut() {
                s.connect();
            }
            Some(Ok(status(app)))
        }
        "account.disconnect" => {
            if let Some(s) = app.account.as_mut() {
                s.disconnect();
            }
            Some(Ok(status(app)))
        }
        _ => None,
    }
}

/// `{connected, accountName?, keyName?, state}` as the control channel reports it. Never the key.
pub fn status(app: &mut FilmApp) -> Value {
    let Some(s) = app.account.as_mut() else {
        return json!({"connected": false, "available": false, "state": "unavailable"});
    };
    let state = match s.state() {
        ConnectState::Idle => json!("idle"),
        ConnectState::Starting => json!("starting"),
        ConnectState::Waiting(p) => json!({"waiting": {"userCode": p.user_code, "url": p.url, "secondsLeft": p.seconds_left}}),
        ConnectState::Failed(why) => json!({"failed": why}),
    };
    match s.account() {
        Some(a) => json!({"connected": true, "available": true, "accountName": a.account_name, "keyName": a.key_name, "state": state}),
        None => json!({"connected": false, "available": true, "state": state}),
    }
}

/// The Connect dialog. Returns false once it should close.
pub fn connect_dialog(app: &mut FilmApp, ctx: &egui::Context) -> bool {
    let mut open = true;
    let title = tl!("Connect to Redrob");
    egui::Window::new(title)
        .id(egui::Id::new("redrob-connect"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(380.0);
            let t = app.tokens;
            if let Some(f) = app.ui.account_feature.clone() {
                ui.label(RichText::new(tlf!("{feature} needs a Redrob workspace.", feature = crate::i18n::t(&f))).color(t.text));
            }
            ui.label(
                RichText::new(tl!("Editing is free and never needs an account. AI features are billed to the Redrob workspace connected here."))
                    .small()
                    .color(t.text_dim),
            );
            ui.add_space(8.0);
            body(app, ui, "connect");
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        open = false;
    }
    if !open {
        app.ui.account_feature = None;
        // A closed dialog abandons a pending code: the next Connect asks for a fresh one.
        if let Some(s) = app.account.as_mut()
            && matches!(s.state(), ConnectState::Starting | ConnectState::Waiting(_))
        {
            s.cancel();
        }
    }
    open
}

/// Settings ▸ Redrob Account block.
pub fn settings_block(app: &mut FilmApp, ui: &mut egui::Ui) {
    body(app, ui, "settings.account");
}

/// The shared account UI: the connected workspace with Disconnect, a pending code with its link
/// and countdown, or the Connect button (with the last failure in plain words above it).
///
/// Automation ids: `<prefix>.connect`, `<prefix>.cancel`, `<prefix>.disconnect`, `<prefix>.code`,
/// `<prefix>.open`.
fn body(app: &mut FilmApp, ui: &mut egui::Ui, prefix: &str) {
    let t = app.tokens;
    let Some(svc) = app.account.as_mut() else {
        ui.label(RichText::new(tl!("Connecting a Redrob workspace is available in the desktop app.")).small().color(t.text_dim));
        return;
    };
    let state = svc.state();
    let account = svc.account();
    let mut action: Option<fn(&mut dyn AccountService)> = None;
    let mut open_url: Option<String> = None;
    match (&account, &state) {
        (_, ConnectState::Starting) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(tl!("Asking console.redrob.ai for a code…")).small().color(t.text_dim));
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
        (_, ConnectState::Waiting(p)) => {
            ui.label(RichText::new(tl!("Approve this code on console.redrob.ai:")).small().color(t.text_dim));
            let code = ui.label(RichText::new(&p.user_code).size(26.0).monospace().color(t.text));
            app.auto.add(&format!("{prefix}.code"), code.rect, &p.user_code);
            ui.horizontal(|ui| {
                let link = ui.link(tl!("Open console.redrob.ai"));
                app.auto.add(&format!("{prefix}.open"), link.rect, &p.url);
                if link.clicked() {
                    open_url = Some(p.url.clone());
                }
                ui.label(RichText::new(tlf!("{time} left", time = format!("{}:{:02}", p.seconds_left / 60, p.seconds_left % 60))).small().color(t.text_dim));
                let cancel = ui.small_button(tl!("Cancel"));
                app.auto.add(&format!("{prefix}.cancel"), cancel.rect, "Cancel");
                if cancel.clicked() {
                    action = Some(|s| s.cancel());
                }
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        }
        (Some(a), _) => {
            ui.label(RichText::new(tlf!("Workspace: {name}", name = a.account_name)).color(t.text));
            ui.label(RichText::new(tlf!("Key: {name}", name = a.key_name)).small().color(t.text_dim));
            ui.add_space(4.0);
            if a.via_launcher {
                ui.label(RichText::new(tl!("Signed in with Redrob Launcher")).small().color(t.text_dim));
            } else {
                let b = ui.add_sized(vec2(140.0, 26.0), egui::Button::new(tl!("Disconnect")));
                app.auto.add(&format!("{prefix}.disconnect"), b.rect, "Disconnect");
                if b.clicked() {
                    action = Some(|s| s.disconnect());
                }
                ui.label(
                    RichText::new(tl!("Disconnecting removes the key from this computer. Revoke it on console.redrob.ai to invalidate it."))
                        .small()
                        .color(t.text_dim),
                );
            }
        }
        (None, s) => {
            if let ConnectState::Failed(why) = s {
                ui.label(RichText::new(why).small().color(t.text_dim));
                ui.add_space(4.0);
            }
            let b =
                ui.add_sized(vec2(220.0, 30.0), egui::Button::new(RichText::new(tl!("Connect a Redrob workspace")).color(egui::Color32::WHITE)).fill(t.accent));
            app.auto.add(&format!("{prefix}.connect"), b.rect, "Connect a Redrob workspace");
            if b.clicked() {
                action = Some(|s| s.connect());
            }
        }
    }
    if let Some(f) = action {
        f(svc.as_mut());
    }
    if let Some(url) = open_url {
        let ctx = ui.ctx().clone();
        crate::links::open(app, &ctx, &url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A scripted account service: the test sets what it answers.
    struct Fake {
        account: Option<Account>,
        state: ConnectState,
        calls: Rc<RefCell<Vec<&'static str>>>,
    }
    impl AccountService for Fake {
        fn account(&self) -> Option<Account> {
            self.account.clone()
        }
        fn state(&mut self) -> ConnectState {
            self.state.clone()
        }
        fn connect(&mut self) {
            self.calls.borrow_mut().push("connect");
            self.state = ConnectState::Waiting(PendingConnect {
                user_code: "ABCD-EFGH".into(),
                url: "https://console.redrob.ai/connect?code=ABCD-EFGH".into(),
                seconds_left: 600,
            });
        }
        fn cancel(&mut self) {
            self.calls.borrow_mut().push("cancel");
            self.state = ConnectState::Idle;
        }
        fn disconnect(&mut self) {
            self.calls.borrow_mut().push("disconnect");
            self.account = None;
        }
        fn generation(&self) -> u64 {
            0
        }
    }

    fn app_with(account: Option<Account>) -> (FilmApp, Rc<RefCell<Vec<&'static str>>>) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut app = FilmApp::new(redrob_film_engine::Session::default());
        app.account = Some(Box::new(Fake { account, state: ConnectState::Idle, calls: calls.clone() }));
        (app, calls)
    }

    #[test]
    fn require_opens_the_dialog_only_when_disconnected() {
        let (mut app, _) = app_with(None);
        assert!(!require(&mut app, "Redrob Code"));
        assert_eq!(app.dialog, Some(crate::Dialog::RedrobConnect));
        assert_eq!(app.ui.account_feature.as_deref(), Some("Redrob Code"));

        let (mut app, _) = app_with(Some(Account { account_name: "Acme".into(), key_name: "Redrob Film".into(), via_launcher: false }));
        assert!(require(&mut app, "Redrob Code"));
        assert_eq!(app.dialog, None);

        // No service at all (web, headless): never connected, and the dialog still explains itself.
        let mut app = FilmApp::new(redrob_film_engine::Session::default());
        assert!(!connected(&app));
        assert!(!require(&mut app, "Redrob Code"));
    }

    #[test]
    fn commands_report_status_without_the_key_and_drive_the_service() {
        let (mut app, calls) = app_with(None);
        let s = route(&mut app, "account.status").unwrap().unwrap();
        assert_eq!(s, json!({"connected": false, "available": true, "state": "idle"}));
        let s = route(&mut app, "account.connect").unwrap().unwrap();
        assert_eq!(s["connected"], json!(false));
        assert_eq!(s["state"]["waiting"]["userCode"], json!("ABCD-EFGH"));
        assert_eq!(app.dialog, Some(crate::Dialog::RedrobConnect));
        assert_eq!(*calls.borrow(), vec!["connect"]);
        assert!(route(&mut app, "account.nope").is_none());

        let (mut app, calls) = app_with(Some(Account { account_name: "Acme".into(), key_name: "Redrob Film".into(), via_launcher: false }));
        let s = route(&mut app, "account.status").unwrap().unwrap();
        assert_eq!(s, json!({"connected": true, "available": true, "accountName": "Acme", "keyName": "Redrob Film", "state": "idle"}));
        assert!(s.to_string().find("rk_").is_none());
        // connect while connected is a no-op
        assert_eq!(route(&mut app, "account.connect").unwrap().unwrap()["connected"], json!(true));
        assert!(calls.borrow().is_empty());
        let s = route(&mut app, "account.disconnect").unwrap().unwrap();
        assert_eq!(s["connected"], json!(false));
        assert_eq!(*calls.borrow(), vec!["disconnect"]);

        let mut app = FilmApp::new(redrob_film_engine::Session::default());
        assert_eq!(route(&mut app, "account.status").unwrap().unwrap(), json!({"connected": false, "available": false, "state": "unavailable"}));
    }
}
