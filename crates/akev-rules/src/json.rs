//! JSON comparison and patching.
//!
//! * [`json_eq`] — deep equality where numbers compare by value (`1 == 1.0`) and object key
//!   order is irrelevant (array order matters).
//! * [`json_subset`] — the "partial" JSON body semantics: every key of an expected object must be
//!   present in the actual object with a (recursively) matching value; an expected array matches
//!   when the actual array is at least as long and each expected element matches the actual
//!   element at the same index (so `[1, 2]` matches `[1, 2, 3]` but not `[2, 1]`); scalars use
//!   [`json_eq`].
//! * [`merge_patch`] — RFC 7386 JSON Merge Patch.
//! * [`apply_patch`] / [`validate_patch`] — RFC 6902 JSON Patch (atomic: on any error the
//!   document is left untouched).
//!
//! Body transforms patch a crate-internal document type instead of a `serde_json::Value`: it
//! keeps every number exactly as written, so numbers the patch doesn't touch are re-serialized
//! byte for byte.

use serde_json::{Map, Number, Value};
use std::collections::HashMap;

/// Deep JSON equality with numeric comparison by value.
pub fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => num_eq(x, y),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| json_eq(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
        }
        _ => a == b,
    }
}

fn num_eq(x: &Number, y: &Number) -> bool {
    if let (Some(a), Some(b)) = (x.as_i64(), y.as_i64()) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (x.as_u64(), y.as_u64()) {
        return a == b;
    }
    match (x.as_f64(), y.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Partial ("contains") JSON matching — see the module docs for the exact semantics.
pub fn json_subset(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => e
            .iter()
            .all(|(k, ev)| a.get(k).is_some_and(|av| json_subset(ev, av))),
        (Value::Array(e), Value::Array(a)) => {
            e.len() <= a.len() && e.iter().zip(a).all(|(ev, av)| json_subset(ev, av))
        }
        _ => json_eq(expected, actual),
    }
}

/// Apply an RFC 7386 merge patch to `target` in place (`null` members delete keys; arrays and
/// scalars replace the target value wholesale).
pub fn merge_patch(target: &mut Value, patch: &Value) {
    merge(target, patch);
}

/// [`merge_patch`] on a number-preserving [`Doc`].
pub(crate) fn merge_patch_doc(target: &mut Doc, patch: &Value) {
    merge(target, patch);
}

fn merge<T: Tree>(target: &mut T, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = T::from_value(patch);
        return;
    };
    if !matches!(target.node(), Node::Object(_)) {
        *target = T::empty_object();
    }
    if let Node::Object(members) = target.node() {
        for (key, value) in patch {
            if value.is_null() {
                members.remove(key);
            } else {
                merge(members.get_or_null(key), value);
            }
        }
    }
}

/// Parse an RFC 6901 JSON pointer into unescaped reference tokens.
pub(crate) fn parse_pointer(pointer: &str) -> Result<Vec<String>, String> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    let Some(rest) = pointer.strip_prefix('/') else {
        return Err(format!(
            "JSON pointer `{pointer}` must be empty or start with `/`"
        ));
    };
    rest.split('/')
        .map(|tok| {
            let mut out = String::with_capacity(tok.len());
            let mut chars = tok.chars();
            while let Some(c) = chars.next() {
                if c == '~' {
                    match chars.next() {
                        Some('0') => out.push('~'),
                        Some('1') => out.push('/'),
                        _ => return Err(format!("invalid `~` escape in JSON pointer `{pointer}`")),
                    }
                } else {
                    out.push(c);
                }
            }
            Ok(out)
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpKind {
    Add,
    Remove,
    Replace,
    Move,
    Copy,
    Test,
}

struct Op<'a> {
    kind: OpKind,
    path: Vec<String>,
    from: Option<Vec<String>>,
    value: Option<&'a Value>,
}

fn parse_op(raw: &Value, index: usize) -> Result<Op<'_>, String> {
    let obj = raw
        .as_object()
        .ok_or_else(|| format!("JSON patch operation #{index} is not an object"))?;
    let op = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("JSON patch operation #{index} has no `op`"))?;
    let kind = match op {
        "add" => OpKind::Add,
        "remove" => OpKind::Remove,
        "replace" => OpKind::Replace,
        "move" => OpKind::Move,
        "copy" => OpKind::Copy,
        "test" => OpKind::Test,
        other => {
            return Err(format!(
                "JSON patch operation #{index}: unknown op `{other}`"
            ));
        }
    };
    let path = obj
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("JSON patch operation #{index} ({op}) has no `path`"))?;
    let path = parse_pointer(path).map_err(|e| format!("JSON patch operation #{index}: {e}"))?;
    let from = if matches!(kind, OpKind::Move | OpKind::Copy) {
        let from = obj
            .get("from")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("JSON patch operation #{index} ({op}) has no `from`"))?;
        Some(parse_pointer(from).map_err(|e| format!("JSON patch operation #{index}: {e}"))?)
    } else {
        None
    };
    let value = if matches!(kind, OpKind::Add | OpKind::Replace | OpKind::Test) {
        Some(
            obj.get("value")
                .ok_or_else(|| format!("JSON patch operation #{index} ({op}) has no `value`"))?,
        )
    } else {
        None
    };
    if kind == OpKind::Move
        && let Some(from) = &from
        && path.len() > from.len()
        && path[..from.len()] == from[..]
    {
        return Err(format!(
            "JSON patch operation #{index}: cannot move a value into one of its children"
        ));
    }
    Ok(Op {
        kind,
        path,
        from,
        value,
    })
}

