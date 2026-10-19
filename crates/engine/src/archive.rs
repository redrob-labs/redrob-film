//! Project archives (Kdenlive's Archive Project / Open Archived Project): a Project Manager copy
//! packed into one `.tar.gz` or `.zip` file, and the way back.
//!
//! - **Pack** ([`pack`]): every file under the collected folder, paths relative to it, plus
//!   `archive.json` (`{"format": "filmcraft.archive", "root": <the folder>, "project": <file>}`).
//!   tar is POSIX ustar (names up to 255 bytes) through gzip; zip entries are deflated, with data
//!   descriptors so nothing is held in memory (zip64 is not written: a file over 4 GiB is refused).
//! - **Open** (`file.openArchive`): extract next to the archive (or to `destination`), refusing
//!   absolute paths and `..`, open the project inside, and relink the media from the folder the
//!   archive was made from to where it was extracted (`media.autoRelink`).

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session};

pub const MANIFEST: &str = "archive.json";

/// The archive kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    TarGz,
    Zip,
}

impl Kind {
    pub fn from_name(s: &str) -> Option<Kind> {
        match s.to_ascii_lowercase().replace(['.', '-', '_', ' '], "").as_str() {
            "targz" | "tgz" | "tar" | "gz" => Some(Kind::TarGz),
            "zip" => Some(Kind::Zip),
            _ => None,
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Kind::TarGz => "tar.gz",
            Kind::Zip => "zip",
        }
    }
    /// The kind of an archive file from its name.
    pub fn of_path(p: &Path) -> Option<Kind> {
        let n = p.file_name()?.to_string_lossy().to_ascii_lowercase();
        if n.ends_with(".tar.gz") || n.ends_with(".tgz") {
            Some(Kind::TarGz)
        } else if n.ends_with(".zip") {
            Some(Kind::Zip)
        } else {
            None
        }
    }
}

/// Every file under `dir` (relative paths with `/`), sorted.
fn walk(dir: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![(String::new(), dir.to_path_buf())];
    while let Some((rel, d)) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if e.file_type()?.is_dir() {
                stack.push((r, e.path()));
            } else {
                out.push((r, e.path()));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn io(e: std::io::Error) -> String {
    e.to_string()
}

/// Pack `dir` (with a manifest naming `project`) into `out`. Returns the archive's size.
pub fn pack(dir: &Path, project: &str, out: &Path, kind: Kind) -> std::result::Result<u64, String> {
    let manifest = json!({"format": "filmcraft.archive", "version": 1, "root": dir.to_string_lossy(), "project": project});
    std::fs::write(dir.join(MANIFEST), serde_json::to_vec_pretty(&manifest).unwrap_or_default()).map_err(io)?;
    let files = walk(dir).map_err(io)?;
    match kind {
        Kind::TarGz => write_tar_gz(&files, out)?,
        Kind::Zip => write_zip(&files, out)?,
    }
    std::fs::metadata(out).map(|m| m.len()).map_err(io)
}

fn octal(field: &mut [u8], v: u64) {
    let n = field.len() - 1;
    let s = format!("{v:0n$o}");
    let b = s.as_bytes();
    let b = &b[b.len().saturating_sub(n)..];
    field[..b.len()].copy_from_slice(b);
    field[n] = 0;
}

/// A ustar header for `name` (split into prefix / name when over 100 bytes).
fn tar_header(name: &str, size: u64, mtime: u64) -> std::result::Result<[u8; 512], String> {
    let mut h = [0u8; 512];
    let (prefix, base) = if name.len() <= 100 {
        ("", name)
    } else {
        // '/' is ASCII, so the cut is always on a character boundary
        let cut = name.as_bytes()[..name.len().min(156)]
            .iter()
            .rposition(|b| *b == b'/')
            .filter(|&i| name.len() - i - 1 <= 100)
            .ok_or_else(|| format!("{name}: path too long for tar"))?;
        (&name[..cut], &name[cut + 1..])
    };
    h[..base.len()].copy_from_slice(base.as_bytes());
    octal(&mut h[100..108], 0o644);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    if size > 0o77777777777 {
        return Err(format!("{name}: file too large for tar"));
    }
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], mtime);
    h[148..156].copy_from_slice(b"        ");
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
    Ok(h)
}

