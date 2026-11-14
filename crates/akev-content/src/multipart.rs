//! `multipart/*` body parsing (RFC 2046 framing, RFC 7578 form-data).
//!
//! * The boundary comes from the Content-Type `boundary` parameter (quoted or not).
//! * The preamble before the first delimiter and the epilogue after the close delimiter are
//!   ignored; delimiters must start a line (CRLF, or bare LF for sloppy clients) and may be
//!   followed by transport padding (spaces/tabs).
//! * Part headers support obsolete line folding and are decoded as UTF-8 (lossy) since browsers
//!   send raw UTF-8 in `filename`.
//! * `Content-Disposition` `name` / `filename`; RFC 5987/8187 `filename*=charset'lang'pct` wins
//!   over `filename`.
//! * A missing close delimiter (truncated capture) yields the parts so far, the last one
//!   extending to the end of the body.

use std::ops::Range;

use serde::Serialize;

use crate::budget::Budget;
use crate::detect::{detect_kind, is_textual};
use crate::{ContentError, Result};

/// Parts up to this size get a `text` rendering when textual.
pub const MAX_TEXT_PART: usize = 64 * 1024;

/// One multipart part.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    /// Part headers in order, as sent.
    pub headers: Vec<(String, String)>,
    /// `Content-Disposition` `name`.
    pub name: Option<String>,
    /// `Content-Disposition` `filename*` (decoded) or `filename`.
    pub filename: Option<String>,
    /// The part's `Content-Type` header.
    pub content_type: Option<String>,
    /// Size of the part body in bytes.
    pub size: usize,
    /// Text content when the part is textual & < 64KB.
    pub text: Option<String>,
    /// The part body (not serialized).
    #[serde(skip)]
    pub data: bytes::Bytes,
}

/// Parse a multipart body using the boundary from the Content-Type header.
pub fn parse(body: &[u8], content_type: &str) -> Result<Vec<Part>> {
    Ok(parse_raw(body, content_type, &mut Budget::unlimited())?
        .into_iter()
        .map(|raw| {
            let mut part = raw.describe(body);
            part.data = bytes::Bytes::copy_from_slice(&body[raw.data]);
            part
        })
        .collect())
}

/// A part located inside the body (no data copy).
pub(crate) struct RawPart {
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) data: Range<usize>,
}

impl RawPart {
    /// Build the [`Part`] metadata (with empty `data`).
    pub(crate) fn describe(&self, body: &[u8]) -> Part {
        let data = &body[self.data.clone()];
        let header = |n: &str| {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(n))
                .map(|(_, v)| v.as_str())
        };
        let (name, filename) = header("content-disposition")
            .map(parse_disposition)
            .unwrap_or_default();
        let content_type = header("content-type").map(str::to_string);
        let text = part_text(data, content_type.as_deref(), filename.as_deref());
        Part {
            headers: self.headers.clone(),
            name,
            filename,
            content_type,
            size: data.len(),
            text,
            data: bytes::Bytes::new(),
        }
    }
}

fn part_text(data: &[u8], content_type: Option<&str>, filename: Option<&str>) -> Option<String> {
    if data.len() > MAX_TEXT_PART {
        return None;
    }
    match (content_type, filename) {
        // A plain form field: text/plain by definition (RFC 7578 §4.4).
        (None, None) => {
            let (text, errors) = crate::text::decode_text(data, None);
            (!errors || crate::detect::looks_like_text(data)).then_some(text)
        }
        (ct, name) => {
            let kind = detect_kind(ct, data, name.unwrap_or(""));
            let svg = kind == akev_core::ContentKind::Image && crate::image::is_svg(data);
            (is_textual(kind) || svg).then(|| crate::text::decode_text(data, ct).0)
        }
    }
}

fn err(message: &str) -> ContentError {
    ContentError::Parse {
        what: "multipart body",
        message: message.to_string(),
    }
}

/// JSON values in a described part besides its headers: the object, `headers`, `name`,
/// `filename`, `contentType`, `size` and `text`.
pub(crate) const PART_NODES: usize = 7;

