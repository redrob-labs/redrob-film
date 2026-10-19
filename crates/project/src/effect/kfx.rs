//! The "More Video Effects" bin: stylize, grain/noise, generate, misc and deprecated clip effects
//! for editors coming from other open editors (effect names are functional facts; parameters,
//! ranges and defaults are our own, written from the public descriptions of what each effect
//! does). Pixel implementations live in `redrob_film_render::vfx::kfx`.
//!
//! Every effect here is a normal clip effect: it is applied from the Effects panel or
//! `effects.apply`, its parameters are edited and keyframed in Effect Controls, and it is saved in
//! the project with the clip.

use super::*;

/// Effects-panel top folder of this bin.
pub const MORE_TOP: &str = "More Video Effects";
const STYLIZE: &[&str] = &[MORE_TOP, "Stylize"];
const GRAIN: &[&str] = &[MORE_TOP, "Grain and Noise"];
const GENERATE: &[&str] = &[MORE_TOP, "Generate"];
const MISC: &[&str] = &[MORE_TOP, "Misc"];
const DEPRECATED: &[&str] = &[MORE_TOP, "Deprecated"];

/// Plane selections of the edge operators and denoisers (YUV planes of the picture; unprocessed
/// planes are copied).
pub const YUV_PLANES: &[&str] = &["None", "Y (Luminance)", "U (Chroma)", "YU", "V (Chroma)", "YV", "UV", "All", "Alpha"];
/// Plane selections of Kirsch / Scharr (YUV or RGB planes).
pub const ANY_PLANES: &[&str] = &["Alpha", "Y (Luminance)", "U (Chroma)", "V (Chroma)", "Red", "Green", "Blue", "All"];
/// Pseudo Color palettes.
pub const PSEUDO_PRESETS: &[&str] = &[
    "Magma",
    "Inferno",
    "Plasma",
    "Viridis",
    "Turbo",
    "Cividis",
    "Range1",
    "Range2",
    "Shadows",
    "Highlights",
    "Solar",
    "Nominal",
    "Preferred",
    "Total",
    "Spectral",
    "Cool",
    "Heat",
    "Fiery",
    "Blues",
    "Greens",
    "Helix",
];
/// Timer formats.
pub const TIMER_FORMATS: &[&str] = &["HH:MM:SS", "HH:MM:SS.S", "MM:SS", "MM:SS.SS", "MM:SS.SSS", "SS", "SS.S", "SS.SS", "SS.SSS"];
pub const H_ALIGN: &[&str] = &["Left", "Center", "Right"];
pub const V_ALIGN: &[&str] = &["Top", "Middle", "Bottom"];
pub const FONT_STYLES: &[&str] = &["Regular", "Bold", "Italic", "Bold Italic"];
/// GPS Graphic data sources and graph types.
pub const GPS_SOURCES: &[&str] = &["GPS Location", "Altitude", "Heart Rate", "Speed"];
pub const GPS_GRAPHS: &[&str] = &["Basic Graph", "Follow Current Position", "Speedometer"];
pub const GPS_COLOR_STYLES: &[&str] = &[
    "One Color",
    "Two Colors",
    "Solid Past, Thin Future",
    "Solid Future, Thin Past",
    "Vertical Gradient",
    "Horizontal Gradient",
    "Color by Duration",
    "Color by Altitude",
    "Color by Heart Rate",
    "Color by Speed",
];
/// GPS Text keywords (inserted as `#keyword#`).
pub const GPS_KEYWORDS: &[&str] = &[
    "gps_lat",
    "gps_lon",
    "gps_elev",
    "gps_speed",
    "gps_dist",
    "gps_datetime_now",
    "vid_datetime_now",
    "gps_hr",
    "gps_bearing",
    "gps_compass",
    "gps_vdist_up",
    "gps_vdist_down",
    "gps_dist_uphill",
    "gps_dist_downhill",
    "gps_dist_flat",
];

