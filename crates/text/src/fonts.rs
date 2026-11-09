//! The font database: bundled fonts (always available, also on the web), the optional
//! [`CRAFT_FONTS`] (present when built with `CRAFT_FONTS_DIR`, also on the web) plus fonts
//! discovered by [`FontSource`]s (system font folders on native platforms).
//!
//! Faces are addressed by [`FaceId`] (an index that never changes for the life of the process).
//! [`resolve`] maps a family + style name (as stored in projects) to a face and the synthetic
//! bold/italic needed when the family lacks that style. Unknown families fall back to Inter.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

use crate::sfnt::{FaceNames, read_faces, read_faces_bytes};

/// Bundled fonts (SIL OFL 1.1; see `assets/fonts/*.attribution` and ATTRIBUTION.md).
pub static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
pub static INTER_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
pub static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
pub static INTER_BOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-Bold.ttf");
pub static INTER_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/Inter-Italic.ttf");
pub static JETBRAINS_MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
pub static NOTO_SERIF_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/NotoSerif-Regular.ttf");
/// Korean (and Latin) text face, so Hangul captions and titles look designed on every platform.
pub static PRETENDARD_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Pretendard-Regular.otf");
pub static PRETENDARD_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Pretendard-SemiBold.otf");
pub static PRETENDARD_BOLD: &[u8] = include_bytes!("../../../assets/fonts/Pretendard-Bold.otf");

/// A font from the optional craft-fonts build input (empty unless built with `CRAFT_FONTS_DIR`; see
/// `build.rs` and the craft-fonts repository's `docs/integration.md`).
pub struct CraftFont {
    pub family: &'static str,
    pub style: &'static str,
    /// ISO 15924 scripts the font is for, e.g. `"Jpan"`.
    pub scripts: &'static [&'static str],
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/craft_fonts.rs"));

/// The craft-fonts entries for Japanese (`Jpan`), in [`CRAFT_FONTS`] order. Empty when the app was
/// built without craft-fonts.
pub fn craft_japanese() -> impl Iterator<Item = &'static CraftFont> {
    CRAFT_FONTS.iter().filter(|f| f.scripts.contains(&"Jpan"))
}

/// Origin of faces registered from [`CRAFT_FONTS`].
pub const CRAFT_ORIGIN: &str = "craft-fonts";

/// The default family for new text.
pub const DEFAULT_FAMILY: &str = "Inter";

/// Index of a face in the database.
pub type FaceId = usize;

/// Where a face's bytes come from.
#[derive(Clone, Debug)]
pub enum FaceData {
    Static(&'static [u8]),
    Shared(Arc<Vec<u8>>),
    /// A file read on first use.
    File(PathBuf),
}

/// Description of a face offered by a [`FontSource`].
#[derive(Clone, Debug)]
pub struct FaceInfo {
    pub family: String,
    pub style: String,
    pub weight: u16,
    pub italic: bool,
    pub index: u32,
    pub data: FaceData,
    /// "bundled", "system", …
    pub origin: &'static str,
}

/// Something that can list fonts (system folders, a web font picker, a project's font folder…).
pub trait FontSource: Send + Sync {
    fn faces(&self) -> Vec<FaceInfo>;
}

/// Scans font folders on disk (reading only name tables). Native only; on the web there are no
/// folders and the scan finds nothing.
pub struct DirectorySource {
    pub dirs: Vec<PathBuf>,
}

impl DirectorySource {
    /// The platform's standard font folders.
    pub fn system() -> Self {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if cfg!(target_os = "macos") {
            dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from));
            if let Some(h) = &home {
                dirs.push(h.join("Library/Fonts"));
            }
        } else if cfg!(windows) {
            let win = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
            dirs.push(win.join("Fonts"));
            if let Some(l) = std::env::var_os("LOCALAPPDATA") {
                dirs.push(PathBuf::from(l).join("Microsoft\\Windows\\Fonts"));
            }
        } else if cfg!(unix) && !cfg!(target_arch = "wasm32") {
            dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
            if let Some(h) = &home {
                dirs.push(h.join(".local/share/fonts"));
                dirs.push(h.join(".fonts"));
            }
        }
        Self { dirs }
    }
}

fn walk(dir: &std::path::Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 4 {
                walk(&p, depth + 1, out);
            }
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc")) {
            out.push(p);
        }
    }
}

impl FontSource for DirectorySource {
    fn faces(&self) -> Vec<FaceInfo> {
        let mut files = Vec::new();
        for d in &self.dirs {
            walk(d, 0, &mut files);
        }
        files.sort();
        let mut out = Vec::new();
        for f in files {
            let Ok(mut file) = std::fs::File::open(&f) else { continue };
            for n in read_faces(&mut file) {
                out.push(info_from(n, FaceData::File(f.clone()), "system"));
            }
        }
        out
    }
}

fn info_from(n: FaceNames, data: FaceData, origin: &'static str) -> FaceInfo {
    FaceInfo { family: n.family, style: n.style, weight: n.weight, italic: n.italic, index: n.index, data, origin }
}

/// A loaded (or loadable) face.
pub struct Face {
    pub id: FaceId,
    pub info: FaceInfo,
    bytes: OnceLock<Option<FaceBytes>>,
    shaper: OnceLock<harfrust::ShaperData>,
}

