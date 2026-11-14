#!/usr/bin/env bash
# Create the public repository: a fresh, single-commit history of the current tree, with one
# author identity and none of this repository's history (old commit messages, session links,
# removed research notes).
#
#   scripts/make-public-repo.sh <dest-dir> "<Author Name>" <author@email>
#
# It never pushes. Check the result, then:
#   cd <dest-dir> && git remote add origin git@github.com:akevdev/akev.git && git push -u origin main
set -euo pipefail
dest=${1:?usage: scripts/make-public-repo.sh <dest-dir> "<Author Name>" <author@email>}
name=${2:?author name}
email=${3:?author email}
cd "$(dirname "$0")/.."

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Commit or stash your changes first: only committed files are published." >&2
  exit 1
fi
if [ -e "$dest" ] && [ -n "$(ls -A "$dest" 2>/dev/null)" ]; then
  echo "$dest exists and isn't empty." >&2
  exit 1
fi
version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml)

# Things that must never be public, even if they slipped into the tree.
for path in docs/analysis httptoolkit .claude; do
  if git ls-files --error-unmatch "$path" >/dev/null 2>&1; then
    echo "$path is tracked; remove it before publishing." >&2
    exit 1
  fi
done
# Session links, and this machine's own home directory (generic /home/me examples are fine).
leaks="claude\\.ai/code/session_|$HOME/|/Users/$(id -un)/"
if git grep -qE "$leaks" -- . ':!scripts/make-public-repo.sh'; then
  echo "Session links or your home-directory path in tracked files:" >&2
  git grep -nE "$leaks" -- . ':!scripts/make-public-repo.sh' >&2
  exit 1
fi

mkdir -p "$dest"
git archive --format=tar HEAD | tar -x -C "$dest"
cd "$dest"
git init -q -b main
git config user.name "$name"
git config user.email "$email"
git add -A
git commit -q -m "Akev $version" -m "The fast, open HTTP(S) debugging proxy."
echo "Created $dest ($(git ls-files | wc -l) files, one commit by $name <$email>). Nothing was pushed."
