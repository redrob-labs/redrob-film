#!/usr/bin/env bash
# Derive every app icon from the pinned design-system sources in assets/brand/ and nothing else.
# Needs ImageMagick 7 (`magick`), python3 and cargo. The outputs are committed, so builds and
# packaging never need these tools.
#
#   packaging/brand-assets.sh
#
# Source: the Redrob Film `icon-web` app icon from @redrob-labs/ui 1.8.0 (rounded tile, transparent
# corners) and its 1024 px Chromium render (assets/brand/README.md has the rules). Every size is the
# 1024 render resized DOWN, never up; the SVG is used as is for the scalable Linux icon.
#   hicolor/{16..512}, redrob-film-{256,512}.png   the 1024 render resized down
#   hicolor/scalable                               redrob-film-icon-web.svg, as is
#   redrob-film.ico                                16, 24, 32, 48, 64, 128, 256
#   redrob-film.icns                               16, 32, 64, 128, 256, 512, 1024
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
B="$ROOT/assets/brand"
A="$ROOT/assets/app-icon"
ID="ai.redrob.film"
SRC="$B/redrob-film-icon-web-1024.png"
command -v magick >/dev/null || { echo "error: magick (ImageMagick 7) not found" >&2; exit 1; }

(cd "$B" && sha256sum -c --quiet SHA256SUMS)

down() { magick "$SRC" -filter Lanczos -resize "$1x$1" "PNG32:$2"; }

# Keep the hand-written licence/readme; everything else here is regenerated.
find "$A" -mindepth 1 ! -name LICENSE.txt ! -name README.md -delete
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$A/hicolor/${s}x${s}/apps"
  down "$s" "$A/hicolor/${s}x${s}/apps/$ID.png"
done
mkdir -p "$A/hicolor/scalable/apps"
cp "$B/redrob-film-icon-web.svg" "$A/hicolor/scalable/apps/$ID.svg"

H="$A/hicolor"
cp "$H/256x256/apps/$ID.png" "$A/redrob-film-256.png"
cp "$H/512x512/apps/$ID.png" "$A/redrob-film-512.png"

(cd "$ROOT" && cargo run -q -p xtask -- ico "$A/redrob-film.ico" \
  "$H/16x16/apps/$ID.png" "$H/24x24/apps/$ID.png" "$H/32x32/apps/$ID.png" "$H/48x48/apps/$ID.png" \
  "$H/64x64/apps/$ID.png" "$H/128x128/apps/$ID.png" "$A/redrob-film-256.png")

python3 "$ROOT/packaging/make-icns.py" "$A/redrob-film.icns" \
  16="$H/16x16/apps/$ID.png" 32="$H/32x32/apps/$ID.png" 64="$H/64x64/apps/$ID.png" \
  128="$H/128x128/apps/$ID.png" 256="$A/redrob-film-256.png" 512="$A/redrob-film-512.png" 1024="$SRC"

# Every derived file gets an attribution sidecar (cargo xtask assets).
python3 "$ROOT/packaging/brand-sidecars.py"
echo "brand assets written to $A"
