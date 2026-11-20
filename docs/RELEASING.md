# Releasing Akev

Releases are built by [`.github/workflows/release.yml`](../.github/workflows/release.yml) on
GitHub's runners. A tag produces a **draft** GitHub Release with every package attached and
a Docker image on GHCR; nothing is public until you publish the draft.

## Cut a release

```bash
scripts/release.sh 0.2.0        # bumps every version, commits "Release v0.2.0", tags v0.2.0
git push origin HEAD v0.2.0     # starts the release workflow
```

`scripts/release.sh` keeps the three places the version lives in step: `Cargo.toml`
(`[workspace.package]`, shared by all crates, and `Cargo.lock`),
`desktop/src-tauri/tauri.conf.json` and `ui/package.json`. The workflow stops if they or
the tag disagree. For a pre-release use a numeric suffix (`0.2.0-1`): Windows MSI versions
can't contain letters. A version with a suffix is marked as a pre-release on GitHub.

It also stamps today's date (UTC) as `RELEASE_DATE` in `crates/akev-core/src/lib.rs`. An Akev
Pro licence covers every version released up to its *updates until* date, so this date decides
which licences unlock the build (a build can also be given one with the `AKEV_RELEASE_DATE`
environment variable at compile time).

Before tagging, the script also:

- **Release notes.** Moves the notes under `## Unreleased` in `CHANGELOG.md` below the new
  version. It refuses to run while that section is empty, so write the notes first.
- **Revocations.** Regenerates the licence revocation list
  (`crates/akev-api/src/revoked.txt`) from Stripe with the website's licensing CLI
  (`bun licensing/cli.ts revoked`). It needs the website checkout (`AKEV_WEBSITE_DIR`, default
  `../akev-website`) and an env file with the Stripe settings (`AKEV_LICENSING_ENV`, default
  `~/.config/akev/production.env`). Refunded and charged-back licences stop unlocking new
  builds. `AKEV_SKIP_REVOCATION=1` releases with the list unchanged. These settings can live in
  `.env` (see `.env.example`), which the script reads.
- **Canary.** Runs the canary test, which proves the build trusts the production licence key
  (see below).

### Licence public key

Release builds trust the production licence-signing key: its public half is committed in
`PUBLIC_KEYS` in `crates/akev-api/src/license.rs` (forks can add their own with
`AKEV_LICENSE_PUBLIC_KEY` at build time). The private half lives only in the licensing service
(`akev-website/licensing`) and an offline backup; never commit it.

- **The canary.** `canary_key_verifies_in_release_builds` checks a committed key, `canary-0001`.
  The production signer signed it with updates that ended on 2026-01-01, so it unlocks nothing,
  but it fails the release if `PUBLIC_KEYS` is missing or wrong.
- **Rotating the key.** Generate a new pair (`bun licensing/cli.ts keygen`), *add* its public
  key to `PUBLIC_KEYS` (never remove the old one: every key it signed would stop working),
  release, then switch the service's `LICENSE_SIGNING_KEY`. Sign a new canary with the new key.
- **Custody.** The signing seed and `ACCOUNT_SECRET` belong in the hosting provider's secret
  store and one offline backup (a password manager), not in a plaintext file on a laptop.

When the workflow finishes (about 30–40 minutes), open **Releases**, check the draft (the
notes are generated from the commits since the last release; edit them freely) and press
**Publish release**. If a job fails, fix it and re-run the workflow for the same tag: uploads
replace the existing files. A re-run uses the workflow files as they were at the tag, so a
fix to the workflow itself doesn't reach it.

Publish only when every job is green. The last one, **Checksums**, writes `SHA256SUMS`, its
Sigstore bundle, the SBOM, the package-manager zip and `latest.json`. A published release
without `latest.json` breaks update checks for every installed app, because they fetch it
from the latest release. That job is its own workflow, `release-checksums.yml`. If it fails
(or its files need redoing), fix it on `main` and run it for the tag. It takes the packages
already attached to the release and replaces its own files there, even on a published release:

```sh
gh workflow run release-checksums.yml -f tag=vX.Y.Z
```

## Test build (nothing published)

**Actions → Release → Run workflow** on any branch builds the same packages and keeps them as
workflow artifacts (`release-linux-x64`, `release-linux-arm64`, `release-macos-universal`,
`release-windows-x64`, `release-windows-arm64`, `release-checksums`) for 90 days; the Docker
image is built but not pushed. Use it before a
release, or after changing anything that affects packaging. (CI no longer builds desktop
bundles on every push.)

