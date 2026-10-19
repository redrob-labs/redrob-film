//! Redrob Film desktop app.
//!
//! The window is a shell ([`shell`]) hosting two editors as modes: Edit (the timeline editor,
//! `FilmcraftApp`) and Effects (the compositor, `EffectcraftApp`), with a shared AI dock
//! ([`ai`]), hand-offs between them ([`handoff`]) and one project bundle ([`bundle`]).
//!
//! Usage: `filmcraft [--control <port>] [--demo|--empty] [--recover|--no-recover] [--data-dir <dir>]
//! [project.rfilm | media files…]`; `--help` prints the options, `--version` the version. An
//! unknown option is an error (exit code 2), not a file to open.
//!
//! Without a project, `--demo` or `--empty`, Settings ▸ General ▸ At Startup decides: Show Home
//! (the demo project), Open Most Recent, or an empty project.
//!
//! `--control <port>` (or `REDROB_FILM_CONTROL_PORT`) starts a localhost JSON-lines control server;
//! see `redrob_film_ui_egui::control` for the methods. A port that is not a number is an error from
//! either source. Without it the app still opens one, on a free port and gated by a per-launch
//! token, for the built-in agent (Redrob Code) alone; see [`agent_setup`].
//!
//! `redrob-film mcp [--connect HOST:PORT]` runs the MCP server on stdio instead of the window: the
//! built-in agent starts it with `--connect`, Redrob Code in a terminal without (an in-process
//! project).
//!
//! Auto-save and the crash-recovery journal run in every session (data in `--data-dir`, else
//! `REDROB_FILM_DATA_DIR`, else the per-user application data folder). If a previous session died
//! with unsaved changes the app asks to recover them; `--recover` recovers the newest without
//! asking, `--no-recover` starts without asking (the changes stay available via File ▸ Recover
//! Unsaved Changes…).
//!
//! `log` records go to standard error and `<data dir>/Logs/redrob-film.log` (`RUST_LOG` sets the
//! levels); see [`logging`].
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

mod acp;
mod agent_setup;
mod ai;
mod app_bridge;
mod app_nap;
mod appearance;
mod args;
mod audio;
mod audio_in;
mod bundle;
mod control_server;
#[cfg(target_os = "linux")]
mod dev_icon;
mod device;
mod file_filters;
#[cfg(any(target_os = "windows", test))]
mod graphics;
mod handoff;
mod logging;
#[cfg(target_os = "macos")]
mod native_menu;
mod shell;
mod update;
mod window_raise;

use args::{Cli, Launch};
use chrono::TimeZone;
use redrob_film_engine::Session;
use redrob_film_engine::autosave::{AutosaveConfig, default_data_dir};
use redrob_film_ui_egui::FilmcraftApp;
use serde_json::json;

/// Reverse-DNS app id: Linux `.desktop` file, hicolor icon name and Wayland app id.
const APP_ID: &str = "ai.redrob.film";

/// The window, Dock, taskbar and app-switcher icon: the Redrob tile (`assets/app-icon/`, derived
/// from the design system's files by `packaging/brand-assets.sh`). One design on every OS: the
/// design system forbids adding a margin or a second radius to the tile.
fn app_icon() -> egui::IconData {
    #[cfg(target_os = "macos")]
    let png: &[u8] = include_bytes!("../../../assets/app-icon/redrob-film-512.png");
    #[cfg(not(target_os = "macos"))]
    let png: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ai.redrob.film.png");
    eframe::icon_data::from_png_bytes(png).unwrap_or_default()
}

/// Print `text` (stdout, or stderr for an error). A Windows release build has no console
/// (`windows_subsystem = "windows"`), so there it is also shown in a message box: otherwise `--help`
/// would print nothing and a bad option would quit without a word.
fn tell(text: &str, error: bool) {
    if error {
        eprint!("{text}");
    } else {
        print!("{text}");
    }
    #[cfg(all(target_os = "windows", not(debug_assertions)))]
    {
        let level = if error { rfd::MessageLevel::Error } else { rfd::MessageLevel::Info };
        let _ = rfd::MessageDialog::new().set_level(level).set_title("Redrob Film").set_description(text).show();
    }
}

