//! Slideshows: an image sequence played as a slideshow (Kdenlive's image sequence clip options).
//! Every image stays up for a number of frames; the show can loop, crop each image to fill the
//! frame, dissolve or wipe from one image to the next, move a slow pan and zoom across each image
//! (Ken Burns) and soften it with a low-pass filter.
//!
//! [`SlideshowSource`] wraps the image sequence source and maps media time to (image, frame within
//! the image). It works in straight-alpha RGBA8 and keeps recent frames in a [`FrameCache`].

use std::sync::Arc;

use redrob_film_frame::{AudioBuffer, VideoFrame};
use redrob_film_time::{FrameRate, Tick};
use serde::{Deserialize, Serialize};

use crate::{FrameCache, FrameRequest, MediaError, MediaInfo, MediaKind, MediaSource, Result, SharedSource};

/// How one image gives way to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SlideWipe {
    /// A plain cross-dissolve.
    #[default]
    Dissolve,
    /// The next image comes in from the left edge.
    Left,
    Right,
    Top,
    Bottom,
    /// From the centre outwards.
    Radial,
}

impl SlideWipe {
    pub const ALL: [SlideWipe; 6] = [SlideWipe::Dissolve, SlideWipe::Left, SlideWipe::Right, SlideWipe::Top, SlideWipe::Bottom, SlideWipe::Radial];
    pub fn label(self) -> &'static str {
        match self {
            SlideWipe::Dissolve => "Dissolve",
            SlideWipe::Left => "Wipe from left",
            SlideWipe::Right => "Wipe from right",
            SlideWipe::Top => "Wipe from top",
            SlideWipe::Bottom => "Wipe from bottom",
            SlideWipe::Radial => "Radial wipe",
        }
    }
    pub fn from_name(s: &str) -> Option<SlideWipe> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
        Self::ALL.into_iter().find(|w| format!("{w:?}").to_ascii_lowercase() == n || w.label().to_ascii_lowercase().replace(' ', "") == n)
    }
    /// Where in the wipe a pixel switches (0 = first, 1 = last), for normalised `u`, `v`.
    fn order(self, u: f32, v: f32) -> f32 {
        match self {
            SlideWipe::Dissolve => 0.5,
            SlideWipe::Left => u,
            SlideWipe::Right => 1.0 - u,
            SlideWipe::Top => v,
            SlideWipe::Bottom => 1.0 - v,
            SlideWipe::Radial => (((u - 0.5) * (u - 0.5) + (v - 0.5) * (v - 0.5)).sqrt() / std::f32::consts::FRAC_1_SQRT_2).min(1.0),
        }
    }
}

/// Slideshow options of an image sequence item.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Slideshow {
    /// How long each image is shown, in frames of the clip's frame rate.
    pub image_frames: u32,
    /// Start again after the last image (the clip can be made as long as wanted).
    pub looped: bool,
    /// Scale each image to fill the frame and crop what overflows (else fit, with black bars).
    pub crop: bool,
    /// Transition length between images in frames (0 = cut).
    pub dissolve_frames: u32,
    pub wipe: SlideWipe,
    /// Softness of a wipe's edge, 0–1.
    pub softness: f32,
    /// Slow pan and zoom across each image.
    pub ken_burns: bool,
    /// Soften the images (reduces flicker of fine detail).
    pub low_pass: bool,
}

impl Default for Slideshow {
    fn default() -> Self {
        Slideshow {
            image_frames: 25,
            looped: false,
            crop: false,
            dissolve_frames: 0,
            wipe: SlideWipe::Dissolve,
            softness: 0.2,
            ken_burns: false,
            low_pass: false,
        }
    }
}

/// How long a looping slideshow says it is (it repeats for as long as a clip asks).
pub const LOOP_LENGTH_SECONDS: i64 = 3 * 3600;

impl Slideshow {
    /// The clip's length for `images` images at `rate`.
    pub fn duration(&self, images: usize, rate: FrameRate) -> Tick {
        if self.looped {
            Tick(LOOP_LENGTH_SECONDS * redrob_film_time::TICKS_PER_SECOND)
        } else {
            rate.tick_of(images.max(1) as i64 * self.image_frames.max(1) as i64)
        }
    }
}

/// An image sequence shown as a slideshow.
pub struct SlideshowSource {
    inner: SharedSource,
    opts: Slideshow,
    images: usize,
    rate: FrameRate,
    info: MediaInfo,
    cache: FrameCache<i64>,
}

impl SlideshowSource {
    /// `inner` plays one image per frame at `inner_rate`; `images` is how many it has. The
    /// slideshow plays at `rate` (the item's frame rate).
    pub fn new(inner: SharedSource, images: usize, rate: FrameRate, opts: Slideshow) -> Self {
        let mut info = inner.info().clone();
        info.kind = MediaKind::ImageSequence;
        info.duration = opts.duration(images, rate);
        if let Some(v) = info.video.as_mut() {
            v.frame_rate = rate;
        }
        SlideshowSource { inner, opts, images: images.max(1), rate, info, cache: FrameCache::new(128 << 20) }
    }

    fn image(&self, idx: usize) -> Result<Arc<VideoFrame>> {
        let r = self.inner.info().frame_rate();
        self.inner.video_frame(FrameRequest::full(r.tick_of(idx as i64)))
    }