/// Check that every operation is well-formed (used when compiling rules).
pub fn validate_patch(ops: &[Value]) -> Result<(), String> {
    for (i, op) in ops.iter().enumerate() {
        parse_op(op, i)?;
    }
    Ok(())
}

/// Apply RFC 6902 operations atomically.
pub fn apply_patch(doc: &mut Value, ops: &[Value]) -> Result<(), String> {
    patch(doc, ops)
}

/// [`apply_patch`] on a number-preserving [`Doc`].
pub(crate) fn apply_patch_doc(doc: &mut Doc, ops: &[Value]) -> Result<(), String> {
    patch(doc, ops)
}

fn patch<T: Tree>(doc: &mut T, ops: &[Value]) -> Result<(), String> {
    let parsed = ops
        .iter()
        .enumerate()
        .map(|(i, op)| parse_op(op, i))
        .collect::<Result<Vec<_>, _>>()?;
    let mut work = doc.clone();
    for (i, op) in parsed.iter().enumerate() {
        apply_op(&mut work, op).map_err(|e| format!("JSON patch operation #{i} failed: {e}"))?;
    }
    *doc = work;
    Ok(())
}

fn patch_value<T: Tree>(op: &Op<'_>) -> T {
    op.value
        .map_or_else(|| T::from_value(&Value::Null), T::from_value)
}

fn apply_op<T: Tree>(doc: &mut T, op: &Op<'_>) -> Result<(), String> {
    match op.kind {
        OpKind::Add => add(doc, &op.path, patch_value(op)),
        OpKind::Remove => remove(doc, &op.path).map(drop),
        OpKind::Replace => {
            let target = get_mut(doc, &op.path)?;
            *target = patch_value(op);
            Ok(())
        }
        OpKind::Move => {
            let from = op.from.as_deref().unwrap_or_default();
            if from == op.path.as_slice() {
                // Moving onto itself still requires the location to exist.
                return get_mut(doc, from).map(drop);
            }
            let value = remove(doc, from)?;
            add(doc, &op.path, value)
        }
        OpKind::Copy => {
            let from = op.from.as_deref().unwrap_or_default();
            let value = get_mut(doc, from)?.clone();
            add(doc, &op.path, value)
        }
        OpKind::Test => {
            let actual = get_mut(doc, &op.path)?;
            let expected = op.value.unwrap_or(&Value::Null);
            if actual.equals(expected) {
                Ok(())
            } else {
                Err(format!(
                    "test failed: value at `{}` is {}, expected {expected}",
                    display_path(&op.path),
                    actual.to_json()
                ))
            }
        }
    }
}

fn display_path(path: &[String]) -> String {
    path.iter()
        .map(|t| format!("/{}", t.replace('~', "~0").replace('/', "~1")))
        .collect()
}

fn parse_index(token: &str, len: usize, allow_end: bool) -> Result<usize, String> {
    if allow_end && token == "-" {
        return Ok(len);
    }
    if token.is_empty()
        || (token.len() > 1 && token.starts_with('0'))
        || !token.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(format!("`{token}` is not a valid array index"));
    }
    let idx: usize = token
        .parse()
        .map_err(|_| format!("array index `{token}` is out of range"))?;
    let max = if allow_end {
        len
    } else {
        len.saturating_sub(1)
    };
    if idx > max || (!allow_end && len == 0) {
        return Err(format!("array index {idx} is out of bounds (length {len})"));
    }
    Ok(idx)
}

fn get_mut<'v, T: Tree>(doc: &'v mut T, path: &[String]) -> Result<&'v mut T, String> {
    let mut cur = doc;
    for (depth, token) in path.iter().enumerate() {
        cur = match cur.node() {
            Node::Object(members) => members.get_mut(token).ok_or_else(|| {
                format!("path `{}` does not exist", display_path(&path[..=depth]))
            })?,
            Node::Array(items) => {
                let idx = parse_index(token, items.len(), false)?;
                &mut items[idx]
            }
            Node::Scalar => {
                return Err(format!(
                    "path `{}` does not exist",
                    display_path(&path[..=depth])
                ));
            }
        };
    }
    Ok(cur)
}

