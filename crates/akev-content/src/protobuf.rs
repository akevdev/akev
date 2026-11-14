//! Schemaless protobuf wire decoding (like `protoc --decode_raw`).
//!
//! Wire types 0 (varint), 1 (64-bit), 2 (length-delimited) and 5 (32-bit) are decoded; the
//! deprecated group wire types 3/4 and the undefined 6/7 make the input invalid. Field number 0
//! and numbers above 2^29-1 are invalid too.
//!
//! Length-delimited payloads are ambiguous on the wire; the heuristics are:
//! 1. empty → empty string;
//! 2. valid UTF-8 consisting only of printable characters (plus tab/CR/LF) → string — printable
//!    text frequently also parses as a message, but text is the likelier reading;
//! 3. a payload that parses as a message, consuming every byte, with at least one field →
//!    nested message (packed repeated scalars typically fail this: their first byte decodes to
//!    field number 0 or an invalid wire type);
//! 4. other valid UTF-8 with few control characters → string;
//! 5. otherwise → bytes.
//!
//! Nesting is bounded ([`MAX_DEPTH`]), beyond which payloads are reported as bytes, so hostile
//! input cannot overflow the stack. The view path decodes under an output [`Budget`]: when it
//! runs out, the fields decoded so far are returned.

use serde::Serialize;

use crate::budget::{Budget, base64_len};
use crate::{ContentError, Result};

/// Maximum nested-message depth decoded.
pub const MAX_DEPTH: usize = 64;
const MAX_FIELD_NUMBER: u64 = (1 << 29) - 1;

/// A schemaless protobuf field (like `protoc --decode_raw`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    /// Field number (1..=2^29-1).
    pub number: u64,
    /// Wire type (0, 1, 2 or 5).
    pub wire_type: u8,
    /// Decoded value.
    pub value: Value,
}

/// A decoded field value.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Value {
    /// Varint. `value` is the raw unsigned value; `signed` is its ZigZag-decoded
    /// (`sint32`/`sint64`) interpretation. (The two's-complement `int64` reading is simply
    /// `value as i64`.)
    Varint {
        /// Raw unsigned value.
        value: u64,
        /// ZigZag-decoded value.
        signed: i64,
    },
    /// 64-bit little-endian value with its `double` reinterpretation.
    Fixed64 {
        /// Raw value.
        value: u64,
        /// The same bits as an IEEE-754 double.
        double: f64,
    },
    /// 32-bit little-endian value with its `float` reinterpretation.
    Fixed32 {
        /// Raw value.
        value: u32,
        /// The same bits as an IEEE-754 float.
        float: f32,
    },
    /// Length-delimited that parsed as a nested message.
    Message {
        /// The nested message's fields.
        fields: Vec<Field>,
    },
    /// Length-delimited valid UTF-8 text.
    String {
        /// The text.
        value: String,
    },
    /// Length-delimited bytes (base64 in JSON).
    Bytes {
        /// Standard padded base64 of the bytes.
        base64: String,
    },
}

/// Decode a raw protobuf message heuristically. Errors if the bytes aren't valid protobuf.
/// An empty input is a valid (empty) message.
pub fn decode_raw(bytes: &[u8]) -> Result<Vec<Field>> {
    decode_raw_budget(bytes, &mut Budget::unlimited())
}

/// [`decode_raw`] under an output budget (one unit per field, string/bytes contents as text).
/// When the budget runs out, the fields decoded so far are returned (a nested message being
/// decoded keeps its decoded fields) and the rest of the input is not looked at.
pub(crate) fn decode_raw_budget(bytes: &[u8], budget: &mut Budget) -> Result<Vec<Field>> {
    parse_message(bytes, 0, budget).map_err(|message| ContentError::Parse {
        what: "protobuf",
        message,
    })
}

/// Render decoded fields as JSON-ish text (`{ "1": "foo", "2": { "1": 42 } }`), repeated
/// field numbers become arrays.
///
/// Rendering rules: varints print as unsigned integers, except values with the top bit set,
/// which print as negative two's-complement numbers (that's how negative `int32`/`int64` are
/// encoded). 64/32-bit values print as their floating-point reading when that is a "plausible"
/// number (finite, and zero or within 1e-9..1e15 / 1e-6..1e9 in magnitude), otherwise as
/// integers. Strings print as strings, bytes as base64, messages as nested objects.
pub fn to_json(fields: &[Field]) -> serde_json::Value {
    let mut map = serde_json::Map::with_capacity(fields.len().min(64));
    let mut digits = [0u8; 20];
    for f in fields {
        let value = value_to_json(&f.value);
        // Format the key on the stack; a String is only allocated for a new key.
        let key = decimal(f.number, &mut digits);
        match map.get_mut(key) {
            Some(serde_json::Value::Array(items)) => items.push(value),
            Some(existing) => {
                let first = std::mem::take(existing);
                *existing = serde_json::Value::Array(vec![first, value]);
            }
            None => {
                map.insert(key.to_owned(), value);
            }
        }
    }
    serde_json::Value::Object(map)
}

