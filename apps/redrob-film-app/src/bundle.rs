//! One project bundle for both editors: the `.rfilm` stays the Edit project, and the effects
//! project is stored beside it as `<name>.rfilm.effects.json` (the effects project's own JSON,
//! `Project::to_json` / `from_json`, editors/effects/crates/project/src/lib.rs L1532 / L1539).
//!
//! The Edit engine's open and save paths are not patched; the shell watches their effects on the
//! session every frame ([`BundleTracker::observe`]):
//! - `open_project` → `install_project` (crates/engine/src/commands.rs L3878 / L3853) puts a new
//!   `MediaPool` in `Session::media` (L3855) and sets `path` (L3870): a changed pool pointer is a
//!   project installed (Open, New, Recover, Revert), and the shell loads the bundle's effects file
//!   (or starts Effects empty when there is none);
//! - `write_project` with `adopt` (Save, Save As; L3814) sets `saved_revision = revision` (L3841)
//!   and `path` (L3838) without touching the pool: a changed `saved_revision` with the same pool
//!   is a save, and the shell writes the effects file next to the new path.
//!
//! After a load, the composition clips of the Edit project get their live sources back
//! ([`crate::handoff::rebind_composition_sources`]).
//!
//! Not covered: Edit's auto-save and crash-recovery journal (crates/engine/src/autosave.rs) hold
//! the Edit project only; the effects project is saved when the project is saved.

use std::path::Path;

/// The effects file of the Edit project at `rfilm`.
pub fn effects_path(rfilm: &str) -> String {
    format!("{rfilm}.effects.json")
}

/// What the Edit session did since the last frame, as far as the bundle is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleEvent {
    /// A project was installed (open / new / recover): load its effects file, if any.
    Installed { path: Option<String> },
    /// The project was saved to `path`: write the effects file beside it.
    Saved { path: String },
}

/// Watches the Edit session's `(media pool identity, path, saved_revision)` for opens and saves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BundleTracker {
    /// `Arc::as_ptr(&session.media)` as seen last frame; `None` before the first frame.
    pool: Option<usize>,
    saved_revision: u64,
}

impl BundleTracker {
    /// Record this frame's `(pool, path, saved_revision)` and report what changed. The first call
    /// reports nothing (the session the shell was built with is already in step).
    pub fn observe(&mut self, pool: usize, path: Option<&str>, saved_revision: u64) -> Option<BundleEvent> {
        let first = self.pool.is_none();
        let installed = self.pool.is_some_and(|p| p != pool);
        let saved = !installed && saved_revision != self.saved_revision && path.is_some();
        self.pool = Some(pool);
        self.saved_revision = saved_revision;
        if first {
            return None;
        }
        if installed {
            return Some(BundleEvent::Installed { path: path.map(str::to_string) });
        }
        if saved {
            return path.map(|p| BundleEvent::Saved { path: p.to_string() });
        }
        None
    }
}

/// Load the effects project stored beside `rfilm` into `effects`, if there is one. `Ok(true)`
/// when a file was loaded, `Ok(false)` when there is none (the effects project is reset to an
/// empty one, unless it has unsaved changes, which are kept so a stray Open loses nothing).
pub fn load_effects(rfilm: &str, effects: &mut redrob_effects_engine::Session) -> Result<bool, String> {
    let path = effects_path(rfilm);
    if !Path::new(&path).is_file() {
        if !effects.is_dirty() {
            effects.replace_project(redrob_effects_project::Project::default(), None);
        }
        return Ok(false);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let project = redrob_effects_project::Project::from_json(&text).map_err(|e| format!("{path}: {e}"))?;
    effects.replace_project(project, Some(path));
    effects.mark_saved();
    Ok(true)
}

/// Write the effects project beside `rfilm`. Nothing is written for an effects project with no
/// items when no effects file exists yet (an Edit-only project stays a single file).
pub fn save_effects(rfilm: &str, effects: &mut redrob_effects_engine::Session) -> Result<Option<String>, String> {
    let path = effects_path(rfilm);
    if effects.project.items.is_empty() && !Path::new(&path).is_file() {
        return Ok(None);
    }
    let json = effects.project.to_json();
    std::fs::write(&path, json).map_err(|e| format!("{path}: {e}"))?;
    effects.path = Some(path.clone());
    effects.mark_saved();
    Ok(Some(path))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_effects_file_sits_beside_the_project() {
        assert_eq!(effects_path("/work/cut.rfilm"), "/work/cut.rfilm.effects.json");
    }

    #[test]
    fn opens_and_saves_are_told_apart_by_the_pool_identity() {
        let mut t = BundleTracker::default();
        // the first frame only records
        assert_eq!(t.observe(1, None, 1), None);
        // an edit changes nothing the bundle cares about
        assert_eq!(t.observe(1, None, 1), None);
        // Save As: same pool, new path, saved revision moves
        assert_eq!(t.observe(1, Some("/a.rfilm"), 5), Some(BundleEvent::Saved { path: "/a.rfilm".into() }));
        // Save again later
        assert_eq!(t.observe(1, Some("/a.rfilm"), 5), None);
        assert_eq!(t.observe(1, Some("/a.rfilm"), 9), Some(BundleEvent::Saved { path: "/a.rfilm".into() }));
        // Open: a new pool, whatever the revision did
        assert_eq!(t.observe(2, Some("/b.rfilm"), 1), Some(BundleEvent::Installed { path: Some("/b.rfilm".into()) }));
        // New project: new pool, no path
        assert_eq!(t.observe(3, None, 1), Some(BundleEvent::Installed { path: None }));
        // a saved revision change without a path (recovery marks) is not a save
        assert_eq!(t.observe(3, None, 0), None);
    }

    #[test]
    fn an_empty_effects_project_writes_no_file_and_a_missing_file_loads_as_empty() {
        let dir = std::env::temp_dir().join(format!("redrob-film-bundle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rfilm = dir.join("cut.rfilm").to_string_lossy().to_string();
        let mut effects = redrob_effects_engine::Session::new();
        assert_eq!(save_effects(&rfilm, &mut effects).unwrap(), None);
        assert!(!Path::new(&effects_path(&rfilm)).exists());
        assert!(!load_effects(&rfilm, &mut effects).unwrap());
        // with a composition the file is written and read back
        effects.execute("comp.new", serde_json::json!({"name": "Title", "open": false})).unwrap();
        let written = save_effects(&rfilm, &mut effects).unwrap();
        assert_eq!(written.as_deref(), Some(effects_path(&rfilm).as_str()));
        assert!(!effects.is_dirty());
        let mut other = redrob_effects_engine::Session::new();
        assert!(load_effects(&rfilm, &mut other).unwrap());
        assert_eq!(other.project.items.len(), effects.project.items.len());
        assert!(other.project.items.values().any(|i| i.name == "Title"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
