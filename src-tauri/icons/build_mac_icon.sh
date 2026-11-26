#!/bin/bash
# Generate icons/icon.icns from the existing PNGs in this directory.
# Run ONCE on the Mac, from src-tauri/icons/:
#     bash build_mac_icon.sh
# Requires the built-in macOS tools `iconutil` and `sips` (no extra deps).
# Produces icon.icns which the Tauri macOS bundler references from
# tauri.conf.json bundle.icon. Re-run only if the source PNGs change.

set -euo pipefail
cd "$(dirname "$0")"

ICONSET="icons.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"

# Helper: resample a source PNG to the requested size and drop into the
# iconset under the filename that `iconutil -c icns` expects.
emit() {
  local src="$1" dst="$2" size="$3"
  if [ -f "$src" ]; then
    sips -s format png -z "$size" "$size" "$src" --out "$ICONSET/$dst" >/dev/null
  else
    echo "warn: missing $src (skipping $dst)" >&2
  fi
}

# iconutil's expected filenames + sizes:
emit icon.png                icon_16x16.png        16
emit icon.png                icon_32x32.png        32
emit icon.png                icon_128x128.png      128
emit 128x128@2x.png          icon_128x128@2x.png  256
emit icon.png                icon_256x256.png      256
emit icon.png                icon_512x512.png      512
# @2x 1024x1024 — only if icon.png happens to be that size; otherwise skip
# (iconutil accepts missing entries, just downscales in the bundle).

iconutil -c icns "$ICONSET" -o icon.icns
rm -rf "$ICONSET"
echo "Built icon.icns ($(stat -f "%z" icon.icns) bytes). Tauri will pick it up on next \`npm run tauri build\` macos run."