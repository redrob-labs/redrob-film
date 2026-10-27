//! Project links (Help menu, About dialog, Home screen).
//!
//! Redrob Film is a Redrob product: the product page lives on the Redrob website and the source in
//! the redrob-labs organisation on GitHub. There is no third-party community service wired in.

/// The Redrob website.
pub const WEBSITE: &str = "https://redrob.io";
/// Redrob Film's source repository.
pub const GITHUB: &str = "https://github.com/redrob-labs/redrob-film";
/// New issue on Redrob Film's repository.
pub const ISSUES: &str = "https://github.com/redrob-labs/redrob-film/issues";

/// (command id, label, url) for every link, in menu order.
pub const ALL: [(&str, &str, &str); 3] =
    [("help.website", "Redrob Website", WEBSITE), ("help.github", "Source code on GitHub", GITHUB), ("help.reportIssue", "Report an Issue…", ISSUES)];

/// The URL a `help.*` link command opens.
pub fn url_for(command: &str) -> Option<&'static str> {
    ALL.iter().find(|(id, _, _)| *id == command).map(|(_, _, u)| *u)
}

/// Open `url` in the system browser (a new tab on the web). The desktop host's
/// [`HostHooks::open_url`](crate::HostHooks::open_url) opens it; without the hook, or when it
/// fails, it goes through `ctx.open_url`, which only does something on the web: the native egui
/// backend is built without its `links` feature, so there it would only log (#642).
pub fn open(app: &mut crate::FilmApp, ctx: &egui::Context, url: &str) {
    if let Some(f) = app.hooks.open_url.as_mut() {
        match f(url) {
            Ok(()) => return,
            Err(e) => log::warn!("can't open {url} in the browser: {e}"),
        }
    }
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_follow_the_redrob_scheme() {
        assert_eq!(GITHUB, "https://github.com/redrob-labs/redrob-film");
        assert_eq!(ISSUES, format!("{GITHUB}/issues"));
        assert!(ALL.iter().all(|(id, _, u)| id.starts_with("help.") && u.starts_with("https://")));
        assert_eq!(url_for("help.website"), Some(WEBSITE));
        assert_eq!(url_for("help.nope"), None);
        // No third-party community service: every link stays on our own domains.
        for (_, _, u) in ALL {
            assert!(u.starts_with("https://redrob.io") || u.starts_with("https://github.com/redrob-labs/"), "{u}");
        }
    }

    #[test]
    fn links_open_through_the_host_hook() {
        // #642: on the desktop every link (Help menu, About, Import screen) must
        // reach the host's browser opener; `ctx.open_url` alone is a no-op natively.
        use std::cell::RefCell;
        use std::rc::Rc;
        let opened: Rc<RefCell<Vec<String>>> = Rc::default();
        let rec = opened.clone();
        let mut app = crate::FilmApp::new(redrob_film_engine::Session::default());
        app.hooks.open_url = Some(Box::new(move |u: &str| {
            rec.borrow_mut().push(u.to_string());
            Ok(())
        }));
        let ctx = egui::Context::default();
        for (id, _, url) in ALL {
            let r = crate::menus::invoke(&mut app, &ctx, id, serde_json::json!({})).unwrap();
            assert_eq!(r["url"], url, "{id}");
            assert_eq!(opened.borrow().last().map(String::as_str), Some(url), "{id}");
        }
        open(&mut app, &ctx, WEBSITE);
        assert_eq!(opened.borrow().last().map(String::as_str), Some(WEBSITE));
        assert_eq!(opened.borrow().len(), ALL.len() + 1);

        // A failing opener falls back to `ctx.open_url` instead of losing the click.
        app.hooks.open_url = Some(Box::new(|_: &str| Err("no browser".into())));
        open(&mut app, &ctx, ISSUES);
        let fallback: Vec<String> =
            ctx.output(|o| o.commands.iter().filter_map(|c| if let egui::OutputCommand::OpenUrl(u) = c { Some(u.url.clone()) } else { None }).collect());
        assert_eq!(fallback, [ISSUES]);
    }
}
