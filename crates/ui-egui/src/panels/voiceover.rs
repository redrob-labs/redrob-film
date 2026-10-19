//! Voice-over recording from the timeline: the audio track header's microphone button and the
//! Voice-Over Record Settings dialog. Recording itself is the engine's `audio.voiceover.*`
//! commands; this module ties them to playback (start from the capture start, restart the capture
//! when the audio clock really starts, stop with playback or after the post-roll past the Out
//! point) and draws the countdown.
//!
//! Automation ids: `timeline.track.<A1…>.voiceover` (click: record on that track / stop; right-
//! click: menu), `timeline.track.<A1…>.voiceover.settings` (menu entry), and in the dialog
//! `voiceover.source`, `voiceover.source.<i>`, `voiceover.input`, `voiceover.input.<i>`,
//! `voiceover.name`, `voiceover.countdown`, `voiceover.preroll`, `voiceover.postroll`,
//! `voiceover.ok`, `voiceover.cancel`; the countdown overlay is `voiceover.countdown.overlay`.
//! UI commands: `voiceover.recordToggle {track?}` and `voiceover.settingsDialog`.

use egui::{Align2, Color32, Rect, RichText, Sense, Stroke, pos2, vec2};
use redrob_film_engine::voiceover::VoiceOverPrefs;
use redrob_film_project::TrackId;
use redrob_film_time::Tick;
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

const RED: Color32 = Color32::from_rgb(0xd8, 0x50, 0x3f);

/// What the header button asked for.
pub enum Action {
    Toggle(TrackId),
    Settings,
}

#[derive(Clone, Default)]
struct Draft {
    prefs: VoiceOverPrefs,
    devices: Vec<String>,
    channels: u16,
}

fn draft_id() -> egui::Id {
    egui::Id::new("voiceover-settings-draft")
}

/// The microphone button of an audio track header. Returns the requested action.
pub fn header_button(app: &mut FilmcraftApp, ui: &mut egui::Ui, track: TrackId, r: Rect, visible: Rect, label: &str, t: &Tokens) -> Option<Action> {
    let recording_here = app.session.voiceover.rec.as_ref().is_some_and(|x| x.track == track);
    let resp = ui.interact(r.intersect(visible), egui::Id::new(("vo", track.0)), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 2.0, t.hover);
    }
    let col = if recording_here { RED } else { t.text_dim };
    icons::paint(&ui.painter().with_clip_rect(visible), r.shrink(1.0), Icon::Mic, col);
    let id = format!("timeline.track.{label}.voiceover");
    app.auto.add(&id, r, if recording_here { "Stop Voice-over Recording" } else { "Voice-over Record" });
    let mut act = None;
    if resp.clicked() {
        act = Some(Action::Toggle(track));
    }
    let resp = resp.on_hover_text(tl!("Voice-over record (right-click: Voice-Over Record Settings…)"));
    resp.context_menu(|ui| {
        let b = ui.button(tl!("Voice-Over Record Settings…"));
        app.auto.add(&format!("{id}.settings"), b.rect, "Voice-Over Record Settings…");
        if b.clicked() {
            act = Some(Action::Settings);
            ui.close();
        }
    });
    act
}

pub fn run(app: &mut FilmcraftApp, ctx: &egui::Context, a: Action) {
    let r = match a {
        Action::Toggle(t) => toggle(app, Some(t.0)),
        Action::Settings => {
            open_settings(app, ctx);
            Ok(Value::Null)
        }
    };
    if let Err(e) = r {
        app.ui.status = e;
    }
}

/// Start recording on `track` (None = the engine's choice), or stop the recording in progress.
pub fn toggle(app: &mut FilmcraftApp, track: Option<u64>) -> Result<Value, String> {
    if app.session.voiceover.recording() {
        if app.playback.playing {
            app.stop(); // runs `audio.voiceover.stop` (see `on_stop`)
            return Ok(json!({"recording": false}));
        }
        return stop(app);
    }
    if app.playback.playing {
        app.stop();
    }
    let p = match track {
        Some(t) => json!({"track": t}),
        None => json!({}),
    };
    let r = app.session.execute("audio.voiceover.start", p).map_err(|e| e.to_string())?;
    let from = Tick(r["captureStart"].as_i64().unwrap_or(0));
    app.session.set_playhead(from);
    app.play(1.0);
    app.ui.status = tl!("Recording voice-over…").into();
    Ok(r)
}

