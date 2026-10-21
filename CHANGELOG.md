# Changelog

## 0.6.1 -- 2026-10-21

### Fixed

- Window placement (Windows, Linux): a window bigger than its screen's work area is maximized into
  it on the first frame in Edit and Effects alike, so its title bar can be reached; it opened at
  1600 × 980 and was fitted only once the Effects editor was shown. The window opens centred.
- Dragging the title bar no longer leaves the window stuck dragging (scrolling stopped until the
  next click); only the primary button moves the window.

## 0.6.0 -- 2026-10-20

### Changed

- App icon: the 2026 Redrob Film icon from `@redrob-labs/ui` 1.8.0 (rounded tile, transparent
  corners) on the window, Dock, taskbar, `.ico`, `.icns`, the Linux icon theme and the
  `redrob-film.png` release asset that Redrob Launcher shows.
- Release builds run on larger runners with `codegen-units = 16`; pull-request CI skips the Rust
  jobs on docs-, version- and profile-only changes.

## 0.5.0 -- 2026-10-19

### Fixed

- Playhead (Windows): pressing on the timeline opened a new WASAPI stream for scrub audio on the
  UI thread, so the needle sat still for ~3 s on the first drag. Scrub audio now goes through the
  output that is kept open between plays. The needle is drawn after input in the same frame.
- Play start: Space no longer waits for the audio device clock and a 6-frame preroll. The picture
  starts at once on the wall clock and the sound joins when the device runs; preroll is 3 frames,
  at most 0.15 s.
- Start-up: the GPU adapter is chosen explicitly and falls back to the CPU compositor; `--safe-mode`
  starts with the CPU compositor and default layout, and is used automatically after a crash during
  the previous start; WASAPI tries 7 output formats before giving up; a crash leaves a notice file
  the next start reports.

### Added

- Premiere Pro projects (`.prproj`) open from File ▸ Open and File ▸ Import, by drag and drop and on
  the command line: gzip or plain XML, CS6 to Premiere 2026. Sequences, tracks, clips, speed,
  links, markers, bins, labels, transitions, Motion, Opacity and Volume; anything not carried over
  is listed in the Events panel. Not yet: Lumetri, graphics and titles, multicam, mixer state.
- 10-bit and 4:2:2 camera media without hardware decoding: H.264 High 10 and High 4:2:2 (8 and
  10-bit), HEVC Main 4:2:2 10.
- Real-world media corpus: `tests/corpus/manifest.toml` (106 entries), `scripts/corpus.py`
  generator and an engine acceptance harness (import, cut, move, effect, export, ffprobe checks),
  run by hand before a release.

## 0.4.0 -- 2026-10-19

### Fixed

- Idle repaint (Linux): winit reports an empty window title on X11 and Wayland; the shell took it
  for a title set by someone else and re-sent its own every frame, and every viewport command
  asks for another frame. The window never went idle: ~125 fps and 6 CPU cores with nothing
  happening on a software rasteriser, which starved decoding (45 % of 1080p30 frames dropped in
  playback) and made the playhead trail the mouse. An empty title now counts as unknown; idle CPU
  drops to near zero.
- Audio: an output that is lost during playback (stream error, stalled or unplugged device) is
  opened again (up to 4 tries per play) while playback continues on the wall clock; a reopened
  device takes the clock back only once it actually consumes samples.
- Audio (Linux, Windows): a silent stream keeps the output awake between plays, so a sound server
  that suspended the idle device no longer holds the first picture of a play for ~0.9 s.
- Previews: render previews stay ProRes 422 HQ by default; the lighter flavours are explicit
  choices in Sequence Settings.

### Added

- Kdenlive parity, written clean-room from the Kdenlive manual and observed behaviour (no GPL
  source was read): 1,202 user-facing features of Kdenlive 26.08.2 inventoried in
  `docs/parity/kdenlive-*.toml`; 1,046 are now fully available (538 before), 126 partly, 30 not
  (each with the reason). Highlights: audio effects (23 new) and the mixer, voice-over pause and
  monitoring; timeline track heights, waveform zoom, group selection, insert mode, sequence tabs;
  stylize, noise, colour, keying, mask, VR and transform video effects; effect stack, effect zones,
  auto-keyframes, title templates and the title typewriter; monitor loop zone, jog shuttle,
  full-screen and undocked monitors, grids and overlays; scopes, still-image proxies and
  multi-job proxies, preview codecs; markers, subtitles and speech to text; bin slideshows,
  generators and clip properties; render presets (PPM, WebP, FLAC), archived projects and backups;
  preferences, project settings and workspaces.