fn add<T: Tree>(doc: &mut T, path: &[String], value: T) -> Result<(), String> {
    let Some((last, parent_path)) = path.split_last() else {
        *doc = value;
        return Ok(());
    };
    match get_mut(doc, parent_path)?.node() {
        Node::Object(members) => {
            members.insert(last.clone(), value);
            Ok(())
        }
        Node::Array(items) => {
            let idx = parse_index(last, items.len(), true)?;
            items.insert(idx, value);
            Ok(())
        }
        Node::Scalar => Err(format!(
            "cannot add `{}`: parent is not an object or array",
            display_path(path)
        )),
    }
}

fn remove<T: Tree>(doc: &mut T, path: &[String]) -> Result<T, String> {
    let Some((last, parent_path)) = path.split_last() else {
        return Err("cannot remove the document root".to_string());
    };
    match get_mut(doc, parent_path)?.node() {
        Node::Object(members) => members
            .remove(last)
            .ok_or_else(|| format!("path `{}` does not exist", display_path(path))),
        Node::Array(items) => {
            let idx = parse_index(last, items.len(), false)?;
            Ok(items.remove(idx))
        }
        Node::Scalar => Err(format!("path `{}` does not exist", display_path(path))),
    }
}

// ------------------------------------------------------------------------------------------
// Document types the patch algorithms work on
// ------------------------------------------------------------------------------------------

/// A JSON document JSON Patch / Merge Patch can edit: `serde_json::Value` or [`Doc`].
trait Tree: Clone {
    type Members: Members<Self>;
    /// A value from a patch (rule data) as this type.
    fn from_value(v: &Value) -> Self;
    fn empty_object() -> Self;
    fn node(&mut self) -> Node<'_, Self>;
    /// RFC 6902 `test` equality ([`json_eq`] semantics).
    fn equals(&self, expected: &Value) -> bool;
    /// Compact JSON (for error messages).
    fn to_json(&self) -> String;
}

enum Node<'a, T: Tree> {
    Object(&'a mut T::Members),
    Array(&'a mut Vec<T>),
    Scalar,
}

/// Object members (unique keys, in order).
trait Members<T> {
    fn get_mut(&mut self, key: &str) -> Option<&mut T>;
    /// The member's value, inserting `null` at the end when absent.
    fn get_or_null(&mut self, key: &str) -> &mut T;
    /// Replace the member's value in place, or append it.
    fn insert(&mut self, key: String, value: T);
    /// Remove the member, keeping the order of the others.
    fn remove(&mut self, key: &str) -> Option<T>;
}

impl Tree for Value {
    type Members = Map<String, Value>;

    fn from_value(v: &Value) -> Self {
        v.clone()
    }

    fn empty_object() -> Self {
        Value::Object(Map::new())
    }

    fn node(&mut self) -> Node<'_, Self> {
        match self {
            Value::Object(map) => Node::Object(map),
            Value::Array(items) => Node::Array(items),
            _ => Node::Scalar,
        }
    }

    fn equals(&self, expected: &Value) -> bool {
        json_eq(self, expected)
    }

    fn to_json(&self) -> String {
        self.to_string()
    }
}

impl Members<Value> for Map<String, Value> {
    fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        Map::get_mut(self, key)
    }

    fn get_or_null(&mut self, key: &str) -> &mut Value {
        self.entry(key).or_insert(Value::Null)
    }

    fn insert(&mut self, key: String, value: Value) {
        Map::insert(self, key, value);
    }

    fn remove(&mut self, key: &str) -> Option<Value> {
        self.shift_remove(key)
    }
}

/// A parsed JSON document that keeps every number exactly as written.
///
/// `serde_json::Value` (built without `arbitrary_precision`, which would change number
/// handling in every crate of the build) stores integers beyond 64 bits and long decimals as
/// `f64`, and rejects numbers beyond the `f64` range such as `1e400`. Patching a body through
/// it would corrupt fields the patch never touched, or fail outright.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Doc {
    Null,
    Bool(bool),
    /// The number token exactly as written (valid JSON number syntax).
    Number(String),
    String(String),
    Array(Vec<Doc>),
    Object(DocMembers),
}

/// Object members in document order, with unique keys: a repeated key keeps its first
/// position and its last value (like `serde_json::Map` with `preserve_order`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DocMembers(Vec<(String, Doc)>);

impl Members<Doc> for DocMembers {
    fn get_mut(&mut self, key: &str) -> Option<&mut Doc> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn get_or_null(&mut self, key: &str) -> &mut Doc {
        let idx = match self.0.iter().position(|(k, _)| k == key) {
            Some(idx) => idx,
            None => {
                self.0.push((key.to_string(), Doc::Null));
                self.0.len() - 1
            }
        };
        &mut self.0[idx].1
    }

    fn insert(&mut self, key: String, value: Doc) {
        match self.get_mut(&key) {
            Some(slot) => *slot = value,
            None => self.0.push((key, value)),
        }
    }

    fn remove(&mut self, key: &str) -> Option<Doc> {
        let idx = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(idx).1)
    }
}