fn stop(app: &mut FilmcraftApp) -> Result<Value, String> {
    let t = app.session.playhead();
    let r = app.session.execute("audio.voiceover.stop", json!({"time": t.0})).map_err(|e| e.to_string())?;
    app.ui.status = match r["path"].as_str() {
        Some(p) => tlf!("Voice-over recorded: {p}", p),
        None => tl!("Voice-over recording stopped (nothing recorded)").into(),
    };
    Ok(r)
}

/// Playback (and the audio clock) really started: capture from here.
pub fn on_play(app: &mut FilmcraftApp) {
    if app.session.voiceover.recording() {
        let t = app.session.playhead();
        if let Err(e) = app.session.execute("audio.voiceover.sync", json!({"time": t.0})) {
            app.ui.status = e.to_string();
        }
    }
}

/// Playback stopped: finish the take (unless it was paused: Space pauses a take, see [`space`]).
pub fn on_stop(app: &mut FilmcraftApp) {
    if app.session.voiceover.recording()
        && app.session.voiceover.paused().is_none()
        && let Err(e) = stop(app)
    {
        app.ui.status = e;
    }
}

/// Where the Program monitor stores its picture rectangle each frame (the countdown is drawn on it).
pub fn program_picture_id() -> egui::Id {
    egui::Id::new("voiceover-program-picture")
}

fn pending_id() -> egui::Id {
    egui::Id::new("voiceover-record-pending")
}

/// Whether Space belongs to the voice-over (a take in progress, or `r` pressed during playback and
/// waiting for Space to start recording).
pub fn space_owned(app: &FilmcraftApp, ctx: &egui::Context) -> bool {
    app.session.voiceover.recording() || ctx.data(|d| d.get_temp::<bool>(pending_id())).unwrap_or(false)
}

/// Space (Kdenlive): while recording, pause the take (timeline playback stops, what was captured
/// is kept); again to resume it from the pause point. After `r` during playback, Space starts the
/// recording at the playhead.
pub fn space(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    if ctx.data(|d| d.get_temp::<bool>(pending_id())).unwrap_or(false) {
        ctx.data_mut(|d| d.remove::<bool>(pending_id()));
        return toggle(app, None);
    }
    let r = app.session.execute("audio.voiceover.pause", json!({"time": app.session.playhead().0})).map_err(|e| e.to_string())?;
    if r["paused"].as_bool() == Some(true) {
        if app.playback.playing {
            app.stop();
        }
        app.ui.status = tl!("Voice-over paused (Space: resume, Esc: stop)").into();
    } else {
        let at = Tick(r["resumeAt"].as_i64().unwrap_or(app.session.playhead().0));
        app.session.set_playhead(at);
        app.play(1.0);
        app.ui.status = tl!("Recording voice-over…").into();
    }
    Ok(r)
}

/// The `r` key while a track is record-armed (Kdenlive): during playback it pauses playback and
/// waits for Space to start recording there; otherwise it starts (or stops) recording.
pub fn record_key(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    if app.playback.playing && !app.session.voiceover.recording() {
        app.stop();
        ctx.data_mut(|d| d.insert_temp(pending_id(), true));
        app.ui.status = tl!("Voice-over ready: press Space to record from here").into();
        return Ok(json!({"pending": true}));
    }
    ctx.data_mut(|d| d.remove::<bool>(pending_id()));
    toggle(app, None)
}

