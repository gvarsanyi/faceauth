#!/usr/bin/env bash
# generate.sh — rasterise faceauth.svg into PNG and ICO variants.
#
# Output:
#   png/faceauth-<size>.png  for each size in SIZES
#   faceauth.ico             from the 32x32 PNG

set -euo pipefail
cd "$(dirname "$0")"

SIZES=(16 32 48 64 128 256 512)

mkdir -p png

svg=faceauth.svg
base=faceauth
echo "==> $svg"

for size in "${SIZES[@]}"; do
    out="png/${base}-${size}.png"
    magick -background none -size "${size}x${size}" "SVG:${svg}" "$out"
    echo "    ${size}x${size} -> $out"
done

magick "png/${base}-32.png" "${base}.ico"
echo "    ico       -> ${base}.ico"