#[derive(Clone)]
enum FaceBytes {
    Static(&'static [u8]),
    Shared(Arc<Vec<u8>>),
}

impl FaceBytes {
    fn get(&self) -> &[u8] {
        match self {
            FaceBytes::Static(b) => b,
            FaceBytes::Shared(b) => b,
        }
    }
}

/// Vertical metrics at a pixel size (all positive distances, y down from the baseline for the
/// underline position).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub cap_height: f32,
    pub x_height: f32,
    pub underline_pos: f32,
    pub underline_thickness: f32,
}

impl Face {
    fn bytes(&self) -> Option<&[u8]> {
        self.bytes
            .get_or_init(|| match &self.info.data {
                FaceData::Static(b) => Some(FaceBytes::Static(b)),
                FaceData::Shared(b) => Some(FaceBytes::Shared(b.clone())),
                FaceData::File(p) => std::fs::read(p).ok().map(|v| FaceBytes::Shared(Arc::new(v))),
            })
            .as_ref()
            .map(FaceBytes::get)
    }
    /// A copy of the font file's bytes (None if it is missing or unreadable).
    pub fn data(&self) -> Option<Vec<u8>> {
        self.bytes().map(<[u8]>::to_vec)
    }
    /// The parsed font (None if the file is missing or unreadable).
    pub fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(self.bytes()?, self.info.index).ok()
    }
    pub(crate) fn shaper_data(&self) -> Option<&harfrust::ShaperData> {
        let f = self.font()?;
        Some(self.shaper.get_or_init(|| harfrust::ShaperData::new(&f)))
    }
    pub fn units_per_em(&self) -> f32 {
        self.font().map(|f| f.metrics(Size::unscaled(), LocationRef::default()).units_per_em as f32).filter(|u| *u > 0.0).unwrap_or(1000.0)
    }
    /// Glyph for a character (None when the face does not cover it).
    pub fn glyph(&self, c: char) -> Option<u32> {
        self.font()?.charmap().map(c).map(|g| g.to_u32()).filter(|g| *g != 0)
    }
    pub fn has_char(&self, c: char) -> bool {
        self.glyph(c).is_some()
    }
    /// Whether the face covers every character of `text`. Unlike [`Face::has_char`], a face whose
    /// file is not loaded yet is read into a temporary buffer that is dropped afterwards, so
    /// probing many system faces does not keep every font file in memory.
    pub fn covers_text(&self, text: &str) -> bool {
        if let (FaceData::File(p), None) = (&self.info.data, self.bytes.get()) {
            let Ok(data) = std::fs::read(p) else { return false };
            let Ok(font) = FontRef::from_index(&data, self.info.index) else { return false };
            let charmap = font.charmap();
            return text.chars().all(|c| charmap.map(c).is_some_and(|g| g.to_u32() != 0));
        }
        text.chars().all(|c| self.has_char(c))
    }
    /// Height of the glyph's vertical origin above its baseline, in pixels.
    /// OpenType VORG takes precedence; TrueType uses the ink top plus vmtx's top bearing.
    pub(crate) fn vertical_origin(&self, gid: u32, px: f32) -> Option<f32> {
        use skrifa::raw::TableProvider;
        let font = self.font()?;
        let glyph = skrifa::GlyphId::new(gid);
        let units = match font.vorg() {
            Ok(table) => f32::from(table.vertical_origin_y(glyph)),
            Err(_) => {
                let bounds = font.glyph_metrics(Size::unscaled(), LocationRef::default()).bounds(glyph)?;
                bounds.y_max + f32::from(font.vmtx().ok()?.side_bearing(glyph)?)
            }
        };
        let origin = units * px / self.units_per_em();
        (origin.is_finite() && origin.abs() < px * 4.0).then_some(origin)
    }

    pub fn metrics(&self, px: f32) -> VMetrics {
        let Some(f) = self.font() else {
            return VMetrics {
                ascent: px * 0.8,
                descent: px * 0.2,
                cap_height: px * 0.7,
                x_height: px * 0.5,
                underline_pos: px * 0.1,
                underline_thickness: px * 0.06,
                ..Default::default()
            };
        };
        let m = f.metrics(Size::new(px), LocationRef::default());
        VMetrics {
            ascent: m.ascent,
            descent: -m.descent,
            line_gap: m.leading,
            cap_height: m.cap_height.unwrap_or(m.ascent * 0.7),
            x_height: m.x_height.unwrap_or(m.ascent * 0.5),
            underline_pos: m.underline.map_or(px * 0.1, |u| -u.offset),
            underline_thickness: m.underline.map_or(px * 0.06, |u| u.thickness.max(px * 0.03)),
        }
    }
    /// Whether the face's GSUB offers an OpenType feature (e.g. `smcp`).
    pub fn has_feature(&self, tag: &[u8; 4]) -> bool {
        use skrifa::raw::TableProvider;
        let Some(f) = self.font() else { return false };
        let Ok(gsub) = f.gsub() else { return false };
        let Ok(list) = gsub.feature_list() else { return false };
        list.feature_records().iter().any(|r| r.feature_tag().to_be_bytes() == *tag)
    }
}

struct Db {
    faces: Vec<Arc<Face>>,
    scanned: bool,
}

