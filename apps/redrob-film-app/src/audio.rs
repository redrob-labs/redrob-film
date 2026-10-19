//! cpal audio output: the playback master clock. Settings ▸ Audio Hardware picks the host
//! ("Device Class"), the output device, the I/O buffer size and the sample rate.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use std::sync::{Arc, Mutex, PoisonError};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use redrob_film_engine::settings::AudioHardwarePrefs;
use redrob_film_ui_egui::{AudioDevices, AudioOut};

pub struct CpalOut {
    stream: Option<cpal::Stream>,
    played: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
    rate: u32,
    channels: u16,
    hw: AudioHardwarePrefs,
    /// Sequence sample rate for "Attempt to force hardware to document sample rate".
    document_rate: Option<u32>,
    /// Status-bar note: the host's default device could not be opened and a fallback device is
    /// in use, or no output device was found (playing without sound).
    note: Option<Note>,
    /// The fallback picked when the default device could not be opened, as (host, device) names,
    /// so later lookups (`devices`, `configure`, `start`) reuse it instead of probing every PCM.
    fallback: Mutex<Option<(String, String)>>,
    /// The last stream error, written by cpal's error callback and shown by `note` (a desktop
    /// launch has no terminal; on Windows `windows_subsystem = "windows"` discards stderr).
    error: Arc<Mutex<Option<String>>>,
    /// A stream kept open while the editor is idle (Linux, Windows): a sound server suspends an
    /// output nothing plays to after a few seconds, and waking it delayed the first picture of
    /// every play by ~0.9 s. It plays silence until `start` installs a callback in its slot, so
    /// play and scrub start on a stream that is already running: opening one on WASAPI (device
    /// lookup, `supported_output_configs`, `IAudioClient::Initialize`) blocked the UI thread for
    /// up to seconds on the first scrub move and on every Play.
    keepalive: Option<Idle>,
    /// The current play / scrub runs on `keepalive` (not on `stream`).
    routed: bool,
    /// The stream configuration last chosen per device name ([`Self::config`]): on WASAPI asking
    /// for the supported configurations probes every format, which is slow. Cleared by
    /// `configure`.
    configs: Mutex<Option<(String, cpal::SupportedStreamConfig)>>,
}

/// The callback a stream plays from.
type Fill = Box<dyn FnMut(&mut [f32], usize) + Send>;

/// The idle stream ([`CpalOut::keepalive`]) and the callback it plays (`None` = silence).
struct Idle {
    _stream: cpal::Stream,
    slot: Arc<Mutex<Option<Fill>>>,
    failed: Arc<AtomicBool>,
}

/// The idle stream's callback: the installed callback, counting its frames into `played`, or
/// silence (not counted, so the playback clock starts at 0 when a callback is installed).
fn routed_fill(slot: Arc<Mutex<Option<Fill>>>, played: Arc<AtomicU64>) -> Fill {
    Box::new(move |buf: &mut [f32], ch: usize| {
        let mut g = slot.lock().unwrap_or_else(PoisonError::into_inner);
        match g.as_mut() {
            Some(fill) => {
                fill(buf, ch);
                played.fetch_add((buf.len() / ch.max(1)) as u64, Ordering::SeqCst);
            }
            None => buf.fill(0.0),
        }
    })
}

/// A hardware note for the status bar, translated when it is shown.
#[derive(Clone, Debug, PartialEq)]
enum Note {
    /// The default output device cannot be opened; this named device is used instead.
    Fallback(String),
    /// No output device at all: playback runs without sound.
    NoDevice,
    /// The output stream could not be opened, or stopped with an error: playback runs without sound.
    Error(String),
}

impl Note {
    fn text(&self) -> String {
        use redrob_film_ui_egui::i18n::{fmt, t};
        match self {
            Note::Fallback(device) => fmt(t("Audio hardware: the default output device cannot be opened; using '{device}'"), &[("device", device)]),
            Note::NoDevice => t("No audio output device found: playing without sound (check Settings ▸ Audio Hardware)").to_string(),
            Note::Error(e) => fmt(t("Audio output failed: {error} (playing without sound; check Settings ▸ Audio Hardware)"), &[("error", e)]),
        }
    }
}

