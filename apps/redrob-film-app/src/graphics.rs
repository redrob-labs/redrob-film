//! Graphics start-up, applied before eframe creates the wgpu instance: the backends the instance
//! may use, the adapter the window takes, the device limits it asks for, and safe mode.
//!
//! eframe's own choice asks wgpu for one high-performance adapter and the full WebGPU default
//! limits. On a machine where that adapter does not exist for the window's surface, or reports
//! lower limits (older Intel and AMD GPUs, OpenGL-only drivers, VMs), the window was never
//! created and the app quit at launch. Here every adapter that can present to the window is
//! considered, the best is taken, the limits asked for are clamped to what it reports, and the
//! reason for each choice is logged.
//!
//! Safe mode (`--safe-mode`, `REDROB_FILM_SAFE_MODE=1`, or automatically after a start that died
//! before its window opened) prefers a software adapter (WARP on Windows), uses DirectX 12 alone on
//! Windows (Vulkan and OpenGL drivers have crashed inside `igvk64.dll` and `atio6axx.dll` before
//! any Rust code could react), and the app composites, exports and decodes on the CPU.

use std::sync::Arc;

use eframe::egui_wgpu::{NativeAdapterSelectorMethod, WgpuSetup};
use eframe::{NativeOptions, wgpu};
use wgpu::{Backends, DeviceType};

/// The file in the log folder that exists while a start has not reached its window yet.
const STARTING: &str = "starting";

/// Why this start runs in safe mode.
#[derive(Clone, Debug, PartialEq)]
pub struct SafeMode {
    /// What turned it on, for the log.
    pub reason: String,
    /// Turned on because the previous start died before its window opened (not asked for).
    pub automatic: bool,
}

/// Safe mode for this start, or `None`: `--safe-mode`, then `REDROB_FILM_SAFE_MODE` (any value but
/// empty, `0`, `false`, `no`, `off`), then a previous start that never reached its window.
pub fn safe_mode(flag: bool, env: Option<&str>, previous_start_failed: bool) -> Option<SafeMode> {
    if flag {
        return Some(SafeMode { reason: "--safe-mode".into(), automatic: false });
    }
    if let Some(v) = env.map(str::trim).filter(|v| !v.is_empty() && !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off")) {
        return Some(SafeMode { reason: format!("REDROB_FILM_SAFE_MODE={v}"), automatic: false });
    }
    previous_start_failed.then(|| SafeMode { reason: "the previous start stopped before its window opened".into(), automatic: true })
}

/// Mark a start in `dir` (the log folder). Returns whether the previous start left its mark, i.e.
/// died between this call and [`window_up`] (a driver crash while the window's GPU device was
/// being created, or no window at all).
pub fn begin_start(dir: &std::path::Path) -> bool {
    let mark = dir.join(STARTING);
    let previous = mark.exists();
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&mark, std::process::id().to_string());
    previous
}

/// The window and its GPU device exist: this start did not fail at the graphics step.
pub fn window_up(dir: &std::path::Path) {
    let _ = std::fs::remove_file(dir.join(STARTING));
}

/// Restrict the instance's backends and install the adapter and device choice. `backend_override`
/// is normally `Backends::from_env()`, i.e. `WGPU_BACKEND`, and always wins.
pub fn configure(options: &mut NativeOptions, backend_override: Option<Backends>, safe: bool) {
    if let WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
        if let Some(backends) = backends(cfg!(target_os = "windows"), safe, backend_override) {
            create.instance_descriptor.backends = backends;
        }
        let selector: NativeAdapterSelectorMethod = if safe { Arc::new(select_safe) } else { Arc::new(select_normal) };
        create.native_adapter_selector = Some(selector);
        create.device_descriptor = Arc::new(device_descriptor);
    }
}

/// The instance's backends, or `None` to keep eframe's.
///
/// Windows: eframe's default includes GL. Creating that backend can crash inside AMD's
/// atio6axx.dll before adapter selection or any Rust error handling runs, so the window never
/// appears. Choosing an adapter afterwards is too late: only leaving GL out of the instance avoids
/// it. Keep Vulkan and DX12 both: forcing DX12 alone sends shaders through FXC (which rejects
/// fx.wgsl, silently falling back to the CPU) and leaves machines without DX12 with no backend at
/// all. Safe mode does use DX12 alone: it composites on the CPU anyway, and DX12 always has WARP.
fn backends(windows: bool, safe: bool, backend_override: Option<Backends>) -> Option<Backends> {
    match (backend_override, windows, safe) {
        (Some(b), _, _) => Some(b),
        (None, true, true) => Some(Backends::DX12),
        (None, true, false) => Some(Backends::PRIMARY),
        (None, false, _) => None,
    }
}

fn select_normal(adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>) -> Result<wgpu::Adapter, String> {
    select_adapter(adapters, surface, false)
}

