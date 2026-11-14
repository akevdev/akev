#!/usr/bin/env bash
# Build signed apt and rpm repositories from a release's .deb and .rpm files, ready to upload
# to any static host (GitHub Pages, S3, a CDN):
#
#   scripts/linux-repos.sh <dist-dir> <out-dir> <gpg-key-id>
#
# Adds the packages to what <out-dir> already holds (download the published repository there
# first to keep older versions). Needs dpkg-dev (dpkg-scanpackages), apt-utils (apt-ftparchive),
# createrepo_c and gpg with the signing key. Users then add the repository with the public key
# published as <out-dir>/akev.gpg (see docs/RELEASING.md).
set -euo pipefail
dist=${1:?usage: scripts/linux-repos.sh <dist-dir> <out-dir> <gpg-key-id>}
out=${2:?out dir}
key=${3:?gpg key id}

mkdir -p "$out/apt/pool/main" "$out/rpm"
gpg --export "$key" > "$out/akev.gpg"
gpg --armor --export "$key" > "$out/akev.asc"

# apt: one "stable" suite, component "main", amd64 and arm64.
cp "$dist"/*.deb "$out/apt/pool/main/"
for arch in amd64 arm64; do
  dir="$out/apt/dists/stable/main/binary-$arch"
  mkdir -p "$dir"
  (cd "$out/apt" && dpkg-scanpackages --arch "$arch" pool/main /dev/null) > "$dir/Packages"
  gzip -9kf "$dir/Packages"
done
(cd "$out/apt/dists/stable" && apt-ftparchive \
  -o APT::FTPArchive::Release::Origin=Akev \
  -o APT::FTPArchive::Release::Label=Akev \
  -o APT::FTPArchive::Release::Suite=stable \
  -o APT::FTPArchive::Release::Codename=stable \
  -o APT::FTPArchive::Release::Architectures="amd64 arm64" \
  -o APT::FTPArchive::Release::Components=main \
  release . > Release)
gpg --batch --yes --local-user "$key" --armor --detach-sign -o "$out/apt/dists/stable/Release.gpg" "$out/apt/dists/stable/Release"
gpg --batch --yes --local-user "$key" --clearsign -o "$out/apt/dists/stable/InRelease" "$out/apt/dists/stable/Release"

# rpm: packages signed individually is optional; the repository metadata is signed.
cp "$dist"/*.rpm "$out/rpm/"
createrepo_c --update "$out/rpm"
gpg --batch --yes --local-user "$key" --armor --detach-sign -o "$out/rpm/repodata/repomd.xml.asc" "$out/rpm/repodata/repomd.xml"
cat > "$out/rpm/akev.repo" <<REPO
[akev]
name=Akev
baseurl=https://packages.akev.dev/rpm
enabled=1
gpgcheck=0
repo_gpgcheck=1
gpgkey=https://packages.akev.dev/akev.asc
REPO
echo "Repositories in $out: upload it to the host behind packages.akev.dev."