/// The host named in the settings (empty or unknown = the default host).
fn host(name: &str) -> cpal::Host {
    if !name.is_empty()
        && let Some(id) = cpal::available_hosts().into_iter().find(|h| h.name() == name)
        && let Ok(h) = cpal::host_from_id(id)
    {
        return h;
    }
    cpal::default_host()
}

/// The output device the settings ask for, with a status-bar note when a fallback is in use.
struct DeviceChoice {
    device: cpal::Device,
    /// The host's default device could not be opened: this device is the fallback.
    fallback: Option<Note>,
}

/// Fallback order when the default device cannot be opened: the sound-server bridges first
/// (`pipewire`, `pulse`: they share the card with every other app), then other named PCMs, and
/// raw `hw:`/`plughw:` devices last (opening one takes the card exclusively and silences the
/// rest of the desktop).
fn fallback_rank(name: &str) -> u8 {
    let n = name.to_ascii_lowercase();
    if n == "pipewire" || n.starts_with("pipewire:") {
        0
    } else if n == "pulse" || n.starts_with("pulse:") {
        1
    } else if n.starts_with("hw:") || n.starts_with("plughw:") {
        3
    } else {
        2
    }
}

/// The output device named in the settings (empty or missing = the host's default). When the
/// default cannot be opened — a broken ALSA `default` (missing `99-pipewire-default.conf`,
/// #23/#106) — a fallback that can, so the app is not silently mute. The fallback list is probed
/// only after the default fails, in [`fallback_rank`] order, and the pick is cached in `cache`.
fn output_device(h: &cpal::Host, name: &str, cache: &Mutex<Option<(String, String)>>) -> Option<DeviceChoice> {
    if !name.is_empty()
        && let Ok(mut devs) = h.output_devices()
        && let Some(d) = devs.find(|d| d.name().is_ok_and(|n| n == name))
    {
        return Some(DeviceChoice { device: d, fallback: None });
    }
    if let Some(d) = h.default_output_device()
        && d.default_output_config().is_ok()
    {
        return Some(DeviceChoice { device: d, fallback: None });
    }
    let host_name = h.id().name().to_string();
    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    let mut devs: Vec<(String, cpal::Device)> = h.output_devices().ok()?.filter_map(|d| d.name().ok().map(|n| (n, d))).collect();
    if let Some((cached_host, cached)) = cache.as_ref()
        && *cached_host == host_name
        && let Some(i) = devs.iter().position(|(n, _)| n == cached)
    {
        let (found, d) = devs.swap_remove(i);
        return Some(DeviceChoice { device: d, fallback: Some(Note::Fallback(found)) });
    }
    devs.sort_by_key(|(n, _)| fallback_rank(n));
    let (found, d) = devs.into_iter().find(|(_, d)| d.default_output_config().is_ok())?;
    log::warn!("the default audio output device cannot be opened; falling back to '{found}'");
    *cache = Some((host_name, found.clone()));
    Some(DeviceChoice { device: d, fallback: Some(Note::Fallback(found)) })
}