fn write_tar_gz(files: &[(String, PathBuf)], out: &Path) -> std::result::Result<(), String> {
    let f = File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut gz = flate2::write::GzEncoder::new(BufWriter::new(f), flate2::Compression::fast());
    for (rel, path) in files {
        let md = std::fs::metadata(path).map_err(io)?;
        let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        gz.write_all(&tar_header(rel, md.len(), mtime)?).map_err(io)?;
        let mut src = BufReader::new(File::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
        let n = std::io::copy(&mut src, &mut gz).map_err(io)?;
        let pad = (512 - (n % 512) as usize) % 512;
        gz.write_all(&vec![0u8; pad]).map_err(io)?;
    }
    gz.write_all(&[0u8; 1024]).map_err(io)?;
    gz.finish().map_err(io)?.flush().map_err(io)
}

/// Counts what passes through to the inner writer.
struct Counting<W: Write> {
    inner: W,
    n: u64,
}

impl<W: Write> Write for Counting<W> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let k = self.inner.write(b)?;
        self.n += k as u64;
        Ok(k)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn write_zip(files: &[(String, PathBuf)], out: &Path) -> std::result::Result<(), String> {
    let f = File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut w = Counting { inner: BufWriter::new(f), n: 0 };
    let mut central = Vec::new();
    let mut count = 0u16;
    for (rel, path) in files {
        let offset = u32::try_from(w.n).map_err(|_| "archive over 4 GiB: use tar.gz".to_string())?;
        let name = rel.as_bytes();
        // local header: deflate, sizes and CRC in the data descriptor (bit 3), UTF-8 names (bit 11)
        let flags: u16 = 0x0808;
        let mut lh = Vec::with_capacity(30 + name.len());
        lh.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        lh.extend_from_slice(&20u16.to_le_bytes());
        lh.extend_from_slice(&flags.to_le_bytes());
        lh.extend_from_slice(&8u16.to_le_bytes());
        lh.extend_from_slice(&[0, 0, 0x21, 0]); // time 00:00, date 1980-01-01
        lh.extend_from_slice(&[0u8; 12]);
        lh.extend_from_slice(&(name.len() as u16).to_le_bytes());
        lh.extend_from_slice(&0u16.to_le_bytes());
        lh.extend_from_slice(name);
        w.write_all(&lh).map_err(io)?;
        let start = w.n;
        let mut crc = flate2::Crc::new();
        let mut size = 0u64;
        {
            let mut enc = flate2::write::DeflateEncoder::new(&mut w, flate2::Compression::fast());
            let mut src = BufReader::new(File::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
            let mut buf = vec![0u8; 1 << 16];
            loop {
                let k = src.read(&mut buf).map_err(io)?;
                if k == 0 {
                    break;
                }
                crc.update(&buf[..k]);
                size += k as u64;
                enc.write_all(&buf[..k]).map_err(io)?;
            }
            enc.finish().map_err(io)?;
        }
        let comp = w.n - start;
        let (comp, size) = (u32::try_from(comp).map_err(|_| format!("{rel}: over 4 GiB, use tar.gz"))?, u32::try_from(size).map_err(|_| format!("{rel}: over 4 GiB, use tar.gz"))?);
        let mut dd = Vec::with_capacity(16);
        dd.extend_from_slice(&0x0807_4b50u32.to_le_bytes());
        dd.extend_from_slice(&crc.sum().to_le_bytes());
        dd.extend_from_slice(&comp.to_le_bytes());
        dd.extend_from_slice(&size.to_le_bytes());
        w.write_all(&dd).map_err(io)?;
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&flags.to_le_bytes());
        central.extend_from_slice(&8u16.to_le_bytes());
        central.extend_from_slice(&[0, 0, 0x21, 0]);
        central.extend_from_slice(&crc.sum().to_le_bytes());
        central.extend_from_slice(&comp.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0u8; 8]); // extra, comment, disk, internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
        count = count.checked_add(1).ok_or("too many files for zip")?;
    }
    let cd_start = u32::try_from(w.n).map_err(|_| "archive over 4 GiB: use tar.gz".to_string())?;
    w.write_all(&central).map_err(io)?;
    let mut end = Vec::with_capacity(22);
    end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    end.extend_from_slice(&[0u8; 4]);
    end.extend_from_slice(&count.to_le_bytes());
    end.extend_from_slice(&count.to_le_bytes());
    end.extend_from_slice(&(central.len() as u32).to_le_bytes());
    end.extend_from_slice(&cd_start.to_le_bytes());
    end.extend_from_slice(&0u16.to_le_bytes());
    w.write_all(&end).map_err(io)?;
    w.flush().map_err(io)
}