fn db() -> &'static RwLock<Db> {
    static DB: OnceLock<RwLock<Db>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = Db { faces: Vec::new(), scanned: false };
        for b in [
            INTER_REGULAR,
            INTER_MEDIUM,
            INTER_SEMIBOLD,
            INTER_BOLD,
            INTER_ITALIC,
            JETBRAINS_MONO_REGULAR,
            NOTO_SERIF_REGULAR,
            PRETENDARD_REGULAR,
            PRETENDARD_SEMIBOLD,
            PRETENDARD_BOLD,
        ] {
            for n in read_faces_bytes(b) {
                push(&mut db, info_from(n, FaceData::Static(b), "bundled"));
            }
        }
        // after the bundled faces, before anything a FontSource finds (a system copy of the same
        // family + style is then skipped as a duplicate)
        for f in CRAFT_FONTS {
            for n in read_faces_bytes(f.bytes) {
                push(&mut db, info_from(n, FaceData::Static(f.bytes), CRAFT_ORIGIN));
            }
        }
        RwLock::new(db)
    })
}

fn push(db: &mut Db, info: FaceInfo) -> FaceId {
    let id = db.faces.len();
    db.faces.push(Arc::new(Face { id, info, bytes: OnceLock::new(), shaper: OnceLock::new() }));
    id
}

/// Add faces from a source (skipping family+style pairs already present). Returns how many were
/// added.
pub fn add_source(src: &dyn FontSource) -> usize {
    let faces = src.faces();
    let mut d = db().write().unwrap_or_else(|e| e.into_inner());
    let mut n = 0;
    for f in faces {
        let dup = d.faces.iter().any(|g| g.info.family.eq_ignore_ascii_case(&f.family) && g.info.style.eq_ignore_ascii_case(&f.style));
        if !dup {
            push(&mut d, f);
            n += 1;
        }
    }
    n
}

/// Register a font from memory (e.g. a font file the user picked on the web). Returns its faces.
pub fn add_font_data(data: Vec<u8>) -> Vec<FaceId> {
    let data = Arc::new(data);
    let names = read_faces_bytes(&data);
    let mut d = db().write().unwrap_or_else(|e| e.into_inner());
    names.into_iter().map(|n| push(&mut d, info_from(n, FaceData::Shared(data.clone()), "user"))).collect()
}

/// Scan the system font folders once (native only; a no-op on wasm). Returns the number of faces
/// added by this call.
pub fn scan_system() -> usize {
    // Held for the whole scan: a concurrent caller waits for the complete list instead of
    // returning early and resolving against a half-filled database.
    static SCAN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _scan = SCAN.lock().unwrap_or_else(|e| e.into_inner());
    if system_scanned() {
        return 0;
    }
    let n = if cfg!(target_arch = "wasm32") { 0 } else { add_source(&DirectorySource::system()) };
    db().write().unwrap_or_else(|e| e.into_inner()).scanned = true;
    n
}

pub fn system_scanned() -> bool {
    db().read().unwrap_or_else(|e| e.into_inner()).scanned
}

/// A face by id (unknown ids give the default face).
// The bundled faces are compiled in (`include_bytes!`) and registered when the database is
// created, so `faces` is never empty.
#[allow(clippy::expect_used)]
pub fn face(id: FaceId) -> Arc<Face> {
    let d = db().read().unwrap_or_else(|e| e.into_inner());
    d.faces.get(id).or_else(|| d.faces.first()).cloned().expect("bundled fonts present")
}

/// All faces (bundled first).
pub fn all_faces() -> Vec<Arc<Face>> {
    db().read().unwrap_or_else(|e| e.into_inner()).faces.clone()
}

/// Families with their style names, sorted by family; styles in weight order.
pub fn families() -> Vec<(String, Vec<String>)> {
    let mut m: std::collections::BTreeMap<String, Vec<(bool, u16, String)>> = Default::default();
    for f in all_faces() {
        let v = m.entry(f.info.family.clone()).or_default();
        if !v.iter().any(|(_, _, s)| s == &f.info.style) {
            v.push((f.info.italic, f.info.weight, f.info.style.clone()));
        }
    }
    m.into_iter()
        .map(|(k, mut v)| {
            v.sort();
            (k, v.into_iter().map(|(_, _, s)| s).collect())
        })
        .collect()
}

/// The face chosen for a family/style request, plus synthetic emboldening / slant when the family
/// lacks a real bold / italic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Resolved {
    pub face: FaceId,
    pub synth_bold: bool,
    pub synth_italic: bool,
    /// The requested family was not found (Premiere shows such fonts as missing).
    pub missing: bool,
}

fn style_wants(style: &str) -> (u16, bool) {
    let s = style.to_ascii_lowercase().replace([' ', '-'], "");
    let italic = s.contains("italic") || s.contains("oblique");
    let w = if s.contains("thin") || s.contains("hairline") {
        100
    } else if s.contains("extralight") || s.contains("ultralight") {
        200
    } else if s.contains("light") {
        300
    } else if s.contains("medium") {
        500
    } else if s.contains("semibold") || s.contains("demibold") {
        600
    } else if s.contains("extrabold") || s.contains("ultrabold") {
        800
    } else if s.contains("black") || s.contains("heavy") {
        900
    } else if s.contains("bold") {
        700
    } else {
        400
    };
    (w, italic)
}

