//! Advanced SubStation Alpha (`.ass`, and SSA v4 `.ssa` on read).
//!
//! Read: the `[Events]` section's `Dialogue:` lines, fields placed by its `Format:` line (the v4+
//! default order when it is missing). `\N` and `\n` become line breaks, `\h` a no-break space,
//! and override blocks (`{\i1}`…) are dropped, since captions draw with the track style. The
//! `Name` field becomes the speaker. Write: a script with one `Default` style at 1920 × 1080 and
//! one `Dialogue:` line per cue.

use redrob_film_time::{TICKS_PER_SECOND, Tick};

use crate::{Cue, Document, Error, Result};

const CS: i64 = TICKS_PER_SECOND / 100;

/// Parse `H:MM:SS.cc` (centiseconds; more fraction digits are read as a decimal fraction).
fn parse_time(s: &str) -> Option<Tick> {
    let s = s.trim();
    let (hms, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut parts = hms.split(':');
    let h: i64 = parts.next()?.trim().parse().ok()?;
    let m: i64 = parts.next()?.trim().parse().ok()?;
    let sec: i64 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() || !(0..60).contains(&m) || !(0..60).contains(&sec) || !(0..10_000).contains(&h) {
        return None;
    }
    let mut t = ((h * 60 + m) * 60 + sec) * TICKS_PER_SECOND;
    if !frac.is_empty() {
        if frac.len() > 9 || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let d: i64 = frac.parse().ok()?;
        t += (d as i128 * TICKS_PER_SECOND as i128 / 10i128.pow(frac.len() as u32)) as i64;
    }
    Some(Tick(t))
}

/// `H:MM:SS.cc`, rounded to the nearest centisecond.
fn format_time(t: Tick) -> String {
    let cs = (i128::from(t.0.max(0)) + i128::from(CS) / 2) / i128::from(CS);
    let (h, m, s, c) = (cs / 360_000, (cs / 6000) % 60, (cs / 100) % 60, cs % 100);
    format!("{h}:{m:02}:{s:02}.{c:02}")
}

/// Dialogue text → caption text: override blocks dropped, `\N` / `\n` line breaks, `\h` spaces.
fn text_in(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            '\\' => match chars.peek() {
                Some('N') | Some('n') => {
                    chars.next();
                    out.push('\n');
                }
                Some('h') => {
                    chars.next();
                    out.push('\u{a0}');
                }
                _ => out.push('\\'),
            },
            _ => out.push(c),
        }
    }
    out.trim_end().to_string()
}

/// Caption text → dialogue text (line breaks as `\N`, braces kept out of override blocks).
fn text_out(s: &str) -> String {
    s.replace('\r', "").replace('{', "(").replace('}', ")").replace('\n', "\\N")
}

pub fn parse(text: &str) -> Result<Document> {
    let mut doc = Document::default();
    let mut section = String::new();
    let default_fields = ["layer", "start", "end", "style", "name", "marginl", "marginr", "marginv", "effect", "text"];
    let mut fields: Vec<String> = default_fields.iter().map(|s| s.to_string()).collect();
    let mut seen_header = false;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.to_ascii_lowercase();
            seen_header |= section == "[script info]" || section == "[events]";
            continue;
        }
        if section != "[events]" {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else { continue };
        match key.trim().to_ascii_lowercase().as_str() {
            "format" => fields = rest.split(',').map(|f| f.trim().to_ascii_lowercase()).collect(),
            "dialogue" => {
                // the text is the last field and may itself contain commas
                let parts: Vec<&str> = rest.splitn(fields.len().max(1), ',').collect();
                let get = |name: &str| fields.iter().position(|f| f == name).and_then(|i| parts.get(i)).map(|s| s.trim_start());
                let (Some(start), Some(end)) = (get("start").and_then(parse_time), get("end").and_then(parse_time)) else {
                    doc.warnings.push(format!("line {}: a dialogue line without valid times was skipped", n + 1));
                    continue;
                };
                let text = get("text").map(text_in).unwrap_or_default();
                let speaker = get("name").map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
                doc.cues.push(Cue { start, end, text, speaker, id: None, settings: String::new() });
            }
            _ => {}
        }
    }
    if !seen_header {
        return Err(Error::NotFormat("SubStation Alpha"));
    }
    doc.normalize();
    Ok(doc)
}

pub fn write(doc: &Document) -> String {
    let mut s = String::from("[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\nWrapStyle: 0\nScaledBorderAndShadow: yes\n\n");
    s.push_str("[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    s.push_str("Style: Default,Inter,64,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,3,0,2,60,60,50,1\n\n");
    s.push_str("[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    for c in &doc.cues {
        let name = c.speaker.as_deref().unwrap_or("").replace(',', " ");
        s.push_str(&format!("Dialogue: 0,{},{},Default,{name},0,0,0,,{}\n", format_time(c.start), format_time(c.end), text_out(&c.text)));
    }
    s
}