fn kv(id: &'static str, name: &'static str, cat: &'static [&'static str], params: Vec<ParamDef>) -> EffectDef {
    EffectDef { id, name, kind: EffectKind::Video, category: cat, params, intrinsic: false, accelerated: false, float32: false, yuv: false }
}
fn int(id: &'static str, label: &'static str, def: f64, min: f64, max: f64) -> ParamDef {
    fs(id, label, def, (min, max), (min, max), "", 0)
}
fn pct(id: &'static str, label: &'static str, def: f64) -> ParamDef {
    f(id, label, def, 0.0, 100.0, "%")
}
fn seed() -> ParamDef {
    int("seed", "Random Seed", 0.0, 0.0, 10000.0)
}
fn planes(def: u32) -> ParamDef {
    ch("planes", "Planes", YUV_PLANES, def)
}
fn edge_params(planes: ParamDef) -> Vec<ParamDef> {
    vec![planes, fs("scale", "Scale", 1.0, (0.0, 65535.0), (0.0, 10.0), "", 1), fs("delta", "Delta", 0.0, (-65535.0, 65535.0), (-255.0, 255.0), "", 0)]
}
/// Shared text controls of Timer and GPS Text.
fn text_box(v: &mut Vec<ParamDef>, size: f64) {
    v.push(grp(txt("font", "Font Family"), "Text"));
    v.push(grp(ch("font_style", "Font Style", FONT_STYLES, 0), "Text"));
    v.push(grp(fs("size", "Size", size, (1.0, 100.0), (1.0, 30.0), "%", 1), "Text"));
    v.push(grp(int("outline", "Outline Width", 0.0, 0.0, 3.0), "Text"));
    v.push(grp(int("padding", "Padding", 0.0, 0.0, 400.0), "Text"));
    v.push(grp(ch("h_align", "Horizontal Alignment", H_ALIGN, 0), "Text"));
    v.push(grp(ch("v_align", "Vertical Alignment", V_ALIGN, 0), "Text"));
    v.push(grp(pct("opacity", "Opacity", 100.0), "Text"));
    v.push(grp(col("fg", "Foreground Color", [1.0, 1.0, 1.0, 1.0]), "Text"));
    v.push(grp(col("bg", "Background Color", [0.0, 0.0, 0.0, 0.0]), "Text"));
    v.push(grp(col("outline_color", "Outline Color", [0.0, 0.0, 0.0, 1.0]), "Text"));
    v.push(grp(pct("rect_x", "X", 5.0), "Geometry"));
    v.push(grp(pct("rect_y", "Y", 5.0), "Geometry"));
    v.push(grp(pct("rect_w", "W", 90.0), "Geometry"));
    v.push(grp(pct("rect_h", "H", 90.0), "Geometry"));
}
fn gps_common(v: &mut Vec<ParamDef>) {
    v.push(txt("gps_file", "GPS File"));
    v.push(fs("time_offset", "Time Offset in Seconds", 0.0, (-31_536_000.0, 31_536_000.0), (-7200.0, 7200.0), "s", 0));
    v.push(int("smoothing", "Smoothing Level", 5.0, 0.0, 100.0));
}

