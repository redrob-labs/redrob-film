//! GPS Text and GPS Graphic: overlays driven by a recorded GPS track (.gpx or .tcx).
//!
//! The track is matched to the picture by wall-clock time: the frame's time comes from the media
//! file ([`crate::vfx::FxEnv::media_datetime_ms`]); "Time Offset" shifts it (positive when the GPS
//! clock is ahead). When the clip's recording time is unknown (generated media, no environment)
//! the track starts with the clip.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use redrob_film_geom::Affine;
use redrob_film_media::MediaSource;

use super::text::{TextBox, draw_text_box};
use super::*;

// ------------------------------------------------------------------ track

#[derive(Clone, Copy, Debug, Default)]
struct Pt {
    /// Milliseconds since 1970 (UTC).
    t: f64,
    lat: f64,
    lon: f64,
    ele: Option<f64>,
    hr: Option<f64>,
    // derived
    dist: f64,
    speed: f64,
    bearing: f64,
    gain: f64,
    loss: f64,
    up: f64,
    down: f64,
    flat: f64,
}

#[derive(Debug, Default)]
struct Track {
    pts: Vec<Pt>,
}

/// Days since 1970-01-01 of a civil date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `(year, month, day)` of days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// ISO 8601 date-time (`2024-05-01T12:30:05.250Z`, `…+02:00`) → ms since 1970.
fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |a: usize, n: usize| -> Option<i64> { s.get(a..a + n)?.parse().ok() };
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, se) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    let mut i = 19;
    let mut frac = 0.0;
    if b.get(i) == Some(&b'.') {
        let st = i + 1;
        i = st;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        frac = format!("0.{}", &s[st..i]).parse().unwrap_or(0.0);
    }
    let mut off = 0i64;
    if let Some(&c) = b.get(i)
        && (c == b'+' || c == b'-')
    {
        let oh = num(i + 1, 2).unwrap_or(0);
        let om = if b.get(i + 3) == Some(&b':') { num(i + 4, 2) } else { num(i + 3, 2) }.unwrap_or(0);
        off = (oh * 60 + om) * if c == b'-' { -1 } else { 1 };
    }
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - off * 60;
    Some((secs as f64 + frac) * 1000.0)
}

/// The text of the first element whose local name is `name` (any namespace prefix).
fn tag<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(p) = block[from..].find(name) {
        let at = from + p;
        from = at + name.len();
        let before = block[..at].chars().last();
        let after = block[at + name.len()..].chars().next();
        if matches!(before, Some('<') | Some(':')) && matches!(after, Some('>') | Some(' ')) {
            // the opening tag must start with '<' (possibly with a prefix)
            let open = block[..at].rfind('<')?;
            if block[open..at].starts_with("</") {
                continue;
            }
            let gt = at + block[at..].find('>')?;
            let end = gt + 1 + block[gt + 1..].find("</")?;
            return Some(block[gt + 1..end].trim());
        }
    }
    None
}

fn attr(block: &str, name: &str) -> Option<f64> {
    let k = format!("{name}=");
    let p = block.find(&k)? + k.len();
    let q = block[p..].chars().next()?;
    let rest = &block[p + 1..];
    let end = rest.find(q)?;
    rest[..end].trim().parse().ok()
}

fn parse_track(text: &str) -> Vec<Pt> {
    let mut pts = Vec::new();
    // GPX <trkpt lat lon>…</trkpt> (also route / way points if there is no track)
    for open in ["<trkpt", "<rtept"] {
        let mut from = 0;
        while let Some(p) = text[from..].find(open) {
            let a = from + p;
            let close = open.replace('<', "</");
            let end = text[a..].find(close.as_str()).map(|e| a + e).unwrap_or(text.len());
            let head_end = a + text[a..].find('>').unwrap_or(0);
            let head = &text[a..head_end.max(a)];
            let block = &text[a..end];
            from = end.max(a + 1);
            let (Some(lat), Some(lon)) = (attr(head, "lat"), attr(head, "lon")) else { continue };
            let Some(t) = tag(block, "time").and_then(parse_time) else { continue };
            pts.push(Pt {
                t,
                lat,
                lon,
                ele: tag(block, "ele").and_then(|v| v.parse().ok()),
                hr: tag(block, "hr").and_then(|v| v.parse().ok()),
                ..Default::default()
            });
        }
        if !pts.is_empty() {
            break;
        }
    }
    // TCX <Trackpoint>…</Trackpoint>
    if pts.is_empty() {
        let mut from = 0;
        while let Some(p) = text[from..].find("<Trackpoint>") {
            let a = from + p;
            let end = text[a..].find("</Trackpoint>").map(|e| a + e).unwrap_or(text.len());
            let block = &text[a..end];
            from = end.max(a + 1);
            let (Some(lat), Some(lon)) = (tag(block, "LatitudeDegrees").and_then(|v| v.parse::<f64>().ok()), tag(block, "LongitudeDegrees").and_then(|v| v.parse::<f64>().ok()))
            else {
                continue;
            };
            let Some(t) = tag(block, "Time").and_then(parse_time) else { continue };
            let hr = block.find("HeartRateBpm").and_then(|i| tag(&block[i..], "Value")).and_then(|v| v.parse::<f64>().ok());
            pts.push(Pt { t, lat, lon, ele: tag(block, "AltitudeMeters").and_then(|v| v.parse().ok()), hr, ..Default::default() });
        }
    }
    pts.sort_by(|a, b| a.t.total_cmp(&b.t));
    pts
}

