#!/bin/sh
# Rasterises the brand SVGs: the UI's PNG favicons (ui/public), the desktop app's icons
# (desktop/src-tauri/icons) and the Windows installer art (desktop/src-tauri/installer,
# from installer/*.html here). Run `bun brand/build.ts` first if the mark changed (`make brand`
# does both). Needs Chromium, ImageMagick, Python with Pillow (for the .icns) and the UI's
# dependencies (`cd ui && bun install`, for the installer's Inter font).
#
#   brand/render.sh            # everything
#   brand/render.sh favicons   # only ui/public/favicon-32.png and apple-touch-icon.png
set -eu
cd "$(dirname "$0")"
desktop=../desktop/src-tauri
icons=$desktop/icons
ui=$(cd ../ui/public && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
chrome=$(command -v chromium-browser || command -v chromium || command -v google-chrome)

# shot <html file> <width> <height> <png out> [background]
shot() {
  "$chrome" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
    --allow-file-access-from-files ${5:+--default-background-color=$5} --window-size="$2,$3" \
    --screenshot="$4" "file://$1" >/dev/null 2>&1
}

# UI favicon fallbacks next to favicon.svg: a 32px PNG for browsers without SVG favicons, and
# the iOS home-screen icon (opaque: iOS fills transparency with black and rounds the corners).
printf '<!doctype html><style>html,body{margin:0;background:transparent}img{display:block;width:512px;height:512px}</style><img src="file://%s/favicon.svg">' "$ui" > "$tmp/favicon.html"
shot "$tmp/favicon.html" 512 512 "$tmp/favicon.png" 00000000
magick "$tmp/favicon.png" -filter Lanczos -resize 32x32 -strip "PNG32:$ui/favicon-32.png"
magick "$tmp/favicon.png" -filter Lanczos -resize 180x180 -background '#0d0e12' -flatten -strip "PNG24:$ui/apple-touch-icon.png"
[ "${1:-all}" = favicons ] && exit 0

# Desktop icons: the PNGs tauri.conf.json lists, plus the Windows .ico and macOS .icns. The other
# sizes those two are made from stay in $tmp.
printf '<!doctype html><style>html,body{margin:0;background:transparent}img{display:block;width:1024px;height:1024px}</style><img src="file://%s/akev-icon.svg">' "$PWD" > "$tmp/icon.html"
shot "$tmp/icon.html" 1024 1024 "$tmp/1024.png" 00000000
png() { magick "$tmp/1024.png" -filter Lanczos -resize "$1x$1" -strip "PNG32:$2"; }
for s in 16 24 48 64 256 1024; do png "$s" "$tmp/$s-icon.png"; done
png 32 "$icons/32x32.png"
png 128 "$icons/128x128.png"
cp "$tmp/256-icon.png" "$icons/128x128@2x.png"
magick "$tmp/16-icon.png" "$tmp/24-icon.png" "$icons/32x32.png" "$tmp/48-icon.png" "$tmp/64-icon.png" \
  "$icons/128x128.png" "$tmp/256-icon.png" "$icons/icon.ico"
python3 -c "
import sys
from PIL import Image
Image.open(sys.argv[1]).save(sys.argv[2], format='ICNS')
" "$tmp/1024-icon.png" "$icons/icon.icns"

# Windows installer art: NSIS wants uncompressed 24-bit BMPs (sidebar 164x314, header 150x57).
for spec in sidebar:164:314 header:150:57; do
  name=${spec%%:*}
  size=${spec#*:}
  shot "$PWD/installer/$name.html" "${size%:*}" "${size#*:}" "$tmp/$name.png"
  magick "$tmp/$name.png" -alpha off -type truecolor "BMP3:$desktop/installer/$name.bmp"
done