/// Locate the parts of a multipart body. Each part ([`PART_NODES`] values plus the size of its
/// text rendering) and header (three values plus its text) is charged to `budget`; when it
/// runs out, the parts located so far are returned.
pub(crate) fn parse_raw(
    body: &[u8],
    content_type: &str,
    budget: &mut Budget,
) -> Result<Vec<RawPart>> {
    let boundary = crate::util::param(content_type, "boundary")
        .ok_or_else(|| err("missing boundary parameter"))?;
    if boundary.is_empty() || boundary.len() > 256 {
        return Err(err("invalid boundary parameter"));
    }
    let mut delimiter = Vec::with_capacity(boundary.len() + 2);
    delimiter.extend_from_slice(b"--");
    delimiter.extend_from_slice(boundary.as_bytes());
    let finder = memchr::memmem::Finder::new(&delimiter);

    // Find a delimiter starting at or after `from` that begins a line and is followed by
    // padding/CRLF/"--"/EOF. Returns (start of delimiter, end of preceding data).
    let find_delimiter = |from: usize| -> Option<(usize, usize)> {
        let mut search = from;
        while let Some(rel) = finder.find(&body[search..]) {
            let at = search + rel;
            let line_start = at == 0 || body[at - 1] == b'\n';
            let after = &body[at + delimiter.len()..];
            let terminated = after.is_empty()
                || after.starts_with(b"--")
                || matches!(after[0], b'\r' | b'\n' | b' ' | b'\t');
            if line_start && terminated {
                let data_end = if at >= 2 && &body[at - 2..at] == b"\r\n" {
                    at - 2
                } else if at >= 1 && body[at - 1] == b'\n' {
                    at - 1
                } else {
                    at
                };
                return Some((at, data_end));
            }
            search = at + 1;
        }
        None
    };

    let (mut at, _) =
        find_delimiter(0).ok_or_else(|| err("no boundary delimiter found in body"))?;
    let mut parts = Vec::new();
    loop {
        let mut p = at + delimiter.len();
        if body[p..].starts_with(b"--") {
            break; // close delimiter; the rest is epilogue
        }
        while p < body.len() && (body[p] == b' ' || body[p] == b'\t') {
            p += 1;
        }
        if body[p..].starts_with(b"\r\n") {
            p += 2;
        } else if body[p..].starts_with(b"\n") {
            p += 1;
        } else if p >= body.len() {
            break; // truncated right after a delimiter
        }
        let (headers, data_start) = parse_headers(body, p, budget);
        if budget.exhausted() {
            break;
        }
        let (data, next) = match find_delimiter(data_start) {
            Some((next, data_end)) => (data_start..data_end.max(data_start), Some(next)),
            None => (data_start..body.len(), None),
        };
        // The part's values, and its text rendering (at most `MAX_TEXT_PART` bytes).
        if !budget.charge(PART_NODES, data.len().min(MAX_TEXT_PART)) {
            break;
        }
        parts.push(RawPart { headers, data });
        match next {
            Some(next) => at = next,
            None => break,
        }
    }
    Ok(parts)
}

/// Parse a header block starting at `p`; returns headers and the offset after the blank line.
/// Stops early (the budget is then exhausted) when `budget` can't hold another header.
fn parse_headers(body: &[u8], mut p: usize, budget: &mut Budget) -> (Vec<(String, String)>, usize) {
    let mut headers: Vec<(String, String)> = Vec::new();
    while p < body.len() {
        let end = memchr::memchr(b'\n', &body[p..]).map_or(body.len(), |e| p + e);
        let mut line = &body[p..end];
        if line.ends_with(b"\r") {
            line = &line[..line.len() - 1];
        }
        p = (end + 1).min(body.len());
        if line.is_empty() {
            return (headers, p);
        }
        // A header (or folded continuation): the pair and its two strings.
        if !budget.charge(3, line.len()) {
            return (headers, body.len());
        }
        if matches!(line[0], b' ' | b'\t') {
            if let Some(last) = headers.last_mut() {
                last.1.push(' ');
                last.1.push_str(String::from_utf8_lossy(line).trim());
            }
            continue;
        }
        if let Some(colon) = memchr::memchr(b':', line) {
            let name = String::from_utf8_lossy(&line[..colon]).trim().to_string();
            let value = String::from_utf8_lossy(&line[colon + 1..])
                .trim()
                .to_string();
            headers.push((name, value));
        }
    }
    (headers, body.len())
}

/// Extract `name` and `filename` from a Content-Disposition value.
pub(crate) fn parse_disposition(value: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut filename = None;
    let mut filename_ext = None;
    for (k, v) in crate::util::params(value) {
        match k.as_str() {
            "name" if name.is_none() => name = Some(v),
            "filename" if filename.is_none() => filename = Some(v),
            "filename*" if filename_ext.is_none() => filename_ext = decode_ext_value(&v),
            _ => {}
        }
    }
    (name, filename_ext.or(filename))
}