impl CpalOut {
    pub fn new() -> Self {
        let host = cpal::default_host();
        let fallback = Mutex::new(None);
        let choice = output_device(&host, "", &fallback);
        let cfg = choice.as_ref().and_then(|c| c.device.default_output_config().ok());
        if let Some(c) = &choice {
            log::info!(
                "audio output device: {} on {} (default config {:?})",
                c.device.name().unwrap_or_else(|_| "?".into()),
                host.id().name(),
                cfg.as_ref().map(|c| (c.sample_format(), c.sample_rate().0, c.channels()))
            );
        }
        let note = match choice {
            Some(c) => c.fallback,
            None => {
                log::warn!("no audio output device found");
                Some(Note::NoDevice)
            }
        };
        let mut out = Self {
            stream: None,
            played: Arc::new(AtomicU64::new(0)),
            failed: Arc::new(AtomicBool::new(false)),
            rate: cfg.as_ref().map_or(48_000, |c| c.sample_rate().0),
            channels: cfg.as_ref().map_or(2, |c| c.channels()),
            hw: AudioHardwarePrefs::default(),
            document_rate: None,
            note,
            fallback,
            error: Arc::new(Mutex::new(None)),
            keepalive: None,
            routed: false,
            configs: Mutex::new(None),
        };
        out.open_keepalive();
        out
    }

    /// (Re)open the idle stream on the device playback will use ([`Self::keepalive`]), in the
    /// configuration playback would open, so `start` can play on it. Best effort: a device that
    /// refuses it opens its own stream on play, as before.
    fn open_keepalive(&mut self) {
        self.keepalive = None;
        self.routed = false;
        if !cfg!(any(target_os = "linux", target_os = "windows")) {
            return;
        }
        let host = host(&self.hw.device_class);
        let Some(choice) = output_device(&host, &self.hw.default_output, &self.fallback) else { return };
        let dev = choice.device;
        // a raw ALSA device is exclusive: a second stream on it would lock playback out
        if dev.name().is_ok_and(|n| fallback_rank(&n) == 3) {
            return;
        }
        let Ok(cfg) = self.config(&dev) else { return };
        if cfg.channels() == 0 || cfg.sample_rate().0 == 0 {
            return;
        }
        let config = self.stream_config(&cfg);
        let slot: Arc<Mutex<Option<Fill>>> = Arc::new(Mutex::new(None));
        let failed = Arc::new(AtomicBool::new(false));
        for (c, sc) in attempts(&cfg, &config, dev.default_output_config().ok()) {
            // the frames are counted by `routed_fill` (only while a callback is installed)
            let fill = routed_fill(slot.clone(), self.played.clone());
            match self.build(&dev, c.sample_format(), &sc, fill, Arc::new(AtomicU64::new(0)), failed.clone()) {
                Ok(s) => {
                    if let Err(e) = s.play() {
                        log::debug!("audio keep-alive stream not started: {e}");
                        return;
                    }
                    self.rate = c.sample_rate().0;
                    self.channels = c.channels();
                    self.keepalive = Some(Idle { _stream: s, slot, failed });
                    return;
                }
                Err(e) => log::debug!("audio keep-alive stream not opened ({:?} {} Hz): {e}", c.sample_format(), c.sample_rate().0),
            }
        }
    }

    /// The stream configuration for `cfg`, with the settings' fixed buffer size when the device
    /// supports it.
    fn stream_config(&self, cfg: &cpal::SupportedStreamConfig) -> cpal::StreamConfig {
        let mut config: cpal::StreamConfig = cfg.clone().into();
        if let cpal::SupportedBufferSize::Range { min, max } = cfg.buffer_size()
            && (*min..=*max).contains(&self.hw.buffer_size)
        {
            config.buffer_size = cpal::BufferSize::Fixed(self.hw.buffer_size);
        }
        config
    }

    /// The stream configuration the settings ask for, falling back to the device default.
    /// Remembered per device until `configure` ([`Self::configs`]).
    fn config(&self, dev: &cpal::Device) -> Result<cpal::SupportedStreamConfig, String> {
        let name = dev.name().unwrap_or_default();
        if let Some((n, c)) = self.configs.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
            && *n == name
        {
            return Ok(c.clone());
        }
        let chosen = self.probe_config(dev)?;
        *self.configs.lock().unwrap_or_else(PoisonError::into_inner) = Some((name, chosen.clone()));
        Ok(chosen)
    }

