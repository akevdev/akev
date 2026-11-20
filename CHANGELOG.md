# Changelog

Every release of Akev, newest first. `scripts/release.sh` moves the notes under
**Unreleased** below the new version when it tags a release; add an entry here with every
user-visible change.

## Unreleased

- In-app updates work for the desktop app installed from a `.deb`, `.rpm` or `.msi` package.
  Before, those installs were offered the AppImage or the NSIS installer, and on Linux the
  update failed with "invalid updater binary format". RPM installs on Fedora 43 and later
  (RPM 6) are upgraded through the system's password prompt.

## 0.1.1 — 2026-11-19

- Update checks trust the operating system's certificate store instead of a bundled copy of
  Mozilla's, so they also work behind TLS-inspecting proxies whose CA the system trusts.
- Rust code exported from a request targets reqwest 0.13.
- `SHA256SUMS.sigstore.json` is a standard Sigstore bundle (signed with cosign 3); cosign 2
  verifies it with `--new-bundle-format`.
- The Docker image runs `akev` with `start …` as its default command, so
  `docker run -i --rm ghcr.io/akevdev/akev mcp` runs the MCP server. Arguments after the image
  name now replace that command rather than adding options to `start`.
- Building from source needs Rust 1.95 or newer.
- Dependency updates.

## 0.1.0 — 2026-11-15

The first public release.

- Capture HTTP/1.1, HTTP/2, HTTP/3, WebSocket, SSE and gRPC traffic from browsers, terminals,
  Android devices, Docker containers, Electron apps and the whole system, with a per-install
  certificate authority.
- Inspect requests with formatted bodies (JSON, XML, HTML, images, protobuf, MessagePack, CBOR),
  a filter language, timings, TLS details and side-by-side comparison.
- Breakpoints, mocks, rewrites, Map Local and Map Remote, throttling and fault injection, with
  rule import from HTTP Toolkit.
- Compose and replay requests; export them as code for 20 targets; import and export HAR
  and Akev sessions; import Charles, mitmproxy and Fiddler (.saz) sessions.
- `akev capture -- <command>` for CI: runs your tests through the proxy and writes a HAR.
- One-click CA trust on macOS, Windows and Linux, and update checks (desktop app updates
  itself after you confirm).
- An MCP server so AI agents can read and work with captured traffic.
- Akev Pro ($49 once, one year of updates): script rules, TLS fingerprint impersonation,
  WireGuard and transparent capture, JVM attach, protobuf schemas, the LLM lens and MCP write
  tools. Licence keys are checked offline; a 14-day trial runs once per computer.
