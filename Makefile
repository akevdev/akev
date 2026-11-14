# Akev developer tasks. `make help` lists them.

# Every recipe runs with the settings in .env (copy .env.example; variables already set in your
# shell win). scripts/with-env does the same for commands you run yourself.
SHELL := $(CURDIR)/scripts/with-env

SYSROOT_ENV := $(HOME)/.local/akev-sysroot/env.sh

# Optional toolchain for the BoringSSL impersonation backend (cmake + libclang), see
# https://akev.dev/docs/tls-impersonation/. Sourced when present; system cmake/clang work too.
IMPERSONATE_ENV := $(HOME)/.local/akev-toolchain/env.sh
IMPERSONATE_FEATURES := akev-cli/impersonate,akev-api/impersonate,akev-proxy/impersonate

.PHONY: help ui ui-dev daemon dev test e2e lint fmt release release-impersonate test-impersonate desktop desktop-dev brand notices clean

help:
	@grep -E '^[a-z0-9-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'

ui: ## Build the web UI into ui/dist (embedded into the daemon)
	cd ui && bun install && bun run build

ui-dev: ## Run the UI dev server (http://localhost:5173, proxies /api to the daemon)
	cd ui && bun run dev

daemon: ## Run the daemon in dev mode (allows the Vite dev server origin)
	cargo run -p akev-cli -- start --no-open --allow-origin http://localhost:5173

dev: ## Daemon + UI dev server
	$(MAKE) -j2 daemon ui-dev

test: ## Run all Rust and UI tests and the UI type check
	cargo test
	cd ui && bun x tsc --noEmit && bun test

e2e: ## UI end-to-end and screenshot tests (Playwright; starts its own daemon)
	cd ui && bun run e2e

lint: ## Clippy, format check, cargo-deny and the UI linter
	cargo clippy --all-targets -- -D warnings
	cargo fmt --all -- --check
	@if command -v cargo-deny >/dev/null; then cargo deny --all-features check; else echo "cargo-deny not installed (cargo install cargo-deny); skipped"; fi
	cd ui && bun run lint

fmt: ## Format everything
	cargo fmt --all

release: ui ## Optimised CLI build (target/release/akev)
	cargo build --release -p akev-cli

release-impersonate: ui ## Release build with browser TLS fingerprints (BoringSSL; needs cmake + clang)
	@[ -f $(IMPERSONATE_ENV) ] && . $(IMPERSONATE_ENV); cargo build --release -p akev-cli --features impersonate

test-impersonate: ## Rust tests with the impersonation backend compiled in
	@[ -f $(IMPERSONATE_ENV) ] && . $(IMPERSONATE_ENV); cargo test --no-fail-fast --features $(IMPERSONATE_FEATURES)

desktop: ui ## Build the desktop app bundle (needs webkit2gtk dev headers on Linux)
	@[ -f $(SYSROOT_ENV) ] && . $(SYSROOT_ENV); cd desktop/src-tauri && cargo tauri build

desktop-dev: ui ## Run the desktop app (debug)
	@[ -f $(SYSROOT_ENV) ] && . $(SYSROOT_ENV); cargo run -p akev-desktop

brand: ## Regenerate the logo SVGs, favicons, desktop icons and installer art from brand/
	bun brand/build.ts
	brand/render.sh

notices: ## Write THIRD-PARTY-NOTICES.md (needs cargo-about and ui/node_modules)
	scripts/third-party-notices.sh

clean: ## Remove build output (target/, ui/dist, ui/node_modules)
	cargo clean
	rm -rf ui/dist ui/node_modules
