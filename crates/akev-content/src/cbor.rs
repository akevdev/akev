//! CBOR (RFC 8949) → JSON.
//!
//! Mapping: byte strings → base64; tags → `{"$tag": n, "value": ...}`; `undefined` → `null`;
//! map keys that aren't text are rendered as their JSON text; integers outside the 64-bit range
//! (bignum-sized values from 65-bit major types) become decimal strings; non-finite floats
//! become `"NaN"` / `"Infinity"` / `"-Infinity"`. A CBOR sequence (RFC 8742: several
//! concatenated items) decodes to a JSON array of the items. Bignums (tags 2/3 over at most 16
//! bytes) become plain integers when they fit 64 bits and keep their tag (with the bytes
//! minus leading zeros) otherwise — the same results `ciborium` gives.
//!
//! The decoder walks the input iteratively (explicit stack) straight into JSON, never building
//! an intermediate CBOR tree, with a nesting limit of [`MAX_DEPTH`] containers/tags. The view
//! path decodes under an output [`Budget`]: when it runs out, the containers decoded so far are
//! closed and returned.

use serde_json::{Map, Value};

use crate::budget::{Budget, base64_len};
use crate::{ContentError, Result};

/// Maximum nesting of arrays, maps and tags.
pub const MAX_DEPTH: usize = 256;

/// Largest up-front allocation for an array's items (bigger arrays grow as items arrive).
const MAX_PREALLOC: usize = 1024;

const TAG_BIGPOS: u64 = 2;
const TAG_BIGNEG: u64 = 3;

/// Decode CBOR into JSON.
pub fn to_json(bytes: &[u8]) -> Result<Value> {
    Ok(to_json_budget(bytes, &mut Budget::unlimited())?.unwrap_or(Value::Null))
}

/// [`to_json`] under an output budget. When the budget runs out the values decoded so far are
/// returned (open containers closed, an incomplete map entry dropped); `None` when nothing
/// could be decoded within it.
pub(crate) fn to_json_budget(bytes: &[u8], budget: &mut Budget) -> Result<Option<Value>> {
    if bytes.is_empty() {
        return Err(err("empty input".into()));
    }
    let mut r = Reader { buf: bytes, pos: 0 };
    let mut values = Vec::new();
    while r.pos < bytes.len() && !budget.exhausted() {
        match read_value(&mut r, budget)? {
            Some(v) => values.push(v),
            None => break,
        }
    }
    Ok(match values.len() {
        0 => None,
        1 => values.pop(),
        _ => Some(Value::Array(values)),
    })
}

fn err(message: String) -> ContentError {
    ContentError::Parse {
        what: "CBOR",
        message,
    }
}

fn err_at(at: usize, what: &str) -> ContentError {
    err(format!("at offset {at}: {what}"))
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or_else(|| {
                err_at(
                    self.pos,
                    &format!("unexpected end of input (need {n} bytes)"),
                )
            })?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn be<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }
}

/// An item head (RFC 8949 §3): major type and argument.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Header {
    Positive(u64),
    /// `-1 - n`.
    Negative(u64),
    /// Byte string; `None` = indefinite length.
    Bytes(Option<usize>),
    /// Text string; `None` = indefinite length.
    Text(Option<usize>),
    Array(Option<usize>),
    Map(Option<usize>),
    Tag(u64),
    Simple(u8),
    Float(f64),
    Break,
}

