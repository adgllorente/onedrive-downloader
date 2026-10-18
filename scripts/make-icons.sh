#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
SRC=assets/icon-source.png
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

magick "$SRC" -alpha extract "$WORK/alpha.png"
magick "$WORK/alpha.png" -threshold 3% \
  -define connected-components:area-threshold=100000 \
  -define connected-components:mean-color=true \
  -connected-components 8 "$WORK/mask.png"
magick "$WORK/alpha.png" "$WORK/mask.png" -compose Multiply -composite "$WORK/alpha_clean.png"
magick "$SRC" "$WORK/alpha_clean.png" -alpha off -compose CopyOpacity -composite -trim +repage "$WORK/clean.png"

magick "$WORK/clean.png" -resize 860x860 -background none -gravity center -extent 1024x1024 "$WORK/mac-1024.png"
magick "$WORK/clean.png" -resize 244x244 -background none -gravity center -extent 256x256 assets/icon-256.png
magick assets/icon-256.png -define icon:auto-resize=256,128,64,48,32,16 assets/icon.ico

ICONSET="$WORK/AppIcon.iconset"
mkdir "$ICONSET"
for size in 16 32 128 256 512; do
  magick "$WORK/mac-1024.png" -resize "${size}x${size}" "$ICONSET/icon_${size}x${size}.png"
  magick "$WORK/mac-1024.png" -resize "$((size * 2))x$((size * 2))" "$ICONSET/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$ICONSET" -o assets/AppIcon.icns

echo "Iconos generados en assets/: AppIcon.icns, icon.ico, icon-256.png"