/// Format `n` in decimal into `buf` without allocating.
fn decimal(mut n: u64, buf: &mut [u8; 20]) -> &str {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    // Only ASCII digits were written.
    std::str::from_utf8(&buf[i..]).unwrap_or("0")
}

fn value_to_json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Varint { value, .. } => {
            if *value > i64::MAX as u64 {
                J::from(*value as i64)
            } else {
                J::from(*value)
            }
        }
        Value::Fixed64 { value, double } => {
            let a = double.abs();
            if *value != 0 && double.is_finite() && (*double == 0.0 || (1e-9..=1e15).contains(&a)) {
                crate::util::json_f64(*double)
            } else {
                J::from(*value)
            }
        }
        Value::Fixed32 { value, float } => {
            let a = float.abs();
            if *value != 0 && float.is_finite() && (*float == 0.0 || (1e-6..=1e9).contains(&a)) {
                // Print the shortest representation of the f32 (0.1, not 0.10000000149).
                let shortest = float
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(f64::from(*float));
                crate::util::json_f64(shortest)
            } else {
                J::from(*value)
            }
        }
        Value::Message { fields } => to_json(fields),
        Value::String { value } => J::String(value.clone()),
        Value::Bytes { base64 } => J::String(base64.clone()),
    }
}

/// Read a base-128 varint (at most 10 bytes, the 10th may only carry one bit).
pub(crate) fn read_varint(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result: u64 = 0;
    for i in 0..10 {
        let b = *bytes.get(*pos)?;
        *pos += 1;
        if i == 9 && b > 1 {
            return None;
        }
        result |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some(result);
        }
    }
    None
}

fn zigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// Parse a message. Once the budget is exhausted this returns `Ok` with the fields decoded so far
/// (never an error), so callers can tell a partial decode from invalid input.
fn parse_message(
    bytes: &[u8],
    depth: usize,
    budget: &mut Budget,
) -> std::result::Result<Vec<Field>, String> {
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        if budget.exhausted() {
            return Ok(fields);
        }
        let start = pos;
        let tag = read_varint(bytes, &mut pos)
            .ok_or_else(|| format!("truncated or invalid tag at offset {start}"))?;
        let number = tag >> 3;
        let wire_type = (tag & 7) as u8;
        if number == 0 || number > MAX_FIELD_NUMBER {
            return Err(format!("invalid field number {number} at offset {start}"));
        }
        let value = match wire_type {
            0 => {
                let v = read_varint(bytes, &mut pos).ok_or_else(|| {
                    format!("truncated or invalid varint for field {number} at offset {start}")
                })?;
                if !budget.charge(1, 0) {
                    return Ok(fields);
                }
                Value::Varint {
                    value: v,
                    signed: zigzag(v),
                }
            }
            1 => {
                let raw: [u8; 8] = bytes
                    .get(pos..pos + 8)
                    .and_then(|s| s.try_into().ok())
                    .ok_or_else(|| {
                        format!("truncated 64-bit value for field {number} at offset {start}")
                    })?;
                pos += 8;
                let v = u64::from_le_bytes(raw);
                if !budget.charge(1, 0) {
                    return Ok(fields);
                }
                Value::Fixed64 {
                    value: v,
                    double: f64::from_bits(v),
                }
            }
            2 => {
                let len = read_varint(bytes, &mut pos).ok_or_else(|| {
                    format!("invalid length for field {number} at offset {start}")
                })?;
                let len = usize::try_from(len)
                    .map_err(|_| format!("length overflow at offset {start}"))?;
                let end = pos
                    .checked_add(len)
                    .filter(|&e| e <= bytes.len())
                    .ok_or_else(|| {
                        format!(
                            "length {len} of field {number} at offset {start} exceeds the message"
                        )
                    })?;
                let payload = &bytes[pos..end];
                pos = end;
                if !budget.charge(1, 0) {
                    return Ok(fields);
                }
                match classify(payload, depth, budget) {
                    Some(value) => value,
                    None => return Ok(fields),
                }
            }
            5 => {
                let raw: [u8; 4] = bytes
                    .get(pos..pos + 4)
                    .and_then(|s| s.try_into().ok())
                    .ok_or_else(|| {
                        format!("truncated 32-bit value for field {number} at offset {start}")
                    })?;
                pos += 4;
                let v = u32::from_le_bytes(raw);
                if !budget.charge(1, 0) {
                    return Ok(fields);
                }
                Value::Fixed32 {
                    value: v,
                    float: f32::from_bits(v),
                }
            }
            3 | 4 => {
                return Err(format!(
                    "unsupported group wire type {wire_type} at offset {start}"
                ));
            }
            _ => return Err(format!("invalid wire type {wire_type} at offset {start}")),
        };
        fields.push(Field {
            number,
            wire_type,
            value,
        });
    }
    Ok(fields)
}

