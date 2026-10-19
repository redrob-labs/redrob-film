# Changelog

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
