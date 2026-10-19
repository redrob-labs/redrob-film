//! More Video Effects ▸ Color and Image Correction / Image Adjustments / Blur and Sharpen: the
//! colour, signal and blur clip effects of the Kdenlive manual's catalogue. Effect names are
//! functional facts; parameters, ranges and defaults follow the public manual pages and the
//! behaviour is our own. Pixel implementations live in `redrob_film_render::vfx::kfx::kcolor`.
//!
//! Like the rest of the bin these are normal clip effects: applied from the Effects panel or
//! `effects.apply`, edited and keyframed in Effect Controls (every colour parameter has the
//! Program-monitor eyedropper), and saved in the project with the clip.

use super::kfx::ANY_PLANES;
use super::*;

const COLOR: &[&str] = &[MORE_TOP, "Color and Image Correction"];
const ADJUST: &[&str] = &[MORE_TOP, "Image Adjustments"];
const BLUR: &[&str] = &[MORE_TOP, "Blur and Sharpen"];

/// Colour matrices of Color Matrix (source and destination).
pub const COLOR_MATRICES: &[&str] = &["BT.709", "FCC", "BT.601", "BT.470", "BT.470BG", "SMPTE 170M", "SMPTE 240M", "BT.2020"];
/// Color Space ▸ Color properties / input override (all properties at once).
pub const CS_ALL: &[&str] = &["Auto", "BT.709", "BT.470M", "BT.470BG", "BT.601-6 525", "BT.601-6 625", "SMPTE 170M", "SMPTE 240M", "BT.2020"];
/// Color Space ▸ colour space (YUV matrix).
pub const CS_SPACE: &[&str] = &["Auto", "BT.709", "FCC", "BT.470BG", "SMPTE 170M", "SMPTE 240M", "YCgCo", "BT.2020 NCL"];
/// Color Space ▸ transfer characteristics.
pub const CS_TRC: &[&str] =
    &["Auto", "BT.709", "BT.470M", "BT.470BG", "Gamma 2.2", "Gamma 2.8", "SMPTE 170M", "SMPTE 240M", "sRGB", "xvYCC", "BT.2020 10-bit", "BT.2020 12-bit"];
/// Color Space ▸ primaries.
pub const CS_PRIMARIES: &[&str] =
    &["Auto", "BT.709", "BT.470M", "BT.470BG", "SMPTE 170M", "SMPTE 240M", "Film", "BT.2020", "SMPTE 431", "SMPTE 432", "JEDEC P22"];
/// Color Space ▸ range.
pub const CS_RANGE: &[&str] = &["Auto", "TV (Limited)", "PC (Full)"];
/// Color Space ▸ output format (chroma subsampling and bit depth).
pub const CS_FORMAT: &[&str] = &[
    "Auto",
    "YUV 4:2:0 8-bit",
    "YUV 4:2:0 10-bit",
    "YUV 4:2:0 12-bit",
    "YUV 4:2:2 8-bit",
    "YUV 4:2:2 10-bit",
    "YUV 4:2:2 12-bit",
    "YUV 4:4:4 8-bit",
    "YUV 4:4:4 10-bit",
    "YUV 4:4:4 12-bit",
];
/// Interleave - Deinterleave modes.
pub const IL_MODES: &[&str] = &["None", "Deinterleave", "Interleave"];
/// Phase modes.
pub const PHASE_MODES: &[&str] = &[
    "Delay the Top Field",
    "Delay the Bottom Field",
    "Keep the Field Order",
    "Auto (Field Flags), Transfer Opposite",
    "Unknown Capture, Transfer Opposite",
    "Top-first Capture, Unknown Transfer",
    "Bottom-first Capture, Unknown Transfer",
    "Auto (Field Flags), Unknown Transfer",
    "Both Unknown",
];

fn kc(id: &'static str, name: &'static str, cat: &'static [&'static str], params: Vec<ParamDef>) -> EffectDef {
    EffectDef { id, name, kind: EffectKind::Video, category: cat, params, intrinsic: false, accelerated: false, float32: false, yuv: false }
}
fn int(id: &'static str, label: &'static str, def: f64, min: f64, max: f64) -> ParamDef {
    fs(id, label, def, (min, max), (min, max), "", 0)
}
fn num(id: &'static str, label: &'static str, def: f64, min: f64, max: f64, decimals: u8) -> ParamDef {
    fs(id, label, def, (min, max), (min, max), "", decimals)
}
fn fixed(mut p: ParamDef) -> ParamDef {
    p.animatable = false;
    p
}
fn planes() -> ParamDef {
    ch("planes", "Planes", ANY_PLANES, 7)
}
/// One CMYK Adjust colour range: Cyan / Magenta / Yellow / Black adjustments in −100 … 100 %.
fn cmyk_range(v: &mut Vec<ParamDef>, group: &'static str, ids: [&'static str; 4]) {
    for (id, label) in ids.into_iter().zip(["Cyan", "Magenta", "Yellow", "Black"]) {
        v.push(grp(f(id, label, 0.0, -100.0, 100.0, "%"), group));
    }
}

