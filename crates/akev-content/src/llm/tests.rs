use super::*;
use serde_json::json;

macro_rules! fx {
    ($name:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/llm/",
            $name
        ))
    };
}

const DONE: Option<f64> = Some(2000.0);

/// A finished exchange with a JSON / SSE response.
fn input<'a>(
    host: &'a str,
    port: u16,
    path: &'a str,
    request: &'a str,
    status: Option<u16>,
    content_type: Option<&'a str>,
    response: &'a str,
) -> LlmInput<'a> {
    LlmInput {
        host,
        port,
        path,
        request_body: request.as_bytes(),
        response_status: status,
        response_content_type: content_type,
        response_body: response.as_bytes(),
        response_truncated: false,
        events: &[],
        sent_ms: Some(100.0),
        done_ms: DONE,
        finished: true,
    }
}

/// The SSE events of `body`, the n-th arriving at `first + n * step` ms.
fn timed(body: &str, first: f64, step: f64) -> Vec<StreamEvent> {
    crate::sse::parse_all(body.as_bytes())
        .into_iter()
        .enumerate()
        .map(|(i, e)| StreamEvent {
            event: e.event,
            data: e.data,
            at_ms: Some(first + i as f64 * step),
        })
        .collect()
}

fn text_of(m: &LlmMessage) -> String {
    m.content
        .iter()
        .filter_map(|p| match p {
            LlmPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn tool_calls(m: &LlmMessage) -> Vec<(Option<String>, String, Value)> {
    m.content
        .iter()
        .filter_map(|p| match p {
            LlmPart::ToolCall {
                id,
                name,
                arguments,
            } => Some((id.clone(), name.clone(), arguments.clone())),
            _ => None,
        })
        .collect()
}

fn thinking(m: &LlmMessage) -> String {
    m.content
        .iter()
        .filter_map(|p| match p {
            LlmPart::Thinking { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn response(x: &LlmExchange) -> &LlmResponse {
    x.response.as_ref().expect("a response")
}

// =============================================================================================
// Recognition

#[test]
fn routes_by_host_and_path() {
    let r = route("api.openai.com", 443, "/v1/chat/completions").unwrap();
    assert_eq!(
        (r.provider, r.api, r.trusted),
        ("openai", LlmApi::OpenAiChat, true)
    );
    let r = route("api.openai.com", 443, "/v1/responses?x=1").unwrap();
    assert_eq!(r.api, LlmApi::OpenAiResponses);
    let r = route("api.openai.com", 443, "/v1/completions").unwrap();
    assert_eq!(r.api, LlmApi::OpenAiCompletions);
    let r = route("api.anthropic.com", 443, "/v1/messages").unwrap();
    assert_eq!(
        (r.provider, r.api),
        ("anthropic", LlmApi::AnthropicMessages)
    );
    // Anthropic's OpenAI-compatible endpoint: provider by host, API by path.
    let r = route("api.anthropic.com", 443, "/v1/chat/completions").unwrap();
    assert_eq!((r.provider, r.api), ("anthropic", LlmApi::OpenAiChat));
    let r = route(
        "generativelanguage.googleapis.com",
        443,
        "/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse",
    )
    .unwrap();
    assert_eq!((r.provider, r.api), ("google", LlmApi::Gemini));
    assert_eq!(r.path_model.as_deref(), Some("gemini-2.5-flash"));
    assert!(r.path_stream);
    let r = route(
        "europe-west4-aiplatform.googleapis.com",
        443,
        "/v1/projects/p/locations/europe-west4/publishers/google/models/gemini-2.5-pro:generateContent",
    )
    .unwrap();
    assert_eq!((r.provider, r.api), ("vertex-ai", LlmApi::Gemini));
    assert_eq!(r.path_model.as_deref(), Some("gemini-2.5-pro"));
    assert!(!r.path_stream);
    let r = route(
        "us-east5-aiplatform.googleapis.com",
        443,
        "/v1/projects/p/locations/us-east5/publishers/anthropic/models/claude-sonnet-4-5@20250929:streamRawPredict",
    )
    .unwrap();
    assert_eq!(
        (r.provider, r.api),
        ("vertex-ai", LlmApi::AnthropicMessages)
    );
    assert_eq!(r.path_model.as_deref(), Some("claude-sonnet-4-5@20250929"));
    assert!(r.path_stream);
    let r = route(
        "acme.openai.azure.com",
        443,
        "/openai/deployments/gpt-4o-prod/chat/completions?api-version=2024-10-21",
    )
    .unwrap();
    assert_eq!((r.provider, r.api), ("azure-openai", LlmApi::OpenAiChat));
    assert_eq!(r.path_model.as_deref(), Some("gpt-4o-prod"));
    for (host, provider) in [
        ("openrouter.ai", "openrouter"),
        ("api.groq.com", "groq"),
        ("api.mistral.ai", "mistral"),
        ("api.deepseek.com", "deepseek"),
        ("api.x.ai", "xai"),
        ("api.together.xyz", "together"),
        ("api.fireworks.ai", "fireworks"),
        ("API.PERPLEXITY.AI", "perplexity"),
    ] {
        let r = route(host, 443, "/api/v1/chat/completions").unwrap();
        assert_eq!(r.provider, provider, "{host}");
        assert!(r.trusted);
    }
    // Local inference servers: OpenAI-compatible, by port.
    let r = route("localhost", 11434, "/v1/chat/completions").unwrap();
    assert_eq!((r.provider, r.trusted), ("ollama", false));
    let r = route("127.0.0.1", 1234, "/v1/chat/completions").unwrap();
    assert_eq!(r.provider, "lmstudio");
    let r = route("10.0.0.7", 8000, "/v1/chat/completions").unwrap();
    assert_eq!(r.provider, "openai-compatible");
    let r = route("llm-gateway.internal", 443, "/v1/messages").unwrap();
    assert_eq!(r.provider, "anthropic-compatible");
    // Ollama's native API only on Ollama hosts.
    let r = route("localhost", 11434, "/api/chat").unwrap();
    assert_eq!((r.provider, r.api), ("ollama", LlmApi::OllamaChat));
    let r = route("ollama", 80, "/api/generate").unwrap();
    assert_eq!(r.api, LlmApi::OllamaGenerate);
    assert!(route("example.com", 443, "/api/chat").is_none());
    // Not generation endpoints.
    for (host, path) in [
        ("api.openai.com", "/v1/embeddings"),
        ("api.openai.com", "/v1/models"),
        ("api.openai.com", "/v1/responses/resp_123"),
        ("api.anthropic.com", "/v1/messages/count_tokens"),
        ("api.anthropic.com", "/v1/messages/batches"),
        (
            "generativelanguage.googleapis.com",
            "/v1beta/models/gemini-2.5-flash:countTokens",
        ),
        ("example.com", "/"),
        ("example.com", "/v1/users:search"),
    ] {
        assert!(route(host, 443, path).is_none(), "{host}{path}");
    }
    // Host suffix matching is on label boundaries.
    assert!(
        route("notopenai.azure.com.evil", 443, "/chat/completions").is_some_and(|r| !r.trusted)
    );
    assert!(route("evilopenai.azure.com", 443, "/chat/completions").is_some_and(|r| !r.trusted));
}

#[test]
fn tags_with_model() {
    // Well-known host: tagged from the path alone, model from the body when there is one.
    assert_eq!(
        tag("api.openai.com", 443, "/v1/chat/completions", None),
        Some(("openai", None))
    );
    assert_eq!(
        tag(
            "api.openai.com",
            443,
            "/v1/chat/completions",
            Some(fx!("openai_chat_tools_request.json").as_bytes())
        ),
        Some(("openai", Some("gpt-4.1".into())))
    );
    // A body that doesn't parse doesn't untag a well-known host.
    assert_eq!(
        tag("api.anthropic.com", 443, "/v1/messages", Some(b"{not json")),
        Some(("anthropic", None))
    );
    // Gemini: model from the path.
    assert_eq!(
        tag(
            "generativelanguage.googleapis.com",
            443,
            "/v1beta/models/gemini-2.5-flash:generateContent",
            Some(fx!("gemini_request.json").as_bytes())
        ),
        Some(("google", Some("gemini-2.5-flash".into())))
    );
    // Azure: the deployment stands in for a missing model.
    assert_eq!(
        tag(
            "acme.openai.azure.com",
            443,
            "/openai/deployments/gpt-4o-prod/chat/completions",
            Some(br#"{"messages":[]}"#)
        ),
        Some(("azure-openai", Some("gpt-4o-prod".into())))
    );
    // Unknown hosts need a body of the right shape.
    assert_eq!(tag("localhost", 11434, "/v1/chat/completions", None), None);
    assert_eq!(
        tag(
            "localhost",
            11434,
            "/v1/chat/completions",
            Some(fx!("ollama_compat_request.json").as_bytes())
        ),
        Some(("ollama", Some("llama3.2:3b".into())))
    );
    assert_eq!(
        tag(
            "app.example.com",
            443,
            "/v1/messages",
            Some(br#"{"messages":"hi"}"#)
        ),
        None
    );
    assert_eq!(
        tag(
            "app.example.com",
            443,
            "/v1/messages",
            Some(br#"{"to":"bob","messages":[{"text":"hi"}]}"#)
        ),
        None,
        "a chat app's messages endpoint isn't Anthropic"
    );
    assert_eq!(
        tag(
            "gw.example.com",
            443,
            "/v1/messages",
            Some(br#"{"model":"claude-x","max_tokens":5,"messages":[]}"#)
        ),
        Some(("anthropic-compatible", Some("claude-x".into())))
    );
    // Non-string models are ignored, not an error; BOM and whitespace are fine.
    assert_eq!(
        tag(
            "api.openai.com",
            443,
            "/v1/chat/completions",
            Some(b"\xEF\xBB\xBF  {\"model\": 5, \"messages\": []}")
        ),
        Some(("openai", None))
    );
    assert_eq!(
        tag(
            "localhost",
            8080,
            "/v1/completions",
            Some(br#"{"model":"m","prompt":["a","b"]}"#)
        ),
        Some(("openai-compatible", Some("m".into())))
    );
    assert_eq!(
        tag("localhost", 8080, "/v1/completions", Some(b"[1,2]")),
        None
    );
}

#[test]
fn unrecognised_exchanges_are_none() {
    let x = input(
        "example.com",
        443,
        "/api/users",
        "{}",
        Some(200),
        None,
        "{}",
    );
    assert!(parse(&x).is_none());
    // Unknown host whose body isn't an LLM request.
    let x = input(
        "chat.example.com",
        443,
        "/v1/messages",
        r#"{"to":"bob","messages":[]}"#,
        Some(200),
        None,
        "{}",
    );
    assert!(parse(&x).is_none());
}

// =============================================================================================
// OpenAI Chat Completions

#[test]
fn openai_streamed_parallel_tool_calls() {
    let events = timed(fx!("openai_chat_tools_stream.sse"), 500.0, 20.0);
    let mut x = input(
        "api.openai.com",
        443,
        "/v1/chat/completions",
        fx!("openai_chat_tools_request.json"),
        Some(200),
        Some("text/event-stream; charset=utf-8"),
        "",
    );
    x.events = &events;
    let out = parse(&x).unwrap();
    assert_eq!(out.provider, "openai");
    assert_eq!(out.api, LlmApi::OpenAiChat);
    assert!(out.stream);
    assert_eq!(out.model.as_deref(), Some("gpt-4.1-2025-04-14"));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    // Request: system hoisted, user message, tools, params.
    let req = &out.request;
    assert!(req.system.as_deref().unwrap().starts_with("You are Atlas"));
    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].role, "user");
    assert!(text_of(&req.messages[0]).contains("umbrella"));
    assert_eq!(req.tools.len(), 2);
    assert_eq!(req.tools[0].name, "get_weather");
    assert_eq!(
        req.tools[0].schema.as_ref().unwrap()["required"],
        json!(["city", "date"])
    );
    assert_eq!(req.params["temperature"], json!(0.3));
    assert_eq!(req.params["tool_choice"], json!("auto"));
    assert!(req.params.contains_key("stream_options"));
    assert!(!req.params.contains_key("messages") && !req.params.contains_key("stream"));
    // Response: two tool calls with assembled arguments.
    let r = response(&out);
    assert_eq!(r.id.as_deref(), Some("chatcmpl-CQx3eTq8Ld2Z"));
    assert_eq!(r.stop_reason.as_deref(), Some("tool_calls"));
    assert_eq!(r.messages.len(), 1);
    let calls = tool_calls(&r.messages[0]);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0.as_deref(), Some("call_Wx9fT2qLm4Hc"));
    assert_eq!(calls[0].1, "get_weather");
    assert_eq!(
        calls[0].2,
        json!({"city": "Ljubljana", "date": "2026-11-10", "units": "metric"})
    );
    assert_eq!(calls[1].1, "search_places");
    assert_eq!(calls[1].2["open_now"], json!(false));
    let usage = r.usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, Some(412));
    assert_eq!(usage.output_tokens, Some(61));
    assert_eq!(usage.cache_read_tokens, None);
    // Timing: first content in the 2nd event (index 1), sent at 100 ms.
    assert_eq!(out.timing.ttft_ms, Some(500.0 + 20.0 - 100.0));
    assert_eq!(out.timing.total_ms, Some(1900.0));
    assert_eq!(out.timing.chunks, Some(13));
    let tps = out.timing.tokens_per_second.unwrap();
    assert!(
        (tps - 61.0 / ((1900.0 - 420.0) / 1000.0)).abs() < 0.01,
        "{tps}"
    );
}

#[test]
fn openai_streamed_answer_after_tool_results() {
    let mut x = input(
        "api.openai.com",
        443,
        "/v1/chat/completions",
        fx!("openai_chat_final_request.json"),
        Some(200),
        Some("text/event-stream"),
        fx!("openai_chat_final_stream.sse"),
    );
    x.sent_ms = None;
    let out = parse(&x).unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let msgs = &out.request.messages;
    assert_eq!(
        msgs.iter().map(|m| m.role.as_str()).collect::<Vec<_>>(),
        ["user", "assistant", "tool", "tool"]
    );
    let calls = tool_calls(&msgs[1]);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].2["query"], json!("indoor museums and markets"));
    match &msgs[2].content[0] {
        LlmPart::ToolResult { id, content, .. } => {
            assert_eq!(id.as_deref(), Some("call_Wx9fT2qLm4Hc"));
            assert!(matches!(&content[0], LlmPart::Text { text } if text.contains("Light rain")));
        }
        other => panic!("{other:?}"),
    }
    let r = response(&out);
    let text = text_of(&r.messages[0]);
    assert!(text.starts_with("**Yes — pack an umbrella.**"), "{text}");
    assert!(text.ends_with("Want me to check Sunday's opening hours as well?"));
    assert!(text.contains("Plečnik"));
    assert_eq!(
        r.messages[0].content.len(),
        1,
        "deltas merge into one text part"
    );
    assert_eq!(r.stop_reason.as_deref(), Some("stop"));
    let usage = r.usage.as_ref().unwrap();
    assert_eq!(usage.cache_read_tokens, Some(384));
    assert_eq!(usage.total_tokens, Some(816));
    // Parsed from the body (no recorded events): no TTFT; total from the start.
    assert_eq!(out.timing.ttft_ms, None);
    assert_eq!(out.timing.total_ms, Some(2000.0));
    assert_eq!(out.timing.tokens_per_second, Some(59.0));
}

#[test]
fn openai_non_streamed_multimodal_with_reasoning_usage() {
    let x = input(
        "api.openai.com",
        443,
        "/v1/chat/completions",
        fx!("openai_chat_request_multimodal.json"),
        Some(200),
        Some("application/json"),
        fx!("openai_chat_response.json"),
    );
    let out = parse(&x).unwrap();
    assert!(!out.stream);
    assert_eq!(
        out.request.system.as_deref(),
        Some("Answer with one sentence.")
    );
    let user = &out.request.messages[0];
    assert_eq!(user.content.len(), 3);
    match &user.content[1] {
        LlmPart::Image {
            media_type,
            data,
            size,
            url,
        } => {
            assert_eq!(media_type.as_deref(), Some("image/png"));
            assert!(data.as_deref().unwrap().starts_with("iVBORw0KGgo"));
            assert_eq!(*size, Some(204));
            assert!(url.is_none());
        }
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(&user.content[2], LlmPart::Image { url: Some(u), data: None, .. } if u == "https://example.com/receipt.jpg")
    );
    assert_eq!(out.request.params["reasoning_effort"], json!("low"));
    let r = response(&out);
    assert_eq!(out.model.as_deref(), Some("o4-mini-2025-04-16"));
    assert_eq!(
        text_of(&r.messages[0]),
        "A checkout summary whose total row is highlighted in red."
    );
    let u = r.usage.as_ref().unwrap();
    assert_eq!(
        (
            u.input_tokens,
            u.output_tokens,
            u.cache_read_tokens,
            u.reasoning_tokens
        ),
        (Some(1210), Some(214), Some(1024), Some(192))
    );
    // Not streamed: throughput over the whole duration.
    assert_eq!(out.timing.tokens_per_second, Some(round2(214.0 / 1.9)));
}

#[test]
fn openai_reasoning_deltas_refusals_and_errors() {
    let body = concat!(
        "data: {\"id\":\"c1\",\"model\":\"deepseek-reasoner\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"Think \"}}]}\n\n",
        "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"hard.\"}}]}\n\n",
        "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Done\"}}]}\n\n",
        "data: not json\n\n",
        "data: {\"error\":{\"message\":\"Internal error\",\"type\":\"server_error\",\"code\":\"internal\"}}\n\n",
    );
    let x = input(
        "api.deepseek.com",
        443,
        "/chat/completions",
        r#"{"model":"deepseek-reasoner","messages":[{"role":"user","content":"hi"}],"stream":true}"#,
        Some(200),
        Some("text/event-stream"),
        body,
    );
    let out = parse(&x).unwrap();
    assert_eq!(out.provider, "deepseek");
    let r = response(&out);
    assert_eq!(thinking(&r.messages[0]), "Think hard.");
    assert_eq!(text_of(&r.messages[0]), "Done");
    let e = out.error.as_ref().unwrap();
    assert_eq!(
        (e.kind.as_deref(), e.message.as_str()),
        (Some("server_error"), "Internal error")
    );
    assert!(
        out.warnings.iter().any(|w| w.contains("malformed")),
        "{:?}",
        out.warnings
    );
    // An error event explains the missing end: no "cut short" warning.
    assert!(!out.warnings.iter().any(|w| w.contains("final event")));

    // A refusal in a non-streamed response.
    let x = input(
        "api.openai.com",
        443,
        "/v1/chat/completions",
        r#"{"model":"gpt-4o","messages":[{"role":"user","content":[{"type":"text","text":"a"},{"type":"text","text":"b"},{"type":"input_audio","input_audio":{"data":"AAAA","format":"wav"}}]}]}"#,
        Some(200),
        None,
        r#"{"choices":[{"message":{"role":"assistant","content":null,"refusal":"I can't help with that."},"finish_reason":"stop"}]}"#,
    );
    let out = parse(&x).unwrap();
    assert_eq!(text_of(&out.request.messages[0]), "a\n\nb");
    assert!(
        matches!(&out.request.messages[0].content[1], LlmPart::File { label, media_type: Some(m), size: Some(3), .. } if label == "audio" && m == "audio/wav")
    );
    assert!(
        matches!(&response(&out).messages[0].content[0], LlmPart::Refusal { text } if text == "I can't help with that.")
    );
}

#[test]
fn openai_http_errors() {
    let x = input(
        "api.openai.com",
        443,
        "/v1/chat/completions",
        r#"{"model":"gpt-4o","messages":[]}"#,
        Some(429),
        Some("application/json"),
        r#"{"error":{"message":"Rate limit reached for gpt-4o in organization org-x on tokens per min (TPM).","type":"tokens","param":null,"code":"rate_limit_exceeded"}}"#,
    );
    let out = parse(&x).unwrap();
    assert!(out.response.is_none());
    let e = out.error.unwrap();
    assert_eq!(e.status, Some(429));
    assert_eq!(e.code.as_deref(), Some("rate_limit_exceeded"));
    assert!(e.message.starts_with("Rate limit reached"));
    // An HTML error page from a gateway.
    let x = input(
        "api.openai.com",
        443,
        "/v1/chat/completions",
        r#"{"model":"gpt-4o","messages":[]}"#,
        Some(502),
        Some("text/html"),
        "<html><body>502 Bad Gateway</body></html>",
    );
    let e = parse(&x).unwrap().error.unwrap();
    assert_eq!(e.status, Some(502));
    assert!(e.message.contains("Bad Gateway"));
    // No body at all.
    let x = input(
        "api.openai.com",
        443,
        "/v1/chat/completions",
        "{}",
        Some(500),
        None,
        "",
    );
    assert_eq!(parse(&x).unwrap().error.unwrap().message, "HTTP 500");
}

#[test]
fn openai_legacy_completions() {
    let req = r#"{"model":"gpt-3.5-turbo-instruct","prompt":"Say this is a test","max_tokens":7,"temperature":0}"#;
    let x = input(
        "api.openai.com",
        443,
        "/v1/completions",
        req,
        Some(200),
        None,
        r#"{"id":"cmpl-1","object":"text_completion","model":"gpt-3.5-turbo-instruct","choices":[{"text":"\n\nThis is indeed a test","index":0,"finish_reason":"length"}],"usage":{"prompt_tokens":5,"completion_tokens":7,"total_tokens":12}}"#,
    );
    let out = parse(&x).unwrap();
    assert_eq!(out.api, LlmApi::OpenAiCompletions);
    assert_eq!(text_of(&out.request.messages[0]), "Say this is a test");
    assert_eq!(out.request.params["max_tokens"], json!(7));
    assert_eq!(
        text_of(&response(&out).messages[0]),
        "\n\nThis is indeed a test"
    );
    assert_eq!(response(&out).stop_reason.as_deref(), Some("length"));
    let stream = concat!(
        "data: {\"id\":\"cmpl-2\",\"model\":\"m\",\"choices\":[{\"text\":\"Hel\",\"index\":0,\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"cmpl-2\",\"model\":\"m\",\"choices\":[{\"text\":\"lo\",\"index\":0,\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let x = input(
        "localhost",
        8000,
        "/v1/completions",
        r#"{"model":"m","prompt":["a"],"stream":true}"#,
        Some(200),
        Some("text/event-stream"),
        stream,
    );
    let out = parse(&x).unwrap();
    assert_eq!(out.provider, "openai-compatible");
    assert_eq!(text_of(&response(&out).messages[0]), "Hello");
    assert!(out.warnings.is_empty());
}

// =============================================================================================
// OpenAI Responses

#[test]
fn responses_request_and_response() {
    let x = input(
        "api.openai.com",
        443,
        "/v1/responses",
        fx!("openai_responses_request.json"),
        Some(200),
        Some("application/json"),
        fx!("openai_responses_response.json"),
    );
    let out = parse(&x).unwrap();
    assert_eq!(out.api, LlmApi::OpenAiResponses);
    assert!(out.stream, "the request asked for a stream");
    let req = &out.request;
    assert_eq!(
        req.system.as_deref(),
        Some("You are a helpful assistant. Use tools when needed.")
    );
    assert_eq!(
        req.messages
            .iter()
            .map(|m| m.role.as_str())
            .collect::<Vec<_>>(),
        ["user", "assistant", "tool"]
    );
    assert!(matches!(req.messages[0].content[1], LlmPart::Image { .. }));
    // Reasoning and the function call share one assistant turn.
    assert_eq!(
        thinking(&req.messages[1]),
        "Arithmetic is trivial; the weather needs the tool."
    );
    assert_eq!(tool_calls(&req.messages[1])[0].2, json!({"city": "Paris"}));
    assert!(
        matches!(&req.messages[2].content[0], LlmPart::ToolResult { id: Some(id), .. } if id == "call_P4r1s")
    );
    assert_eq!(req.tools.len(), 2);
    assert_eq!(req.tools[1].kind.as_deref(), Some("web_search_preview"));
    assert_eq!(req.params["reasoning"]["effort"], json!("low"));
    assert!(!req.params.contains_key("input"));
    let r = response(&out);
    assert_eq!(out.model.as_deref(), Some("gpt-5-2025-08-07"));
    assert_eq!(r.messages.len(), 1);
    assert!(thinking(&r.messages[0]).starts_with("**Combining results**"));
    assert_eq!(
        text_of(&r.messages[0]),
        "2 + 2 = 4. In Paris it's 12 °C and cloudy."
    );
    assert_eq!(r.stop_reason.as_deref(), Some("completed"));
    let u = r.usage.as_ref().unwrap();
    assert_eq!(
        (
            u.input_tokens,
            u.output_tokens,
            u.cache_read_tokens,
            u.reasoning_tokens
        ),
        (Some(328), Some(96), Some(128), Some(64))
    );
}

#[test]
fn responses_stream_complete_and_partial() {
    let events = timed(fx!("openai_responses_stream.sse"), 300.0, 10.0);
    let mut x = input(
        "api.openai.com",
        443,
        "/v1/responses",
        fx!("openai_responses_request.json"),
        Some(200),
        Some("text/event-stream"),
        "",
    );
    x.events = &events;
    let out = parse(&x).unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let r = response(&out);
    assert_eq!(
        text_of(&r.messages[0]),
        "2 + 2 = 4. In Paris it's 12 °C and cloudy."
    );
    assert_eq!(r.usage.as_ref().unwrap().total_tokens, Some(424));
    // First token: the first reasoning summary delta (event index 4).
    assert_eq!(out.timing.ttft_ms, Some(300.0 + 40.0 - 100.0));

    // Cut off before response.completed: assembled from the deltas, function call included.
    let events = timed(fx!("openai_responses_stream_partial.sse"), 300.0, 10.0);
    x.events = &events;
    let out = parse(&x).unwrap();
    let r = response(&out);
    assert_eq!(r.id.as_deref(), Some("resp_68c1c9e0a1b2"));
    assert_eq!(
        text_of(&r.messages[0]),
        "2 + 2 = 4. In Paris it's 12 °C and cloudy."
    );
    assert_eq!(
        thinking(&r.messages[0]),
        "**Combining results**\n\nAnswer the sum, then report the tool's weather."
    );
    let calls = tool_calls(&r.messages[0]);
    assert_eq!(calls[0].1, "get_time");
    assert_eq!(calls[0].2, json!({"city": "Paris"}));
    assert!(
        out.warnings.iter().any(|w| w.contains("final event")),
        "{:?}",
        out.warnings
    );
    // Still streaming: no warning.
    x.finished = false;
    assert!(parse(&x).unwrap().warnings.is_empty());

    // A failed response.
    let failed = r#"{"type":"response.failed","response":{"id":"resp_1","status":"failed","output":[],"error":{"code":"server_error","message":"The model failed."}}}"#;
    let body = format!("event: response.failed\ndata: {failed}\n\n");
    let x = input(
        "api.openai.com",
        443,
        "/v1/responses",
        r#"{"model":"gpt-5","input":"hi"}"#,
        Some(200),
        Some("text/event-stream"),
        &body,
    );
    let out = parse(&x).unwrap();
    assert_eq!(out.error.unwrap().message, "The model failed.");
}

// =============================================================================================
// Anthropic

#[test]
fn anthropic_streamed_thinking_text_and_tool_use() {
    let events = timed(fx!("anthropic_stream.sse"), 900.0, 15.0);
    let mut x = input(
        "api.anthropic.com",
        443,
        "/v1/messages",
        fx!("anthropic_request.json"),
        Some(200),
        Some("text/event-stream; charset=utf-8"),
        "",
    );
    x.events = &events;
    let out = parse(&x).unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(out.provider, "anthropic");
    assert_eq!(out.model.as_deref(), Some("claude-sonnet-4-5-20250929"));
    assert!(out.stream);
    let req = &out.request;
    assert!(req.system.as_deref().unwrap().starts_with("You are Forge"));
    assert_eq!(req.tools.len(), 3);
    assert_eq!(req.tools[1].name, "edit_file");
    assert!(req.tools[1].schema.is_some());
    assert_eq!(req.params["thinking"]["budget_tokens"], json!(4096));
    assert_eq!(req.params["max_tokens"], json!(16000));
    assert_eq!(req.messages.len(), 3);
    assert!(
        matches!(&req.messages[0].content[1], LlmPart::Image { media_type: Some(m), data: Some(_), .. } if m == "image/png")
    );
    assert!(thinking(&req.messages[1]).starts_with("A one-cent discrepancy"));
    assert_eq!(
        tool_calls(&req.messages[1])[0].2,
        json!({"path": "src/money.ts"})
    );
    match &req.messages[2].content[0] {
        LlmPart::ToolResult {
            id,
            content,
            is_error,
            ..
        } => {
            assert_eq!(id.as_deref(), Some("toolu_01A9mK2xQ7vR4tN8bH3cZ5wP"));
            assert!(!is_error);
            assert!(matches!(&content[0], LlmPart::Text { text } if text.contains("Math.floor")));
        }
        other => panic!("{other:?}"),
    }
    let r = response(&out);
    let m = &r.messages[0];
    assert_eq!(m.role, "assistant");
    assert_eq!(m.content.len(), 3, "{:?}", m.content);
    assert!(thinking(m).contains("`1998.9999999999998`"));
    assert!(thinking(m).ends_with("what the payment provider expects."));
    assert!(text_of(m).starts_with("Found it: `toCents`"));
    let calls = tool_calls(m);
    assert_eq!(
        calls[0].0.as_deref(),
        Some("toolu_01D7FvYq2nW8xR5tK3mJ6pLs")
    );
    assert_eq!(
        calls[0].2["new_string"],
        json!("return Math.round(amount * 100);")
    );
    assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
    let u = r.usage.as_ref().unwrap();
    assert_eq!(u.input_tokens, Some(24 + 3918));
    assert_eq!(u.cache_read_tokens, Some(3918));
    assert_eq!(u.cache_write_tokens, None);
    assert_eq!(u.output_tokens, Some(187));
    // First thinking delta is event index 3.
    assert_eq!(out.timing.ttft_ms, Some(900.0 + 45.0 - 100.0));
}

#[test]
fn anthropic_truncated_stream_and_errors() {
    // Cut in the middle of the tool input JSON.
    let full = fx!("anthropic_stream.sse");
    let cut = &full[..full.find("\"new_string").unwrap()];
    let x = input(
        "api.anthropic.com",
        443,
        "/v1/messages",
        fx!("anthropic_request.json"),
        Some(200),
        Some("text/event-stream"),
        cut,
    );
    let out = parse(&x).unwrap();
    let r = response(&out);
    assert!(text_of(&r.messages[0]).ends_with("add a regression test."));
    let calls = tool_calls(&r.messages[0]);
    assert!(calls[0].2.is_string(), "partial arguments are kept as text");
    assert!(
        out.warnings.iter().any(|w| w.contains("not valid JSON")),
        "{:?}",
        out.warnings
    );
    assert!(out.warnings.iter().any(|w| w.contains("final event")));
    assert_eq!(r.stop_reason, None);

    // HTTP 529 with Anthropic's error body.
    let x = input(
        "api.anthropic.com",
        443,
        "/v1/messages",
        fx!("anthropic_error_request.json"),
        Some(529),
        Some("application/json"),
        fx!("anthropic_error_response.json"),
    );
    let out = parse(&x).unwrap();
    assert_eq!(
        out.request.system.as_deref(),
        Some("Classify the support ticket as billing, bug, feature or other. Reply with one word.")
    );
    assert!(out.response.is_none());
    let e = out.error.unwrap();
    assert_eq!(
        (e.status, e.kind.as_deref(), e.message.as_str()),
        (Some(529), Some("overloaded_error"), "Overloaded")
    );

    // An error event mid-stream.
    let body = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"model\":\"claude\",\"role\":\"assistant\",\"usage\":{\"input_tokens\":3}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"Hi\"}}\n\n",
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
    );
    let x = input(
        "api.anthropic.com",
        443,
        "/v1/messages",
        fx!("anthropic_error_request.json"),
        Some(200),
        Some("text/event-stream"),
        body,
    );
    let out = parse(&x).unwrap();
    assert_eq!(text_of(&response(&out).messages[0]), "Hi");
    assert_eq!(out.error.unwrap().kind.as_deref(), Some("overloaded_error"));
}

#[test]
fn anthropic_non_streamed_with_cache_writes() {
    let x = input(
        "api.anthropic.com",
        443,
        "/v1/messages",
        fx!("anthropic_error_request.json"),
        Some(200),
        Some("application/json"),
        fx!("anthropic_response.json"),
    );
    let out = parse(&x).unwrap();
    assert!(!out.stream);
    let r = response(&out);
    assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(tool_calls(&r.messages[0])[0].2, json!({"pattern": "money"}));
    let u = r.usage.as_ref().unwrap();
    assert_eq!(
        (u.input_tokens, u.cache_write_tokens, u.total_tokens),
        (Some(3248), Some(2048), Some(3290))
    );
    // Vertex AI Claude: model from the path, body without `model`.
    let x = input(
        "us-east5-aiplatform.googleapis.com",
        443,
        "/v1/projects/p/locations/us-east5/publishers/anthropic/models/claude-haiku-4-5:rawPredict",
        r#"{"anthropic_version":"vertex-2023-10-16","max_tokens":5,"messages":[{"role":"user","content":"hi"}]}"#,
        None,
        None,
        "",
    );
    let out = parse(&x).unwrap();
    assert_eq!(out.model.as_deref(), Some("claude-haiku-4-5"));
    assert!(out.response.is_none() && out.error.is_none());
    assert_eq!(
        out.request.params["anthropic_version"],
        json!("vertex-2023-10-16")
    );
}

// =============================================================================================
// Gemini

#[test]
fn gemini_non_streamed() {
    let x = input(
        "generativelanguage.googleapis.com",
        443,
        "/v1beta/models/gemini-2.5-flash:generateContent",
        fx!("gemini_request.json"),
        Some(200),
        Some("application/json; charset=UTF-8"),
        fx!("gemini_response.json"),
    );
    let out = parse(&x).unwrap();
    assert_eq!((out.provider.as_str(), out.api), ("google", LlmApi::Gemini));
    assert!(!out.stream);
    assert!(
        out.request
            .system
            .as_deref()
            .unwrap()
            .starts_with("You are a product analyst")
    );
    assert_eq!(out.request.messages[0].role, "user");
    // generationConfig is flattened into the params.
    assert_eq!(out.request.params["temperature"], json!(0.2));
    assert_eq!(
        out.request.params["thinkingConfig"]["thinkingBudget"],
        json!(512)
    );
    assert!(out.request.params.contains_key("safetySettings"));
    assert!(!out.request.params.contains_key("contents"));
    let r = response(&out);
    assert_eq!(out.model.as_deref(), Some("gemini-2.5-flash"));
    assert_eq!(r.messages[0].role, "assistant");
    assert!(thinking(&r.messages[0]).starts_with("**Grouping the feedback**"));
    assert!(text_of(&r.messages[0]).contains("| Battery life | 2 | positive |"));
    assert_eq!(r.stop_reason.as_deref(), Some("STOP"));
    let u = r.usage.as_ref().unwrap();
    assert_eq!(
        (
            u.input_tokens,
            u.output_tokens,
            u.reasoning_tokens,
            u.total_tokens
        ),
        (Some(152), Some(131 + 126), Some(126), Some(409))
    );
}

#[test]
fn gemini_streams_sse_and_array() {
    let req = r#"{"contents":[{"role":"user","parts":[{"text":"Why is the sky blue?"}]}],"tools":[{"functionDeclarations":[{"name":"lookup","description":"Search","parameters":{"type":"OBJECT"}}]},{"googleSearch":{}}]}"#;
    let check = |out: &LlmExchange| {
        assert!(out.stream);
        assert_eq!(out.model.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(out.request.tools.len(), 2);
        assert_eq!(out.request.tools[1].kind.as_deref(), Some("googleSearch"));
        let r = response(out);
        let m = &r.messages[0];
        assert_eq!(thinking(m), "The user asks about the sky colour.");
        assert_eq!(
            text_of(m),
            "The sky is blue because of Rayleigh scattering."
        );
        assert_eq!(tool_calls(m)[0].2, json!({"q": "rayleigh scattering"}));
        assert_eq!(r.usage.as_ref().unwrap().cache_read_tokens, Some(4));
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    };
    let x = input(
        "generativelanguage.googleapis.com",
        443,
        "/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse",
        req,
        Some(200),
        Some("text/event-stream"),
        fx!("gemini_stream.sse"),
    );
    check(&parse(&x).unwrap());
    let x = input(
        "generativelanguage.googleapis.com",
        443,
        "/v1beta/models/gemini-2.5-pro:streamGenerateContent",
        req,
        Some(200),
        Some("application/json"),
        fx!("gemini_stream_array.json"),
    );
    let out = parse(&x).unwrap();
    check(&out);
    assert_eq!(out.timing.chunks, Some(4));
    // The array cut off mid-element: the complete elements are kept.
    let full = fx!("gemini_stream_array.json");
    let cut = &full[..full.find("functionCall").unwrap()];
    let x = input(
        "generativelanguage.googleapis.com",
        443,
        "/v1beta/models/gemini-2.5-pro:streamGenerateContent",
        req,
        Some(200),
        Some("application/json"),
        cut,
    );
    let out = parse(&x).unwrap();
    assert_eq!(
        text_of(&response(&out).messages[0]),
        "The sky is blue because of Rayleigh scattering."
    );
    assert!(
        out.warnings.iter().any(|w| w.contains("cut off")),
        "{:?}",
        out.warnings
    );
    assert!(out.warnings.iter().any(|w| w.contains("final event")));
    // An error array.
    let x = input(
        "generativelanguage.googleapis.com",
        443,
        "/v1beta/models/gemini-2.5-pro:generateContent",
        req,
        Some(429),
        Some("application/json"),
        r#"[{"error":{"code":429,"message":"Resource has been exhausted (e.g. check quota).","status":"RESOURCE_EXHAUSTED"}}]"#,
    );
    let e = parse(&x).unwrap().error.unwrap();
    assert_eq!(
        (e.kind.as_deref(), e.code.as_deref()),
        (Some("RESOURCE_EXHAUSTED"), Some("429"))
    );
    // snake_case REST fields, inline data, function responses and a blocked prompt.
    let req = r#"{"system_instruction":{"parts":[{"text":"sys"}]},"contents":[{"role":"user","parts":[{"inline_data":{"mime_type":"image/jpeg","data":"/9j/4AAQ"}},{"inline_data":{"mime_type":"application/pdf","data":"JVBERi0x"}}]},{"role":"model","parts":[{"function_call":{"name":"f","args":{"a":1}}}]},{"role":"user","parts":[{"function_response":{"name":"f","response":{"ok":true}}}]}],"generation_config":{"max_output_tokens":10}}"#;
    let x = input(
        "generativelanguage.googleapis.com",
        443,
        "/v1beta/models/gemini-2.0-flash:generateContent",
        req,
        Some(200),
        None,
        r#"{"promptFeedback":{"blockReason":"SAFETY"},"usageMetadata":{"promptTokenCount":5}}"#,
    );
    let out = parse(&x).unwrap();
    assert_eq!(out.request.system.as_deref(), Some("sys"));
    assert!(
        matches!(&out.request.messages[0].content[0], LlmPart::Image { media_type: Some(m), .. } if m == "image/jpeg")
    );
    assert!(
        matches!(&out.request.messages[0].content[1], LlmPart::File { label, .. } if label == "document")
    );
    assert_eq!(tool_calls(&out.request.messages[1])[0].2, json!({"a": 1}));
    assert!(
        matches!(&out.request.messages[2].content[0], LlmPart::ToolResult { name: Some(n), content, .. } if n == "f" && matches!(&content[0], LlmPart::Text { text } if text == r#"{"ok":true}"#))
    );
    assert_eq!(out.request.params["max_output_tokens"], json!(10));
    assert_eq!(response(&out).stop_reason.as_deref(), Some("SAFETY"));
    assert!(out.warnings.iter().any(|w| w.contains("blocked")));
}

// =============================================================================================
// Ollama

#[test]
fn ollama_openai_compatible_stream() {
    let events = timed(fx!("ollama_compat_stream.sse"), 250.0, 30.0);
    let mut x = input(
        "localhost",
        11434,
        "/v1/chat/completions",
        fx!("ollama_compat_request.json"),
        Some(200),
        Some("text/event-stream"),
        "",
    );
    x.events = &events;
    let out = parse(&x).unwrap();
    assert_eq!(out.provider, "ollama");
    assert_eq!(out.model.as_deref(), Some("llama3.2:3b"));
    let r = response(&out);
    assert_eq!(
        text_of(&r.messages[0]),
        "Packets drift like leaves\nthrough the quiet middle box—\nthe bug shows its face."
    );
    assert_eq!(r.usage.as_ref().unwrap().output_tokens, Some(19));
    assert_eq!(out.timing.ttft_ms, Some(150.0));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
}

#[test]
fn ollama_native_ndjson() {
    let x = input(
        "localhost",
        11434,
        "/api/chat",
        fx!("ollama_chat_request.json"),
        Some(200),
        Some("application/x-ndjson"),
        fx!("ollama_chat_stream.ndjson"),
    );
    let out = parse(&x).unwrap();
    assert_eq!(out.api, LlmApi::OllamaChat);
    assert!(out.stream);
    assert_eq!(out.request.system.as_deref(), Some("Be brief."));
    assert!(matches!(
        out.request.messages[0].content[1],
        LlmPart::Image { .. }
    ));
    // `options` are flattened into the params.
    assert_eq!(out.request.params["num_ctx"], json!(8192));
    assert_eq!(out.request.params["think"], json!(true));
    let r = response(&out);
    assert_eq!(
        thinking(&r.messages[0]),
        "A small UI mockup with a red total row."
    );
    assert_eq!(
        text_of(&r.messages[0]),
        "A checkout summary with a highlighted total."
    );
    assert_eq!(r.stop_reason.as_deref(), Some("stop"));
    assert_eq!(r.usage.as_ref().unwrap().input_tokens, Some(1032));
    // Ollama's own measurement: 38 tokens in 0.95 s.
    assert_eq!(out.timing.tokens_per_second, Some(40.0));
    assert_eq!(out.timing.chunks, Some(5));

    // /api/generate, not streamed, with an error line.
    let x = input(
        "localhost",
        11434,
        "/api/generate",
        r#"{"model":"llama3.2","prompt":"Why?","system":"Be terse.","stream":false}"#,
        Some(200),
        Some("application/json"),
        r#"{"model":"llama3.2","response":"Because.","done":true,"done_reason":"stop","prompt_eval_count":12,"eval_count":2}"#,
    );
    let out = parse(&x).unwrap();
    assert!(!out.stream);
    assert_eq!(out.request.system.as_deref(), Some("Be terse."));
    assert_eq!(text_of(&response(&out).messages[0]), "Because.");
    let x = input(
        "localhost",
        11434,
        "/api/chat",
        r#"{"model":"nope","messages":[]}"#,
        Some(404),
        None,
        r#"{"error":"model \"nope\" not found, try pulling it first"}"#,
    );
    assert!(
        parse(&x)
            .unwrap()
            .error
            .unwrap()
            .message
            .contains("try pulling")
    );
}

// =============================================================================================
// Robustness

#[test]
fn malformed_and_partial_inputs_never_fail() {
    // Garbage everywhere on a trusted host: still recognised, with warnings.
    let x = input(
        "api.openai.com",
        443,
        "/v1/chat/completions",
        "\u{0}\u{1}garbage",
        Some(200),
        Some("application/json"),
        "{\"choices\": [",
    );
    let out = parse(&x).unwrap();
    assert!(out.response.is_none());
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("request body is not valid JSON"))
    );
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("response body is not valid JSON"))
    );
    // Pending: request only.
    let mut x = input(
        "api.anthropic.com",
        443,
        "/v1/messages",
        fx!("anthropic_request.json"),
        None,
        None,
        "",
    );
    x.done_ms = None;
    x.finished = false;
    let out = parse(&x).unwrap();
    assert!(out.response.is_none() && out.error.is_none() && out.warnings.is_empty());
    assert_eq!(out.timing, LlmTiming::default());
    // Streaming in progress: partial content, no warnings.
    let full = fx!("anthropic_stream.sse");
    let partial = &full[..full.find("so some EUR").unwrap()];
    let mut x = input(
        "api.anthropic.com",
        443,
        "/v1/messages",
        fx!("anthropic_request.json"),
        Some(200),
        Some("text/event-stream"),
        partial,
    );
    x.finished = false;
    x.done_ms = None;
    let out = parse(&x).unwrap();
    assert!(text_of(&response(&out).messages[0]).starts_with("Found it"));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    // Wrongly typed fields everywhere.
    let weird = r#"{"model":["x"],"messages":[{"role":7,"content":{"a":1}},"str",null,{"role":"assistant","tool_calls":[{"function":{"name":null,"arguments":{"x":1}}},5]}],"tools":"nope"}"#;
    let x = input(
        "api.openai.com",
        443,
        "/v1/chat/completions",
        weird,
        Some(200),
        None,
        r#"{"choices":"x","usage":[1]}"#,
    );
    let out = parse(&x).unwrap();
    assert_eq!(out.model, None);
    assert!(out.request.tools.is_empty());
    assert!(response(&out).messages.is_empty());
    // Every provider survives arbitrary byte soup and truncation at every offset.
    let cases: &[(&str, u16, &str, &str, &str)] = &[
        (
            "api.openai.com",
            443,
            "/v1/chat/completions",
            fx!("openai_chat_final_request.json"),
            fx!("openai_chat_final_stream.sse"),
        ),
        (
            "api.openai.com",
            443,
            "/v1/responses",
            fx!("openai_responses_request.json"),
            fx!("openai_responses_stream.sse"),
        ),
        (
            "api.anthropic.com",
            443,
            "/v1/messages",
            fx!("anthropic_request.json"),
            fx!("anthropic_stream.sse"),
        ),
        (
            "generativelanguage.googleapis.com",
            443,
            "/v1beta/models/g:streamGenerateContent",
            fx!("gemini_request.json"),
            fx!("gemini_stream_array.json"),
        ),
        (
            "localhost",
            11434,
            "/api/chat",
            fx!("ollama_chat_request.json"),
            fx!("ollama_chat_stream.ndjson"),
        ),
    ];
    for (host, port, path, req, resp) in cases {
        let step = (resp.len() / 97).max(1);
        let mut cut = 0;
        while cut <= resp.len() {
            let end = (0..=cut)
                .rev()
                .find(|i| resp.is_char_boundary(*i))
                .unwrap_or(0);
            let ct = if resp.starts_with("data:") || resp.starts_with("event:") {
                Some("text/event-stream")
            } else {
                None
            };
            let mut x = input(host, *port, path, req, Some(200), ct, &resp[..end]);
            x.response_truncated = true;
            let out = parse(&x).unwrap();
            serde_json::to_string(&out).unwrap();
            cut += step;
        }
        let rq = &req[..req.len() / 2];
        let x = input(host, *port, path, rq, Some(200), None, "\u{fffd}\u{0}[]{}");
        if let Some(out) = parse(&x) {
            serde_json::to_string(&out).unwrap();
        }
    }
}

#[test]
fn serialized_shape() {
    let x = input(
        "api.anthropic.com",
        443,
        "/v1/messages",
        fx!("anthropic_request.json"),
        Some(200),
        Some("text/event-stream"),
        fx!("anthropic_stream.sse"),
    );
    let v = serde_json::to_value(parse(&x).unwrap()).unwrap();
    assert_eq!(v["provider"], "anthropic");
    assert_eq!(v["api"], "anthropic-messages");
    assert_eq!(v["stream"], true);
    let parts = &v["request"]["messages"][1]["content"];
    assert_eq!(parts[0]["type"], "thinking");
    assert_eq!(parts[2]["type"], "tool-call");
    assert_eq!(parts[2]["arguments"]["path"], "src/money.ts");
    let result = &v["request"]["messages"][2]["content"][0];
    assert_eq!(result["type"], "tool-result");
    assert!(result.get("isError").is_none(), "false flags are omitted");
    let image = &v["request"]["messages"][0]["content"][1];
    assert_eq!(image["mediaType"], "image/png");
    assert_eq!(v["response"]["stopReason"], "tool_use");
    assert_eq!(v["response"]["usage"]["cacheReadTokens"], 3918);
    assert_eq!(v["timing"]["totalMs"], 1900.0);
    assert!(v.get("error").is_none() && v.get("warnings").is_none());
    assert_eq!(v["request"]["tools"][0]["schema"]["required"][0], "path");
}

#[test]
fn compact_for_agents() {
    let x = input(
        "api.anthropic.com",
        443,
        "/v1/messages",
        fx!("anthropic_request.json"),
        Some(200),
        Some("text/event-stream"),
        fx!("anthropic_stream.sse"),
    );
    let mut out = parse(&x).unwrap();
    compact(&mut out, 20);
    assert!(
        out.request
            .system
            .as_deref()
            .unwrap()
            .contains("more characters]")
    );
    assert!(matches!(
        &out.request.messages[0].content[1],
        LlmPart::Image {
            data: None,
            size: Some(204),
            ..
        }
    ));
    assert!(text_of(&response(&out).messages[0]).starts_with("Found it: `toCents` …"));
    let text = content_text(&response(&out).messages[0].content);
    assert!(text.contains("[tool call] edit_file("), "{text}");
}

#[test]
fn base64_sizes() {
    assert_eq!(base64_size(""), 0);
    assert_eq!(base64_size("AAAA"), 3);
    assert_eq!(base64_size("AAA="), 2);
    assert_eq!(base64_size("AA=="), 1);
    assert_eq!(base64_size("AAAAAA"), 4);
    assert!(
        matches!(image_url("data:image/gif;base64,R0lGOD"), LlmPart::Image { media_type: Some(m), size: Some(4), .. } if m == "image/gif")
    );
    assert!(matches!(
        image_url("data:text/plain,hello"),
        LlmPart::Image {
            url: Some(_),
            data: None,
            ..
        }
    ));
}
