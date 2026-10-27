# Redrob Film

Redrob Film is an agentic video editor: describe the cut and let the agent make it. One window, two
modes: **Edit** (the timeline NLE) and **Effects** (motion graphics and compositing), with a shared
AI dock beside whichever one is active.

한국어: [README.ko.md](./README.ko.md)

> **Origins.** Redrob Film started from open-source code and is developed here on its own; it is not
> synced with that code any more. Where it came from, and which old names are still read from existing
> files, is in [SOURCE_ORIGINS.md](./SOURCE_ORIGINS.md). The original copyright is retained in
> [NOTICE](./NOTICE) with ours added below it. The Redrob marks in `assets/brand/` are Redrob's trademarks.

## What it is

- Edit mode: a non-linear video editor in pure Rust, with its own H.264, HEVC, ProRes and AAC codecs and a GPU
  compositor that works in linear light.
- Effects mode: motion graphics and compositing (layers, keyframes, effects, expressions, text) in the
  same window and the same project bundle.
- Mode menu hand-offs: **Send Selection to Effects** turns the selected clips (or the in/out range, or
  the whole sequence) into a new composition; **Place Composition on Timeline** puts the active
  composition on the timeline as a clip rendered live by the Effects renderer. Edit / Effects switch
  with the top-bar buttons or their shortcuts.
- Every action in the app is a command. The UI, the CLI, the JSON control channel and the MCP server
  call the same commands, so an agent cuts, trims, grades, mixes and exports exactly as a person does.
- **Redrob Code is built in.** The AI dock (the AI button in the top bar, Mode ▸ Show AI, Ctrl+Shift+A)
  is a chat with Redrob Code, which edits the open project through those commands in both modes. Its
  model calls are billed to the Redrob workspace you connect (Help ▸ Connect to Redrob…); editing
  itself is free and never asks for an account. Other agents connect through `redrob-film-cli mcp`
  ([docs/agents.md](./docs/agents.md)).
- Native on macOS, Windows and Linux.

## Download

The easiest way is [Redrob Launcher](https://github.com/redrob-labs/redrob-launcher), which installs
Redrob Film, keeps it up to date and signs every Redrob app in with one workspace. Direct downloads
are on the [releases page](https://github.com/redrob-labs/redrob-film/releases); a copy installed by
hand checks for updates itself (Updates menu in the top bar; `REDROB_FILM_NO_UPDATE_CHECK=1` turns
the automatic check off).

| Platform | Download |
|---|---|
| Linux x86_64 | `redrob-film-<version>-linux-x86_64.tar.gz` (run `./install.sh`, no sudo needed) or `redrob-film-<version>-linux-x86_64.deb` |
| macOS (Apple silicon and Intel) | `redrob-film-<version>-macos-universal.dmg` |
| Windows x64 | `redrob-film-<version>-windows-x64.msi` or `redrob-film-<version>-windows-x64-portable.zip` |

`SHA256SUMS.txt` on the release lists every file. Details: [docs/releasing.md](./docs/releasing.md).

## Getting started

```sh
cargo run --release -p redrob-film-app                     # the desktop app (redrob-film), demo project
cargo run --release -p redrob-film-app -- --control 9876   # with the JSON-lines control server
cargo run --release -p redrob-film-cli -- commands     # every engine command (redrob-film-cli)
cargo run --release -p redrob-film-cli -- mcp          # MCP server (headless)
```

The Cargo packages are `redrob-film-*` (Edit mode, the app and the CLI) and `redrob-effects-*` (the
Effects editor, `editors/effects/`). There is one product executable, `redrob-film`, plus the
`redrob-film-cli` command-line tool; the app id is `ai.redrob.film`.

## Control from Redrob Code

While the desktop app is open it registers itself with the shared Redrob Code daemon, so the agent in any Redrob surface (a phone chat included) can drive the open project through the `app:film` tools:

- `project_info` - the project's file path, the active sequence's duration, frame rate, resolution, track and clip counts.
- `open_project` - open a project file (`.rfilm`) and describe it.
- `list_clips` - the sequence's clips (optionally one track): id, track, name, timeline in/out and source in/out in seconds.
- `trim_clip` - move a clip's in and/or out point to new timeline positions in seconds.
- `split_clip` - cut a clip in two at a timeline position; answers the ids of the pieces.
- `set_playhead` - move the playhead to a timeline position in seconds.
- `render` - start the app's own export job for the sequence (optional output path and preset); answers the job id and output path.
- `render_status` - a render job's progress, and the written path once it has finished.

The tools run against the live editor, exactly as the menus do; the app works as usual when no daemon is running.

## Branches and contributing

- Default branch `develop`, release branch `main`. Branch names follow the `feat/ fix/ chore/ docs/
  test/ refactor/ perf/ sync/ hotfix/ release/` prefixes, checked by the shared redrob-labs gitflow workflow (one type list for every repository: `feat fix chore docs test refactor perf ci build sync hotfix release`).
- Organisation conventions: CONTRIBUTING.md and FORKS.md in [redrob-labs/.github](https://github.com/redrob-labs/.github).
- Gates: `python3 scripts/check_brand.py` and `cargo xtask ci`.

## License

MIT or Apache-2.0, at your option ([LICENSE-MIT](./LICENSE-MIT), [LICENSE-APACHE](./LICENSE-APACHE)).
Copyright notices are in [NOTICE](./NOTICE); ours is added below the original authors':

```
Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
Copyright (c) 2026-present Janghoon Lee (Redrob) and contributors
```

The Redrob name and marks are not covered by that license ([assets/brand/README.md](./assets/brand/README.md)).
Per-asset attribution is in [ATTRIBUTION.md](./ATTRIBUTION.md).
