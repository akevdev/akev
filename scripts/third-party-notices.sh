#!/usr/bin/env bash
# Write THIRD-PARTY-NOTICES.md: the licences of everything Akev ships, for the release archives
# and app bundles.
#
#   scripts/third-party-notices.sh [output]      (default: THIRD-PARTY-NOTICES.md)
#
# Needs cargo-about (`cargo install cargo-about --features cli --locked`) and the UI's
# node_modules (`cd ui && bun install`).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$root/THIRD-PARTY-NOTICES.md}
cd "$root"

# Rust crates (all features: official builds include TLS impersonation).
cargo about generate --workspace --all-features -c packaging/notices/about.toml packaging/notices/about.hbs -o "$out"

# BoringSSL is C code inside boring-sys2: its own licence isn't in the crate metadata.
boring=$(cargo metadata --format-version 1 --all-features 2>/dev/null |
  python3 -c 'import json,sys; m=json.load(sys.stdin); print(next((p["manifest_path"] for p in m["packages"] if p["name"]=="boring-sys2"), ""))')
if [ -n "$boring" ]; then
  lic=$(dirname "$boring")/deps/boringssl/LICENSE
  if [ -f "$lic" ]; then
    { printf '\n---\n\n## BoringSSL (TLS fingerprint impersonation)\n\n```\n'; cat "$lic"; printf '```\n'; } >> "$out"
  fi
fi

# The web UI's npm packages (production dependencies and what they pull in), fonts included.
python3 - "$root/ui" >> "$out" <<'PY'
import json, os, sys
ui = sys.argv[1]
nm = os.path.join(ui, "node_modules")
pkg = json.load(open(os.path.join(ui, "package.json")))
seen, todo, rows = set(), list(pkg.get("dependencies", {})), []
while todo:
    name = todo.pop()
    if name in seen:
        continue
    seen.add(name)
    d = os.path.join(nm, name)
    try:
        meta = json.load(open(os.path.join(d, "package.json")))
    except OSError:
        continue
    todo.extend(meta.get("dependencies", {}))
    lic = meta.get("license") or (meta.get("licenses") or [{}])[0].get("type") or "see package"
    text = ""
    for f in sorted(os.listdir(d)):
        if f.upper().startswith(("LICENSE", "LICENCE", "COPYING", "OFL")):
            try:
                text = open(os.path.join(d, f), encoding="utf-8", errors="replace").read().strip()
            except OSError:
                pass
            break
    rows.append((name, meta.get("version", "?"), lic, text))
print("\n---\n\n## Web UI (npm packages)\n")
for name, version, lic, _ in sorted(rows):
    print(f"- {name} {version}: {lic}")
for name, version, lic, text in sorted(rows):
    if text:
        print(f"\n### {name} {version} ({lic})\n\n```\n{text}\n```")
PY
echo "wrote $out"