/// Monitoring mode follows the record-armed tracks; the input is read every frame while armed or
/// recording, for the input meters and the live waveform.
fn poll_input(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let armed = app.session.active_sequence().is_some_and(|q| q.audio_tracks.iter().any(|t| t.mixer.record_arm));
    if armed != app.session.voiceover.monitoring() {
        let _ = app.session.execute("audio.voiceover.monitor", json!({"on": armed}));
    }
    let recording = app.session.voiceover.recording() && app.playback.playing;
    if !recording && !app.session.voiceover.monitoring() {
        return;
    }
    let sr = app.session.active_sequence().map_or(48_000, |q| q.settings.sample_rate.max(1)) as f32;
    let frames = (ctx.input(|i| i.stable_dt).clamp(0.0, 0.1) * sr) as u64;
    let p = if recording { json!({"time": app.session.playhead().0}) } else { json!({"frames": frames}) };
    match app.session.execute("audio.voiceover.poll", p) {
        Ok(r) => {
            let peaks: Vec<f32> = r["peaks"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect()).unwrap_or_default();
            if !peaks.is_empty() {
                app.session.previews.live.post_input_meter(&peaks);
            }
        }
        Err(e) => app.ui.status = e.to_string(),
    }
    ctx.request_repaint();
}

/// Countdown cue beeps of the recording in progress, for an output at `sr`: (start sample, tone).
pub fn cues(s: &redrob_film_engine::Session, sr: u32) -> (Vec<i64>, Vec<f32>) {
    match &s.voiceover.rec {
        Some(rec) if s.prefs.voice_over.countdown_sound_cues => {
            let at = redrob_film_engine::voiceover::cue_times(rec.capture_start, rec.record_start).into_iter().map(|t| t.to_units_floor(sr as i64)).collect();
            (at, redrob_film_engine::voiceover::cue_tone(sr))
        }
        _ => (Vec::new(), Vec::new()),
    }
}

/// Add the cue beeps that fall in the output block starting at device sample `cursor`.
pub fn mix_cues(buf: &mut [f32], ch: usize, cursor: i64, cues: &(Vec<i64>, Vec<f32>)) {
    let (at, tone) = cues;
    let ch = ch.max(1);
    let n = (buf.len() / ch) as i64;
    for &c in at {
        let (a, b) = (c.max(cursor), (c + tone.len() as i64).min(cursor + n));
        for i in a..b {
            let x = tone[(i - c) as usize];
            for s in &mut buf[(i - cursor) as usize * ch..(i - cursor + 1) as usize * ch] {
                *s += x;
            }
        }
    }
}

/// UI commands `voiceover.recordToggle {track?}` and `voiceover.settingsDialog`.
pub fn route(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, p: &Value) -> Option<Result<Value, String>> {
    match id {
        "voiceover.recordToggle" => {
            let track = match p.get("track") {
                None => None,
                Some(v) => {
                    let seq = app.session.active_sequence()?;
                    match redrob_film_engine::mixer::strip_ref(seq, v) {
                        Some(t) => Some(t.0),
                        None => return Some(Err(format!("no audio track {v}"))),
                    }
                }
            };
            Some(toggle(app, track))
        }
        "voiceover.settingsDialog" => {
            open_settings(app, ctx);
            Some(Ok(Value::Null))
        }
        _ => None,
    }
}

fn open_settings(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let v = app.session.execute("audio.voiceover.settings", json!({})).unwrap_or_default();
    let d = Draft {
        prefs: app.session.prefs.voice_over.clone(),
        devices: v["devices"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        channels: v["channels"].as_u64().unwrap_or(0) as u16,
    };
    ctx.data_mut(|m| m.insert_temp(draft_id(), Some(d)));
}

/// Every frame: the settings dialog, the countdown overlay, and the punch-out + post-roll stop.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    settings_dialog(app, ctx);
    poll_input(app, ctx);
    let Some(rec) = app.session.voiceover.rec.clone() else { return };
    if !app.playback.playing {
        return;
    }
    let now = app.session.playhead();
    if let Some(out) = rec.punch_out {
        let post = Tick::from_seconds_f64(app.session.prefs.voice_over.postroll_seconds);
        if now >= out + post {
            app.stop();
            return;
        }
    }
    let screen = ctx.content_rect();
    if now < rec.record_start && !app.session.prefs.voice_over.show_countdown {
        return;
    }
    let text = if now < rec.record_start {
        let left = (rec.record_start - now).seconds().ceil().max(1.0) as i64;
        format!("{left}")
    } else if rec.punch_out.is_some_and(|o| now >= o) {
        tl!("Post-roll").into()
    } else {
        tl!("● Recording").into()
    };
    // Kdenlive: the countdown runs in the project (Program) monitor; without one on screen, at
    // the top of the window
    let r = match ctx.data(|d| d.get_temp::<Rect>(program_picture_id())).filter(|p| p.width() > 120.0 && p.height() > 80.0) {
        Some(pic) => Rect::from_center_size(pic.center(), vec2(220.0f32.min(pic.width() - 20.0), 64.0)),
        None => Rect::from_center_size(pos2(screen.center().x, screen.min.y + 90.0), vec2(220.0, 64.0)),
    };
    egui::Area::new(egui::Id::new("voiceover-countdown")).order(egui::Order::Foreground).fixed_pos(r.min).interactable(false).show(ctx, |ui| {
        let p = ui.painter();
        p.rect_filled(r, 8.0, Color32::from_black_alpha(180));
        p.rect_stroke(r, 8.0, Stroke::new(2.0, RED), egui::StrokeKind::Inside);
        p.text(r.center(), Align2::CENTER_CENTER, &text, Tokens::semibold(if text.len() <= 2 { 36.0 } else { 20.0 }), Color32::WHITE);
    });
    app.auto.add("voiceover.countdown.overlay", r, &text);
    ctx.request_repaint();
}