    fn probe_config(&self, dev: &cpal::Device) -> Result<cpal::SupportedStreamConfig, String> {
        let default = dev.default_output_config().map_err(|e| e.to_string())?;
        let want = if self.hw.force_document_rate { self.document_rate.unwrap_or(self.hw.sample_rate) } else { self.hw.sample_rate };
        let exact = dev.supported_output_configs().ok().and_then(|it| {
            it.filter(|c| c.channels() > 0 && (c.min_sample_rate().0..=c.max_sample_rate().0).contains(&want))
                .max_by_key(|c| {
                    (c.channels() == default.channels(), c.sample_format() == cpal::SampleFormat::F32, c.sample_format() == default.sample_format())
                })
                .and_then(|c| c.try_with_sample_rate(cpal::SampleRate(want)))
        });
        Ok(exact.unwrap_or(default))
    }

    /// One output stream in `config`, converting from the f32 the mixer produces when the device
    /// wants another sample format.
    fn build(
        &self,
        dev: &cpal::Device,
        format: cpal::SampleFormat,
        config: &cpal::StreamConfig,
        fill: Box<dyn FnMut(&mut [f32], usize) + Send>,
        played: Arc<AtomicU64>,
        failed: Arc<AtomicBool>,
    ) -> Result<cpal::Stream, cpal::BuildStreamError> {
        match format {
            cpal::SampleFormat::F32 => self.f32_stream(dev, config, fill, played, failed),
            cpal::SampleFormat::I8 => self.converted_stream::<i8>(dev, config, fill, played, failed),
            cpal::SampleFormat::I16 => self.converted_stream::<i16>(dev, config, fill, played, failed),
            cpal::SampleFormat::I24 => self.converted_stream::<cpal::I24>(dev, config, fill, played, failed),
            cpal::SampleFormat::I32 => self.converted_stream::<i32>(dev, config, fill, played, failed),
            cpal::SampleFormat::I64 => self.converted_stream::<i64>(dev, config, fill, played, failed),
            cpal::SampleFormat::U8 => self.converted_stream::<u8>(dev, config, fill, played, failed),
            cpal::SampleFormat::U16 => self.converted_stream::<u16>(dev, config, fill, played, failed),
            cpal::SampleFormat::U32 => self.converted_stream::<u32>(dev, config, fill, played, failed),
            cpal::SampleFormat::U64 => self.converted_stream::<u64>(dev, config, fill, played, failed),
            cpal::SampleFormat::F64 => self.converted_stream::<f64>(dev, config, fill, played, failed),
            _ => Err(cpal::BuildStreamError::StreamConfigNotSupported),
        }
    }

    fn f32_stream(
        &self,
        dev: &cpal::Device,
        config: &cpal::StreamConfig,
        mut fill: Box<dyn FnMut(&mut [f32], usize) + Send>,
        played: Arc<AtomicU64>,
        failed: Arc<AtomicBool>,
    ) -> Result<cpal::Stream, cpal::BuildStreamError> {
        let channels = usize::from(config.channels);
        let error = self.error.clone();
        dev.build_output_stream(
            config,
            move |buf: &mut [f32], _| {
                fill(buf, channels);
                played.fetch_add((buf.len() / channels) as u64, Ordering::SeqCst);
            },
            move |e| stream_error(&failed, &error, e),
            None,
        )
    }

    fn converted_stream<T>(
        &self,
        dev: &cpal::Device,
        config: &cpal::StreamConfig,
        mut fill: Box<dyn FnMut(&mut [f32], usize) + Send>,
        played: Arc<AtomicU64>,
        failed: Arc<AtomicBool>,
    ) -> Result<cpal::Stream, cpal::BuildStreamError>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let channels = usize::from(config.channels);
        let error_flag = failed.clone();
        let error = self.error.clone();
        let mut scratch = Vec::new();
        dev.build_output_stream(
            config,
            move |buf: &mut [T], _| {
                // Reuse the conversion buffer. A pathological driver buffer must not panic the audio thread.
                if buf.len() > 1_048_576 || scratch.try_reserve(buf.len().saturating_sub(scratch.len())).is_err() {
                    buf.fill(T::EQUILIBRIUM);
                    if !failed.swap(true, Ordering::SeqCst) {
                        eprintln!("redrob-film: unable to allocate the audio conversion buffer");
                    }
                    return;
                }
                scratch.resize(buf.len(), 0.0);
                fill(&mut scratch, channels);
                convert_samples(&scratch, buf);
                played.fetch_add((buf.len() / channels) as u64, Ordering::SeqCst);
            },
            move |e| stream_error(&error_flag, &error, e),
            None,
        )
    }
}

