#!/usr/bin/env bash
# generate.sh — rasterise SVG masters into PNG and ICO variants.
#
# Usage: ./generate.sh [SVG...]
#   With no arguments, processes every *.svg in this directory.
#
# Output:
#   png/<base>-<size>.png  for each size in SIZES
#   <base>.ico             from the 32x32 PNG

set -euo pipefail
cd "$(dirname "$0")"

SIZES=(16 32 48 64 128 256 512)

mkdir -p png

if [[ $# -gt 0 ]]; then
    svgs=("$@")
else
    svgs=(*.svg)
fi

for svg in "${svgs[@]}"; do
    [[ -f "$svg" ]] || { echo "Not found: $svg" >&2; continue; }
    base="${svg%.svg}"
    echo "==> $svg"

    for size in "${SIZES[@]}"; do
        out="png/${base}-${size}.png"
        magick -background none -size "${size}x${size}" "SVG:${svg}" "$out"
        echo "    ${size}x${size} → $out"
    done

    magick "png/${base}-32.png" "${base}.ico"
    echo "    ico       → ${base}.ico"
done
