# Akev Architecture

Akev is an HTTP(S) interception, inspection, mocking and debugging tool — a faster, lighter,
more capable alternative to HTTP Toolkit. It's a Rust workspace plus a TypeScript web UI.

```
                ┌────────────────────────── akev daemon (one process) ─────────────────────────┐
 clients ──────▶│ akev-proxy ──events──▶ akev-store ──StoreEvents──▶ akev-api ──WS/REST──▶ UI │◀── browser / Tauri webview
 (browsers,     │   │  ▲   ▲                  ▲  (records, bodies,         │  (axum)               │
  apps, CLIs,   │   │  │   └ akev-rules      │   HAR/sessions, filter)   ├─ MCP endpoint ◀────────┼── AI agents (akev mcp)
  devices)      │   │  └ akev-ca (leaf certs)│                           ├─ embedded UI assets    │
                │   ▼                         └ akev-filter, akev-content└─ akev-intercept ─────┼──▶ launches/configures clients
 servers ◀──────│ upstream (DNS, TLS, pools, upstream proxies)                                     │
                └──────────────────────────────────────────────────────────────────────────────────┘
```

## Crates

| Crate | Role |
|---|---|
| `akev-core` | Shared types: exchanges, events, headers, rules model, settings, breakpoints, interceptor descriptors. serde camelCase ⇄ UI. |
| `akev-ca` | Root CA load/generate (ECDSA P-256) & cached leaf certificates for MITM. |
| `akev-content` | Content-encoding decode/encode, content sniffing, pretty formatting, protobuf/gRPC/msgpack/CBOR/SSE/multipart/JWT/image decoders, typed protobuf decoding with user schemas (`schema`, prost-reflect), `view_body` for the UI, the LLM traffic lens (`llm`: recognises OpenAI / Anthropic / Gemini / Ollama / OpenAI-compatible calls and normalises them into conversations, reassembling streams). |
| `akev-codegen` | Snippet generation for 20 targets; cURL parser. |
| `akev-filter` | Traffic filter query language (parser, evaluator, autocompletion, docs). |
| `akev-rules` | Rule compilation & evaluation (matchers, modifier stacking, terminal actions), transforms, mock/map-local/forward helpers, QuickJS scripting, HTTP Toolkit rule import. |
| `akev-store` | Event → record ingestion, body memory budget with zstd spill-to-disk, queries, live StoreEvents, HAR 1.2 import/export, `.akev` sessions and the session library (`SessionLibrary`), Charles `.chlsj` and mitmproxy flow import. |
| `akev-proxy` | Proxy engine: protocol sniffing, CONNECT, SOCKS4/5, TLS MITM (rustls) with JA3/JA4, custom HTTP/1 codec (exact header fidelity), HTTP/2 (h2), HTTP/3 (quinn + h3: QUIC listener `src/http3/`, upstream client `src/upstream/h3.rs` with Alt-Svc cache `alt_svc.rs`), WebSockets, SSE, rules pipeline, breakpoints, throttling, upstream client (DNS overrides, upstream proxies, client certs, pooling), process attribution, device setup pages, Compose `send`, TLS fingerprint impersonation (`upstream/fingerprint`, optional BoringSSL backend behind the `impersonate` feature). |
| `akev-intercept` | Interceptors: Chromium-family browsers, Firefox, fresh & existing terminals, Android (ADB), Docker, Electron apps, running JVMs (Java agent attach: `java/` sources built into an embedded jar by `build.rs`), system proxy (GNOME/KDE/macOS/Windows), transparent capture command generation (Linux nftables/iptables + cgroup v2), manual setup. |
| `akev-wireguard` | WireGuard mode: userspace WireGuard server (boringtun `noise::Tunn`, one client peer) + smoltcp TCP/IP stack accepting any destination; each TCP connection becomes a `TcpStream` (local address = original destination) handed to the proxy via `ProxyHandle::serve_tunnelled`; DNS relay (hickory), QUIC drop, UDP relay; persisted x25519 keys and client config / QR payload. |
| `akev-api` | App orchestration (`start`), settings/rules persistence, REST + WebSocket stream ([HTTP API](https://akev.dev/docs/api/)), breakpoint manager, MCP server, embedded UI, protobuf schema registry (`protos`: `.proto` files compiled at runtime with protox, descriptor sets, URL mappings). |
| `akev-cli` | `akev` binary: `start` (daemon + open UI), `capture` (headless CI capture to HAR), `ca`, `mcp` (stdio bridge), `intercept`, `status`, `export`. |
| `akev-loadgen` | Benchmark load generator (not shipped; `docs/PERFORMANCE.md`). Not a default member: build it with `-p akev-loadgen`. |
| `desktop/src-tauri` | Tauri 2 desktop shell: runs `akev_api::start` in-process, native window/menus/tray/notifications/file dialogs. |
| `ui/` | React + TypeScript UI (`docs/DESIGN.md`). Built to `ui/dist`, embedded into `akev-api` via rust-embed. |
| `fuzz/` | cargo-fuzz targets for the proxy's parsers, the store and the content decoders (its own workspace; the `fuzz` CI job). |

The rest of the repository (brand, packaging, scripts) and where each kind of file goes:
[CONTRIBUTING.md § Repository layout](../CONTRIBUTING.md#repository-layout).

## Key flows

**Capture**: client → proxy accepts → sniff → (CONNECT/SOCKS unwrap) → TLS MITM with leaf from
`akev-ca` → HTTP/1 or HTTP/2 request parsed → `RequestStarted` event → rules decision →
(request breakpoint) → upstream or local response → `ResponseStarted`/`ResponseCompleted` events
→ store builds the record → `StoreEvent` → API batches to UIs (≤30 Hz) with per-connection
server-side filtering.

**HTTP/3**: upstream, `UpstreamClient::send` first asks `upstream/h3.rs` whether to try QUIC
(the request's `httpVersion`, else `settings.proxy.http3.upstream`: `alt-svc` looks up the
per-origin `Alt-Svc` cache filled from every HTTPS response, `prefer` tries every HTTPS origin;
never with an upstream proxy or for upgrades). It reuses a pooled QUIC connection (one per origin
+ alternative + TLS mode, a second one past 64 in-flight requests) or races a handshake over the
resolved addresses with the TCP path's rustls config (ALPN `h3`, own session cache). A
connection that can't be established hands the untouched body back and the request continues
over TCP; the origin is marked broken (5 min → 1 h backoff). Inbound, the `QuicListener`
(`src/http3/mod.rs`) follows the settings (started/moved/stopped by `update_settings` and
`rebind`) and `http3/server.rs` turns each request stream into an `ExchangeRequest`
(`HttpVersion::H3`, `ConnCtx` with the QUIC peer and SNI) run by the same `pipeline::run`, with
an `H3Responder` variant of `Responder`. `ProxyHandle::serve_quic` serves any caller-provided
`quinn::Endpoint` the same way (original destination → tunnel target).

**Breakpoints**: rules decide `break_on_request` → proxy calls `BreakpointHandler::on_request`
(implemented by `akev-api`) → API broadcasts `paused` → UI edits → `POST
/api/breakpoints/:id/resume` → oneshot resolves → proxy continues with edited request. At most
`breakpoints.maxPaused` exchanges wait at once (further matches go on unpaused) and
`breakpoints.autoReleaseSecs` continues a forgotten pause by itself; both leave a notice on
the exchange and a `notice` for UIs.

**WebSocket tools**: each relayed WebSocket direction runs a frame-aware pump
(`akev-proxy/src/ws_pump.rs`) that streams frames as they arrive, writes messages of its own
only between messages, and holds complete messages when a `ws-breakpoint` rule (matched at the
upgrade, re-checked per message against the live rule set) may pause them →
`BreakpointHandler::on_message` → `message-paused` → `POST /api/breakpoints/messages/:pauseId/resume`.
Live sockets register per-direction injection queues by exchange id (`ws_control.rs`) →
`ProxyHandle::inject_ws_message` ← `POST /api/exchanges/:id/messages`.

**LLM traffic lens**: when a request starts / completes the store tags its summary with
`llm: { provider, model }` (`akev_content::llm::tag`: URL for well-known providers, plus a cheap
body probe — no JSON tree — for other hosts), which the filter language reads (`is:llm`,
`model:`, `provider:`). `GET /api/exchanges/:id/llm` and MCP `akev_llm_conversation`
(`akev-api/src/llm.rs`) decode the bodies and take the SSE events with their arrival times from
the recorded stream messages, then `akev_content::llm::parse` builds the provider-neutral
conversation (reassembled reply, usage, time to first token) on the blocking pool.

**WireGuard mode**: one task per server (`akev-wireguard/src/server.rs`) owns the UDP socket,
the boringtun tunnel and the smoltcp interface. Datagram → `Tunn::decapsulate` → IP packet:
TCP goes to smoltcp (a SYN to a new 4-tuple first gets a socket listening on that exact
destination, processed immediately so concurrent SYNs never share a listener); UDP to
`10.0.0.53:53` is answered by the DNS relay, UDP 443 dropped, other UDP relayed through per-flow
sockets. Established sockets become `TcpStream`s (64 KiB staging per direction; the TCP window
closes when the proxy doesn't read) → `ProxyBridge` → `ProxyHandle::serve_tunnelled`
(`akev-proxy/src/tunnelled.rs`), which runs `conn::handle` with a `ConnCtx` already inside a
tunnel to the original destination (`ProxyMode::Wireguard`, interceptor `wireguard`, no process
lookup) — so sniffing, TLS MITM by SNI, HTTP parsing, rules and raw tunnelling are the CONNECT
code paths. `akev-api/src/wireguard.rs` starts/stops the server from `settings.proxy.wireguard`,
looks up the current proxy on every connection (it is replaced when the CA is regenerated),
serves `/api/wireguard*` and registers the `wireguard` interceptor.

**Transparent capture (Linux)**: a second listener (`akev-proxy/src/transparent/`,
`settings.proxy.transparent`) on loopback + the proxy's bind address. For each accepted socket
it reads the conntrack original destination (`getsockopt(SO_ORIGINAL_DST)` /
`IP6T_SO_ORIGINAL_DST` via libc; injectable through `ProxyHandle::set_original_destination_lookup`
for tests) and hands the connection to the normal layer loop as a tunnel
(`ConnCtx::enter_tunnel(dst, ProxyMode::Transparent)`), so sniffing, SNI-based MITM,
passthrough and raw tunnels are the CONNECT code paths. Loop protection: the generated rules
exclude Akev (cgroup, uid or `SO_MARK`, set on every upstream socket in
`upstream/connect.rs` when a mark is configured), and connections from Akev's own upstream
sockets are refused and counted. `akev-intercept/src/transparent/` generates the nftables /
iptables / systemd commands (never runs them) and backs the `transparent-linux` interceptor;
`GET /api/transparent` reads the listener counters through `TransparentProbe` for the redirect
check.

**Upstream TLS fingerprints** (`akev-proxy/src/upstream/fingerprint/`): the pipeline fills
`RequestOptions.fingerprint` with the client's browser family (`detect_client`: JA4 cipher hash
+ `User-Agent`) and the last matching `tls-profile` rule; `ProfileSelector` (compiled from
`proxy.tlsProfile`) resolves rule → host override → default → `mirror-client`, and the chosen
browser goes into the pool key. `connect::establish` wraps the transport in a `HelloRecorder`
(records and parses the ClientHello actually written → JA3/JA4 in
`UpstreamInfo.fingerprint`) and runs the TLS handshake with rustls (native) or, for browser
profiles, BoringSSL (`fingerprint/boring.rs`, cargo feature `impersonate`, `boring2` /
`tokio-boring2` — BoringSSL patched for extension order, record size limit, delegated
credentials, key-share limit, ALPS codepoint, ffdhe groups and 3DES suites). Certificate
verification stays in rustls/webpki: BoringSSL's custom verify callback hands the chain to
`TlsContext::verify_chain`, so trust, per-host bypass and error codes are identical; client
certificates come from the same `TlsContext`. The profile data (`fingerprint/profiles.rs`)
is plain data shared with the catalogue (`GET /api/tls-profiles`) and the tests; HTTP/2 uses
the `http2` crate (an `h2` fork that can order SETTINGS / pseudo-headers and set HEADERS
priority) in `impersonate` builds and `h2` otherwise. Browser connection windows are small
(Chrome: 15 MiB for 6 MiB streams), so `upstream/h2.rs` grows the connection window as streams
open (`ensure_connection_window`), keeping stalled streams from starving the connection.
Without the feature, browser profiles resolve to native with a `note`.

**JVM attach**: `jvm` lists JVMs with `jps -l` (process table without it), writes the
embedded agent jar (`crates/akev-intercept/java`, Java 8 bytecode, compiled from source by
`build.rs` with `javac --release 8`; official builds pin JDK 17 and set
`AKEV_AGENT_REQUIRE_JDK=1`, and a build without a JDK has no agent and reports JVM attach as
unavailable) to
`data_dir/jvm/akev-agent-<hash>.jar` and runs `java -cp <jar> dev.akev.Attach <pid> --proxy …
--ca …` with the target's own Java when possible. The helper loads the agent through the Attach
API; inside the target `agentmain` installs a default `ProxySelector`, a default `SSLContext` /
`HttpsURLConnection` factory whose trust manager accepts the Akev CA *or* the original roots,
swaps that trust manager into the previous default context (JSSE internals opened with
`Instrumentation.redefineModule`), points `javax.net.ssl.trustStore` at a combined store and
sets the proxy system properties when safe, saving every original. It writes a `key=value`
report the helper prints; `--restore` (a second attach) puts the originals back. Attached
JVMs are journaled in `data_dir/jvm/attached.json` and restored on shutdown.

**Settings**: UI `PUT /api/settings` → persisted `settings.json` → proxy `update_settings`
(live) or `rebind` (port/bind change) → store `update_settings` → broadcast `settings`.
`POST /api/proxy/rebind` moves the listener explicitly (exact port, no fallback, settings
untouched).

**Sessions & autosave**: `POST /api/sessions` streams the store into a temp file in
`data_dir/sessions/` on a blocking thread (`Store::save_session` one exchange at a time), then
renames it into place (+ sidecar). The autosave task (`akev-api/src/sessions.rs`) does the same
every `autosaveIntervalSecs` when the store's generation counter moved, and once on graceful
shutdown. Restore on startup reserves ids (`ExchangeId::reserve`) before the proxy starts and
loads the autosave in the background.

**Rules**: each exchange takes one snapshot of the rule set (`RulesEngine::snapshot`) and uses
it both to decide whether the request body must be buffered and to evaluate, so a concurrent
`PUT /api/rules` never applies half to an exchange. Compiling a rule set runs every script
rule's top-level code (bounded by the script watchdog), so the API compiles on a blocking
thread, never on an async worker.

## Performance design

Everything that grows with traffic is bounded by default (numbers in
[PERFORMANCE.md](PERFORMANCE.md)):

| Limit | Setting (default) | When it's reached |
|---|---|---|
| Exchanges kept | `capture.maxExchanges` (200,000) | oldest unpinned evicted (`stats.evicted`) |
| Body memory | `capture.memoryBudgetBytes` (512 MiB) | oldest bodies spilled to disk, zstd |
| Spill files | `capture.spillMaxBytes` (4 GiB) | oldest spilled bodies deleted, exchange kept with `BodyMeta.dropped` and a notice (`stats.diskDroppedBodies`) |
| Proxy → store queue | `capture.eventQueueBytes` (64 MiB) | new exchanges wait ≤ 250 ms, then bodies are dropped with a notice (`stats.droppedBodies`) |
| Bodies held for rules | `proxy.ruleBodyBudgetBytes` (1 GiB, all exchanges) | the body streams through untouched, notice "Body too large to apply rules; passed through" (`stats.passedThrough`) |
| Paused exchanges | `breakpoints.maxPaused` (100) | further matches don't pause, with a notice (`stats.autoReleased`) |
| Response head wait | `proxy.responseTimeoutMs` (120 s) | the exchange fails with a timeout |
| Body silence | `proxy.bodyIdleTimeoutMs` (5 min) | the exchange fails with a timeout |
| API request bodies | 2 MiB; 32 MiB rules; 1 GiB imports and Compose | 413 |
| Autosave | `sessions.autosaveMaxExchanges` / `autosaveMaxBytes` (20,000 / 256 MiB) | newest kept |

* Proxy hot path never blocks on the store: events go through a byte-bounded queue
  (`akev_core::queue`) to a dedicated ingest thread, which applies them in batches under the
  write lock and spills on the same thread (never on a Tokio worker). Bodies are tee'd while
  streaming (no full buffering unless a rule needs the body or a breakpoint/transform
  requires it).
* Readers never decompress under the store lock: body filters copy what the evaluator needs
  (summary, headers, body references) in chunks of 2,048 records, release the lock and
  evaluate the copies on several threads; body, message and export reads copy references
  (`Bytes` or spill handles) and read after releasing it. Spill segment ids are never reused,
  so a handle that outlived its blob reads an error, not someone else's bytes.
* A new stream client (or one resyncing after it fell behind) gets the newest 2,000 summaries
  in `hello` and the rest in `hello-more` chunks of 5,000; WebSocket messages are paged
  (`after` / `before` / `limit` / `tail`). The UI's list appends are amortised O(1).
* Scripts only hold back the side they hook: an `onRequest`-only script buffers the request
  but lets the response stream; an `onResponse`-only script doesn't delay the upload.
* Upstream HTTP/2 multiplexes requests over one connection per origin and opens an extra
  connection only when the existing ones are saturated (at their concurrent-stream limit).
* Upstream HTTP/1.1 connections authenticated with NTLM / Negotiate (connection-oriented auth)
  are pinned to the client connection they were authenticated for, never shared through the
  pool with another client.
* Store keeps compact summaries; bodies are reference-counted `Bytes`, spilled to disk past the
  memory budget.
* The UI receives only summaries; details & bodies are fetched on demand; filtering runs in
  Rust; the list is virtualised. Body views (decode + format + structured decode) are built on
  the blocking pool, at most three at once, so parallel views of large bodies can't multiply
  peak memory.
* HAR export streams one exchange at a time (`Store::write_har`) straight into the HTTP
  response, the MCP target file or `akev capture`'s output: memory stays flat however much
  was captured.

## Security model

Akev sees everything its clients send, so the daemon is locked down by default:

* **One secret, never exposed.** The API and MCP endpoint require the session token as an
  `Authorization: Bearer` header (or `?token=` where headers can't be set). It is never put in
  process arguments, which other local users can read: `akev start` opens the UI through a
  0600 redirect file in the data directory, and the existing-terminal one-liners pass it to curl
  as a header from a config on stdin (`-K -`). It is never accepted from a cookie (cookies
  aren't isolated by port) and never written to logs (request logs redact `token`).
* **Loopback by default.** The API and the proxy bind to `127.0.0.1`. Browser requests from any
  other origin are rejected (drive-by protection).
* **Proxy clients are vetted** (`proxy.access`, `akev_core::access`). Bound to a network
  address, the proxy admits loopback and private-network clients by default (RFC 1918,
  link-local, IPv6 ULA, CGNAT for Docker/Tailscale), or an allowlist, or anyone; blocked
  addresses are always refused. Refused connections are closed without a response and counted
  (`refusedConnections`); blocking a client also cuts its open connections, and the UI offers
  "Block" when a new device connects (`client-connected`). Optional credentials make clients
  on other machines authenticate (HTTP 407 / `Proxy-Authorization: Basic`, SOCKS5
  username/password, constant-time compare; loopback, WireGuard and redirected transparent
  clients are exempt; TLS or HTTP/2 straight to the port is refused for them). Clients on other
  machines can't make the proxy connect to this machine's loopback (`127.0.0.0/8`, `::1`,
  `0.0.0.0`, `localhost`, also after DNS; checked in `upstream/mod.rs` and `connect.rs`, with
  separate pool keys) unless `allowLoopbackTargets` is on or a user rule chose the target
  (WireGuard devices, authenticated by their key, are treated like local clients).
  PROXY protocol headers are only honoured with `trustProxyProtocol`, and then the policy
  applies to the declared client.
* **Network binds are hardened.** A non-loopback `--api-bind` requires a strong token (generated
  256-bit, or a custom one of at least 24 non-repetitive characters), protects the static UI
  with a separate UI-only session cookie, and rate-limits peers that try many wrong tokens.
  Other machines must use HTTPS (`akev-api/src/tls.rs`: the listener sniffs TLS, the
  certificate comes from Akev's CA for the bind address, `localhost`, the hostname and the
  LAN addresses, and is re-issued when the CA is regenerated); plain HTTP is answered only for
  this machine itself, so local tools keep using `http://127.0.0.1:<port>`.
* **Agents can't launch programs.** MCP may activate interceptors, but not the ones that start
  an arbitrary executable (`electron`) unless the user opted in with `--mcp-allow-launch`:
  captured traffic is attacker-influenced, so a prompt injection must not be able to run code.
* **Agents only touch files in allowed folders.** `akev_export_har` writes, and rules an agent
  creates or edits read (`file` bodies, `map-local`), only inside `<data dir>/exports` and
  folders the user passed with `--mcp-allow-path` (`mcp/files.rs`: `~` expanded, paths
  canonicalised so `..` and symlinks can't leave a root). Rules the agent passes back
  unchanged, and everything done in the UI or through the REST API, are not restricted.
* **Agents see secrets redacted.** MCP tools that return captured headers, URLs or bodies run
  them through one shared redactor (`akev_core::redact`): credential headers and cookies,
  secret query parameters, and bearer tokens / JWTs / API keys / secret-named JSON and form
  fields in text become `[redacted]`; searches run over the redacted text, so they can't
  probe a secret either. A call may pass `reveal: true` only when the user started Akev with
  `--mcp-reveal-secrets`. HAR and `.akev` exports use the same redactor on request
  (`?redact=1`, `akev export --redact`; the UI redacts HAR exports by default).
* **Electron launches are restricted** to real Electron applications (bundle / `app.asar`
  layout checked, symlinks resolved) with an allow-list of harmless switches; scripts and other
  programs are refused.
* **Scripts are sandboxed.** Rule scripts run in QuickJS with no file system or network access,
  a fresh runtime per call, a 64 MB memory cap, a time limit enforced by the interrupt handler
  plus a hard wall-clock watchdog thread, and a JS stack limit far below the native stack.

## Data directory

Linux `~/.local/share/akev`, macOS `~/Library/Application Support/akev`, Windows
`%LOCALAPPDATA%\akev`: `ca.pem`, `ca.key` (0600 on Unix, an owner-only DACL on Windows; the previous pair is
kept as `*.bak` when the user regenerates it), `settings.json`, `rules.json`, `profiles/` (browser
profiles), `spill/`, `protos/` (imported protobuf schemas under `files/`, URL mappings in
`mappings.json`), `sessions/` (saved sessions and the autosave: `<id>.akev` + `<id>.json`),
`jvm/` (the JVM agent jar and the attached-JVM journal), `license.json` (0600: Akev Pro licence
key and trial start; never part of `settings.json` or settings exports).

`akev.lock` + `instance.json`: one instance per data directory. An OS file lock taken at
startup keeps the desktop app and the daemon from clobbering each other's settings, rules and
autosave; `instance.json` names the holder (pid, kind, version, API URL) for the second
instance's error. Ephemeral sessions skip it. `logs/`: `akev.log`, rotated at 5 MB with three
files kept, written by the daemon and the desktop app (not by short CLI commands). `crashes/`:
`crash-<UTC time>.txt` from the panic hook (version, OS, thread, panic, backtrace, the last log
lines; tokens and credentials scrubbed; 30 kept). `GET /api/diagnostics` reports the version,
OS, counters, a settings summary without secrets, recent warnings and crash report names for
bug reports (`akev-api/src/observability.rs`, `diagnostics.rs`, `datadir_lock.rs`).

## Licensing (Akev Pro)

`akev-api/src/license.rs` verifies licence keys offline (`AKEV1.<payload>.<Ed25519 signature>`,
public keys built in) and tracks the 14-day trial; `akev-core::license` holds the `Feature`
list and `LicenseStatus` (`GET/PUT/DELETE /api/license`, `POST /api/license/trial`,
`Status.license`, stream message `license`). A licence covers builds whose `RELEASE_DATE` is on
or before its `updatesUntil`; nothing expires at run time. Enforcement never rewrites stored
data:

* **Rules** — the rules engine skips enabled rules whose action type is locked (`script`,
  `tls-profile`) and reports them as rule errors; the rule set is kept as saved.
* **Proxy settings** — `App::effective_proxy` switches WireGuard, the transparent listener and
  browser TLS profiles off in what the proxy and the WireGuard server run with;
  `App::update_settings` answers 402 `pro-required` only when a change turns one *on*.
* **Routes** — 402 `pro-required` (with `feature`) from protobuf schema uploads/mappings, the
  LLM conversation view, transparent capture plans, and activating the `wireguard`, `jvm` and
  `transparent-linux` interceptors. Protobuf schemas stay stored but aren't applied.
* **MCP** — tools that change state (and `akev_llm_conversation`) return a tool error; reading,
  searching and exporting stay free. Deactivating interceptors is always allowed.
* `App::apply_license` re-applies all of the above live when the key or trial changes (and when
  the trial ends).

## Conventions

* Rust 2024 edition, `cargo clippy -- -D warnings` clean, `cargo fmt`. No `unwrap()`/`expect()`
  on runtime-fallible paths outside tests. `tracing` for logs. Errors via `thiserror` in libs,
  `anyhow` in binaries.
* Public APIs documented; every crate has unit tests; `akev-api` has end-to-end tests that run
  real traffic through the proxy.
