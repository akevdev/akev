//! MessagePack → JSON.
//!
//! Mapping: binary → base64 string; extension types → `{"$ext": type, "data": base64}` (the
//! timestamp extension, type -1, additionally gets `"timestamp": "<RFC 3339>"`); map keys that
//! aren't strings are rendered as their JSON text (`1`, `true`, ...); non-finite floats become
//! the strings `"NaN"` / `"Infinity"` / `"-Infinity"`; invalid UTF-8 in strings is replaced.
//! A body holding several concatenated values (a MessagePack stream) decodes to a JSON array of
//! those values.
//!
//! The decoder is iterative (explicit stack) with a nesting limit of [`MAX_DEPTH`], and never
//! pre-allocates more than the remaining input can justify. The view path decodes under an
//! output [`Budget`]: when it runs out, the containers decoded so far are closed and returned.

use serde_json::{Map, Value};

use crate::budget::{Budget, base64_len};
use crate::{ContentError, Result};

/// Maximum container nesting.
pub const MAX_DEPTH: usize = 512;

/// Largest up-front allocation for an array's items (bigger arrays grow as items arrive).
const MAX_PREALLOC: usize = 1024;

/// Decode MessagePack into JSON.
pub fn to_json(bytes: &[u8]) -> Result<Value> {
    Ok(to_json_budget(bytes, &mut Budget::unlimited())?.unwrap_or(Value::Null))
}

/// [`to_json`] under an output budget. When the budget runs out the values decoded so far are
/// returned (open containers closed, an incomplete map entry dropped); `None` when nothing
/// could be decoded within it.
pub(crate) fn to_json_budget(bytes: &[u8], budget: &mut Budget) -> Result<Option<Value>> {
    let mut r = Reader { buf: bytes, pos: 0 };
    if bytes.is_empty() {
        return Err(err("empty input".into()));
    }
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
        what: "MessagePack",
        message,
    }
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
                err(format!(
                    "unexpected end of input at offset {} (need {n} bytes)",
                    self.pos
                ))
            })?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn be<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }
    fn len16(&mut self) -> Result<usize> {
        Ok(u16::from_be_bytes(self.be()?) as usize)
    }
    fn len32(&mut self) -> Result<usize> {
        Ok(u32::from_be_bytes(self.be()?) as usize)
    }
}

enum Item {
    Scalar(Value),
    /// A scalar already charged to the budget (strings, binaries, extensions).
    Charged(Value),
    Array(usize),
    Map(usize),
    /// The output budget ran out before this item could be decoded.
    OverBudget,
}

enum Frame {
    Array {
        items: Vec<Value>,
        remaining: usize,
    },
    Map {
        map: Map<String, Value>,
        remaining: usize,
        key: Option<String>,
    },
}

fn key_string(v: Value) -> String {
    crate::util::key_text(v)
}

/// Close every open container (innermost first) around `value`, the last completed item, and
/// return the outermost one. A map entry still waiting for its value is dropped.
fn close_partial(mut stack: Vec<Frame>, mut value: Option<Value>) -> Option<Value> {
    while let Some(frame) = stack.pop() {
        value = Some(match frame {
            Frame::Array { mut items, .. } => {
                items.extend(value.take());
                Value::Array(items)
            }
            Frame::Map { mut map, key, .. } => {
                if let (Some(k), Some(v)) = (key, value.take()) {
                    map.insert(k, v);
                }
                Value::Object(map)
            }
        });
    }
    value
}

