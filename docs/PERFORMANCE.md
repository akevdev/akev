# Performance

Measured numbers behind the scale claims, and how to reproduce them. Everything that grows with
traffic is bounded by default; the limits and what happens when one is reached are listed in
[ARCHITECTURE.md](ARCHITECTURE.md#performance-design).

## Method

* Machine: Intel Core i7-6700 (4 cores, 8 threads, 3.4 GHz, 2015), 31 GiB RAM, Fedora Linux 45.
  The machine was **not idle**: other builds kept the load average between 11 and 18 on its 8
  threads during every run. Treat throughput and latency as lower bounds; memory is unaffected.
* Build: release, but without LTO (`CARGO_PROFILE_RELEASE_LTO=false
  CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16`), so the shipped binaries are a little faster.
* Memory is the process's resident set size (`VmRSS` in `/proc/<pid>/status`), so it includes
  the allocator's free lists, not just live data.
* Typical exchange: a `GET` with browser-like request headers and a JSON response of about
  1 KB with six response headers.

### Store benchmark

`crates/akev-store/tests/scale.rs` ingests exchanges straight into the store (no network) and
times the queries the UI and API make. Run each benchmark alone, in release mode:

```sh
cargo test -p akev-store --release --test scale -- --ignored --nocapture --test-threads=1
```

`bounded_memory` in the same file runs with every `cargo test` (debug build): 20,000 exchanges
into a 15,000 cap must keep the store's body bytes within the memory budget and grow RSS by
less than 10 KB per ingested exchange (about twice what is measured), so a leak or unbounded
buffering fails CI.

### End to end

The release `akev` daemon (ephemeral data directory, default settings: 200,000 exchange cap,
autosave on) with `akev-loadgen` sending requests through the proxy to its built-in origin, then
queries timed with `curl` against the HTTP API (best of three, JSON serialisation included):

```sh
akev --token <token> start --ephemeral --no-open --proxy-port 18000 --api-port 18456
akev-loadgen --proxy http://127.0.0.1:18000 --requests 100000 --concurrency 64 --body-size 1024
akev-loadgen --proxy http://127.0.0.1:18000 --requests 200000 --concurrency 64 --body-size 1024
```

## Results

### Memory

| What | RSS |
|---|---|
| Idle daemon | 22 MiB |
| 100k exchanges, end to end | 360 MiB (about 3.7 KB per exchange, 1 KB body included) |
| 100k exchanges, store only, 32 MiB body budget (64 MiB spilled to 5 MiB of zstd files) | 322 MiB (3.3 KB per exchange) |
| 300k requests end to end, 200k kept (default cap) | 909 MiB (100k evicted) |
| 1M exchanges into the default cap (store only) | 773 MiB with 200k kept, 800k evicted |
| 1M exchanges, no cap (store only) | 3.3 GiB (3.5 KB per exchange) |

With the default cap a long session levels off at the size of 200,000 exchanges: about
750–900 MB for typical API traffic. `capture.maxExchanges` scales it linearly (100,000 keeps it
under 400 MB). Bodies never take more than `capture.memoryBudgetBytes` (512 MiB) of it; beyond
that they are compressed to spill files, themselves capped at `capture.spillMaxBytes` (4 GiB).

### Throughput

| What | Rate |
|---|---|
| Through the proxy, 64 concurrent clients, 1 KB responses | 17,600 req/s for 100k (p50 2.6 ms, p99 13.9 ms); 15,700 req/s for 200k more at the cap (p50 2.7 ms, p99 16.6 ms); 0 failures |
| Store ingest, 4 events per exchange | 43,000 exchanges/s (100k, bodies spilling); 60,000/s (1M into the cap, evicting) |

No body was dropped by the bounded event queue in these runs (`droppedBodies` stayed 0). The
throughput depends heavily on what else the machine is doing: a later 100k run while other
builds were running gave 5,800 req/s (p99 32 ms), with the same 362 MiB RSS.

### Query latency

Through the HTTP API, daemon holding 100k exchanges (bodies in memory):

| Query | Time |
|---|---|
| `GET /api/status` | < 1 ms |
| Newest 100 summaries | 1 ms |
| All 100k summaries | 142 ms |
| Summary filter, full scan (`path:/item/99999`) | 19 ms |
| Body filter, newest 100 matches (`resbody:ok`) | 5 ms |
| Body filter, full scan, no match | 220 ms (466 ms over 200k) |

Store only (no HTTP), 100k exchanges with most bodies spilled / 200k at the cap / 1M uncapped:

| Query | 100k | 200k | 1M |
|---|---|---|---|
| All summaries (what a `hello` used to send) | 183 ms | 328 ms | 1.9 s |
| `host:` filter | 26 ms | 53 ms | 347 ms |
| `status:404 method:GET` | 14 ms | 30 ms | 171 ms |
| Body filter, newest 500 matches | 21 ms | 29 ms | 20 ms |
| Body filter, full scan, no match | 1.65 s (decompressing) | 3.2 s | 14 s |
| Filter suggestions | 147 ms | 257 ms | 1.1 s |
| HAR export of 1,000 exchanges | 82 ms | 137 ms | 91 ms |

A UI that connects (or resyncs) no longer receives every summary at once: `hello` carries the
newest 2,000 and the rest follow in chunks of 5,000, so the first screen is ready long before a
large store has been sent. Full-scan body filters don't hold the store lock while they read and
decompress bodies, so capture and other requests carry on meanwhile.

## Earlier figures

The README and PRODUCT.md previously quoted "100k exchanges in ~350 MB at ~28k req/s, p99
4.5 ms". The memory figure holds (322–360 MiB above); the throughput figure was measured on an
idle machine and is not reproduced by the loaded runs above.
