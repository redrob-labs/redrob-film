# Where Redrob Film falls short of Premiere Pro

> **Last reviewed:** 2026-10-19 · **Last updated:** 2026-10-19 · **Change:** minor (G2: `.prproj` import landed) · **Target:** Adobe Premiere Pro 2026 (26.5.2)

Every known shortfall, one entry each, ranked by what most stops an editor using Redrob Film for real
work. This is the work list: agents should prefer it over adding checklist items. Numbers are in
[target-app-parity.md](target-app-parity.md); estimates are Opus 5.5 agent-hours, calibrated there.
When you close or shrink a gap, update its entry, its parity doc and the revision history.

Summary by kind: **feature gaps** G1, G2, G12, G13, G18, G19; **UI / UX** G8; **file formats**
G2, G10, G13, G15; **codecs** G3, G10; **hardware** G5; **localization** G14; **stability and
platforms** G4, G9, G17; **performance** G16; **ecosystem** G11; **measurement** G1, G6, G7;
**web** G20 (closed).

| # | Gap | Kind | Blocks beta | Estimate |
|---|---|---|---|---|
| G1 | Nothing compares our output with Premiere's | measurement | no, but every ready number depends on it | 40–70 h |
| G2 | Premiere's `.prproj` cannot be opened (import landed 2026-10-19, unmeasured) | file format | **yes** | 30–60 h |
| G3 | 10-bit and 4:2:2 camera media needs hardware | codec | yes | 30–50 h |
| G4 | Crashes at start-up, and no test CI on pull requests | stability | yes | 40–80 h |
| G5 | Hardware acceleration is partial | hardware | partly | 120–200 h |
| G6 | Real-world media corpus: harness in, not yet green or pinned | measurement | yes | 20–40 h |
| G7 | No measured parity tool in CI | measurement | no | 10–20 h |
| G8 | UI fidelity: monitor handles, Effect Controls, docking | UI / UX | partly | 120–200 h |
| G9 | Windows and Linux at run time | platforms | partly | 50–90 h |
| G10 | Container and codec gaps on import | file format, codec | partly | 120–200 h |
| G11 | No plugin hosting | ecosystem | no | 200–400 h |
| G12 | AI features | AI | no | 150–300 h |
| G13 | Delivery formats | file format | no | 60–110 h |
| G14 | Localization: 8 of the 12 key languages missing | localization | no | 150–230 h |
| G15 | Interchange never validated in Avid / Pro Tools; slow XML import | file format | partly | 20–40 h |
| G16 | Performance reports on 0.5.0 | performance | partly | 30–60 h |
| G17 | Audio device and playback reliability | stability | partly | 20–40 h |
| G18 | Effects depth: approximations and missing behaviour | feature | no | 60–110 h |
| G19 | Speech to text off in release builds | feature, AI | no | 5–10 h |
| G20 | The web app shell (closed 2026-10-15: no browser build) | platforms | no | – |

## G1. Nothing compares our output with Premiere's

- **Missing:** a fidelity harness. Every effect, transition, Lumetri control, blend mode, keyframe
  interpolation and audio effect counts once it exists; nothing renders the same project in
  Premiere and Redrob Film and compares frames or samples. Effects described as approximations
  count the same as exact ones.
- **Evidence:** catalogue tests check names and folders only. Redrob Effects's live After Effects
  comparison (its gaps.md G1) found real bugs in features marked done on its first run.
- **Impact:** "ready for real work" stays an estimate; colour and effect differences surface only
  when a user notices.
- **Done when:** a corpus of original test projects (our own media), opened in Premiere through
  FCP7 XML / OTIO, rendered by both, with per-effect PSNR / ΔE and audio-sample scores reproducible
  from one command; Premiere output stays local in `plan/premiere/` (gitignored). Owner runs
  Premiere (installed). **40–70 h.** Doc: [target-app-parity.md](target-app-parity.md).

## G2. Premiere's `.prproj` cannot be opened

