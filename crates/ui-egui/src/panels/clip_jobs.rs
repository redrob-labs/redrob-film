//! The Project panel item menu's clip jobs and bin-to-timeline entries (Kdenlive's Clip Jobs,
//! Clip in Timeline, Locate Clip, Replace audio/video, Adjust profile, Insert at timecode), and
//! their dialogs: Duplicate with Speed Change, Transcode (with the transcode profile editor) and
//! Custom Clip Jobs. Every action runs an engine command (`clip.stabilizeJob`, `clip.speedJob`,
//! `clip.transcode`, `transcode.profiles.*`, `clipJobs.*`, `clip.instances`,
//! `clip.goToInstance`, `clip.reveal`, `clip.replacePart`, `sequence.matchClip`,
//! `clip.insertAtTimecode`).
//!
//! Automation ids: `project.itemMenu.clipJobs.<stabilize|speed|transcode|custom.<name>|manage>`,
//! `project.itemMenu.clipInTimeline.<n>`, `project.itemMenu.<reveal|replaceAudio|replaceVideo|matchProfile|insertAtTimecode>`;
//! dialog controls `clipJob.<key>`, `clipJob.ok`, `clipJob.cancel`.

use redrob_film_engine::project::{ItemId, ItemKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::FilmcraftApp;

type Actions = Vec<(String, Value)>;

/// The open clip job dialog (`UiState::extras`-like, kept in [`ClipJobsUi`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClipJobDraft {
    /// `speed` | `transcode` | `custom`.
    pub kind: String,
    pub items: Vec<u64>,
    pub speed: f64,
    pub reverse: bool,
    pub pitch: bool,
    pub profile: String,
    pub add_to_project: bool,
    pub replace: bool,
    /// Transcode profile editor: name, export preset and scale of the profile being written.
    pub profile_name: String,
    pub profile_preset: String,
    pub profile_scale: f64,
    /// Custom clip job editor.
    pub job_name: String,
    pub job_program: String,
    pub job_args: String,
    pub job_extension: String,
    pub message: String,
}

/// Frontend state of this module.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipJobsUi {
    pub dialog: Option<ClipJobDraft>,
}