pub(super) fn defs() -> Vec<EffectDef> {
    let mut cmyk = vec![ch("method", "Correction Method", &["Absolute", "Relative"], 0)];
    cmyk_range(&mut cmyk, "Reds", ["reds_c", "reds_m", "reds_y", "reds_k"]);
    cmyk_range(&mut cmyk, "Yellows", ["yellows_c", "yellows_m", "yellows_y", "yellows_k"]);
    cmyk_range(&mut cmyk, "Greens", ["greens_c", "greens_m", "greens_y", "greens_k"]);
    cmyk_range(&mut cmyk, "Cyans", ["cyans_c", "cyans_m", "cyans_y", "cyans_k"]);
    cmyk_range(&mut cmyk, "Blues", ["blues_c", "blues_m", "blues_y", "blues_k"]);
    cmyk_range(&mut cmyk, "Magentas", ["magentas_c", "magentas_m", "magentas_y", "magentas_k"]);
    cmyk_range(&mut cmyk, "Whites", ["whites_c", "whites_m", "whites_y", "whites_k"]);
    cmyk_range(&mut cmyk, "Neutrals", ["neutrals_c", "neutrals_m", "neutrals_y", "neutrals_k"]);
    cmyk_range(&mut cmyk, "Blacks", ["blacks_c", "blacks_m", "blacks_y", "blacks_k"]);

    // Gradient Map: up to 8 stops (position % and colour); the first `stops` are used.
    let mut gmap = vec![fixed(int("stops", "Number of Stops", 2.0, 2.0, 8.0))];
    let gm_ids: [(&str, &str, &str); 8] = [
        ("stop1_pos", "stop1_color", "Stop 1"),
        ("stop2_pos", "stop2_color", "Stop 2"),
        ("stop3_pos", "stop3_color", "Stop 3"),
        ("stop4_pos", "stop4_color", "Stop 4"),
        ("stop5_pos", "stop5_color", "Stop 5"),
        ("stop6_pos", "stop6_color", "Stop 6"),
        ("stop7_pos", "stop7_color", "Stop 7"),
        ("stop8_pos", "stop8_color", "Stop 8"),
    ];
    for (k, (pos, colr, g)) in gm_ids.into_iter().enumerate() {
        let x = if k == 0 {
            0.0
        } else if k == 1 {
            100.0
        } else {
            (k as f64 - 1.0) * 100.0 / 7.0
        };
        let c = if k == 0 { 0.0 } else { 1.0 };
        gmap.push(grp(f(pos, "Position", x, 0.0, 100.0, "%"), g));
        gmap.push(grp(col(colr, "Color", [c, c, c, 1.0]), g));
    }

    vec![
        // ---------------------------------------------------------------- Blur and Sharpen
        kc("k_avgblur", "Average Blur", BLUR, vec![int("size_x", "X Size", 1.0, 0.0, 1024.0), int("size_y", "Y Size", 1.0, 0.0, 1024.0), planes()]),
        kc(
            "k_bilateral",
            "Bilateral",
            BLUR,
            vec![planes(), num("sigma_s", "Spatial Sigma", 0.1, 0.0, 512.0, 3), num("sigma_r", "Range Sigma", 0.1, 0.0, 1.0, 3)],
        ),
        kc(
            "k_boxblur",
            "BoxBlur",
            BLUR,
            vec![
                b("preserve_alpha", "Preserve Alpha", false),
                fs("hori", "Horizontal Multiplicator", 1.0, (0.0, 1000.0), (0.0, 100.0), "%", 0),
                fs("vert", "Vertical Multiplicator", 1.0, (0.0, 1000.0), (0.0, 100.0), "%", 0),
            ],
        ),
        kc("k_cas", "Contrast Adaptive Sharpen (CAS)", BLUR, vec![planes(), num("strength", "Strength", 0.0, 0.0, 1.0, 3)]),
        kc(
            "k_planes_blur",
            "Planes Blur",
            BLUR,
            vec![
                fixed(int("luma_radius", "Luma Radius", 2.0, 0.0, 100.0)),
                fixed(int("chroma_radius", "Chroma Radius", 2.0, 0.0, 100.0)),
                fixed(int("alpha_radius", "Alpha Radius", 2.0, 0.0, 100.0)),
                fixed(int("luma_power", "Luma Power", 2.0, 0.0, 10.0)),
                fixed(int("chroma_power", "Chroma Power", 2.0, 0.0, 10.0)),
                fixed(int("alpha_power", "Alpha Power", 2.0, 0.0, 10.0)),
            ],
        ),
        kc(
            "k_sab",
            "Shape Adaptive Blur",
            BLUR,
            vec![
                fixed(num("luma_radius", "Luma Radius", 1.0, 0.1, 4.0, 1)),
                fixed(num("luma_pre", "Luma Pre-filter Radius", 1.0, 0.1, 2.0, 1)),
                fixed(num("luma_strength", "Luma Strength", 1.0, 0.1, 100.0, 1)),
                fixed(num("chroma_radius", "Chroma Radius", 0.0, 0.0, 4.0, 1)),
                fixed(num("chroma_pre", "Chroma Pre-filter Radius", 1.0, 0.1, 2.0, 1)),
                fixed(num("chroma_strength", "Chroma Strength", 0.0, 0.0, 100.0, 1)),
            ],
        ),
        kc(
            "k_smartblur",
            "Smartblur",
            BLUR,
            vec![
                fixed(num("luma_radius", "Luma Radius", 1.0, 0.1, 5.0, 1)),
                fixed(num("luma_strength", "Luma Strength", 1.0, -1.0, 1.0, 2)),
                fixed(int("luma_threshold", "Luma Threshold", 0.0, -30.0, 30.0)),
                fixed(num("chroma_radius", "Chroma Radius", 1.0, 0.1, 5.0, 1)),
                fixed(num("chroma_strength", "Chroma Strength", 1.0, -1.0, 1.0, 2)),
                fixed(int("chroma_threshold", "Chroma Threshold", 0.0, -30.0, 30.0)),
            ],
        ),
        kc("k_squareblur", "Square Blur", BLUR, vec![int("kernel", "Kernel Size", 50.0, 0.0, 500.0)]),
        // ---------------------------------------------------------------- Color and Image Correction
        kc(
            "k_three_point",
            "3-Point Balance",
            COLOR,
            vec![
                col("black", "Black Color", [0.0, 0.0, 0.0, 1.0]),
                col("gray", "Gray Color", [0.5, 0.5, 0.5, 1.0]),
                col("white", "White Color", [1.0, 1.0, 1.0, 1.0]),
                b("split", "Split Preview", false),
                b("split_left", "Source Image on Left Side", false),
            ],
        ),
        kc("k_cmyk_adjust", "CMYK Adjust", COLOR, cmyk),
        kc(
            "k_color_contrast",
            "Color Contrast",
            COLOR,
            vec![
                num("rc", "Red-Cyan Contrast", 0.0, -1.0, 1.0, 2),
                num("gm", "Green-Magenta Contrast", 0.0, -1.0, 1.0, 2),
                num("by", "Blue-Yellow Contrast", 0.0, -1.0, 1.0, 2),
                num("rcw", "Red-Cyan Weight", 0.0, 0.0, 1.0, 2),
                num("gmw", "Green-Magenta Weight", 0.0, 0.0, 1.0, 2),
                num("byw", "Blue-Yellow Weight", 0.0, 0.0, 1.0, 2),
                num("pl", "Preserve Lightness", 0.0, 0.0, 1.0, 2),
            ],
        ),
        kc(
            "k_color_overlay",
            "Color Overlay",
            COLOR,
            vec![
                fs("hue", "Hue", 0.0, (0.0, 360.0), (0.0, 360.0), "°", 0),
                num("saturation", "Saturation", 0.5, 0.0, 1.0, 2),
                num("lightness", "Lightness", 0.5, 0.0, 1.0, 2),
                num("mix", "Mix", 1.0, 0.0, 1.0, 2),
            ],
        ),
        kc(
            "k_colorize",
            "Colorize",
            COLOR,
            vec![num("hue", "Hue", 0.5, 0.0, 1.0, 2), num("saturation", "Saturation", 0.5, 0.0, 1.0, 2), num("lightness", "Lightness", 0.5, 0.0, 1.0, 2)],
        ),
        kc("k_equaliz0r", "Equaliz0r", COLOR, vec![]),
        kc(
            "k_fftfilt",
            "FFT-based FIR",
            COLOR,
            vec![
                int("dc_y", "Gain in Y Plane", 0.0, 0.0, 250.0),
                int("dc_u", "Gain in U Plane", 0.0, 0.0, 250.0),
                int("dc_v", "Gain in V Plane", 0.0, 0.0, 250.0),
                num("weight_y", "Luminance Y Plane", 1.0, 0.0, 5.0, 3),
                num("weight_u", "Luminance U Plane", 1.0, 0.0, 5.0, 3),
                num("weight_v", "Luminance V Plane", 1.0, 0.0, 5.0, 3),
                ch("eval", "Evaluate", &["Init", "Frame"], 0),
            ],
        ),
        kc("k_gradient_map", "Gradient Map", COLOR, gmap),
        kc("k_grayworld", "Grayworld", COLOR, vec![]),
        kc(
            "k_histeq",
            "Histogram Equalizer",
            COLOR,
            vec![
                num("strength", "Strength", 0.2, 0.0, 1.0, 3),
                num("intensity", "Intensity", 0.21, 0.0, 1.0, 3),
                ch("antibanding", "Antibanding Level", &["None", "Weak", "Strong"], 0),
            ],
        ),
        kc(
            "k_normaliz0r",
            "Normaliz0r",
            COLOR,
            vec![
                col("blackpt", "BlackPt", [0.0, 0.0, 0.0, 1.0]),
                col("whitept", "WhitePt", [1.0, 1.0, 1.0, 1.0]),
                fixed(int("smoothing", "Smoothing", 0.0, 0.0, 30.0)),
                num("independence", "Independence", 1.0, 0.0, 1.0, 2),
                num("strength", "Strength", 1.0, 0.0, 1.0, 2),
            ],
        ),
        kc(
            "k_normalize",
            "Normalize RGB Video",
            COLOR,
            vec![
                col("blackpt", "Output Darkest Input Color", [0.0, 0.0, 0.0, 1.0]),
                col("whitept", "Output Brightest Input Color", [1.0, 1.0, 1.0, 1.0]),
                fixed(int("smoothing", "Smoothing", 0.0, 0.0, 30.0)),
                num("independence", "Independence", 1.0, 0.0, 1.0, 2),
                num("strength", "Strength", 1.0, 0.0, 1.0, 2),
            ],
        ),
        kc("k_swapuv", "Swapuv", COLOR, vec![]),
        kc(
            "k_technicolor",
            "Technicolor",
            COLOR,
            vec![int("blue_yellow", "Blue/Yellow Axis", 190.0, -400.0, 400.0), int("red_green", "Red/Green Axis", 190.0, -400.0, 400.0)],
        ),
        kc(
            "k_wb_lms",
            "White Balance (LMS)",
            COLOR,
            vec![
                col("neutral", "Neutral Color", [1.0, 1.0, 1.0, 1.0]),
                fs("temperature", "Color Temperature", 6500.0, (1000.0, 15000.0), (2000.0, 12000.0), "K", 0),
            ],
        ),
        // ---------------------------------------------------------------- Image Adjustments
        kc(
            "k_colormatrix",
            "Color Matrix",
            ADJUST,
            vec![ch("src", "Source Color Matrix", COLOR_MATRICES, 2), ch("dst", "Destination Color Matrix", COLOR_MATRICES, 0)],
        ),
        kc(
            "k_colorspace",
            "Color Space",
            ADJUST,
            vec![
                ch("all", "Color Properties", CS_ALL, 1),
                ch("space", "Output Color Space", CS_SPACE, 0),
                ch("trc", "Output Transfer Characteristics", CS_TRC, 0),
                ch("primaries", "Output Color Primaries", CS_PRIMARIES, 0),
                ch("range", "Output Color Range", CS_RANGE, 0),
                ch("format", "Output Color Format", CS_FORMAT, 0),
                b("fast", "Fast Conversion", false),
                ch("dither", "Dithering Mode", &["None", "Floyd-Steinberg"], 0),
                ch("wpadapt", "Whitepoint Adaptation", &["Identity", "Bradford", "von Kries"], 1),
                grp(ch("iall", "Override All Input Properties", CS_ALL, 0), "Input"),
                grp(ch("ispace", "Override Input Color Space", CS_SPACE, 0), "Input"),
                grp(ch("iprimaries", "Override Input Color Primaries", CS_PRIMARIES, 0), "Input"),
                grp(ch("itrc", "Override Input Transfer Characteristics", CS_TRC, 0), "Input"),
                grp(ch("irange", "Override Input Color Range", CS_RANGE, 0), "Input"),
            ],
        ),
        kc(
            "k_deband",
            "Deband",
            ADJUST,
            vec![
                num("thr1", "1st Plane Threshold", 0.02, 0.00003, 0.5, 5),
                num("thr2", "2nd Plane Threshold", 0.02, 0.00003, 0.5, 5),
                num("thr3", "3rd Plane Threshold", 0.02, 0.00003, 0.5, 5),
                num("thr4", "4th Plane Threshold", 0.02, 0.00003, 0.5, 5),
                int("range", "Range", 16.0, -64.0, 64.0),
                num("direction", "Direction", std::f64::consts::TAU, -std::f64::consts::TAU, std::f64::consts::TAU, 3),
                b("blur", "Blur", true),
                b("coupling", "Coupling", false),
            ],
        ),
        kc(
            "k_dilation",
            "Dilation",
            ADJUST,
            vec![
                int("thr1", "1st Plane Threshold", 65535.0, 0.0, 65535.0),
                int("thr2", "2nd Plane Threshold", 65535.0, 0.0, 65535.0),
                int("thr3", "3rd Plane Threshold", 65535.0, 0.0, 65535.0),
                int("thr4", "4th Plane Threshold", 65535.0, 0.0, 65535.0),
                fixed(int("coordinates", "Coordinates", 255.0, 0.0, 255.0)),
            ],
        ),
        kc(
            "k_erosion",
            "Erosion",
            ADJUST,
            vec![
                int("thr1", "1st Plane Threshold", 65535.0, 0.0, 65535.0),
                int("thr2", "2nd Plane Threshold", 65535.0, 0.0, 65535.0),
                int("thr3", "3rd Plane Threshold", 65535.0, 0.0, 65535.0),
                int("thr4", "4th Plane Threshold", 65535.0, 0.0, 65535.0),
                fixed(int("coordinates", "Coordinates", 255.0, 0.0, 255.0)),
            ],
        ),
        kc("k_epx", "EPX Scaler", ADJUST, vec![ch("n", "Scale Factor", &["2x", "3x"], 1)]),
        kc("k_hqx", "HQ*X Interpolator", ADJUST, vec![ch("n", "Scale Factor", &["2x", "3x", "4x"], 1)]),
        kc("k_super2xsai", "Super2xSaI", ADJUST, vec![]),
        kc("k_xbr", "XBR Interpolator", ADJUST, vec![ch("n", "Scaling Dimension", &["2xBR", "3xBR", "4xBR"], 1)]),
        kc(
            "k_interleave",
            "Interleave - Deinterleave",
            ADJUST,
            vec![
                ch("luma_mode", "Luma Mode", IL_MODES, 0),
                ch("chroma_mode", "Chroma Mode", IL_MODES, 0),
                ch("alpha_mode", "Alpha Mode", IL_MODES, 0),
                b("luma_swap", "Swap Luma Fields", false),
                b("chroma_swap", "Swap Chroma Fields", false),
                b("alpha_swap", "Swap Alpha Fields", false),
            ],
        ),
        kc(
            "k_kerndeint",
            "Kernel Deinterlacer",
            ADJUST,
            vec![
                int("threshold", "Threshold", 10.0, 0.0, 255.0),
                b("map", "Paint in White Pixels Exceeding the Threshold", false),
                b("order", "Swap Fields", false),
                b("sharp", "Enable Additional Sharpening", false),
                b("twoway", "Enable Two-way Sharpening", false),
            ],
        ),
        kc(
            "k_mcdeint",
            "Motion Compensation Deinterlacer",
            ADJUST,
            vec![
                ch("mode", "Mode", &["Fast", "Medium", "Slow", "Extra Slow"], 1),
                ch("parity", "Picture Field Parity", &["Top Field First", "Bottom Field First"], 1),
                fixed(int("qp", "QP", 1.0, 0.0, 60.0)),
            ],
        ),
        kc("k_phase", "Phase", ADJUST, vec![ch("mode", "Mode", PHASE_MODES, 1)]),
        kc("k_setrange", "Set Range", ADJUST, vec![ch("range", "Set Range", &["Auto", "Unspecified", "Limited", "Full"], 0)]),
    ]
}