/// Resolve a family + style name. Family matching is case-insensitive; a style that the family
/// lacks picks the nearest weight and synthesises bold (≥ 600 requested, ≤ 500 found) or italic.
pub fn resolve(family: &str, style: &str) -> Resolved {
    if let Some(r) = resolve_in(family, style) {
        return r;
    }
    if !system_scanned() && !family.is_empty() && !family.eq_ignore_ascii_case(DEFAULT_FAMILY) {
        scan_system();
        if let Some(r) = resolve_in(family, style) {
            return r;
        }
    }
    let mut r = resolve_in(DEFAULT_FAMILY, style).unwrap_or(Resolved { face: 0, synth_bold: false, synth_italic: false, missing: true });
    r.missing = !family.is_empty() && !family.eq_ignore_ascii_case(DEFAULT_FAMILY);
    r
}

fn resolve_in(family: &str, style: &str) -> Option<Resolved> {
    let faces = all_faces();
    let fam: Vec<&Arc<Face>> = faces.iter().filter(|f| f.info.family.eq_ignore_ascii_case(family)).collect();
    if fam.is_empty() {
        return None;
    }
    if let Some(f) = fam.iter().find(|f| f.info.style.eq_ignore_ascii_case(style)) {
        return Some(Resolved { face: f.id, synth_bold: false, synth_italic: false, missing: false });
    }
    let (w, it) = style_wants(style);
    let best = fam.iter().min_by_key(|f| {
        let italic_pen = if f.info.italic == it { 0 } else { 1000 };
        italic_pen + (f.info.weight as i32 - w as i32).unsigned_abs()
    })?;
    Some(Resolved { face: best.id, synth_bold: w >= 600 && best.info.weight <= 500, synth_italic: it && !best.info.italic, missing: false })
}

/// Whether a family name reads as a serif face (Noto Serif, Shippori Mincho, …), to pick a Mincho
/// fallback for serif text and a Gothic one otherwise.
fn is_serif(family: &str) -> bool {
    let f = family.to_ascii_lowercase();
    (f.contains("serif") && !f.contains("sans")) || f.contains("mincho")
}

/// The writing systems that need a dedicated fallback family: the bundled fonts cover none of
/// them, and picking "any face with the glyph" gives Chinese glyph shapes to Japanese text (or the
/// other way round), since Han ideographs are shared, and a calligraphic display face to Arabic,
/// Hebrew, Indic or Thai text when that is the first installed face that covers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum FallbackScript {
    /// Kana: unambiguously Japanese.
    Japanese,
    /// Hangul: unambiguously Korean.
    Korean,
    /// Han ideographs, CJK punctuation and full-width forms: shared by Japanese, Chinese and
    /// Korean; the locale decides which family to try first.
    Han,
    Arabic,
    Hebrew,
    /// Devanagari, Bengali, Gurmukhi, Gujarati, Oriya, Tamil, Telugu, Kannada, Malayalam and
    /// Sinhala (one family covers them all on Windows; elsewhere each has its own).
    Indic,
    Thai,
    Other,
}

impl FallbackScript {
    /// Kana, Hangul or Han: these share the Han ideographs, so each tries the others' families
    /// next.
    fn is_cjk(self) -> bool {
        matches!(self, FallbackScript::Japanese | FallbackScript::Korean | FallbackScript::Han)
    }
}

fn script_of(c: char) -> FallbackScript {
    match c as u32 {
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => FallbackScript::Japanese,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF => FallbackScript::Korean,
        0x2E80..=0x2FDF
        | 0x3000..=0x303F
        | 0x3100..=0x312F
        | 0x3190..=0x31EF
        | 0x3200..=0x9FFF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF => FallbackScript::Han,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFE => FallbackScript::Arabic,
        0x0590..=0x05FF | 0xFB1D..=0xFB4F => FallbackScript::Hebrew,
        0x0900..=0x0DFF | 0xA8E0..=0xA8FF => FallbackScript::Indic,
        0x0E00..=0x0E7F => FallbackScript::Thai,
        _ => FallbackScript::Other,
    }
}

/// Families usually installed for each CJK language, in the order the platforms' own fallback
/// tables prefer them (Windows, macOS, then the Linux/Noto names). Missing ones cost one name
/// lookup each.
const JAPANESE_FAMILIES: &[&str] = &[
    "Yu Gothic",
    "Meiryo",
    "BIZ UDPGothic",
    "MS Gothic",
    "Yu Gothic UI",
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    "Noto Sans CJK JP",
    "Noto Sans JP",
    "Source Han Sans JP",
    "Source Han Sans",
    "IPAGothic",
    "VL Gothic",
];
/// Han ideograph families for Simplified Chinese, in the order the platforms' own fallback tables
/// prefer them (Windows, macOS, then the Linux/Noto names). Missing ones cost one name lookup each.
const SIMPLIFIED_CHINESE_FAMILIES: &[&str] =
    &["Microsoft YaHei", "SimSun", "PingFang SC", "Noto Sans CJK SC", "Noto Sans SC", "Source Han Sans SC", "WenQuanYi Micro Hei"];
/// The same for Traditional Chinese. The two scripts share almost all of their code points but not
/// the glyph forms (`东` against `東`'s neighbours, `直`, `骨`), so one list for both drew a
/// Traditional interface with Simplified forms - the faces do not differ in *coverage*, which is
/// what the fallback search checks, so the script has to pick the list.
const TRADITIONAL_CHINESE_FAMILIES: &[&str] =
    &["Microsoft JhengHei", "MingLiU", "PingFang TC", "Heiti TC", "Hiragino Sans CNS", "Noto Sans CJK TC", "Noto Sans TC", "Source Han Sans TC"];
