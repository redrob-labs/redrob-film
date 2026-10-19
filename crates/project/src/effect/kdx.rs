//! More Video Effects for editors coming from Kdenlive: alpha / mask / keying tools, burnt-in
//! scopes and measuring overlays, VR360 and stereoscopic conversions, motion glitches and border /
//! stretch tools. Effect names follow the Kdenlive manual's effect list (feature names); the
//! parameters are those the manual describes, with our own ranges and defaults where it does not
//! pin them. Pixel implementations live in `redrob_film_render::vfx::kfx::{alpha, scopes, vr360,
//! motion, borders}`.
//!
//! Like the rest of the bin, every effect is a normal clip effect (Effects panel / `effects.apply`,
//! edited and keyframed in Effect Controls, saved with the clip).
//!
//! Mask chains: [`MASK_STARTS`] take a snapshot of the picture and a selection; the effects after
//! them change the picture only inside that selection, up to a **Mask Apply** (or the end of the
//! clip's effects), as the Kdenlive manual describes for its "(Mask)" effects.

use super::*;
use super::kfx::{MORE_TOP, YUV_PLANES};

const ALPHA: &[&str] = &[MORE_TOP, "Alpha, Mask and Keying"];
const UTILITY: &[&str] = &[MORE_TOP, "Utility"];
const VR: &[&str] = &[MORE_TOP, "VR360 and 3D"];
const MOTION: &[&str] = &[MORE_TOP, "Motion"];
const TRANSFORM: &[&str] = &[MORE_TOP, "Transform, Distort and Perspective"];

/// Effects that start a mask chain (see the module docs).
pub const MASK_STARTS: &[&str] = &["k_shape_alpha_mask", "k_color_select_mask"];
/// The effect that ends a mask chain.
pub const MASK_APPLY: &str = "k_mask_apply";

pub const ALPHA_GRADIENT_OPS: &[&str] = &["Write on Clear", "Add", "Min", "Max", "Subtract"];
pub const ALPHA_DISPLAYS: &[&str] =
    &["Image", "Alpha as Gray", "Gray + Red", "Selection on Black", "Selection on Gray", "Selection on White", "Selection on Checkers"];