pub(super) fn defs() -> Vec<EffectDef> {
    let mut v = vec![
        // ---------------------------------------------------------------- Stylize
        kv("k_threshold3", "3-Level Threshold", STYLIZE, vec![]),
        kv(
            "k_analog_echo",
            "Analog Video Echo",
            STYLIZE,
            vec![
                fs("fade", "Fade Factor", 0.6, (0.0, 0.99), (0.0, 0.99), "", 2),
                ch("direction", "Direction", &["Brighter", "Darker"], 0),
                b("keep_r", "Keep Red", true),
                b("keep_g", "Keep Green", true),
                b("keep_b", "Keep Blue", true),
                int("strobe", "Strobe Period", 0.0, 0.0, 8.0),
            ],
        ),
        kv("k_binarize", "Binarize", STYLIZE, vec![b("use_alpha", "Use Transparency", false), int("threshold", "Threshold Value", 128.0, 0.0, 255.0)]),
        kv("k_binarize_dynamic", "Binarize Dynamically", STYLIZE, vec![]),
        kv("k_burning_tv", "Burning TV", STYLIZE, vec![b("foreground", "Foreground Only", false), int("threshold", "Movement Threshold", 50.0, 0.0, 255.0)]),
        kv(
            "k_cartoon",
            "Cartoon",
            STYLIZE,
            vec![fs("trip", "Level of Trip", 0.4, (0.0, 1.0), (0.0, 1.0), "", 2), fs("diffspace", "Difference Space", 2.0, (1.0, 16.0), (1.0, 8.0), "", 0)],
        ),
        kv(
            "k_charcoal",
            "Charcoal",
            STYLIZE,
            vec![
                b("invert", "Invert", false),
                int("h_scatter", "Horizontal Scatter", 2.0, 1.0, 10.0),
                int("v_scatter", "Vertical Scatter", 2.0, 1.0, 10.0),
                int("scale", "Scale", 1.0, 1.0, 10.0),
                int("mix", "Mix", 0.0, 0.0, 10.0),
            ],
        ),
        kv("k_color_distance", "Color Distance", STYLIZE, vec![col("color", "Source Color", [0.5, 0.5, 0.5, 1.0])]),
        kv(
            "k_color_effect",
            "Color Effect",
            STYLIZE,
            vec![ch("type", "Type", &["X-Pro", "Sepia", "Heat", "Red-Green", "Old Photo", "X-Ray", "Esses", "Yellow-Blue"], 0)],
        ),
        kv(
            "k_heatmap",
            "Heatmap0r",
            STYLIZE,
            vec![
                col("black", "Map Black To", [0.0, 0.0, 1.0, 1.0]),
                col("grey", "Map Grey To", [1.0, 1.0, 0.0, 1.0]),
                col("white", "Map White To", [1.0, 0.0, 0.0, 1.0]),
            ],
        ),
        kv(
            "k_kaleidoscope",
            "Kaleidoscope",
            STYLIZE,
            vec![
                b("specify_source", "Specify Source Segment", false),
                ch("direction", "Segmentation Direction", &["None", "Counter Clockwise", "Clockwise"], 2),
                b("reflect_edges", "Reflect Edges", true),
                ch("corner", "Preferred Corner", &["Top Right", "Top Left", "Bottom Left", "Bottom Right"], 0),
                b("corner_search", "Corner Search", true),
                b("no_alpha_bg", "No Alpha Background", true),
                pct("origin_x", "Origin X", 50.0),
                pct("origin_y", "Origin Y", 50.0),
                fs("segmentation", "Segmentation", 16.0, (1.0, 128.0), (1.0, 64.0), "", 0),
                ang("source_segment", "Source Segment", 0.0),
                fs("edge_threshold", "Edge Threshold", 0.0, (0.0, 4.0), (0.0, 4.0), "", 0),
                col("bg_color", "Background Color", [1.0, 0.0, 1.0, 1.0]),
            ],
        ),
        kv("k_kirsch", "Kirsch", STYLIZE, edge_params(ch("planes", "Planes", ANY_PLANES, 1))),
        kv(
            "k_light_graffiti",
            "Light Graffiti",
            STYLIZE,
            vec![
                grp(b("stats_brightness", "Show Brightness Statistics", false), "Statistics"),
                grp(b("stats_diff", "Show Background Difference Statistics", false), "Statistics"),
                grp(b("stats_diff_sum", "Show Background Difference Sum Statistics", false), "Statistics"),
                b("transparent", "Transparent Background", false),
                b("nonlinear", "Nonlinear Dimming", false),
                b("reset", "Reset", false),
                int("brightness", "Brightness Threshold", 450.0, 0.0, 765.0),
                int("difference", "Difference Threshold", 40.0, 0.0, 255.0),
                int("difference_sum", "Difference Sum Threshold", 100.0, 0.0, 765.0),
                int("sensitivity", "Sensitivity", 100.0, 0.0, 500.0),
                int("lower_overexposure", "Lower Overexposure", 0.0, 0.0, 10.0),
                pct("dimming", "Dimming", 0.0),
                pct("bg_weight", "Background Weight", 0.0),
                pct("alpha", "α", 0.0),
                fs("saturation", "Saturation", 100.0, (0.0, 400.0), (0.0, 300.0), "%", 0),
            ],
        ),
        kv(
            "k_ndvi",
            "NDVI Filter",
            STYLIZE,
            vec![
                int("levels", "Levels", 256.0, 2.0, 1000.0),
                fs("vis_scale", "VIS Scale", 1.0, (0.0, 10.0), (0.0, 10.0), "", 2),
                f("vis_offset", "VIS Offset", 0.0, -100.0, 100.0, "%"),
                fs("nir_scale", "NIR Scale", 1.0, (0.0, 10.0), (0.0, 10.0), "", 2),
                f("nir_offset", "NIR Offset", 0.0, -100.0, 100.0, "%"),
            ],
        ),
        kv("k_ntsc", "NTSC", STYLIZE, vec![b("progressive", "Progressive Scan", true), b("scanlines", "Scanlines", false), pct("noise", "Signal Noise", 10.0)]),
        kv(
            "k_old_film",
            "Old Film Simulator",
            STYLIZE,
            vec![
                int("move", "Maximum Up/Down Move", 2.0, 0.0, 100.0),
                pct("move_every", "Frames With Movement", 40.0),
                int("bright_up", "Brightness Up", 20.0, 0.0, 100.0),
                int("bright_down", "Brightness Down", 30.0, 0.0, 100.0),
                pct("bright_every", "Brightness Every", 70.0),
                int("develop_up", "Uneven Develop Up", 60.0, 0.0, 100.0),
                int("develop_down", "Uneven Develop Down", 20.0, 0.0, 100.0),
                int("develop_duration", "Uneven Develop Duration", 70.0, 1.0, 1000.0),
            ],
        ),
        kv(
            "k_photosensitivity",
            "Photosensitivity Filter",
            STYLIZE,
            vec![
                b("bypass", "Bypass", false),
                int("frames", "Frames to Use", 30.0, 2.0, 240.0),
                fs("threshold", "Threshold", 1.0, (0.1, 100.0), (0.1, 10.0), "", 1),
                int("skip", "Skip", 1.0, 1.0, 1024.0),
            ],
        ),
        kv(
            "k_pixelize",
            "Pixelize (advanced)",
            STYLIZE,
            vec![
                ch("mode", "Pixelize Mode", &["Average", "Minimum", "Maximum"], 0),
                planes(7),
                int("block_w", "Block Width", 16.0, 1.0, 1024.0),
                int("block_h", "Block Height", 16.0, 1.0, 1024.0),
            ],
        ),
        kv(
            "k_pixels0rt",
            "Pixels0rt",
            STYLIZE,
            vec![
                fs("threshold", "Threshold", 0.5, (0.0, 1.0), (0.0, 1.0), "", 2),
                ch("direction", "Direction", &["Vertical Top to Bottom", "Vertical Bottom to Top", "Horizontal Left to Right", "Horizontal Right to Left"], 0),
                b("reversed", "Reversed", false),
                fs("alpha_threshold", "Transparency Threshold", 0.0, (0.0, 1.0), (0.0, 1.0), "", 2),
            ],
        ),
        kv("k_posterize_elbg", "Posterize (ELBG)", STYLIZE, vec![int("codebook", "Codebook Length", 9.0, 1.0, 50.0), int("steps", "Steps", 1.0, 1.0, 50.0)]),
        kv("k_prewitt", "Prewitt", STYLIZE, edge_params(planes(1))),
        kv("k_primaries", "Primaries", STYLIZE, vec![int("factor", "Factor", 9.0, 0.0, 32.0)]),
        kv(
            "k_pseudocolor",
            "Pseudo Color",
            STYLIZE,
            vec![
                ch("component", "Base Component", &["Red", "Green", "Blue", "Alpha"], 0),
                ch("preset", "Preset", PSEUDO_PRESETS, 0),
                pct("opacity", "Opacity", 100.0),
            ],
        ),
        kv(
            "k_rgb_splitter",
            "RGB Splitter",
            STYLIZE,
            vec![
                fs("v_split", "Vertical Split Distance", 10.0, (-1000.0, 1000.0), (-100.0, 100.0), "", 1),
                fs("h_split", "Horizontal Split Distance", 10.0, (-1000.0, 1000.0), (-100.0, 100.0), "", 1),
            ],
        ),
        kv("k_roberts", "Roberts", STYLIZE, edge_params(planes(7))),
        kv("k_scharr", "Scharr", STYLIZE, edge_params(ch("planes", "Planes", ANY_PLANES, 7))),
        kv("k_sobel", "Sobel", STYLIZE, vec![]),
        kv("k_sobel_planes", "Sobel with Planes", STYLIZE, edge_params(planes(7))),
        kv(
            "k_squigglevision",
            "Squigglevision",
            STYLIZE,
            vec![pct("strength", "Strength", 20.0), int("fps", "FPS", 8.0, 1.0, 30.0), int("scale", "Scale", 32.0, 4.0, 128.0)],
        ),
        kv("k_threshold", "Threshold", STYLIZE, vec![int("threshold", "Threshold", 128.0, 0.0, 255.0)]),
        kv(
            "k_typewriter",
            "Typewriter",
            STYLIZE,
            vec![
                int("step", "Frames Step", 2.0, 1.0, 100.0),
                int("fluctuation", "Fluctuation", 0.0, 0.0, 100.0),
                seed(),
                ch("type", "Type", &["Char by Char", "Word by Word", "Line by Line"], 0),
            ],
        ),
        // ---------------------------------------------------------------- Grain and Noise
        kv(
            "k_fft_denoise",
            "3D FFT Denoiser",
            GRAIN,
            vec![
                fs("sigma", "Sigma", 1.0, (0.0, 30.0), (0.0, 30.0), "", 1),
                fs("amount", "Amount", 1.0, (0.0, 1.0), (0.0, 1.0), "", 2),
                int("block", "Block", 4.0, 2.0, 6.0),
                fs("overlap", "Overlap", 0.5, (0.2, 0.8), (0.2, 0.8), "", 2),
                ch("method", "Method", &["Wiener", "Hard"], 0),
                b("prev", "Add Previous Frame to Temporal Denoise", false),
                b("next", "Add Next Frame to Temporal Denoise", false),
                planes(7),
            ],
        ),
        kv(
            "k_chroma_nr",
            "Chroma Noise Reduction",
            GRAIN,
            vec![
                int("threshold", "Y+U+V Threshold", 30.0, 1.0, 200.0),
                int("size_w", "Horizontal Size", 5.0, 1.0, 100.0),
                int("size_h", "Vertical Size", 5.0, 1.0, 100.0),
                int("step_w", "Horizontal Step", 1.0, 1.0, 50.0),
                int("step_h", "Vertical Step", 1.0, 1.0, 50.0),
                int("thr_y", "Y Threshold", 200.0, 1.0, 200.0),
                int("thr_u", "U Threshold", 200.0, 1.0, 200.0),
                int("thr_v", "V Threshold", 200.0, 1.0, 200.0),
                ch("distance", "Distance", &["Manhattan", "Euclidean"], 0),
            ],
        ),
        kv(
            "k_hqdn3d",
            "HQ 3D Denoiser",
            GRAIN,
            vec![fs("spatial", "Spatial", 4.0, (0.0, 100.0), (0.0, 30.0), "", 1), fs("temporal", "Temporal", 6.0, (0.0, 100.0), (0.0, 30.0), "", 1)],
        ),
        kv("k_dither", "Dither", GRAIN, vec![int("levels", "Levels", 8.0, 2.0, 255.0), int("matrix", "Matrix ID", 2.0, 0.0, 6.0)]),
        kv(
            "k_dust",
            "Dust",
            GRAIN,
            vec![fs("diameter", "Maximum Diameter", 2.0, (0.1, 20.0), (0.1, 10.0), "%", 1), int("count", "Maximum Number of Particles", 10.0, 0.0, 200.0)],
        ),
        kv(
            "k_film_grain",
            "Film Grain",
            GRAIN,
            vec![
                pct("grain", "Grain Amount", 20.0),
                pct("red", "Red Grain", 75.0),
                pct("green", "Green Grain", 70.0),
                pct("blue", "Blue Grain", 85.0),
                pct("blur", "Blur Amount", 20.0),
                pct("dust", "Dust Amount", 10.0),
                pct("flicker", "Flicker", 10.0),
            ],
        ),
        kv("k_gradfun", "Gradfun", GRAIN, vec![fs("strength", "Strength", 1.2, (0.51, 64.0), (0.51, 20.0), "", 2), int("radius", "Radius", 16.0, 4.0, 32.0)]),
        kv(
            "k_scratch_lines",
            "Scratch Lines",
            GRAIN,
            vec![
                int("width", "Width of Line", 2.0, 1.0, 20.0),
                int("count", "Max Number of Lines", 5.0, 0.0, 100.0),
                int("darker", "Max Darker", 40.0, 0.0, 100.0),
                int("lighter", "Max Lighter", 40.0, 0.0, 100.0),
            ],
        ),
        kv(
            "k_wavelet_denoise",
            "Wavelet Denoiser",
            GRAIN,
            vec![
                ch("method", "Method", &["Soft", "Hard", "Garrote"], 2),
                ch("threshold_type", "Threshold Type", &["Universal", "Bayes"], 0),
                fs("threshold", "Threshold", 2.0, (0.0, 255.0), (0.0, 30.0), "", 1),
                int("steps", "Steps", 6.0, 1.0, 32.0),
                pct("percent", "Percentage", 85.0),
                planes(7),
            ],
        ),
        // ---------------------------------------------------------------- Generate
        kv(
            "k_draw_box",
            "Draw Box",
            GENERATE,
            vec![
                col("color", "Color", [1.0, 0.0, 0.0, 1.0]),
                b("replace", "Replace Color and Alpha", false),
                fs("x", "Top-left X", 100.0, (-10000.0, 10000.0), (0.0, 1920.0), "px", 0),
                fs("y", "Top-left Y", 100.0, (-10000.0, 10000.0), (0.0, 1080.0), "px", 0),
                fs("w", "Width (px)", 400.0, (0.0, 10000.0), (0.0, 1920.0), "px", 0),
                fs("h", "Height (px)", 300.0, (0.0, 10000.0), (0.0, 1080.0), "px", 0),
                fs("thickness", "Thickness (px)", 4.0, (0.0, 10000.0), (0.0, 200.0), "px", 0),
            ],
        ),
        kv("k_scanline", "Scanline0r", GENERATE, vec![]),
        // ---------------------------------------------------------------- Misc
        kv("k_alphaextract", "Alphaextract", MISC, vec![]),
        kv(
            "k_deinterlace",
            "Xine Deinterlacer",
            MISC,
            vec![ch("method", "Method", &["Linear Blend", "One Field", "Linear Interpolation"], 0), b("bottom_first", "Bottom Field First", false)],
        ),
        // ---------------------------------------------------------------- Deprecated
        kv(
            "k_box_blur",
            "Box Blur",
            DEPRECATED,
            vec![fs("hori", "Horizontal", 2.0, (0.0, 1000.0), (0.0, 100.0), "px", 0), fs("vert", "Vertical", 2.0, (0.0, 1000.0), (0.0, 100.0), "px", 0)],
        ),
        kv("k_denoiser", "Denoiser", DEPRECATED, vec![int("spatial", "Spatial", 4.0, 0.0, 100.0), int("temporal", "Temporal", 6.0, 0.0, 100.0)]),
        kv("k_grain", "Grain", DEPRECATED, vec![pct("noise", "Noise", 40.0), pct("blur", "Blur Amount", 0.0)]),
        kv(
            "k_hqdn3d_legacy",
            "Hqdn3d",
            DEPRECATED,
            vec![fs("spatial", "Spatial", 4.0, (0.0, 100.0), (0.0, 30.0), "", 1), fs("temporal", "Temporal", 6.0, (0.0, 100.0), (0.0, 30.0), "", 1)],
        ),
        kv(
            "k_kmeans",
            "K-Means Clustering",
            DEPRECATED,
            vec![int("clusters", "Number of Clusters", 20.0, 2.0, 64.0), fs("distance_weight", "Distance Weight", 0.5, (0.0, 1.0), (0.0, 1.0), "", 2)],
        ),
    ];
    // Timer
    let mut timer = vec![
        ch("format", "Format", TIMER_FORMATS, 0),
        fs("start", "Start", 0.0, (0.0, 86_400.0), (0.0, 600.0), "s", 2),
        fs("duration", "Duration", 60.0, (0.0, 86_400.0), (0.0, 600.0), "s", 2),
        fs("offset", "Offset", 0.0, (0.0, 86_400.0), (0.0, 600.0), "s", 2),
        ch("direction", "Direction", &["Up", "Down"], 0),
    ];
    text_box(&mut timer, 8.0);
    v.push(kv("k_timer", "Timer", GENERATE, timer));
    // GPS Text
    let mut gps_text = vec![txt("text", "Text"), ch("keyword", "Keyword", GPS_KEYWORDS, 0)];
    gps_common(&mut gps_text);
    gps_text.push(int("updates", "Updates per Second", 1.0, 0.0, 60.0));
    text_box(&mut gps_text, 5.0);
    gps_text.push(grp(b("underline", "Underline", false), "Text"));
    gps_text.push(grp(b("strikethrough", "Strikethrough", false), "Text"));
    v.push(kv("k_gps_text", "GPS Text", GENERATE, gps_text));
    // GPS Graphic
    let mut gps_graphic = Vec::new();
    gps_common(&mut gps_graphic);
    gps_graphic.extend([
        fs("speed_multiplier", "Speed Multiplier", 1.0, (0.001, 1000.0), (0.1, 60.0), "", 2),
        ch("source", "Graph Data Source", GPS_SOURCES, 0),
        ch("graph", "Graph Type", GPS_GRAPHS, 0),
        pct("trim_start", "Trim Start", 0.0),
        pct("trim_end", "Trim End", 100.0),
        grp(b("crop_h_abs", "Crop Mode Horizontal (Absolute)", false), "Crop"),
        grp(fs("crop_left", "Crop Left", 0.0, (-1e9, 1e9), (-100.0, 100.0), "", 3), "Crop"),
        grp(fs("crop_right", "Crop Right", 100.0, (-1e9, 1e9), (0.0, 200.0), "", 3), "Crop"),
        grp(b("crop_v_abs", "Crop Mode Vertical (Absolute)", false), "Crop"),
        grp(fs("crop_bottom", "Crop Bottom", 0.0, (-1e9, 1e9), (-100.0, 100.0), "", 3), "Crop"),
        grp(fs("crop_top", "Crop Top", 100.0, (-1e9, 1e9), (0.0, 200.0), "", 3), "Crop"),
        ch("color_style", "Graph Color Style", GPS_COLOR_STYLES, 0),
        col("color1", "Color 1", [0.2, 0.6, 1.0, 1.0]),
        col("color2", "Color 2", [1.0, 0.4, 0.1, 1.0]),
        b("now_dot", "Show Now Dot", true),
        col("now_dot_color", "Now Dot Color", [0.0, 0.0, 0.0, 0.0]),
        b("now_text", "Show Now Text", false),
        ang("rotation", "Rotation", 0.0),
        int("thickness", "Line Thickness (px)", 5.0, 1.0, 100.0),
        b("legend", "Draw Legend", false),
        txt("legend_unit", "Legend Unit"),
        b("dots_only", "Show Dots Only", false),
        grp(txt("bg_image", "Background Image"), "Background"),
        grp(fs("bg_scale", "Background Scale", 1.0, (0.0, 10.0), (0.0, 3.0), "", 2), "Background"),
        grp(pct("bg_opacity", "Background Opacity", 100.0), "Background"),
        grp(pct("rect_x", "X", 60.0), "Geometry"),
        grp(pct("rect_y", "Y", 55.0), "Geometry"),
        grp(pct("rect_w", "W", 35.0), "Geometry"),
        grp(pct("rect_h", "H", 40.0), "Geometry"),
    ]);
    v.push(kv("k_gps_graphic", "GPS Graphic", GENERATE, gps_graphic));
    v
}

/// The bin's effects run on the CPU in 8-bit-like display space: no GPU / 32-bit / YUV badges.
pub(super) fn apply_badges(v: &mut [EffectDef]) {
    for e in v.iter_mut().filter(|e| e.category.first() == Some(&MORE_TOP)) {
        e.accelerated = false;
        e.float32 = false;
        e.yuv = false;
    }
}