    /// Image `idx` placed in a `w`×`h` frame at `local` frames into its time on screen.
    fn placed(&self, idx: usize, local: u32, w: usize, h: usize) -> Result<Vec<u8>> {
        let f = self.image(idx)?;
        let (iw, ih) = (f.width as usize, f.height as usize);
        let src = f.to_rgba8();
        let src = if self.opts.low_pass { box_blur(&src, iw, ih) } else { src };
        // the image's scale: cover (crop) or fit
        let (sx, sy) = (w as f32 / iw.max(1) as f32, h as f32 / ih.max(1) as f32);
        let mut scale = if self.opts.crop { sx.max(sy) } else { sx.min(sy) };
        let (mut ox, mut oy) = (0.0f32, 0.0f32);
        if self.opts.ken_burns {
            // zoom 100 % → 115 % (in on even images, out on odd ones), drifting diagonally
            let span = (self.opts.image_frames.max(1) + self.opts.dissolve_frames) as f32;
            let p = (local as f32 / span).clamp(0.0, 1.0);
            let p = if idx % 2 == 0 { p } else { 1.0 - p };
            scale *= 1.0 + 0.15 * p;
            let dir = if idx % 3 == 0 { 1.0 } else { -1.0 };
            ox = dir * 0.04 * w as f32 * (p - 0.5);
            oy = 0.03 * h as f32 * (p - 0.5);
        }
        let mut out = vec![0u8; w * h * 4];
        for (k, c) in out.chunks_exact_mut(4).enumerate() {
            c[3] = 255;
            let (x, y) = ((k % w) as f32 + 0.5, (k / w) as f32 + 0.5);
            // output pixel → image coordinates (image centred in the frame)
            let u = (x - w as f32 / 2.0 - ox) / scale + iw as f32 / 2.0;
            let v = (y - h as f32 / 2.0 - oy) / scale + ih as f32 / 2.0;
            if u >= 0.0 && v >= 0.0 && u < iw as f32 && v < ih as f32 {
                c.copy_from_slice(&bilinear(&src, iw, ih, u - 0.5, v - 0.5));
            } else {
                c.copy_from_slice(&[0, 0, 0, 255]);
            }
        }
        Ok(out)
    }

    fn render(&self, frame: i64) -> Result<Arc<VideoFrame>> {
        let v = self.info.video.as_ref().ok_or(MediaError::NoStream("video"))?;
        let (w, h) = (v.width.max(2) as usize, v.height.max(2) as usize);
        let per = self.opts.image_frames.max(1) as i64;
        let n = self.images as i64;
        let mut f = frame.max(0);
        if self.opts.looped {
            f %= per * n;
        }
        let idx = (f / per).min(n - 1);
        let local = (f - idx * per) as u32;
        let mut px = self.placed(idx as usize, local + self.opts.dissolve_frames, w, h)?;
        // the transition into the next image during this image's last frames
        let d = self.opts.dissolve_frames.min(per as u32 - 1) as i64;
        let has_next = idx + 1 < n || self.opts.looped;
        if d > 0 && has_next && (local as i64) >= per - d {
            let next = ((idx + 1) % n) as usize;
            let into = local as i64 - (per - d);
            let progress = (into + 1) as f32 / (d + 1) as f32;
            let other = self.placed(next, into as u32, w, h)?;
            let soft = self.opts.softness.clamp(0.0, 1.0).max(0.001);
            for (k, (a, b)) in px.chunks_exact_mut(4).zip(other.chunks_exact(4)).enumerate() {
                let (u, vv) = ((k % w) as f32 / w as f32, (k / w) as f32 / h as f32);
                let mix = if self.opts.wipe == SlideWipe::Dissolve {
                    progress
                } else {
                    // the edge sweeps past every pixel: 0 before it, 1 after
                    ((progress * (1.0 + soft) - self.opts.wipe.order(u, vv)) / soft).clamp(0.0, 1.0)
                };
                for (ac, bc) in a.iter_mut().zip(b).take(3) {
                    *ac = (*ac as f32 + (*bc as f32 - *ac as f32) * mix).round() as u8;
                }
            }
        }
        Ok(Arc::new(VideoFrame::rgba8(w as u32, h as u32, px).with_pts(self.rate.tick_of(frame))))
    }
}

impl MediaSource for SlideshowSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }

    fn video_frame(&self, req: FrameRequest) -> Result<Arc<VideoFrame>> {
        let frame = self.rate.frame_at(req.time.max(Tick::ZERO));
        self.cache.get_or_insert_with(frame, || self.render(frame))
    }

    fn audio(&self, _start: i64, _frames: usize, _sample_rate: u32) -> Result<AudioBuffer> {
        Err(MediaError::NoStream("audio"))
    }
}

fn bilinear(src: &[u8], w: usize, h: usize, x: f32, y: f32) -> [u8; 4] {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: usize, yy: usize, c: usize| src[(yy * w + xx) * 4 + c] as f32;
    let mut out = [0u8; 4];
    for (c, o) in out.iter_mut().enumerate() {
        let top = at(x0, y0, c) + (at(x1, y0, c) - at(x0, y0, c)) * fx;
        let bot = at(x0, y1, c) + (at(x1, y1, c) - at(x0, y1, c)) * fx;
        *o = (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// A 3×3 box blur (the low-pass filter).
fn box_blur(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = src.to_vec();
    if w < 3 || h < 3 {
        return out;
    }
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            for c in 0..3 {
                let mut s = 0u32;
                for dy in 0..3 {
                    for dx in 0..3 {
                        s += src[((y + dy - 1) * w + (x + dx - 1)) * 4 + c] as u32;
                    }
                }
                out[(y * w + x) * 4 + c] = (s / 9) as u8;
            }
        }
    }
    out
}