pub const ALPHA_OPS: &[&str] = &["No Op", "Shave", "Shrink Hard", "Shrink Soft", "Grow Hard", "Grow Soft", "Threshold", "Blur"];
pub const SPILL_MASKS: &[&str] = &["Color Distance", "Transparency", "Edge Inwards", "Edge Outwards"];
pub const SPILL_OPS: &[&str] = &["None", "De-Key", "Target", "Desaturate", "Luma Adjust"];
pub const TRACKER_SHAPES: &[&str] = &["Rectangle", "Ellipse", "Arrow"];
pub const TRACKER_BLURS: &[&str] = &["None", "Median Blur", "Gaussian Blur", "Pixelate", "Opaque Fill"];
pub const INTERPOLATIONS: &[&str] = &["Nearest-Neighbor", "Bilinear"];
pub const STEREO_INPUTS: &[&str] = &[
    "Side by Side Parallel",
    "Side by Side Crosseye",
    "Above-Below (Left Above)",
    "Above-Below (Right Above)",
    "Interleaved Rows (Left Top)",
    "Interleaved Rows (Right Top)",
    "Interleaved Columns (Left First)",
    "Interleaved Columns (Right First)",
];
pub const STEREO_OUTPUTS: &[&str] = &[
    "Side by Side Parallel",
    "Side by Side Crosseye",
    "Above-Below (Left Above)",
    "Above-Below (Right Above)",
    "Interleaved Rows (Left Top)",
    "Interleaved Rows (Right Top)",
    "Anaglyph Red/Blue Gray",
    "Anaglyph Red/Green Gray",
    "Anaglyph Red/Cyan Gray",
    "Anaglyph Red/Cyan Half Colored",
    "Anaglyph Red/Cyan Color",
    "Anaglyph Red/Cyan Dubois",
    "Anaglyph Green/Magenta Gray",
    "Anaglyph Green/Magenta Half Colored",
    "Anaglyph Green/Magenta Colored",
    "Anaglyph Green/Magenta Dubois",
    "Anaglyph Yellow/Blue Gray",
    "Anaglyph Yellow/Blue Half Colored",
    "Anaglyph Yellow/Blue Colored",
    "Anaglyph Yellow/Blue Dubois",
    "Mono Output Left",
    "Mono Output Right",
    "Checkerboard (Left First)",
    "Checkerboard (Right First)",
    "Interleaved Columns (Left First)",
    "Interleaved Columns (Right First)",
    "HDMI Frame Pack",
];
pub const FILL_MODES: &[&str] = &["Smear", "Mirror", "Color", "Reflect", "Wrap", "Fade", "Margins"];
pub const CIE_SYSTEMS: &[&str] = &["NTSC", "EBU", "SMPTE", "Apple", "Wide RGB", "CIE 1931", "Rec. 709", "Rec. 2020", "DCI-P3"];
pub const CIE_GAMUTS: &[&str] = &["None", "NTSC", "EBU", "SMPTE", "Apple", "Wide RGB", "CIE 1931", "Rec. 709", "Rec. 2020", "DCI-P3"];
pub const CIE_SPACES: &[&str] = &["xyY", "UCS", "LUV"];
pub const DATA_SIZES: &[&str] = &["128p", "360p", "PAL SD", "NTSC SD", "480p", "720 HD", "1080 Full HD", "2K", "4K"];
pub const DATA_MODES: &[&str] = &["Mono", "Color", "Color2"];
pub const NUMBER_FORMATS: &[&str] = &["Hex", "Dec"];
pub const SCOPE_DISPLAYS: &[&str] = &["Overlay", "Stack", "Parade"];
pub const HISTOGRAM_SCALES: &[&str] = &["Linear", "Logarithmic"];
pub const SCOPE_COLORS: &[&str] = &["White on Black", "Black on White", "White on Gray", "Black on Gray", "Color on Black", "Color on White", "Color on Gray"];
pub const SCOPE_CHANNELS: &[&str] = &["R", "G", "B", "Y'", "Pr", "Pb", "Alpha"];
pub const YPRPB_SPACES: &[&str] = &["CCIR Rec. 601", "CCIR Rec. 709"];
pub const VECTOR_MODES: &[&str] = &["Gray", "Tint", "Color", "Color 2", "Color 3", "Color 4", "Color 5"];
pub const YUV_AXES: &[&str] = &["Y", "U", "V"];
pub const ENVELOPES: &[&str] = &["None", "Instant", "Peak", "Peak+Instant"];
pub const VECTOR_GRATICULES: &[&str] = &["None", "Green", "Color"];
pub const VECTOR_FLAGS: &[&str] = &["None", "White", "Black", "Name"];
pub const SCOPE_SPACES: &[&str] = &["Auto", "BT.601", "BT.709"];
pub const MEASUREMENTS: &[&str] = &["RGB", "Y'PbPr - Rec.601", "Y'PbPr - Rec.709", "HSV", "HSL"];
pub const WAVE_MODES: &[&str] = &["Row", "Column"];
pub const WAVE_COMPONENTS: &[&str] = &["Y", "U", "YU", "V", "YV", "UV", "All"];
pub const WAVE_FILTERS: &[&str] = &["Lowpass", "Flat", "Aflat", "Xflat", "Yflat", "Chroma", "Color", "Acolor"];
pub const WAVE_GRATICULES: &[&str] = &["None", "Green", "Orange", "Invert"];
pub const WAVE_FLAGS: &[&str] = &["Show Numbers", "Draw Dots"];
pub const WAVE_SCALES: &[&str] = &["Digital", "Millivolts", "IRE"];

