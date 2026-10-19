//! Text overlays of the "More Video Effects" bin (Timer, GPS Text): text in a rectangle with
//! alignment, padding, a background, an outline and optional underline / strikethrough.

use redrob_film_text::{Align, ParagraphStyle, TextStyle, layout, render};

use super::*;
use crate::graphics::{paint_mask, premul_linear};

/// How a text overlay is drawn (working-image pixels).
pub(crate) struct TextBox {
    pub family: String,
    pub style: &'static str,
    pub size: f32,
    pub outline: f32,
    pub padding: f32,
    pub h_align: u32,
    pub v_align: u32,
    pub opacity: f32,
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub outline_color: [f32; 4],
    /// `[x, y, w, h]`.
    pub rect: [f32; 4],
    pub underline: bool,
    pub strikethrough: bool,
}

impl TextBox {
    /// The shared text parameters of Timer / GPS Text.
    pub fn from_params(img: &Image, e: &EffectInstance, cx: &FxCtx) -> TextBox {
        let fam = text_param(e, "font");
        let (w, h) = (img.w as f32, img.h as f32);
        let s = cx.px_scale;
        TextBox {
            family: if fam.trim().is_empty() { redrob_film_text::fonts::DEFAULT_FAMILY.to_string() } else { fam.trim().to_string() },
            style: ["Regular", "Bold", "Italic", "Bold Italic"][pick(e, "font_style").min(3) as usize],
            size: (fv(e, "size", cx) / 100.0 * h).max(2.0),
            outline: fv(e, "outline", cx).round().clamp(0.0, 3.0) * s.max(0.25),
            padding: fv(e, "padding", cx).max(0.0) * s,
            h_align: pick(e, "h_align"),
            v_align: pick(e, "v_align"),
            opacity: (fv(e, "opacity", cx) / 100.0).clamp(0.0, 1.0),
            fg: color(e, "fg", cx),
            bg: color(e, "bg", cx),
            outline_color: color(e, "outline_color", cx),
            rect: [fv(e, "rect_x", cx) / 100.0 * w, fv(e, "rect_y", cx) / 100.0 * h, fv(e, "rect_w", cx) / 100.0 * w, fv(e, "rect_h", cx) / 100.0 * h],
            underline: flag(e, "underline"),
            strikethrough: flag(e, "strikethrough"),
        }
    }
}

/// Fill an axis-aligned rectangle (premultiplied colour over the picture, with partial pixel
/// coverage at the edges).
pub(crate) fn fill_rect(img: &mut Image, r: [f32; 4], c: [f32; 4], opacity: f32) {
    let col = premul_linear(c);
    let k = opacity.clamp(0.0, 1.0);
    if col[3] <= 0.0 || k <= 0.0 {
        return;
    }
    let (x0, y0, x1, y1) = (r[0].max(0.0), r[1].max(0.0), r[2].min(img.w as f32), r[3].min(img.h as f32));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let cy = ((y as f32 + 1.0).min(y1) - (y as f32).max(y0)).clamp(0.0, 1.0);
        if cy <= 0.0 {
            return;
        }
        for x in (x0.floor() as usize)..(x1.ceil() as usize).min(w) {
            let cxv = ((x as f32 + 1.0).min(x1) - (x as f32).max(x0)).clamp(0.0, 1.0);
            let a = cy * cxv * k;
            if a <= 0.0 {
                continue;
            }
            let p = &mut row[x * 4..x * 4 + 4];
            let inv = 1.0 - col[3] * a;
            for i in 0..4 {
                p[i] = col[i] * a + p[i] * inv;
            }
        }
    });
}

/// Grow a coverage mask by `r` pixels (round brush).
fn dilate(m: &redrob_film_text::Mask, r: usize) -> redrob_film_text::Mask {
    let (w, h) = (m.w + 2 * r, m.h + 2 * r);
    let ri = r as isize;
    let mut a = vec![0.0f32; w * h];
    a.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut v: f32 = 0.0;
            for dy in -ri..=ri {
                for dx in -ri..=ri {
                    if dx * dx + dy * dy > ri * ri + ri {
                        continue;
                    }
                    let (sx, sy) = (x as isize - ri + dx, y as isize - ri + dy);
                    if sx >= 0 && sy >= 0 && (sx as usize) < m.w && (sy as usize) < m.h {
                        v = v.max(m.a[sy as usize * m.w + sx as usize]);
                    }
                }
            }
            *o = v;
        }
    });
    redrob_film_text::Mask { w, h, a }
}