- **Status (2026-10-19, beta/g2):** import landed, not yet gate-built or measured. File ▸ Open of a
  `.prproj` makes a new unsaved project from it; File ▸ Import, drag and drop and the command line
  take it too (`redrob_film_interchange::prproj`). Read: bins, item labels, media (path, offline
  state, start timecode, stream characteristics), adjustment layers, Premiere's generators by name,
  sequences (name, rate, frame size, sample rate, start time, markers, links), video and audio
  tracks (locked, output / mute), clips (timeline in / out, source in, speed, reverse, disabled,
  clip gain, labels), clip and sequence markers, transitions (nearest of ours by name), Motion,
  Opacity, Volume and Channel Volume with keyframes, other effects where our library has one by
  that name. Everything else (graphics and titles, captions, blend modes, variable speed, Lumetri,
  track mixer inserts, …) is named in the import report: each kind in the Events panel, the
  count in the status bar.
- **Evidence:** ten projects, project versions 22–45 (Premiere CS6 to 2026, gzipped and plain),
  read by hand; their structure, counts, rates and transitions checked with a throw-away script.
  `cargo test -p redrob-film-interchange --test prproj` runs hand-written fixtures and a mutation
  fuzz; `REDROB_FILM_PRPROJ_DIR=<dir>` adds every real project in `<dir>` (sequence and clip
  counts must match the document). No sample is in the repository.
- **Left:** a green gate run; keyframe easing (read as linear / Bézier), point keyframes (no
  sample has one), reverse speed (assumed a negative `PlaybackSpeed`; no sample), blend-mode
  mapping, colour matte colour, multicam, graphics; Lumetri basic correction; track mixer state;
  export (Selection as Premiere Project). **Remaining estimate: 30–60 h.**
  Doc: [file-format-parity.md](file-format-parity.md).

## G3. 10-bit and 4:2:2 camera media needs hardware

- **Missing:** our H.264 decoder is 8-bit 4:2:0 only (`crates/h264/README.md`: no High 10, High
  4:2:2, 4:4:4, lossless, interlaced field / MBAFF); HEVC has no 4:2:2 / RExt. Most mirrorless and
  cinema cameras record exactly these (Sony XAVC S-I / XAVC HS 4:2:2 10-bit, Panasonic, Canon
  XF-AVC, Fujifilm).
- **Evidence:** #626 (Panasonic Lumix 4K 10-bit 4:2:2 MOV). On macOS VideoToolbox decodes 10-bit
  4:2:2 H.264 / HEVC on chips that support it; Windows Media Foundation and Linux decline them, and
  there is no software fallback, so the clip fails. Premiere decodes them everywhere (and in
  hardware on Apple silicon, Intel and NVIDIA Blackwell).
- **Impact:** camera originals from most current cameras do not open on Windows or Linux.
- **Estimate:** H.264 High 10 + High 4:2:2 (chroma geometry, deblocking, transforms) 15–25 h;
  HEVC Main 4:2:2 10 (RExt subset) 15–25 h. Doc: [codec-parity.md](codec-parity.md).

## G4. Crashes at start-up, and no test CI on pull requests