fn kv(id: &'static str, name: &'static str, cat: &'static [&'static str], params: Vec<ParamDef>) -> EffectDef {
    EffectDef { id, name, kind: EffectKind::Video, category: cat, params, intrinsic: false, accelerated: false, float32: false, yuv: false }
}
fn int(id: &'static str, label: &'static str, def: f64, min: f64, max: f64) -> ParamDef {
    fs(id, label, def, (min, max), (min, max), "", 0)
}
fn pct(id: &'static str, label: &'static str, def: f64) -> ParamDef {
    f(id, label, def, 0.0, 100.0, "%")
}
fn unit(id: &'static str, label: &'static str, def: f64) -> ParamDef {
    fs(id, label, def, (0.0, 1.0), (0.0, 1.0), "", 3)
}
fn deg(id: &'static str, label: &'static str, def: f64, min: f64, max: f64) -> ParamDef {
    fs(id, label, def, (min, max), (min, max), "°", 1)
}
fn interp() -> ParamDef {
    ch("interpolation", "Interpolation", INTERPOLATIONS, 1)
}
/// A rectangle in percent of the frame (`x`, `y` = top-left corner).
fn rect(v: &mut Vec<ParamDef>, x: f64, y: f64, w: f64, h: f64) {
    v.push(grp(pct("rect_x", "X", x), "Geometry"));
    v.push(grp(pct("rect_y", "Y", y), "Geometry"));
    v.push(grp(pct("rect_w", "W", w), "Geometry"));
    v.push(grp(pct("rect_h", "H", h), "Geometry"));
}
/// The file-matte controls of Shape Alpha and Shape Alpha (Mask).
fn shape_alpha_params() -> Vec<ParamDef> {
    vec![
        txt("resource", "Image or Video Resource"),
        fs("softness", "Softness", 0.1, (0.0, 1.0), (0.0, 1.0), "", 2),
        b("invert", "Invert", false),
        b("use_luma", "Use Luma", false),
        b("use_threshold", "Use Threshold", false),
        fs("offset", "Offset", 0.0, (0.0, 86_400.0), (0.0, 600.0), "s", 2),
        fs("out", "Out", 0.0, (0.0, 86_400.0), (0.0, 600.0), "s", 2),
        pct("threshold", "Threshold", 50.0),
    ]
}
fn cap_params(v: &mut Vec<ParamDef>, g: &'static str, p: &'static [&'static str; 10], start: f64) {
    v.push(grp(b(p[0], "Enabled", true), g));
    v.push(grp(deg(p[1], "Start", start, 0.0, 90.0), g));
    v.push(grp(deg(p[2], "End", 90.0, 0.0, 90.0), g));
    v.push(grp(deg(p[3], "Blend In", 0.0, 0.0, 90.0), g));
    v.push(grp(deg(p[4], "Blend Out", 5.0, 0.0, 90.0), g));
    v.push(grp(deg(p[5], "Fade In", 5.0, 0.0, 90.0), g));
    v.push(grp(deg(p[6], "Blur Width at Start", 0.0, 0.0, 360.0), g));
    v.push(grp(deg(p[7], "Blur Width at End", 360.0, 0.0, 360.0), g));
    v.push(grp(deg(p[8], "Blur Height at Start", 0.0, 0.0, 90.0), g));
    v.push(grp(deg(p[9], "Blur Height at End", 10.0, 0.0, 90.0), g));
}
const CAP_TOP: [&str; 10] = ["top", "top_start", "top_end", "top_blend_in", "top_blend_out", "top_fade_in", "top_bw_start", "top_bw_end", "top_bh_start", "top_bh_end"];
const CAP_BOTTOM: [&str; 10] = [
    "bottom",
    "bottom_start",
    "bottom_end",
    "bottom_blend_in",
    "bottom_blend_out",
    "bottom_fade_in",
    "bottom_bw_start",
    "bottom_bw_end",
    "bottom_bh_start",
    "bottom_bh_end",
];

pub(super) fn defs() -> Vec<EffectDef> {
    let mut v = Vec::new();
    // ---------------------------------------------------------------- Alpha, Mask and Keying
    v.push(kv(
        "k_alpha_gradient",
        "Alpha Gradient",
        ALPHA,
        vec![
            ch("operation", "Operation", ALPHA_GRADIENT_OPS, 0),
            pct("position", "Position", 50.0),
            pct("width", "Transition Width", 50.0),
            deg("tilt", "Tilt", 0.0, 0.0, 360.0),
            pct("transparency_top", "Transparency Top", 100.0),
            pct("transparency_bottom", "Transparency Bottom", 0.0),
        ],
    ));
    v.push(kv(
        "k_alpha_ops",
        "Alpha Operations",
        ALPHA,
        vec![
            ch("display", "Display", ALPHA_DISPLAYS, 0),
            b("display_input", "Display Input Alpha", false),
            ch("operation", "Operation", ALPHA_OPS, 0),
            b("invert", "Invert", false),
            pct("threshold", "Threshold", 50.0),
            int("amount", "Shrink/Grow/Blur", 1.0, 0.0, 100.0),
        ],
    ));
    v.push(kv("k_alpha_strobe", "Alpha Strobing", ALPHA, vec![b("invert", "Invert", false), int("interval", "Interval", 1.0, 1.0, 1000.0)]));
    v.push(kv(
        "k_backgroundkey",
        "Backgroundkey",
        ALPHA,
        vec![unit("threshold", "Threshold", 0.08), unit("similarity", "Similarity", 0.1), unit("blend", "Blend", 0.0)],
    ));
    v.push(kv(
        "k_hsv_key",
        "HSV Keying",
        ALPHA,
        vec![
            deg("hue", "Hue", 0.0, -360.0, 360.0),
            fs("saturation", "Saturation", 0.0, (-1.0, 1.0), (-1.0, 1.0), "", 3),
            fs("value", "Value", 0.0, (-1.0, 1.0), (-1.0, 1.0), "", 3),
            fs("similarity", "Similarity", 1.0, (0.0, 100.0), (0.0, 100.0), "%", 2),
            pct("blend", "Blend", 0.0),
        ],
    ));
    v.push(kv(
        "k_spill_mopup",
        "Key Spill Mop Up",
        ALPHA,
        vec![
            ch("mask_type", "Mask Type", SPILL_MASKS, 0),
            ch("op1", "Operation 1", SPILL_OPS, 1),
            ch("op2", "Operation 2", SPILL_OPS, 0),
            b("show_mask", "Show Mask", false),
            b("mask_to_alpha", "Mask to Alpha", false),
            col("key", "Key Color", [0.1, 0.8, 0.1, 1.0]),
            col("target", "Target Color", [0.78, 0.5, 0.4, 1.0]),
            int("tolerance", "Tolerance", 24.0, 0.0, 100.0),
            int("slope", "Slope", 40.0, 0.0, 100.0),
            int("hue_gate", "Hue Gate", 25.0, 0.0, 100.0),
            int("sat_threshold", "Saturation Threshold", 15.0, 0.0, 100.0),
            int("amount1", "Amount 1", 55.0, 0.0, 100.0),
            int("amount2", "Amount 2", 0.0, 0.0, 100.0),
        ],
    ));
    let mut tracker = vec![
        ch("shape", "Frame Shape", TRACKER_SHAPES, 0),
        int("shape_width", "Shape Width", 1.0, 0.0, 100.0),
        col("shape_color", "Shape Color", [1.0, 0.0, 0.0, 1.0]),
        int("blur", "Blur", 5.0, 0.0, 100.0),
        ch("blur_type", "Blur Type", TRACKER_BLURS, 0),
    ];
    rect(&mut tracker, 40.0, 40.0, 20.0, 20.0);
    v.push(kv("k_motion_tracker", "Motion Tracker", ALPHA, tracker));
    v.push(kv("k_premultiply", "Premultiply or Unpremultiply", ALPHA, vec![b("unpremultiply", "Unpremultiply", true)]));
    v.push(kv(
        "k_color_select_mask",
        "Secondary Color Correction Area Selection (Mask)",
        ALPHA,
        vec![
            col("color", "Color", [0.1, 0.8, 0.1, 1.0]),
            pct("tolerance", "Tolerance", 20.0),
            pct("softness", "Edge Softness", 10.0),
            b("invert", "Invert Selection", false),
        ],
    ));
    v.push(kv("k_shape_alpha", "Shape Alpha", ALPHA, shape_alpha_params()));
    v.push(kv("k_shape_alpha_mask", "Shape Alpha (Mask)", ALPHA, shape_alpha_params()));
    v.push(kv(MASK_APPLY, "Mask Apply", ALPHA, vec![]));
    let mut spot = Vec::new();
    rect(&mut spot, 45.0, 45.0, 10.0, 10.0);
    v.push(kv("k_spot_remover", "Spot Remover", ALPHA, spot));
    // ---------------------------------------------------------------- Motion
    v.push(kv(
        "k_gate_weave",
        "Gate Weave",
        MOTION,
        vec![
            int("interval", "Interval", 4.0, 1.0, 100.0),
            fs("max_x", "Maximum Horizontal Movement", 1.0, (0.0, 10.0), (0.0, 10.0), "", 2),
            fs("max_y", "Maximum Vertical Movement", 1.0, (0.0, 10.0), (0.0, 10.0), "", 2),
            fs("seed", "Random Seed", 0.0, (0.0, 10000.0), (0.0, 1000.0), "", 0),
        ],
    ));
    v.push(kv(
        "k_glitch0r",
        "Glitching (Rows and Color)",
        MOTION,
        vec![
            pct("frequency", "Glitch Frequency", 50.0),
            pct("block_height", "Block Height", 50.0),
            pct("shift", "Shift Intensity", 50.0),
            pct("color", "Color Glitching Intensity", 50.0),
        ],
    ));
    v.push(kv(
        "k_pixs0r",
        "Glitching (Rows Only)",
        MOTION,
        vec![
            unit("shift", "Shift Intensity", 0.5),
            fs("block_height", "Block Height", 0.0, (0.0, 200.0), (0.0, 100.0), "px", 0),
            fs("min_height", "Minimum Block Height", 1.0, (1.0, 200.0), (1.0, 100.0), "px", 0),
            fs("max_height", "Maximum Block Height", 40.0, (1.0, 400.0), (1.0, 200.0), "px", 0),
        ],
    ));
    v.push(kv("k_lagfun", "Lag Fun", MOTION, vec![fs("decay", "Decay", 0.95, (0.0, 1.0), (0.0, 1.0), "", 3)]));
    v.push(kv("k_nervous", "Nervous", MOTION, vec![]));
    v.push(kv("k_vertigo", "Vertigo", MOTION, vec![int("phase", "Phase Increment", 2.0, 0.0, 100.0), int("zoom", "Zoom Rate", 101.0, 98.0, 105.0)]));
    // ---------------------------------------------------------------- Transform, Distort and Perspective
    v.push(kv(
        "k_elastic_scale",
        "Elastic Scale",
        TRANSFORM,
        vec![
            pct("center", "Center", 50.0),
            pct("linear_width", "Linear Width", 0.0),
            fs("linear_scale", "Linear Scale Factor", 1.0, (0.1, 10.0), (0.5, 2.0), "", 2),
            unit("nonlinear_scale", "Non-Linear Scale Factor", 0.0),
        ],
    ));
    v.push(kv(
        "k_fill_borders",
        "Fill Borders",
        TRANSFORM,
        vec![
            ch("mode", "Mode", FILL_MODES, 0),
            col("color", "Color", [0.0, 0.0, 0.0, 1.0]),
            fs("left", "Left", 0.0, (0.0, 4000.0), (0.0, 200.0), "px", 0),
            fs("right", "Right", 0.0, (0.0, 4000.0), (0.0, 200.0), "px", 0),
            fs("top", "Top", 0.0, (0.0, 4000.0), (0.0, 200.0), "px", 0),
            fs("bottom", "Bottom", 0.0, (0.0, 4000.0), (0.0, 200.0), "px", 0),
        ],
    ));
    v.push(kv("k_letterbox", "Letter-boxing", TRANSFORM, vec![b("transparent", "Transparency", false), pct("border", "Border Width", 40.0)]));
    v.push(kv("k_nosync0r", "Nosync0r (Broken TV)", TRANSFORM, vec![int("hsync", "HSync", 0.0, 0.0, 1000.0)]));
    let mut pillar = vec![pct("blur", "Blur", 10.0)];
    rect(&mut pillar, 34.375, 0.0, 31.25, 100.0);
    v.push(kv("k_pillar_echo", "Pillar Echo", TRANSFORM, pillar));
    // ---------------------------------------------------------------- Utility (burnt-in scopes)
    v.push(kv(
        "k_cie_scope",
        "CIE Scope",
        UTILITY,
        vec![
            ch("system", "Color System", CIE_SYSTEMS, 6),
            ch("cie", "CIE System", CIE_SPACES, 0),
            ch("gamuts", "Gamuts to Draw", CIE_GAMUTS, 0),
            int("size", "Size", 512.0, 64.0, 2048.0),
            unit("intensity", "Intensity", 0.1),
            unit("contrast", "Contrast", 0.75),
            b("correct_gamma", "Display Correct Gamma", true),
            b("white_point", "Show White Point", false),
            fs("gamma", "Input Gamma", 2.6, (0.1, 6.0), (0.1, 6.0), "", 2),
            b("fill", "Fill with CIE Colors", true),
        ],
    ));
    v.push(kv(
        "k_datascope",
        "Data Scope",
        UTILITY,
        vec![
            ch("size", "Size", DATA_SIZES, 5),
            fs("x", "X Offset", 0.0, (0.0, 8192.0), (0.0, 1920.0), "px", 0),
            fs("y", "Y Offset", 0.0, (0.0, 8192.0), (0.0, 1080.0), "px", 0),
            ch("components", "Components", YUV_PLANES, 7),
            ch("mode", "Mode", DATA_MODES, 0),
            b("axis", "Show Axis", false),
            unit("opacity", "Opacity", 0.75),
            ch("format", "Format", NUMBER_FORMATS, 0),
        ],
    ));
    v.push(kv(
        "k_histogram",
        "Histogram",
        UTILITY,
        vec![
            int("level_height", "Level Height", 200.0, 50.0, 2048.0),
            int("scale_height", "Scale Height", 12.0, 0.0, 40.0),
            ch("display", "Display", SCOPE_DISPLAYS, 1),
            ch("mode", "Mode", HISTOGRAM_SCALES, 0),
            ch("components", "Components to Display", YUV_PLANES, 7),
            unit("fg_opacity", "Foreground Opacity", 0.7),
            unit("bg_opacity", "Background Opacity", 0.5),
            ch("colors", "Colors Mode", SCOPE_COLORS, 0),
        ],
    ));
    v.push(kv("k_d90_fix", "Nikon D90 Stairstepping Fix", UTILITY, vec![]));
    v.push(kv(
        "k_oscilloscope",
        "Oscilloscope (Simple)",
        UTILITY,
        vec![
            ch("components", "Components", YUV_PLANES, 7),
            b("grid", "Draw Scope Grid", true),
            b("statistics", "Print Statistics", true),
            b("trace_line", "Draw Trace Profile Line", true),
            pct("trace_x", "Trace X Position", 50.0),
            pct("trace_y", "Trace Y Position", 50.0),
            pct("size", "Size", 80.0),
            deg("tilt", "Tilt", 0.0, 0.0, 360.0),
            pct("opacity", "Opacity", 80.0),
            pct("scope_x", "Scope X Position", 50.0),
            pct("scope_y", "Scope Y Position", 80.0),
            pct("scope_w", "Scope Width", 80.0),
            pct("scope_h", "Scope Height", 30.0),
        ],
    ));
    v.push(kv(
        "k_oscilloscope_adv",
        "Oscilloscope (Advanced)",
        UTILITY,
        vec![
            ch("channel", "Channel", SCOPE_CHANNELS, 3),
            b("rgb", "R / G / B Trace", true),
            b("ypp", "Y / Pr / Pb Trace", false),
            b("alpha", "Alpha Trace", false),
            b("average", "Display Average", true),
            b("rms", "Display RMS", false),
            b("minmax", "Display Minimum / Maximum", true),
            b("scale256", "256 Scale", false),
            ch("space", "Color", YPRPB_SPACES, 1),
            pct("x", "X", 50.0),
            pct("y", "Y", 50.0),
            deg("tilt", "Tilt", 0.0, 0.0, 360.0),
            pct("length", "Length", 80.0),
            pct("marker1", "Marker 1", 25.0),
            pct("marker2", "Marker 2", 75.0),
            col("crosshair", "Crosshair Color", [0.0, 1.0, 0.0, 1.0]),
        ],
    ));
    v.push(kv("k_rgb_parade", "RGB Parade", UTILITY, vec![]));
    v.push(kv(
        "k_timeout",
        "Timeout Indicator",
        UTILITY,
        vec![col("color", "Indicator Color", [0.3, 0.55, 0.95, 1.0]), int("time", "Time", 0.0, 0.0, 1000.0), pct("transparency", "Transparency", 0.0)],
    ));
    v.push(kv("k_vectorscope", "Vectorscope", UTILITY, vec![]));
    v.push(kv(
        "k_vectorscope_adv",
        "Vectorscope (Advanced)",
        UTILITY,
        vec![
            ch("mode", "Mode", VECTOR_MODES, 4),
            fs("tint1", "Tint 1", 0.0, (-1.0, 1.0), (-1.0, 1.0), "", 3),
            fs("tint2", "Tint 2", 0.0, (-1.0, 1.0), (-1.0, 1.0), "", 3),
            ch("x_axis", "X", YUV_AXES, 1),
            ch("y_axis", "Y", YUV_AXES, 2),
            fs("intensity", "Intensity", 0.004, (0.0, 1.0), (0.0, 0.1), "", 4),
            ch("envelope", "Envelope", ENVELOPES, 0),
            ch("graticule", "Graticule", VECTOR_GRATICULES, 1),
            unit("graticule_opacity", "Graticule Opacity", 0.75),
            ch("flags", "Graticule Flags", VECTOR_FLAGS, 2),
            unit("bg_opacity", "Background Opacity", 0.3),
            unit("low", "Low Threshold", 0.0),
            unit("high", "High Threshold", 1.0),
            ch("space", "Color Space", SCOPE_SPACES, 0),
        ],
    ));
    v.push(kv(
        "k_video_values",
        "Video Values",
        UTILITY,
        vec![
            ch("measurement", "Measurement", MEASUREMENTS, 0),
            b("scale256", "256-Scale", false),
            b("show_alpha", "Show Alpha", false),
            b("big", "Big Window", false),
            pct("x", "X Position", 50.0),
            pct("y", "Y Position", 50.0),
            int("size_x", "X Size", 1.0, 1.0, 64.0),
            int("size_y", "Y Size", 1.0, 1.0, 64.0),
        ],
    ));
    v.push(kv(
        "k_waveform",
        "Video Waveform Monitor",
        UTILITY,
        vec![
            ch("mode", "Mode", WAVE_MODES, 1),
            fs("intensity", "Intensity", 0.04, (0.0, 1.0), (0.0, 0.3), "", 3),
            b("mirror", "Mirror", false),
            ch("display", "Display", SCOPE_DISPLAYS, 1),
            ch("components", "Components", WAVE_COMPONENTS, 0),
            ch("envelope", "Envelope", ENVELOPES, 0),
            ch("filter", "Filter", WAVE_FILTERS, 0),
            ch("graticule", "Graticule", WAVE_GRATICULES, 1),
            unit("graticule_opacity", "Graticule Opacity", 0.75),
            ch("flags", "Flags", WAVE_FLAGS, 0),
            ch("scale", "Scale", WAVE_SCALES, 0),
            unit("bg_opacity", "Background Opacity", 0.75),
        ],
    ));
    // ---------------------------------------------------------------- VR360 and 3D
    v.push(kv(
        "k_stereo3d",
        "Stereoscopic 3D",
        VR,
        vec![ch("input", "Input Format", STEREO_INPUTS, 0), ch("output", "Output Format", STEREO_OUTPUTS, 11)],
    ));
    let mut cap = vec![interp()];
    cap_params(&mut cap, "Top", &CAP_TOP, 75.0);
    cap_params(&mut cap, "Bottom", &CAP_BOTTOM, 75.0);
    v.push(kv("k_vr_cap", "VR360 Cap Top and Bottom", VR, cap));
    v.push(kv(
        "k_vr_equi2rect",
        "VR360 Equirectangular to Rectilinear",
        VR,
        vec![
            interp(),
            deg("yaw", "Yaw", 0.0, -360.0, 360.0),
            deg("pitch", "Pitch", 0.0, -180.0, 180.0),
            deg("roll", "Roll", 0.0, -180.0, 180.0),
            deg("fov", "FOV", 90.0, 1.0, 360.0),
            pct("fisheye", "Fisheye", 0.0),
        ],
    ));
    v.push(kv(
        "k_vr_equi2stereo",
        "VR360 Equirectangular to Stereo",
        VR,
        vec![
            interp(),
            deg("yaw", "Yaw", 0.0, -360.0, 360.0),
            deg("pitch", "Pitch", -90.0, -180.0, 180.0),
            deg("roll", "Roll", 0.0, -180.0, 180.0),
            deg("fov", "FOV", 270.0, 1.0, 359.0),
            pct("amount", "Amount", 100.0),
        ],
    ));
    v.push(kv(
        "k_vr_equi_mask",
        "VR360 Equirectangular Mask",
        VR,
        vec![
            interp(),
            deg("h_start", "Horizontal Start", 90.0, 0.0, 180.0),
            deg("h_end", "Horizontal End", 100.0, 0.0, 180.0),
            deg("v_start", "Vertical Start", 90.0, 0.0, 90.0),
            deg("v_end", "Vertical End", 90.0, 0.0, 90.0),
        ],
    ));
    v.push(kv(
        "k_vr_hemi2equi",
        "VR360 Hemispherical to Equirectangular",
        VR,
        vec![
            ch("projection", "Projection", &["Equidistant Fisheye"], 0),
            interp(),
            deg("yaw", "Alignment Yaw", 0.0, -180.0, 180.0),
            deg("pitch", "Alignment Pitch", 0.0, -180.0, 180.0),
            deg("roll", "Alignment Roll", 0.0, -180.0, 180.0),
            deg("lens_fov", "Lens FOV", 190.0, 90.0, 360.0),
            fs("lens_radius", "Lens Radius", 0.25, (0.0, 1.0), (0.0, 0.5), "", 4),
            grp(fs("front_x", "Front X", 0.25, (0.0, 1.0), (0.0, 1.0), "", 4), "Front"),
            grp(fs("front_y", "Front Y", 0.5, (0.0, 1.0), (0.0, 1.0), "", 4), "Front"),
            grp(deg("front_up", "Front Up", 90.0, -360.0, 360.0), "Front"),
            grp(fs("back_x", "Back X", 0.75, (0.0, 1.0), (0.0, 1.0), "", 4), "Back"),
            grp(fs("back_y", "Back Y", 0.5, (0.0, 1.0), (0.0, 1.0), "", 4), "Back"),
            grp(deg("back_up", "Back Up", 270.0, -360.0, 360.0), "Back"),
            fs("nadir_radius", "Nadir Radius", 0.0, (0.0, 1.0), (0.0, 0.5), "", 4),
            fs("nadir_start", "Nadir Start", 1.0, (0.0, 1.0), (0.0, 1.0), "", 4),
            grp(fs("dist_a", "Lens Distortion A", 0.0, (-1.0, 1.0), (-0.5, 0.5), "", 4), "Lens Distortion"),
            grp(fs("dist_b", "Lens Distortion B", 0.0, (-1.0, 1.0), (-0.5, 0.5), "", 4), "Lens Distortion"),
            grp(fs("dist_c", "Lens Distortion C", 0.0, (-1.0, 1.0), (-0.5, 0.5), "", 4), "Lens Distortion"),
            grp(fs("dist_radius", "Lens Distortion Radius", 0.25, (0.01, 1.0), (0.01, 1.0), "", 4), "Lens Distortion"),
            grp(fs("vig_a", "Lens Vignetting A", 1.0, (0.01, 4.0), (0.5, 2.0), "", 4), "Lens Vignetting"),
            grp(fs("vig_b", "Lens Vignetting B", 0.0, (-4.0, 4.0), (-1.0, 1.0), "", 4), "Lens Vignetting"),
            grp(fs("vig_c", "Lens Vignetting C", 0.0, (-4.0, 4.0), (-1.0, 1.0), "", 4), "Lens Vignetting"),
            grp(fs("vig_d", "Lens Vignetting D", 0.0, (-4.0, 4.0), (-1.0, 1.0), "", 4), "Lens Vignetting"),
            grp(fs("vig_radius", "Lens Vignetting Radius", 0.559, (0.01, 2.0), (0.01, 1.0), "", 4), "Lens Vignetting"),
        ],
    ));
    v.push(kv(
        "k_vr_stabilize",
        "VR360 Stabilize",
        VR,
        vec![
            int("sample_radius", "Analysis Sample Radius", 16.0, 4.0, 64.0),
            int("search_radius", "Analysis Search Radius", 24.0, 2.0, 128.0),
            deg("track_offset", "Analysis Offset", 60.0, 10.0, 90.0),
            b("back_points", "Use Backwards-Facing Track Point", true),
            pct("yaw_amount", "Yaw Amount", 100.0),
            pct("pitch_amount", "Pitch Amount", 100.0),
            pct("roll_amount", "Roll Amount", 100.0),
            int("yaw_smooth", "Yaw Smoothing", 120.0, 1.0, 1000.0),
            int("pitch_smooth", "Pitch Smoothing", 120.0, 1.0, 1000.0),
            int("roll_smooth", "Roll Smoothing", 120.0, 1.0, 1000.0),
            f("yaw_bias", "Yaw Time Bias", 0.0, -100.0, 100.0, "%"),
            f("pitch_bias", "Pitch Time Bias", 0.0, -100.0, 100.0, "%"),
            f("roll_bias", "Roll Time Bias", 0.0, -100.0, 100.0, "%"),
            interp(),
        ],
    ));
    v.push(kv(
        "k_vr_wrap",
        "VR360 Wrap",
        VR,
        vec![
            interp(),
            deg("top", "Top", 45.0, 1.0, 90.0),
            deg("bottom", "Bottom", 45.0, 1.0, 90.0),
            deg("left", "Left", 90.0, 1.0, 180.0),
            deg("right", "Right", 90.0, 1.0, 180.0),
            pct("blur_start", "Blur Start", 0.0),
            pct("blur_end", "Blur End", 10.0),
        ],
    ));
    v
}