/// Draw `text` (one or more lines) into `b.rect`.
pub(crate) fn draw_text_box(img: &mut Image, text: &str, b: &TextBox) {
    if text.is_empty() {
        return;
    }
    let st = TextStyle { family: b.family.clone(), style: b.style.into(), size: b.size, ..Default::default() };
    let align = match b.h_align {
        1 => Align::Center,
        2 => Align::Right,
        _ => Align::Left,
    };
    let lines: Vec<&str> = text.split('\n').collect();
    let lays: Vec<_> = lines.iter().map(|l| layout(l, &st, &ParagraphStyle { align, ..Default::default() })).collect();
    let lh = b.size * 1.2;
    // vertical metrics of a line box from the first line
    let (top, bot) = lays.first().map(|l| (l.bounds[1], l.bounds[3])).unwrap_or((-b.size * 0.8, b.size * 0.2));
    let block_h = (lays.len() as f32 - 1.0) * lh + (bot - top);
    let [rx, ry, rw, rh] = b.rect;
    let y_first = match b.v_align {
        1 => ry + (rh - block_h) / 2.0 - top,
        2 => ry + rh - block_h - top,
        _ => ry - top,
    };
    // line origins and their bounds
    let placed: Vec<(f32, f32, [f32; 4])> = lays
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let bb = l.bounds;
            let ox = match b.h_align {
                1 => rx + rw / 2.0 - (bb[0] + bb[2]) / 2.0,
                2 => rx + rw - bb[2],
                _ => rx - bb[0],
            };
            let oy = y_first + i as f32 * lh;
            (ox, oy, [ox + bb[0], oy + bb[1], ox + bb[2], oy + bb[3]])
        })
        .collect();
    if b.bg[3] > 0.0 {
        let x0 = placed.iter().map(|p| p.2[0]).fold(f32::MAX, f32::min);
        let x1 = placed.iter().map(|p| p.2[2]).fold(f32::MIN, f32::max);
        let y0 = placed.first().map(|p| p.2[1]).unwrap_or(0.0);
        let y1 = placed.last().map(|p| p.2[3]).unwrap_or(0.0);
        fill_rect(img, [x0 - b.padding, y0 - b.padding, x1 + b.padding, y1 + b.padding], b.bg, b.opacity);
    }
    let fg = premul_linear(b.fg);
    for (l, (ox, oy, bb)) in lays.iter().zip(&placed) {
        let (m, x0, y0) = render::rasterize(l, &render::at(*ox, oy.round()));
        if b.outline > 0.0 {
            let r = b.outline.round().max(1.0) as usize;
            let dm = dilate(&m, r);
            paint_mask(img, &dm, x0 - r as i32, y0 - r as i32, premul_linear(b.outline_color), b.opacity);
        }
        paint_mask(img, &m, x0, y0, fg, b.opacity);
        let thick = (b.size * 0.06).max(1.0);
        if b.underline {
            let y = oy + b.size * 0.1;
            fill_rect(img, [bb[0], y, bb[2], y + thick], b.fg, b.opacity);
        }
        if b.strikethrough {
            let y = oy - b.size * 0.3;
            fill_rect(img, [bb[0], y, bb[2], y + thick], b.fg, b.opacity);
        }
    }
}

/// `v` seconds in one of [`redrob_film_project::effect::TIMER_FORMATS`].
pub(crate) fn format_timer(v: f64, fmt: u32) -> String {
    let v = v.max(0.0);
    let total = v.floor() as i64;
    let frac = v - v.floor();
    let digits = |n: u32| -> String {
        let f = (frac * 10f64.powi(n as i32)).floor() as i64;
        format!("{f:0width$}", width = n as usize)
    };
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    match fmt {
        0 => format!("{h:02}:{m:02}:{s:02}"),
        1 => format!("{h:02}:{m:02}:{s:02}.{}", digits(1)),
        2 => format!("{:02}:{s:02}", total / 60),
        3 => format!("{:02}:{s:02}.{}", total / 60, digits(2)),
        4 => format!("{:02}:{s:02}.{}", total / 60, digits(3)),
        5 => format!("{total:02}"),
        6 => format!("{total:02}.{}", digits(1)),
        7 => format!("{total:02}.{}", digits(2)),
        _ => format!("{total:02}.{}", digits(3)),
    }
}

/// The value a Timer shows `t` seconds into the clip: frozen at its start value until Start,
/// counting for Duration, then frozen; Offset is added (counting down ends at Offset).
pub(crate) fn timer_value(t: f64, start: f64, duration: f64, offset: f64, down: bool) -> f64 {
    let el = (t - start).clamp(0.0, duration.max(0.0));
    if down { offset + duration.max(0.0) - el } else { offset + el }
}

/// Timer: a count-up / count-down clock over the clip.
pub fn timer(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let v = timer_value(cx.seconds, fv(e, "start", cx) as f64, fv(e, "duration", cx) as f64, fv(e, "offset", cx) as f64, pick(e, "direction") == 1);
    let text = format_timer(v, pick(e, "format"));
    let b = TextBox::from_params(img, e, cx);
    draw_text_box(img, &text, &b);
}