/// The item menu entries (a file media item).
pub fn item_menu(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: ItemId, kind: &ItemKind, actions: &mut Actions) {
    let ItemKind::Media(m) = kind else { return };
    if !matches!(m.media, redrob_film_engine::project::MediaRef::File { .. }) {
        return;
    }
    let has_video = m.info.video.is_some();
    ui.separator();
    // Clip in Timeline: every use, click to go there
    let uses = app.session.execute("clip.instances", json!({"item": id.0})).ok().and_then(|v| v["instances"].as_array().cloned()).unwrap_or_default();
    ui.add_enabled_ui(!uses.is_empty(), |ui| {
        ui.menu_button(tl!("Clip in Timeline"), |ui| {
            for (n, u) in uses.iter().enumerate() {
                let label = format!("{} · {} · {}", u["sequenceName"].as_str().unwrap_or(""), u["track"].as_str().unwrap_or(""), u["frame"].as_i64().unwrap_or(0));
                let r = ui.button(&label);
                app.auto.add(&format!("project.itemMenu.clipInTimeline.{n}"), r.rect, &label);
                if r.clicked() {
                    actions.push(("clip.goToInstance".into(), json!({"clip": u["clip"]})));
                    ui.close();
                }
            }
        });
    });
    let seq = app.session.state.active_sequence.is_some();
    let custom: Vec<String> =
        app.session.execute("clipJobs.list", json!({})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().filter_map(|j| j["name"].as_str().map(str::to_string)).collect();
    let mut entry = |ui: &mut egui::Ui, key: &str, label: &str, cmd: &str, p: Value, enabled: bool| {
        let r = ui.add_enabled(enabled, egui::Button::new(label));
        app.auto.add(&format!("project.itemMenu.{key}"), r.rect, label);
        if r.clicked() {
            actions.push((cmd.into(), p));
            ui.close();
        }
    };
    entry(ui, "reveal", tl!("Reveal in File Manager"), "clip.reveal", json!({"item": id.0}), true);
    entry(ui, "replaceAudio", tl!("Replace Audio Part…"), "clip.replacePart", json!({"item": id.0, "part": "audio"}), true);
    entry(ui, "replaceVideo", tl!("Replace Video Part…"), "clip.replacePart", json!({"item": id.0, "part": "video"}), has_video);
    entry(ui, "matchProfile", tl!("Adjust Profile to Current Clip"), "sequence.matchClip", json!({"item": id.0}), has_video && seq);
    entry(ui, "insertAtTimecode", tl!("Insert at Timecode"), "clip.insertAtTimecode", json!({"item": id.0}), seq && m.info.start_timecode.is_some());
    let mut open: Option<ClipJobDraft> = None;
    ui.menu_button(tl!("Clip Jobs"), |ui| {
        let r = ui.add_enabled(has_video, egui::Button::new(tl!("Stabilize")));
        app.auto.add("project.itemMenu.clipJobs.stabilize", r.rect, "Stabilize");
        if r.clicked() {
            actions.push(("clip.stabilizeJob".into(), json!({"items": [id.0]})));
            ui.close();
        }
        for (key, label) in [("speed", tl!("Duplicate with Speed Change…")), ("transcode", tl!("Transcode…"))] {
            let r = ui.button(label);
            app.auto.add(&format!("project.itemMenu.clipJobs.{key}"), r.rect, label);
            if r.clicked() {
                open = Some(ClipJobDraft::new(key, id.0));
                ui.close();
            }
        }
        if !custom.is_empty() {
            ui.separator();
        }
        for name in &custom {
            let r = ui.button(name);
            app.auto.add(&format!("project.itemMenu.clipJobs.custom.{name}"), r.rect, name);
            if r.clicked() {
                actions.push(("clipJobs.run".into(), json!({"name": name, "items": [id.0]})));
                ui.close();
            }
        }
        ui.separator();
        let r = ui.button(tl!("Configure Clip Jobs…"));
        app.auto.add("project.itemMenu.clipJobs.manage", r.rect, "Configure Clip Jobs…");
        if r.clicked() {
            open = Some(ClipJobDraft::new("custom", id.0));
            ui.close();
        }
    });
    if let Some(d) = open {
        app.ui.clip_jobs.dialog = Some(d);
    }
}

impl ClipJobDraft {
    pub fn new(kind: &str, item: u64) -> Self {
        ClipJobDraft {
            kind: kind.into(),
            items: vec![item],
            speed: 50.0,
            reverse: false,
            pitch: true,
            profile: "ProRes 422 LT (edit friendly)".into(),
            add_to_project: true,
            profile_scale: 1.0,
            job_args: "-i {input} {output}".into(),
            job_extension: "mov".into(),
            ..Default::default()
        }
    }
}

fn push(app: &mut FilmcraftApp, id: &str, r: &egui::Response, label: &str) {
    app.auto.add(&format!("clipJob.{id}"), r.rect, label);
}

/// Draw the open clip job dialog.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.clip_jobs.dialog.clone() else { return };
    let mut keep = true;
    let mut run: Option<(&'static str, Value)> = None;
    let title = match d.kind.as_str() {
        "speed" => tl!("Duplicate with Speed Change"),
        "transcode" => tl!("Transcode Clips"),
        _ => tl!("Custom Clip Jobs"),
    };
    let profiles: Vec<Value> = app.session.execute("transcode.profiles.list", json!({})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let presets: Vec<String> = app.session.export_presets.all().into_iter().map(|p| p.name).collect();
    let jobs: Vec<Value> = app.session.execute("clipJobs.list", json!({})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    egui::Window::new(title).id(egui::Id::new("clip-job-dialog")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        match d.kind.as_str() {
            "speed" => {
                ui.horizontal(|ui| {
                    ui.label(tl!("Speed"));
                    let r = ui.add(egui::DragValue::new(&mut d.speed).range(1.0..=10_000.0).suffix(" %"));
                    push(app, "speed", &r, "Speed");
                });
                let r = ui.checkbox(&mut d.reverse, tl!("Reverse"));
                push(app, "reverse", &r, "Reverse");
                let r = ui.checkbox(&mut d.pitch, tl!("Pitch compensation"));
                push(app, "pitch", &r, "Pitch compensation");
                if ui.button(tl!("Create")).clicked() {
                    run = Some(("clip.speedJob", json!({"items": d.items, "speed": d.speed, "reverse": d.reverse, "pitchCompensation": d.pitch})));
                }
            }
            "transcode" => {
                ui.label(egui::RichText::new(tl!("Profile")).strong());
                for (i, p) in profiles.iter().enumerate() {
                    let name = p["name"].as_str().unwrap_or("").to_string();
                    ui.horizontal(|ui| {
                        let r = ui.radio(d.profile == name, crate::i18n::t(&name));
                        push(app, &format!("profile.{i}"), &r, &name);
                        if r.clicked() {
                            d.profile = name.clone();
                        }
                        ui.label(egui::RichText::new(format!("{} · ×{}", crate::i18n::t(p["preset"].as_str().unwrap_or("")), p["scale"])).weak());
                        if !p["builtin"].as_bool().unwrap_or(true) && ui.small_button(tl!("Delete")).clicked() {
                            run = Some(("transcode.profiles.delete", json!({"name": name})));
                        }
                    });
                }
                let r = ui.checkbox(&mut d.add_to_project, tl!("Add the result to the project"));
                push(app, "addToProject", &r, "Add the result to the project");
                let r = ui.checkbox(&mut d.replace, tl!("Replace the original clip"));
                push(app, "replace", &r, "Replace the original clip");
                if ui.button(tl!("Transcode")).clicked() {
                    run = Some(("clip.transcode", json!({"items": d.items, "profile": d.profile, "addToProject": d.add_to_project, "replace": d.replace})));
                }
                ui.separator();
                // the profile editor (Settings ▸ Transcode)
                ui.label(egui::RichText::new(tl!("New or changed profile")).strong());
                ui.horizontal(|ui| {
                    ui.label(tl!("Name"));
                    let r = ui.text_edit_singleline(&mut d.profile_name);
                    push(app, "profileName", &r, "Name");
                });
                ui.horizontal(|ui| {
                    ui.label(tl!("Export preset"));
                    egui::ComboBox::from_id_salt("clipJob.profilePreset").selected_text(crate::i18n::t(&d.profile_preset)).width(240.0).show_ui(ui, |ui| {
                        for p in &presets {
                            if ui.selectable_label(*p == d.profile_preset, crate::i18n::t(p)).clicked() {
                                d.profile_preset = p.clone();
                            }
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(tl!("Scale"));
                    let r = ui.add(egui::DragValue::new(&mut d.profile_scale).range(0.05..=4.0).speed(0.01));
                    push(app, "profileScale", &r, "Scale");
                    if ui.add_enabled(!d.profile_name.trim().is_empty() && !d.profile_preset.is_empty(), egui::Button::new(tl!("Save Profile"))).clicked() {
                        run = Some(("transcode.profiles.save", json!({"name": d.profile_name, "preset": d.profile_preset, "scale": d.profile_scale})));
                    }
                });
            }
            _ => {
                ui.label(egui::RichText::new(tl!("Saved jobs")).strong());
                if jobs.is_empty() {
                    ui.label(egui::RichText::new(tl!("No custom clip jobs yet.")).weak());
                }
                for j in &jobs {
                    let name = j["name"].as_str().unwrap_or("").to_string();
                    ui.horizontal(|ui| {
                        ui.label(&name);
                        ui.label(egui::RichText::new(format!("{} {}", j["program"].as_str().unwrap_or(""), j["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default())).weak());
                        if ui.small_button(tl!("Run")).clicked() {
                            run = Some(("clipJobs.run", json!({"name": name, "items": d.items})));
                        }
                        if ui.small_button(tl!("Edit")).clicked() {
                            d.job_name = name.clone();
                            d.job_program = j["program"].as_str().unwrap_or("").into();
                            d.job_args = j["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default();
                            d.job_extension = j["extension"].as_str().unwrap_or("").into();
                        }
                        if ui.small_button(tl!("Delete")).clicked() {
                            run = Some(("clipJobs.delete", json!({"name": name})));
                        }
                    });
                }
                ui.separator();
                for (key, label, v) in [
                    ("jobName", tl!("Name"), &mut d.job_name),
                    ("jobProgram", tl!("Program"), &mut d.job_program),
                    ("jobArgs", tl!("Arguments ({input}, {output})"), &mut d.job_args),
                    ("jobExtension", tl!("Output extension"), &mut d.job_extension),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        let r = ui.text_edit_singleline(v);
                        push(app, key, &r, label);
                    });
                }
                if ui.button(tl!("Save Job")).clicked() {
                    let args: Vec<&str> = d.job_args.split_whitespace().collect();
                    run = Some(("clipJobs.save", json!({"name": d.job_name, "program": d.job_program, "args": args, "extension": d.job_extension})));
                }
            }
        }
        if !d.message.is_empty() {
            ui.label(egui::RichText::new(&d.message).weak());
        }
        ui.separator();
        let r = ui.button(tl!("Close"));
        push(app, "cancel", &r, "Close");
        if r.clicked() {
            keep = false;
        }
    });
    if let Some((cmd, p)) = run {
        match crate::menus::invoke(app, ctx, cmd, p) {
            Ok(v) => {
                d.message = match cmd {
                    "clip.speedJob" | "clip.transcode" => {
                        keep = false;
                        tlf!("Started job {job}", job = v["job"])
                    }
                    "clipJobs.run" => {
                        keep = false;
                        tlf!("Started job {job}", job = v["job"])
                    }
                    _ => tl!("Saved").to_string(),
                };
                app.ui.status = d.message.clone();
            }
            Err(e) => d.message = e,
        }
    }
    app.ui.clip_jobs.dialog = (keep && !ctx.input(|i| i.key_pressed(egui::Key::Escape))).then_some(d);
}