/// `rel` joined to `dest`, or an error when it would leave `dest`.
fn safe_join(dest: &Path, rel: &str) -> std::result::Result<PathBuf, String> {
    let p = Path::new(rel);
    if p.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(format!("{rel}: unsafe path in archive"));
    }
    Ok(dest.join(p))
}

fn parse_octal(b: &[u8]) -> u64 {
    b.iter().take_while(|c| **c != 0 && **c != b' ').fold(0, |a, c| if (b'0'..=b'7').contains(c) { a * 8 + u64::from(c - b'0') } else { a })
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> std::result::Result<(), String> {
    let f = File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut r = flate2::read::GzDecoder::new(BufReader::new(f));
    let mut h = [0u8; 512];
    loop {
        r.read_exact(&mut h).map_err(io)?;
        if h.iter().all(|b| *b == 0) {
            return Ok(());
        }
        let field = |a: usize, b: usize| String::from_utf8_lossy(h[a..b].split(|c| *c == 0).next().unwrap_or_default()).into_owned();
        let (name, prefix) = (field(0, 100), field(345, 500));
        let name = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let size = parse_octal(&h[124..136]);
        let typ = h[156];
        let padded = size.div_ceil(512) * 512;
        if typ == b'0' || typ == 0 {
            let out = safe_join(dest, &name)?;
            if let Some(d) = out.parent() {
                std::fs::create_dir_all(d).map_err(io)?;
            }
            let mut w = BufWriter::new(File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?);
            std::io::copy(&mut (&mut r).take(size), &mut w).map_err(io)?;
            w.flush().map_err(io)?;
            std::io::copy(&mut (&mut r).take(padded - size), &mut std::io::sink()).map_err(io)?;
        } else {
            // directories, links and extended headers carry nothing to write
            std::io::copy(&mut (&mut r).take(padded), &mut std::io::sink()).map_err(io)?;
        }
    }
}

fn le16(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 2).map_or(0, |x| u64::from(u16::from_le_bytes([x[0], x[1]])))
}

fn le32(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 4).map_or(0, |x| u64::from(u32::from_le_bytes([x[0], x[1], x[2], x[3]])))
}

