//! Jog shuttle support (Kdenlive's ShuttlePRO / ShuttleXpress): on Linux the device's event node
//! (Settings ▸ Playback ▸ Jog Shuttle ▸ Device path, or Detect) is read on a background thread and
//! turned into the editor's own commands over the control channel, as the menus run them:
//!
//! * a button runs the command id set for it (Button 1 … Button 15);
//! * the jog wheel steps one frame per click (`playhead.stepForward` / `playhead.stepBack`, on the
//!   focused monitor);
//! * the shuttle ring plays at 1, 1.5, 2, 3, 5.5, 8 or 10x either way and stops in the middle
//!   (`playback.setSpeed`).
//!
//! The thread reads the preferences again every two seconds, so switching the shuttle off or
//! choosing another device takes effect without a restart. Other systems do nothing here.

use std::sync::mpsc::Sender;
use std::time::Duration;

use redrob_film_ui_egui::ControlRequest;
use serde_json::{Value, json};

/// Start the reader thread (Linux only).
pub fn start(tx: Sender<ControlRequest>, ctx: egui::Context) {
    #[cfg(target_os = "linux")]
    {
        let spawned = std::thread::Builder::new().name("jog-shuttle".into()).spawn(move || linux::run(&tx, &ctx));
        if let Err(e) = spawned {
            log::warn!("jog shuttle: could not start its thread: {e}");
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (tx, ctx);
    }
}

/// Run one command on the UI thread and return its result (None when it refused or the editor
/// is gone).
fn call(tx: &Sender<ControlRequest>, ctx: &egui::Context, command: &str, params: Value) -> Option<Value> {
    let (req, reply) = ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    tx.send(req).ok()?;
    ctx.request_repaint();
    let answer = reply.recv_timeout(Duration::from_secs(10)).ok()?;
    if answer.get("ok").and_then(Value::as_bool) == Some(true) { Some(answer.get("result").cloned().unwrap_or(Value::Null)) } else { None }
}

/// The jog shuttle preferences the reader needs.
#[derive(Clone, Debug, Default, PartialEq)]
struct Config {
    enabled: bool,
    device: String,
    /// Command of button n at index n - 1.
    buttons: Vec<String>,
}

fn config(tx: &Sender<ControlRequest>, ctx: &egui::Context) -> Option<Config> {
    let v = call(tx, ctx, "prefs.get", json!({"key": "playback.jogShuttle"}))?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    Some(Config {
        enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(false),
        device: s("device"),
        buttons: (1..=15).map(|n| s(&format!("button{n}"))).collect(),
    })
}

/// Playback speed of a shuttle ring position (-7 ..= 7; 0 = stopped).
fn ring_speed(v: i32) -> f64 {
    const SPEEDS: [f64; 7] = [1.0, 1.5, 2.0, 3.0, 5.5, 8.0, 10.0];
    if v == 0 {
        return 0.0;
    }
    let i = (v.unsigned_abs() as usize).clamp(1, SPEEDS.len()) - 1;
    SPEEDS[i] * f64::from(v.signum())
}

/// Clicks the jog wheel turned between two readings of its position counter (0 ..= 255, wrapping).
fn jog_delta(last: i32, now: i32) -> i32 {
    let mut d = now - last;
    if d > 128 {
        d -= 256;
    } else if d < -128 {
        d += 256;
    }
    d
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io::Read;
    use std::sync::mpsc::Sender;
    use std::time::{Duration, Instant};

    use redrob_film_ui_egui::ControlRequest;
    use serde_json::json;

    use super::{Config, call, config, jog_delta, ring_speed};

    const EV_KEY: u16 = 1;
    const EV_REL: u16 = 2;
    const REL_WHEEL: u16 = 8;
    const REL_DIAL: u16 = 7;
    /// The first button's key code (BTN_0).
    const BTN_0: u16 = 0x100;
    /// `struct input_event`: a timeval (two longs), then type u16, code u16, value i32.
    const EVENT: usize = 2 * std::mem::size_of::<usize>() + 8;

    pub fn run(tx: &Sender<ControlRequest>, ctx: &egui::Context) {
        let mut warned = String::new();
        loop {
            // the editor closed its channel: the app is quitting
            let Some(cfg) = config(tx, ctx) else {
                if tx.send(ControlRequest::new("ui.inspect", json!({})).0).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_secs(2));
                continue;
            };
            if !cfg.enabled || cfg.device.is_empty() {
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            match std::fs::File::open(&cfg.device) {
                Ok(f) => {
                    log::info!("jog shuttle: reading {}", cfg.device);
                    warned.clear();
                    read(tx, ctx, f, &cfg);
                }
                Err(e) => {
                    if warned != cfg.device {
                        log::warn!("jog shuttle: cannot open {}: {e}", cfg.device);
                        warned = cfg.device.clone();
                    }
                    std::thread::sleep(Duration::from_secs(3));
                }
            }
        }
    }

    /// Read events until the device goes away or the preferences change.
    fn read(tx: &Sender<ControlRequest>, ctx: &egui::Context, mut f: std::fs::File, cfg: &Config) {
        let mut buf = [0u8; EVENT];
        let mut jog: Option<i32> = None;
        let mut ring = 0;
        let mut checked = Instant::now();
        loop {
            if f.read_exact(&mut buf).is_err() {
                return;
            }
            if checked.elapsed() > Duration::from_secs(2) {
                checked = Instant::now();
                if config(tx, ctx).as_ref() != Some(cfg) {
                    return;
                }
            }
            let at = EVENT - 8;
            let kind = u16::from_ne_bytes([buf[at], buf[at + 1]]);
            let code = u16::from_ne_bytes([buf[at + 2], buf[at + 3]]);
            let value = i32::from_ne_bytes([buf[at + 4], buf[at + 5], buf[at + 6], buf[at + 7]]);
            match (kind, code) {
                (EV_KEY, c) if c >= BTN_0 && value == 1 => {
                    let n = usize::from(c - BTN_0);
                    if let Some(cmd) = cfg.buttons.get(n).filter(|c| !c.is_empty()) {
                        let _ = call(tx, ctx, cmd, json!({}));
                    }
                }
                (EV_REL, REL_DIAL) => {
                    let d = jog.map_or(0, |last| jog_delta(last, value));
                    jog = Some(value);
                    let cmd = if d > 0 { "playhead.stepForward" } else { "playhead.stepBack" };
                    for _ in 0..d.unsigned_abs().min(32) {
                        let _ = call(tx, ctx, cmd, json!({}));
                    }
                }
                (EV_REL, REL_WHEEL) if value != ring => {
                    ring = value;
                    let _ = call(tx, ctx, "playback.setSpeed", json!({"speed": ring_speed(value)}));
                }
                _ => {}
            }
        }
    }
}