fn main() -> eframe::Result {
    // `redrob-film mcp ...`: the MCP server, no window. Before logging, so stdout stays the protocol's.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("mcp") {
        std::process::exit(agent_setup::serve_mcp(&argv[1..]));
    }
    // First, so the panic hook and every start-up record are logged (`logging`).
    let logger = logging::install();
    // lossy, so a value that is not Unicode is reported like any other bad port instead of ignored
    let env_port = std::env::var_os("REDROB_FILM_CONTROL_PORT")
        .or_else(|| std::env::var_os("FILMCRAFT_CONTROL_PORT")) // legacy: the pre-rename variable still works
        .map(|p| p.to_string_lossy().into_owned());
    let Launch { control_port, files, demo, startup_flag, recover, data_dir } = match args::parse(env_port, std::env::args().skip(1)) {
        Ok(Cli::Run(l)) => l,
        Ok(Cli::Help) => {
            tell(args::USAGE, false);
            return Ok(());
        }
        Ok(Cli::Version) => {
            tell(&format!("redrob-film {}\n", env!("CARGO_PKG_VERSION")), false);
            return Ok(());
        }
        Err(e) => {
            tell(&format!("redrob-film: {e}\n\n{}", args::USAGE), true);
            std::process::exit(2);
        }
    };
    let log_dir = data_dir.clone().or_else(default_data_dir).map(|d| d.join(logging::LOG_DIR));
    log::info!("Redrob Film {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    // The log file lives next to the crash logs; opened after the arguments, so `--help`,
    // `--version` and usage errors leave no file behind. Records logged until now go in first.
    if let Some(logger) = logger {
        match log_dir.as_deref().map(|d| logger.attach_dir(d)) {
            Some(Ok(path)) => log::info!("log file {}", path.display()),
            Some(Err(e)) => log::warn!("no log file: {e}"),
            None => {
                logger.stderr_only();
                log::warn!("no log file: no data directory (set REDROB_FILM_DATA_DIR or --data-dir)");
            }
        }
    }
    app_nap::disable();
    #[cfg(target_os = "linux")]
    dev_icon::ensure_dev_desktop_entry();
    // Panics anywhere go to <data dir>/Logs/crash-<day>.log with a backtrace; the UI pass and
    // frame workers catch them and keep running (see redrob_film_ui_egui::crash).
    redrob_film_ui_egui::crash::install(log_dir);
    // OS hardware video decoders (VideoToolbox on macOS) in front of our own; Settings ▸ Playback ▸
    // Hardware decoding switches them off. Unsupported streams and failures use our decoders.
    register_hardware_decoders();
    register_gpu_frame_renderer();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Redrob Film")
            .with_inner_size([1600.0, 980.0])
            .with_min_inner_size([900.0, 560.0])
            .with_drag_and_drop(true)
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            // Started for an agent (`--control`): open without taking the user's keyboard focus.
            .with_active(control_port.is_none())
            .with_icon(app_icon())
            // Wayland matches the window to `packaging/linux/ai.redrob.film.desktop`.
            .with_app_id(APP_ID),
        event_loop_builder: agent_event_loop(control_port.is_some()),
        ..Default::default()
    };
    // Before eframe creates the wgpu instance: leave OpenGL out on Windows (see graphics.rs).
    #[cfg(target_os = "windows")]
    let options = {
        let mut options = options;
        graphics::configure(&mut options, eframe::wgpu::Backends::from_env());
        options
    };
    let started = eframe::run_native(
        "Redrob Film",
        options,
        Box::new(move |cc| {
            let mut session = Session::default();
            if let Some(dir) = data_dir.clone().or_else(default_data_dir) {
                let mut cfg = AutosaveConfig::new(dir);
                cfg.local_offset = local_offset;
                if let Err(e) = session.start_autosave(cfg) {
                    log::warn!("auto-save and crash recovery unavailable: {e}");
                }
            }
            // voice-over recording reads the microphone through cpal
            session.voiceover.input = Some(Box::new(audio_in::CpalIn::new(&session.prefs.audio_hardware.device_class)));
            let project = files.iter().find(|f| f.ends_with(".rfilm") || f.ends_with(".fcproj")).cloned(); // legacy: .fcproj still opens
            let project_given = project.clone();
            if let Some(p) = project {
                if let Err(e) = session.execute("file.open", json!({"path": p})) {
                    log::error!("{e}");
                }
            } else if !startup_flag && session.prefs.general.at_startup == "openMostRecent" {
                // Settings ▸ General ▸ At Startup ▸ Open Most Recent
                let recent = session.prefs.general.recent_projects.iter().find(|p| std::path::Path::new(p).exists()).cloned();
                match recent {
                    Some(p) => {
                        if let Err(e) = session.execute("file.open", json!({"path": p})) {
                            log::error!("{e}");
                        }
                    }
                    None => {
                        let _ = session.execute("file.openDemoProject", json!({}));
                    }
                }
            } else if !startup_flag && matches!(session.prefs.general.at_startup.as_str(), "emptyProject" | "showWelcome") {
            } else if demo {
                let _ = session.execute("file.openDemoProject", json!({}));
            }
            let media: Vec<String> = files.iter().filter(|f| !(f.ends_with(".rfilm") || f.ends_with(".fcproj"))).cloned().collect(); // legacy: .fcproj
            if !media.is_empty() {
                let _ = session.execute("file.import", json!({"paths": media}));
            }
            if recover == Some(true) && !session.recovery_candidates().is_empty() {
                let id = session.recovery_candidates()[0].id.clone();
                match session.execute("file.recover", json!({"id": id})) {
                    Ok(r) => log::info!("recovered {r}"),
                    Err(e) => log::error!("recovery failed: {e}"),
                }
            }
            let mut app = FilmcraftApp::new(session);
            if recover == Some(false) {
                app.dialog = None;
            }
            // Settings ▸ General ▸ At Startup ▸ Show Welcome Screen
            app.ui.extras.welcome = project_given.is_none() && !startup_flag && app.session.prefs.general.at_startup == "showWelcome";
            app.integrated_titlebar = cfg!(target_os = "macos");
            if let Some(rs) = cc.wgpu_render_state.clone()
                && std::env::var_os("REDROB_FILM_CPU_COMPOSITE").is_none()
                && std::env::var_os("FILMCRAFT_CPU_COMPOSITE").is_none()
            // legacy: the pre-rename variable still works
            {
                app.set_wgpu(rs);
            }
            // Keep device selection available even when the default device is unavailable.
            // Settings ▸ Audio Hardware is applied on the first frame (`apply_prefs`).
            app.audio = Some(Box::new(audio::CpalOut::new()));
            app.hooks.pick_files = Some(Box::new(|exts: &[&str]| {
                rfd::FileDialog::new()
                    .add_filter(redrob_film_ui_egui::i18n::t("Media"), &file_filters::extensions(exts))
                    .pick_files()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect()
            }));
            // Link Media ▸ Locate…, Attach Proxies, Reconnect Full Resolution: one path, not imported.
            app.hooks.pick_file_for_relink = Some(Box::new(|exts: &[&str], _hint| {
                rfd::FileDialog::new()
                    .add_filter(redrob_film_ui_egui::i18n::t("Media"), &file_filters::extensions(exts))
                    .pick_file()
                    .map(|p| p.to_string_lossy().to_string())
            }));
            app.hooks.pick_save = Some(Box::new(|name: &str| {
                rfd::FileDialog::new()
                    .add_filter(redrob_film_ui_egui::i18n::t("Redrob Film Project"), &file_filters::extensions(&["rfilm"]))
                    .set_file_name(name)
                    .save_file()
                    .map(|p| p.to_string_lossy().to_string())
            }));
            app.hooks.pick_save_as = Some(Box::new(|filter: &str, exts: &[&str], name: &str| {
                rfd::FileDialog::new()
                    .add_filter(redrob_film_ui_egui::i18n::t(filter), &file_filters::extensions(exts))
                    .set_file_name(name)
                    .save_file()
                    .map(|p| p.to_string_lossy().to_string())
            }));
            app.hooks.pick_folder = Some(Box::new(|| rfd::FileDialog::new().pick_folder().map(|p| p.to_string_lossy().to_string())));
            app.hooks.pick_folder_at =
                Some(Box::new(|dir: &str| rfd::FileDialog::new().set_directory(dir).pick_folder().map(|p| p.to_string_lossy().into_owned())));
            app.hooks.open_path = Some(Box::new(open_path));
            // Help menu, About and Discord links (#642): eframe is built without its `links`
            // feature, so `ctx.open_url` does nothing on the desktop. Detached, so a launcher
            // that waits for the browser never freezes the UI.
            app.hooks.open_url = Some(Box::new(|url: &str| open::that_detached(url).map_err(|e| e.to_string())));
            // Redrob workspace (device flow against console.redrob.ai). Editing never needs it;
            // the AI features ask for it through `account::require` when they start.
            let account = device::DeviceAccount::new(
                device::config_dir().map(|d| d.join("account.json")),
                device::shared_account_path(),
                std::sync::Arc::new(device::Https),
                device::console_base(),
                Some(cc.egui_ctx.clone()),
            );
            let keys = account.key_source();
            app.account = Some(Box::new(account));
            // Redrob Code in a terminal can use this app's tools too (Agent panel, Help menu).
            app.agent_services.add_to_redrob_code = Some(Box::new(|| {
                let config = agent_setup::redrob_config_path().ok_or("no home folder to find Redrob Code's config in")?;
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                agent_setup::add_to_redrob_code(&config, &exe)?;
                Ok(config.display().to_string())
            }));
            app.hooks.clipboard_media = Some(Box::new(clipboard_media));
            // Settings ▸ General ▸ Interface Language ▸ System Language (#218).
            app.hooks.cursor_screen_position = Some(Box::new(redrob_film_platform::cursor::cursor_screen_position));
            app.hooks.system_languages = Some(Box::new(|| sys_locale::get_locales().collect()));
            // Settings ▸ Appearance ▸ Appearance Mode ▸ Sync with system on Linux desktops whose
            // compositor reports no theme to winit (no polling: see appearance.rs).
            app.hooks.system_theme = appearance::service();
            app.hooks.raise_without_focus = Some(Box::new(|| {
                window_raise::raise_without_focus();
            }));
            app.hooks.pick_open_file = Some(Box::new(|filter: &str, exts: &[&str]| {
                rfd::FileDialog::new()
                    .add_filter(redrob_film_ui_egui::i18n::t(filter), &file_filters::extensions(exts))
                    .pick_file()
                    .map(|p| p.to_string_lossy().to_string())
            }));
            app.hooks.pick_open_project = Some(Box::new(|| {
                rfd::FileDialog::new()
                    .add_filter(redrob_film_ui_egui::i18n::t("Redrob Film Project"), &file_filters::extensions(&["rfilm", "fcproj"])) // legacy: .fcproj still opens
                    .pick_file()
                    .map(|p| p.to_string_lossy().to_string())
            }));
            #[cfg(target_os = "macos")]
            {
                let (rx, update) = native_menu::install(&app, cc.egui_ctx.clone());
                app.command_inbox = Some(rx);
                app.hooks.shortcuts_changed = Some(update);
                app.ui.show_menu_bar = false;
            }
            // `--control PORT` keeps the upstream tokenless port; otherwise a free port and a fresh
            // token that only the built-in agent's MCP server is handed.
            let token = control_port.is_none().then(agent_setup::control_token);
            let (rx, bound) = control_server::start(control_port.unwrap_or(0), cc.egui_ctx.clone(), token.clone());
            // The Redrob Code app bridge (app_bridge.rs): the shared daemon's agent drives this
            // window too, through the same channel; the bridge is optional (no daemon, no bridge).
            let (rx, bridge_tx) = app_bridge::relay(rx);
            let bridge = app_bridge::start(bridge_tx, cc.egui_ctx.clone());
            // The shell sits on the channel first (its own `mode.*` commands, the catalogue, a
            // `ui.*` refusal while Effects is on screen) and forwards the rest to the Edit editor.
            let (edit_tx, edit_rx) = std::sync::mpsc::channel();
            app = app.with_control(edit_rx);
            // The built-in agent: Redrob Code over ACP, started by the Agent panel's first message
            // (after `account::require`), with the connected workspace's key.
            if let Some(port) = bound {
                let link = acp::McpLink { addr: format!("127.0.0.1:{port}"), token };
                let ctx = cc.egui_ctx.clone();
                app.agent_services.factory = Some(Box::new(move || {
                    let bin = acp::find_binary().ok_or_else(|| acp::INSTALL_HINT.to_string())?;
                    // The connected workspace's key, as the variable Redrob Code reads for Redrob.
                    let env: Vec<(&str, String)> = keys.current().map(|(k, _)| vec![("REDROB_API_KEY", k.api_key)]).unwrap_or_default();
                    let host = acp::AcpHost::start(&bin, &agent_setup::agent_cwd(), link.clone(), &env, ctx.clone())?;
                    Ok(Box::new(host) as Box<dyn redrob_film_ui_egui::agent::AgentHost>)
                }));
            }
            // One window, two modes: the Edit editor built above, Effects on its first switch.
            Ok(Box::new(shell::ShellApp::new(app, Some(rx), edit_tx).with_app_bridge(bridge)))
        }),
    );
    // The window could not be created (no usable GPU / graphics driver, no display): say so
    // instead of exiting silently, and keep the reason in the crash log.
    if let Err(e) = &started {
        let msg = format!("Redrob Film could not start its window: {e}\n\nUpdating the graphics driver usually fixes this.");
        log::error!("{msg}");
        redrob_film_ui_egui::crash::record(&msg);
        let _ = rfd::MessageDialog::new().set_title("Redrob Film").set_description(&msg).set_level(rfd::MessageLevel::Error).show();
    }
    started
}