const KOREAN_FAMILIES: &[&str] = &["Malgun Gothic", "Gulim", "Apple SD Gothic Neo", "Noto Sans CJK KR", "Noto Sans KR", "Source Han Sans KR", "NanumGothic"];
/// The platforms' plain text families for the other scripts the bundled fonts lack, in the same
/// order (Windows, macOS, then the Linux/Noto names).
const ARABIC_FAMILIES: &[&str] = &["Segoe UI", "Tahoma", "Geeza Pro", "Noto Sans Arabic", "Noto Naskh Arabic", "DejaVu Sans", "Arial"];
const HEBREW_FAMILIES: &[&str] = &["Segoe UI", "Tahoma", "Arial Hebrew", "Noto Sans Hebrew", "DejaVu Sans", "Arial"];
const INDIC_FAMILIES: &[&str] = &[
    "Nirmala UI",
    "Kohinoor Devanagari",
    "Kohinoor Bangla",
    "Kohinoor Gujarati",
    "Kohinoor Telugu",
    "Devanagari Sangam MN",
    "Bangla Sangam MN",
    "Gurmukhi Sangam MN",
    "Gujarati Sangam MN",
    "Oriya Sangam MN",
    "Tamil Sangam MN",
    "Telugu Sangam MN",
    "Kannada Sangam MN",
    "Malayalam Sangam MN",
    "Sinhala Sangam MN",
    "Noto Sans Devanagari",
    "Noto Sans Bengali",
    "Noto Sans Gurmukhi",
    "Noto Sans Gujarati",
    "Noto Sans Oriya",
    "Noto Sans Tamil",
    "Noto Sans Telugu",
    "Noto Sans Kannada",
    "Noto Sans Malayalam",
    "Noto Sans Sinhala",
];
const THAI_FAMILIES: &[&str] = &["Leelawadee UI", "Tahoma", "Thonburi", "Noto Sans Thai", "Noto Sans Thai UI", "Loma", "Garuda"];

/// The CJK locale the host asked fallbacks to prefer, when it has one (see [`set_cjk_locale`]).
static REQUESTED_CJK_LOCALE: RwLock<Option<String>> = RwLock::new(None);

/// Ask CJK font fallbacks to prefer `tag`'s language for Han ideographs instead of the locale
/// environment, so a Chinese interface is drawn with a Chinese face on a machine whose locale is
/// Japanese (and the other way round). The interface sets this from its language setting; an empty
/// tag, `c` or `posix` clears the request and returns to the environment. Process-global, like the
/// environment it replaces: every layout in the process follows it, so composition text is drawn
/// with the same language's Han forms as the interface, not only the interface chrome.
pub fn set_cjk_locale(tag: &str) {
    let tag = tag.trim().to_ascii_lowercase();
    let requested = if tag.is_empty() || tag == "c" || tag == "posix" { None } else { Some(tag) };
    *REQUESTED_CJK_LOCALE.write().unwrap_or_else(|e| e.into_inner()) = requested;
    // The order of the CJK families follows the language, so a change of language drops the cached
    // fallbacks.
    fallback_cache().write().unwrap_or_else(|e| e.into_inner()).clear();
    family_faces_cache().write().unwrap_or_else(|e| e.into_inner()).clear();
}

fn requested_cjk_locale() -> Option<String> {
    REQUESTED_CJK_LOCALE.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The CJK locale to use: [`set_cjk_locale`] when the host asked for one, otherwise the locale
/// environment (`LANG`, `LC_ALL`, `LC_CTYPE`; `REDROB_EFFECTS_CJK_LOCALE` overrides them).
fn active_cjk_locale() -> String {
    requested_cjk_locale().unwrap_or_else(|| {
        ["REDROB_EFFECTS_CJK_LOCALE", "LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .map(|v| v.trim().to_ascii_lowercase())
            .find(|v| !v.is_empty() && v != "c" && v != "posix")
            .unwrap_or_default()
    })
}

/// The CJK language to try first for Han ideographs. Japanese when nothing says otherwise: the
/// ambiguity only matters for ideographs, and kana / hangul in the same text pick their language
/// regardless.
fn han_order_for(tag: &str) -> [FallbackScript; 3] {
    let tag = tag.trim().to_ascii_lowercase();
    if tag.starts_with("zh") {
        [FallbackScript::Han, FallbackScript::Japanese, FallbackScript::Korean]
    } else if tag.starts_with("ko") {
        [FallbackScript::Korean, FallbackScript::Japanese, FallbackScript::Han]
    } else {
        [FallbackScript::Japanese, FallbackScript::Han, FallbackScript::Korean]
    }
}

/// The order for the locale in force ([`active_cjk_locale`]).
fn han_order() -> [FallbackScript; 3] {
    han_order_for(&active_cjk_locale())
}

/// Whether a CJK tag asks for Traditional Chinese: a `Hant` script, or a Taiwan, Hong Kong or
/// Macao region (`zh-TW`, `zh_HK`, `zh_MO.UTF-8`). Anything else Chinese - including a bare `zh` -
/// is Simplified, as in the interface's own locale mapping.
fn asks_traditional_chinese(tag: &str) -> bool {
    tag.to_ascii_lowercase().split(['-', '_', '.']).any(|p| matches!(p, "hant" | "tw" | "hk" | "mo"))
}

/// The Chinese families to try first for `tag`: one list per script, since coverage does not tell
/// the two apart - a Simplified face covers the Traditional glyphs' code points, so the fallback
/// search would keep it and draw the wrong forms.
fn chinese_families_for(tag: &str) -> &'static [&'static str] {
    if asks_traditional_chinese(tag) { TRADITIONAL_CHINESE_FAMILIES } else { SIMPLIFIED_CHINESE_FAMILIES }
}

/// [`chinese_families_for`] for the locale in force ([`active_cjk_locale`]).
fn chinese_families() -> &'static [&'static str] {
    chinese_families_for(&active_cjk_locale())
}

