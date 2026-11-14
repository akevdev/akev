//! LLM API traffic: recognise calls to chat/generation APIs and normalise them into one
//! conversation model ([`LlmExchange`]) for the UI, the API (`GET /api/exchanges/:id/llm`)
//! and MCP.
//!
//! Recognised APIs (by host + path, and — for hosts that aren't well-known providers — by the
//! shape of the request body):
//!
//! * **OpenAI Chat Completions** (`…/chat/completions`) and legacy **Completions**
//!   (`…/completions`) — OpenAI, Azure OpenAI and every OpenAI-compatible server (OpenRouter,
//!   Groq, Mistral, DeepSeek, xAI, Together, Fireworks, Perplexity, Ollama, vLLM, LM Studio…);
//! * **OpenAI Responses** (`…/v1/responses`);
//! * **Anthropic Messages** (`…/v1/messages`, and Vertex AI `…/publishers/anthropic/…:rawPredict`);
//! * **Google Gemini** (`…:generateContent`, `…:streamGenerateContent`, AI Studio and Vertex AI);
//! * **Ollama** native (`/api/chat`, `/api/generate` on port 11434 or an `ollama` host);
//! * **Amazon Bedrock**: the Converse API and InvokeModel with Anthropic models, including
//!   their binary AWS event streams (`bedrock-runtime.<region>.amazonaws.com`).
//!
//! Streaming responses are reassembled into the final assistant message(s): OpenAI SSE
//! deltas (`data: {…}` … `[DONE]`) including tool-call argument fragments, Anthropic SSE
//! (`message_start` / `content_block_*` with `text_delta`, `input_json_delta`,
//! `thinking_delta` / `message_delta` / `message_stop`), Responses API events, Gemini SSE and
//! JSON-array streams, and Ollama NDJSON. When the stream's events carry timestamps (the
//! proxy records every SSE event as it arrives) the time to first token is measured.
//!
//! Everything is best effort: malformed, truncated or unexpected bodies never fail — what
//! could be understood is returned, and [`LlmExchange::warnings`] says what couldn't.

mod anthropic;
mod bedrock;
mod gemini;
mod ollama;
mod openai;
mod responses;
#[cfg(test)]
mod tests;

use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Base64 image data larger than this (in characters) is left out of [`LlmPart::Image`]
/// (`size` still says how big it was).
pub const MAX_INLINE_IMAGE: usize = 8 << 20;

/// At most this many warnings are reported (the last one says how many were left out).
const MAX_WARNINGS: usize = 20;

// =============================================================================================
// Model
// =============================================================================================

/// The wire API of a recognised LLM call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LlmApi {
    /// OpenAI Chat Completions (and every OpenAI-compatible server).
    #[serde(rename = "openai-chat")]
    OpenAiChat,
    /// OpenAI legacy text Completions (`prompt` in, `choices[].text` out).
    #[serde(rename = "openai-completions")]
    OpenAiCompletions,
    /// OpenAI Responses API.
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    /// Anthropic Messages API.
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
    /// Google Gemini `generateContent` / `streamGenerateContent`.
    #[serde(rename = "gemini")]
    Gemini,
    /// Ollama's native `/api/chat`.
    #[serde(rename = "ollama-chat")]
    OllamaChat,
    /// Ollama's native `/api/generate`.
    #[serde(rename = "ollama-generate")]
    OllamaGenerate,
    /// Amazon Bedrock's Converse / ConverseStream API.
    #[serde(rename = "bedrock-converse")]
    BedrockConverse,
}

impl LlmApi {
    /// Human-readable name, e.g. "Chat Completions".
    pub fn label(self) -> &'static str {
        match self {
            LlmApi::OpenAiChat => "Chat Completions",
            LlmApi::OpenAiCompletions => "Completions",
            LlmApi::OpenAiResponses => "Responses",
            LlmApi::AnthropicMessages => "Messages",
            LlmApi::Gemini => "Gemini generateContent",
            LlmApi::OllamaChat => "Ollama chat",
            LlmApi::OllamaGenerate => "Ollama generate",
            LlmApi::BedrockConverse => "Bedrock Converse",
        }
    }
}

/// A recognised LLM API call, normalised across providers.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmExchange {
    /// Provider id: `openai`, `anthropic`, `google`, `vertex-ai`, `azure-openai`, `openrouter`,
    /// `groq`, `mistral`, `deepseek`, `xai`, `together`, `fireworks`, `perplexity`, `cerebras`,
    /// `huggingface`, `ollama`, `lmstudio`, `aws-bedrock`, or `openai-compatible` / `anthropic-compatible`
    /// for unknown hosts.
    pub provider: String,
    /// The wire API.
    pub api: LlmApi,
    /// The model that answered (from the response), else the one requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Whether the response was streamed.
    pub stream: bool,
    pub request: LlmRequest,
    /// The response, once there is one (`None` while waiting, or when the API returned an
    /// error instead).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<LlmResponse>,
    pub timing: LlmTiming,
    /// An error reported by the API (HTTP error body, or an error event in the stream).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LlmError>,
    /// What couldn't be understood (malformed JSON, truncated streams…).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// The prompt side of the call.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRequest {
    /// System prompt / instructions (several system blocks are joined with blank lines).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// The conversation sent to the model, in order.
    pub messages: Vec<LlmMessage>,
    /// Tool (function) definitions offered to the model.
    pub tools: Vec<LlmTool>,
    /// Every other request parameter (temperature, max_tokens, top_p, reasoning, tool_choice,
    /// response_format…) under the provider's own names. Gemini `generationConfig` and Ollama
    /// `options` entries are flattened into it.
    pub params: Map<String, Value>,
}

