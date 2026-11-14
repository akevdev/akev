#!/usr/bin/env bash
# Render the package-manager manifests in packaging/ for a release.
#
#   scripts/package-manifests.sh <version> <dist-dir> <out-dir>
#
# <dist-dir> holds the release files (as the release workflow collects them). Placeholders:
# @VERSION@, @PKGVER@ (version with - as _, for pacman), @DATE@ (UTC, YYYY-MM-DD) and
# @SHA:<file name>@ (that file's SHA-256 from <dist-dir>; the file name may itself contain
# @VERSION@). A missing file fails the render: a manifest must never point at nothing.
set -euo pipefail

version=${1:?usage: scripts/package-manifests.sh <version> <dist-dir> <out-dir>}
dist=${2:?dist dir}
out=${3:?out dir}
root=$(cd "$(dirname "$0")/.." && pwd)
date=$(date -u +%Y-%m-%d)
pkgver=${version//-/_}

mkdir -p "$out"
status=0
while IFS= read -r -d '' src; do
  rel=${src#"$root/packaging/"}
  dest="$out/$rel"
  mkdir -p "$(dirname "$dest")"
  text=$(sed -e "s/@VERSION@/$version/g" -e "s/@PKGVER@/$pkgver/g" -e "s/@DATE@/$date/g" "$src")
  for file in $(grep -o '@SHA:[^@]*@' <<<"$text" | sort -u | sed -e 's/^@SHA://' -e 's/@$//'); do
    if [ ! -f "$dist/$file" ]; then
      echo "missing $file (needed by packaging/$rel)" >&2
      status=1
      continue
    fi
    sum=$(sha256sum "$dist/$file" | cut -d' ' -f1)
    text=${text//"@SHA:$file@"/$sum}
  done
  printf '%s\n' "$text" > "$dest"
done < <(find "$root/packaging" -type f ! -name README.md -print0)
exit $status
