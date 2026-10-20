# Redrob Film app icon

The Redrob Film app icon (`icon-web`, a rounded tile with transparent corners) from
`@redrob-labs/ui` 1.8.0. Every file here is derived from the pinned sources in
[`../brand/`](../brand/README.md) by `packaging/brand-assets.sh` and nothing else; do not edit them
by hand.

## Files

| File | What |
|---|---|
| `hicolor/<size>/apps/ai.redrob.film.png`, `hicolor/scalable/apps/ai.redrob.film.svg` | Linux icon theme, used by `packaging/linux/ai.redrob.film.desktop` |
| `redrob-film-256.png` | runtime window icon on Windows and Linux (embedded by `apps/redrob-film-app/src/main.rs`) |
| `redrob-film-512.png` | runtime Dock icon on macOS, and the release's launcher icon (`redrob-film.png`) |
| `redrob-film.icns` | macOS bundle icon (`CFBundleIconFile`) |
| `redrob-film.ico` | Windows icon, 16-256 px (embedded in the exe by `apps/redrob-film-app/build.rs`) |

## Rules

Never redrawn, recolored, re-proportioned, given a shadow, border, margin or second radius. Each
size is the 1024 px Chromium render resized down, never up.

Licence: [LICENSE.txt](LICENSE.txt).