/// One message (turn) of the conversation.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmMessage {
    /// `system`, `developer`, `user`, `assistant` or `tool` (Gemini's `model` is `assistant`).
    pub role: String,
    /// Participant / function name, when given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: Vec<LlmPart>,
}

/// One piece of message content.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum LlmPart {
    /// Text (usually Markdown).
    Text { text: String },
    /// An image: inline base64 `data` (without the `data:` prefix) or a remote `url`.
    Image {
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// Decoded size in bytes of inline data.
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },
    /// A document, audio clip or other file attached to a message (contents not included).
    File {
        /// `document`, `audio` or `file`.
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },
    /// The model calling a tool. `arguments` is the parsed JSON (the raw string when it isn't
    /// valid JSON).
    ToolCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        arguments: Value,
    },
    /// The result of a tool call, sent back to the model.
    ToolResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        content: Vec<LlmPart>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
    /// Reasoning / thinking. `redacted` when only an encrypted form was sent.
    Thinking {
        text: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        redacted: bool,
    },
    /// The model declined to answer.
    Refusal { text: String },
    /// A content block Akev doesn't model (built-in tool results, citations…), kept as JSON.
    Other { label: String, value: Value },
}

/// A tool the model may call.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema of the arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    /// Built-in tool type (`web_search_20250305`, `code_interpreter`, `googleSearch`…); absent
    /// for ordinary function tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// What the model answered.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Usually one assistant message (OpenAI `n > 1` gives one per choice).
    pub messages: Vec<LlmMessage>,
    /// Why generation stopped, in the provider's words (`stop`, `end_turn`, `tool_use`,
    /// `tool_calls`, `length`, `max_tokens`, `STOP`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsage>,
}

/// Token usage. `input_tokens` counts every prompt token, cached ones included (Anthropic
/// reports uncached input separately — Akev adds its cache reads and writes);
/// `output_tokens` includes reasoning tokens.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// Prompt tokens read from the provider's cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Prompt tokens written to the cache (Anthropic).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// Output tokens spent on reasoning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}

impl LlmUsage {
    fn is_empty(&self) -> bool {
        *self == LlmUsage::default()
    }
}

/// Latency figures. All durations are measured from the moment the request was sent upstream.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmTiming {
    /// Time to the first generated token (from the timestamps of the stream's events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<f64>,
    /// Time until the response was complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_ms: Option<f64>,
    /// Output tokens per second of generation (after the first token when that is known;
    /// Ollama's own measurement when it reports one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_per_second: Option<f64>,
    /// Number of stream events (chunks) the response arrived in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<u32>,
}

/// An error returned by the LLM API.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmError {
    /// HTTP status of the response, when it was an error status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Error type / status name (`rate_limit_error`, `invalid_request_error`, `RESOURCE_EXHAUSTED`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Error code (`context_length_exceeded`, `429`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
}

/// A compact tag for the traffic list (`ExchangeSummary::llm`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmRoute {
    /// Provider id (see [`LlmExchange::provider`]).
    pub provider: &'static str,
    pub api: LlmApi,
    /// Model named by the path (Gemini, Vertex AI, Azure deployments).
    pub path_model: Option<String>,
    /// The host is a well-known provider: the path alone is enough to recognise the call.
    /// Otherwise the request body must look like the API's.
    pub trusted: bool,
    /// The path asks for a streamed response (`:streamGenerateContent`, `:streamRawPredict`).
    pub path_stream: bool,
}

// =============================================================================================
// Recognition
// =============================================================================================

/// Well-known API hosts: exact host or `.suffix`.
const PROVIDER_HOSTS: &[(&str, &str)] = &[
    ("api.openai.com", "openai"),
    ("api.anthropic.com", "anthropic"),
    ("generativelanguage.googleapis.com", "google"),
    (".openai.azure.com", "azure-openai"),
    (".cognitiveservices.azure.com", "azure-openai"),
    (".services.ai.azure.com", "azure-openai"),
    ("openrouter.ai", "openrouter"),
    ("api.groq.com", "groq"),
    ("api.mistral.ai", "mistral"),
    ("api.deepseek.com", "deepseek"),
    ("api.x.ai", "xai"),
    ("api.together.xyz", "together"),
    ("api.together.ai", "together"),
    ("api.fireworks.ai", "fireworks"),
    ("api.perplexity.ai", "perplexity"),
    ("api.cerebras.ai", "cerebras"),
    ("router.huggingface.co", "huggingface"),
    ("api-inference.huggingface.co", "huggingface"),
];

/// The provider of a well-known API host.
fn known_provider(host: &str) -> Option<&'static str> {
    let host = host.trim_end_matches('.');
    // Vertex AI: `aiplatform.googleapis.com` and regional `{region}-aiplatform.googleapis.com`.
    if host.ends_with("aiplatform.googleapis.com") {
        return Some("vertex-ai");
    }
    // Bedrock: `bedrock-runtime.{region}.amazonaws.com` (and the `-fips` variant).
    if host.starts_with("bedrock-runtime") && host.ends_with(".amazonaws.com") {
        return Some("aws-bedrock");
    }
    PROVIDER_HOSTS.iter().find_map(|(pattern, id)| {
        let hit = match pattern.strip_prefix('.') {
            Some(suffix) => {
                host.len() > suffix.len() + 1
                    && host.ends_with(suffix)
                    && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
            }
            None => host.eq_ignore_ascii_case(pattern),
        };
        hit.then_some(*id)
    })
}

