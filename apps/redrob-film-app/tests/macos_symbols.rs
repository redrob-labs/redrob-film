//! Every VideoToolbox / CoreMedia / CoreVideo name the macOS FFI modules use exists on the oldest
//! macOS we support (`LSMinimumSystemVersion` / `MACOSX_DEPLOYMENT_TARGET` = 11.0).
//!
//! Rust links every framework symbol it names strongly, whatever its availability: a symbol newer
//! than the running macOS makes dyld refuse to start the whole app ("Symbol not found:
//! _kVTCompressionPropertyKey_ConstantBitRate", upstream #468 / #655 / #661, on macOS 12). A newer
//! symbol must be looked up at run time with `dlsym` instead (as `constant_bitrate_key` does) and
//! listed in [`RUNTIME_ONLY`].
//!
//! The table below gives the first macOS version of each name, from Apple's API reference
//! ("Availability" on each symbol's page). A name the modules start using that is not in the table
//! fails this test until its availability has been checked and added. Types and enum constants are
//! listed too: they are not linked, but keeping one list is simpler than telling them apart.
//!
//! The objc2 methods the modules call wrap these functions, all macOS 10.7–10.9 except where noted:
//! `VTCompressionSession::create` / `encode_frame` / `complete_frames` / `invalidate` /
//! `pixel_buffer_pool` (10.8), `prepare_to_encode_frames` (10.9); `VTDecompressionSession::create` /
//! `decode_frame` / `wait_for_asynchronous_frames` / `invalidate` (10.8); `CMBlockBuffer::*`,
//! `CMSampleBuffer::*` (10.7).

/// (name, first macOS version as (major, minor)).
const AVAILABILITY: &[(&str, (u32, u32))] = &[
    // VideoToolbox: decoding
    ("VTDecompressionSession", (10, 8)),
    ("VTDecompressionSessionCreate", (10, 8)),
    ("VTDecompressionSessionDecodeFrame", (10, 8)),
    ("VTDecompressionOutputCallbackRecord", (10, 8)),
    ("VTDecodeFrameFlags", (10, 8)),
    ("VTDecodeInfoFlags", (10, 8)),
    ("VTIsHardwareDecodeSupported", (10, 13)),
    ("kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder", (10, 9)),
    // VideoToolbox: encoding
    ("VTCompressionSession", (10, 8)),
    ("VTCompressionSessionCreate", (10, 8)),
    ("VTCompressionSessionEncodeFrame", (10, 8)),
    ("VTEncodeInfoFlags", (10, 8)),
    ("VTSessionCopyProperty", (10, 8)),
    ("VTSessionSetProperty", (10, 8)),
    ("kVTCompressionPropertyKey_AllowFrameReordering", (10, 8)),
    ("kVTCompressionPropertyKey_AllowOpenGOP", (10, 14)),
    ("kVTCompressionPropertyKey_AverageBitRate", (10, 8)),
    ("kVTCompressionPropertyKey_ColorPrimaries", (10, 8)),
    ("kVTCompressionPropertyKey_DataRateLimits", (10, 8)),
    ("kVTCompressionPropertyKey_ExpectedFrameRate", (10, 8)),
    ("kVTCompressionPropertyKey_MaxKeyFrameInterval", (10, 8)),
    ("kVTCompressionPropertyKey_ProfileLevel", (10, 8)),
    ("kVTCompressionPropertyKey_RealTime", (10, 9)),
    ("kVTCompressionPropertyKey_TransferFunction", (10, 8)),
    ("kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder", (10, 9)),
    ("kVTCompressionPropertyKey_YCbCrMatrix", (10, 8)),
    ("kVTProfileLevel_H264_Baseline_AutoLevel", (10, 8)),
    ("kVTProfileLevel_H264_Main_AutoLevel", (10, 8)),
    ("kVTProfileLevel_H264_High_AutoLevel", (10, 8)),
    ("kVTProfileLevel_HEVC_Main_AutoLevel", (10, 13)),
    ("kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder", (10, 9)),
    // CoreMedia
    ("CMBlockBuffer", (10, 7)),
    ("CMBlockBufferCreateWithMemoryBlock", (10, 7)),
    ("CMBlockBufferReplaceDataBytes", (10, 7)),
    ("CMFormatDescription", (10, 7)),
    ("CMSampleBuffer", (10, 7)),
    ("CMSampleBufferCreateReady", (10, 7)),
    ("CMSampleTimingInfo", (10, 7)),
    ("CMTime", (10, 7)),
    ("CMTimeFlags", (10, 7)),
    ("CMVideoFormatDescription", (10, 7)),
    ("CMVideoFormatDescriptionCreateFromH264ParameterSets", (10, 9)),
    ("CMVideoFormatDescriptionCreateFromHEVCParameterSets", (10, 13)),
    ("CMVideoFormatDescriptionGetH264ParameterSetAtIndex", (10, 9)),
    ("CMVideoFormatDescriptionGetHEVCParameterSetAtIndex", (10, 13)),
    ("kCMBlockBufferAssureMemoryNowFlag", (10, 7)),
    ("kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms", (10, 7)),
    ("kCMTimeInvalid", (10, 7)),
    ("kCMVideoCodecType_H264", (10, 7)),
    ("kCMVideoCodecType_HEVC", (10, 11)),
    // CoreVideo
    ("CVImageBuffer", (10, 4)),
    ("CVPixelBuffer", (10, 4)),
    ("CVPixelBufferCreate", (10, 4)),
    ("CVPixelBufferGetBaseAddressOfPlane", (10, 4)),
    ("CVPixelBufferGetBytesPerRowOfPlane", (10, 4)),
    ("CVPixelBufferGetHeight", (10, 4)),
    ("CVPixelBufferGetHeightOfPlane", (10, 4)),
    ("CVPixelBufferGetPixelFormatType", (10, 4)),
    ("CVPixelBufferGetPlaneCount", (10, 4)),
    ("CVPixelBufferGetWidth", (10, 4)),
    ("CVPixelBufferGetWidthOfPlane", (10, 4)),
    ("CVPixelBufferLockBaseAddress", (10, 4)),
    ("CVPixelBufferLockFlags", (10, 4)),
    ("CVPixelBufferPool", (10, 4)),
    ("CVPixelBufferUnlockBaseAddress", (10, 4)),
    ("kCVImageBufferColorPrimaries_ITU_R_709_2", (10, 4)),
    ("kCVImageBufferTransferFunction_ITU_R_709_2", (10, 4)),
    ("kCVImageBufferYCbCrMatrix_ITU_R_709_2", (10, 4)),
    ("kCVPixelBufferHeightKey", (10, 4)),
    ("kCVPixelBufferIOSurfacePropertiesKey", (10, 4)),
    ("kCVPixelBufferPixelFormatTypeKey", (10, 4)),
    ("kCVPixelBufferWidthKey", (10, 4)),
];

