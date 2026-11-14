//! HTTP primitives that preserve wire fidelity (header order & casing, raw reason phrases).

use serde::{Deserialize, Serialize};
use std::fmt;

/// HTTP protocol version as seen on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum HttpVersion {
    #[serde(rename = "HTTP/0.9")]
    Http09,
    #[serde(rename = "HTTP/1.0")]
    Http10,
    #[default]
    #[serde(rename = "HTTP/1.1")]
    Http11,
    #[serde(rename = "HTTP/2")]
    H2,
    #[serde(rename = "HTTP/3")]
    H3,
}

impl HttpVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpVersion::Http09 => "HTTP/0.9",
            HttpVersion::Http10 => "HTTP/1.0",
            HttpVersion::Http11 => "HTTP/1.1",
            HttpVersion::H2 => "HTTP/2",
            HttpVersion::H3 => "HTTP/3",
        }
    }
}

impl fmt::Display for HttpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Ordered multi-map of headers preserving duplicates, order and casing.
///
/// Serialized as `[["Name","value"], ...]` (compact, order-preserving, JS-friendly).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Headers(pub Vec<(String, String)>);

impl Serialize for Headers {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Headers {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Accept both the canonical array-of-pairs form and a JSON object (convenient for
        // hand-written rule files / API callers).
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Pairs(Vec<(String, String)>),
            Map(serde_json::Map<String, serde_json::Value>),
        }
        Ok(match Repr::deserialize(d)? {
            Repr::Pairs(p) => Headers(p),
            Repr::Map(m) => {
                let mut out = Vec::with_capacity(m.len());
                for (k, v) in m {
                    match v {
                        serde_json::Value::Array(items) => {
                            for item in items {
                                out.push((k.clone(), json_value_to_string(item)));
                            }
                        }
                        other => out.push((k, json_value_to_string(other))),
                    }
                }
                Headers(out)
            }
        })
    }
}

fn json_value_to_string(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
}

impl Headers {
    pub fn new() -> Self {
        Headers(Vec::new())
    }

    pub fn with_capacity(n: usize) -> Self {
        Headers(Vec::with_capacity(n))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// First value for a header (case-insensitive name match).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// All values for a header (case-insensitive).
    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.0
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Append a header (keeps existing ones).
    pub fn append(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.0.push((name.into(), value.into()));
    }

    /// Replace all values of `name` with a single value, keeping the position of the first
    /// occurrence (or appending if absent).
    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        if let Some(pos) = self
            .0
            .iter()
            .position(|(k, _)| k.eq_ignore_ascii_case(name))
        {
            self.0[pos].1 = value;
            let mut i = pos + 1;
            while i < self.0.len() {
                if self.0[i].0.eq_ignore_ascii_case(name) {
                    self.0.remove(i);
                } else {
                    i += 1;
                }
            }
        } else {
            self.0.push((name.to_string(), value));
        }
    }

    /// Remove all values of a header; returns true if any were removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.0.len();
        self.0.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        before != self.0.len()
    }

    /// Parsed `Content-Length`, if present and valid.
    pub fn content_length(&self) -> Option<u64> {
        self.get("content-length")
            .and_then(|v| v.trim().parse().ok())
    }

    /// `Content-Type` essence (lowercased, without parameters), e.g. `application/json`.
    pub fn content_type_essence(&self) -> Option<String> {
        self.get("content-type").map(|ct| {
            ct.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
    }

    /// The `Content-Encoding` chain in the order it was applied (e.g. `["gzip"]`).
    pub fn content_encodings(&self) -> Vec<String> {
        self.get_all("content-encoding")
            .flat_map(|v| v.split(','))
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty() && s != "identity")
            .collect()
    }

    /// Approximate size of the header block in bytes (HTTP/1 framing).
    pub fn wire_size(&self) -> usize {
        self.0
            .iter()
            .map(|(k, v)| k.len() + v.len() + 4)
            .sum::<usize>()
            + 2
    }

    /// Convert from an `http::HeaderMap`. Note `HeaderMap` loses original casing (names
    /// are lowercased) — prefer capturing raw headers where the engine allows it.
    pub fn from_header_map(map: &http::HeaderMap) -> Self {
        let mut out = Headers::with_capacity(map.len());
        for (k, v) in map.iter() {
            out.append(k.as_str(), header_value_to_string(v.as_bytes()));
        }
        out
    }

    /// Convert into an `http::HeaderMap`, skipping invalid names/values.
    pub fn to_header_map(&self) -> http::HeaderMap {
        let mut map = http::HeaderMap::with_capacity(self.0.len());
        for (k, v) in &self.0 {
            if let (Ok(name), Ok(value)) = (
                http::HeaderName::from_bytes(k.as_bytes()),
                http::HeaderValue::from_bytes(&string_to_header_bytes(v)),
            ) {
                map.append(name, value);
            }
        }
        map
    }
}

