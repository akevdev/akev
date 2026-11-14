//! JSON helpers: deciding whether a body can be re-expressed as a native literal, and printing
//! such literals in each language's syntax.

use serde_json::Value;

/// Parse `text` as a JSON object/array, returning the value only when re-serialising it yields
/// the same token stream (strings compared by decoded value). This rejects duplicate keys
/// (which `serde_json` collapses) and numbers whose spelling would change (`1E2`, `1.50`, huge
/// integers), so emitting the value natively never changes the body's meaning.
pub(crate) fn parse_faithful(text: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(text).ok()?;
    if !matches!(value, Value::Object(_) | Value::Array(_)) {
        return None;
    }
    let reserialized = serde_json::to_string(&value).ok()?;
    let mut a = Tokens { s: text, pos: 0 };
    let mut b = Tokens {
        s: &reserialized,
        pos: 0,
    };
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Some(value),
            (Some(x), Some(y)) if x == y => {}
            (Some(x), Some(y)) if x.starts_with('"') && y.starts_with('"') => {
                let x: String = serde_json::from_str(x).ok()?;
                let y: String = serde_json::from_str(y).ok()?;
                if x != y {
                    return None;
                }
            }
            _ => return None,
        }
    }
}

/// Minimal tokenizer over already-validated JSON.
struct Tokens<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Iterator for Tokens<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let bytes = self.s.as_bytes();
        while self.pos < bytes.len() && matches!(bytes[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
        let start = self.pos;
        let first = *bytes.get(start)?;
        match first {
            b'{' | b'}' | b'[' | b']' | b':' | b',' => self.pos += 1,
            b'"' => {
                self.pos += 1;
                while self.pos < bytes.len() {
                    match bytes[self.pos] {
                        b'\\' => self.pos += 2,
                        b'"' => {
                            self.pos += 1;
                            break;
                        }
                        _ => self.pos += 1,
                    }
                }
            }
            _ => {
                while self.pos < bytes.len()
                    && !matches!(
                        bytes[self.pos],
                        b' ' | b'\t'
                            | b'\n'
                            | b'\r'
                            | b'{'
                            | b'}'
                            | b'['
                            | b']'
                            | b':'
                            | b','
                            | b'"'
                    )
                {
                    self.pos += 1;
                }
            }
        }
        self.s.get(start..self.pos.min(bytes.len()))
    }
}

/// Every integer in `value` is exactly representable as an IEEE double (JavaScript numbers).
pub(crate) fn fits_js_numbers(value: &Value) -> bool {
    const MAX_SAFE: u64 = (1 << 53) - 1;
    match value {
        Value::Number(n) => match (n.as_u64(), n.as_i64()) {
            (Some(u), _) => u <= MAX_SAFE,
            (None, Some(i)) => i.unsigned_abs() <= MAX_SAFE,
            _ => true,
        },
        Value::Array(items) => items.iter().all(fits_js_numbers),
        Value::Object(map) => map.values().all(fits_js_numbers),
        _ => true,
    }
}

/// Can `value` be written as nested PHP arrays and `json_encode`d back to the same JSON?
/// PHP turns integer-like string keys into integer keys (so `{"0":1}` would encode as a list)
/// and has no unsigned 64-bit integers.
pub(crate) fn fits_php_arrays(value: &Value) -> bool {
    match value {
        Value::Number(n) => n.is_f64() || n.as_i64().is_some(),
        Value::Array(items) => items.iter().all(fits_php_arrays),
        Value::Object(map) => {
            map.keys().all(|k| !is_php_integer_key(k)) && map.values().all(fits_php_arrays)
        }
        _ => true,
    }
}

fn is_php_integer_key(key: &str) -> bool {
    let digits = key.strip_prefix('-').unwrap_or(key);
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && !(key.starts_with('-') && digits == "0")
}

/// How a language spells JSON-ish literals.
pub(crate) struct Syntax {
    pub indent: &'static str,
    pub null: &'static str,
    pub true_lit: &'static str,
    pub false_lit: &'static str,
    pub object_open: &'static str,
    pub object_close: &'static str,
    pub empty_object: &'static str,
    pub array_open: &'static str,
    pub array_close: &'static str,
    pub key_sep: &'static str,
    pub trailing_comma: bool,
    pub string: fn(&str) -> String,
    pub key: fn(&str) -> String,
}

/// Maximum width of an inline array of scalars.
const INLINE_ARRAY_WIDTH: usize = 60;

/// Render `value` as a literal. `base` is the indentation of the line the literal starts on;
/// nested lines are indented relative to it.
pub(crate) fn render(value: &Value, syn: &Syntax, base: &str) -> String {
    let mut out = String::new();
    write_value(&mut out, value, syn, base, 0);
    out
}