fn read_header(r: &mut Reader<'_>) -> Result<Header> {
    let at = r.pos;
    let initial = r.take(1)?[0];
    let major = initial >> 5;
    let info = initial & 0x1f;
    // The argument: `None` for the indefinite-length marker (31).
    let arg: Option<u64> = match info {
        0..=23 => Some(u64::from(info)),
        24 => Some(u64::from(r.take(1)?[0])),
        25 => Some(u64::from(u16::from_be_bytes(r.be()?))),
        26 => Some(u64::from(u32::from_be_bytes(r.be()?))),
        27 => Some(u64::from_be_bytes(r.be()?)),
        31 => None,
        _ => return Err(err_at(at, "reserved additional information value")),
    };
    let int = |arg: Option<u64>| arg.ok_or_else(|| err_at(at, "indefinite length not allowed"));
    let len = |arg: Option<u64>| {
        arg.map(|n| usize::try_from(n).map_err(|_| err_at(at, "length overflow")))
            .transpose()
    };
    Ok(match major {
        0 => Header::Positive(int(arg)?),
        1 => Header::Negative(int(arg)?),
        2 => Header::Bytes(len(arg)?),
        3 => Header::Text(len(arg)?),
        4 => Header::Array(len(arg)?),
        5 => Header::Map(len(arg)?),
        6 => Header::Tag(int(arg)?),
        _ => match info {
            0..=24 => Header::Simple(arg.map_or(0, |v| v as u8)),
            25 => Header::Float(f16_to_f64(arg.map_or(0, |v| v as u16))),
            26 => Header::Float(f64::from(f32::from_bits(arg.map_or(0, |v| v as u32)))),
            27 => Header::Float(f64::from_bits(arg.unwrap_or(0))),
            _ => Header::Break,
        },
    })
}

/// IEEE 754 half precision → f64 (exact).
fn f16_to_f64(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let mant = f64::from(h & 0x3ff);
    match exp {
        0 => sign * mant * 2f64.powi(-24),
        31 if mant == 0.0 => sign * f64::INFINITY,
        31 => f64::NAN,
        _ => sign * (1.0 + mant / 1024.0) * 2f64.powi(exp - 15),
    }
}

/// The bytes of a byte string (concatenating the chunks of an indefinite-length one), or `None`
/// when the budget can't hold them.
fn read_bytes(
    r: &mut Reader<'_>,
    len: Option<usize>,
    budget: &mut Budget,
) -> Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    let mut ok = true;
    read_chunks(r, len, true, |chunk| {
        ok = ok && budget.charge(0, chunk.len());
        if ok {
            out.extend_from_slice(chunk);
        }
        Ok(())
    })?;
    Ok(ok.then_some(out))
}

/// The text of a text string (each chunk must be valid UTF-8 on its own), or `None` when the
/// budget can't hold it.
fn read_text(
    r: &mut Reader<'_>,
    len: Option<usize>,
    at: usize,
    budget: &mut Budget,
) -> Result<Option<String>> {
    let mut out = String::new();
    let mut ok = true;
    read_chunks(r, len, false, |chunk| {
        let s = std::str::from_utf8(chunk).map_err(|_| err_at(at, "invalid UTF-8 in text"))?;
        ok = ok && budget.charge(0, s.len());
        if ok {
            out.push_str(s);
        }
        Ok(())
    })?;
    Ok(ok.then_some(out))
}

/// Feed the chunks of a (possibly indefinite-length) string to `f`. Chunks of an indefinite
/// string must be strings of the same major type (nested indefinite chunks are tolerated, like
/// `ciborium` does).
fn read_chunks(
    r: &mut Reader<'_>,
    len: Option<usize>,
    bytes: bool,
    mut f: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    if let Some(n) = len {
        return f(r.take(n)?);
    }
    let mut nested = 1usize;
    loop {
        let chunk_at = r.pos;
        let chunk_len = match (read_header(r)?, bytes) {
            (Header::Break, _) => {
                nested -= 1;
                if nested == 0 {
                    return Ok(());
                }
                continue;
            }
            (Header::Bytes(n), true) | (Header::Text(n), false) => n,
            _ => {
                return Err(err_at(
                    chunk_at,
                    "invalid chunk in indefinite-length string",
                ));
            }
        };
        match chunk_len {
            Some(n) => f(r.take(n)?)?,
            None => nested += 1,
        }
    }
}

/// An integer as JSON: a number within the 64-bit ranges, a decimal string otherwise.
fn int_json(n: i128) -> Value {
    if let Ok(x) = i64::try_from(n) {
        Value::from(x)
    } else if let Ok(x) = u64::try_from(n) {
        Value::from(x)
    } else {
        Value::String(n.to_string())
    }
}

fn tagged(tag: u64, value: Value) -> Value {
    let mut m = Map::new();
    m.insert("$tag".into(), Value::from(tag));
    m.insert("value".into(), value);
    Value::Object(m)
}