/// Files copied in the file manager, else an image, on the system clipboard (Paste, #611).
fn clipboard_media() -> Option<redrob_film_ui_egui::ClipboardMedia> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    if let Ok(files) = clipboard.get().file_list()
        && !files.is_empty()
    {
        return Some(redrob_film_ui_egui::ClipboardMedia::Files(files.iter().map(|p| p.to_string_lossy().into_owned()).collect()));
    }
    let image = clipboard.get_image().ok()?;
    Some(redrob_film_ui_egui::ClipboardMedia::Image { width: image.width, height: image.height, rgba: image.bytes.into_owned() })
}

/// Open a file in its default application, or reveal it in the file manager (Edit Original,
/// Reveal Log Files).
fn open_path(path: &str, reveal: bool) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        if reveal {
            c.arg("-R");
        }
        c.arg(path);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        if reveal {
            c.arg(format!("/select,{path}"));
        } else {
            c.arg(path);
        }
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        let p = std::path::Path::new(path);
        c.arg(if reveal { p.parent().unwrap_or(p) } else { p });
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| format!("can't open {path}: {e}"))
}

/// The GPU export frame renderer (redrob-film-gpu's off-screen compositor behind redrob-film-export's
/// frame-renderer hook): exports with GPU rendering Auto composite on the GPU and fall back to the
/// CPU reference renderer wherever it cannot.
struct GpuFrameRenderer(redrob_film_gpu::ExportRenderer);

