#!/usr/bin/env bash
# Bump Akev's version everywhere, commit, and create the release tag.
#
#   scripts/release.sh 0.2.0          # or 0.2.0-1 for a pre-release
#   git push origin HEAD v0.2.0       # starts .github/workflows/release.yml
#
# The version lives in Cargo.toml ([workspace.package], shared by every crate), the desktop
# app's tauri.conf.json and ui/package.json; the release workflow refuses to run if they or the
# tag disagree. Pre-release suffixes must be a plain number (0.2.0-1, not 0.2.0-beta.1): Windows
# MSI versions are numeric only.
#
# It also stamps today's date (UTC) as the build's release date (`RELEASE_DATE` in
# crates/akev-core/src/lib.rs): an Akev Pro licence covers versions released until its
# updates-until date.
#
# Before tagging it:
#   * moves the "## Unreleased" notes in CHANGELOG.md under the new version (there must be some);
#   * regenerates the licence revocation list (crates/akev-api/src/revoked.txt) from Stripe with
#     the website's licensing CLI: set AKEV_WEBSITE_DIR (default ../akev-website) and
#     AKEV_LICENSING_ENV (an env file with STRIPE_SECRET_KEY and STRIPE_PAYMENT_LINKS, default
#     ~/.config/akev/production.env). AKEV_SKIP_REVOCATION=1 releases with the list unchanged;
#     like every setting here, they can live in .env (see .env.example);
#   * checks that this build trusts the production licence key (the canary test).
set -euo pipefail

version=${1:-}
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9]+)?$ ]]; then
  echo "usage: scripts/release.sh <version>   (e.g. 0.2.0, or 0.2.0-1 for a pre-release:" >&2
  echo "       Windows installers need numeric versions, so no -beta.1)" >&2
  exit 2
fi
cd "$(dirname "$0")/.."
. scripts/with-env

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Commit or stash your changes first." >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  echo "Tag v$version already exists." >&2
  exit 1
fi

# Release notes: the "## Unreleased" section must say something.
notes=$(awk '/^## Unreleased/{f=1; next} /^## /{f=0} f' CHANGELOG.md | grep -v '^[[:space:]]*$' || true)
if [ -z "$notes" ]; then
  echo "Write the release notes under \"## Unreleased\" in CHANGELOG.md first." >&2
  exit 1
fi

# Licence revocations (refunds, chargebacks) since the last release.
if [ "${AKEV_SKIP_REVOCATION:-}" = 1 ]; then
  echo "Skipping the revocation list refresh (AKEV_SKIP_REVOCATION=1)." >&2
else
  website=${AKEV_WEBSITE_DIR:-../akev-website}
  envfile=${AKEV_LICENSING_ENV:-$HOME/.config/akev/production.env}
  if [ ! -f "$website/licensing/cli.ts" ] || [ ! -f "$envfile" ]; then
    echo "Can't refresh the revocation list: need $website/licensing/cli.ts and $envfile" >&2
    echo "(set AKEV_WEBSITE_DIR / AKEV_LICENSING_ENV, or AKEV_SKIP_REVOCATION=1 to release without it)." >&2
    exit 1
  fi
  revoked=$(mktemp)
  (cd "$website" && bun --env-file="$envfile" licensing/cli.ts revoked) >"$revoked"
  grep -q '^#' "$revoked" || { echo "The revocation list from Stripe looks wrong:" >&2; cat "$revoked" >&2; exit 1; }
  mv "$revoked" crates/akev-api/src/revoked.txt
  echo "Revoked licences: $(grep -cv '^\(#\|$\)' crates/akev-api/src/revoked.txt || true)"
fi

# A build that can't verify production keys would refuse every buyer.
cargo test --quiet -p akev-api --lib license::tests::canary_key_verifies_in_release_builds

# First occurrence only: the version line right under [workspace.package], and the top-level
# "version" of each JSON file.
perl -0pi -e "s/(\\[workspace\\.package\\]\\nversion = )\"[^\"]*\"/\${1}\"$version\"/" Cargo.toml
perl -0pi -e "s/\"version\": \"[^\"]*\"/\"version\": \"$version\"/" desktop/src-tauri/tauri.conf.json ui/package.json

for f in Cargo.toml desktop/src-tauri/tauri.conf.json ui/package.json; do
  grep -q "\"$version\"" "$f" || { echo "Couldn't set the version in $f" >&2; exit 1; }
done

release_date=$(date -u +%Y-%m-%d)
perl -pi -e "s/(    None => )\"\d{4}-\d{2}-\d{2}\",/\${1}\"$release_date\",/" crates/akev-core/src/lib.rs
grep -q "None => \"$release_date\"," crates/akev-core/src/lib.rs ||
  { echo "Couldn't set the release date in crates/akev-core/src/lib.rs" >&2; exit 1; }

# CHANGELOG.md: "## Unreleased" becomes this version, and a fresh empty section goes on top.
perl -0pi -e "s/^## Unreleased\n/## Unreleased\n\n## $version — $release_date\n/m" CHANGELOG.md
grep -q "^## $version — $release_date" CHANGELOG.md ||
  { echo "Couldn't add the version heading to CHANGELOG.md" >&2; exit 1; }

# Refresh the workspace crates' versions in Cargo.lock (no other dependency changes).
cargo update --workspace --quiet

git add Cargo.toml Cargo.lock desktop/src-tauri/tauri.conf.json ui/package.json \
  crates/akev-core/src/lib.rs CHANGELOG.md crates/akev-api/src/revoked.txt
git commit --quiet -m "Release v$version"
git tag -a "v$version" -m "Akev $version"
echo "Committed and tagged v$version. Push both to build the release:"
echo "  git push origin HEAD v$version"