fn select_safe(adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>) -> Result<wgpu::Adapter, String> {
    select_adapter(adapters, surface, true)
}

/// One adapter in the log and in crash reports: name, kind, backend and driver.
fn describe(info: &wgpu::AdapterInfo) -> String {
    let driver = [info.driver.as_str(), info.driver_info.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
    format!("{} ({:?}, {:?}{})", info.name, info.device_type, info.backend, if driver.is_empty() { String::new() } else { format!(", {driver}") })
}

/// The adapter the window uses: among those that can present to its surface, the best by
/// [`order`].
fn select_adapter(adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>, safe: bool) -> Result<wgpu::Adapter, String> {
    let infos: Vec<wgpu::AdapterInfo> = adapters.iter().map(wgpu::Adapter::get_info).collect();
    let presents: Vec<bool> = adapters.iter().map(|a| surface.is_none_or(|s| a.is_surface_supported(s))).collect();
    for (info, ok) in infos.iter().zip(&presents) {
        log::info!("graphics adapter: {}{}", describe(info), if *ok { "" } else { " — cannot present to the window" });
    }
    let candidates: Vec<Candidate> = infos
        .iter()
        .zip(&presents)
        .map(|(i, ok)| Candidate {
            presents: *ok,
            device_type: i.device_type,
            software: redrob_film_ui_egui::software_rasterizer(i).is_some(),
            gl: i.backend == wgpu::Backend::Gl,
        })
        .collect();
    let Some(&best) = order(&candidates, safe).first() else {
        let msg = if adapters.is_empty() {
            "no graphics adapter found (update the graphics driver)".to_string()
        } else {
            format!("none of the {} graphics adapters can draw to the window (update the graphics driver)", adapters.len())
        };
        log::error!("{msg}");
        return Err(msg);
    };
    let chosen = describe(&infos[best]);
    log::info!("graphics adapter chosen{}: {chosen}", if safe { " (safe mode)" } else { "" });
    redrob_film_ui_egui::crash::set_gpu(chosen);
    Ok(adapters[best].clone())
}

/// What the choice looks at, per adapter.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    presents: bool,
    device_type: DeviceType,
    /// A software rasterizer (WARP, llvmpipe, SwiftShader), whatever type it reports.
    software: bool,
    gl: bool,
}

/// Indices of the adapters that can present, best first. Normally: discrete, integrated, virtual,
/// other, software (eframe's high-performance preference). Safe mode: software first, then the
/// same order. Within a kind, a non-OpenGL backend first; then enumeration order.
fn order(candidates: &[Candidate], safe: bool) -> Vec<usize> {
    let rank = |c: &Candidate| -> u8 {
        let kind = match c.device_type {
            _ if c.software => 4,
            DeviceType::DiscreteGpu => 0,
            DeviceType::IntegratedGpu => 1,
            DeviceType::VirtualGpu => 2,
            DeviceType::Other => 3,
            DeviceType::Cpu => 4,
        };
        if safe && kind == 4 { 0 } else { kind + u8::from(safe) }
    };
    let mut out: Vec<usize> = (0..candidates.len()).filter(|&i| candidates[i].presents).collect();
    out.sort_by_key(|&i| (rank(&candidates[i]), candidates[i].gl, i));
    out
}

/// The device the window asks for: eframe's limits (WebGPU defaults, WebGL2 ones on OpenGL, 8192
/// pixel textures for 4K+ displays), lowered to what the adapter reports where it reports less.
/// Asking for more than the adapter has fails `request_device`, and that failure ended the start.
fn device_descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
    let supported = adapter.limits();
    let required_limits = clamp_limits(adapter.get_info().backend == wgpu::Backend::Gl, &supported);
    wgpu::DeviceDescriptor { label: Some("egui wgpu device"), required_limits, ..Default::default() }
}

