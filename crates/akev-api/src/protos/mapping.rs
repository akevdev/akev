//! User mappings: URL pattern → protobuf message type, for protobuf bodies that neither name
//! their type in the Content-Type nor are gRPC.
//!
//! Patterns use `*` as the only wildcard (any run of characters, `/` included) and are matched
//! case-insensitively. Their form decides what they're matched against:
//!
//! | Pattern | Matched against |
//! |---|---|
//! | `/v1/items/*` (starts with `/`) | the path |
//! | `https://api.example.com/v1/*` (has `://`) | the whole URL |
//! | `api.example.com/v1/*` | host (with `:port` when not the default) + path |
//! | `*.example.com` (no `/`) | the host |
//!
//! Path and URL patterns match with or without the query string.

use akev_core::Side;
use serde::{Deserialize, Serialize};

/// Most mappings accepted.
pub const MAX_MAPPINGS: usize = 500;
/// Longest pattern accepted.
pub const MAX_PATTERN_LEN: usize = 2048;
/// Longest message type name accepted.
pub const MAX_TYPE_LEN: usize = 512;

/// Which bodies of matching exchanges a mapping applies to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MappingSide {
    /// Request bodies only.
    Request,
    /// Response bodies only.
    Response,
    /// Both.
    #[default]
    Both,
}

impl MappingSide {
    fn includes(self, side: Side) -> bool {
        match self {
            MappingSide::Both => true,
            MappingSide::Request => side == Side::Request,
            MappingSide::Response => side == Side::Response,
        }
    }
}

/// `ProtoMapping` (see the HTTP API reference, akev.dev/docs/api).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtoMapping {
    /// Stable id (generated when missing).
    #[serde(default)]
    pub id: String,
    /// Disabled mappings are kept but never applied.
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    /// URL pattern (see the module docs).
    pub pattern: String,
    /// Request bodies, response bodies or both.
    #[serde(default)]
    pub side: MappingSide,
    /// Fully-qualified message type (`shop.v1.Item`; a leading `.` is dropped).
    pub message_type: String,
}

fn enabled_default() -> bool {
    true
}

impl ProtoMapping {
    /// Whether this (enabled) mapping applies to the `side` body of a request to `url`.
    pub fn matches(&self, url: &Target<'_>, side: Side) -> bool {
        self.enabled && self.side.includes(side) && url.matches(&self.pattern)
    }
}

/// Validate and normalize a mapping list: trimmed patterns and types, ids filled in (and made
/// unique). Errors name the offending entry (1-based).
pub fn normalize(mappings: Vec<ProtoMapping>) -> Result<Vec<ProtoMapping>, String> {
    if mappings.len() > MAX_MAPPINGS {
        return Err(format!("at most {MAX_MAPPINGS} mappings are supported"));
    }
    let mut seen = std::collections::HashSet::new();
    mappings
        .into_iter()
        .enumerate()
        .map(|(i, mut m)| {
            let n = i + 1;
            m.pattern = m.pattern.trim().to_string();
            m.message_type = m.message_type.trim().trim_start_matches('.').to_string();
            if m.pattern.is_empty() {
                return Err(format!("mapping {n}: the URL pattern is empty"));
            }
            if m.pattern.len() > MAX_PATTERN_LEN || m.pattern.chars().any(char::is_control) {
                return Err(format!("mapping {n}: invalid URL pattern"));
            }
            if !valid_type_name(&m.message_type) {
                return Err(format!(
                    "mapping {n}: {:?} is not a message type name (e.g. `shop.v1.Item`)",
                    m.message_type
                ));
            }
            m.id = m.id.trim().to_string();
            if m.id.is_empty() || m.id.len() > 64 || !seen.insert(m.id.clone()) {
                m.id = uuid::Uuid::new_v4().simple().to_string();
                seen.insert(m.id.clone());
            }
            Ok(m)
        })
        .collect()
}

/// `package.Message` style names: identifiers separated by dots.
pub fn valid_type_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TYPE_LEN
        && name.split('.').all(|part| {
            part.chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The parts of a request URL patterns are matched against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target<'a> {
    /// The URL as captured.
    pub url: &'a str,
    /// `host` or `host:port` (port only when not the scheme's default), lowercase.
    pub authority: String,
    /// Path without the query.
    pub path: String,
    /// Query string without `?` (None when absent).
    pub query: Option<String>,
}

impl<'a> Target<'a> {
    /// Split a request URL.
    pub fn new(url: &'a str) -> Self {
        let Ok(parsed) = url::Url::parse(url) else {
            return Target {
                url,
                path: url.to_string(),
                ..Target::default()
            };
        };
        let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
        let authority = match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        };
        Target {
            url,
            authority,
            path: parsed.path().to_string(),
            query: parsed.query().map(str::to_string),
        }
    }

    fn with_query(&self, base: &str) -> Option<String> {
        self.query.as_ref().map(|q| format!("{base}?{q}"))
    }

    /// Whether `pattern` matches this URL (see the module docs).
    pub fn matches(&self, pattern: &str) -> bool {
        let candidates: Vec<String> = if pattern.starts_with('/') {
            let mut c = vec![self.path.clone()];
            c.extend(self.with_query(&self.path));
            c
        } else if pattern.contains("://") {
            let base = self.url.split(['?', '#']).next().unwrap_or(self.url);
            vec![base.to_string(), self.url.to_string()]
        } else if !pattern.contains('/') {
            let host = self.authority.split(':').next().unwrap_or_default();
            vec![self.authority.clone(), host.to_string()]
        } else {
            let base = format!("{}{}", self.authority, self.path);
            let mut c = vec![base.clone()];
            c.extend(self.with_query(&base));
            c
        };
        candidates.iter().any(|c| wildcard_match(pattern, c))
    }
}

