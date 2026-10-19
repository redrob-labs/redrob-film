//! Panic reporting: a panic anywhere (UI pass, frame workers, background jobs) is written to a
//! crash log with its thread, location and backtrace, and the last one is kept for the in-app
//! error window. Without this a panic on the UI thread closes the app silently and one in a
//! worker leaves a monitor blank with no trace.

use std::sync::Mutex;

/// The most recent panic, as shown to the user: message, `file:line`, thread.
static LAST: Mutex<Option<String>> = Mutex::new(None);
/// Where crash logs go (desktop); `None` = stderr only (web, tests).
static LOG_DIR: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
/// The graphics adapter the window runs on (`set_gpu`), written into every crash report.
static GPU: Mutex<Option<String>> = Mutex::new(None);
/// The operating system and its version, read once at `install` (never inside the hook).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
static OS: Mutex<Option<String>> = Mutex::new(None);

/// The file in the log folder that names the newest crash report until the next start shows it.
const PENDING: &str = "last-crash.txt";

/// Record the graphics adapter in use (name, backend, driver), for crash reports.
pub fn set_gpu(desc: String) {
    *GPU.lock().unwrap_or_else(|e| e.into_inner()) = Some(desc);
}

/// The crash report a previous session left behind and nobody has been shown yet, if any. Taken
/// once: the next call returns `None` until another crash is written.
pub fn take_pending_report() -> Option<std::path::PathBuf> {
    let dir = LOG_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    take_pending_in(&dir)
}

fn take_pending_in(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let marker = dir.join(PENDING);
    let path = std::fs::read_to_string(&marker).ok()?;
    let _ = std::fs::remove_file(&marker);
    let path = std::path::PathBuf::from(path.trim());
    path.is_file().then_some(path)
}

/// Name `report` as the crash report the next start shows.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn mark_pending(dir: &std::path::Path, report: &std::path::Path) {
    let _ = std::fs::write(dir.join(PENDING), report.display().to_string());
}

/// The session ended normally: a panic it caught and showed in the error window is not a crash
/// to report again on the next start.
pub fn clean_exit() {
    if let Some(dir) = LOG_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        let _ = std::fs::remove_file(dir.join(PENDING));
    }
}

/// Operating system, version and CPU architecture, e.g. `macos 12.7.6 (x86_64)`. Best effort: a
/// version that cannot be read cheaply is left out.
#[cfg(not(target_arch = "wasm32"))]
fn os_description() -> String {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let version = if cfg!(target_os = "macos") {
        std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok().and_then(|s| plist_string(&s, "ProductVersion"))
    } else if cfg!(target_os = "linux") {
        std::fs::read_to_string("/etc/os-release").ok().and_then(|s| os_release_name(&s))
    } else {
        None
    };
    match version {
        Some(v) => format!("{os} {v} ({arch})"),
        None => format!("{os} ({arch})"),
    }
}

/// The string value after `<key>KEY</key>` in an XML property list.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn plist_string(plist: &str, key: &str) -> Option<String> {
    let rest = &plist[plist.find(&format!("<key>{key}</key>"))?..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(rest[start..start + end].trim().to_string())
}

/// `PRETTY_NAME` from `/etc/os-release`, unquoted.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn os_release_name(text: &str) -> Option<String> {
    let v = text.lines().find_map(|l| l.strip_prefix("PRETTY_NAME="))?;
    Some(v.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty())
}

/// Install the panic hook (idempotent). Panics are also passed to the previous hook, so they
/// still reach stderr / the browser console.
pub fn install(log_dir: Option<std::path::PathBuf>) {
    *LOG_DIR.lock().unwrap_or_else(|e| e.into_inner()) = log_dir;
    #[cfg(not(target_arch = "wasm32"))]
    {
        *OS.lock().unwrap_or_else(|e| e.into_inner()) = Some(os_description());
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let msg = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
            let summary = format!("{msg} (at {at}, thread {thread})");
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(summary.clone());
            write_log(&summary);
            prev(info);
        }));
    });
}

/// Deliberately panic: the fault-injection hooks (`ui.injectPanic` and the frame-job equivalent)
/// use this to prove the guard above keeps the session alive.
#[allow(clippy::panic)]
pub fn injected_fault(what: &str) -> ! {
    panic!("{what}")
}

/// Write a non-panic fatal error (e.g. the window could not be created) to the crash log.
pub fn record(msg: &str) {
    write_log(msg);
}

/// Take the last panic summary (the error window shows it once).
pub fn take_last() -> Option<String> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The crash log file for today, if logging to disk.
pub fn log_path() -> Option<std::path::PathBuf> {
    let dir = LOG_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    Some(dir.join(format!("crash-{}.log", secs / 86_400)))
}

#[cfg(not(target_arch = "wasm32"))]
fn write_log(summary: &str) {
    use std::io::Write;
    let Some(path) = log_path() else { return };
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let bt = std::backtrace::Backtrace::force_capture();
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let os = OS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH));
    let gpu = GPU.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| "not chosen yet".into());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(
            f,
            "=== {} Redrob Film {} (unix {secs})\nOS: {os}\nGPU: {gpu}\n{summary}\n{bt}\n",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        );
        // the next start says where this report is (`take_pending_report`)
        if let Some(d) = path.parent() {
            mark_pending(d, &path);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn write_log(_summary: &str) {}

#[cfg(test)]
mod tests {
    #[test]
    fn hook_records_and_logs_worker_panics() {
        let dir = std::env::temp_dir().join(format!("fc-crash-{}", std::process::id()));
        super::install(Some(dir.clone()));
        let _ = super::take_last();
        let r = std::thread::Builder::new().name("filmcraft-test-worker".into()).spawn(|| panic!("boom in a worker")).unwrap().join();
        assert!(r.is_err());
        let last = super::take_last().expect("recorded");
        assert!(last.contains("boom in a worker") && last.contains("filmcraft-test-worker"), "{last}");
        let log = std::fs::read_to_string(super::log_path().unwrap()).unwrap();
        assert!(log.contains("boom in a worker"));
        assert!(log.contains("\nOS: ") && log.contains("\nGPU: "), "{log}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The next start is told where the report is, once.
    #[test]
    fn a_pending_report_is_shown_once() {
        let dir = std::env::temp_dir().join(format!("fc-crash-pending-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("crash-1.log");
        assert_eq!(super::take_pending_in(&dir), None);
        super::mark_pending(&dir, &report);
        assert_eq!(super::take_pending_in(&dir), None, "a marker naming a missing file is dropped");
        std::fs::write(&report, "x").unwrap();
        super::mark_pending(&dir, &report);
        assert_eq!(super::take_pending_in(&dir), Some(report));
        assert_eq!(super::take_pending_in(&dir), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn os_version_parsers() {
        let plist = "<dict>\n<key>ProductName</key>\n<string>macOS</string>\n<key>ProductVersion</key>\n<string>12.7.6</string>\n</dict>";
        assert_eq!(super::plist_string(plist, "ProductVersion").as_deref(), Some("12.7.6"));
        assert_eq!(super::plist_string(plist, "Missing"), None);
        assert_eq!(super::os_release_name("NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 22.04.4 LTS\"\n").as_deref(), Some("Ubuntu 22.04.4 LTS"));
        assert_eq!(super::os_release_name("NAME=x\n"), None);
    }
}