fn families_for(s: FallbackScript) -> &'static [&'static str] {
    match s {
        FallbackScript::Japanese => JAPANESE_FAMILIES,
        FallbackScript::Han => chinese_families(),
        FallbackScript::Korean => KOREAN_FAMILIES,
        FallbackScript::Arabic => ARABIC_FAMILIES,
        FallbackScript::Hebrew => HEBREW_FAMILIES,
        FallbackScript::Indic => INDIC_FAMILIES,
        FallbackScript::Thai => THAI_FAMILIES,
        FallbackScript::Other => &[],
    }
}

/// Whether a not-yet-loaded face covers `c`, reading its file without keeping the bytes (the
/// last-resort search may look at every installed font once).
fn file_covers(f: &Face, c: char) -> bool {
    if f.bytes.get().is_some() {
        return f.has_char(c);
    }
    let FaceData::File(p) = &f.info.data else { return f.has_char(c) };
    let Ok(data) = std::fs::read(p) else { return false };
    FontRef::from_index(&data, f.info.index).ok().is_some_and(|font| covers(&font, c))
}

/// Characters that legitimately draw nothing: whitespace and the default-ignorable code points
/// (soft hyphen, joiners, bidi controls, variation selectors…).
fn blank_by_nature(c: char) -> bool {
    c.is_whitespace()
        || matches!(c as u32,
            0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F | 0x200B..=0x200F
            | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
}

/// Whether `font` maps `c` to a glyph it can draw (see [`Face::has_char`]).
fn covers(font: &FontRef, c: char) -> bool {
    let Some(gid) = font.charmap().map(c).filter(|g| g.to_u32() != 0) else { return false };
    blank_by_nature(c) || glyph_draws(font, gid)
}

/// Whether a glyph has an outline, a colour glyph or a bitmap in any strike.
fn glyph_draws(font: &FontRef, gid: skrifa::GlyphId) -> bool {
    use skrifa::outline::{DrawSettings, OutlinePen};
    /// Counts the drawing segments of an outline.
    struct Segments(usize);
    impl OutlinePen for Segments {
        fn move_to(&mut self, _: f32, _: f32) {}
        fn line_to(&mut self, _: f32, _: f32) {
            self.0 += 1;
        }
        fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
            self.0 += 1;
        }
        fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
            self.0 += 1;
        }
        fn close(&mut self) {}
    }
    if let Some(outline) = font.outline_glyphs().get(gid) {
        let mut pen = Segments(0);
        if outline.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen).is_ok() && pen.0 > 0 {
            return true;
        }
    }
    if font.color_glyphs().get(gid).is_some() {
        return true;
    }
    let strikes = font.bitmap_strikes();
    (0..strikes.len()).any(|i| strikes.get(i).is_some_and(|s| s.get(gid).is_some()))
}

/// Results of the search over every installed font in [`fallback_for`], per 256-character block and
/// preferred face (so bold text gets the bold cut of the fallback family). `None` records that
/// nothing installed covers the block, so the search over every font runs once per block, not once
/// per character.
fn fallback_cache() -> &'static RwLock<std::collections::HashMap<(u32, FaceId), Option<FaceId>>> {
    static CACHE: OnceLock<RwLock<std::collections::HashMap<(u32, FaceId), Option<FaceId>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The results of [`family_faces`], per script and preferred face.
fn family_faces_cache() -> &'static RwLock<std::collections::HashMap<(FallbackScript, FaceId), Arc<Vec<FaceId>>>> {
    static CACHE: OnceLock<RwLock<std::collections::HashMap<(FallbackScript, FaceId), Arc<Vec<FaceId>>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// A face that covers `c`, preferring `prefer`. Bundled faces are tried first (cheap), then the