fn is_local(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    matches!(
        h,
        "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "host.docker.internal"
    ) || h.ends_with(".localhost")
        || h.starts_with("127.")
}

/// Provider for a host that isn't a well-known API: local inference servers by their default
/// port, else `fallback`.
fn local_provider(host: &str, port: u16, fallback: &'static str) -> &'static str {
    if host.contains("ollama") || (port == 11434 && (is_local(host) || !host.contains('.'))) {
        return "ollama";
    }
    if port == 1234 && is_local(host) {
        return "lmstudio";
    }
    fallback
}

/// Recognise an LLM API call from where it was sent. `path` may include the query.
///
/// Returns `None` for anything that isn't a generation endpoint. A route with
/// `trusted == false` still needs [`request_looks_like`] on the request body.
pub fn route(host: &str, port: u16, path: &str) -> Option<LlmRoute> {
    let path_only = path.split(['?', '#']).next().unwrap_or("");
    let p = path_only.trim_end_matches('/');
    // Every captured exchange passes through here: bail out early, without allocating, for
    // paths that can't be a generation endpoint.
    let candidate = p.contains(':')
        || [
            "/completions",
            "/responses",
            "/messages",
            "/api/chat",
            "/api/generate",
            "/invoke",
            "/invoke-with-response-stream",
            "/converse",
            "/converse-stream",
        ]
        .iter()
        .any(|s| ends_with_ci(p, s));
    if !candidate {
        return None;
    }
    let host_lc = lower(host);
    let known = known_provider(&host_lc);

    // Bedrock: `/model/{modelId}/converse[-stream]` or `/invoke[-with-response-stream]` (the
    // model id is percent-encoded and may contain `:`).
    if known == Some("aws-bedrock") {
        let rest = p.strip_prefix("/model/")?;
        let (model, op) = rest.rsplit_once('/')?;
        let model = percent_decode(model);
        let (api, stream) = match op {
            "converse" => (LlmApi::BedrockConverse, false),
            "converse-stream" => (LlmApi::BedrockConverse, true),
            // InvokeModel bodies are the model's own format: Anthropic's is understood.
            "invoke" | "invoke-with-response-stream" if model.contains("anthropic.") => {
                (LlmApi::AnthropicMessages, op != "invoke")
            }
            _ => return None,
        };
        return Some(LlmRoute {
            provider: "aws-bedrock",
            api,
            path_model: Some(model),
            trusted: true,
            path_stream: stream,
        });
    }

    // Gemini & Vertex AI: `…/models/{model}:generateContent`.
    if let Some((before, method)) = p.rsplit_once(':') {
        let method_lc = method.to_ascii_lowercase();
        let model = before
            .rsplit_once("/models/")
            .map(|(_, m)| m)
            .filter(|m| !m.is_empty() && !m.contains('/'))
            .map(percent_decode);
        let gemini = matches!(
            method_lc.as_str(),
            "generatecontent" | "streamgeneratecontent"
        );
        if gemini {
            return Some(LlmRoute {
                provider: known.unwrap_or("google"),
                api: LlmApi::Gemini,
                path_model: model,
                trusted: true,
                path_stream: method_lc == "streamgeneratecontent",
            });
        }
        // Claude on Vertex AI: `…/publishers/anthropic/models/{model}:rawPredict`.
        if matches!(method_lc.as_str(), "rawpredict" | "streamrawpredict")
            && before.contains("/publishers/anthropic/")
        {
            return Some(LlmRoute {
                provider: known.unwrap_or("vertex-ai"),
                api: LlmApi::AnthropicMessages,
                path_model: model,
                trusted: true,
                path_stream: method_lc == "streamrawpredict",
            });
        }
        return None;
    }

    let lower = lower(p);
    let azure_model = || {
        lower
            .find("/deployments/")
            .map(|i| &p[i + "/deployments/".len()..])
            .and_then(|rest| rest.split('/').next())
            .filter(|d| !d.is_empty())
            .map(percent_decode)
    };
    let (api, fallback) = if lower.ends_with("/chat/completions") {
        (LlmApi::OpenAiChat, "openai-compatible")
    } else if lower.ends_with("/completions") {
        (LlmApi::OpenAiCompletions, "openai-compatible")
    } else if lower.ends_with("/v1/responses") || lower.ends_with("/openai/responses") {
        (LlmApi::OpenAiResponses, "openai-compatible")
    } else if lower.ends_with("/v1/messages")
        || (known == Some("anthropic") && lower.ends_with("/messages"))
    {
        (LlmApi::AnthropicMessages, "anthropic-compatible")
    } else if lower == "/api/chat" || lower == "/api/generate" {
        let ollama = local_provider(&host_lc, port, "") == "ollama";
        if !ollama {
            return None;
        }
        let api = if lower == "/api/chat" {
            LlmApi::OllamaChat
        } else {
            LlmApi::OllamaGenerate
        };
        return Some(LlmRoute {
            provider: "ollama",
            api,
            path_model: None,
            trusted: false,
            path_stream: false,
        });
    } else {
        return None;
    };
    Some(LlmRoute {
        provider: known.unwrap_or_else(|| local_provider(&host_lc, port, fallback)),
        api,
        path_model: if known == Some("azure-openai") {
            azure_model()
        } else {
            None
        },
        trusted: known.is_some(),
        path_stream: false,
    })
}

/// ASCII-lowercased, borrowing when already lowercase.
fn lower(s: &str) -> std::borrow::Cow<'_, str> {
    if s.bytes().any(|b| b.is_ascii_uppercase()) {
        std::borrow::Cow::Owned(s.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

fn ends_with_ci(s: &str, suffix: &str) -> bool {
    s.len() >= suffix.len()
        && s.as_bytes()[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

/// Whether a parsed request body has the shape of `api`'s requests (used for hosts that
/// aren't well-known providers, where the path alone could be a coincidence).
pub fn request_looks_like(api: LlmApi, body: &Value) -> bool {
    let has_model = body.get("model").is_some_and(Value::is_string);
    let array = |k: &str| body.get(k).is_some_and(Value::is_array);
    match api {
        LlmApi::OpenAiChat => array("messages"),
        LlmApi::OpenAiCompletions => has_model && body.get("prompt").is_some(),
        LlmApi::OpenAiResponses => {
            has_model
                && (body.get("input").is_some()
                    || body.get("instructions").is_some()
                    || body.get("previous_response_id").is_some())
        }
        LlmApi::AnthropicMessages => {
            array("messages") && (has_model || body.get("max_tokens").is_some())
        }
        LlmApi::Gemini => true,
        LlmApi::OllamaChat => has_model && array("messages"),
        LlmApi::OllamaGenerate => has_model,
        LlmApi::BedrockConverse => array("messages"),
    }
}

/// Cheap shape of a JSON value, for [`probe`] (contents are skipped, not built).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Array,
    Other,
}

impl<'de> Deserialize<'de> for Shape {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Shape;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Shape, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Shape::Array)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Shape, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Shape::Other)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Shape, E> {
                Ok(Shape::Other)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Shape, E> {
                Ok(Shape::Other)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Shape, E> {
                Ok(Shape::Other)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Shape, E> {
                Ok(Shape::Other)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Shape, E> {
                Ok(Shape::Other)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Shape, E> {
                Ok(Shape::Other)
            }
        }
        d.deserialize_any(V)
    }
}

/// A string, or nothing for any other JSON value (skipped without being built).
struct LenientString(Option<String>);

impl<'de> Deserialize<'de> for LenientString {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = LenientString;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<LenientString, E> {
                Ok(LenientString(Some(v.to_string())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<LenientString, E> {
                Ok(LenientString(Some(v)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<LenientString, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(LenientString(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<LenientString, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(LenientString(None))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<LenientString, E> {
                Ok(LenientString(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<LenientString, E> {
                Ok(LenientString(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<LenientString, E> {
                Ok(LenientString(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<LenientString, E> {
                Ok(LenientString(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<LenientString, E> {
                Ok(LenientString(None))
            }
        }
        d.deserialize_any(V)
    }
}

/// The few top-level request fields [`tag`] needs; everything else is skipped without being
/// built (so tagging a large prompt costs one scan of the JSON).
#[derive(Deserialize)]
struct Probe {
    model: Option<LenientString>,
    messages: Option<Shape>,
    prompt: Option<IgnoredAny>,
    input: Option<IgnoredAny>,
    instructions: Option<IgnoredAny>,
    previous_response_id: Option<IgnoredAny>,
    max_tokens: Option<IgnoredAny>,
}

impl Probe {
    fn looks_like(&self, api: LlmApi) -> bool {
        let has_model = self.model.as_ref().is_some_and(|m| m.0.is_some());
        let messages = self.messages == Some(Shape::Array);
        match api {
            LlmApi::OpenAiChat => messages,
            LlmApi::OpenAiCompletions => has_model && self.prompt.is_some(),
            LlmApi::OpenAiResponses => {
                has_model
                    && (self.input.is_some()
                        || self.instructions.is_some()
                        || self.previous_response_id.is_some())
            }
            LlmApi::AnthropicMessages => messages && (has_model || self.max_tokens.is_some()),
            LlmApi::Gemini => true,
            LlmApi::OllamaChat => has_model && messages,
            LlmApi::OllamaGenerate => has_model,
            LlmApi::BedrockConverse => messages,
        }
    }
}

/// The traffic-list tag of a call: provider and model. `request_body` is the decoded request
/// body when available (`None` while it is still uploading): for well-known hosts the tag is
/// known from the path alone, for other hosts it is only returned once the body confirms the
/// API's shape. Cheap: the body is scanned, never turned into a JSON tree.
pub fn tag(
    host: &str,
    port: u16,
    path: &str,
    request_body: Option<&[u8]>,
) -> Option<(&'static str, Option<String>)> {
    let route = route(host, port, path)?;
    let probe = request_body
        .map(crate::util::skip_bom_ws)
        .filter(|b| b.first() == Some(&b'{'))
        .and_then(|b| serde_json::from_slice::<Probe>(b).ok());
    match &probe {
        Some(p) if p.looks_like(route.api) => {}
        _ if route.trusted => {}
        _ => return None,
    }
    let model = probe
        .and_then(|p| p.model)
        .and_then(|m| m.0)
        .filter(|m| !m.trim().is_empty())
        .or(route.path_model);
    Some((route.provider, model))
}

// =============================================================================================
// Parsing
// =============================================================================================

/// One server-sent event of a streamed response.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamEvent {
    /// `event:` name.
    pub event: Option<String>,
    pub data: String,
    /// When it arrived, in ms since the exchange started.
    pub at_ms: Option<f64>,
}

/// Everything [`parse`] looks at.
#[derive(Clone, Debug, Default)]
pub struct LlmInput<'a> {
    pub host: &'a str,
    pub port: u16,
    /// Path including the query.
    pub path: &'a str,
    /// Decoded (content-encoding removed) request body.
    pub request_body: &'a [u8],
    /// Response status, once the response head arrived.
    pub response_status: Option<u16>,
    pub response_content_type: Option<&'a str>,
    /// Decoded response body (so far).
    pub response_body: &'a [u8],
    /// The capture holds only a prefix of the response body.
    pub response_truncated: bool,
    /// The response's SSE events as recorded with their arrival times (preferred over parsing
    /// `response_body` when non-empty).
    pub events: &'a [StreamEvent],
    /// When the request was sent upstream (ms since the exchange started).
    pub sent_ms: Option<f64>,
    /// When the response was complete (ms since the exchange started).
    pub done_ms: Option<f64>,
    /// The exchange is finished (complete or aborted): a stream without its final event was
    /// cut short rather than still running.
    pub finished: bool,
}

/// Collected warnings (bounded).
#[derive(Default, Debug)]
pub(crate) struct Warnings {
    list: Vec<String>,
    dropped: usize,
}

impl Warnings {
    pub(crate) fn push(&mut self, w: impl Into<String>) {
        let w = w.into();
        if self.list.contains(&w) {
            return;
        }
        if self.list.len() < MAX_WARNINGS - 1 {
            self.list.push(w);
        } else {
            self.dropped += 1;
        }
    }

    fn finish(mut self) -> Vec<String> {
        if self.dropped > 0 {
            self.list
                .push(format!("…and {} more warnings", self.dropped));
        }
        self.list
    }
}

/// The parsed request of one API.
pub(crate) struct ParsedRequest {
    pub request: LlmRequest,
    pub stream: bool,
    pub model: Option<String>,
}

/// The parsed response (or stream) of one API.
#[derive(Default)]
pub(crate) struct ParsedResponse {
    pub response: LlmResponse,
    pub error: Option<LlmError>,
    /// When the first generated content arrived (stream event timestamp).
    pub first_token_ms: Option<f64>,
    /// Throughput measured by the server itself (Ollama).
    pub server_tps: Option<f64>,
    /// The stream's terminating event was seen (or the response wasn't a stream).
    pub complete: bool,
    pub chunks: Option<u32>,
}

/// Recognise and normalise an LLM API call. `None` when the exchange isn't one.
pub fn parse(input: &LlmInput<'_>) -> Option<LlmExchange> {
    let route = route(input.host, input.port, input.path)?;
    let mut w = Warnings::default();
    let req_json = parse_json(input.request_body);
    let looks_like = req_json
        .as_ref()
        .ok()
        .is_some_and(|v| request_looks_like(route.api, v));
    if !route.trusted && !looks_like {
        return None;
    }
    let req_value = match req_json {
        Ok(v) => v,
        Err(e) => {
            if input.request_body.is_empty() {
                w.push("The request has no body");
            } else {
                w.push(format!("The request body is not valid JSON: {e}"));
            }
            Value::Null
        }
    };
    let parsed = match route.api {
        LlmApi::OpenAiChat => openai::chat_request(&req_value, &mut w),
        LlmApi::OpenAiCompletions => openai::completions_request(&req_value, &mut w),
        LlmApi::OpenAiResponses => responses::request(&req_value, &mut w),
        LlmApi::AnthropicMessages => anthropic::request(&req_value, &mut w),
        LlmApi::Gemini => gemini::request(&req_value, &mut w),
        LlmApi::OllamaChat => ollama::chat_request(&req_value, &mut w),
        LlmApi::OllamaGenerate => ollama::generate_request(&req_value, &mut w),
        LlmApi::BedrockConverse => bedrock::request(&req_value, &mut w),
    };
    let mut stream = parsed.stream || route.path_stream;

    let (response, error, timing) = parse_response(input, route.api, &mut stream, &mut w);
    let model = response
        .as_ref()
        .and_then(|r| r.model.clone())
        .or(parsed.model)
        .or(route.path_model);
    Some(LlmExchange {
        provider: route.provider.to_string(),
        api: route.api,
        model,
        stream,
        request: parsed.request,
        response,
        timing,
        error,
        warnings: w.finish(),
    })
}

fn parse_json(body: &[u8]) -> Result<Value, String> {
    let body = crate::util::skip_bom_ws(body);
    if body.is_empty() {
        return Err("empty".into());
    }
    serde_json::from_slice(body).map_err(|e| e.to_string())
}

fn is_event_stream(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|ct| crate::util::essence(ct) == "text/event-stream")
}

fn parse_response(
    input: &LlmInput<'_>,
    api: LlmApi,
    stream: &mut bool,
    w: &mut Warnings,
) -> (Option<LlmResponse>, Option<LlmError>, LlmTiming) {
    let mut timing = LlmTiming {
        total_ms: match (input.sent_ms, input.done_ms) {
            (Some(s), Some(d)) => Some((d - s).max(0.0)),
            (None, Some(d)) => Some(d.max(0.0)),
            _ => None,
        },
        ..LlmTiming::default()
    };
    let Some(status) = input.response_status else {
        return (None, None, timing);
    };
    let body = crate::util::skip_bom_ws(input.response_body);
    if input.response_truncated {
        w.push("The response body was only partly captured (capture size limit)");
    }

    if status >= 400 {
        let error = error_from_body(input.response_body, status);
        return (None, Some(error), timing);
    }

    // Streamed responses: recorded SSE events, an SSE body, an AWS event stream (Bedrock),
    // Ollama NDJSON or a Gemini array.
    let aws_stream = input
        .response_content_type
        .is_some_and(|ct| crate::util::essence(ct) == bedrock::EVENT_STREAM);
    let sse = !input.events.is_empty() || is_event_stream(input.response_content_type);
    let parsed = if aws_stream {
        *stream = true;
        let events = bedrock::event_stream(input.response_body, w);
        let mut p = match api {
            LlmApi::BedrockConverse => bedrock::stream(&events, w),
            LlmApi::AnthropicMessages => anthropic::stream(&events, w),
            _ => {
                w.push("An AWS event stream from an API that doesn't use one");
                ParsedResponse::default()
            }
        };
        p.chunks = Some(u32::try_from(events.len()).unwrap_or(u32::MAX));
        p
    } else if sse {
        *stream = true;
        let parsed_events;
        let events = if input.events.is_empty() {
            // A stream that is still open may end in a partial event: leave it for later.
            let raw = if input.finished {
                crate::sse::parse_all(input.response_body)
            } else {
                crate::sse::SseParser::new().push(input.response_body)
            };
            parsed_events = raw
                .into_iter()
                .map(|e| StreamEvent {
                    event: e.event,
                    data: e.data,
                    at_ms: None,
                })
                .collect::<Vec<_>>();
            &parsed_events[..]
        } else {
            input.events
        };
        let mut p = match api {
            LlmApi::OpenAiChat => openai::chat_stream(events, w),
            LlmApi::OpenAiCompletions => openai::completions_stream(events, w),
            LlmApi::OpenAiResponses => responses::stream(events, w),
            LlmApi::AnthropicMessages => anthropic::stream(events, w),
            LlmApi::Gemini => gemini::stream(events, w),
            LlmApi::OllamaChat | LlmApi::OllamaGenerate => ollama::sse(api, events, w),
            LlmApi::BedrockConverse => bedrock::stream(events, w),
        };
        p.chunks = Some(u32::try_from(events.len()).unwrap_or(u32::MAX));
        p
    } else if matches!(api, LlmApi::OllamaChat | LlmApi::OllamaGenerate) {
        let lines = ndjson(body, w);
        if lines.len() > 1 {
            *stream = true;
        }
        let mut p = ollama::chunks(api, &lines, w);
        if *stream {
            p.chunks = Some(u32::try_from(lines.len()).unwrap_or(u32::MAX));
        }
        p
    } else if api == LlmApi::Gemini && body.first() == Some(&b'[') {
        *stream = true;
        let chunks = json_array_prefix(body, w);
        let mut p = gemini::chunks(chunks.iter().map(|c| (c, None)), w);
        p.chunks = Some(u32::try_from(chunks.len()).unwrap_or(u32::MAX));
        p
    } else if body.is_empty() {
        if input.finished {
            w.push("The response has no body");
        }
        return (None, None, timing);
    } else {
        match parse_json(body) {
            Ok(v) => {
                let mut p = match api {
                    LlmApi::OpenAiChat => openai::chat_response(&v, w),
                    LlmApi::OpenAiCompletions => openai::completions_response(&v, w),
                    LlmApi::OpenAiResponses => responses::response(&v, w),
                    LlmApi::AnthropicMessages => anthropic::response(&v, w),
                    LlmApi::Gemini => gemini::chunks(std::iter::once((&v, None)), w),
                    LlmApi::OllamaChat | LlmApi::OllamaGenerate => {
                        ollama::chunks(api, std::slice::from_ref(&v), w)
                    }
                    LlmApi::BedrockConverse => bedrock::response(&v, w),
                };
                p.complete = true;
                p
            }
            Err(e) => {
                if input.finished || !input.response_truncated {
                    w.push(format!("The response body is not valid JSON: {e}"));
                }
                return (None, None, timing);
            }
        }
    };

    if *stream && !parsed.complete && input.finished && parsed.error.is_none() {
        w.push("The stream ended before its final event: the response may be incomplete");
    }
    if let Some(first) = parsed.first_token_ms {
        timing.ttft_ms = Some((first - input.sent_ms.unwrap_or(0.0)).max(0.0));
    }
    timing.chunks = parsed.chunks;
    let output = parsed.response.usage.as_ref().and_then(|u| u.output_tokens);
    timing.tokens_per_second = parsed.server_tps.or_else(|| {
        let total = timing.total_ms?;
        let generating = match timing.ttft_ms {
            Some(ttft) if *stream => total - ttft,
            _ => total,
        };
        let out = output.filter(|n| *n > 0)? as f64;
        (generating >= 10.0).then(|| round2(out / (generating / 1000.0)))
    });
    let response = parsed.response;
    let empty = response.messages.is_empty()
        && response.usage.is_none()
        && response.stop_reason.is_none()
        && response.id.is_none();
    (
        (!empty || parsed.error.is_none()).then_some(response),
        parsed.error,
        timing,
    )
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Lines of an NDJSON body (blank and invalid lines skipped with a warning).
fn ndjson(body: &[u8], w: &mut Warnings) -> Vec<Value> {
    let mut out = Vec::new();
    let mut bad = 0usize;
    let lines: Vec<&[u8]> = body.split(|b| *b == b'\n').collect();
    let last = lines.len().saturating_sub(1);
    for (i, line) in lines.into_iter().enumerate() {
        let line = line.trim_ascii();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Value>(line) {
            Ok(v) => out.push(v),
            // A cut-off last line is a truncated stream, reported elsewhere.
            Err(_) if i == last => {}
            Err(_) => bad += 1,
        }
    }
    if bad > 0 {
        w.push(format!("Skipped {bad} malformed stream lines"));
    }
    out
}

/// The complete elements of a (possibly cut-off) JSON array, as Gemini streams without
/// `alt=sse`: `[{…},\r\n{…}…`.
fn json_array_prefix(body: &[u8], w: &mut Warnings) -> Vec<Value> {
    if let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(body) {
        return items;
    }
    // Incomplete: parse element by element.
    let mut out = Vec::new();
    let mut rest = &body[1..];
    loop {
        rest = rest.trim_ascii_start();
        rest = rest.strip_prefix(b",").unwrap_or(rest).trim_ascii_start();
        if rest.is_empty() || rest[0] == b']' {
            break;
        }
        let mut de = serde_json::Deserializer::from_slice(rest).into_iter::<Value>();
        match de.next() {
            Some(Ok(v)) => {
                let used = de.byte_offset();
                out.push(v);
                rest = &rest[used..];
            }
            _ => {
                w.push("The streamed JSON array was cut off");
                break;
            }
        }
    }
    out
}

/// The error described by an error response body (any provider's shape).
pub(crate) fn error_from_body(body: &[u8], status: u16) -> LlmError {
    let mut e = match parse_json(body) {
        Ok(v) => error_from_value(&v).unwrap_or_default(),
        Err(_) => LlmError::default(),
    };
    e.status = Some(status);
    if e.message.is_empty() {
        let text = String::from_utf8_lossy(crate::util::skip_bom_ws(body));
        let text = text.trim();
        e.message = if text.is_empty() {
            format!("HTTP {status}")
        } else {
            truncate(text, 2000)
        };
    }
    e
}

/// An error object in any provider's shape: `{"error": {"message", "type", "code"}}` (OpenAI),
/// `{"type": "error", "error": {"type", "message"}}` (Anthropic), `{"error": {"code",
/// "message", "status"}}` (Gemini), `{"error": "text"}` (Ollama), `[{"error": …}]`, `{"detail"}`.
pub(crate) fn error_from_value(v: &Value) -> Option<LlmError> {
    if let Value::Array(items) = v {
        return items.iter().find_map(error_from_value);
    }
    let obj = v.as_object()?;
    let inner = obj.get("error");
    match inner {
        Some(Value::String(s)) => Some(LlmError {
            message: s.clone(),
            ..LlmError::default()
        }),
        Some(e @ Value::Object(_)) => Some(LlmError {
            status: None,
            kind: str_of(e, "type")
                .or_else(|| str_of(e, "status"))
                .map(str::to_string),
            code: e.get("code").and_then(scalar_string),
            message: str_of(e, "message")
                .map(str::to_string)
                .unwrap_or_else(|| e.to_string()),
        }),
        _ => {
            let message =
                obj.get("detail")
                    .or_else(|| obj.get("message"))
                    .and_then(|d| match d {
                        Value::String(s) => Some(s.clone()),
                        Value::Null => None,
                        other => Some(other.to_string()),
                    })?;
            Some(LlmError {
                message,
                ..LlmError::default()
            })
        }
    }
}

// =============================================================================================
// Shared helpers for the provider modules
// =============================================================================================

pub(crate) fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// First present key of several spellings (`inlineData` / `inline_data`).
pub(crate) fn get_any<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| v.get(*k).filter(|x| !x.is_null()))
}

pub(crate) fn u64_of(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(|x| {
        x.as_u64()
            .or_else(|| x.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
    })
}

/// A string or number as text.
pub(crate) fn scalar_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Text from a string or an array of `{type: "text", text}` blocks (other blocks ignored).
pub(crate) fn plain_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let texts: Vec<&str> = items
                .iter()
                .filter_map(|b| match b {
                    Value::String(s) => Some(s.as_str()),
                    _ => str_of(b, "text"),
                })
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n\n"))
        }
        _ => None,
    }
}

/// Parse tool-call arguments given as a JSON string; the raw string when it isn't JSON.
pub(crate) fn parse_arguments(raw: &str, name: &str, w: &mut Warnings) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Value::Object(Map::new());
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(v) => v,
        Err(_) => {
            w.push(format!(
                "The arguments of tool call `{name}` are not valid JSON"
            ));
            Value::String(raw.to_string())
        }
    }
}

/// Decoded size of standard base64 text.
pub(crate) fn base64_size(data: &str) -> u64 {
    let len = data.trim_end().len();
    let pad = data
        .trim_end()
        .bytes()
        .rev()
        .take_while(|b| *b == b'=')
        .count();
    ((len / 4) * 3 + (len % 4).saturating_sub(1)).saturating_sub(pad.min(2)) as u64
}

/// An image part from inline base64 data.
pub(crate) fn image_data(media_type: Option<&str>, data: &str) -> LlmPart {
    let size = base64_size(data);
    LlmPart::Image {
        media_type: media_type.map(str::to_string),
        data: (data.len() <= MAX_INLINE_IMAGE).then(|| data.to_string()),
        url: None,
        size: Some(size),
    }
}

/// An image part from a URL (`data:` URLs become inline data).
pub(crate) fn image_url(url: &str) -> LlmPart {
    if let Some(rest) = url.strip_prefix("data:")
        && let Some((meta, data)) = rest.split_once(',')
        && meta.ends_with(";base64")
    {
        let media_type = meta.trim_end_matches(";base64");
        return image_data((!media_type.is_empty()).then_some(media_type), data);
    }
    LlmPart::Image {
        media_type: None,
        data: None,
        url: Some(url.to_string()),
        size: None,
    }
}

/// A file part (documents, audio, files): metadata only.
pub(crate) fn file_part(
    label: &str,
    media_type: Option<&str>,
    name: Option<&str>,
    url: Option<&str>,
    base64: Option<&str>,
) -> LlmPart {
    LlmPart::File {
        label: label.to_string(),
        media_type: media_type.map(str::to_string),
        name: name.map(str::to_string),
        url: url.filter(|u| !u.starts_with("data:")).map(str::to_string),
        size: base64.map(base64_size),
    }
}

/// Append text to a message, merging with a trailing text part.
pub(crate) fn push_text(content: &mut Vec<LlmPart>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(LlmPart::Text { text: last }) = content.last_mut() {
        last.push_str(text);
    } else {
        content.push(LlmPart::Text {
            text: text.to_string(),
        });
    }
}

/// Append thinking text, merging with a trailing thinking part.
pub(crate) fn push_thinking(content: &mut Vec<LlmPart>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(LlmPart::Thinking {
        text: last,
        redacted: false,
    }) = content.last_mut()
    {
        last.push_str(text);
    } else {
        content.push(LlmPart::Thinking {
            text: text.to_string(),
            redacted: false,
        });
    }
}

/// Request parameters: every top-level field except the structural `skip` ones.
pub(crate) fn params_except(v: &Value, skip: &[&str]) -> Map<String, Value> {
    v.as_object()
        .map(|o| {
            o.iter()
                .filter(|(k, _)| !skip.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Leading system / developer messages become the system prompt.
pub(crate) fn hoist_system(messages: &mut Vec<LlmMessage>) -> Option<String> {
    let n = messages
        .iter()
        .take_while(|m| m.role == "system" || m.role == "developer")
        .count();
    if n == 0 {
        return None;
    }
    let texts: Vec<String> = messages
        .drain(..n)
        .map(|m| {
            m.content
                .into_iter()
                .filter_map(|p| match p {
                    LlmPart::Text { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .filter(|t| !t.is_empty())
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n\n"))
}

/// Join non-empty system texts.
pub(crate) fn join_system(parts: Vec<String>) -> Option<String> {
    let parts: Vec<String> = parts.into_iter().filter(|p| !p.trim().is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Usage with empty fields dropped.
pub(crate) fn usage_or_none(u: LlmUsage) -> Option<LlmUsage> {
    (!u.is_empty()).then_some(u)
}

/// Agent-friendly copy: inline image data dropped and texts cut to `max_chars` characters
/// (used by MCP so a conversation with megabytes of base64 doesn't flood the context).
pub fn compact(exchange: &mut LlmExchange, max_chars: usize) {
    fn cut(text: &mut String, max: usize) {
        if text.chars().count() > max {
            let total = text.chars().count();
            let kept: String = text.chars().take(max).collect();
            *text = format!("{kept}… [{} more characters]", total - max);
        }
    }
    fn parts(content: &mut [LlmPart], max: usize) {
        for p in content {
            match p {
                LlmPart::Text { text }
                | LlmPart::Thinking { text, .. }
                | LlmPart::Refusal { text } => cut(text, max),
                LlmPart::Image { data, .. } => *data = None,
                LlmPart::ToolResult { content, .. } => parts(content, max),
                LlmPart::ToolCall { arguments, .. } => {
                    if let Value::String(s) = arguments {
                        cut(s, max);
                    }
                }
                LlmPart::File { .. } | LlmPart::Other { .. } => {}
            }
        }
    }
    if let Some(s) = exchange.request.system.as_mut() {
        cut(s, max_chars);
    }
    for m in &mut exchange.request.messages {
        parts(&mut m.content, max_chars);
    }
    if let Some(r) = exchange.response.as_mut() {
        for m in &mut r.messages {
            parts(&mut m.content, max_chars);
        }
    }
}

/// Plain-text rendering of message content (Markdown-ish), e.g. for copying.
pub fn content_text(content: &[LlmPart]) -> String {
    let mut out = Vec::new();
    for p in content {
        match p {
            LlmPart::Text { text } => out.push(text.clone()),
            LlmPart::Refusal { text } => out.push(format!("[refusal] {text}")),
            LlmPart::Thinking { text, redacted } => {
                if *redacted {
                    out.push("[thinking redacted]".into());
                } else {
                    out.push(format!("[thinking] {text}"));
                }
            }
            LlmPart::Image {
                media_type, url, ..
            } => out.push(format!(
                "[image{}]",
                url.as_deref()
                    .or(media_type.as_deref())
                    .map(|s| format!(" {s}"))
                    .unwrap_or_default()
            )),
            LlmPart::File { label, name, .. } => out.push(format!(
                "[{label}{}]",
                name.as_deref().map(|n| format!(" {n}")).unwrap_or_default()
            )),
            LlmPart::ToolCall {
                name, arguments, ..
            } => out.push(format!("[tool call] {name}({arguments})")),
            LlmPart::ToolResult { content, name, .. } => out.push(format!(
                "[tool result{}] {}",
                name.as_deref().map(|n| format!(" {n}")).unwrap_or_default(),
                content_text(content)
            )),
            LlmPart::Other { label, .. } => out.push(format!("[{label}]")),
        }
    }
    out.join("\n\n")
}
