//! Community and project links (shown in Help ▸, the About dialog, the start screen and the
//! header's issue button).

pub const APP_NAME: &str = "Redrob Film";
pub const APP_SLUG: &str = "effectcraft";
pub const WEBSITE: &str = "https://redrob.ai";
pub const APP_PAGE: &str = "https://redrob.ai";
pub const GITHUB: &str = "https://github.com/redrob-labs/redrob-film";
pub const ISSUES: &str = "https://github.com/redrob-labs/redrob-film/issues";

/// Sibling apps of the Redrob suite: (name, GitHub repo slug under redrob-labs).
pub const SIBLINGS: &[(&str, &str)] = &[
    ("Redrob Canvas", "redrob-canvas"),
    ("Redrob Shape", "redrob-shape"),
    ("Redrob Design", "redrob-design"),
    ("Redrob Office", "redrob-office"),
    ("Redrob Code", "redrob-code"),
];

pub fn github_of(slug: &str) -> String {
    format!("https://github.com/redrob-labs/{slug}")
}
pub fn page_of(_slug: &str) -> String {
    String::from("https://redrob.ai")
}