/// Read one complete value. `None` when the budget ran out before anything was decoded; a
/// partial value when it ran out inside a container.
fn read_value(r: &mut Reader<'_>, budget: &mut Budget) -> Result<Option<Value>> {
    let mut stack: Vec<Frame> = Vec::new();
    loop {
        let mut value = match read_item(r, budget)? {
            Item::OverBudget => return Ok(close_partial(stack, None)),
            Item::Scalar(v) | Item::Charged(v) => v,
            Item::Array(n) | Item::Map(n) if n > 0 && stack.len() >= MAX_DEPTH => {
                return Err(err(format!("nesting deeper than {MAX_DEPTH}")));
            }
            Item::Array(0) => Value::Array(Vec::new()),
            Item::Map(0) => Value::Object(Map::new()),
            Item::Array(n) => {
                let capacity = n
                    .min(r.remaining())
                    .min(budget.remaining_nodes())
                    .min(MAX_PREALLOC);
                stack.push(Frame::Array {
                    items: Vec::with_capacity(capacity),
                    remaining: n,
                });
                continue;
            }
            Item::Map(n) => {
                stack.push(Frame::Map {
                    map: Map::new(),
                    remaining: n,
                    key: None,
                });
                continue;
            }
        };
        // Attach the completed value to its parent(s).
        loop {
            match stack.last_mut() {
                None => return Ok(Some(value)),
                Some(Frame::Array { items, remaining }) => {
                    items.push(value);
                    *remaining -= 1;
                    if *remaining > 0 {
                        break;
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
                        *remaining -= 1;
                        if *remaining > 0 {
                            break;
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

fn read_str(r: &mut Reader<'_>, n: usize, budget: &mut Budget) -> Result<Item> {
    let s = r.take(n)?;
    if !budget.charge(1, n) {
        return Ok(Item::OverBudget);
    }
    Ok(Item::Charged(Value::String(
        String::from_utf8_lossy(s).into_owned(),
    )))
}

fn read_bin(r: &mut Reader<'_>, n: usize, budget: &mut Budget) -> Result<Item> {
    let data = r.take(n)?;
    if !budget.charge(1, base64_len(n)) {
        return Ok(Item::OverBudget);
    }
    Ok(Item::Charged(Value::String(crate::util::b64(data))))
}

fn read_ext(r: &mut Reader<'_>, n: usize, budget: &mut Budget) -> Result<Item> {
    let ty = r.u8()? as i8;
    let data = r.take(n)?;
    // {"$ext", "data", "timestamp"?}: the object, its two members and maybe a timestamp.
    if !budget.charge(4, base64_len(n) + 32) {
        return Ok(Item::OverBudget);
    }
    let mut obj = Map::new();
    obj.insert("$ext".into(), Value::from(ty));
    obj.insert("data".into(), Value::String(crate::util::b64(data)));
    if ty == -1
        && let Some(ts) = timestamp(data)
    {
        obj.insert("timestamp".into(), Value::String(ts));
    }
    Ok(Item::Charged(Value::Object(obj)))
}

/// MessagePack timestamp extension (type -1): 32, 64 or 96-bit forms.
fn timestamp(data: &[u8]) -> Option<String> {
    let (secs, nanos) = match data.len() {
        4 => (i64::from(u32::from_be_bytes(data.try_into().ok()?)), 0),
        8 => {
            let v = u64::from_be_bytes(data.try_into().ok()?);
            ((v & 0x3_ffff_ffff) as i64, (v >> 34) as u32)
        }
        12 => {
            let nanos = u32::from_be_bytes(data[..4].try_into().ok()?);
            (i64::from_be_bytes(data[4..].try_into().ok()?), nanos)
        }
        _ => return None,
    };
    crate::util::format_rfc3339(secs, nanos)
}

fn read_item(r: &mut Reader<'_>, budget: &mut Budget) -> Result<Item> {
    let item = read_header_item(r, budget)?;
    // Strings, binaries and extensions charge themselves (before allocating); every other item
    // is one value.
    Ok(match item {
        Item::Charged(_) | Item::OverBudget => item,
        _ if budget.charge(1, 0) => item,
        _ => Item::OverBudget,
    })
}

fn read_header_item(r: &mut Reader<'_>, budget: &mut Budget) -> Result<Item> {
    let at = r.pos;
    let b = r.u8()?;
    Ok(match b {
        0x00..=0x7f => Item::Scalar(Value::from(b)),
        0x80..=0x8f => Item::Map((b & 0x0f) as usize),
        0x90..=0x9f => Item::Array((b & 0x0f) as usize),
        0xa0..=0xbf => return read_str(r, (b & 0x1f) as usize, budget),
        0xc0 => Item::Scalar(Value::Null),
        0xc1 => return Err(err(format!("reserved byte 0xc1 at offset {at}"))),
        0xc2 => Item::Scalar(Value::Bool(false)),
        0xc3 => Item::Scalar(Value::Bool(true)),
        0xc4 => {
            let n = r.u8()? as usize;
            return read_bin(r, n, budget);
        }
        0xc5 => {
            let n = r.len16()?;
            return read_bin(r, n, budget);
        }
        0xc6 => {
            let n = r.len32()?;
            return read_bin(r, n, budget);
        }
        0xc7 => {
            let n = r.u8()? as usize;
            return read_ext(r, n, budget);
        }
        0xc8 => {
            let n = r.len16()?;
            return read_ext(r, n, budget);
        }
        0xc9 => {
            let n = r.len32()?;
            return read_ext(r, n, budget);
        }
        0xca => Item::Scalar(crate::util::json_f64(f64::from(f32::from_be_bytes(
            r.be()?,
        )))),
        0xcb => Item::Scalar(crate::util::json_f64(f64::from_be_bytes(r.be()?))),
        0xcc => Item::Scalar(Value::from(r.u8()?)),
        0xcd => Item::Scalar(Value::from(u16::from_be_bytes(r.be()?))),
        0xce => Item::Scalar(Value::from(u32::from_be_bytes(r.be()?))),
        0xcf => Item::Scalar(Value::from(u64::from_be_bytes(r.be()?))),
        0xd0 => Item::Scalar(Value::from(r.u8()? as i8)),
        0xd1 => Item::Scalar(Value::from(i16::from_be_bytes(r.be()?))),
        0xd2 => Item::Scalar(Value::from(i32::from_be_bytes(r.be()?))),
        0xd3 => Item::Scalar(Value::from(i64::from_be_bytes(r.be()?))),
        0xd4 => return read_ext(r, 1, budget),
        0xd5 => return read_ext(r, 2, budget),
        0xd6 => return read_ext(r, 4, budget),
        0xd7 => return read_ext(r, 8, budget),
        0xd8 => return read_ext(r, 16, budget),
        0xd9 => {
            let n = r.u8()? as usize;
            return read_str(r, n, budget);
        }
        0xda => {
            let n = r.len16()?;
            return read_str(r, n, budget);
        }
        0xdb => {
            let n = r.len32()?;
            return read_str(r, n, budget);
        }
        0xdc => Item::Array(r.len16()?),
        0xdd => Item::Array(r.len32()?),
        0xde => Item::Map(r.len16()?),
        0xdf => Item::Map(r.len32()?),
        0xe0..=0xff => Item::Scalar(Value::from(b as i8)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_keyed_by_maps_decode_quickly() {
        // fixmap(1) keyed by fixmap(1) keyed by ... (found by fuzzing: seconds before the cap).
        let mut data = vec![0x81u8; 64];
        data.extend(std::iter::repeat_n(0x00u8, 65));
        let started = std::time::Instant::now();
        let v = to_json(&data).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        let key = v.as_object().unwrap().keys().next().unwrap();
        assert!(
            key.chars().count() <= crate::util::MAX_KEY_CHARS + 1,
            "{key}"
        );
        // Non-string keys stay readable.
        assert_eq!(
            to_json(&[0x81, 0x01, 0x02]).unwrap(),
            serde_json::json!({"1": 2})
        );
    }
    use serde_json::json;

    #[test]
    fn scalars_and_containers() {
        // {"compact": true, "schema": 0}
        let bytes = b"\x82\xa7compact\xc3\xa6schema\x00";
        assert_eq!(
            to_json(bytes).unwrap(),
            json!({"compact": true, "schema": 0})
        );
        assert_eq!(to_json(&[0xc0]).unwrap(), json!(null));
        assert_eq!(to_json(&[0xff]).unwrap(), json!(-1));
        assert_eq!(to_json(&[0xd0, 0x80]).unwrap(), json!(-128));
        assert_eq!(
            to_json(&[0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).unwrap(),
            json!(u64::MAX)
        );
        assert_eq!(
            to_json(&[0xd3, 0x80, 0, 0, 0, 0, 0, 0, 0]).unwrap(),
            json!(i64::MIN)
        );
        assert_eq!(
            to_json(&[0xcb, 0x3f, 0xf8, 0, 0, 0, 0, 0, 0]).unwrap(),
            json!(1.5)
        );
        assert_eq!(to_json(&[0xca, 0x7f, 0xc0, 0, 0]).unwrap(), json!("NaN"));
        assert_eq!(
            to_json(&[0x93, 0x01, 0x92, 0x02, 0x03, 0x90]).unwrap(),
            json!([1, [2, 3], []])
        );
        assert_eq!(to_json(&[0x80]).unwrap(), json!({}));
        let mut s = vec![0xd9, 40];
        s.extend(std::iter::repeat_n(b'x', 40));
        assert_eq!(to_json(&s).unwrap(), json!("x".repeat(40)));
    }

    #[test]
    fn binary_ext_and_keys() {
        assert_eq!(to_json(&[0xc4, 0x03, 1, 2, 3]).unwrap(), json!("AQID"));
        assert_eq!(
            to_json(&[0xd4, 0x05, 0xaa]).unwrap(),
            json!({"$ext": 5, "data": "qg=="})
        );
        // timestamp 32: 1700000000
        let mut t = vec![0xd6, 0xff];
        t.extend_from_slice(&1_700_000_000u32.to_be_bytes());
        assert_eq!(
            to_json(&t).unwrap(),
            json!({"$ext": -1, "data": "ZVPxAA==", "timestamp": "2023-11-14T22:13:20Z"})
        );
        // timestamp 64 with nanos
        let v: u64 = (500_000_000u64 << 34) | 1;
        let mut t64 = vec![0xd7, 0xff];
        t64.extend_from_slice(&v.to_be_bytes());
        assert_eq!(
            to_json(&t64).unwrap()["timestamp"],
            json!("1970-01-01T00:00:01.500Z")
        );
        // integer and nil keys
        assert_eq!(
            to_json(&[0x82, 0x01, 0xa1, b'a', 0xc0, 0x02]).unwrap(),
            json!({"1": "a", "null": 2})
        );
    }

    #[test]
    fn streams_and_errors() {
        assert_eq!(to_json(&[0x01, 0x02]).unwrap(), json!([1, 2]));
        assert!(to_json(&[]).is_err());
        assert!(to_json(&[0xc1]).is_err());
        assert!(to_json(&[0x92, 0x01]).is_err()); // truncated array
        assert!(to_json(&[0xdb, 0xff, 0xff, 0xff, 0xff]).is_err()); // huge string, no data
        assert!(to_json(&[0xdd, 0xff, 0xff, 0xff, 0xff]).is_err()); // huge array, no data: no OOM
        assert!(to_json(&[0x81, 0x01]).is_err()); // map missing value
    }

    #[test]
    fn deep_nesting_is_rejected_not_overflowed() {
        let deep = vec![0x91u8; 100_000];
        assert!(to_json(&deep).is_err());
        let mut ok = vec![0x91u8; 200];
        ok.push(0x01);
        assert!(to_json(&ok).is_ok());
    }
}