fn write_scalar(out: &mut String, value: &Value, syn: &Syntax) {
    match value {
        Value::Null => out.push_str(syn.null),
        Value::Bool(true) => out.push_str(syn.true_lit),
        Value::Bool(false) => out.push_str(syn.false_lit),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&(syn.string)(s)),
        Value::Array(_) | Value::Object(_) => {}
    }
}

fn push_indent(out: &mut String, base: &str, unit: &str, level: usize) {
    out.push_str(base);
    for _ in 0..level {
        out.push_str(unit);
    }
}

fn write_value(out: &mut String, value: &Value, syn: &Syntax, base: &str, level: usize) {
    match value {
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str(syn.array_open);
                out.push_str(syn.array_close);
                return;
            }
            if items
                .iter()
                .all(|v| !matches!(v, Value::Array(_) | Value::Object(_)))
            {
                let mut inline = String::from(syn.array_open);
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        inline.push_str(", ");
                    }
                    write_scalar(&mut inline, item, syn);
                }
                inline.push_str(syn.array_close);
                if inline.chars().count() <= INLINE_ARRAY_WIDTH && !inline.contains('\n') {
                    out.push_str(&inline);
                    return;
                }
            }
            out.push_str(syn.array_open);
            out.push('\n');
            for (i, item) in items.iter().enumerate() {
                push_indent(out, base, syn.indent, level + 1);
                write_value(out, item, syn, base, level + 1);
                if i + 1 < items.len() || syn.trailing_comma {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, base, syn.indent, level);
            out.push_str(syn.array_close);
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str(syn.empty_object);
                return;
            }
            out.push_str(syn.object_open);
            out.push('\n');
            let len = map.len();
            for (i, (key, item)) in map.iter().enumerate() {
                push_indent(out, base, syn.indent, level + 1);
                out.push_str(&(syn.key)(key));
                out.push_str(syn.key_sep);
                write_value(out, item, syn, base, level + 1);
                if i + 1 < len || syn.trailing_comma {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, base, syn.indent, level);
            out.push_str(syn.object_close);
        }
        scalar => write_scalar(out, scalar, syn),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faithful_accepts_equivalent_spellings() {
        assert!(parse_faithful(r#"{"a": 1, "b": [true, null, "x"]}"#).is_some());
        assert!(parse_faithful("{\n  \"a\": \"\\u00e9\\/\"\n}").is_some());
        assert!(parse_faithful(r#"[1.5, -2, 18446744073709551615]"#).is_some());
        assert!(parse_faithful(r#"{"a":{"b":{}}}"#).is_some());
    }

    #[test]
    fn faithful_rejects_lossy_values() {
        assert!(parse_faithful(r#"{"a":1,"a":2}"#).is_none());
        assert!(parse_faithful(r#"[1E2]"#).is_none());
        assert!(parse_faithful(r#"[1.50]"#).is_none());
        assert!(parse_faithful(r#"[123456789012345678901234567890]"#).is_none());
        assert!(parse_faithful(r#""just a string""#).is_none());
        assert!(parse_faithful(r#"{"a":1"#).is_none());
        assert!(parse_faithful("42").is_none());
    }

    #[test]
    fn number_range_checks() {
        let v: Value = serde_json::from_str("[9007199254740991, -9007199254740991, 1.5]").unwrap();
        assert!(fits_js_numbers(&v));
        let v: Value = serde_json::from_str("[9007199254740993]").unwrap();
        assert!(!fits_js_numbers(&v));
        let v: Value = serde_json::from_str(r#"{"a":[1,{"b":2}],"01":1,"-0":1}"#).unwrap();
        assert!(fits_php_arrays(&v));
        let v: Value = serde_json::from_str(r#"{"0":1}"#).unwrap();
        assert!(!fits_php_arrays(&v));
        let v: Value = serde_json::from_str(r#"{"-5":1}"#).unwrap();
        assert!(!fits_php_arrays(&v));
        let v: Value = serde_json::from_str("[18446744073709551615]").unwrap();
        assert!(!fits_php_arrays(&v));
    }

    #[test]
    fn renders_nested_literal() {
        let syn = Syntax {
            indent: "  ",
            null: "null",
            true_lit: "true",
            false_lit: "false",
            object_open: "{",
            object_close: "}",
            empty_object: "{}",
            array_open: "[",
            array_close: "]",
            key_sep: ": ",
            trailing_comma: false,
            string: |s| format!("{s:?}"),
            key: |s| format!("{s:?}"),
        };
        let v: Value =
            serde_json::from_str(r#"{"a":[1,2],"b":{"c":null,"d":[{}]},"e":[]}"#).unwrap();
        assert_eq!(
            render(&v, &syn, "  "),
            "{\n    \"a\": [1, 2],\n    \"b\": {\n      \"c\": null,\n      \"d\": [\n        {}\n      ]\n    },\n    \"e\": []\n  }"
        );
    }
}