/// `{"$tag": tag, "value": base64(magnitude without leading zeros)}` for a bignum outside the
/// 64-bit range.
fn bignum_tagged(tag: u64, magnitude: u128) -> Value {
    let bytes = magnitude.to_be_bytes();
    let first = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
    tagged(tag, Value::String(crate::util::b64(&bytes[first..])))
}

/// Tag 2/3 over a byte string of at most 16 bytes (a 128-bit bignum).
fn bignum(tag: u64, data: &[u8], at: usize) -> Result<Value> {
    let mut magnitude = 0u128;
    for &b in data {
        magnitude = (magnitude << 8) | u128::from(b);
    }
    if tag == TAG_BIGPOS {
        return Ok(match u64::try_from(magnitude) {
            Ok(v) => Value::from(v),
            Err(_) => bignum_tagged(TAG_BIGPOS, magnitude),
        });
    }
    // -1 - magnitude
    let Ok(m) = i128::try_from(magnitude) else {
        return Err(err_at(at, "integer too large"));
    };
    Ok(if magnitude <= u128::from(u64::MAX) {
        int_json(-1 - m)
    } else {
        bignum_tagged(TAG_BIGNEG, magnitude)
    })
}

enum Frame {
    /// `remaining`: items left for a definite-length array, `None` until a break.
    Array {
        items: Vec<Value>,
        remaining: Option<usize>,
    },
    /// `remaining`: entries left for a definite-length map, `None` until a break.
    Map {
        map: Map<String, Value>,
        remaining: Option<usize>,
        key: Option<String>,
    },
    Tag(u64),
}

fn key_string(v: Value) -> String {
    crate::util::key_text(v)
}

/// Close every open container (innermost first) around `value`, the last completed item, and
/// return the outermost one. A map entry still waiting for its value and a tag without its
/// value are dropped.
fn close_partial(mut stack: Vec<Frame>, mut value: Option<Value>) -> Option<Value> {
    while let Some(frame) = stack.pop() {
        value = match frame {
            Frame::Array { mut items, .. } => {
                items.extend(value.take());
                Some(Value::Array(items))
            }
            Frame::Map { mut map, key, .. } => {
                if let (Some(k), Some(v)) = (key, value.take()) {
                    map.insert(k, v);
                }
                Some(Value::Object(map))
            }
            Frame::Tag(tag) => value.take().map(|v| tagged(tag, v)),
        };
    }
    value
}

/// What one header turned into.
enum Step {
    /// A complete value.
    Value(Value),
    /// A container or tag was opened.
    Opened,
    /// The output budget ran out.
    OverBudget,
}

/// Read one complete data item. `None` when the budget ran out before anything was decoded; a
/// partial value when it ran out inside a container.
fn read_value(r: &mut Reader<'_>, budget: &mut Budget) -> Result<Option<Value>> {
    let mut stack: Vec<Frame> = Vec::new();
    loop {
        let at = r.pos;
        let header = read_header(r)?;
        let mut value = if header == Header::Break {
            // Ends the innermost indefinite-length container (where a key may start).
            match stack.pop() {
                Some(Frame::Array {
                    items,
                    remaining: None,
                }) => Value::Array(items),
                Some(Frame::Map {
                    map,
                    remaining: None,
                    key: None,
                }) => Value::Object(map),
                _ => return Err(err_at(at, "unexpected break")),
            }
        } else {
            match step(r, header, at, &mut stack, budget)? {
                Step::Value(v) => v,
                Step::Opened => continue,
                Step::OverBudget => return Ok(close_partial(stack, None)),
            }
        };
        // Attach the completed value to its parent(s).
        loop {
            match stack.last_mut() {
                None => return Ok(Some(value)),
                Some(Frame::Tag(tag)) => {
                    let tag = *tag;
                    stack.pop();
                    value = tagged(tag, value);
                }
                Some(Frame::Array { items, remaining }) => {
                    items.push(value);
                    match remaining {
                        Some(n) => {
                            *n -= 1;
                            if *n > 0 {
                                break;
                            }
                        }
                        None => break,
                    }
                    match stack.pop() {
                        Some(Frame::Array { items, .. }) => value = Value::Array(items),
                        _ => return Err(err("internal stack mismatch".into())),
                    }
                }
                Some(Frame::Map {
                    map,
                    remaining,
                    key,
                }) => match key.take() {
                    None => {
                        *key = Some(key_string(value));
                        break;
                    }
                    Some(k) => {
                        map.insert(k, value);
                        match remaining {
                            Some(n) => {
                                *n -= 1;
                                if *n > 0 {
                                    break;
                                }
                            }
                            None => break,
                        }
                        match stack.pop() {
                            Some(Frame::Map { map, .. }) => value = Value::Object(map),
                            _ => return Err(err("internal stack mismatch".into())),
                        }
                    }
                },
            }
        }
    }
}

