//! Interchange documents (CMX 3600 EDL, FCP7 XML, FCPXML, OTIO, AAF, OMF; Premiere Pro projects,
//! import only) ↔ the session's project.

use serde_json::{Value, json};

use redrob_film_interchange::{ExportOptions, Format, ImportOptions};
use redrob_film_project::{ItemId, ItemKind, MediaRef};

use crate::{EngineError, Result, Session};

/// The interchange format of a file, if it is one (by content, with the extension as a hint).
pub fn detect(path: &str, bytes: &[u8]) -> Option<Format> {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if !matches!(ext.as_deref(), Some("edl" | "xml" | "fcpxml" | "otio" | "aaf" | "omf" | "omfi" | "prproj")) {
        return None;
    }
    redrob_film_interchange::detect(bytes, ext.as_deref())
}

fn report_json(r: &redrob_film_interchange::Report) -> Vec<String> {
    r.entries.iter().map(|e| if e.count > 1 { format!("{} (×{})", e.message, e.count) } else { e.message.clone() }).collect()
}

fn is_file_media(item: &redrob_film_project::ProjectItem) -> bool {
    matches!(&item.kind, ItemKind::Media(m) if matches!(m.media, MediaRef::File { .. }))
}

/// Import a document: merge its bins, media and sequences into the project (one undo step), then
/// link each media file that exists on disk. Returns the new sequences.
pub fn import(s: &mut Session, path: &str, bytes: &[u8], format: Format) -> Result<Value> {
    let p = std::path::Path::new(path);
    let opts = ImportOptions {
        base_dir: p.parent().map(|d| d.to_string_lossy().to_string()),
        name: p.file_stem().map(|n| n.to_string_lossy().to_string()),
        ..Default::default()
    };
    let (fragment, report) = match format {
        // AAF / OMF may embed audio: write it next to the document (the media items point there)
        Format::Aaf | Format::Omf => {
            let r = if format == Format::Aaf { redrob_film_interchange::aaf::import(bytes, &opts) } else { redrob_film_interchange::omf::import(bytes, &opts) };
            let (fragment, extracted, report) = r.map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
            for m in extracted {
                if let Some(dir) = std::path::Path::new(&m.path).parent().filter(|_| !s.services.export_in_memory()) {
                    let _ = std::fs::create_dir_all(dir);
                }
                s.services.write_file(&m.path, &m.wav).map_err(|e| EngineError::Other(format!("{}: {e}", m.path)))?;
            }
            (fragment, report)
        }
        _ => redrob_film_interchange::import_with(bytes, format, &opts).map_err(|e| EngineError::Other(format!("{path}: {e}")))?,
    };
    let before: std::collections::HashSet<ItemId> = s.project.items.keys().copied().collect();
    let name = opts.name.clone().unwrap_or_else(|| format.name().to_string());
    let seqs = s.edit(&format!("Import {name}"), |proj, _| {
        let seqs = redrob_film_interchange::merge_into(proj, fragment, None);
        // A document that sets only some Motion parameters leaves the others at their "auto"
        // (NaN) defaults. Resolve them as placing a clip does, for every format. A media file's
        // picture size is only what the document says so far (an EDL says nothing: the importer
        // assumes the sequence size), so its clips' anchors wait for Link Media below.
        proj.resolve_placed_auto_points(|seq| !before.contains(&seq), |source| !is_file_media(source));
        Ok(seqs)
    })?;
    // Link media: probe each new file-backed item that exists, through the host's reader as a
    // direct import does (only the container index is read now, not the whole file: #461).
    let new_media: Vec<(ItemId, String)> = s
        .project
        .items
        .values()
        .filter(|i| !before.contains(&i.id))
        .filter_map(|i| match &i.kind {
            ItemKind::Media(m) => match &m.media {
                MediaRef::File { path } => Some((i.id, path.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let mut linked = 0;
    let mut offline = Vec::new();
    for (id, mpath) in new_media {
        match s.media.open_file(&mpath, &*s.services).ok() {
            Some(src) => {
                let info = src.info().clone();
                let identity = crate::relink::identity_of(&*s.services, &mpath).ok();
                let rebase = (format == Format::Edl).then_some(info.start_timecode).flatten();
                s.edit("Link Media", |proj, _| {
                    if let Some(item) = proj.items.get_mut(&id)
                        && let ItemKind::Media(m) = &mut item.kind
                    {
                        let rate = info.video.as_ref().map(|v| v.frame_rate);
                        m.info = info.clone();
                        m.offline = false;
                        m.identity = identity;
                        if let (Some(tc), Some(rate)) = (rebase, rate) {
                            redrob_film_interchange::rebase_source_timecode(proj, id, tc, rate);
                        }
                    }
                    // the picture size is now the file's own: centre the anchors of its clips in it
                    proj.resolve_placed_auto_points(|seq| !before.contains(&seq), |source| source.id == id);
                    Ok(())
                })?;
                s.media.insert_file(id, &mpath, src);
                linked += 1;
            }
            None => offline.push(mpath),
        }
    }
    // What did not carry over goes to the Events panel, one line per kind (the frontend says how
    // many there were): an opened Premiere project must never lose things silently.
    let doc_name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
    for e in &report.entries {
        let level = match e.level {
            redrob_film_interchange::Level::Warning => crate::panels::Level::Warning,
            redrob_film_interchange::Level::Info => crate::panels::Level::Info,
        };
        let msg = if e.count > 1 { format!("{} (×{})", e.message, e.count) } else { e.message.clone() };
        s.log.push(level, "import", format!("{doc_name}: {msg}"));
    }
    let warnings: u64 = report.warnings().map(|e| u64::from(e.count)).sum();
    if let Some(&first) = seqs.first() {
        s.state.active_sequence = Some(first);
        if !s.state.open_sequences.contains(&first) {
            s.state.open_sequences.push(first);
        }
    }
    Ok(json!({
        "format": format.name(),
        "sequences": seqs.iter().map(|i| i.0).collect::<Vec<_>>(),
        "linkedMedia": linked,
        "offlineMedia": offline,
        "report": report_json(&report),
        "warnings": warnings,
    }))
}

/// File ▸ Open of a Premiere Pro project: a new, unsaved project holding everything the `.prproj`
/// carries (saving asks for a `.rfilm` path; the Premiere file is never written).
pub fn open_premiere(s: &mut Session, path: &str, bytes: &[u8]) -> Result<Value> {
    let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Premiere Project".into());
    s.execute("file.newProject", json!({ "name": name }))?;
    let mut r = import(s, path, bytes, Format::Prproj)?;
    // the import's merge is not something to undo back to an empty project
    s.history = Default::default();
    s.history.limit = 200;
    if let Some(seq) = s.state.active_sequence {
        s.events.push(crate::Event::OpenSequence(seq));
    }
    r["path"] = Value::Null;
    r["importedFrom"] = json!(path);
    Ok(r)
}

/// `file.exportInterchange {format: "edl"|"xml"|"fcpxml"|"otio"|"aaf"|"omf" = "xml", path, sequence?}`
pub fn export(s: &mut Session, p: &Value) -> Result<Value> {
    // FCP7 XML only when no format is asked for: a format we don't know is an error, not XML.
    // The names are the documented ones (each format's extension, in either case) and nothing
    // else, so the error can list exactly what is accepted.
    let format = match p.get("format") {
        None | Some(Value::Null) => Format::Fcp7Xml,
        Some(v) => v.as_str().and_then(|name| Format::ALL.into_iter().find(|f| f.extension().eq_ignore_ascii_case(name))).ok_or_else(|| {
            let names: Vec<&str> = Format::ALL.iter().map(|f| f.extension()).collect();
            crate::commands::bad("file.exportInterchange", format!("unknown format {v}: use one of {}", names.join(", ")))
        })?,
    };
    let path = p.get("path").and_then(Value::as_str).ok_or_else(|| EngineError::Other("need `path`".into()))?.to_string();
    let seq = p.get("sequence").and_then(Value::as_u64).map(ItemId).or(s.state.active_sequence).ok_or(EngineError::NoSequence)?;
    let opts = ExportOptions {
        relative_to: std::path::Path::new(&path).parent().map(|d| d.to_string_lossy().to_string()),
        name: s.project.item(seq).map(|i| i.name.clone()),
        ..Default::default()
    };
    let (bytes, report) = redrob_film_interchange::export(&s.project, seq, format, &opts).map_err(|e| EngineError::Other(e.to_string()))?;
    s.services.write_file(&path, &bytes).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"path": path, "bytes": bytes.len(), "report": report_json(&report)}))
}