fn is_printable(s: &str) -> bool {
    s.chars()
        .all(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
}

fn mostly_printable(s: &str) -> bool {
    let mut total = 0usize;
    let mut control = 0usize;
    for c in s.chars() {
        total += 1;
        if c.is_control() && !matches!(c, '\t' | '\n' | '\r') {
            control += 1;
        }
    }
    control * 10 <= total
}

/// Interpret a length-delimited payload (the field itself is already charged). `None` when the
/// budget can't hold the result.
fn classify(payload: &[u8], depth: usize, budget: &mut Budget) -> Option<Value> {
    if payload.is_empty() {
        return Some(Value::String {
            value: String::new(),
        });
    }
    let text = std::str::from_utf8(payload).ok();
    if let Some(s) = text
        && is_printable(s)
    {
        return budget.charge(0, s.len()).then(|| Value::String {
            value: s.to_owned(),
        });
    }
    if depth + 1 < MAX_DEPTH {
        // A speculative parse: what it charged is given back if the payload isn't a message.
        let before = *budget;
        match parse_message(payload, depth + 1, budget) {
            Ok(fields) if !fields.is_empty() => return Some(Value::Message { fields }),
            Ok(_) => {}
            Err(_) => *budget = before,
        }
        if budget.exhausted() {
            return None;
        }
    }
    if let Some(s) = text
        && mostly_printable(s)
    {
        return budget.charge(0, s.len()).then(|| Value::String {
            value: s.to_owned(),
        });
    }
    budget
        .charge(0, base64_len(payload.len()))
        .then(|| Value::Bytes {
            base64: crate::util::b64(payload),
        })
}

/// Fields that must validate before [`wire_valid`] accepts a body it stopped scanning early.
const MIN_FIELDS_FOR_EARLY_ACCEPT: usize = 3;

/// Shallow wire-format validation of a whole body (no nested decoding): every top-level field
/// must be well-formed and lie entirely within `bytes`. The walk only *reads* up to `scan_limit`
/// bytes of field headers (length-delimited payloads are skipped in O(1)), so huge bodies are
/// cheap: once the walk has passed `scan_limit` after at least
/// [`MIN_FIELDS_FOR_EARLY_ACCEPT`] valid fields, the body is accepted; otherwise the fields
/// must end exactly at the end of `bytes`. Requires at least one field.
///
/// Every length is checked against the real body (never against a prefix), which keeps random
/// binary data from passing via one oversized length field.
pub(crate) fn wire_valid(bytes: &[u8], scan_limit: usize) -> bool {
    let mut pos = 0;
    let mut count = 0usize;
    while pos < bytes.len() {
        if pos >= scan_limit && count >= MIN_FIELDS_FOR_EARLY_ACCEPT {
            return true;
        }
        let Some(tag) = read_varint(bytes, &mut pos) else {
            return false;
        };
        let number = tag >> 3;
        if number == 0 || number > MAX_FIELD_NUMBER {
            return false;
        }
        let end = match tag & 7 {
            0 => read_varint(bytes, &mut pos).map(|_| pos),
            1 => pos.checked_add(8),
            5 => pos.checked_add(4),
            2 => read_varint(bytes, &mut pos)
                .and_then(|l| usize::try_from(l).ok())
                .and_then(|len| pos.checked_add(len)),
            _ => return false,
        };
        match end {
            Some(end) if end <= bytes.len() => pos = end,
            _ => return false,
        }
        count += 1;
    }
    count > 0
}

/// HTTP Toolkit's quick protobuf check: the first byte encodes a field number in 1..=3 with wire
/// type 0, 1, 2 or 5 (first byte in 0x08..=0x1D). Cheap first filter for sniffing.
pub(crate) fn quick_check(bytes: &[u8]) -> bool {
    if bytes.len() < 2 {
        return false;
    }
    let field = bytes[0] >> 3;
    let wire = bytes[0] & 7;
    (1..=3).contains(&field) && matches!(wire, 0 | 1 | 2 | 5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- tiny encoder for tests ---
    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
    }
    fn tag(n: u64, wt: u64, out: &mut Vec<u8>) {
        varint(n << 3 | wt, out);
    }
    fn ld(n: u64, payload: &[u8], out: &mut Vec<u8>) {
        tag(n, 2, out);
        varint(payload.len() as u64, out);
        out.extend_from_slice(payload);
    }

    #[test]
    fn decodes_scalars() {
        let mut m = Vec::new();
        tag(1, 0, &mut m);
        varint(150, &mut m);
        tag(2, 1, &mut m);
        m.extend_from_slice(&1.5f64.to_le_bytes());
        tag(3, 5, &mut m);
        m.extend_from_slice(&0.25f32.to_le_bytes());
        tag(4, 0, &mut m);
        varint(u64::MAX, &mut m); // -1 as int64
        tag(5, 0, &mut m);
        varint(3, &mut m); // sint: -2
        let fields = decode_raw(&m).unwrap();
        assert_eq!(
            fields[0],
            Field {
                number: 1,
                wire_type: 0,
                value: Value::Varint {
                    value: 150,
                    signed: 75
                }
            }
        );
        assert_eq!(
            fields[1].value,
            Value::Fixed64 {
                value: 1.5f64.to_bits(),
                double: 1.5
            }
        );
        assert_eq!(
            fields[2].value,
            Value::Fixed32 {
                value: 0.25f32.to_bits(),
                float: 0.25
            }
        );
        assert_eq!(
            fields[4].value,
            Value::Varint {
                value: 3,
                signed: -2
            }
        );
        assert_eq!(
            to_json(&fields),
            json!({"1": 150, "2": 1.5, "3": 0.25, "4": -1, "5": 3})
        );
    }

    #[test]
    fn nested_strings_bytes_repeated() {
        let mut inner = Vec::new();
        tag(1, 0, &mut inner);
        varint(42, &mut inner);
        ld(2, b"nested text", &mut inner);
        let mut m = Vec::new();
        ld(1, b"hello", &mut m);
        ld(2, &inner, &mut m);
        ld(3, &[0xff, 0x00, 0xfe], &mut m);
        ld(1, b"world", &mut m);
        ld(4, b"", &mut m);
        let fields = decode_raw(&m).unwrap();
        assert!(matches!(fields[1].value, Value::Message { .. }));
        assert_eq!(
            fields[2].value,
            Value::Bytes {
                base64: "/wD+".into()
            }
        );
        assert_eq!(
            to_json(&fields),
            json!({"1": ["hello", "world"], "2": {"1": 42, "2": "nested text"}, "3": "/wD+", "4": ""})
        );
        // serde shape of Value
        assert_eq!(
            serde_json::to_value(&fields[0]).unwrap(),
            json!({"number": 1, "wireType": 2, "value": {"type": "string", "value": "hello"}})
        );
    }

    #[test]
    fn printable_text_beats_accidental_message() {
        // "hi" = 0x68 0x69 parses as {13: 105} but is text.
        assert!(decode_raw(b"hi").is_ok());
        let mut m = Vec::new();
        ld(1, b"hi", &mut m);
        assert_eq!(
            decode_raw(&m).unwrap()[0].value,
            Value::String { value: "hi".into() }
        );
    }

    #[test]
    fn packed_varints_are_bytes() {
        // packed [1, 2, 3, 300]: first byte 0x01 = field 0 → not a message, not text.
        let mut packed = Vec::new();
        for v in [1u64, 2, 3, 300] {
            varint(v, &mut packed);
        }
        let mut m = Vec::new();
        ld(4, &packed, &mut m);
        let f = decode_raw(&m).unwrap();
        assert_eq!(
            f[0].value,
            Value::Bytes {
                base64: crate::util::b64(&packed)
            }
        );
        // packed [8, 150] → first byte 0x08 parses as field 1 varint 150? 0x08, 0x96 0x01 → yes a
        // message {1: 150}; that's the inherent ambiguity: we report the message reading.
        let mut m2 = Vec::new();
        ld(5, &[0x08, 0x96, 0x01], &mut m2);
        assert!(matches!(
            decode_raw(&m2).unwrap()[0].value,
            Value::Message { .. }
        ));
    }

    #[test]
    fn invalid_inputs() {
        assert!(decode_raw(&[0x0b]).is_err()); // group start (field 1, wt 3)
        assert!(decode_raw(&[0x0c]).is_err()); // group end
        assert!(decode_raw(&[0x0e, 0x00]).is_err()); // wt 6
        assert!(decode_raw(&[0x00, 0x00]).is_err()); // field 0
        assert!(decode_raw(&[0x08]).is_err()); // truncated varint
        assert!(decode_raw(&[0x0a, 0x05, b'a']).is_err()); // length past end
        assert!(decode_raw(&[0x09, 1, 2, 3]).is_err()); // truncated fixed64
        assert!(
            decode_raw(&[
                0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f
            ])
            .is_err()
        ); // 10th byte > 1
        assert!(decode_raw(b"{\"json\": true}").is_err()); // '{' = field 15 wt 3
        assert!(decode_raw(b"[1,2]").is_err());
        assert_eq!(decode_raw(b"").unwrap(), vec![]);
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let mut m = Vec::new();
        tag(1, 0, &mut m);
        varint(1, &mut m);
        for _ in 0..200 {
            let mut outer = Vec::new();
            ld(1, &m, &mut outer);
            m = outer;
        }
        let fields = decode_raw(&m).unwrap();
        let mut depth = 0;
        let mut cur = &fields;
        loop {
            match &cur[0].value {
                Value::Message { fields } => {
                    depth += 1;
                    cur = fields;
                }
                Value::Bytes { .. } => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(depth, MAX_DEPTH - 1);
        let _ = to_json(&fields);
    }

    #[test]
    fn float_rendering_heuristics() {
        let mut m = Vec::new();
        tag(1, 1, &mut m);
        m.extend_from_slice(&1_700_000_000_000u64.to_le_bytes()); // ms timestamp → integer
        tag(2, 5, &mut m);
        m.extend_from_slice(&7u32.to_le_bytes()); // small fixed32 → integer
        tag(3, 5, &mut m);
        m.extend_from_slice(&0.1f32.to_le_bytes());
        tag(4, 1, &mut m);
        m.extend_from_slice(&0u64.to_le_bytes());
        assert_eq!(
            to_json(&decode_raw(&m).unwrap()),
            json!({"1": 1_700_000_000_000u64, "2": 7, "3": 0.1, "4": 0})
        );
    }

    #[test]
    fn wire_validation() {
        let all = usize::MAX;
        let mut m = Vec::new();
        ld(1, b"hello", &mut m);
        tag(2, 0, &mut m);
        varint(1, &mut m);
        assert!(wire_valid(&m, all));
        assert!(!wire_valid(&m[..m.len() - 1], all));
        // a cut-off field is never accepted, even with a small scan limit
        assert!(!wire_valid(&m[..8], 0));
        assert!(!wire_valid(&m[..4], 0));
        assert!(!wire_valid(b"", all));
        assert!(!wire_valid(b"\x0b", all));
        // Early accept after the scan limit needs several valid fields...
        let mut many = Vec::new();
        for i in 0..10u64 {
            tag(1 + i % 3, 0, &mut many);
            varint(i, &mut many);
        }
        many.extend_from_slice(&[0xff; 5]); // garbage after the scanned region
        assert!(wire_valid(&many, 4));
        assert!(!wire_valid(&many, all));
        // ...and every length must fit the real body: one oversized length field (the classic
        // false positive on random binary data) is rejected however far the scan reaches.
        let mut bogus = Vec::new();
        tag(1, 0, &mut bogus);
        varint(5, &mut bogus);
        tag(2, 2, &mut bogus);
        varint(1 << 30, &mut bogus);
        bogus.extend_from_slice(&[0x55; 64]);
        assert!(!wire_valid(&bogus, 0));
        assert!(!wire_valid(&bogus, all));
        // A big body made of one huge length-delimited field is fine (skipped in O(1)).
        let mut huge = Vec::new();
        ld(1, &vec![0u8; 3 << 20], &mut huge);
        assert!(wire_valid(&huge, 1 << 20));
        assert!(quick_check(&m));
        assert!(!quick_check(b"{}"));
        assert!(!quick_check(b"\x08"));
    }
}