/// Newer than the deployment target: may appear only inside a string (the `dlsym` name), never as
/// a linked name.
const RUNTIME_ONLY: &[(&str, (u32, u32))] = &[("kVTCompressionPropertyKey_ConstantBitRate", (13, 0))];

const DEPLOYMENT_TARGET: (u32, u32) = (11, 0);

const SOURCES: &[(&str, &str)] = &[
    ("crates/platform/src/videotoolbox.rs", include_str!("../../../crates/platform/src/videotoolbox.rs")),
    ("crates/platform/src/videotoolbox_encode.rs", include_str!("../../../crates/platform/src/videotoolbox_encode.rs")),
];

/// The identifiers of Rust source outside comments and string literals.
fn code_identifiers(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut ident = String::new();
    let mut chars = src.chars().peekable();
    let flush = |ident: &mut String, out: &mut Vec<String>| {
        if !ident.is_empty() {
            out.push(std::mem::take(ident));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                flush(&mut ident, &mut out);
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                flush(&mut ident, &mut out);
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            '"' => {
                flush(&mut ident, &mut out);
                let mut escaped = false;
                for c in chars.by_ref() {
                    match c {
                        _ if escaped => escaped = false,
                        '\\' => escaped = true,
                        '"' => break,
                        _ => {}
                    }
                }
            }
            c if c.is_ascii_alphanumeric() || c == '_' => ident.push(c),
            _ => flush(&mut ident, &mut out),
        }
    }
    flush(&mut ident, &mut out);
    out
}

/// A VideoToolbox / CoreMedia / CoreVideo name: `VT…`, `CM…`, `CV…`, `kVT…`, `kCM…`, `kCV…`
/// followed by a capitalised word (`CMTime`, not `CMD`).
fn is_apple_media_name(id: &str) -> bool {
    let rest = id.strip_prefix('k').unwrap_or(id);
    ["VT", "CM", "CV"].iter().any(|p| {
        let mut word = rest.strip_prefix(p).unwrap_or("").chars();
        word.next().is_some_and(|c| c.is_ascii_uppercase()) && word.next().is_some_and(|c| c.is_ascii_lowercase())
    })
}

#[test]
fn every_linked_media_symbol_exists_on_the_oldest_supported_macos() {
    let mut unknown = Vec::new();
    let mut too_new = Vec::new();
    for (path, src) in SOURCES {
        for id in code_identifiers(src).into_iter().filter(|id| is_apple_media_name(id)) {
            if let Some((name, v)) = RUNTIME_ONLY.iter().find(|(n, _)| *n == id) {
                too_new.push(format!("{path}: {name} (macOS {}.{}) is linked; look it up with dlsym", v.0, v.1));
                continue;
            }
            match AVAILABILITY.iter().find(|(n, _)| *n == id) {
                Some((_, v)) if *v > DEPLOYMENT_TARGET => too_new.push(format!("{path}: {id} needs macOS {}.{}", v.0, v.1)),
                Some(_) => {}
                None => {
                    let line = format!("{path}: {id}");
                    if !unknown.contains(&line) {
                        unknown.push(line);
                    }
                }
            }
        }
    }
    assert!(too_new.is_empty(), "symbols newer than macOS {}.{} are linked directly: {too_new:#?}", DEPLOYMENT_TARGET.0, DEPLOYMENT_TARGET.1);
    assert!(unknown.is_empty(), "add these to AVAILABILITY after checking Apple's availability for each: {unknown:#?}");
}

/// The run-time lookups stay in place: the newer keys appear only as `dlsym` names.
#[test]
fn newer_keys_are_looked_up_at_run_time() {
    let encode = SOURCES[1].1;
    for (name, _) in RUNTIME_ONLY {
        assert!(encode.contains(&format!("c\"{name}\"")), "{name} is no longer looked up with dlsym");
    }
}

#[test]
fn the_scanner_skips_comments_and_strings() {
    let ids = code_identifiers("use a::{VTFoo, kCMBar}; // CVHidden\nlet s = c\"kVTInString\"; /* CMHidden */ CVSeen");
    assert_eq!(ids.iter().filter(|i| is_apple_media_name(i)).collect::<Vec<_>>(), ["VTFoo", "kCMBar", "CVSeen"]);
    assert!(!is_apple_media_name("VtCodec") && !is_apple_media_name("CMD") && !is_apple_media_name("kvt"));
}
