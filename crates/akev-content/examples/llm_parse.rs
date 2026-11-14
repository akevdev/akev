//! Run the LLM traffic parser on captured bodies and print the normalised conversation.
//!
//! ```text
//! cargo run -p akev-content --example llm_parse -- \
//!     host=api.openai.com path=/v1/chat/completions request=req.json \
//!     [response=resp.sse] [status=200] [content-type=text/event-stream] [port=443] \
//!     [event-step=20]
//! ```
//!
//! `event-step=N` gives the n-th SSE event of the response the arrival time `n * N` ms (as if
//! recorded by the proxy), so the time to first token is reported in those units.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_content::llm::{LlmInput, StreamEvent, parse};
use std::collections::HashMap;

fn main() {
    let args: HashMap<String, String> = std::env::args()
        .skip(1)
        .filter_map(|a| {
            a.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect();
    let arg = |k: &str| args.get(k).map(String::as_str);
    let read = |k: &str| {
        arg(k)
            .map(|p| std::fs::read(p).unwrap_or_else(|e| panic!("cannot read {p}: {e}")))
            .unwrap_or_default()
    };
    let (Some(host), Some(path)) = (arg("host"), arg("path")) else {
        eprintln!(
            "usage: llm_parse host=H path=P request=FILE [response=FILE] [status=N] [content-type=CT] [port=N] [event-step=MS]"
        );
        std::process::exit(2);
    };
    let request = read("request");
    let response = read("response");
    let events: Vec<StreamEvent> = match arg("event-step").and_then(|s| s.parse::<f64>().ok()) {
        Some(step) => akev_content::sse::parse_all(&response)
            .into_iter()
            .enumerate()
            .map(|(i, e)| StreamEvent {
                event: e.event,
                data: e.data,
                at_ms: Some(i as f64 * step),
            })
            .collect(),
        None => Vec::new(),
    };
    let has_response = arg("response").is_some();
    let input = LlmInput {
        host,
        port: arg("port").and_then(|p| p.parse().ok()).unwrap_or(443),
        path,
        request_body: &request,
        response_status: has_response
            .then(|| arg("status").and_then(|s| s.parse().ok()).unwrap_or(200)),
        response_content_type: arg("content-type"),
        response_body: &response,
        response_truncated: false,
        events: &events,
        sent_ms: Some(0.0),
        done_ms: has_response.then_some(1000.0),
        finished: has_response,
    };
    match parse(&input) {
        Some(x) => println!("{}", serde_json::to_string_pretty(&x).unwrap_or_default()),
        None => {
            eprintln!("not a recognised LLM API call");
            std::process::exit(1);
        }
    }
}
