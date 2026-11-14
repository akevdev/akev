# Contributing to Akev

Thanks for helping. Bug reports, fixes and features are all welcome.

## Before you start

- **Bugs:** open an issue with the version, your OS, what you did, and what happened.
- **Features:** open an issue first, so we can agree on the shape before you write code.
- **Security problems:** see [SECURITY.md](SECURITY.md); don't open a public issue.

## Development

```bash
make ui          # build the web UI (Bun)
make dev         # daemon + UI dev server
make test        # Rust and UI tests
make lint        # clippy, rustfmt, cargo-deny, the UI linter
make e2e         # Playwright end-to-end and screenshot tests
```

Development settings (a separate data folder, a fixed API token, log levels, the UI's mock
backend, …) go in one file: copy [`.env.example`](.env.example) to `.env` and uncomment what you
need. Every `make` target reads it; for a command you run yourself, prefix it with
`scripts/with-env` (for example `scripts/with-env cargo test -p akev-api`).

[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains how the pieces fit together, and
[docs/DESIGN.md](docs/DESIGN.md) the UI conventions. CI runs the same checks on Linux, macOS
and Windows; a pull request needs them green.

## Repository layout

```
crates/      the Rust crates: the engine (akev-core … akev-api), the `akev` CLI (akev-cli) and
             the benchmark load generator (akev-loadgen); docs/ARCHITECTURE.md has one line each
ui/          the web UI (React, TypeScript, Bun, Vite), with its own scripts/ and e2e/;
             ui/README.md has its layout
desktop/     the desktop app: the Tauri shell (src-tauri/) and its splash page
brand/       the logo sources and the scripts that render every icon and installer image
packaging/   package-manager templates (Homebrew, winget, Scoop, AUR, AppStream) and notices/
             (cargo-about's config and template for THIRD-PARTY-NOTICES.md)
scripts/     repository scripts: releasing, packaging, notices, demo traffic, with-env
fuzz/        cargo-fuzz targets (a separate workspace, nightly)
docs/        contributor docs; screenshots/ holds the README's images
.github/     CI, CodeQL, the release workflow, issue and PR templates
```

The user guides and the HTTP API reference are on [akev.dev/docs](https://akev.dev/docs/),
which lives in the website's own repository.

Where things go:

- **Scripts.** Repository-wide ones in `scripts/`; ones that belong to a package in that
  package's `scripts/` (`ui/scripts/`); generators next to their sources (`brand/`). Each one is
  a `make` target or is named in a doc (`make help` lists the targets).
- **Images.** The only sources are in `brand/` (`make brand` renders them). Everything else is
  generated (favicons, desktop icons, installer art, the README screenshots) or a test
  baseline (`ui/e2e/visual/__screenshots__/`).
- **Rust tests.** Unit tests in `src/**/tests.rs` (split into `src/tests/` when large);
  integration tests in `tests/*.rs`, their shared helpers in `tests/common/` and their data in
  `tests/fixtures/` (expected output in `tests/golden/`).
- **Names.** Folders are lowercase, with a hyphen between words (`first-run`, `kitchen-sink`).

Code conventions:

- No `unwrap`, `expect` or `panic!` outside tests (clippy enforces it): request paths must
  never take the process down.
- New behaviour comes with tests. Protocol and parser changes need tests with real bytes.
- User-facing text is plain, sentence case, and says what to do next.

## Sign your commits (DCO)

Akev is licensed under AGPL-3.0-or-later and sold as Akev Pro by its maintainers. To keep the
right to license the code that way, every commit must be signed off under the
[Developer Certificate of Origin](https://developercertificate.org/):

```bash
git commit -s -m "Fix the thing"
```

The `Signed-off-by:` line certifies that you wrote the change or otherwise have the right to
submit it under the project's licence. Contributions are accepted under AGPL-3.0-or-later and,
by signing off, you also allow the maintainers to distribute them as part of Akev Pro.

## Trademark

"Akev" and the Akev logo identify the official builds. Forks are welcome under the AGPL, but
please give them a different name and logo so users can tell them apart.