/// RFC 8187 ext-value: `charset'[language]'pct-encoded`.
pub(crate) fn decode_ext_value(v: &str) -> Option<String> {
    let mut it = v.splitn(3, '\'');
    let charset = it.next()?;
    let _lang = it.next()?;
    let encoded = it.next()?;
    let bytes: Vec<u8> = percent_encoding::percent_decode_str(encoded).collect();
    let enc = if charset.is_empty() {
        encoding_rs::UTF_8
    } else {
        encoding_rs::Encoding::for_label(charset.trim().as_bytes())?
    };
    let (text, _) = enc.decode_without_bom_handling(&bytes);
    Some(text.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CT: &str = "multipart/form-data; boundary=----WebKitFormBoundary7MA4YWxkTrZu0gW";

    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"preamble to ignore\r\n");
        b.extend_from_slice(b"------WebKitFormBoundary7MA4YWxkTrZu0gW\r\n");
        b.extend_from_slice(b"Content-Disposition: form-data; name=\"title\"\r\n\r\n");
        b.extend_from_slice(b"Hello world\r\n");
        b.extend_from_slice(b"------WebKitFormBoundary7MA4YWxkTrZu0gW\r\n");
        b.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"a.json\"\r\n",
        );
        b.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
        b.extend_from_slice(b"{\"a\": 1}\r\n");
        b.extend_from_slice(b"------WebKitFormBoundary7MA4YWxkTrZu0gW\r\n");
        b.extend_from_slice(b"Content-Disposition: form-data; name=\"img\"; filename=\"x.png\"; filename*=UTF-8''%E2%82%AC%20rates.png\r\n");
        b.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
        b.extend_from_slice(b"\x89PNG\r\n\x1a\n\0\0binary--data\r\n");
        b.extend_from_slice(b"------WebKitFormBoundary7MA4YWxkTrZu0gW--\r\n");
        b.extend_from_slice(b"epilogue");
        b
    }

    #[test]
    fn parses_form_data() {
        let body = sample();
        let parts = parse(&body, CT).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].name.as_deref(), Some("title"));
        assert_eq!(parts[0].text.as_deref(), Some("Hello world"));
        assert_eq!(parts[0].size, 11);
        assert_eq!(parts[0].content_type, None);
        assert_eq!(parts[1].filename.as_deref(), Some("a.json"));
        assert_eq!(parts[1].content_type.as_deref(), Some("application/json"));
        assert_eq!(parts[1].text.as_deref(), Some("{\"a\": 1}"));
        assert_eq!(parts[2].filename.as_deref(), Some("€ rates.png"));
        assert_eq!(parts[2].text, None);
        assert_eq!(&parts[2].data[..], b"\x89PNG\r\n\x1a\n\0\0binary--data");
        let json = serde_json::to_value(&parts[1]).unwrap();
        assert_eq!(json["contentType"], "application/json");
        assert!(json.get("data").is_none());
        assert_eq!(json["headers"][0][0], "Content-Disposition");
    }

    #[test]
    fn quoted_boundary_lf_endings_and_truncation() {
        let ct = "multipart/mixed; boundary=\"simple boundary\"";
        let body = b"--simple boundary\nContent-Type: text/plain; charset=iso-8859-1\n\ncaf\xe9\n--simple boundary  \n\nno headers\n--simple boundary\nX: y\n\ntruncated part";
        let parts = parse(body, ct).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].text.as_deref(), Some("café"));
        assert_eq!(parts[1].headers, vec![]);
        assert_eq!(parts[1].text.as_deref(), Some("no headers"));
        assert_eq!(&parts[2].data[..], b"truncated part");
    }

    #[test]
    fn boundary_lookalikes_inside_data() {
        let ct = "multipart/form-data; boundary=XyZ";
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nline --XyZ not at start\r\n--XyZabc not terminated\r\n--XyZ--";
        let parts = parse(body, ct).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(
            parts[0].text.as_deref(),
            Some("line --XyZ not at start\r\n--XyZabc not terminated")
        );
    }

    #[test]
    fn empty_parts_and_folding() {
        let ct = "multipart/form-data; boundary=b";
        let body = b"--b\r\nContent-Disposition: form-data;\r\n name=\"folded\"\r\n\r\n\r\n--b--";
        let parts = parse(body, ct).unwrap();
        assert_eq!(parts[0].name.as_deref(), Some("folded"));
        assert_eq!(parts[0].size, 0);
        assert_eq!(parts[0].text.as_deref(), Some(""));
    }

    #[test]
    fn errors() {
        assert!(parse(b"--a\r\n\r\nx\r\n--a--", "multipart/form-data").is_err());
        assert!(parse(b"no delimiters here", "multipart/form-data; boundary=a").is_err());
        assert!(parse(b"", "multipart/form-data; boundary=a").is_err());
    }

    #[test]
    fn disposition_parsing() {
        assert_eq!(
            parse_disposition(
                "attachment; filename=\"plain.txt\"; filename*=UTF-8''na%C3%AFve.txt"
            ),
            (None, Some("naïve.txt".into()))
        );
        assert_eq!(
            parse_disposition("form-data; name=field; filename=unquoted.bin"),
            (Some("field".into()), Some("unquoted.bin".into()))
        );
        assert_eq!(
            decode_ext_value("iso-8859-1'en'%A3%20rates"),
            Some("£ rates".into())
        );
        assert_eq!(decode_ext_value("bogus-charset''x"), None);
        assert_eq!(decode_ext_value("no-quotes"), None);
    }
}