fn haversine(a: &Pt, b: &Pt) -> f64 {
    let r = 6_371_000.0;
    let (la1, la2) = (a.lat.to_radians(), b.lat.to_radians());
    let dl = la2 - la1;
    let dn = (b.lon - a.lon).to_radians();
    let h = (dl / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dn / 2.0).sin().powi(2);
    2.0 * r * h.sqrt().min(1.0).asin()
}

fn fill_missing(pts: &mut [Pt], get: fn(&Pt) -> Option<f64>, set: fn(&mut Pt, f64)) {
    let known: Vec<(usize, f64)> = pts.iter().enumerate().filter_map(|(i, p)| get(p).map(|v| (i, v))).collect();
    if known.is_empty() {
        return;
    }
    for i in 0..pts.len() {
        if get(&pts[i]).is_some() {
            continue;
        }
        let after = known.iter().position(|(k, _)| *k > i);
        let v = match after {
            Some(0) => known[0].1,
            Some(j) => {
                let ((i0, v0), (i1, v1)) = (known[j - 1], known[j]);
                v0 + (v1 - v0) * (i - i0) as f64 / (i1 - i0) as f64
            }
            None => known[known.len() - 1].1,
        };
        set(&mut pts[i], v);
    }
}

/// Smooth positions over `level` points and compute the derived fields.
fn derive(raw: &[Pt], level: usize) -> Vec<Pt> {
    let mut pts = raw.to_vec();
    if level == 0 {
        return pts;
    }
    fill_missing(&mut pts, |p| p.ele, |p, v| p.ele = Some(v));
    fill_missing(&mut pts, |p| p.hr, |p, v| p.hr = Some(v));
    if level > 1 {
        let half = level / 2;
        let src = pts.clone();
        for i in 0..pts.len() {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(src.len()));
            let n = (b - a) as f64;
            pts[i].lat = src[a..b].iter().map(|p| p.lat).sum::<f64>() / n;
            pts[i].lon = src[a..b].iter().map(|p| p.lon).sum::<f64>() / n;
            if src[a..b].iter().all(|p| p.ele.is_some()) {
                pts[i].ele = Some(src[a..b].iter().filter_map(|p| p.ele).sum::<f64>() / n);
            }
        }
    }
    for i in 1..pts.len() {
        let (p, c) = (pts[i - 1], pts[i]);
        let d = haversine(&p, &c);
        let dt = ((c.t - p.t) / 1000.0).max(1e-3);
        let de = match (p.ele, c.ele) {
            (Some(a), Some(b)) => b - a,
            _ => 0.0,
        };
        let y = (c.lon - p.lon).to_radians().sin() * c.lat.to_radians().cos();
        let x = p.lat.to_radians().cos() * c.lat.to_radians().sin() - p.lat.to_radians().sin() * c.lat.to_radians().cos() * (c.lon - p.lon).to_radians().cos();
        let cur = &mut pts[i];
        cur.dist = p.dist + d;
        cur.speed = d / dt;
        cur.bearing = if d > 0.01 { y.atan2(x).to_degrees().rem_euclid(360.0) } else { p.bearing };
        cur.gain = p.gain + de.max(0.0);
        cur.loss = p.loss + (-de).max(0.0);
        let grade = if d > 0.01 { de / d } else { 0.0 };
        cur.up = p.up + if grade > 0.02 { d } else { 0.0 };
        cur.down = p.down + if grade < -0.02 { d } else { 0.0 };
        cur.flat = p.flat + if grade.abs() <= 0.02 { d } else { 0.0 };
    }
    if pts.len() > 1 {
        pts[0].speed = pts[1].speed;
        pts[0].bearing = pts[1].bearing;
    }
    pts
}