impl redrob_film_export::FrameRenderer for GpuFrameRenderer {
    fn render(
        &mut self,
        project: &redrob_film_project::Project,
        seq: redrob_film_project::ItemId,
        t: redrob_film_time::Tick,
        opts: redrob_film_render::RenderOptions,
        sources: &dyn redrob_film_render::SourceProvider,
    ) -> Option<redrob_film_render::Image> {
        self.0.render(project, seq, t, opts, sources)
    }
}

fn register_gpu_frame_renderer() {
    redrob_film_export::register_frame_renderer(|| {
        redrob_film_gpu::ExportRenderer::new().map(|r| Box::new(GpuFrameRenderer(r)) as Box<dyn redrob_film_export::FrameRenderer>)
    });
}

/// Put the OS hardware video decoders in front of our own. Registered in a statement of its own:
/// a log macro does not evaluate its arguments while no logger takes its level, which left the
/// hardware decoders out of every run.
fn register_hardware_decoders() -> redrob_film_platform::Availability {
    let hardware = redrob_film_platform::register();
    log::info!("hardware decoding: {hardware:?}");
    hardware
}

/// Local UTC offset at a unix time (auto-save file names and recovery times use local time).
fn local_offset(unix: i64) -> i32 {
    chrono::Local.timestamp_opt(unix, 0).single().map(|d| d.offset().local_minus_utc()).unwrap_or(0)
}

/// Driven by an agent (`--control`): don't activate the app on launch, so the user's keyboard
/// focus stays where it is (macOS; winit activates ignoring other apps by default).
fn agent_event_loop(agent: bool) -> Option<eframe::EventLoopBuilderHook> {
    #[cfg(target_os = "macos")]
    if agent {
        return Some(Box::new(|b| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            b.with_activate_ignoring_other_apps(false);
        }));
    }
    let _ = agent;
    None
}

#[cfg(test)]
mod tests {
    /// Start-up registers the hardware decoders whether or not anything is logged (no logger is
    /// installed here, as in a normal run).
    #[test]
    fn startup_registers_the_hardware_decoders_without_a_logger() {
        assert!(!log::log_enabled!(log::Level::Info));
        let hardware = super::register_hardware_decoders();
        // always on macOS and Windows; on Linux when a VA-API driver or NVIDIA's driver (NVDEC) is there
        let expected = cfg!(any(target_os = "macos", target_os = "windows"))
            || (cfg!(target_os = "linux") && matches!(hardware, redrob_film_platform::Availability::Available(_)));
        assert_eq!(redrob_film_platform::registered(), expected, "{hardware:?}");
    }
}
