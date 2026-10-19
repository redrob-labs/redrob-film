//! FLAC Audio export: the mix losslessly compressed into a native FLAC stream (`.flac`), written
//! from the format's public specification (RFC 9639): a STREAMINFO block, then frames of 4096
//! samples per channel, each channel a FIXED-predictor subframe (order 0–4, whichever codes
//! smallest) with Rice-coded residuals in one partition, or VERBATIM when that is smaller. The
//! channels are coded independently. The MD5 signature is left zero ("not computed"), which the
//! specification allows.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use redrob_film_project::{ItemId, Project};
use redrob_film_render::SourceProvider;
use redrob_film_time::TimeRange;

use crate::audio_out::AudioOut;
use crate::{ExportError, ExportSettings, Out, Progress, Result};

/// Samples per channel in a frame.
const BLOCK: usize = 4096;

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn new() -> Self {
        Bits { out: Vec::new(), acc: 0, n: 0 }
    }
    /// Write the low `bits` bits of `v` (bits <= 32).
    fn put(&mut self, v: u64, bits: u32) {
        if bits == 0 {
            return;
        }
        let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
        self.acc = (self.acc << bits) | (v & mask);
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
        self.acc &= if self.n == 0 { 0 } else { (1u64 << self.n) - 1 };
    }
    fn put_signed(&mut self, v: i64, bits: u32) {
        self.put(v as u64, bits);
    }
    /// `q` zero bits then a one.
    fn unary(&mut self, mut q: u64) {
        while q >= 32 {
            self.put(0, 32);
            q -= 32;
        }
        self.put(1, q as u32 + 1);
    }
    fn align(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put(0, pad);
        }
    }
}

fn crc8(data: &[u8]) -> u8 {
    let mut c: u8 = 0;
    for &b in data {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    c
}

fn crc16(data: &[u8]) -> u16 {
    let mut c: u16 = 0;
    for &b in data {
        c ^= u16::from(b) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
        }
    }
    c
}

/// The frame number in FLAC's UTF-8-like variable-length coding.
fn utf8_number(n: u64, out: &mut Vec<u8>) {
    if n < 0x80 {
        out.push(n as u8);
        return;
    }
    let mut bytes = 2;
    while bytes < 7 && n >= 1u64 << (5 * bytes + 1) {
        bytes += 1;
    }
    let lead: u8 = (0xFFu16 << (8 - bytes)) as u8;
    out.push(lead | (n >> (6 * (bytes - 1))) as u8);
    for i in (0..bytes - 1).rev() {
        out.push(0x80 | ((n >> (6 * i)) & 0x3F) as u8);
    }
}