fn settings_dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(Some(mut d)) = ctx.data(|m| m.get_temp::<Option<Draft>>(draft_id())) else { return };
    let mut elems: Vec<(String, Rect, String)> = Vec::new();
    let mut close = false;
    let mut apply = false;
    let mut discard = false;
    let accent = app.tokens.accent;
    // the title is translated; the window keeps one id whatever the interface language
    let id = egui::Id::new("voiceover-settings");
    egui::Window::new(tl!("Voice-Over Record Settings")).id(id).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        egui::Grid::new("voiceover-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tl!("Name:"));
            let r = ui.add(egui::TextEdit::singleline(&mut d.prefs.name).desired_width(220.0));
            elems.push(("voiceover.name".into(), r.rect, d.prefs.name.clone()));
            ui.end_row();
            ui.label(tl!("Source:"));
            let shown = if d.prefs.source.is_empty() { tl!("Default Input").to_string() } else { d.prefs.source.clone() };
            let previous_source = d.prefs.source.clone();
            let cb = egui::ComboBox::from_id_salt("voiceover-source").selected_text(&shown).width(220.0).show_ui(ui, |ui| {
                let mut opts = vec![(String::new(), tl!("Default Input").to_string())];
                opts.extend(d.devices.iter().map(|x| (x.clone(), x.clone())));
                for (i, (val, label)) in opts.into_iter().enumerate() {
                    let r = ui.selectable_label(d.prefs.source == val, &label);
                    elems.push((format!("voiceover.source.{i}"), r.rect, label));
                    if r.clicked() {
                        d.prefs.source = val;
                    }
                }
            });
            elems.push(("voiceover.source".into(), cb.response.rect, shown));
            if d.prefs.source != previous_source {
                let device = if d.prefs.source.is_empty() { app.session.prefs.audio_hardware.default_input.as_str() } else { d.prefs.source.as_str() };
                if let Some(input) = app.session.voiceover.input.as_ref() {
                    d.channels = input.channels(device);
                    d.prefs.input_channel = d.prefs.input_channel.min(u32::from(d.channels.saturating_sub(1)));
                }
            }
            ui.end_row();
            ui.label(tl!("Input:"));
            let n = u32::from(d.channels.clamp(1, 64));
            d.prefs.input_channel = d.prefs.input_channel.min(63);
            let cur = tlf!("Channel {n}", n = d.prefs.input_channel + 1);
            let cb = egui::ComboBox::from_id_salt("voiceover-input").selected_text(&cur).width(220.0).show_ui(ui, |ui| {
                for c in 0..n.max(d.prefs.input_channel + 1) {
                    let label = tlf!("Channel {n}", n = c + 1);
                    let r = ui.selectable_label(d.prefs.input_channel == c, &label);
                    elems.push((format!("voiceover.input.{c}"), r.rect, label));
                    if r.clicked() {
                        d.prefs.input_channel = c;
                    }
                }
            });
            elems.push(("voiceover.input".into(), cb.response.rect, cur));
            ui.end_row();
            ui.label("");
            let r = ui.checkbox(&mut d.prefs.countdown_sound_cues, tl!("Countdown Sound Cues"));
            elems.push(("voiceover.countdown".into(), r.rect, "Countdown Sound Cues".into()));
            ui.end_row();
            ui.label("");
            let r = ui.checkbox(&mut d.prefs.show_countdown, tl!("Show the Countdown in the Program Monitor"));
            elems.push(("voiceover.showCountdown".into(), r.rect, "Show the Countdown in the Program Monitor".into()));
            ui.end_row();
            ui.label(tl!("Pre-roll:"));
            let r = ui.add(egui::DragValue::new(&mut d.prefs.preroll_seconds).speed(0.1).range(0.0..=60.0).suffix(tl!(" seconds")));
            elems.push(("voiceover.preroll".into(), r.rect, format!("{}", d.prefs.preroll_seconds)));
            ui.end_row();
            ui.label(tl!("Post-roll:"));
            let r = ui.add(egui::DragValue::new(&mut d.prefs.postroll_seconds).speed(0.1).range(0.0..=60.0).suffix(tl!(" seconds")));
            elems.push(("voiceover.postroll".into(), r.rect, format!("{}", d.prefs.postroll_seconds)));
            ui.end_row();
            ui.label(tl!("Capture volume:"));
            let r = ui.add(egui::Slider::new(&mut d.prefs.volume_db, -40.0..=20.0).suffix(" dB"));
            elems.push(("voiceover.volume".into(), r.rect, format!("{:.1} dB", d.prefs.volume_db)));
            ui.end_row();
            ui.label(tl!("Sample format:"));
            let fmts = redrob_film_engine::voiceover::SAMPLE_FORMATS;
            let cur = fmts.iter().find(|(k, _)| *k == d.prefs.sample_format).map_or(fmts[0].1, |f| f.1);
            let cb = egui::ComboBox::from_id_salt("voiceover-format").selected_text(crate::i18n::t(cur)).width(220.0).show_ui(ui, |ui| {
                for (k, label) in fmts {
                    let r = ui.selectable_label(d.prefs.sample_format == k, crate::i18n::t(label));
                    elems.push((format!("voiceover.format.{k}"), r.rect, label.to_string()));
                    if r.clicked() {
                        d.prefs.sample_format = k.to_string();
                    }
                }
            });
            elems.push(("voiceover.format".into(), cb.response.rect, cur.to_string()));
            ui.end_row();
        });
        ui.label(
            RichText::new(tl!("Playback starts the pre-roll before the playhead (or the In point); with In/Out marked, recording punches in and out there."))
                .weak()
                .small(),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if app.session.voiceover.recording() {
                let r = ui.button(tl!("Discard take"));
                elems.push(("voiceover.discard".into(), r.rect, "Discard the active take".into()));
                if r.clicked() {
                    discard = true;
                }
            }
            let c = ui.button(tl!("Cancel"));
            elems.push(("voiceover.cancel".into(), c.rect, "Cancel".into()));
            if c.clicked() {
                close = true;
            }
            let o = ui.add(egui::Button::new(RichText::new(tl!("OK")).color(Color32::WHITE)).fill(accent));
            elems.push(("voiceover.ok".into(), o.rect, "OK".into()));
            if o.clicked() {
                apply = true;
            }
        });
    });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if discard {
        match app.session.execute("audio.voiceover.stop", json!({"discard":true})) {
            Ok(_) => {
                if app.playback.playing {
                    app.stop();
                }
                app.ui.status = tl!("Voice-over take discarded").into();
                close = true;
            }
            Err(error) => app.ui.status = error.to_string(),
        }
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if apply {
        let v = &d.prefs;
        let p = json!({
            "source": v.source,
            "inputChannel": v.input_channel,
            "name": v.name,
            "countdownSoundCues": v.countdown_sound_cues,
            "prerollSeconds": v.preroll_seconds,
            "volumeDb": v.volume_db,
            "sampleFormat": v.sample_format,
            "postrollSeconds": v.postroll_seconds,
            "showCountdown": v.show_countdown,
        });
        match app.session.execute("audio.voiceover.settings", p) {
            Ok(_) => close = true,
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    ctx.data_mut(|m| m.insert_temp(draft_id(), if close { None } else { Some(d) }));
}