## 0.3.0 -- 2026-10-17

### Added

- Control from Redrob Code: while the desktop app is open it registers itself with the shared Redrob
  Code daemon through the app bridge (redrob-code v0.7.0 protocol), so the agent in any Redrob
  surface can drive the open project through the `app:film` tools -- `project_info`, `open_project`,
  `list_clips`, `trim_clip`, `split_clip`, `set_playhead`, `render` and `render_status`. The tools
  run against the live editor, exactly as the menus do; the app works as usual when no daemon is
  running.
- Release: the 512 px app icon ships as `redrob-film.png`, named in `redrob-launcher.json` so Redrob
  Launcher's catalog shows this release's own logo.

## 0.2.2 -- 2026-10-16

### Fixed

- Timeline: pressing the ruler or the playhead seeks at once and the playhead follows the mouse from
  the press on, also when the pointer leaves the ruler or the handle; the playhead has a 12 px
  handle over the full height of the tracks (Selection tool) with a resize cursor.
- Audio: the output stream is opened with the configured settings, then without the fixed buffer
  size, then with the device's own default format, rate and channels, so a device whose mix format
  differs from the settings (Windows WASAPI) no longer stays silent; an output that cannot be
  opened or stops with an error says so in the status bar, and the device and its default
  configuration are logged at start-up.
- Account: a sign-in in Redrob Launcher (the shared account file) is used ahead of this app's own
  key, so signing in once in the launcher signs Redrob Film in as well.

## 0.2.1 -- 2026-10-16

### Fixed

- Switching Edit / Effects no longer freezes the window: the merged font set is built once and no
  longer rebuilt on every switch, and the Effects editor's system-font scan runs in the background
  at start-up instead of on the UI thread.
- Dragging the playhead follows the mouse: the frame already being decoded for the view is left to
  finish instead of being cancelled by the next drag event, so scrubbing shows frames while moving.
- Audio plays: the output-stall watchdog gives a device that filled its first buffer four seconds to
  start consuming (devices commonly wake 1.5-2 s after the stream opens) instead of muting after one.
- The AI dock (Mode > AI) can be resized by dragging its edge and remembers its width for the
  session; its header no longer runs under the close button.
- Help menu lists only live links; About > Contributors is built from this repository's history and
  the Models tab is gone.

## 0.2.0 -- 2026-10-16

### Changed

- Upstream identifiers are gone from everything a user or contributor meets. Crates are
  `redrob-film-*` (`redrob-film-app` is the desktop shell, its binary still `redrob-film`) and
  `redrob-effects-*`; `tools/rename_crates.py` performs the rename idempotently and
  `tools/merge_effectcraft.py` runs it, together with the new `tools/prune_upstream.py` (drops the
  effectcraft subtree's README, packaging, CI, apps, xtask and other repository furniture), after
  every subtree pull.
- Projects save as `.rfilm` (effects sidecar `.rfilm.effects.json`); effects projects as `.rfx`,
  XML copies as `.rfxx` with root `<RedrobEffectsProject>`. MIME types are
  `application/x-redrob-film` / `application/x-redrob-effects` (declared for macOS and Linux);
  MCP resources are `redrob-film://…` and `redrob-effects://…`; environment variables are
  `REDROB_FILM_*` / `REDROB_EFFECTS_*`; MKV files carry `WritingApp` `redrob-film`.
- Old files and settings keep working: `.fcproj`, `.ecproj`, `.ecprojx`, `<EffectCraftProject>`
  documents, template archives with `project.ecproj`, pre-rename recovery snapshots and auto-saves,
  the old MCP URIs, and `FILMCRAFT_DATA_DIR`, `FILMCRAFT_CONFIG_DIR`, `FILMCRAFT_CONTROL_PORT`,
  `FILMCRAFT_CPU_COMPOSITE`, `EFFECTCRAFT_CONFIG_DIR`, `EFFECTCRAFT_FONT_DIRS` are read as
  fallbacks. Nothing writes the old spelling.
- `scripts/check_brand.py` now fails on any upstream crate name, extension, env prefix, URI scheme,
  MIME type or XML root outside the attribution files and the `// legacy:`-marked read paths.