impl Tree for Doc {
    type Members = DocMembers;

    fn from_value(v: &Value) -> Self {
        match v {
            Value::Null => Doc::Null,
            Value::Bool(b) => Doc::Bool(*b),
            Value::Number(n) => Doc::Number(n.to_string()),
            Value::String(s) => Doc::String(s.clone()),
            Value::Array(items) => Doc::Array(items.iter().map(Doc::from_value).collect()),
            Value::Object(map) => Doc::Object(DocMembers(
                map.iter()
                    .map(|(k, v)| (k.clone(), Doc::from_value(v)))
                    .collect(),
            )),
        }
    }

    fn empty_object() -> Self {
        Doc::Object(DocMembers::default())
    }

    fn node(&mut self) -> Node<'_, Self> {
        match self {
            Doc::Object(members) => Node::Object(members),
            Doc::Array(items) => Node::Array(items),
            _ => Node::Scalar,
        }
    }

    fn equals(&self, expected: &Value) -> bool {
        match (self, expected) {
            (Doc::Null, Value::Null) => true,
            (Doc::Bool(a), Value::Bool(b)) => a == b,
            (Doc::Number(a), Value::Number(b)) => number_text_eq(a, b),
            (Doc::String(a), Value::String(b)) => a == b,
            (Doc::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.equals(b))
            }
            (Doc::Object(a), Value::Object(b)) => {
                a.0.len() == b.len()
                    && a.0
                        .iter()
                        .all(|(k, v)| b.get(k).is_some_and(|w| v.equals(w)))
            }
            _ => false,
        }
    }

    fn to_json(&self) -> String {
        String::from_utf8_lossy(&self.to_vec()).into_owned()
    }
}

/// [`json_eq`]'s number comparison between a number token and a parsed number: exact for
/// integers within 64 bits, otherwise by `f64` value (finite values only).
fn number_text_eq(text: &str, n: &Number) -> bool {
    if let (Ok(a), Some(b)) = (text.parse::<i64>(), n.as_i64()) {
        return a == b;
    }
    if let (Ok(a), Some(b)) = (text.parse::<u64>(), n.as_u64()) {
        return a == b;
    }
    match (text.parse::<f64>(), n.as_f64()) {
        (Ok(a), Some(b)) => a.is_finite() && a == b,
        _ => false,
    }
}

/// Containers may nest at most this deep (127 levels, `serde_json`'s default limit), which
/// bounds the recursion of parsing, patching, serializing and dropping a [`Doc`].
const MAX_DOC_DEPTH: usize = 127;

impl Doc {
    /// Parse a JSON text (RFC 8259). Accepts exactly what `serde_json` accepts, plus numbers
    /// outside the `f64` range.
    pub(crate) fn parse(input: &[u8]) -> Result<Doc, String> {
        let mut p = DocParser { input, pos: 0 };
        p.skip_ws();
        let doc = p.value(0)?;
        p.skip_ws();
        if p.pos < input.len() {
            return Err(p.error("trailing characters"));
        }
        Ok(doc)
    }

    /// Compact JSON. Strings are escaped exactly as `serde_json` does; numbers are written as
    /// they were read.
    pub(crate) fn to_vec(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Doc::Null => out.extend_from_slice(b"null"),
            Doc::Bool(true) => out.extend_from_slice(b"true"),
            Doc::Bool(false) => out.extend_from_slice(b"false"),
            Doc::Number(n) => out.extend_from_slice(n.as_bytes()),
            Doc::String(s) => write_json_string(out, s),
            Doc::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    item.write(out);
                }
                out.push(b']');
            }
            Doc::Object(members) => {
                out.push(b'{');
                for (i, (key, value)) in members.0.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write_json_string(out, key);
                    out.push(b':');
                    value.write(out);
                }
                out.push(b'}');
            }
        }
    }
}

/// `serde_json`'s string escaping: `"`, `\` and control characters; everything else as is.
fn write_json_string(out: &mut Vec<u8>, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    let bytes = s.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let escape: &[u8] = match b {
            b'"' => b"\\\"",
            b'\\' => b"\\\\",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            0x08 => b"\\b",
            0x0c => b"\\f",
            0x00..=0x1f => {
                out.extend_from_slice(&bytes[start..i]);
                out.extend_from_slice(b"\\u00");
                out.push(HEX[usize::from(b >> 4)]);
                out.push(HEX[usize::from(b & 0xf)]);
                start = i + 1;
                continue;
            }
            _ => continue,
        };
        out.extend_from_slice(&bytes[start..i]);
        out.extend_from_slice(escape);
        start = i + 1;
    }
    out.extend_from_slice(&bytes[start..]);
    out.push(b'"');
}