type TrackCache = Mutex<HashMap<(String, usize), (Option<std::time::SystemTime>, Arc<Track>)>>;

fn load(path: &str, level: usize) -> Option<Arc<Track>> {
    static CACHE: OnceLock<TrackCache> = OnceLock::new();
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
    let key = (path.to_string(), level);
    let cache = CACHE.get_or_init(Default::default);
    if let Some((t, tr)) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key)
        && *t == mtime
    {
        return Some(tr.clone());
    }
    let text = std::fs::read_to_string(path).ok()?;
    let raw = parse_track(&text);
    if raw.is_empty() {
        return None;
    }
    let tr = Arc::new(Track { pts: derive(&raw, level) });
    let mut c = cache.lock().unwrap_or_else(|p| p.into_inner());
    if c.len() > 16 {
        c.clear();
    }
    c.insert(key, (mtime, tr.clone()));
    Some(tr)
}

/// The GPS time (ms) the current frame matches.
fn gps_now(tr: &Track, cx: &FxCtx, offset_s: f64) -> f64 {
    let video = cx.env.and_then(|e| e.media_datetime_ms());
    match video {
        Some(v) => v + offset_s * 1000.0,
        None => tr.pts.first().map_or(0.0, |p| p.t) + (cx.seconds + offset_s) * 1000.0,
    }
}

/// The track interpolated at time `t` (and the index of the point before it).
fn at(tr: &Track, t: f64) -> (Pt, usize) {
    let p = &tr.pts;
    let i = p.partition_point(|q| q.t <= t);
    if i == 0 {
        return (p[0], 0);
    }
    if i >= p.len() {
        return (p[p.len() - 1], p.len() - 1);
    }
    let (a, b) = (p[i - 1], p[i]);
    let k = ((t - a.t) / (b.t - a.t).max(1e-9)).clamp(0.0, 1.0);
    let l = |x: f64, y: f64| x + (y - x) * k;
    let lo = |x: Option<f64>, y: Option<f64>| match (x, y) {
        (Some(x), Some(y)) => Some(l(x, y)),
        (x, y) => x.or(y),
    };
    (
        Pt {
            t,
            lat: l(a.lat, b.lat),
            lon: l(a.lon, b.lon),
            ele: lo(a.ele, b.ele),
            hr: lo(a.hr, b.hr),
            dist: l(a.dist, b.dist),
            speed: l(a.speed, b.speed),
            bearing: b.bearing,
            gain: l(a.gain, b.gain),
            loss: l(a.loss, b.loss),
            up: l(a.up, b.up),
            down: l(a.down, b.down),
            flat: l(a.flat, b.flat),
        },
        i - 1,
    )
}

// ------------------------------------------------------------------ GPS Text

fn dist_unit(v: f64, f: &str) -> String {
    let f = f.trim().to_ascii_lowercase();
    match f.as_str() {
        "km" | "kilometer" => format!("{:.2}", v / 1000.0),
        "mi" | "mile" => format!("{:.2}", v / 1609.344),
        "ft" | "feet" => format!("{:.0}", v * 3.280_84),
        "nm" | "nautical" => format!("{:.2}", v / 1852.0),
        _ => format!("{v:.0}"),
    }
}
fn speed_unit(v: f64, f: &str) -> String {
    let f = f.trim().to_ascii_lowercase();
    match f.as_str() {
        "ms" | "m/s" | "meters" => format!("{v:.1}"),
        "mih" | "mi/h" | "mileh" => format!("{:.1}", v * 2.236_936),
        "fts" | "ft/s" | "feet" => format!("{:.1}", v * 3.280_84),
        "kn" | "nm/h" | "knots" => format!("{:.1}", v * 1.943_844),
        _ => format!("{:.1}", v * 3.6),
    }
}