/// Residuals of the fixed predictor of `order` (the first `order` samples are warm-up).
fn fixed_residual(x: &[i64], order: usize) -> Vec<i64> {
    (order..x.len())
        .map(|n| match order {
            0 => x[n],
            1 => x[n] - x[n - 1],
            2 => x[n] - 2 * x[n - 1] + x[n - 2],
            3 => x[n] - 3 * x[n - 1] + 3 * x[n - 2] - x[n - 3],
            _ => x[n] - 4 * x[n - 1] + 6 * x[n - 2] - 4 * x[n - 3] + x[n - 4],
        })
        .collect()
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// The best Rice parameter (0..=14) for `res` and its cost in bits.
fn rice_choice(res: &[i64]) -> (u32, u64) {
    let u: Vec<u64> = res.iter().map(|&v| zigzag(v)).collect();
    let mut best = (0, u64::MAX);
    for k in 0..=14u32 {
        let bits: u64 = u.iter().map(|&x| (x >> k) + 1 + u64::from(k)).sum();
        if bits < best.1 {
            best = (k, bits);
        }
    }
    best
}

/// One channel's subframe of `x` at `bps` bits.
fn subframe(b: &mut Bits, x: &[i64], bps: u32) {
    let verbatim = x.len() as u64 * u64::from(bps);
    let mut best: Option<(usize, u32, u64, Vec<i64>)> = None;
    for order in 0..=4usize.min(x.len().saturating_sub(1)) {
        let res = fixed_residual(x, order);
        // a residual must fit 32 bits signed
        if res.iter().any(|v| *v > i64::from(i32::MAX) || *v < i64::from(i32::MIN)) {
            continue;
        }
        let (k, bits) = rice_choice(&res);
        let cost = order as u64 * u64::from(bps) + 6 + bits;
        if best.as_ref().is_none_or(|bb| cost < bb.2) {
            best = Some((order, k, cost, res));
        }
    }
    match best {
        Some((order, k, cost, res)) if cost < verbatim => {
            // header: 0, type 001xxx (fixed, order), no wasted bits
            b.put(0, 1);
            b.put(0b001000 | order as u64, 6);
            b.put(0, 1);
            for &w in &x[..order] {
                b.put_signed(w, bps);
            }
            b.put(0, 2); // Rice, 4-bit parameters
            b.put(0, 4); // partition order 0
            b.put(u64::from(k), 4);
            for &r in &res {
                let u = zigzag(r);
                b.unary(u >> k);
                b.put(u, k);
            }
        }
        _ => {
            b.put(0, 1);
            b.put(0b000001, 6);
            b.put(0, 1);
            for &s in x {
                b.put_signed(s, bps);
            }
        }
    }
}

/// One frame of `planar` integer samples (`n` per channel).
fn frame(number: u64, planar: &[Vec<i64>], n: usize, bps: u32) -> Vec<u8> {
    let mut head = vec![0xFF, 0xF8];
    // block size: 16-bit (size - 1) at the end of the header; sample rate from STREAMINFO
    head.push(0x70);
    let size_code: u8 = if bps >= 24 { 0b110 } else { 0b100 };
    head.push(((planar.len() as u8 - 1) << 4) | (size_code << 1));
    utf8_number(number, &mut head);
    head.extend_from_slice(&((n - 1) as u16).to_be_bytes());
    let c8 = crc8(&head);
    head.push(c8);
    let mut b = Bits::new();
    b.out = head;
    for ch in planar {
        subframe(&mut b, &ch[..n], bps);
    }
    b.align();
    let c16 = crc16(&b.out);
    b.out.extend_from_slice(&c16.to_be_bytes());
    b.out
}

fn streaminfo(sr: u32, channels: u32, bps: u32, total: u64) -> Vec<u8> {
    let mut v = b"fLaC".to_vec();
    v.push(0x80); // last metadata block, STREAMINFO
    v.extend_from_slice(&[0, 0, 34]);
    let mut b = Bits::new();
    b.put(BLOCK as u64, 16);
    b.put(BLOCK as u64, 16);
    b.put(0, 24);
    b.put(0, 24);
    b.put(u64::from(sr), 20);
    b.put(u64::from(channels - 1), 3);
    b.put(u64::from(bps - 1), 5);
    b.put(total & ((1u64 << 36) - 1), 36);
    for _ in 0..4 {
        b.put(0, 32);
    }
    v.extend_from_slice(&b.out);
    v
}

fn quantise(s: f32, bps: u32) -> i64 {
    let max = if bps >= 24 { 8_388_607.0 } else { 32_767.0 };
    (s.clamp(-1.0, 1.0) * max).round() as i64
}

/// Export the mix of `range` as a FLAC file; returns (bytes written, sample frames).
pub(crate) fn export(project: &Arc<Project>, seq: ItemId, settings: &ExportSettings, range: TimeRange, sources: &dyn SourceProvider, progress: &Progress) -> Result<(u64, u64)> {
    use std::io::Write;
    let mut a = AudioOut::new(project.clone(), seq, settings, range)?;
    let (channels, sr) = (a.channels as u32, a.sr);
    if !(1..=8).contains(&channels) || sr == 0 || sr >= 1 << 20 {
        return Err(ExportError::Unsupported(format!("FLAC: {channels} channels at {sr} Hz")));
    }
    let bps: u32 = if settings.audio.bits >= 24 { 24 } else { 16 };
    let frames = a.remaining();
    let cancelled = || progress.cancel.load(Ordering::Relaxed);
    if !settings.part_of_batch {
        progress.total.store(frames.div_ceil(BLOCK as u64).max(1), Ordering::Relaxed);
        progress.set_status("Exporting audio (FLAC)".to_string());
    }
    a.measure(settings, sources, &cancelled)?;
    *progress.loudness.lock().unwrap_or_else(|e| e.into_inner()) = a.loudness;
    let mut out = Out::create(settings)?;
    let io = |e: std::io::Error| ExportError::Io(e.to_string());
    out.write_all(&streaminfo(sr, channels, bps, frames)).map_err(io)?;
    let mut number = 0u64;
    let mut pending: Vec<Vec<i64>> = vec![Vec::new(); channels as usize];
    let flush = |pending: &mut [Vec<i64>], all: bool, out: &mut Out, number: &mut u64| -> Result<()> {
        loop {
            let have = pending.first().map_or(0, Vec::len);
            if have == 0 || (have < BLOCK && !all) {
                return Ok(());
            }
            let n = have.min(BLOCK);
            out.write_all(&frame(*number, pending, n, bps)).map_err(io)?;
            *number += 1;
            for ch in pending.iter_mut() {
                ch.drain(..n);
            }
            if !settings.part_of_batch {
                progress.done.fetch_add(1, Ordering::Relaxed);
            }
        }
    };
    while let Some(planar) = a.pull(a.out.saturating_add(BLOCK as i64), sources) {
        if cancelled() {
            return Err(ExportError::Cancelled);
        }
        for (dst, src) in pending.iter_mut().zip(&planar) {
            dst.extend(src.iter().map(|s| quantise(*s, bps)));
        }
        flush(pending.as_mut_slice(), false, &mut out, &mut number)?;
    }
    flush(pending.as_mut_slice(), true, &mut out, &mut number)?;
    let n = out.finish(settings)?;
    if settings.part_of_batch {
        progress.done.store(1, Ordering::Relaxed);
    }
    Ok((n, frames))
}