/// Case-insensitive match of `text` against `pattern`, where `*` matches any run of
/// characters. Linear-time greedy matching with backtracking to the last `*` (O(n·m) worst
/// case, no recursion).
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            // Collapse runs of stars.
            while pi < p.len() && p[pi] == '*' {
                pi += 1;
            }
            star = Some((pi, ti));
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(pattern: &str, side: MappingSide) -> ProtoMapping {
        ProtoMapping {
            id: String::new(),
            enabled: true,
            pattern: pattern.into(),
            side,
            message_type: "a.B".into(),
        }
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("a*c", "abbbc"));
        assert!(wildcard_match("a*c", "ac"));
        assert!(!wildcard_match("a*c", "acb"));
        assert!(wildcard_match("*.Example.com", "api.example.COM"));
        assert!(wildcard_match("/v1/*/items", "/v1/x/y/items"));
        assert!(!wildcard_match("/v1/items", "/v1/items/2"));
        assert!(wildcard_match("a**b", "ab"));
        assert!(wildcard_match("?x", "?x"), "? is literal");
        assert!(!wildcard_match("?x", "ax"));
        // Pathological input stays fast.
        let long = "a".repeat(5000);
        assert!(!wildcard_match(&format!("{}b", "*a".repeat(50)), &long));
    }

    #[test]
    fn pattern_forms() {
        let t = Target::new("https://API.example.com/v1/items/42?fields=name");
        assert_eq!(t.authority, "api.example.com");
        assert_eq!(t.path, "/v1/items/42");
        for yes in [
            "/v1/items/*",
            "/v1/items/42",
            "/v1/items/42?fields=name",
            "/v1/*?fields=*",
            "https://api.example.com/v1/*",
            "https://api.example.com/v1/items/42",
            "*://api.example.com/*",
            "api.example.com/v1/items/*",
            "api.example.com/v1/items/42?fields=name",
            "*.example.com",
            "api.example.com",
            "*example.com/*",
        ] {
            assert!(t.matches(yes), "{yes}");
        }
        for no in [
            "/v1/items",
            "/v2/*",
            "http://api.example.com/*",
            "other.example.com/*",
            "example.com",
            "api.example.com/v1",
        ] {
            assert!(!t.matches(no), "{no}");
        }
        let port = Target::new("http://localhost:8080/rpc");
        assert_eq!(port.authority, "localhost:8080");
        assert!(port.matches("localhost:8080/rpc"));
        assert!(port.matches("localhost"));
        assert!(port.matches("localhost:*/*"));
        assert!(!port.matches("localhost/rpc"));
        // Unparseable URLs still match path patterns.
        assert!(Target::new("/just/a/path").matches("/just/*"));
    }

    #[test]
    fn sides_and_enabled() {
        let t = Target::new("https://x.test/a");
        assert!(mapping("/a", MappingSide::Both).matches(&t, Side::Request));
        assert!(mapping("/a", MappingSide::Response).matches(&t, Side::Response));
        assert!(!mapping("/a", MappingSide::Response).matches(&t, Side::Request));
        assert!(!mapping("/a", MappingSide::Request).matches(&t, Side::Response));
        let mut off = mapping("/a", MappingSide::Both);
        off.enabled = false;
        assert!(!off.matches(&t, Side::Request));
    }

    #[test]
    fn normalizing() {
        let out = normalize(vec![
            ProtoMapping {
                id: "keep".into(),
                message_type: " .shop.v1.Item ".into(),
                pattern: "  /items/* ".into(),
                ..mapping("x", MappingSide::Both)
            },
            ProtoMapping {
                id: "keep".into(),
                ..mapping("/b", MappingSide::Request)
            },
        ])
        .unwrap();
        assert_eq!(out[0].id, "keep");
        assert_eq!(out[0].pattern, "/items/*");
        assert_eq!(out[0].message_type, "shop.v1.Item");
        assert_ne!(out[1].id, "keep", "duplicate ids are replaced");
        assert!(!out[1].id.is_empty());
        let err = normalize(vec![mapping("  ", MappingSide::Both)]).unwrap_err();
        assert!(err.contains("mapping 1") && err.contains("empty"), "{err}");
        let mut bad = mapping("/a", MappingSide::Both);
        bad.message_type = "shop..Item".into();
        assert!(
            normalize(vec![bad])
                .unwrap_err()
                .contains("not a message type")
        );
        assert!(normalize(vec![mapping("/a", MappingSide::Both); MAX_MAPPINGS + 1]).is_err());
        // Wire format: defaults for the optional fields.
        let m: ProtoMapping =
            serde_json::from_str(r#"{"pattern":"/a","messageType":"a.B"}"#).unwrap();
        assert!(m.enabled);
        assert_eq!(m.side, MappingSide::Both);
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({"id": "", "enabled": true, "pattern": "/a", "side": "both", "messageType": "a.B"})
        );
    }

    #[test]
    fn type_names() {
        for ok in ["Item", "shop.v1.Item", "_x.Y_2"] {
            assert!(valid_type_name(ok), "{ok}");
        }
        for bad in ["", ".", "a.", "1a.B", "a-b.C", "a b"] {
            assert!(!valid_type_name(bad), "{bad}");
        }
    }
}