/// A strftime-style format of `ms` since 1970 (UTC): %Y %y %m %d %H %I %M %S %p %F %T %%.
pub(crate) fn strftime(ms: f64, fmt: &str) -> String {
    let secs = (ms / 1000.0).floor() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem / 60) % 60, rem % 60);
    let mut out = String::new();
    let mut it = fmt.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('Y') => out += &format!("{y:04}"),
            Some('y') => out += &format!("{:02}", y.rem_euclid(100)),
            Some('m') => out += &format!("{m:02}"),
            Some('d') => out += &format!("{d:02}"),
            Some('H') => out += &format!("{hh:02}"),
            Some('I') => out += &format!("{:02}", if hh % 12 == 0 { 12 } else { hh % 12 }),
            Some('M') => out += &format!("{mm:02}"),
            Some('S') => out += &format!("{ss:02}"),
            Some('p') => out += if hh < 12 { "AM" } else { "PM" },
            Some('F') => out += &format!("{y:04}-{m:02}-{d:02}"),
            Some('T') => out += &format!("{hh:02}:{mm:02}:{ss:02}"),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

const COMPASS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];

fn keyword_value(kw: &str, p: &Pt, gps_t: f64, vid_t: f64) -> String {
    let mut parts = kw.trim().splitn(2, ' ');
    let name = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    let time_fmt = |base: f64| {
        // trailing "+3600" / "-60" shifts the time; the rest is the format
        let (mut fmt, mut shift) = (rest.to_string(), 0.0);
        if let Some(last) = rest.split_whitespace().last()
            && (last.starts_with('+') || last.starts_with('-'))
            && let Ok(v) = last.parse::<f64>()
        {
            shift = v;
            fmt = rest[..rest.len() - last.len()].trim().to_string();
        }
        strftime(base + shift * 1000.0, if fmt.is_empty() { "%Y-%m-%d %H:%M:%S" } else { fmt.as_str() })
    };
    match name {
        "gps_lat" => format!("{:.6}", p.lat),
        "gps_lon" => format!("{:.6}", p.lon),
        "gps_elev" => p.ele.map(|v| dist_unit(v, rest)).unwrap_or_else(|| "-".into()),
        "gps_speed" => speed_unit(p.speed, rest),
        "gps_dist" => dist_unit(p.dist, rest),
        "gps_datetime_now" => time_fmt(gps_t),
        "vid_datetime_now" => time_fmt(vid_t),
        "gps_hr" => p.hr.map(|v| format!("{v:.0}")).unwrap_or_else(|| "-".into()),
        "gps_bearing" => format!("{:.0}", p.bearing),
        "gps_compass" => COMPASS[(((p.bearing + 22.5).rem_euclid(360.0)) / 45.0) as usize % 8].to_string(),
        "gps_vdist_up" => dist_unit(p.gain, rest),
        "gps_vdist_down" => dist_unit(p.loss, rest),
        "gps_dist_uphill" => dist_unit(p.up, rest),
        "gps_dist_downhill" => dist_unit(p.down, rest),
        "gps_dist_flat" => dist_unit(p.flat, rest),
        _ => format!("#{kw}#"),
    }
}