impl FromIterator<(String, String)> for Headers {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Headers(iter.into_iter().collect())
    }
}

/// Decode header bytes: UTF-8 when valid, otherwise Latin-1 (lossless round trip).
pub fn header_value_to_string(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Encode a header string back to bytes: chars <= 0xFF become single bytes (Latin-1), others UTF-8.
pub fn string_to_header_bytes(s: &str) -> Vec<u8> {
    if s.is_ascii() {
        return s.as_bytes().to_vec();
    }
    if s.chars().all(|c| (c as u32) <= 0xFF) {
        s.chars().map(|c| c as u8).collect()
    } else {
        s.as_bytes().to_vec()
    }
}

/// Request head as captured by the proxy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestHead {
    pub method: String,
    /// Absolute URL as the client intended it, e.g. `https://example.com:8443/a?b=c`.
    pub url: String,
    /// `http` / `https` / `ws` / `wss`.
    pub scheme: String,
    /// Hostname (no port), lowercase, IPv6 without brackets.
    pub host: String,
    pub port: u16,
    /// Path including query string (`/a?b=c`), as sent on the wire.
    pub path: String,
    pub http_version: HttpVersion,
    pub headers: Headers,
}

impl RequestHead {
    /// The query string part of `path` (without '?'), if any.
    pub fn query(&self) -> Option<&str> {
        self.path.split_once('?').map(|(_, q)| q)
    }

    /// Path without the query string.
    pub fn path_only(&self) -> &str {
        self.path.split('?').next().unwrap_or("")
    }

    /// `host[:port]` where the port is omitted when it's the scheme default.
    pub fn authority(&self) -> String {
        let default = matches!(
            (self.scheme.as_str(), self.port),
            ("http", 80) | ("ws", 80) | ("https", 443) | ("wss", 443)
        );
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if default {
            host
        } else {
            format!("{}:{}", host, self.port)
        }
    }

    pub fn is_tls(&self) -> bool {
        matches!(self.scheme.as_str(), "https" | "wss")
    }
}

/// Response head as captured by the proxy (or generated by a rule).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseHead {
    pub status: u16,
    /// Reason phrase as sent (HTTP/1), or the canonical one for HTTP/2.
    pub status_message: String,
    pub http_version: HttpVersion,
    pub headers: Headers,
}

/// Canonical reason phrase for a status code ("" if unknown).
pub fn canonical_reason(status: u16) -> &'static str {
    http::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_preserve_order_and_case() {
        let mut h = Headers::new();
        h.append("X-First", "1");
        h.append("content-type", "text/plain; charset=utf-8");
        h.append("X-First", "2");
        assert_eq!(h.get("x-first"), Some("1"));
        assert_eq!(h.get_all("X-FIRST").collect::<Vec<_>>(), vec!["1", "2"]);
        assert_eq!(h.content_type_essence().as_deref(), Some("text/plain"));
        h.set("x-first", "3");
        assert_eq!(
            h.0,
            vec![
                ("X-First".to_string(), "3".to_string()),
                (
                    "content-type".to_string(),
                    "text/plain; charset=utf-8".to_string()
                ),
            ]
        );
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(
            json,
            r#"[["X-First","3"],["content-type","text/plain; charset=utf-8"]]"#
        );
        let back: Headers = serde_json::from_str(&json).unwrap();
        assert_eq!(back, h);
        let from_map: Headers = serde_json::from_str(r#"{"a":"1","b":["2","3"]}"#).unwrap();
        assert_eq!(from_map.len(), 3);
    }

    #[test]
    fn latin1_roundtrip() {
        let raw = [0x63u8, 0xE9, 0x74];
        let s = header_value_to_string(&raw);
        assert_eq!(string_to_header_bytes(&s), raw.to_vec());
    }

    #[test]
    fn authority_omits_default_port() {
        let head = RequestHead {
            method: "GET".into(),
            url: "https://example.com/".into(),
            scheme: "https".into(),
            host: "example.com".into(),
            port: 443,
            path: "/?q=1".into(),
            http_version: HttpVersion::H2,
            headers: Headers::new(),
        };
        assert_eq!(head.authority(), "example.com");
        assert_eq!(head.query(), Some("q=1"));
        assert_eq!(head.path_only(), "/");
    }
}