/// Turn one (non-break) header into a value, or open a container/tag frame.
fn step(
    r: &mut Reader<'_>,
    header: Header,
    at: usize,
    stack: &mut Vec<Frame>,
    budget: &mut Budget,
) -> Result<Step> {
    let too_deep = || err_at(at, &format!("nesting deeper than {MAX_DEPTH}"));
    if matches!(header, Header::Array(_) | Header::Map(_)) && stack.len() >= MAX_DEPTH {
        return Err(too_deep());
    }
    // Strings charge their bytes as they are read; everything is (at least) one value.
    if !budget.charge(1, 0) {
        return Ok(Step::OverBudget);
    }
    let value = match header {
        Header::Positive(v) => Value::from(v),
        Header::Negative(v) => int_json(-1 - i128::from(v)),
        Header::Float(f) => crate::util::json_f64(f),
        Header::Simple(20) => Value::Bool(false),
        Header::Simple(21) => Value::Bool(true),
        Header::Simple(22 | 23) => Value::Null,
        Header::Simple(_) => return Err(err_at(at, "unknown simple value")),
        Header::Bytes(len) => match read_bytes(r, len, budget)? {
            Some(b) => {
                if !budget.charge(0, base64_len(b.len()).saturating_sub(b.len())) {
                    return Ok(Step::OverBudget);
                }
                Value::String(crate::util::b64(&b))
            }
            None => return Ok(Step::OverBudget),
        },
        Header::Text(len) => match read_text(r, len, at, budget)? {
            Some(s) => Value::String(s),
            None => return Ok(Step::OverBudget),
        },
        Header::Array(Some(0)) => Value::Array(Vec::new()),
        Header::Map(Some(0)) => Value::Object(Map::new()),
        Header::Array(remaining) => {
            let capacity = remaining
                .unwrap_or(0)
                .min(r.remaining())
                .min(budget.remaining_nodes())
                .min(MAX_PREALLOC);
            stack.push(Frame::Array {
                items: Vec::with_capacity(capacity),
                remaining,
            });
            return Ok(Step::Opened);
        }
        Header::Map(remaining) => {
            stack.push(Frame::Map {
                map: Map::new(),
                remaining,
                key: None,
            });
            return Ok(Step::Opened);
        }
        Header::Tag(tag) => {
            // `{"$tag", "value"}`: one more value than the tag itself.
            if !budget.charge(1, 0) {
                return Ok(Step::OverBudget);
            }
            if tag == TAG_BIGPOS || tag == TAG_BIGNEG {
                let save = r.pos;
                if let Header::Bytes(Some(n)) = read_header(r)?
                    && n <= 16
                {
                    let data = r.take(n)?;
                    if !budget.charge(1, 32) {
                        return Ok(Step::OverBudget);
                    }
                    return Ok(Step::Value(bignum(tag, data, at)?));
                }
                r.pos = save;
            }
            if stack.len() >= MAX_DEPTH {
                return Err(too_deep());
            }
            stack.push(Frame::Tag(tag));
            return Ok(Step::Opened);
        }
        Header::Break => return Err(err_at(at, "unexpected break")),
    };
    Ok(Step::Value(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_keyed_by_maps_decode_quickly() {
        let mut data = vec![0xa1u8; 64];
        data.extend(std::iter::repeat_n(0x00u8, 65));
        let started = std::time::Instant::now();
        let v = to_json(&data).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        let key = v.as_object().unwrap().keys().next().unwrap();
        assert!(
            key.chars().count() <= crate::util::MAX_KEY_CHARS + 1,
            "{key}"
        );
    }
    use serde_json::json;

    #[test]
    fn rfc8949_examples() {
        assert_eq!(to_json(&[0x00]).unwrap(), json!(0));
        assert_eq!(to_json(&[0x38, 0x63]).unwrap(), json!(-100));
        assert_eq!(
            to_json(&[0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).unwrap(),
            json!(u64::MAX)
        );
        assert_eq!(
            to_json(&[0x3b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).unwrap(),
            json!("-18446744073709551616")
        );
        assert_eq!(to_json(&[0xf9, 0x3e, 0x00]).unwrap(), json!(1.5)); // half float
        assert_eq!(to_json(&[0xf9, 0x7c, 0x00]).unwrap(), json!("Infinity"));
        assert_eq!(to_json(&[0xf4]).unwrap(), json!(false));
        assert_eq!(to_json(&[0xf6]).unwrap(), json!(null));
        assert_eq!(to_json(&[0xf7]).unwrap(), json!(null)); // undefined
        assert_eq!(to_json(&[0x44, 1, 2, 3, 4]).unwrap(), json!("AQIDBA=="));
        assert_eq!(to_json(&[0x62, 0xc3, 0xbc]).unwrap(), json!("ü"));
        assert_eq!(
            to_json(&[0x83, 0x01, 0x82, 0x02, 0x03, 0x82, 0x04, 0x05]).unwrap(),
            json!([1, [2, 3], [4, 5]])
        );
        // {"a": 1, "b": [2, 3]}
        assert_eq!(
            to_json(&[0xa2, 0x61, 0x61, 0x01, 0x61, 0x62, 0x82, 0x02, 0x03]).unwrap(),
            json!({"a": 1, "b": [2, 3]})
        );
        // {1: 2, 3: 4}
        assert_eq!(
            to_json(&[0xa2, 0x01, 0x02, 0x03, 0x04]).unwrap(),
            json!({"1": 2, "3": 4})
        );
        // indefinite-length array [_ 1, 2]
        assert_eq!(to_json(&[0x9f, 0x01, 0x02, 0xff]).unwrap(), json!([1, 2]));
        // tag 1 (epoch time)
        assert_eq!(
            to_json(&[0xc1, 0x1a, 0x51, 0x4b, 0x67, 0xb0]).unwrap(),
            json!({"$tag": 1, "value": 1363896240})
        );
        // tag 0 (date string)
        let mut t0 = vec![0xc0, 0x74];
        t0.extend_from_slice(b"2013-03-21T20:04:00Z");
        assert_eq!(
            to_json(&t0).unwrap(),
            json!({"$tag": 0, "value": "2013-03-21T20:04:00Z"})
        );
        // indefinite-length strings: (_ h'0102', h'03') and (_ "strea", "ming")
        assert_eq!(
            to_json(&[0x5f, 0x42, 1, 2, 0x41, 3, 0xff]).unwrap(),
            json!("AQID")
        );
        assert_eq!(
            to_json(b"\x7f\x65strea\x64ming\xff").unwrap(),
            json!("streaming")
        );
        // bignums: 2^64 (tag 2), -2^64 - 1 (tag 3), small values become integers
        assert_eq!(
            to_json(&[0xc2, 0x49, 1, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap(),
            json!({"$tag": 2, "value": "AQAAAAAAAAAA"})
        );
        assert_eq!(
            to_json(&[0xc3, 0x49, 1, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap(),
            json!({"$tag": 3, "value": "AQAAAAAAAAAA"})
        );
        assert_eq!(to_json(&[0xc2, 0x42, 0, 7]).unwrap(), json!(7));
        assert_eq!(to_json(&[0xc3, 0x41, 7]).unwrap(), json!(-8));
    }

    #[test]
    fn sequences_and_errors() {
        assert_eq!(to_json(&[0x01, 0x61, 0x61]).unwrap(), json!([1, "a"]));
        assert!(to_json(&[]).is_err());
        assert!(to_json(&[0x82, 0x01]).is_err());
        assert!(to_json(&[0x5b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(to_json(&[0x9f; 10_000]).is_err()); // nesting limit, no stack overflow
        assert!(to_json(&[0xc1; 10_000]).is_err()); // nested tags too
        assert!(to_json(&[0xff]).is_err()); // break outside a container
        assert!(to_json(&[0x82, 0x01, 0xff]).is_err()); // break in a definite array
        assert!(to_json(&[0xbf, 0x01, 0xff]).is_err()); // break where a map value belongs
        assert!(to_json(&[0x62, 0xff, 0xfe]).is_err()); // invalid UTF-8
        assert!(to_json(&[0xf0]).is_err()); // unknown simple value
        assert!(to_json(&[0x1c]).is_err()); // reserved additional information
        assert!(to_json(&[0x1f]).is_err()); // indefinite integer
        // A huge declared array without data fails without allocating for it.
        assert!(to_json(&[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_err());
    }

    /// The decoder before the budgeted rewrite (ciborium's tree + conversion): the reference
    /// for the differential test below.
    fn reference(bytes: &[u8]) -> Option<Value> {
        fn convert(v: ciborium::Value) -> Value {
            match v {
                ciborium::Value::Integer(i) => int_json(i128::from(i)),
                ciborium::Value::Bytes(b) => Value::String(crate::util::b64(&b)),
                ciborium::Value::Float(f) => crate::util::json_f64(f),
                ciborium::Value::Text(s) => Value::String(s),
                ciborium::Value::Bool(b) => Value::Bool(b),
                ciborium::Value::Null => Value::Null,
                ciborium::Value::Tag(tag, inner) => tagged(tag, convert(*inner)),
                ciborium::Value::Array(items) => {
                    Value::Array(items.into_iter().map(convert).collect())
                }
                ciborium::Value::Map(entries) => {
                    let mut m = Map::new();
                    for (k, v) in entries {
                        m.insert(key_string(convert(k)), convert(v));
                    }
                    Value::Object(m)
                }
                _ => Value::Null,
            }
        }
        if bytes.is_empty() {
            return None;
        }
        let mut rest = bytes;
        let mut values = Vec::new();
        while !rest.is_empty() {
            let item: ciborium::Value = ciborium::de::from_reader(&mut rest).ok()?;
            values.push(convert(item));
        }
        Some(if values.len() == 1 {
            values.pop()?
        } else {
            Value::Array(values)
        })
    }

    /// xorshift64*: deterministic pseudo-random numbers without a dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A random (mostly well-formed) CBOR item.
    fn gen_item(rng: &mut Rng, depth: u32, out: &mut Vec<u8>) {
        let head = |out: &mut Vec<u8>, major: u8, arg: u64, wide: bool| {
            let m = major << 5;
            if arg < 24 && !wide {
                out.push(m | arg as u8);
            } else if arg <= 0xff {
                out.push(m | 24);
                out.push(arg as u8);
            } else if arg <= 0xffff {
                out.push(m | 25);
                out.extend_from_slice(&(arg as u16).to_be_bytes());
            } else if arg <= 0xffff_ffff {
                out.push(m | 26);
                out.extend_from_slice(&(arg as u32).to_be_bytes());
            } else {
                out.push(m | 27);
                out.extend_from_slice(&arg.to_be_bytes());
            }
        };
        let interesting = [
            0u64,
            1,
            23,
            24,
            255,
            256,
            65535,
            65536,
            u32::MAX as u64,
            u64::MAX,
        ];
        let n = |rng: &mut Rng| {
            if rng.below(2) == 0 {
                interesting[rng.below(interesting.len() as u64) as usize]
            } else {
                rng.next() >> rng.below(64)
            }
        };
        let choice = if depth > 4 {
            rng.below(8)
        } else {
            rng.below(13)
        };
        match choice {
            0 => head(out, 0, n(rng), rng.below(4) == 0),
            1 => head(out, 1, n(rng), false),
            2 => {
                let len = rng.below(20) as usize;
                head(out, 2, len as u64, false);
                out.extend((0..len).map(|_| rng.next() as u8));
            }
            3 => {
                let words = ["", "a", "ü", "日本", "\u{1F600}", "key"];
                let s = words[rng.below(words.len() as u64) as usize];
                head(out, 3, s.len() as u64, false);
                out.extend_from_slice(s.as_bytes());
            }
            4 => out.push(0xf4 + rng.below(4) as u8),
            5 => {
                out.push(0xf9);
                out.extend_from_slice(&(rng.next() as u16).to_be_bytes());
            }
            6 => {
                out.push(0xfa);
                out.extend_from_slice(&(rng.next() as u32).to_be_bytes());
            }
            7 => {
                out.push(0xfb);
                out.extend_from_slice(&rng.next().to_be_bytes());
            }
            8 | 9 => {
                let len = rng.below(4);
                let indefinite = rng.below(3) == 0;
                let major = if choice == 8 { 4 } else { 5 };
                if indefinite {
                    out.push((major << 5) | 31);
                } else {
                    head(out, major, len, false);
                }
                let items = if major == 5 { len * 2 } else { len };
                for _ in 0..items {
                    gen_item(rng, depth + 1, out);
                }
                if indefinite {
                    out.push(0xff);
                }
            }
            10 => {
                let tag = [0u64, 1, 2, 3, 2, 3, 55799, 1 << 40][rng.below(8) as usize];
                head(out, 6, tag, false);
                if (tag == 2 || tag == 3) && rng.below(2) == 0 {
                    let len = rng.below(20);
                    head(out, 2, len, false);
                    out.extend((0..len).map(|_| rng.next() as u8));
                } else {
                    gen_item(rng, depth + 1, out);
                }
            }
            11 => {
                // indefinite-length byte/text string
                let text = rng.below(2) == 0;
                out.push(if text { 0x7f } else { 0x5f });
                for _ in 0..rng.below(3) {
                    if text {
                        out.extend_from_slice(b"\x62hi");
                    } else {
                        out.extend_from_slice(&[0x42, 1, 2]);
                    }
                }
                out.push(0xff);
            }
            _ => out.push(rng.next() as u8), // anything at all
        }
    }

    #[test]
    fn matches_ciborium_on_random_input() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut compared = 0;
        for i in 0..20_000 {
            let mut bytes = Vec::new();
            for _ in 0..=rng.below(2) {
                gen_item(&mut rng, 0, &mut bytes);
            }
            // Corrupt some inputs: truncation and byte flips.
            if i % 5 == 0 && !bytes.is_empty() {
                let at = rng.below(bytes.len() as u64) as usize;
                bytes[at] = rng.next() as u8;
            }
            if i % 7 == 0 {
                let keep = rng.below(bytes.len() as u64 + 1) as usize;
                bytes.truncate(keep);
            }
            let ours = to_json(&bytes).ok();
            let theirs = reference(&bytes);
            assert_eq!(ours, theirs, "input {bytes:02x?}");
            compared += usize::from(theirs.is_some());
        }
        assert!(compared > 5_000, "too few valid inputs: {compared}");
    }

    #[test]
    fn budget_keeps_a_prefix() {
        // An array of 10 000 nulls under a 100-value budget.
        let mut big = vec![0x99, 0x27, 0x10];
        big.extend(std::iter::repeat_n(0xf6, 10_000));
        let mut budget = Budget::new(100, usize::MAX);
        let v = to_json_budget(&big, &mut budget).unwrap().unwrap();
        assert!(budget.exhausted());
        assert_eq!(v.as_array().unwrap().len(), 99); // the array itself is one value
        // Nested: the open containers are closed around what was decoded.
        let nested = [0x81, 0xa2, 0x61, b'a', 0x01, 0x61, b'b', 0x82, 0x02, 0x03];
        let mut budget = Budget::new(5, usize::MAX);
        let v = to_json_budget(&nested, &mut budget).unwrap().unwrap();
        assert_eq!(v, json!([{"a": 1}]));
        // Text counts against the text budget.
        let mut budget = Budget::new(100, 3);
        assert_eq!(
            to_json_budget(
                &[0x82, 0x61, b'x', 0x64, b'l', b'o', b'n', b'g'],
                &mut budget
            )
            .unwrap(),
            Some(json!(["x"]))
        );
        let mut budget = Budget::new(0, 0);
        assert_eq!(to_json_budget(&[0x01], &mut budget).unwrap(), None);
    }
}