/// cpal's error callback: the stream is dead, so the playback clock is given up and the status
/// bar says why (`note`). Not the realtime data callback, but kept allocation-light all the same.
fn stream_error(failed: &AtomicBool, error: &Mutex<Option<String>>, e: cpal::StreamError) {
    if !failed.swap(true, Ordering::SeqCst) {
        eprintln!("redrob-film: audio stream error: {e}");
        *error.lock().unwrap_or_else(PoisonError::into_inner) = Some(e.to_string());
    }
}

fn convert_samples<T: cpal::Sample + cpal::FromSample<f32>>(input: &[f32], output: &mut [T]) {
    // Integer conversion represents [-1, 1). In particular I24 cannot represent +1 exactly.
    let upper = f32::from_bits(1.0_f32.to_bits() - 1);
    for (sample, converted) in input.iter().zip(output) {
        *converted = T::from_sample(if sample.is_finite() { sample.clamp(-1.0, upper) } else { 0.0 });
    }
}

/// The stream configurations to try, in order: the one the settings ask for; the same with the
/// device's own buffer size (WASAPI and some ALSA/CoreAudio devices refuse a fixed size); the
/// device's default configuration (on Windows the shared-mode mix format, `GetMixFormat`); that
/// mix format as 32-bit float (the engine's own sample type); then stereo float / 16-bit at the
/// mix rate and at 48 / 44.1 kHz. Opening the first that works is what keeps a Windows machine
/// from playing in silence when the mix format is not what the settings expect (upstream #1705:
/// "unsupported stream configuration" on every device). Duplicates are tried once.
fn attempts(
    wanted: &cpal::SupportedStreamConfig,
    config: &cpal::StreamConfig,
    default: Option<cpal::SupportedStreamConfig>,
) -> Vec<(cpal::SupportedStreamConfig, cpal::StreamConfig)> {
    fn push(out: &mut Vec<(cpal::SupportedStreamConfig, cpal::StreamConfig)>, c: cpal::SupportedStreamConfig, buffer: cpal::BufferSize) {
        let mut sc: cpal::StreamConfig = c.clone().into();
        sc.buffer_size = buffer;
        if c.channels() > 0 && c.sample_rate().0 > 0 && !out.iter().any(|(o, s)| o.sample_format() == c.sample_format() && *s == sc) {
            out.push((c, sc));
        }
    }
    let mut out = Vec::new();
    push(&mut out, wanted.clone(), config.buffer_size);
    push(&mut out, wanted.clone(), cpal::BufferSize::Default);
    let base = default.unwrap_or_else(|| wanted.clone());
    push(&mut out, base.clone(), cpal::BufferSize::Default);
    let like = |channels: u16, rate: u32, format: cpal::SampleFormat| {
        cpal::SupportedStreamConfig::new(channels, cpal::SampleRate(rate), *base.buffer_size(), format)
    };
    let rate = base.sample_rate().0;
    push(&mut out, like(base.channels(), rate, cpal::SampleFormat::F32), cpal::BufferSize::Default);
    push(&mut out, like(2, rate, cpal::SampleFormat::F32), cpal::BufferSize::Default);
    push(&mut out, like(2, rate, cpal::SampleFormat::I16), cpal::BufferSize::Default);
    for r in [48_000, 44_100] {
        push(&mut out, like(2, r, cpal::SampleFormat::F32), cpal::BufferSize::Default);
    }
    out
}