## What a release contains

| File | What |
|---|---|
| `Akev_<v>_universal.dmg` | macOS app (Apple Silicon + Intel) |
| `Akev_<v>_universal.app.tar.gz` | the same `.app`, zipped (for scripted installs) |
| `Akev_<v>_x64-setup.exe`, `Akev_<v>_x64_en-US.msi` (and `_arm64`) | Windows installers (NSIS, MSI) for x64 and Arm64 |
| `Akev_<v>_amd64.AppImage`, `Akev_<v>_amd64.deb`, `Akev-<v>-1.x86_64.rpm` (and the `aarch64` / `arm64` builds) | Linux desktop app for x64 and Arm64 |
| `akev-<v>-<target>.tar.gz` / `.zip` | the `akev` command-line tool alone, for Linux x64 and Arm64 (glibc 2.35+), macOS (universal) and Windows x64 and Arm64 |
| `SHA256SUMS` | checksums of all of the above |
| `ghcr.io/akevdev/akev:<v>` (and `:<major>.<minor>`, `:latest`) | headless Docker image (linux/amd64, linux/arm64) |

## Code signing

Without signing, the packages work but the operating system warns first-time users. That's
acceptable for a test build and a poor first impression for a launch.

### macOS (Developer ID + notarization)

Needs an Apple Developer Program membership. Add these repository secrets
(**Settings → Secrets and variables → Actions**) and the workflow signs and notarizes the app
automatically:

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | your *Developer ID Application* certificate exported as `.p12`, base64-encoded (`base64 -i cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | for notarization: your Apple ID, an [app-specific password](https://support.apple.com/102654) and team ID |

With no secrets the app is ad-hoc signed: it runs, but on first launch macOS asks the user to
allow it in **System Settings → Privacy & Security → Open Anyway**. The CLI archive isn't
signed; downloaded with a browser it needs `xattr -d com.apple.quarantine akev` once.

### Windows (Azure Trusted Signing)

The workflow signs every executable and installer through Tauri's
`bundle.windows.signCommand` with [trusted-signing-cli](https://github.com/Levminer/trusted-signing-cli)
when these repository secrets exist:

| Secret | Value |
|---|---|
| `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID` | an app registration with the *Trusted Signing Certificate Profile Signer* role |
| `TRUSTED_SIGNING_ENDPOINT` | e.g. `https://weu.codesigning.azure.net/` |
| `TRUSTED_SIGNING_ACCOUNT`, `TRUSTED_SIGNING_PROFILE` | the signing account and certificate profile names |

Without them the installers are unsigned and SmartScreen warns ("Windows protected your PC" →
*More info* → *Run anyway*).

### In-app updates

The desktop app updates itself from `latest.json` on the latest GitHub release
(`plugins.updater` in `tauri.conf.json`), and only installs packages signed with the updater
key. The key pair was made with `cargo tauri signer generate`:

- the public key is in `tauri.conf.json`;
- the private key and its password are in `~/.config/akev/updater.key` and
  `~/.config/akev/updater.env`. Add them as the `TAURI_SIGNING_PRIVATE_KEY` (the key file's
  contents) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets, keep a copy in a password manager,
  then delete the files.

**Losing the private key means installed apps can never update again**, and a leaked key
lets someone ship updates to every user. With the secrets set, the workflow builds the update
packages (`.sig` files) and the checksums job writes `latest.json`. Without them no update
packages are made and installed apps don't see the release (they can still download it).

An installed app updates from the same kind of package it was installed from. It first looks
for `<os>-<arch>-<installer>` in `latest.json` (`linux-x86_64-rpm`, `windows-x86_64-msi`, …)
and then `<os>-<arch>` (AppImage, NSIS setup, macOS `.app`). If a `.deb` or `.rpm` install
fell back to the AppImage, it would fail with "invalid updater binary format". So every
package type needs its `.sig`, and the checksums job fails if one is missing. The app
upgrades an RPM itself with `pkexec rpm -U --nosignature`: RPM 6 (Fedora 43+) refuses
packages without an OpenPGP signature, and the updater signature has already been checked.

The CLI and browser UI only *check* (`GET /api/update`, `akev update`): users install with
their package manager or the download.

## Third-party notices

Every archive, app bundle and the Docker image ship `THIRD-PARTY-NOTICES.md`: the licences of
the Rust crates (cargo-about, `packaging/notices/`), BoringSSL and the UI's npm packages
and fonts. The release workflow generates it with `scripts/third-party-notices.sh`; run it
locally with `make notices` (needs `cargo install cargo-about --features cli --locked`). A new
licence type must be added to both `deny.toml` and `packaging/notices/about.toml`.

## Package managers

The release's `akev-<version>-packaging.zip` holds the manifests from `packaging/`, filled in
with that release's checksums (`scripts/package-manifests.sh`). The Homebrew tap and the Scoop
bucket pick them up by themselves: their `sync` workflow copies the latest release's files every
6 hours (after checking the zip against the signed `SHA256SUMS`), then tests them. So fix a
formula, cask or Scoop manifest here in `packaging/`: an edit made only in the tap or bucket is
overwritten by the next release. The other channels are published by hand:

| Channel | File | How |
|---|---|---|
| Homebrew | `homebrew/akev.rb` (CLI), `homebrew/akev-desktop.rb` (cask) | automatic: the `akevdev/homebrew-tap` sync (`brew install akevdev/tap/akev`); later submit to homebrew-core/cask once eligible |
| winget | `winget/*.yaml` (schema 1.12) | PR to `microsoft/winget-pkgs` under `manifests/a/Akev/Akev/<version>/` (or `wingetcreate submit`) |
| Scoop | `scoop/akev.json` | automatic: the `akevdev/scoop-bucket` sync (`scoop install akev/akev`) |
| AUR | `aur/PKGBUILD` | push to `ssh://aur@aur.archlinux.org/akev-bin.git` with a fresh `.SRCINFO` (`makepkg --printsrcinfo > .SRCINFO`; without Arch, run makepkg and namcap in the `archlinux` container image as a non-root user) |
| apt / dnf | the release's `.deb` / `.rpm` | `scripts/linux-repos.sh dist out <gpg key>` builds signed repositories to upload to a static host (e.g. `packages.akev.dev`) |

**Not offered:**
- **Flatpak / Flathub.** The sandbox would stop Akev from launching browsers and terminals,
  changing the system proxy or trusting its CA, so the app would install but not work.
- **Snap.** Strict confinement has the same problem.
- **crates.io.** The crates aren't published; `cargo install --path crates/akev-cli` from a
  checkout works.

## The website repository

The website ([akev.dev](https://akev.dev), with the user docs, pricing and the licensing
service) is a separate repository, checked out next to this one as `../akev-website`. The two
meet in a few places, all by hand:

| What | This repository | The website |
|---|---|---|
| Licence revocations | `crates/akev-api/src/revoked.txt`, refreshed by `scripts/release.sh` | `licensing/cli.ts revoked` (`AKEV_WEBSITE_DIR`) |
| Licence public key | `PUBLIC_KEYS` in `crates/akev-api/src/license.rs` | checked before every production build by `scripts/check-launch.ts` (`AKEV_APP_DIR`) |
| README screenshots | `docs/screenshots/*.png` | made by `scripts/screenshots.ts --format png --scale 1 --out ../akev/docs/screenshots` |
| Version | `Cargo.toml`, `tauri.conf.json`, `ui/package.json` (set by `scripts/release.sh`) | `lib/site.ts` (`version`): update it after each release |
| Release file names | `.github/workflows/release.yml` | `lib/downloads.ts` |
| Pro feature ids | `crates/akev-core/src/license.rs` | `ProFeature` in `lib/plans.ts` |
| Brand icon | `brand/akev-icon.svg`, `ui/public/favicon.svg` | copies in `public/brand/akev-icon.svg` and `app/icon.svg` |
| Changelog | `CHANGELOG.md` | `content/changelog.md` (longer, for users) |
| Doc links | `README.md` and the UI link to `akev.dev/docs/<slug>/` | `content/docs/<slug>.md`: renaming a page breaks those links |

Two scripts here are only run by hand: `scripts/make-public-repo.sh` creates the public
repository as a single fresh commit (it never pushes), and `scripts/demo-traffic.ts` sends a mix
of realistic requests through a running proxy, for demos and manual QA.

## Not automated yet

* **Package manager submissions** (see above): the winget PR and the AUR push are by hand (the
  tap and bucket sync themselves).