struct DocParser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl DocParser<'_> {
    fn error(&self, what: &str) -> String {
        format!("{what} at byte {}", self.pos)
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8, what: &str) -> Result<(), String> {
        self.skip_ws();
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(what))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Doc, String> {
        match self.peek() {
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            Some(b'"') => self.string().map(Doc::String),
            Some(b't') => self.literal(b"true", Doc::Bool(true)),
            Some(b'f') => self.literal(b"false", Doc::Bool(false)),
            Some(b'n') => self.literal(b"null", Doc::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("expected value")),
            None => Err(self.error("EOF while parsing a value")),
        }
    }

    fn literal(&mut self, word: &[u8], doc: Doc) -> Result<Doc, String> {
        if self.input[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(doc)
        } else {
            Err(self.error("expected value"))
        }
    }

    fn enter(&self, depth: usize) -> Result<(), String> {
        if depth > MAX_DOC_DEPTH {
            Err(self.error("recursion limit exceeded"))
        } else {
            Ok(())
        }
    }

    fn array(&mut self, depth: usize) -> Result<Doc, String> {
        self.enter(depth)?;
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Doc::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value(depth)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Doc::Array(items));
                }
                _ => return Err(self.error("expected `,` or `]`")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Doc, String> {
        self.enter(depth)?;
        self.pos += 1;
        let mut members: Vec<(String, Doc)> = Vec::new();
        // Key → index, built once the object is big enough for linear lookups to matter.
        let mut index: Option<HashMap<String, usize>> = None;
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Doc::Object(DocMembers(members)));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.error("key must be a string"));
            }
            let key = self.string()?;
            self.expect(b':', "expected `:`")?;
            self.skip_ws();
            let value = self.value(depth)?;
            if index.is_none() && members.len() >= 16 {
                index = Some(
                    members
                        .iter()
                        .enumerate()
                        .map(|(i, (k, _))| (k.clone(), i))
                        .collect(),
                );
            }
            let existing = match &index {
                Some(index) => index.get(&key).copied(),
                None => members.iter().position(|(k, _)| *k == key),
            };
            match existing {
                Some(i) => members[i].1 = value,
                None => {
                    if let Some(index) = &mut index {
                        index.insert(key.clone(), members.len());
                    }
                    members.push((key, value));
                }
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Doc::Object(DocMembers(members)));
                }
                _ => return Err(self.error("expected `,` or `}`")),
            }
        }
    }

    /// A string token (the current byte is its opening quote), decoded by `serde_json` so
    /// escapes, surrogates, control characters and UTF-8 are validated exactly as before.
    fn string(&mut self) -> Result<String, String> {
        let start = self.pos;
        let mut i = start + 1;
        loop {
            match self.input.get(i) {
                Some(b'"') => break,
                Some(b'\\') => i += 2,
                Some(_) => i += 1,
                None => {
                    self.pos = self.input.len();
                    return Err(self.error("EOF while parsing a string"));
                }
            }
        }
        let token = &self.input[start..=i];
        let decoded = serde_json::from_slice::<String>(token)
            .map_err(|e| format!("invalid string at byte {start}: {e}"))?;
        self.pos = i + 1;
        Ok(decoded)
    }

    /// `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?`, kept verbatim.
    fn number(&mut self) -> Result<Doc, String> {
        let start = self.pos;
        let digits = |p: &mut Self| {
            let from = p.pos;
            while matches!(p.peek(), Some(b'0'..=b'9')) {
                p.pos += 1;
            }
            p.pos - from
        };
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                digits(self);
            }
            _ => return Err(self.error("invalid number")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if digits(self) == 0 {
                return Err(self.error("invalid number"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if digits(self) == 0 {
                return Err(self.error("invalid number"));
            }
        }
        // ASCII only, so this is lossless.
        Ok(Doc::Number(
            String::from_utf8_lossy(&self.input[start..self.pos]).into_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn equality_compares_numbers_by_value_and_ignores_key_order() {
        assert!(json_eq(
            &json!({"a": 1, "b": [1, 2.0]}),
            &json!({"b": [1.0, 2], "a": 1.0})
        ));
        assert!(!json_eq(&json!([1, 2]), &json!([2, 1])));
        assert!(!json_eq(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
        assert!(!json_eq(&json!(1), &json!("1")));
        assert!(json_eq(&json!(u64::MAX), &json!(u64::MAX)));
        assert!(!json_eq(&json!(-1), &json!(u64::MAX)));
    }

    #[test]
    fn subset_semantics() {
        let actual = json!({"user": {"id": 7, "name": "x", "tags": ["a", "b", "c"]}, "v": 2});
        assert!(json_subset(&json!({}), &actual));
        assert!(json_subset(&json!({"user": {"id": 7}}), &actual));
        assert!(json_subset(&json!({"user": {"tags": ["a", "b"]}}), &actual));
        assert!(!json_subset(&json!({"user": {"tags": ["b"]}}), &actual));
        assert!(!json_subset(
            &json!({"user": {"tags": ["a", "b", "c", "d"]}}),
            &actual
        ));
        assert!(!json_subset(&json!({"missing": null}), &actual));
        assert!(json_subset(&json!({"v": 2.0}), &actual));
        assert!(!json_subset(&json!({"user": "x"}), &actual));
        assert!(json_subset(
            &json!([{"id": 1}]),
            &json!([{"id": 1, "x": 2}, {"id": 3}])
        ));
    }

    #[test]
    fn merge_patch_rfc7386_examples() {
        let cases = [
            (json!({"a": "b"}), json!({"a": "c"}), json!({"a": "c"})),
            (
                json!({"a": "b"}),
                json!({"b": "c"}),
                json!({"a": "b", "b": "c"}),
            ),
            (json!({"a": "b"}), json!({"a": null}), json!({})),
            (
                json!({"a": "b", "b": "c"}),
                json!({"a": null}),
                json!({"b": "c"}),
            ),
            (json!({"a": ["b"]}), json!({"a": "c"}), json!({"a": "c"})),
            (json!({"a": "c"}), json!({"a": ["b"]}), json!({"a": ["b"]})),
            (
                json!({"a": {"b": "c"}}),
                json!({"a": {"b": "d", "c": null}}),
                json!({"a": {"b": "d"}}),
            ),
            (
                json!({"a": [{"b": "c"}]}),
                json!({"a": [1]}),
                json!({"a": [1]}),
            ),
            (json!(["a", "b"]), json!(["c", "d"]), json!(["c", "d"])),
            (json!({"a": "b"}), json!(["c"]), json!(["c"])),
            (json!({"a": "foo"}), json!(null), json!(null)),
            (json!({"a": "foo"}), json!("bar"), json!("bar")),
            (
                json!({"e": null}),
                json!({"a": 1}),
                json!({"e": null, "a": 1}),
            ),
            (
                json!([1, 2]),
                json!({"a": "b", "c": null}),
                json!({"a": "b"}),
            ),
            (
                json!({}),
                json!({"a": {"bb": {"ccc": null}}}),
                json!({"a": {"bb": {}}}),
            ),
        ];
        for (target, patch, expected) in cases {
            let mut t = target.clone();
            merge_patch(&mut t, &patch);
            assert_eq!(t, expected, "target {target} patch {patch}");
        }
    }

    #[test]
    fn merge_patch_preserves_key_order() {
        let mut t = json!({"a": 1, "b": 2, "c": 3});
        merge_patch(&mut t, &json!({"a": null}));
        assert_eq!(serde_json::to_string(&t).unwrap(), r#"{"b":2,"c":3}"#);
    }

    fn patch(doc: Value, ops: Value) -> Result<Value, String> {
        let mut d = doc;
        apply_patch(&mut d, ops.as_array().unwrap())?;
        Ok(d)
    }

    #[test]
    fn json_patch_all_ops() {
        let doc = json!({"foo": "bar", "list": [1, 2, 3], "obj": {"x": 1}});
        assert_eq!(
            patch(
                doc.clone(),
                json!([{"op": "add", "path": "/baz", "value": "qux"}])
            )
            .unwrap(),
            json!({"foo": "bar", "list": [1, 2, 3], "obj": {"x": 1}, "baz": "qux"})
        );
        assert_eq!(
            patch(
                doc.clone(),
                json!([{"op": "add", "path": "/list/1", "value": 9}])
            )
            .unwrap()["list"],
            json!([1, 9, 2, 3])
        );
        assert_eq!(
            patch(
                doc.clone(),
                json!([{"op": "add", "path": "/list/-", "value": 4}])
            )
            .unwrap()["list"],
            json!([1, 2, 3, 4])
        );
        assert_eq!(
            patch(doc.clone(), json!([{"op": "remove", "path": "/list/0"}])).unwrap()["list"],
            json!([2, 3])
        );
        assert_eq!(
            patch(
                doc.clone(),
                json!([{"op": "replace", "path": "/foo", "value": [1]}])
            )
            .unwrap()["foo"],
            json!([1])
        );
        let moved = patch(
            doc.clone(),
            json!([{"op": "move", "from": "/obj/x", "path": "/y"}]),
        )
        .unwrap();
        assert_eq!(moved["y"], json!(1));
        assert_eq!(moved["obj"], json!({}));
        let copied = patch(
            doc.clone(),
            json!([{"op": "copy", "from": "/list", "path": "/copy"}]),
        )
        .unwrap();
        assert_eq!(copied["copy"], json!([1, 2, 3]));
        assert_eq!(copied["list"], json!([1, 2, 3]));
        assert!(
            patch(
                doc.clone(),
                json!([{"op": "test", "path": "/obj/x", "value": 1.0}])
            )
            .is_ok()
        );
        assert!(
            patch(
                doc.clone(),
                json!([{"op": "test", "path": "/obj/x", "value": 2}])
            )
            .is_err()
        );
        assert_eq!(
            patch(
                doc.clone(),
                json!([{"op": "replace", "path": "", "value": 5}])
            )
            .unwrap(),
            json!(5)
        );
        assert_eq!(
            patch(
                json!({"a/b": 1, "m~n": 2}),
                json!([{"op": "remove", "path": "/a~1b"}, {"op": "remove", "path": "/m~0n"}])
            )
            .unwrap(),
            json!({})
        );
    }

    #[test]
    fn json_patch_errors_are_atomic() {
        let mut doc = json!({"a": 1});
        let ops =
            json!([{"op": "add", "path": "/b", "value": 2}, {"op": "remove", "path": "/missing"}]);
        assert!(apply_patch(&mut doc, ops.as_array().unwrap()).is_err());
        assert_eq!(doc, json!({"a": 1}));

        for bad in [
            json!([{"op": "remove", "path": "/list/3"}]),
            json!([{"op": "remove", "path": "/list/01"}]),
            json!([{"op": "remove", "path": "/list/-"}]),
            json!([{"op": "add", "path": "/list/5", "value": 1}]),
            json!([{"op": "add", "path": "/nope/x", "value": 1}]),
            json!([{"op": "replace", "path": "/nope", "value": 1}]),
            json!([{"op": "remove", "path": ""}]),
            json!([{"op": "add", "path": "/foo/x", "value": 1}]),
        ] {
            assert!(
                patch(json!({"list": [1, 2, 3], "foo": "s"}), bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    /// [`Doc`] must accept exactly the JSON `serde_json` accepts (except numbers beyond the
    /// `f64` range, which it keeps instead of rejecting) and serialize it the same way.
    #[test]
    fn doc_agrees_with_serde_json() {
        let accepted = [
            "null",
            " true ",
            "false",
            "0",
            "-0",
            "12",
            "-12",
            "1.5",
            "-1.25e3",
            "1E+2",
            "2e-3",
            "\"\"",
            r#""plain""#,
            r#""esc \" \\ \/ \b \f \n \r \t \u00e9 \ud83d\ude00 \u001f""#,
            "\"caf\u{e9} \u{1f600} \u{2028}\"",
            "[]",
            "[1,[2,[3]],{}]",
            "{}",
            r#"{"a":1,"b":[true,false,null],"c":{"d":"e"}}"#,
            " {\n\t\"a\" : [ 1 , 2 ] ,\r\n \"b\":{ } } ",
            r#"{"a":1,"b":2,"a":3}"#,
            r#"{"k0":0,"k1":1,"k2":2,"k3":3,"k4":4,"k5":5,"k6":6,"k7":7,"k8":8,"k9":9,"k10":10,"k11":11,"k12":12,"k13":13,"k14":14,"k15":15,"k16":16,"k17":17,"k2":"dup","k17":"dup"}"#,
        ];
        for text in accepted {
            let expected: Value = serde_json::from_str(text).unwrap();
            let doc = Doc::parse(text.as_bytes()).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert!(doc.equals(&expected), "{text}");
            // The output is valid JSON with the same value (numbers kept as written)...
            let written: Value = serde_json::from_slice(&doc.to_vec()).unwrap();
            assert_eq!(written, expected, "{text}");
            // ... and byte for byte serde_json's output when serde_json wrote the input.
            let canonical = serde_json::to_string(&expected).unwrap();
            let doc = Doc::parse(canonical.as_bytes()).unwrap();
            assert_eq!(
                String::from_utf8(doc.to_vec()).unwrap(),
                canonical,
                "{text}"
            );
            assert_eq!(doc, Doc::from_value(&expected), "{text}");
        }
        let rejected: &[&[u8]] = &[
            b"",
            b" ",
            b"01",
            b"1.",
            b".5",
            b"-",
            b"+1",
            b"1e",
            b"1e+",
            b"--1",
            b"0x10",
            b"NaN",
            b"Infinity",
            b"[1,]",
            b"[1 2]",
            b"{\"a\":1,}",
            b"{a:1}",
            b"{\"a\" 1}",
            b"{1:2}",
            b"[",
            b"]",
            b"{",
            b"nul",
            b"tru",
            b"truex",
            b"1 2",
            b"\"abc",
            b"\"\\x\"",
            b"\"\\u12\"",
            b"\"\x01\"",
            b"\"\\ud800\"",
            b"\"\xff\"",
            b"\xef\xbb\xbf{}",
            b"'a'",
            b"[1]x",
        ];
        for text in rejected {
            assert!(
                serde_json::from_slice::<Value>(text).is_err(),
                "serde_json accepts {:?}",
                String::from_utf8_lossy(text)
            );
            assert!(
                Doc::parse(text).is_err(),
                "Doc accepts {:?}",
                String::from_utf8_lossy(text)
            );
        }
        // Nesting limit: the same as serde_json's.
        for depth in [100, 126, 127, 128, 129, 200] {
            let text = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
            assert_eq!(
                Doc::parse(text.as_bytes()).is_ok(),
                serde_json::from_str::<Value>(&text).is_ok(),
                "depth {depth}"
            );
            let text = format!("{}1{}", "{\"a\":".repeat(depth), "}".repeat(depth));
            assert_eq!(
                Doc::parse(text.as_bytes()).is_ok(),
                serde_json::from_str::<Value>(&text).is_ok(),
                "object depth {depth}"
            );
        }
        // Random token soup: both parsers agree on what is valid.
        const PIECES: &[&str] = &[
            "{",
            "}",
            "[",
            "]",
            ",",
            ":",
            "\"",
            "\"k\"",
            "a",
            "0",
            "1",
            "9",
            "-",
            "+",
            ".",
            "e",
            "E",
            "\\",
            "u",
            " ",
            "true",
            "null",
            "\"\\u00e9\"",
        ];
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..100_000 {
            let n = (next() % 9) as usize;
            let text: String = (0..n)
                .map(|_| PIECES[(next() % PIECES.len() as u64) as usize])
                .collect();
            let ours = Doc::parse(text.as_bytes());
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => {
                    let doc = ours.unwrap_or_else(|e| panic!("{text:?}: {e}"));
                    assert!(doc.equals(&v), "{text:?}");
                }
                Err(e) if e.to_string().contains("number out of range") => {
                    assert!(ours.is_ok(), "{text:?}");
                }
                Err(_) => assert!(ours.is_err(), "{text:?} accepted"),
            }
        }
    }

    #[test]
    fn doc_string_escaping_matches_serde_json() {
        let mut s = String::new();
        for c in (0u32..0x80).chain([0xe9, 0x2028, 0x2029, 0xfeff, 0x1f600]) {
            if let Some(c) = char::from_u32(c) {
                s.push(c);
            }
        }
        for text in [s.as_str(), "", "\"", "\\", "a\u{0}b", "\u{7f}"] {
            let doc = Doc::String(text.to_string());
            assert_eq!(
                String::from_utf8(doc.to_vec()).unwrap(),
                serde_json::to_string(text).unwrap()
            );
            assert_eq!(Doc::parse(&doc.to_vec()).unwrap(), doc);
        }
    }

    #[test]
    fn doc_merge_and_patch_follow_the_value_versions() {
        let target = json!({"a": {"b": "c", "x": [1, 2]}, "n": 1.5, "l": [1, {"m": 2}]});
        let merges = [
            json!({"a": {"b": "d", "c": null, "x": null}, "z": 1}),
            json!(null),
            json!([1]),
            json!({"n": {"deep": {"er": true}}}),
        ];
        for patch in &merges {
            let mut value = target.clone();
            merge_patch(&mut value, patch);
            let mut doc = Doc::from_value(&target);
            merge_patch_doc(&mut doc, patch);
            assert_eq!(doc, Doc::from_value(&value), "{patch}");
        }
        let patches = [
            json!([{"op": "add", "path": "/a/y", "value": {"q": [1]}}]),
            json!([{"op": "remove", "path": "/l/0"}, {"op": "replace", "path": "/n", "value": "s"}]),
            json!([{"op": "move", "from": "/a/x", "path": "/l/1"}, {"op": "copy", "from": "/n", "path": "/n2"}]),
            json!([{"op": "test", "path": "/n", "value": 1.5}, {"op": "add", "path": "", "value": 3}]),
            json!([{"op": "test", "path": "/a", "value": {"x": [1, 2], "b": "c"}}]),
            json!([{"op": "test", "path": "/a", "value": {"x": [1, 2]}}]),
            json!([{"op": "remove", "path": "/missing"}]),
            json!([{"op": "add", "path": "/l/9", "value": 1}]),
            json!([{"op": "add", "path": "/n/x", "value": 1}]),
        ];
        for ops in &patches {
            let ops = ops.as_array().unwrap();
            let mut value = target.clone();
            let value_result = apply_patch(&mut value, ops);
            let mut doc = Doc::from_value(&target);
            let doc_result = apply_patch_doc(&mut doc, ops);
            assert_eq!(doc_result, value_result, "{ops:?}");
            assert_eq!(doc, Doc::from_value(&value), "{ops:?}");
        }
    }

    #[test]
    fn json_patch_validation() {
        assert!(validate_patch(&[json!({"op": "add", "path": "/a", "value": 1})]).is_ok());
        assert!(validate_patch(&[json!({"op": "nope", "path": "/a"})]).is_err());
        assert!(validate_patch(&[json!({"op": "add", "path": "a", "value": 1})]).is_err());
        assert!(validate_patch(&[json!({"op": "add", "path": "/a"})]).is_err());
        assert!(validate_patch(&[json!({"op": "copy", "path": "/a"})]).is_err());
        assert!(validate_patch(&[json!({"op": "move", "from": "/a", "path": "/a/b"})]).is_err());
        assert!(validate_patch(&[json!({"op": "remove", "path": "/a~2"})]).is_err());
        assert!(validate_patch(&[json!("add")]).is_err());
    }
}