/// craft-fonts faces. For a
/// character of a script with usual families (CJK, Arabic, Hebrew, Indic, Thai), the platform's
/// usual families for that script are tried next, in the preferred face's style. Faces already
/// loaded are tried after that (cheap), and any installed face that covers the character is tried
/// last. `prefer` is returned when nothing covers it (the glyph renders as the missing-glyph box).
pub fn fallback_for(c: char, prefer: FaceId) -> FaceId {
    let p = face(prefer);
    if p.has_char(c) || c.is_control() {
        return prefer;
    }
    let faces = all_faces();
    // prefer a face of the same italic-ness that is bundled (cheap)
    // Pretendard is bundled for Hangul only: its kana and ideographs would give Japanese and Chinese
    // text Korean glyph shapes, so those keep their own script's families below.
    let hangul = matches!(c as u32, 0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF);
    let mut covering: Vec<&Arc<Face>> =
        faces.iter().filter(|f| f.info.origin == "bundled" && f.has_char(c) && (hangul || !f.info.family.eq_ignore_ascii_case("Pretendard"))).collect();
    covering.sort_by_key(|f| (f.info.italic != p.info.italic, f.info.weight.abs_diff(p.info.weight)));
    if let Some(f) = covering.first() {
        return f.id;
    }
    // then the craft-fonts faces (Japanese): Mincho for serif text, Gothic otherwise, nearest weight
    let serif = is_serif(&p.info.family);
    let mut craft: Vec<&Arc<Face>> = faces.iter().filter(|f| f.info.origin == CRAFT_ORIGIN).collect();
    craft.sort_by_key(|f| (is_serif(&f.info.family) != serif, f.info.italic != p.info.italic, f.info.weight.abs_diff(p.info.weight)));
    for f in craft {
        if f.has_char(c) {
            return f.id;
        }
    }
    // The usual families of the character's script come before the faces that are loaded already
    // and before the search results of its block. Which faces are loaded depends on every earlier
    // lookup, and Arial Unicode MS, the first face in the macOS scan, covers every script with
    // usual families except Sinhala.
    let script = script_of(c);
    if script != FallbackScript::Other {
        if cfg!(not(target_arch = "wasm32")) && !system_scanned() {
            scan_system();
        }
        if let Some(&id) = family_faces(script, &p).iter().find(|&&id| face(id).has_char(c)) {
            return id;
        }
    }
    // Faces that are loaded already come next (cheap).
    for f in faces.iter().filter(|f| f.info.origin != "bundled" && f.bytes.get().is_some()) {
        if f.has_char(c) {
            return f.id;
        }
    }
    let key = (c as u32 >> 8, prefer);
    if let Some(cached) = fallback_cache().read().unwrap_or_else(|e| e.into_inner()).get(&key).copied() {
        match cached {
            Some(id) if face(id).has_char(c) => return id,
            Some(_) => {}
            None => return prefer,
        }
    }
    if cfg!(not(target_arch = "wasm32")) && !system_scanned() {
        scan_system();
    }
    let found = system_fallback(c, &p.info.style);
    fallback_cache().write().unwrap_or_else(|e| e.into_inner()).insert(key, found);
    found.unwrap_or(prefer)
}

/// The script-aware part of [`fallback_for`]: the installed faces of the platform's usual families
/// for `script`, in the order they are tried, each in the style of `prefer`. For CJK, the other CJK
/// languages' families follow in the locale's order.
fn family_faces(script: FallbackScript, prefer: &Face) -> Arc<Vec<FaceId>> {
    if let Some(faces) = family_faces_cache().read().unwrap_or_else(|e| e.into_inner()).get(&(script, prefer.id)) {
        return faces.clone();
    }
    let order: Vec<FallbackScript> = match script {
        FallbackScript::Other => vec![],
        FallbackScript::Han => han_order().to_vec(),
        s if s.is_cjk() => {
            let mut v = vec![s];
            v.extend(han_order().iter().copied().filter(|o| *o != s));
            v
        }
        s => vec![s],
    };
    let style = &prefer.info.style;
    let faces: Arc<Vec<FaceId>> = Arc::new(order.into_iter().flat_map(families_for).filter_map(|fam| resolve_in(fam, style)).map(|r| r.face).collect());
    family_faces_cache().write().unwrap_or_else(|e| e.into_inner()).insert((script, prefer.id), faces.clone());
    faces
}

