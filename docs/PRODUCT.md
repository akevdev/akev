# Akev: product principles

**One line:** Akev is a fast, lightweight, open HTTP(S) debugging proxy. Intercept anything in
one click, see every request as it happens (even at hundreds of thousands of exchanges), and
mock, rewrite and breakpoint traffic for free, with power features in a one-time Pro licence
instead of a subscription.

This file is for contributors: what Akev promises, so changes can be checked against it. User
documentation lives at <https://akev.dev/docs>.

## Promises

- **Your data stays yours.** Everything Akev captures stays in its data directory. No
  telemetry, no analytics, no crash uploads. The only request Akev makes on its own is the
  update check (a small JSON file from GitHub Releases, at most once a day while the UI is
  open; headless runs never check), and it can be turned off.
- **The basics are free, forever.** Capture, every interceptor except JVM attach, persistent
  rules (mock, rewrite, map, delay, throttle, block), breakpoints, WebSocket tools,
  Compose/replay/code export, imports and exports, filters, HTTP/3, upstream proxies and client
  certificates, schema-less protobuf/gRPC, MCP read tools, the CLI and the API.
- **Licence state never touches your data.** Without Pro, Pro features stop running; their
  configuration stays saved. Nothing is deleted or locked.
- **Bytes stay as they were.** HTTP/1 header case, order and duplicates are preserved in both
  directions; paths are forwarded verbatim; replay uses the same model as capture.
- **Bounded, and honest about it.** Memory, disk, queues and timeouts all have limits by
  default. When a limit drops or skips something, the exchange and the status counters say so.
- **One process, offline.** The UI is bundled and pinned to the daemon's version; nothing is
  loaded from the internet.
- **Explain failures.** Errors have a type and a plain-language explanation with what to do
  next.

## Akev Pro

- **$49 once**, including one year of updates; every version released in that year keeps
  working forever. Another year of updates is $25. A 14-day trial runs once per computer.
- Unlocks script rules, TLS fingerprint impersonation, WireGuard mode, transparent capture, JVM
  attach, protobuf schemas, the LLM lens and MCP tools that change state.
- **Licence keys are checked offline** (Ed25519 signatures): no account, no activation server.
  One person, any number of their machines; team purchases carry a seat count.
- The source is AGPL-3.0-or-later, Pro features included. The licence is how the project is
  funded, not a lock.

Later: **Akev Teams**, with shared rule sets and sessions (with redaction), a relay for webhooks
and remote devices, SSO and audit logs.

## Quality bars

Measured numbers and the method are in [PERFORMANCE.md](PERFORMANCE.md).

- Idle memory ≤ 150 MB (measured about 22 MB); 100,000 exchanges ≤ 400 MB (about 360 MB).
- Ingest ≥ 2,000 requests/s through the proxy (measured about 17,000 req/s, p99 14 ms, on a
  loaded quad-core).
- Bounded by default: memory levels off at 200,000 exchanges, spill files at 4 GiB.
- Cold start to a usable UI in under a second.
- No silent data loss: settings and rules are written atomically and never reset, sessions
  export losslessly, and autosave (on by default) brings back the newest 20,000 exchanges
  after a restart or crash. Traffic captured after the last autosave (at most 60 s) is lost in
  a crash.
- Every release passes the full test suite on Linux, macOS and Windows, with TLS
  impersonation built in.
