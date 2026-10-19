//! YouTube SubViewer (`.sbv`): blocks of `H:MM:SS.mmm,H:MM:SS.mmm` then the text lines, separated
//! by a blank line.

use redrob_film_time::Tick;

use crate::{Cue, Document, Error, Result, TICKS_PER_MS, parse_clock};

fn timing(line: &str) -> Option<(Tick, Tick)> {
    let (a, b) = line.trim().split_once(',')?;
    Some((parse_clock(a)?, parse_clock(b)?))
}

/// `H:MM:SS.mmm`, rounded to the nearest millisecond.
fn format_time(t: Tick) -> String {
    let ms = (i128::from(t.0.max(0)) + i128::from(TICKS_PER_MS) / 2) / i128::from(TICKS_PER_MS);
    let (h, m, s, f) = (ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000);
    format!("{h}:{m:02}:{s:02}.{f:03}")
}

/// Whether the text looks like SBV (a timing line among the first lines).
pub fn sniff(text: &str) -> bool {
    text.lines().take(10).any(|l| timing(l).is_some())
}

pub fn parse(text: &str) -> Result<Document> {
    let mut doc = Document::default();
    let mut cur: Option<Cue> = None;
    for line in text.lines() {
        if let Some((start, end)) = timing(line) {
            if let Some(c) = cur.take() {
                doc.cues.push(c);
            }
            cur = Some(Cue { start, end, ..Default::default() });
            continue;
        }
        if line.trim().is_empty() {
            if let Some(c) = cur.take() {
                doc.cues.push(c);
            }
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        if !c.text.is_empty() {
            c.text.push('\n');
        }
        c.text.push_str(line.trim_end());
    }
    if let Some(c) = cur.take() {
        doc.cues.push(c);
    }
    if doc.cues.is_empty() && !text.trim().is_empty() {
        return Err(Error::NotFormat("SubViewer (SBV)"));
    }
    doc.normalize();
    Ok(doc)
}

pub fn write(doc: &Document) -> String {
    let mut s = String::new();
    for (i, c) in doc.cues.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        s.push_str(&format!("{},{}\n{}\n", format_time(c.start), format_time(c.end), c.text));
    }
    s
}
