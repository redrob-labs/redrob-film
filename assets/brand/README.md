# Brand sources

Verbatim copies of shipped Redrob brand files. `SHA256SUMS` pins them; `scripts/check_brand.py`
fails if any changes. `packaging/brand-assets.sh` derives everything under `assets/app-icon/` from
these and nothing else.

| File | Use |
|---|---|
| `redrob-film-icon-web.svg` | the Redrob Film app icon, rounded tile with transparent corners (`icon-web` from `@redrob-labs/ui` 1.8.0, `dist/brand/app-icons/redrob-film/`, sha256 in its `manifest.json`) |
| `redrob-film-icon-web-1024.png` | the same SVG rendered at 1024 px by Chromium; every PNG app icon is this resized down |
| `redrob-logo-primary-white.png` | the lockup on dark themes (About, Home) |
| `redrob-logo-primary.png` | the lockup on the light theme |

Rules that bind here (design system `10-logo.md`, `assets/Logos/README.md`):

- Never redrawn, recolored, re-proportioned, traced, given a shadow, border or second radius. Pick
  the variant that matches the ground; never tint or fade a mark.
- Desktop icons (window, Dock, taskbar, .ico, .icns, Linux theme, the launcher PNG) use the rounded
  `icon-web` variant, because Windows, Linux and Redrob Launcher show the PNG unmasked.
- Render the SVG with Chromium (or `rsvg-convert`), never ImageMagick's built-in SVG reader: it
  drops the gradients. Every size is the 1024 render resized down; nothing is upscaled.
- The lockup is at least 18 px tall on screen, with its clear space (0.5x above and below, 0.3x left
  and right, x = symbol height). The product name follows it at the UI's 600 weight, never inside
  the lockup's letterforms.

To update the icon: copy the new SVG from `@redrob-labs/ui`, render it at 1024 px with Chromium,
update `SHA256SUMS` and rerun `packaging/brand-assets.sh`; nothing else changes.

Licence: the Redrob name and marks are trademarks of Redrob (McKinley Rice, Inc.), not covered by
this repository's MIT OR Apache-2.0 licence. A fork that is not Redrob's must remove them.
