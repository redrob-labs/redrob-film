//! Wiring for the built-in agent: the token-gated control port it drives, the MCP server it
//! starts (`redrob-film mcp`), and adding this app to the user's Redrob Code config.
//!
//! The control port (a free port, a fresh token per launch) is how Redrob Code's MCP server reaches
//! this window; `--control <port>` keeps the upstream tokenless port instead.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The server's name in Redrob Code's config and in `session/new`.
pub const MCP_NAME: &str = "redrob-film";

/// A per-launch secret for the control port (RandomState is seeded from the OS's randomness).
pub fn control_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u8 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u8(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// Where the agent works: the launch directory, or the home directory when launched from a
/// desktop shell (whose working directory is `/`).
pub fn agent_cwd() -> String {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let cwd = std::env::current_dir().ok().filter(|d| d.parent().is_some());
    cwd.or(home).map(|d| d.to_string_lossy().to_string()).unwrap_or_else(|| ".".into())
}

/// `redrob-film mcp [--connect HOST:PORT]`: the MCP server on stdio. With `--connect` it drives the
/// running window (the built-in agent's case, token from the environment); without, an
/// in-process empty project, which is what Redrob Code in a terminal gets from
/// [`add_to_redrob_code`].
pub fn serve_mcp(args: &[String]) -> i32 {
    let server = match args {
        [] => redrob_film_automation::FilmMcp::headless(redrob_film_engine::Session::default()),
        [flag, addr] if flag == "--connect" => match redrob_film_automation::FilmMcp::bridge(addr) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("redrob-film mcp: {e}");
                return 1;
            }
        },
        _ => {
            eprintln!("usage: redrob-film mcp [--connect HOST:PORT]");
            return 2;
        }
    };
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("redrob-film mcp: {e}");
            return 1;
        }
    };
    match rt.block_on(server.serve_stdio()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("redrob-film mcp: {e}");
            1
        }
    }
}

/// Redrob Code's global config file: `$XDG_CONFIG_HOME/redrob/redrob.json`, else
/// `~/.config/redrob/redrob.json` (on every platform, as Redrob Code itself resolves it).
pub fn redrob_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("redrob").join("redrob.json"))
}

/// Add (or refresh) `mcp."redrob-film"` in Redrob Code's config, keeping everything else in the
/// file. A file that is not plain JSON (comments) is left alone and reported, never rewritten.
pub fn add_to_redrob_code(config: &Path, exe: &Path) -> Result<(), String> {
    let mut doc: Value = match std::fs::read_to_string(config) {
        Ok(text) if text.trim().is_empty() => json!({}),
        Ok(text) => {
            serde_json::from_str(&text).map_err(|e| format!("{} is not plain JSON ({e}); add the server with `redrob mcp add` instead", config.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(format!("{}: {e}", config.display())),
    };
    let Some(root) = doc.as_object_mut() else { return Err(format!("{} does not hold a JSON object", config.display())) };
    let mcp = root.entry("mcp").or_insert_with(|| json!({}));
    let Some(mcp) = mcp.as_object_mut() else { return Err(format!("`mcp` in {} is not an object", config.display())) };
    mcp.insert(MCP_NAME.into(), json!({"type": "local", "command": [exe.to_string_lossy(), "mcp"], "enabled": true}));
    if let Some(dir) = config.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    // Written next to the file, then renamed over it: a crash never leaves half a config.
    let tmp = config.with_extension("json.redrob-film-tmp");
    std::fs::write(&tmp, format!("{text}\n")).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, config).map_err(|e| format!("{}: {e}", config.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("redrob-film-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn adds_the_server_and_keeps_the_rest() {
        let d = dir("keep");
        let cfg = d.join("redrob").join("redrob.json");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, r#"{"model":"redrob/auto","mcp":{"other":{"type":"remote","url":"https://x"}}}"#).unwrap();
        add_to_redrob_code(&cfg, Path::new("/opt/Redrob Film/redrob-film")).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(v["model"], "redrob/auto");
        assert_eq!(v["mcp"]["other"]["url"], "https://x");
        assert_eq!(v["mcp"]["redrob-film"], json!({"type": "local", "command": ["/opt/Redrob Film/redrob-film", "mcp"], "enabled": true}));
        // again: the same entry, no duplicate, no leftover temp file
        add_to_redrob_code(&cfg, Path::new("/opt/Redrob Film/redrob-film")).unwrap();
        let again: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(again, v);
        assert_eq!(std::fs::read_dir(cfg.parent().unwrap()).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn creates_a_missing_config() {
        let d = dir("new");
        let cfg = d.join("redrob").join("redrob.json");
        add_to_redrob_code(&cfg, Path::new("/usr/bin/redrob-film")).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(v["mcp"]["redrob-film"]["command"], json!(["/usr/bin/redrob-film", "mcp"]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_commented_config_is_left_alone() {
        let d = dir("jsonc");
        let cfg = d.join("redrob.json");
        std::fs::create_dir_all(&d).unwrap();
        let text = "{\n  // mine\n  \"model\": \"x\"\n}\n";
        std::fs::write(&cfg, text).unwrap();
        let e = add_to_redrob_code(&cfg, Path::new("/usr/bin/redrob-film")).unwrap_err();
        assert!(e.contains("not plain JSON"), "{e}");
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), text);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn tokens_differ_per_call() {
        let (a, b) = (control_token(), control_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