/// Replace `#keyword …#` in `text` (`\#` is a literal '#').
fn expand(text: &str, p: &Pt, gps_t: f64, vid_t: f64) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'#') => {
                chars.next();
                out.push('#');
            }
            '#' => {
                let mut kw = String::new();
                let mut closed = false;
                for k in chars.by_ref() {
                    if k == '#' {
                        closed = true;
                        break;
                    }
                    kw.push(k);
                }
                if closed {
                    out += &keyword_value(&kw, p, gps_t, vid_t);
                } else {
                    out.push('#');
                    out += &kw;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// GPS Text: the text with its `#keywords#` replaced by the track's values now. With no text,
/// the chosen Keyword alone is shown.
pub fn gps_text(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let level = fv(e, "smoothing", cx).round().clamp(0.0, 100.0) as usize;
    let Some(tr) = load(&text_param(e, "gps_file"), level) else { return };
    let offset = fv(e, "time_offset", cx) as f64;
    let mut gt = gps_now(&tr, cx, offset);
    let ups = fv(e, "updates", cx).round().max(0.0) as f64;
    let (_, i) = at(&tr, gt);
    let p = if ups > 0.0 {
        let q = 1000.0 / ups;
        gt = (gt / q).floor() * q;
        at(&tr, gt).0
    } else {
        // only real points
        gt = tr.pts[i].t;
        tr.pts[i]
    };
    let vid = gt - offset * 1000.0;
    let mut text = text_param(e, "text");
    if text.trim().is_empty() {
        let k = redrob_film_project::effect::GPS_KEYWORDS;
        text = format!("#{}#", k[(pick(e, "keyword") as usize).min(k.len() - 1)]);
    }
    let s = expand(&text, &p, gt, vid);
    let b = TextBox::from_params(img, e, cx);
    draw_text_box(img, &s, &b);
}

// ------------------------------------------------------------------ GPS Graphic

fn value_of(p: &Pt, src: u32) -> Option<f64> {
    match src {
        1 => p.ele,
        2 => p.hr,
        3 => Some(p.speed),
        _ => Some(p.lat),
    }
}

/// A value in the legend unit (altitude m/ft, speed km/h / mph / knots / m/s).
fn in_unit(v: f64, src: u32, unit: &str) -> f64 {
    let u = unit.to_ascii_lowercase();
    match src {
        1 if u.contains("ft") || u.contains("feet") => v * 3.280_84,
        3 if u.contains("mi") => v * 2.236_936,
        3 if u.contains("kn") => v * 1.943_844,
        3 if u.contains("m/s") => v,
        3 => v * 3.6,
        _ => v,
    }
}

/// Anti-aliased round-capped line of radius `r` onto a premultiplied layer.
fn line(img: &mut Image, a: (f32, f32), b: (f32, f32), r: f32, c: [f32; 4]) {
    let x0 = (a.0.min(b.0) - r - 1.0).floor().max(0.0) as usize;
    let x1 = ((a.0.max(b.0) + r + 1.0).ceil().max(0.0) as usize).min(img.w);
    let y0 = (a.1.min(b.1) - r - 1.0).floor().max(0.0) as usize;
    let y1 = ((a.1.max(b.1) + r + 1.0).ceil().max(0.0) as usize).min(img.h);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let ll = (dx * dx + dy * dy).max(1e-9);
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let t = (((px - a.0) * dx + (py - a.1) * dy) / ll).clamp(0.0, 1.0);
            let (qx, qy) = (a.0 + dx * t - px, a.1 + dy * t - py);
            let cov = (r + 0.5 - (qx * qx + qy * qy).sqrt()).clamp(0.0, 1.0) * c[3];
            if cov <= 0.0 {
                continue;
            }
            let i = (y * img.w + x) * 4;
            let p = &mut img.px[i..i + 4];
            let k = 1.0 - cov;
            p[0] = c[0] * cov + p[0] * k;
            p[1] = c[1] * cov + p[1] * k;
            p[2] = c[2] * cov + p[2] * k;
            p[3] = cov + p[3] * k;
        }
    }
}

fn lerp_col(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}
/// Straight sRGB colour → linear (alpha kept straight) for [`line`].
fn lin_col(c: [f32; 4]) -> [f32; 4] {
    let l = crate::effects::dec([c[0], c[1], c[2]]);
    [l[0], l[1], l[2], c[3]]
}

type ImageCache = Mutex<HashMap<String, Arc<Image>>>;

fn background(path: &str) -> Option<Arc<Image>> {
    static CACHE: OnceLock<ImageCache> = OnceLock::new();
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let cache = CACHE.get_or_init(Default::default);
    if let Some(i) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(path) {
        return Some(i.clone());
    }
    let src = redrob_film_media::open_path(std::path::Path::new(path), &[]).ok()?;
    let f = src.video_frame(redrob_film_media::FrameRequest::full(redrob_film_time::Tick::ZERO)).ok()?;
    let rgba = f.to_rgba8();
    let (w, h) = (f.width as usize, f.height as usize);
    let mut img = Image::new(w, h);
    img.px.par_chunks_mut(4).zip(rgba.par_chunks(4)).for_each(|(p, s)| {
        let a = s[3] as f32 / 255.0;
        let c = crate::effects::dec([s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0]);
        p.copy_from_slice(&[c[0] * a, c[1] * a, c[2] * a, a]);
    });
    let img = Arc::new(img);
    let mut c = cache.lock().unwrap_or_else(|p| p.into_inner());
    if c.len() > 8 {
        c.clear();
    }
    c.insert(path.to_string(), img.clone());
    Some(img)
}