/// The exhaustive search behind [`fallback_for`]: anything installed, in scan order; the style's
/// weight and slant break ties among the faces of the first covering family.
fn system_fallback(c: char, style: &str) -> Option<FaceId> {
    let faces = all_faces();
    let first = faces.iter().find(|f| f.info.origin != "bundled" && file_covers(f, c))?;
    let r = resolve_in(&first.info.family, style)?;
    Some(if face(r.face).has_char(c) { r.face } else { first.id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_families_and_resolution() {
        let fams = families();
        let inter = fams.iter().find(|(f, _)| f == "Inter").expect("Inter");
        for s in ["Regular", "Medium", "SemiBold", "Bold", "Italic"] {
            assert!(inter.1.iter().any(|x| x == s), "{s} in {:?}", inter.1);
        }
        assert!(fams.iter().any(|(f, _)| f == "Noto Serif"));
        assert!(fams.iter().any(|(f, _)| f == "JetBrains Mono"));
        let b = resolve("inter", "bold");
        assert_eq!(face(b.face).info.style, "Bold");
        assert!(!b.synth_bold);
        // Noto Serif has only Regular: Bold Italic is synthesised
        let s = resolve("Noto Serif", "Bold Italic");
        assert_eq!(face(s.face).info.family, "Noto Serif");
        assert!(s.synth_bold && s.synth_italic);
        // Inter Bold Italic: nearest is Italic (400) → synth bold
        let bi = resolve("Inter", "Bold Italic");
        assert!(face(bi.face).info.italic && bi.synth_bold && !bi.synth_italic);
    }

    #[test]
    fn missing_family_falls_back_to_inter() {
        let r = resolve("No Such Font Family 123", "Regular");
        assert_eq!(face(r.face).info.family, "Inter");
        assert!(r.missing);
    }

    /// Native: the system scan reads name tables only and resolves families installed there.
    #[test]
    fn system_scan_finds_fonts() {
        let t = std::time::Instant::now();
        scan_system(); // (another test may have triggered the scan already)
        let n = all_faces().iter().filter(|f| f.info.origin == "system").count();
        eprintln!("system fonts: {n} faces in {:?}", t.elapsed());
        assert_eq!(scan_system(), 0, "scans once");
        if cfg!(target_os = "macos") {
            assert!(n > 20, "{n}");
            let r = resolve("Helvetica", "Bold");
            assert_eq!(face(r.face).info.family, "Helvetica");
            let l = crate::layout("Helvetica", &crate::TextStyle { family: "Helvetica".into(), size: 30.0, ..Default::default() }, &Default::default());
            assert!(!l.missing_font && l.glyphs.iter().all(|g| g.id != 0));
        }
    }

    /// craft-fonts faces are registered after the bundled ones and serve Japanese characters the
    /// requested face lacks: Gothic for sans text, Mincho for serif text.
    #[test]
    fn japanese_falls_back_to_craft_fonts() {
        let craft: Vec<_> = all_faces().into_iter().filter(|f| f.info.origin == CRAFT_ORIGIN).collect();
        if craft_japanese().next().is_none() {
            eprintln!("SKIPPED: built without craft-fonts (set CRAFT_FONTS_DIR to run)");
            assert!(craft.is_empty());
            return;
        }
        assert!(craft.len() >= craft_japanese().count(), "{} faces", craft.len());
        let bundled = all_faces().iter().take_while(|f| f.info.origin == "bundled").count();
        assert!(craft.iter().all(|f| f.id >= bundled), "craft-fonts faces come after the bundled faces");
        let sans = resolve("Inter", "Regular").face;
        let serif = resolve("Noto Serif", "Regular").face;
        for c in "日本語の文字".chars() {
            let g = face(fallback_for(c, sans));
            assert_eq!(g.info.origin, CRAFT_ORIGIN, "{c}");
            assert!(g.has_char(c) && !is_serif(&g.info.family), "{c}: {}", g.info.family);
            let m = face(fallback_for(c, serif));
            assert!(m.has_char(c) && is_serif(&m.info.family), "{c}: {}", m.info.family);
        }
        // the bold UI face for bold text
        let bold = face(fallback_for('日', resolve("Inter", "Bold").face));
        assert!(bold.info.weight >= 600, "{} {}", bold.info.family, bold.info.style);
        // and laid out with real glyphs (no tofu)
        let l = crate::layout("日本語の文字", &crate::TextStyle { size: 40.0, ..Default::default() }, &Default::default());
        assert_eq!(l.glyphs.len(), 6);
        assert!(l.glyphs.iter().all(|g| g.id != 0), "{:?}", l.glyphs);
    }

    /// Hangul in the default family (Inter has none) is drawn with an installed Korean face, not
    /// missing-glyph boxes - the burned-in captions of a Korean short were all boxes before.
    #[test]
    fn hangul_falls_back_to_a_system_face() {
        scan_system();
        if !all_faces().iter().any(|f| f.info.origin == "system" && f.covers_text("한")) {
            eprintln!("SKIPPED: no installed font covers Hangul");
            return;
        }
        let inter = resolve("Inter", "Regular").face;
        let k = face(fallback_for('한', inter));
        assert!(k.has_char('한'), "{}", k.info.family);
        let l = crate::layout("한국어 자막", &crate::TextStyle { size: 40.0, ..Default::default() }, &Default::default());
        assert!(l.glyphs.iter().all(|g| g.id != 0), "{:?}", l.glyphs);
    }

    #[test]
    fn metrics_and_coverage() {
        let f = face(resolve("Inter", "Regular").face);
        let m = f.metrics(100.0);
        assert!(m.ascent > 80.0 && m.ascent < 110.0, "{m:?}");
        assert!(m.descent > 10.0 && m.descent < 40.0);
        assert!(m.cap_height > 60.0 && m.cap_height < 80.0);
        assert!(f.has_char('A') && f.has_char('Ж'));
        assert!(!f.has_char('\u{5d0}'), "Inter has no Hebrew");
        assert!(f.has_feature(b"liga") || f.has_feature(b"calt"));
    }

    #[test]
    fn covers_text_does_not_keep_file_bytes() {
        let path = std::env::temp_dir().join(format!("redrob-film-covers-text-{}.ttf", std::process::id()));
        std::fs::write(&path, INTER_REGULAR).expect("write temp font");
        let info = FaceInfo {
            family: "Inter".into(),
            style: "Regular".into(),
            weight: 400,
            italic: false,
            index: 0,
            data: FaceData::File(path.clone()),
            origin: "system",
        };
        let f = Face { id: 0, info, bytes: OnceLock::new(), shaper: OnceLock::new() };
        assert!(f.covers_text("A\u{416}"));
        assert!(!f.covers_text("A\u{5d0}"));
        assert!(f.bytes.get().is_none(), "probing coverage must not cache the file");
        assert!(f.data().is_some());
        assert!(f.bytes.get().is_some());
        let _ = std::fs::remove_file(&path);
    }
}