fn extract_zip(archive: &Path, dest: &Path) -> std::result::Result<(), String> {
    let mut f = File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let len = f.metadata().map_err(io)?.len();
    // the end of central directory record is in the last 64 KiB + 22 bytes
    let tail_len = len.min(65_557);
    f.seek(SeekFrom::Start(len - tail_len)).map_err(io)?;
    let mut tail = vec![0u8; tail_len as usize];
    f.read_exact(&mut tail).map_err(io)?;
    let eocd = (0..tail.len().saturating_sub(21)).rev().find(|&i| le32(&tail, i) == 0x0605_4b50).ok_or("not a zip file")?;
    let (count, cd_size, cd_off) = (le16(&tail, eocd + 10), le32(&tail, eocd + 12), le32(&tail, eocd + 16));
    f.seek(SeekFrom::Start(cd_off)).map_err(io)?;
    let mut cd = vec![0u8; cd_size as usize];
    f.read_exact(&mut cd).map_err(io)?;
    let mut at = 0usize;
    for _ in 0..count {
        if le32(&cd, at) != 0x0201_4b50 {
            return Err("damaged zip central directory".into());
        }
        let method = le16(&cd, at + 10);
        let comp = le32(&cd, at + 20);
        let (nlen, xlen, clen) = (le16(&cd, at + 28) as usize, le16(&cd, at + 30) as usize, le16(&cd, at + 32) as usize);
        let local = le32(&cd, at + 42);
        let name = String::from_utf8_lossy(cd.get(at + 46..at + 46 + nlen).ok_or("damaged zip")?).into_owned();
        at += 46 + nlen + xlen + clen;
        if name.ends_with('/') {
            continue;
        }
        let mut lh = [0u8; 30];
        f.seek(SeekFrom::Start(local)).map_err(io)?;
        f.read_exact(&mut lh).map_err(io)?;
        let skip = le16(&lh, 26) + le16(&lh, 28);
        f.seek(SeekFrom::Current(skip as i64)).map_err(io)?;
        let out = safe_join(dest, &name)?;
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(io)?;
        }
        let mut w = BufWriter::new(File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?);
        let data = (&mut f).take(comp);
        match method {
            0 => {
                std::io::copy(&mut BufReader::new(data), &mut w).map_err(io)?;
            }
            8 => {
                std::io::copy(&mut flate2::read::DeflateDecoder::new(BufReader::new(data)), &mut w).map_err(io)?;
            }
            m => return Err(format!("{name}: zip method {m} is not supported")),
        }
        w.flush().map_err(io)?;
    }
    Ok(())
}

/// Extract an archive into `dest`.
pub fn extract(archive: &Path, dest: &Path) -> std::result::Result<(), String> {
    std::fs::create_dir_all(dest).map_err(io)?;
    match Kind::of_path(archive) {
        Some(Kind::TarGz) => extract_tar_gz(archive, dest),
        Some(Kind::Zip) => extract_zip(archive, dest),
        None => Err(format!("{}: not a .tar.gz or .zip archive", archive.display())),
    }
}

/// `file.openArchive {path, destination?}`.
fn open_archive(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "file.openArchive";
    let path = PathBuf::from(str_p(p, "path").ok_or_else(|| bad(cmd, "need `path`"))?);
    Kind::of_path(&path).ok_or_else(|| bad(cmd, "the archive must be .tar.gz, .tgz or .zip"))?;
    let dest = match str_p(p, "destination") {
        Some(d) => PathBuf::from(d),
        None => {
            let n = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let stem = n.strip_suffix(".tar.gz").or_else(|| n.strip_suffix(".tgz")).or_else(|| n.strip_suffix(".zip")).unwrap_or(&n).to_string();
            path.parent().unwrap_or(Path::new(".")).join(stem)
        }
    };
    extract(&path, &dest).map_err(EngineError::Other)?;
    let manifest: Value = std::fs::read(dest.join(MANIFEST)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let project = manifest["project"].as_str().map(|n| dest.join(n)).filter(|p| p.is_file()).or_else(|| {
        walk(&dest).ok()?.into_iter().map(|(_, p)| p).find(|p| p.extension().and_then(|e| e.to_str()) == Some("rfilm"))
    });
    let project = project.ok_or_else(|| EngineError::Other("the archive holds no project file".into()))?;
    let project_s = project.to_string_lossy().into_owned();
    let mut out = s.execute("file.open", json!({"path": project_s}))?;
    // the media paths point into the folder the archive was made from: move them here
    if let Some(root) = manifest["root"].as_str()
        && Path::new(root) != dest
    {
        let to = dest.to_string_lossy().into_owned();
        match s.execute("media.autoRelink", json!({"from": root, "to": to})) {
            Ok(v) => out["relinked"] = v,
            Err(e) => out["relinkError"] = json!(e.to_string()),
        }
    }
    out["extractedTo"] = json!(dest);
    Ok(out)
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "file.openArchive",
        label: "Open Archived Project…",
        menu: &["File"],
        shortcut: None,
        params: r#"{"path":".tar.gz|.tgz|.zip","destination":dir?}"#,
        enabled: always,
        run: open_archive,
        journal: true,
    }]
}