- **Missing:** a launch that never fails, and CI that runs the gates.
- **Evidence:** open issues: macOS 12 Monterey aborts on a missing VideoToolbox symbol (#468,
  #661, #655), Intel UHD graphics (`igvk64.dll`, #512), Windows 11 does not run or fails after setup
  (#380, #582, #413, #477), crash on new project + import (#439), "it just keeps crashing" (#687).
  Until 2026-10-15 no workflow ran `cargo test` on a pull request, so 243 PRs merged since 10-05
  were gated by contributors' local runs (main needed "green again after the batch merges" on 10-10).
  `.github/workflows/ci.yml` now runs fmt, clippy and the tests of our crates (`--profile ci`) on
  Linux; macOS and Windows runs are still missing.
- **Impact:** a user who cannot start the app gets zero value; regressions land unnoticed.
- **Done when:** weak-linked or runtime-loaded OS symbols on old macOS, a software / safe-mode
  start when the GPU adapter fails, a PR workflow running `cargo xtask ci` on macOS, Windows and
  Linux, and a crash-report triage pass. **40–80 h.**
- **Done (beta/g4, 2026-10-19; not yet run on real hardware):**
  - *Old macOS:* every VideoToolbox / CoreMedia / CoreVideo name in `crates/platform` was checked
    against Apple's availability. Only `kVTCompressionPropertyKey_ConstantBitRate` (macOS 13) is
    newer than the 11.0 deployment target, and it is already looked up with `dlsym`.
    `apps/redrob-film-app/tests/macos_symbols.rs` fails when a linked name is newer than 11.0, or
    when a new name has no recorded availability.
  - *GPU start-up:* `apps/redrob-film-app/src/graphics.rs` picks the adapter itself, on every OS.
    It takes the best one that can present to the window, logs each adapter and the reason, and
    clamps the device limits to what the adapter reports, so `request_device` no longer fails on
    older GPUs. `--safe-mode` / `REDROB_FILM_SAFE_MODE=1` starts on a software adapter, with
    DirectX 12 alone on Windows, CPU compositing and export, and no hardware decoders. Safe mode
    also turns on by itself after a start that died before its window opened. A GPU compositor
    that panics while building its pipelines (FXC, #1458) sends exports to the CPU.
  - *Audio (#1705):* when WASAPI refuses the configured stream, the app tries the mix format,
    float, stereo float / 16-bit, then 48 / 44.1 kHz. A stream that fails to start counts as one
    more failed attempt, and playback continues without sound.
  - *Crash reports:* a crash report includes the OS version and the GPU adapter. The next start
    shows where it is.
  - *CI:* `rust-desktop` runs clippy and the tests on macOS 14 and Windows 2022, with the same
    "did any Rust input change" skip. Formatting is checked on Linux only.
  - *Still open:* checks on macOS 12, Intel UHD and Windows 11 machines; the triage pass over
    crash reports (#380, #413, #439, #477, #582, #687). An adapter that crashes the driver
    during instance creation is caught only on the next start (by safe mode), not in the same one.

## G5. Hardware acceleration is partial

- **Have:** hardware decode on macOS (VideoToolbox H.264 / HEVC), Windows (Media Foundation /
  D3D11: H.264, HEVC, VP9, AV1) and Linux (VA-API and NVDEC: H.264, HEVC); hardware encode
  VideoToolbox H.264 / HEVC 8-bit, NVENC H.264 (Windows, Linux) and HEVC incl. Main 10 HDR
  (Windows); GPU compositing, blend modes, 34 effect ids incl. Lumetri basic / creative / vignette,
  GPU export rendering (#421).
- **Missing:** zero-copy decoded frames into wgpu (every frame is read back to the CPU and
  re-uploaded); VideoToolbox Main 10 / HDR HEVC encode and B-frames; ProRes hardware decode /
  encode on Apple silicon; Intel Quick Sync and AMD AMF encoders (and their 4:2:2 decode);
  VA-API VP9 / AV1 and VA-API encode; HEVC encode on Linux; AV1 hardware encode; ~48 of Premiere's
  ~82 accelerated effects (keys, masks, Lumetri curves / wheels / HSL, distortions, transitions);
  external video I/O (DeckLink / AJA, Mercury Transmit), control surfaces (EUCON / Mackie);
  a CPU fallback adapter renders everything on the CPU (#632).
- **Estimate:** 120–200 h (hardware we don't own needs a human). Doc:
  [hardware-parity.md](hardware-parity.md).

## G6. No real-world media corpus or workflow acceptance tests

- **Have:** a 106-entry manifest, `tests/corpus/manifest.toml` ([corpus.md](corpus.md)): phones
  (iPhone HLG / VFR + rotation / 240 fps, Android HDR10 / AV1 / VFR, 3GP), cameras (8-bit and
  10-bit 4:2:2 H.264, HEVC Main 10 and 4:2:2, XF-AVC MXF, AVCHD, 1080i, HDV, XDCAM, DV), ProRes
  Proxy to 4444 XQ, DNxHR LB / SQ / HQX, CineForm, FFV1, screen recordings (OBS / macOS VFR, Game
  Bar, odd sizes, MediaRecorder WebM), 17 audio-only kinds, multi-track sound, odd containers
  (fragmented MP4, edit lists, timecode, VOB, FLV, AVI, WMV, OGV, OP-Atom), stills and damaged
  files. 83 are generated by `scripts/corpus.py` with ffmpeg as a black box (each < 2 MB), 15
  large ones match the film-gap corpus, 8 are camera / phone originals a person supplies. Each
  says whether the round trip must pass, must fail with an error, or is only reported (known G3 /
  G10 gaps, open issues). `crates/engine/src/corpus_tests.rs` runs every present file through
  import → razor → move → effect → export and checks duration, frame count, audio and a clean
  decode with ffprobe / ffmpeg; `scripts/corpus.py generate` makes the small tier, and the
  harness is run by hand before a release (no scheduled CI job).
- **Missing:** a first green run (the harness has not run yet, so the `pass` / `report`
  labels are a starting guess to correct from its log); the 8 real originals and their sha256
  pins; pinned hashes for the generated tier (ffmpeg output differs between builds, so only the
  size limit is enforced); scripted MCP sessions that do real jobs end to end.
- **Evidence:** export stopped producing frames with iPhone HEVC clips on Linux (#480); WebM shows
  black (#432); only the first audio track imported (#397, #416, #601; multi-stream support landed
  in #361, reporters not yet confirmed); bitrate target produced 6.1 Mb/s for 40 (#371). Each has
  a corpus entry carrying its issue number.
- **Done when:** corpus fetched pinned and sha256-verified (a `cargo xtask corpus`),
  runs green before each release, ≥ 50 files. **20–40 h left.**

## G7. No measured parity tool in CI

- **Done:** `cargo xtask parity` measures menus (#781): our shipped menu bar (the tree the app
  builds, `examples/menu_tree.rs`) against a reference list of menu paths, by default the local
  `plan/premiere/menus.json`; presence and placement reported separately (same path, elsewhere,
  missing), app / Help / recent / account / macOS items out of scope, a markdown checklist, and
  `--min` to fail below a presence threshold. Without a reference it prints our inventory only.
- **Missing:** the other areas (commands, effects, transitions, panels, preferences, formats),
  fidelity tiers (exact, approximate, stub), a CI job, and a committed `docs/parity-checklist.md`.
- **Evidence:** the 92% menu figure in this pass came from an ad hoc label-matching script; it has
  not yet been re-measured with the tool.
- **Estimate:** 8–17 h left (the menu snapshot is local-only, so CI needs a committed, Adobe-free
  list of menu paths for `--reference`, or the check runs only where `plan/` exists).

## G8. UI fidelity: monitor handles, Effect Controls, docking

- **Missing:** on-screen transform handles for Motion / Transform in the Program monitor (#639);
  Effect Controls keyframe zoom bar, draggable divider, row lines (#640–#645); panel docking
  rearrangement (#493); clicking empty space to deselect (#683); filmstrip thumbnails (#614);
  marquee selection in the Project panel (#578); Freeform bins and middle-mouse pan (#579);
  Program monitor Button Editor (+) (#431); keyframe curves for Position (#448); transition
  editing controls (#577: Effect Controls has the transition's own timeline, drag to slide or resize;
  the A/B previews with Start / End sliders remain); track renaming (#654); window controls on macOS (#637); UI scale on
  X11 (#457).
- **Estimate:** 120–200 h. Doc: [ui-parity.md](ui-parity.md).

## G9. Windows and Linux at run time

- **Missing:** routine runtime testing off macOS. Linux playback stutter (#617), Wayland title
  bar (#433), case-sensitive extensions (#593), "Report an issue" link (#642), Windows bins cannot
  be left (#587), Sequence Settings won't open on Windows (#586, #351), old hardware (#491).
- **Estimate:** 50–90 h (overlaps G4's CI work).

## G10. Container and codec gaps on import

- **Have:** E-AC-3 (Dolby Digital Plus) in MP4, MOV, Matroska and MPEG-TS (#647): independent
  substream, 1–6 blocks per syncframe, spectral extension. MP2 (MPEG audio layer II) in MP4 and
  MOV, sample-exact against ffmpeg (#800).
- **Missing:** MP2 at bit rates ISO/IEC 11172-3 disallows: mono above 192 kbit/s (ffmpeg's default
  for mono MP2 is 384 kbit/s) and stereo below 64 kbit/s; the bootstrap decoder (symphonia, also
  0.6) rejects every such frame, so the track plays silent: needs our own layer I/II decoder.
  E-AC-3 adaptive hybrid transform, enhanced coupling and the extra channels of 7.1 dependent
  substreams (played as the 5.1 core), AVI (#598; listed as an extension but no
  demuxer), WMV / ASF, DV / DVCPRO / DV100, MPEG-4 Part 2, FLAC in MP4 (#603), camera RAW (R3D
  incl. R3D NE, ARRIRAW, Sony RAW / X-OCN, Canon RAW, BRAW, ProRes RAW; #345, #383), JPEG 2000 /
  JPEG XS MXF, Cinema DNG; stills: PSD, EXR, HEIF / HEIC, Radiance HDR, AI / EPS, and the rarer
  DPX layouts (packed 10/12-bit, run-length, Y'CbCr; Targa and DPX stills and sequences import
  since #821, deeper than 8 bits kept at 8); transparent video (#402); Sony start timecode in
  `rtmd` (#460); stereo 3D flag (#500); MKV multi-audio (#601).
- **Estimate:** 120–200 h (camera RAW alone 60–100 h, and some SDKs have licences we cannot use:
  clean-room from public specs only where specs exist). Docs: [codec-parity.md](codec-parity.md),
  [file-format-parity.md](file-format-parity.md).

## G11. No plugin hosting

- **Missing:** VST3 / Audio Units, OpenFX, Premiere / After Effects plug-in APIs, UXP panels
  (#325).
- **Impact:** many professional editors depend on third-party effects and audio plug-ins.
- **Needs:** an owner decision (FFI under the `unsafe` rule, AGENTS.md §0.3). **200–400 h.**

## G12. AI features

- **Missing:** Object Mask (Premiere 26.0, #406, #694), Generative Extend (needs a large video
  model; not reachable locally), media-intelligence search, caption translation, model-based
  Enhance Speech, Auto Reframe beyond heuristics, auto colour, Auto-Tag Audio Types.
- **Have:** local Whisper transcription (off in releases, G19), Kokoro-82M text-to-speech
  narration (#278), scene edit detection, colour match.
- **Needs:** openly licensed models we can ship. **150–300 h.**

## G13. Delivery formats

- **Missing:** AV1 export (software or hardware), MPEG-2 / DVD / Blu-ray, DCP, JPEG 2000 MXF,
  AS-10 / AS-11, XDCAM HD MXF, P2, compressed (ZIP / PIZ / DWA) OpenEXR, MP3 audio, interlaced
  encoding, smart render, HEVC software encoder (HEVC exists only where a hardware encoder does).
- **Have:** PNG, TIFF, BMP, JPEG, Targa and DPX (10-bit) image sequences (#779); OpenEXR
  sequences (half float, linear, uncompressed; #806); AAC Audio (audio-only `.m4a`, or ADTS
  `.aac`; #814); ProRes 4444 / 4444 XQ in the export UI (#342).
- **Estimate:** 60–110 h. Doc: [file-format-parity.md](file-format-parity.md).

## G14. Localization: 8 of the 12 key languages missing

- **Have:** Spanish, Japanese, Simplified Chinese and French (#498) catalogs cover ~99.6–100% of
  the ~3,500 cataloged strings; Portuguese (Brazil) and Ukrainian ~10% (menus).
- **Missing:** Hindi, Arabic (no RTL interface layout; Arabic titles reported broken, #395),
  Indonesian, German beyond menus (#490), Korean (#508; needs Hangul fonts in craft-fonts),
  Vietnamese, Portuguese beyond menus; Premiere also ships Italian and Russian. Engine errors,
  CLI and MCP stay English. No language has a recorded native-speaker review.
- **Estimate:** 150–230 h. Doc: [localization-parity.md](localization-parity.md).

## G15. Interchange never validated in Avid / Pro Tools; slow XML import

- **Missing:** AAF and OMF exports have never been opened in Media Composer or Pro Tools; FCP7 XML
  / OTIO import of a Premiere sequence took 97 s / 66 s for three media files (#461); `file.import`
  merges into existing project content in headless round trips (#592).
- **Estimate:** 20–40 h (needs a human with Avid / Pro Tools, both installed on the owner's Mac).

## G16. Performance reports on 0.5.0

- **Missing:** steady UI under load. Constant lag after 0.4 → 0.5 (#525), ~100% CPU (#523). AV1
  software decode is ~4 fps at 1080p single-threaded; 8K is not real time.
- **Done:** Render (Effects) In to Out renders only the frames between In and Out, as partial
  previews named by segment hash and relative frames, instead of every segment In/Out touches
  (#424); Render Audio still renders whole audio segments.
- **Done (0.4.0, measured on Linux, llvmpipe):** the window no longer repaints while idle (an
  empty X11/Wayland title was re-sent every frame): idle CPU 638–661 % → 0–18 %; 1080p30 playback
  317 → 597 of 600 frames, 0 dropped; playhead drag 68 / 104 → 63 / 82 ms p50 / p95
  ([gap/kdenlive-report.md](gap/kdenlive-report.md)). Not yet measured on Windows or macOS.
- **Estimate:** 30–60 h. Doc: [performance.md](performance.md).

## G17. Audio device and playback reliability

- **Evidence:** Asus Xonar DX output broken (#605), no audio (#602), audio cuts out with
  overlapping clips (#336), buggy audio (#410), Linux stutter (#617). No ASIO; cpal only.
- **Done (0.4.0):** a lost or stalled output is reopened during playback (up to 4 tries per play);
  a silent keep-alive stream stops the sound server from suspending the output between plays
  (first picture after Space 865–890 → 197 ms on a suspended PulseAudio sink); live playback
  dropouts 2 → 0 in 30 s. Measured on Linux only.
- **Estimate:** 20–40 h.

## G18. Effects depth: approximations and missing behaviour

- **Missing:** Warp Stabilizer is 2-D (Premiere's is 3-D subspace), Morph Cut has no face tracking
  (and occlusion edges are soft), Lumetri HDR-aware maths, multicam paging beyond 16 angles,
  redesigned 26.0 mask tools (constrained lines; an on-monitor corner-roundness handle).
- **Have:** Optical Flow time interpolation synthesises motion-compensated in-between frames (dense
  coarse-to-fine Lucas–Kanade, CPU; #780); Morph Cut morphs the subject along that optical flow
  instead of cross-dissolving (CPU; #805); Transform shutter angle motion blur (#415); Ultra Key
  Setting applied as a parameter preset (#458); rectangle and polygon masks round their corners
  with circular fillets (`masks.add` `cornerRadius`, `masks.roundCorners`, every path keyframe;
  #813).
- **Estimate:** 60–110 h.

## G19. Speech to text off in release builds

- **Missing:** the `whisper` feature is off by default and in releases, so Transcribe Sequence,
  text-based editing and auto captions need a source build.
- **Estimate:** 5–10 h (model download UX, licence check).

## G20. The web app shell

- **Closed (2026-10-15):** Redrob Film does not ship a browser build. `apps/redrob-film-web` and the web
  packaging were removed with the release trim to three desktop targets ([releasing.md](releasing.md)).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-10 | major | Created from ROADMAP.md's "Where we are lacking" (2026-10-05); re-ranked against Premiere 26.5.2 and the 188 open issues; added `.prproj`, 10-bit / 4:2:2 camera decode, launch crashes and missing PR CI, localization, delivery formats |
| 2026-10-19 | minor | 0.4.0: G16 idle repaint fix and G17 audio reopen / keep-alive recorded with measured numbers (Linux); G20 marked closed in the table; Redrob Film → Redrob Film in the prose |
| 2026-10-19 | minor | G6: real-world media corpus manifest (106 entries), generator, engine acceptance harness (run by hand); left: a green run, real originals, pins |
| 2026-10-19 | minor | G2: `.prproj` import (open, import, drag and drop, CLI) with an import report; tests with hand-written fixtures and an opt-in real-project directory; left: gate run, easing, blend modes, graphics, export |
