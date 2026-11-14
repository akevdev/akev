//! `akev-loadgen` — fire lots of requests through an Akev proxy at a local origin server.
//!
//! ```text
//! cargo run --release -p akev-loadgen -- --proxy http://127.0.0.1:8000 --requests 100000 --concurrency 64
//! ```
//! Starts an in-process origin server (HTTP/1.1, keep-alive) returning JSON bodies of
//! `--body-size` bytes, then sends `--requests` GETs (or POSTs with `--post`) through the proxy
//! and prints throughput and latency percentiles.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use clap::Parser;
use futures::StreamExt;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;

#[derive(Parser, Debug)]
#[command(about = "Benchmark the Akev proxy")]
struct Args {
    /// Proxy URL (e.g. http://127.0.0.1:8000). Omit to hit the origin directly (baseline).
    #[arg(long)]
    proxy: Option<String>,
    /// Total requests to send.
    #[arg(long, default_value_t = 10_000)]
    requests: u64,
    /// Concurrent in-flight requests.
    #[arg(long, default_value_t = 32)]
    concurrency: usize,
    /// Response body size in bytes.
    #[arg(long, default_value_t = 1024)]
    body_size: usize,
    /// Send POST requests with a body of this size instead of GETs.
    #[arg(long)]
    post: Option<usize>,
    /// Use an external origin instead of the built-in one (e.g. http://localhost:3000/).
    #[arg(long)]
    origin: Option<String>,
}

async fn start_origin(body_size: usize) -> anyhow::Result<SocketAddr> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let filler = "x".repeat(body_size.saturating_sub(32));
    let body = Bytes::from(format!("{{\"ok\":true,\"pad\":\"{filler}\"}}"));
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let body = body.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |_req: Request<hyper::body::Incoming>| {
                    let body = body.clone();
                    async move {
                        Ok::<_, Infallible>(
                            Response::builder()
                                .header("content-type", "application/json")
                                .body(Full::new(body))
                                .unwrap_or_default(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .keep_alive(true)
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    Ok(addr)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let origin = match &args.origin {
        Some(o) => o.clone(),
        None => format!("http://{}/", start_origin(args.body_size).await?),
    };

    let mut builder = reqwest::Client::builder()
        .pool_max_idle_per_host(args.concurrency)
        .timeout(Duration::from_secs(30));
    builder = match &args.proxy {
        Some(p) => builder.proxy(reqwest::Proxy::all(p)?),
        None => builder.no_proxy(),
    };
    let client = builder.build()?;

    let ok = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let post_body = args.post.map(|n| Bytes::from(vec![b'a'; n]));
    let started = Instant::now();

    let latencies: Vec<Duration> = futures::stream::iter(0..args.requests)
        .map(|i| {
            let client = client.clone();
            let url = format!("{origin}item/{i}?q={i}");
            let ok = ok.clone();
            let failed = failed.clone();
            let post_body = post_body.clone();
            async move {
                let t = Instant::now();
                let req = match post_body {
                    Some(b) => client.post(&url).body(b),
                    None => client.get(&url),
                };
                match req.send().await {
                    Ok(resp) => match resp.bytes().await {
                        Ok(_) => {
                            ok.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(_) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                        }
                    },
                    Err(_) => {
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
                t.elapsed()
            }
        })
        .buffer_unordered(args.concurrency)
        .collect()
        .await;

    let elapsed = started.elapsed();
    let mut sorted = latencies;
    sorted.sort();
    let pct = |p: f64| -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
        sorted[idx].as_secs_f64() * 1000.0
    };
    let okc = ok.load(Ordering::Relaxed);
    println!(
        "target      : {}",
        args.proxy.as_deref().unwrap_or("direct (no proxy)")
    );
    println!(
        "requests    : {} ok, {} failed",
        okc,
        failed.load(Ordering::Relaxed)
    );
    println!("elapsed     : {:.2}s", elapsed.as_secs_f64());
    println!(
        "throughput  : {:.0} req/s",
        okc as f64 / elapsed.as_secs_f64()
    );
    println!(
        "latency ms  : p50 {:.2}  p90 {:.2}  p99 {:.2}  max {:.2}",
        pct(0.5),
        pct(0.9),
        pct(0.99),
        pct(1.0)
    );
    Ok(())
}
