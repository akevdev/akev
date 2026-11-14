# Changelog

Every release of Akev, newest first. `scripts/release.sh` moves the notes under
**Unreleased** below the new version when it tags a release; add an entry here with every
user-visible change.

## Unreleased

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