/// GPS Graphic: the track (map line, a value over time, or a speedometer) drawn in a rectangle,
/// with the current position marked.
pub fn gps_graphic(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let level = fv(e, "smoothing", cx).round().clamp(0.0, 100.0) as usize;
    let Some(tr) = load(&text_param(e, "gps_file"), level) else { return };
    let n = tr.pts.len();
    let (ts, te) = ((fv(e, "trim_start", cx) / 100.0).clamp(0.0, 1.0), (fv(e, "trim_end", cx) / 100.0).clamp(0.0, 1.0));
    let (i0, i1) = (((n - 1) as f32 * ts).round() as usize, ((n - 1) as f32 * te).round() as usize);
    if i1 <= i0 {
        return;
    }
    let pts = &tr.pts[i0..=i1];
    let src = pick(e, "source");
    let graph = pick(e, "graph");
    let unit = text_param(e, "legend_unit");
    // the current time: a time-lapse covers Speed Multiplier seconds of track per clip second
    let mult = fv(e, "speed_multiplier", cx).max(0.001) as f64;
    let offset = fv(e, "time_offset", cx) as f64;
    let base = gps_now(&tr, cx, offset);
    let now_t = match cx.env.and_then(|en| en.media_datetime_ms()) {
        Some(_) => base + (mult - 1.0) * cx.seconds * 1000.0,
        None => tr.pts[0].t + offset * 1000.0 + cx.seconds * mult * 1000.0,
    };
    let (now, _) = at(&tr, now_t);
    let (w, h) = (img.w as f32, img.h as f32);
    let rect = [fv(e, "rect_x", cx) / 100.0 * w, fv(e, "rect_y", cx) / 100.0 * h, (fv(e, "rect_w", cx) / 100.0 * w).max(2.0), (fv(e, "rect_h", cx) / 100.0 * h).max(2.0)];
    let (lw, lh) = (rect[2].ceil() as usize, rect[3].ceil() as usize);
    let mut layer = Image::new(lw, lh);
    let s = cx.px_scale.max(0.05);
    let thick = (fv(e, "thickness", cx) * s).max(1.0);
    // background image, fitted to the rectangle and scaled about its centre
    let bgs = fv(e, "bg_scale", cx);
    if bgs > 0.0
        && let Some(bg) = background(&text_param(e, "bg_image"))
    {
        let fit = (lw as f64 / bg.w as f64).max(lh as f64 / bg.h as f64) / bgs as f64;
        let m = Affine::translate(lw as f64 / 2.0, lh as f64 / 2.0).then_apply(&Affine::scale(fit, fit)).then_apply(&Affine::translate(-(bg.w as f64) / 2.0, -(bg.h as f64) / 2.0));
        let mut b = bg.transformed(lw, lh, &m);
        b.scale_alpha((fv(e, "bg_opacity", cx) / 100.0).clamp(0.0, 1.0));
        layer = b;
    }
    // data → x/y
    let xv = |p: &Pt| if src == 0 { p.lon * p.lat.to_radians().cos().abs().max(0.01) } else { p.t };
    let yv = |p: &Pt| if src == 0 { Some(p.lat) } else { value_of(p, src) };
    let xs: Vec<f64> = pts.iter().map(xv).collect();
    let ys: Vec<Option<f64>> = pts.iter().map(yv).collect();
    let (xmin, xmax) = xs.iter().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let (ymin, ymax) = ys.iter().flatten().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    if ymin > ymax {
        return;
    }
    let crop = |abs: bool, lo: f64, hi: f64, min: f64, max: f64| -> (f64, f64) {
        if abs { (lo, hi) } else { (min + (max - min) * lo / 100.0, min + (max - min) * hi / 100.0) }
    };
    let (cl, cr) = (fv(e, "crop_left", cx) as f64, fv(e, "crop_right", cx) as f64);
    let (cb, ct) = (fv(e, "crop_bottom", cx) as f64, fv(e, "crop_top", cx) as f64);
    let (mut x0, mut x1) = crop(flag(e, "crop_h_abs"), cl, cr, xmin, xmax);
    let (mut y0, mut y1) = crop(flag(e, "crop_v_abs"), cb, ct, ymin, ymax);
    // the map keeps its aspect: the shorter axis grows
    let pad = thick as f64 + 2.0;
    let (gw, gh) = (lw as f64 - 2.0 * pad, lh as f64 - 2.0 * pad);
    if src == 0 && graph != 2 {
        let (sx, sy) = ((x1 - x0).abs().max(1e-12) / gw.max(1.0), (y1 - y0).abs().max(1e-12) / gh.max(1.0));
        let k = sx.max(sy);
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        x0 = mx - k * gw / 2.0;
        x1 = mx + k * gw / 2.0;
        y0 = my - k * gh / 2.0;
        y1 = my + k * gh / 2.0;
    }
    if graph == 1 {
        // follow: the window keeps its size and is centred on the current position
        let (hw, hh) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
        let (cxv, cyv) = (xv(&now), yv(&now).unwrap_or((y0 + y1) / 2.0));
        x0 = cxv - hw;
        x1 = cxv + hw;
        if src == 0 {
            y0 = cyv - hh;
            y1 = cyv + hh;
        }
    }
    let map = |x: f64, y: f64| -> (f32, f32) {
        let u = (x - x0) / (x1 - x0).abs().max(1e-12) * (x1 - x0).signum();
        let v = (y - y0) / (y1 - y0).abs().max(1e-12) * (y1 - y0).signum();
        ((pad + u * gw) as f32, (pad + (1.0 - v) * gh) as f32)
    };
    let (c1, c2) = (lin_col(color(e, "color1", cx)), lin_col(color(e, "color2", cx)));
    let style = pick(e, "color_style");
    let (emin, emax) = tr.pts.iter().filter_map(|p| p.ele).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
    let (hmin, hmax) = tr.pts.iter().filter_map(|p| p.hr).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
    let (smin, smax) = tr.pts.iter().map(|p| p.speed).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
    let norm = |v: f64, a: f64, b: f64| if b > a { ((v - a) / (b - a)) as f32 } else { 0.0 };
    let color_at = |i: usize, p: &Pt, at: (f32, f32)| -> ([f32; 4], f32) {
        let past = p.t <= now.t;
        let thin = if thick < 3.0 { 1.0 } else { 2.0 * s };
        match style {
            1 => (if past { c1 } else { c2 }, thick),
            2 => (if past { c1 } else { c2 }, if past { thick } else { thin }),
            3 => (if past { c2 } else { c1 }, if past { thin } else { thick }),
            4 => (lerp_col(c1, c2, at.1 / lh as f32), thick),
            5 => (lerp_col(c1, c2, at.0 / lw as f32), thick),
            6 => (lerp_col(c1, c2, i as f32 / pts.len().max(2) as f32), thick),
            7 => (lerp_col(c1, c2, p.ele.map_or(0.0, |v| norm(v, emin, emax))), thick),
            8 => (lerp_col(c1, c2, p.hr.map_or(0.0, |v| norm(v, hmin, hmax))), thick),
            9 => (lerp_col(c1, c2, norm(p.speed, smin, smax)), thick),
            _ => (c1, thick),
        }
    };
    let mut now_color = c1;
    if graph == 2 {
        // speedometer: a 240° dial from the minimum to the maximum shown value
        let (ccx, ccy) = (lw as f32 / 2.0, lh as f32 * 0.55);
        let r = (lw.min(lh) as f32 / 2.0 - thick - 2.0).max(2.0);
        let ang = |t: f32| (210.0 - 240.0 * t.clamp(0.0, 1.0)).to_radians();
        let frac = if src == 0 {
            norm(now.t, pts[0].t, pts[pts.len() - 1].t)
        } else {
            yv(&now).map_or(0.0, |v| norm(v, y0, y1))
        };
        let segs = 60;
        for k in 0..segs {
            let (ta, tb) = (k as f32 / segs as f32, (k + 1) as f32 / segs as f32);
            let (a, b) = (ang(ta), ang(tb));
            let col = if style == 1 || style == 2 { if tb <= frac { c1 } else { c2 } } else { lerp_col(c1, c2, ta) };
            line(&mut layer, (ccx + r * a.cos(), ccy - r * a.sin()), (ccx + r * b.cos(), ccy - r * b.sin()), thick / 2.0, col);
        }
        if flag(e, "legend") {
            for k in 0..=4 {
                let a = ang(k as f32 / 4.0);
                line(&mut layer, (ccx + (r - thick * 2.0) * a.cos(), ccy - (r - thick * 2.0) * a.sin()), (ccx + r * a.cos(), ccy - r * a.sin()), 1.0, [1.0; 4]);
                let val = in_unit(y0 + (y1 - y0) * k as f64 / 4.0, src, &unit);
                let tb = TextBox {
                    family: redrob_film_text::fonts::DEFAULT_FAMILY.into(),
                    style: "Regular",
                    size: (r / 7.0).max(6.0),
                    outline: 0.0,
                    padding: 0.0,
                    h_align: 1,
                    v_align: 1,
                    opacity: 1.0,
                    fg: [1.0; 4],
                    bg: [0.0; 4],
                    outline_color: [0.0; 4],
                    rect: [ccx + (r - thick * 4.0) * a.cos() - 20.0, ccy - (r - thick * 4.0) * a.sin() - 10.0, 40.0, 20.0],
                    underline: false,
                    strikethrough: false,
                };
                draw_text_box(&mut layer, &format!("{val:.0}"), &tb);
            }
        }
        if flag(e, "now_dot") {
            let a = ang(frac);
            let nd = color(e, "now_dot_color", cx);
            let col = if nd[3] <= 0.0 { [1.0, 1.0, 1.0, 1.0] } else { lin_col(nd) };
            line(&mut layer, (ccx, ccy), (ccx + r * 0.9 * a.cos(), ccy - r * 0.9 * a.sin()), (thick / 2.5).max(1.0), col);
        }
    } else {
        let placed: Vec<Option<(f32, f32)>> = xs.iter().zip(&ys).map(|(x, y)| y.map(|y| map(*x, y))).collect();
        let dots = flag(e, "dots_only");
        for i in 0..placed.len() {
            let Some(b) = placed[i] else { continue };
            let (col, t) = color_at(i, &pts[i], b);
            if pts[i].t <= now.t {
                now_color = col;
            }
            if dots {
                line(&mut layer, b, b, t / 2.0, col);
            } else if i > 0
                && let Some(a) = placed[i - 1]
            {
                line(&mut layer, a, b, t / 2.0, col);
            }
        }
        if flag(e, "legend") {
            let tb = |r: [f32; 4], al: u32| TextBox {
                family: redrob_film_text::fonts::DEFAULT_FAMILY.into(),
                style: "Regular",
                size: (lh as f32 / 18.0).max(6.0),
                outline: 0.0,
                padding: 0.0,
                h_align: al,
                v_align: 1,
                opacity: 1.0,
                fg: [1.0; 4],
                bg: [0.0; 4],
                outline_color: [0.0; 4],
                rect: r,
                underline: false,
                strikethrough: false,
            };
            for k in 0..=4 {
                let fr = k as f64 / 4.0;
                let (_, y) = map(x0, y0 + (y1 - y0) * fr);
                line(&mut layer, (pad as f32, y), (lw as f32 - pad as f32, y), 0.5, [1.0, 1.0, 1.0, 0.5]);
                let val = in_unit(y0 + (y1 - y0) * fr, src, &unit);
                let label = if src == 0 { format!("{val:.4}") } else { format!("{val:.0}{unit}") };
                draw_text_box(&mut layer, &label, &tb([pad as f32 + 2.0, y - 12.0, lw as f32, 12.0], 0));
                if src == 0 {
                    let (x, _) = map(x0 + (x1 - x0) * fr, y0);
                    line(&mut layer, (x, pad as f32), (x, lh as f32 - pad as f32), 0.5, [1.0, 1.0, 1.0, 0.5]);
                }
            }
        }
        if flag(e, "now_dot")
            && let Some(y) = yv(&now)
        {
            let c = map(xv(&now), y);
            let nd = color(e, "now_dot_color", cx);
            let outer = if nd[3] <= 0.0 { now_color } else { lin_col(nd) };
            line(&mut layer, c, c, thick, outer);
            line(&mut layer, c, c, thick * 0.55, [1.0, 1.0, 1.0, 1.0]);
        }
    }
    if flag(e, "now_text") {
        let v = match src {
            0 => format!("{:.0}%", 100.0 * norm(now.t, pts[0].t, pts[pts.len() - 1].t)),
            _ => yv(&now).map(|v| format!("{:.1}{unit}", in_unit(v, src, &unit))).unwrap_or_default(),
        };
        let tb = TextBox {
            family: redrob_film_text::fonts::DEFAULT_FAMILY.into(),
            style: "Bold",
            size: (lh as f32 / 7.0).max(8.0),
            outline: 0.0,
            padding: 0.0,
            h_align: 2,
            v_align: 2,
            opacity: 1.0,
            fg: [1.0; 4],
            bg: [0.0; 4],
            outline_color: [0.0; 4],
            rect: [0.0, 0.0, lw as f32 - 4.0, lh as f32 - 4.0],
            underline: false,
            strikethrough: false,
        };
        draw_text_box(&mut layer, &v, &tb);
    }
    // place the layer: rotated about the rectangle's centre
    let rot = fv(e, "rotation", cx) as f64;
    let m = Affine::translate((rect[0] + rect[2] / 2.0) as f64, (rect[1] + rect[3] / 2.0) as f64)
        .then_apply(&Affine::rotate_deg(rot))
        .then_apply(&Affine::translate(-(lw as f64) / 2.0, -(lh as f64) / 2.0));
    let placed = layer.transformed(img.w, img.h, &m);
    img.px.par_chunks_mut(4).zip(placed.px.par_chunks(4)).for_each(|(d, s)| {
        let k = 1.0 - s[3];
        for i in 0..4 {
            d[i] = s[i] + d[i] * k;
        }
    });
}