fn clamp_limits(gl: bool, supported: &wgpu::Limits) -> wgpu::Limits {
    let base = if gl { wgpu::Limits::downlevel_webgl2_defaults() } else { wgpu::Limits::default() };
    let wanted = wgpu::Limits { max_texture_dimension_2d: 8192, ..base };
    if wanted.check_limits(supported) {
        return wanted;
    }
    let mut short = Vec::new();
    wanted.check_limits_with_fail_fn(supported, false, |name, want, have| short.push(format!("{name} {have} < {want}")));
    log::warn!("graphics adapter reports lower limits than the default; asking for what it has ({})", short.join(", "));
    wanted.or_worse_values_from(supported)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_backends(windows: bool, safe: bool, backend_override: Option<Backends>) -> Backends {
        let WgpuSetup::CreateNew(create) = NativeOptions::default().wgpu_options.wgpu_setup else {
            panic!("default native options must create a wgpu instance");
        };
        backends(windows, safe, backend_override).unwrap_or(create.instance_descriptor.backends)
    }

    #[test]
    fn windows_default_leaves_out_gl() {
        // eframe's own default (PRIMARY | GL) would initialize OpenGL as well.
        let WgpuSetup::CreateNew(eframe_default) = NativeOptions::default().wgpu_options.wgpu_setup else {
            panic!("default native options must create a wgpu instance");
        };
        if Backends::from_env().is_none() {
            assert!(eframe_default.instance_descriptor.backends.contains(Backends::GL));
        }
        let configured = configured_backends(true, false, None);
        assert_eq!(configured, Backends::PRIMARY);
        assert!(!configured.contains(Backends::GL));
        assert!(configured.contains(Backends::DX12) && configured.contains(Backends::VULKAN));
        // safe mode: DirectX 12 alone (WARP is always there; no Vulkan / GL driver is loaded)
        assert_eq!(configured_backends(true, true, None), Backends::DX12);
        // other systems keep eframe's choice
        assert_eq!(backends(false, false, None), None);
        assert_eq!(backends(false, true, None), None);
    }

    #[test]
    fn explicit_backend_override_is_preserved() {
        for backends in [Backends::VULKAN, Backends::GL, Backends::DX12, Backends::VULKAN | Backends::DX12, Backends::empty()] {
            for (windows, safe) in [(true, false), (true, true), (false, false), (false, true)] {
                assert_eq!(configured_backends(windows, safe, Some(backends)), backends);
            }
        }
    }

    #[test]
    fn configure_installs_the_adapter_and_device_choice() {
        let mut options = NativeOptions::default();
        configure(&mut options, None, false);
        let WgpuSetup::CreateNew(create) = options.wgpu_options.wgpu_setup else {
            panic!("default native options must create a wgpu instance");
        };
        assert!(create.native_adapter_selector.is_some());
    }

    fn c(device_type: DeviceType, software: bool, gl: bool) -> Candidate {
        Candidate { presents: true, device_type, software, gl }
    }

    #[test]
    fn adapters_are_ordered_hardware_first_and_software_first_in_safe_mode() {
        let list = [
            c(DeviceType::Cpu, true, false),            // 0: WARP / llvmpipe
            c(DeviceType::IntegratedGpu, false, false), // 1
            c(DeviceType::DiscreteGpu, false, true),    // 2: discrete, OpenGL
            c(DeviceType::DiscreteGpu, false, false),   // 3: discrete, Vulkan / DX12 / Metal
            c(DeviceType::Other, true, false),          // 4: llvmpipe reporting "Other"
            Candidate { presents: false, ..c(DeviceType::DiscreteGpu, false, false) }, // 5: cannot draw to the window
        ];
        assert_eq!(order(&list, false), [3, 2, 1, 0, 4]);
        assert_eq!(order(&list, true), [0, 4, 3, 2, 1]);
        assert!(order(&[], false).is_empty());
        assert!(order(&list[5..], false).is_empty(), "an adapter that cannot present is never chosen");
    }

    #[test]
    fn limits_are_clamped_to_what_the_adapter_reports() {
        // a capable adapter gets exactly eframe's request
        let big = wgpu::Limits { max_texture_dimension_2d: 16384, ..wgpu::Limits::default() };
        assert_eq!(clamp_limits(false, &big), wgpu::Limits { max_texture_dimension_2d: 8192, ..wgpu::Limits::default() });
        // an older GPU with downlevel limits gets those instead of a failed request_device
        let small = wgpu::Limits::downlevel_defaults();
        let got = clamp_limits(false, &small);
        assert!(got.check_limits(&small), "{got:?}");
        assert_eq!(got.max_texture_dimension_2d, small.max_texture_dimension_2d.min(8192));
    }

    #[test]
    fn safe_mode_sources() {
        assert_eq!(safe_mode(false, None, false), None);
        assert!(safe_mode(true, None, false).is_some_and(|s| !s.automatic && s.reason == "--safe-mode"));
        for on in ["1", "yes", "true"] {
            assert!(safe_mode(false, Some(on), false).is_some_and(|s| !s.automatic), "{on}");
        }
        for off in ["", "0", "false", "No", "off", " "] {
            assert_eq!(safe_mode(false, Some(off), false), None, "{off:?}");
        }
        assert!(safe_mode(false, None, true).is_some_and(|s| s.automatic));
        // asked for explicitly: not reported as automatic even after a failed start
        assert!(safe_mode(true, None, true).is_some_and(|s| !s.automatic));
    }

    #[test]
    fn a_start_that_never_reaches_its_window_is_noticed_once() {
        let dir = std::env::temp_dir().join(format!("rf-start-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!begin_start(&dir), "first start");
        // died before the window: the next start sees the mark
        assert!(begin_start(&dir));
        window_up(&dir);
        assert!(!begin_start(&dir), "the window came up last time");
        window_up(&dir);
        let _ = std::fs::remove_dir_all(dir);
    }
}