impl AudioOut for CpalOut {
    fn start(&mut self, fill: Box<dyn FnMut(&mut [f32], usize) + Send>) -> Result<u32, String> {
        self.stop();
        *self.error.lock().unwrap_or_else(PoisonError::into_inner) = None;
        // the idle stream died (device unplugged, sound server restarted): open it again
        if self.keepalive.as_ref().is_some_and(|k| k.failed.load(Ordering::SeqCst)) {
            self.open_keepalive();
        }
        // Play on the idle stream when there is one: it already runs, so nothing is opened on
        // the UI thread and the clock moves on the device's next callback.
        if let Some(k) = self.keepalive.as_ref() {
            let old = {
                let mut slot = k.slot.lock().unwrap_or_else(PoisonError::into_inner);
                self.played.store(0, Ordering::SeqCst);
                slot.replace(fill)
            };
            drop(old);
            self.routed = true;
            return Ok(self.rate);
        }
        let r = self.open(fill);
        if let Err(e) = &r {
            log::warn!("audio output unavailable: {e}");
            self.note = Some(Note::Error(e.clone()));
        }
        r
    }
    fn stop(&mut self) {
        self.stream = None;
        if std::mem::take(&mut self.routed)
            && let Some(k) = self.keepalive.as_ref()
        {
            // back to silence; the old callback (and its mixing thread) is dropped outside the lock
            let old = k.slot.lock().unwrap_or_else(PoisonError::into_inner).take();
            drop(old);
        }
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> usize {
        self.channels as usize
    }
    fn played_frames(&self) -> Option<u64> {
        if self.routed {
            return self.keepalive.as_ref().filter(|k| !k.failed.load(Ordering::SeqCst)).map(|_| self.played.load(Ordering::SeqCst));
        }
        self.stream.as_ref().filter(|_| !self.failed.load(Ordering::SeqCst)).map(|_| self.played.load(Ordering::SeqCst))
    }
    fn devices(&self) -> AudioDevices {
        let h = host(&self.hw.device_class);
        let outputs = h.output_devices().map(|d| d.filter_map(|x| x.name().ok()).collect()).unwrap_or_default();
        let inputs = h.input_devices().map(|d| d.filter_map(|x| x.name().ok()).collect()).unwrap_or_default();
        let output_channels =
            output_device(&h, &self.hw.default_output, &self.fallback).and_then(|c| self.config(&c.device).ok()).map(|cfg| cfg.channels()).unwrap_or(0);
        AudioDevices { hosts: cpal::available_hosts().iter().map(|h| h.name().to_string()).collect(), inputs, outputs, output_channels }
    }
    fn configure(&mut self, hw: &AudioHardwarePrefs, document_rate: Option<u32>) {
        self.hw = hw.clone();
        self.document_rate = document_rate;
        // the settings (or the document rate) may ask for another configuration
        *self.configs.lock().unwrap_or_else(PoisonError::into_inner) = None;
        // the rate playback mixes at must be known before `start`
        let host = host(&self.hw.device_class);
        if let Some(choice) = output_device(&host, &self.hw.default_output, &self.fallback)
            && let Ok(cfg) = self.config(&choice.device)
        {
            self.rate = cfg.sample_rate().0;
            self.channels = cfg.channels();
            self.note = choice.fallback;
        }
        // the device may have changed: keep the one playback will use awake
        self.open_keepalive();
    }
    fn note(&self) -> Option<String> {
        if let Some(e) = self.error.lock().unwrap_or_else(PoisonError::into_inner).clone() {
            return Some(Note::Error(e).text());
        }
        self.note.as_ref().map(Note::text)
    }
}

impl CpalOut {
    fn open(&mut self, fill: Box<dyn FnMut(&mut [f32], usize) + Send>) -> Result<u32, String> {
        let host = host(&self.hw.device_class);
        let DeviceChoice { device: dev, fallback } = output_device(&host, &self.hw.default_output, &self.fallback).ok_or("no output device")?;
        self.note = fallback;
        let cfg = self.config(&dev)?;
        if cfg.channels() == 0 || cfg.sample_rate().0 == 0 {
            return Err("the output device returned an invalid channel count or sample rate".into());
        }
        let config = self.stream_config(&cfg);
        self.played.store(0, Ordering::SeqCst);
        self.failed.store(false, Ordering::SeqCst);
        // Every attempt gets the same mixer: a failed `build_output_stream` consumes its closure.
        let fill = Arc::new(Mutex::new(fill));
        let shared = || {
            let f = fill.clone();
            Box::new(move |buf: &mut [f32], ch: usize| (f.lock().unwrap_or_else(PoisonError::into_inner))(buf, ch)) as Box<dyn FnMut(&mut [f32], usize) + Send>
        };
        let mut opened = None;
        let mut last = String::from("no stream configuration to try");
        for (c, sc) in attempts(&cfg, &config, dev.default_output_config().ok()) {
            // A stream that builds but will not start (WASAPI can refuse at `Start`) is one more
            // configuration that failed, not the end of playback: try the next.
            let started = self.build(&dev, c.sample_format(), &sc, shared(), self.played.clone(), self.failed.clone()).map_err(|e| e.to_string());
            match started.and_then(|s| s.play().map(|()| s).map_err(|e| e.to_string())) {
                Ok(s) => {
                    opened = Some((s, c, sc));
                    break;
                }
                Err(e) => {
                    log::warn!(
                        "audio output: cannot open {:?} {} Hz {} ch buffer {:?}: {e}",
                        c.sample_format(),
                        c.sample_rate().0,
                        c.channels(),
                        sc.buffer_size
                    );
                    last = e;
                }
            }
        }
        let (stream, cfg, config) = opened.ok_or(last)?;
        self.rate = cfg.sample_rate().0;
        self.channels = cfg.channels();
        log::info!(
            "audio output: {} on {} ({:?}, {} Hz, {} ch, buffer {:?})",
            dev.name().unwrap_or_else(|_| "?".into()),
            host.id().name(),
            cfg.sample_format(),
            self.rate,
            self.channels,
            config.buffer_size
        );
        self.stream = Some(stream);
        Ok(self.rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::Sample;

    #[test]
    fn fallback_prefers_sound_servers_over_raw_hardware() {
        let mut names = vec!["plughw:CARD=PCH,DEV=0", "hw:CARD=PCH,DEV=0", "sysdefault:CARD=PCH", "pulse", "pipewire"];
        names.sort_by_key(|n| fallback_rank(n));
        assert_eq!(names, ["pipewire", "pulse", "sysdefault:CARD=PCH", "plughw:CARD=PCH,DEV=0", "hw:CARD=PCH,DEV=0"]);
    }

    #[test]
    fn notes_name_the_fallback_device() {
        assert!(Note::Fallback("pipewire".into()).text().contains("'pipewire'"));
        assert!(!Note::NoDevice.text().is_empty());
    }

    #[test]
    fn integer_conversion_clips_and_preserves_interleaved_channels() {
        let mut signed = [0_i16; 8];
        convert_samples(&[-2.0, 2.0, -0.5, 0.5, 0.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY], &mut signed);
        assert_eq!(signed, [i16::MIN, i16::MAX, -16_384, 16_384, 0, 0, 0, 0]);
        let mut unsigned = [0_u16; 3];
        convert_samples(&[-1.0, 0.0, 1.0], &mut unsigned);
        assert_eq!(unsigned, [0, 32_768, u16::MAX]);
    }

    #[test]
    fn full_scale_24_bit_samples_remain_representable() {
        let mut samples = [cpal::I24::EQUILIBRIUM; 3];
        convert_samples(&[-1.0, 0.0, 1.0], &mut samples);
        assert_eq!(samples.map(|s| s.inner()), [-8_388_608, 0, 8_388_607]);
    }

    /// The idle stream plays silence without moving the clock, and the installed callback (play,
    /// scrub) from the next device buffer on, counting its frames.
    #[test]
    fn idle_stream_plays_the_installed_callback() {
        let slot: Arc<Mutex<Option<Fill>>> = Arc::new(Mutex::new(None));
        let played = Arc::new(AtomicU64::new(0));
        let mut cb = routed_fill(slot.clone(), played.clone());
        let mut buf = vec![9.0_f32; 256 * 2];
        cb(&mut buf, 2);
        assert!(buf.iter().all(|s| *s == 0.0), "silence while idle");
        assert_eq!(played.load(Ordering::SeqCst), 0, "idle silence is not playback");
        let half: Fill = Box::new(|b: &mut [f32], _| b.fill(0.5));
        *slot.lock().unwrap() = Some(half);
        cb(&mut buf, 2);
        assert!(buf.iter().all(|s| *s == 0.5));
        assert_eq!(played.load(Ordering::SeqCst), 256);
        slot.lock().unwrap().take();
        cb(&mut buf, 2);
        assert!(buf.iter().all(|s| *s == 0.0), "back to silence after stop");
        assert_eq!(played.load(Ordering::SeqCst), 256);
    }

    /// Upstream #1705: WASAPI refused the configuration the settings asked for, and nothing else
    /// was tried that the shared-mode engine accepts. The list must end in the mix format, its
    /// float form and plain stereo float, each once.
    #[test]
    fn attempts_fall_back_to_the_mix_format_then_stereo_float() {
        let range = cpal::SupportedBufferSize::Range { min: 64, max: 4096 };
        let wanted = cpal::SupportedStreamConfig::new(2, cpal::SampleRate(96_000), range, cpal::SampleFormat::F32);
        let mut config: cpal::StreamConfig = wanted.clone().into();
        config.buffer_size = cpal::BufferSize::Fixed(256);
        // a 7.1 mix format in 24-bit integers at 44.1 kHz
        let mix = cpal::SupportedStreamConfig::new(8, cpal::SampleRate(44_100), range, cpal::SampleFormat::I24);
        let list = attempts(&wanted, &config, Some(mix));
        let key = |(c, s): &(cpal::SupportedStreamConfig, cpal::StreamConfig)| (c.sample_format(), s.channels, s.sample_rate.0, s.buffer_size);
        let keys: Vec<_> = list.iter().map(key).collect();
        use cpal::{BufferSize::Default as D, SampleFormat as F};
        assert_eq!(
            keys,
            [
                (F::F32, 2, 96_000, cpal::BufferSize::Fixed(256)),
                (F::F32, 2, 96_000, D),
                (F::I24, 8, 44_100, D),
                (F::F32, 8, 44_100, D),
                (F::F32, 2, 44_100, D),
                (F::I16, 2, 44_100, D),
                (F::F32, 2, 48_000, D),
            ]
        );
        // a stereo float mix format at the wanted rate collapses to two attempts plus 48 / 44.1 kHz
        let same = attempts(&wanted, &wanted.clone().into(), Some(wanted.clone()));
        let keys: Vec<_> = same.iter().map(key).collect();
        assert_eq!(keys, [(F::F32, 2, 96_000, D), (F::I16, 2, 96_000, D), (F::F32, 2, 48_000, D), (F::F32, 2, 44_100, D)]);
    }

    #[test]
    fn failed_stream_relinquishes_the_playback_clock() {
        let failed = AtomicBool::new(false);
        let error = Mutex::new(None);
        stream_error(&failed, &error, cpal::StreamError::DeviceNotAvailable);
        assert!(failed.load(Ordering::SeqCst));
        assert!(error.lock().unwrap_or_else(PoisonError::into_inner).is_some());
        let output = CpalOut::new();
        assert_eq!(output.played_frames(), None);
    }
}
