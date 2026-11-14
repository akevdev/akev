//! Tag-aware HTML pretty-printer.
//!
//! * Block-level elements start a new line and indent their children; inline (phrasing)
//!   elements and text flow on the current line, with whitespace runs collapsed to one space
//!   (which doesn't change rendering outside `pre`-like elements). An element whose content is
//!   only inline stays on one line: `<p>Hello <b>world</b></p>`.
//! * Void elements (`<br>`, `<img>`, `<meta>`, ...) and self-closing tags don't indent.
//! * `<pre>`, `<textarea>`, `<listing>`, `<plaintext>`, `<xmp>`, `<iframe>`, `<noembed>`,
//!   `<noframes>` content is copied verbatim. `<script>` / `<style>` content is beautified with
//!   the JavaScript / CSS / JSON formatter when the type is known (kept verbatim otherwise).
//! * Tags (with their attributes), comments, doctypes and CDATA are copied verbatim.
//! * Implied end tags (`<li>`, `<p>`, `<td>`, `<tr>`, `<option>`, `<dt>`/`<dd>`, ...) are
//!   handled with a simplified version of the HTML tree-construction rules so indentation
//!   doesn't drift on real-world markup; `<html>` doesn't indent its children (like
//!   js-beautify's default).
//!
//! Linear time with an explicit element stack.

use crate::util::push_indent;

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr", "keygen", "command", "basefont", "bgsound", "frame", "image",
];
const INLINE: &[&str] = &[
    "a", "abbr", "acronym", "b", "bdi", "bdo", "big", "br", "button", "cite", "code", "data",
    "del", "dfn", "em", "font", "i", "img", "input", "ins", "kbd", "label", "mark", "meter",
    "output", "progress", "q", "ruby", "rp", "rt", "s", "samp", "small", "span", "strike",
    "strong", "sub", "sup", "time", "tt", "u", "var", "wbr",
];
const VERBATIM: &[&str] = &[
    "pre",
    "textarea",
    "listing",
    "plaintext",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
];
/// Start tags that close an open `<p>`.
const CLOSES_P: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "details",
    "dialog",
    "div",
    "dl",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];
/// How far down the open-element stack implied-end / end-tag matching looks.
const MAX_SCOPE_SEARCH: usize = 512;

/// Elements taking part in implied-end-tag handling. Tag names are interned to `u32` ids and
/// these come first, so a tracked element's id is its bit position in a `u64` scope mask
/// (scope searches compare integers, not strings).
const TRACKED: [&str; 30] = [
    "li", "ul", "ol", "menu", "dt", "dd", "dl", "tr", "table", "thead", "tbody", "tfoot", "td",
    "th", "option", "select", "datalist", "optgroup", "rt", "rp", "ruby", "p", "html", "body",
    "button", "object", "template", "marquee", "applet", "caption",
];

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Scope mask of tracked names (evaluated at compile time; unknown names contribute nothing).
const fn mask(names: &[&str]) -> u64 {
    let mut m = 0u64;
    let mut i = 0;
    while i < names.len() {
        let mut j = 0;
        while j < TRACKED.len() {
            if str_eq(names[i], TRACKED[j]) {
                m |= 1 << j;
            }
            j += 1;
        }
        i += 1;
    }
    m
}

/// Id of the `html` element (its children aren't indented).
const HTML_ID: u32 = mask(&["html"]).trailing_zeros();

/// Implied-end rules: (elements implicitly closed, elements that stop the search).
const LI_RULE: (u64, u64) = (mask(&["li"]), mask(&["ul", "ol", "menu"]));
const DT_DD_RULE: (u64, u64) = (mask(&["dt", "dd"]), mask(&["dl"]));
const TR_RULE: (u64, u64) = (mask(&["tr"]), mask(&["table", "thead", "tbody", "tfoot"]));
const CELL_RULE: (u64, u64) = (mask(&["td", "th"]), mask(&["tr", "table"]));
const SECTION_RULE: (u64, u64) = (mask(&["thead", "tbody", "tfoot"]), mask(&["table"]));
const OPTION_RULE: (u64, u64) = (mask(&["option"]), mask(&["select", "datalist", "optgroup"]));
const OPTGROUP_RULE: (u64, u64) = (mask(&["option", "optgroup"]), mask(&["select"]));
const RUBY_RULE: (u64, u64) = (mask(&["rt", "rp"]), mask(&["ruby"]));
/// A `<p>` is closed by [`CLOSES_P`] start tags, searching up to the scope boundary.
const P_RULE: (u64, u64) = (
    mask(&["p"]),
    mask(&[
        "html", "body", "table", "td", "th", "button", "object", "template", "marquee", "applet",
        "caption",
    ]),
);

/// Scope-mask bit of an interned tag id (0 for untracked names).
fn id_bit(id: u32) -> u64 {
    if (id as usize) < TRACKED.len() {
        1 << id
    } else {
        0
    }
}

#[derive(Debug)]
struct Elem {
    /// Interned tag name.
    id: u32,
    block: bool,
}

struct Printer {
    out: String,
    /// Current (not yet flushed) line content.
    line: String,
    line_depth: usize,
    /// Index in `stack` of the block element whose start tag begins `line`, if its content
    /// so far is inline only.
    line_owner: Option<usize>,
    pending_space: bool,
    depth: usize,
    stack: Vec<Elem>,
    /// Tag name → id (one hash per tag; std's randomized hasher, so crafted names can't
    /// degrade it).
    ids: std::collections::HashMap<String, u32>,
    /// Number of open elements per id: implied-end and end-tag handling skip the stack search
    /// when nothing could match (keeps unclosed / stray tag soup linear and fast).
    counts: Vec<u32>,
}

impl Printer {
    fn new(capacity: usize) -> Self {
        let ids = TRACKED
            .iter()
            .enumerate()
            .map(|(i, n)| ((*n).to_string(), i as u32))
            .collect();
        Printer {
            out: String::with_capacity(capacity),
            line: String::new(),
            line_depth: 0,
            line_owner: None,
            pending_space: false,
            depth: 0,
            stack: Vec::new(),
            ids,
            counts: vec![0; TRACKED.len()],
        }
    }

    /// Intern a tag name.
    fn id_for(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = self.counts.len() as u32;
        self.ids.insert(name.to_string(), id);
        self.counts.push(0);
        id
    }

    fn push_elem(&mut self, name: &str, block: bool) {
        let id = self.id_for(name);
        self.counts[id as usize] += 1;
        self.stack.push(Elem { id, block });
    }

    fn pop_elem(&mut self) -> Option<Elem> {
        let e = self.stack.pop()?;
        if let Some(c) = self.counts.get_mut(e.id as usize) {
            *c = c.saturating_sub(1);
        }
        Some(e)
    }

    /// Whether any element of the scope mask `m` is open.
    fn any_open(&self, mut m: u64) -> bool {
        while m != 0 {
            if self.counts[m.trailing_zeros() as usize] > 0 {
                return true;
            }
            m &= m - 1;
        }
        false
    }

    fn flush(&mut self) {
        let content = self.line.trim_end();
        if !content.is_empty() {
            if !self.out.is_empty() {
                self.out.push('\n');
            }
            push_indent(&mut self.out, self.line_depth);
            self.out.push_str(content);
        }
        self.line.clear();
        self.line_owner = None;
        self.pending_space = false;
    }

    /// Write a complete line (flushing the current one first).
    fn own_line(&mut self, s: &str, depth: usize) {
        self.flush();
        self.line_depth = depth;
        self.line.push_str(s);
        self.flush();
    }

    /// Append inline content to the current line.
    fn inline(&mut self, s: &str) {
        if self.line.is_empty() {
            self.line_depth = self.depth;
        } else if self.pending_space {
            self.line.push(' ');
        }
        self.pending_space = false;
        self.line.push_str(s);
    }

    fn text(&mut self, t: &str) {
        let mut words = t.split_ascii_whitespace().peekable();
        let leading = t.starts_with(|c: char| c.is_ascii_whitespace());
        let trailing = t.ends_with(|c: char| c.is_ascii_whitespace());
        if words.peek().is_none() {
            if !t.is_empty() && !self.line.is_empty() {
                self.pending_space = true;
            }
            return;
        }
        if leading && !self.line.is_empty() {
            self.pending_space = true;
        }
        let mut first = true;
        for w in words {
            if !first {
                self.pending_space = true;
            }
            self.inline(w);
            first = false;
        }
        self.pending_space = trailing;
    }

    /// Index of the innermost open element matching `pred`, searching at most
    /// [`MAX_SCOPE_SEARCH`] elements from the top (keeps pathological nesting linear).
    fn find_open(&self, pred: impl Fn(&Elem) -> bool) -> Option<usize> {
        let floor = self.stack.len().saturating_sub(MAX_SCOPE_SEARCH);
        self.stack[floor..]
            .iter()
            .rposition(pred)
            .map(|i| floor + i)
    }

    /// Pop the innermost open element of `targets`, unless an element of `stops` is closer.
    fn close_any(&mut self, (targets, stops): (u64, u64)) {
        if !self.any_open(targets) {
            return;
        }
        if let Some(pos) = self.find_open(|e| id_bit(e.id) & (targets | stops) != 0)
            && id_bit(self.stack[pos].id) & targets != 0
        {
            self.pop_to(pos);
        }
    }

    /// Pop elements implicitly closed before a start tag of `name`.
    fn implied_end(&mut self, name: &str) {
        match name {
            "li" => self.close_any(LI_RULE),
            "dt" | "dd" => self.close_any(DT_DD_RULE),
            "tr" => self.close_any(TR_RULE),
            "td" | "th" => self.close_any(CELL_RULE),
            "thead" | "tbody" | "tfoot" => self.close_any(SECTION_RULE),
            "option" => self.close_any(OPTION_RULE),
            "optgroup" => self.close_any(OPTGROUP_RULE),
            "rt" | "rp" => self.close_any(RUBY_RULE),
            _ => {}
        }
        if CLOSES_P.contains(&name) {
            self.close_any(P_RULE);
        }
    }

    /// Pop the stack down to (and including) `pos`, adjusting depth for block elements.
    fn pop_to(&mut self, pos: usize) {
        while self.stack.len() > pos {
            if let Some(e) = self.pop_elem()
                && e.block
            {
                if self.line_owner == Some(self.stack.len()) {
                    // the element's content was inline; keep it on its line
                    self.line_owner = None;
                }
                self.depth = self.depth.saturating_sub(1);
            }
        }
    }

    fn start_block(&mut self, raw: &str, name: &str, indents: bool) {
        self.flush();
        self.line_depth = self.depth;
        self.line.push_str(raw);
        self.push_elem(name, true);
        self.line_owner = Some(self.stack.len() - 1);
        if indents {
            self.depth += 1;
        }
    }

    fn end_tag(&mut self, raw: &str, name: &str) {
        let pos = match self.ids.get(name) {
            Some(&id) if self.counts[id as usize] > 0 => self.find_open(|e| e.id == id),
            _ => None,
        };
        let Some(pos) = pos else {
            // stray end tag
            if INLINE.contains(&name) {
                self.inline(raw);
            } else {
                self.own_line(raw, self.depth);
            }
            return;
        };
        // implicitly close anything opened inside
        if pos + 1 < self.stack.len() {
            self.pop_to(pos + 1);
        }
        let Some(elem) = self.pop_elem() else { return };
        if !elem.block {
            self.pending_space = false;
            self.inline(raw);
            return;
        }
        let indents = elem.id != HTML_ID;
        if indents {
            self.depth = self.depth.saturating_sub(1);
        }
        if self.line_owner == Some(pos) {
            self.pending_space = false;
            self.line.push_str(raw);
            self.flush();
        } else {
            self.own_line(raw, self.depth);
        }
    }
}

enum Tok<'a> {
    Start {
        raw: &'a str,
        name: String,
        self_closing: bool,
    },
    End {
        raw: &'a str,
        name: String,
    },
    /// Comment, doctype, CDATA, processing instruction.
    Markup(&'a str),
}

fn tag_name(s: &[u8], mut i: usize) -> (String, usize) {
    let start = i;
    while i < s.len() && !s[i].is_ascii_whitespace() && !matches!(s[i], b'>' | b'/') {
        i += 1;
    }
    (
        String::from_utf8_lossy(&s[start..i]).to_ascii_lowercase(),
        i,
    )
}

/// End of a tag starting at `start` (index after `>`), honouring quoted attribute values.
fn tag_end(s: &[u8], start: usize) -> usize {
    let mut i = start;
    let mut quote: Option<u8> = None;
    while i < s.len() {
        let c = s[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => {
                // Only treat as a quote when it starts an attribute value (after '=').
                let mut j = i;
                while j > start && s[j - 1].is_ascii_whitespace() {
                    j -= 1;
                }
                if j > start && s[j - 1] == b'=' {
                    quote = Some(c);
                }
            }
            None if c == b'>' => return i + 1,
            None => {}
        }
        i += 1;
    }
    s.len()
}

/// Find `</name` (case-insensitive) followed by whitespace, `/` or `>`; returns its index.
fn find_close(s: &[u8], from: usize, name: &str) -> Option<usize> {
    let n = name.as_bytes();
    let mut search = from;
    while let Some(rel) = memchr::memmem::find(&s[search..], b"</") {
        let at = search + rel;
        let after = at + 2;
        if s.len() >= after + n.len() && s[after..after + n.len()].eq_ignore_ascii_case(n) {
            match s.get(after + n.len()) {
                None | Some(b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r' | 0x0c) => return Some(at),
                _ => {}
            }
        }
        search = at + 2;
    }
    None
}

fn attr(raw: &str, name: &str) -> Option<String> {
    // Minimal attribute lookup for `type` on script/style.
    let lower = raw.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut search = 0;
    while let Some(rel) = lower[search..].find(name) {
        let at = search + rel;
        let before_ok = at > 0 && b[at - 1].is_ascii_whitespace();
        let mut i = at + name.len();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if before_ok && b.get(i) == Some(&b'=') {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let (start, end) = match b.get(i) {
                Some(&q @ (b'"' | b'\'')) => {
                    let s = i + 1;
                    (s, lower[s..].find(q as char).map_or(b.len(), |e| s + e))
                }
                _ => {
                    let s = i;
                    let e = lower[s..]
                        .find(|c: char| c.is_ascii_whitespace() || c == '>')
                        .map_or(b.len(), |e| s + e);
                    (s, e)
                }
            };
            return Some(lower[start..end].trim().to_string());
        }
        search = at + name.len();
    }
    None
}

enum Embedded {
    Js,
    Json,
    Verbatim,
}

fn script_kind(raw_tag: &str) -> Embedded {
    match attr(raw_tag, "type")
        .as_deref()
        .map(|t| t.split(';').next().unwrap_or("").trim().to_string())
    {
        None => Embedded::Js,
        Some(t) => match t.as_str() {
            ""
            | "module"
            | "text/javascript"
            | "application/javascript"
            | "text/ecmascript"
            | "application/ecmascript"
            | "application/x-javascript"
            | "text/x-javascript"
            | "text/jscript"
            | "text/babel"
            | "jsx" => Embedded::Js,
            "importmap"
            | "speculationrules"
            | "application/json"
            | "application/ld+json"
            | "text/json" => Embedded::Json,
            t if t.ends_with("+json") => Embedded::Json,
            _ => Embedded::Verbatim,
        },
    }
}

pub(crate) fn pretty(text: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim().is_empty() {
        return None;
    }
    let s = text.as_bytes();
    let mut p = Printer::new(text.len() + text.len() / 4 + 64);
    let budget = crate::util::output_budget(text.len());
    let mut i = 0usize;
    let mut text_start = 0usize;
    while i < s.len() {
        if p.out.len() > budget {
            return None;
        }
        let Some(rel) = memchr::memchr(b'<', &s[i..]) else {
            break;
        };
        let lt = i + rel;
        let next = s.get(lt + 1).copied();
        let tok_end;
        let tok = if s[lt..].starts_with(b"<!--") {
            tok_end =
                memchr::memmem::find(&s[lt + 4..], b"-->").map_or(s.len(), |e| lt + 4 + e + 3);
            Tok::Markup(&text[lt..tok_end])
        } else if s[lt..].starts_with(b"<![CDATA[") {
            tok_end =
                memchr::memmem::find(&s[lt + 9..], b"]]>").map_or(s.len(), |e| lt + 9 + e + 3);
            Tok::Markup(&text[lt..tok_end])
        } else if matches!(next, Some(b'!' | b'?')) {
            tok_end = memchr::memchr(b'>', &s[lt..]).map_or(s.len(), |e| lt + e + 1);
            Tok::Markup(&text[lt..tok_end])
        } else if next == Some(b'/') && s.get(lt + 2).is_some_and(u8::is_ascii_alphabetic) {
            let (name, _) = tag_name(s, lt + 2);
            tok_end = tag_end(s, lt + 2);
            Tok::End {
                raw: &text[lt..tok_end],
                name,
            }
        } else if next.is_some_and(|c| c.is_ascii_alphabetic()) {
            let (name, after) = tag_name(s, lt + 1);
            tok_end = tag_end(s, after);
            let raw = &text[lt..tok_end];
            let self_closing = raw.ends_with("/>");
            Tok::Start {
                raw,
                name,
                self_closing,
            }
        } else {
            // a literal '<' in text
            i = lt + 1;
            continue;
        };
        if text_start < lt {
            p.text(&text[text_start..lt]);
        }
        i = tok_end;
        text_start = tok_end;
        match tok {
            Tok::Markup(m) => {
                let d = p.depth;
                p.own_line(m.trim_end(), d);
            }
            Tok::End { raw, name } => p.end_tag(raw, &name),
            Tok::Start {
                raw,
                name,
                self_closing,
            } => {
                p.implied_end(&name);
                let void = VOID.contains(&name.as_str());
                let inline = INLINE.contains(&name.as_str());
                if void || self_closing {
                    if inline {
                        p.inline(raw);
                    } else {
                        let d = p.depth;
                        p.own_line(raw, d);
                    }
                    continue;
                }
                let raw_text = name == "script" || name == "style";
                let verbatim = VERBATIM.contains(&name.as_str());
                if raw_text || verbatim {
                    let close = find_close(s, i, &name).unwrap_or(s.len());
                    let content = &text[i..close];
                    let end_tag_end = if close < s.len() {
                        tag_end(s, close + 2)
                    } else {
                        s.len()
                    };
                    let end_raw = &text[close..end_tag_end];
                    i = end_tag_end;
                    text_start = end_tag_end;
                    let d = p.depth;
                    if verbatim {
                        if name == "textarea" || name == "iframe" {
                            p.inline(raw);
                        } else {
                            p.flush();
                            p.line_depth = d;
                            p.line.push_str(raw);
                        }
                        p.line.push_str(content);
                        p.line.push_str(end_raw);
                        if name != "textarea" && name != "iframe" {
                            p.flush();
                        }
                        continue;
                    }
                    let formatted = if content.trim().is_empty() {
                        None
                    } else if name == "style" {
                        super::css::pretty(content, d + 1)
                    } else {
                        match script_kind(raw) {
                            Embedded::Js => super::js::pretty(content, d + 1),
                            Embedded::Json => {
                                super::json::pretty(content).map(|j| indent_lines(&j, d + 1))
                            }
                            Embedded::Verbatim => None,
                        }
                    };
                    p.flush();
                    p.line_depth = d;
                    p.line.push_str(raw);
                    match formatted {
                        Some(f) => {
                            p.flush();
                            p.out.push('\n');
                            p.out.push_str(&f);
                            p.own_line(end_raw, d);
                        }
                        None => {
                            if content.trim().is_empty() {
                                p.line.push_str(end_raw);
                            } else {
                                p.line.push_str(content);
                                p.line.push_str(end_raw);
                            }
                            p.flush();
                        }
                    }
                    continue;
                }
                if inline {
                    p.inline(raw);
                    p.push_elem(&name, false);
                } else {
                    let indents = name != "html";
                    p.start_block(raw, &name, indents);
                }
            }
        }
    }
    if text_start < s.len() {
        p.text(&text[text_start..]);
    }
    p.flush();
    (!p.out.is_empty() && p.out.len() <= budget).then_some(p.out)
}

fn indent_lines(s: &str, levels: usize) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 8);
    for (i, line) in s.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        push_indent(&mut out, levels);
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_structure() {
        let input = "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Test  page</title><link rel=stylesheet href=a.css></head><body><div class=\"a\"><p>Hello <b>world</b>!</p><ul><li>one<li>two</ul></div></body></html>";
        let expected = "<!DOCTYPE html>\n<html>\n<head>\n  <meta charset=\"utf-8\">\n  <title>Test page</title>\n  <link rel=stylesheet href=a.css>\n</head>\n<body>\n  <div class=\"a\">\n    <p>Hello <b>world</b>!</p>\n    <ul>\n      <li>one\n      <li>two\n    </ul>\n  </div>\n</body>\n</html>";
        assert_eq!(pretty(input).unwrap(), expected);
        assert_eq!(pretty(expected).unwrap(), expected);
    }

    #[test]
    fn script_style_pre_textarea() {
        let input = "<div><script>if(a){b()}</script><style>a{color:red}</style><pre>  keep\n   this  </pre><textarea> raw  <b> </textarea><script type=\"text/template\"><p>{{x}}</p></script><script src=\"x.js\"></script></div>";
        let expected = "<div>\n  <script>\n    if (a) {\n      b()\n    }\n  </script>\n  <style>\n    a {\n      color: red\n    }\n  </style>\n  <pre>  keep\n   this  </pre>\n  <textarea> raw  <b> </textarea>\n  <script type=\"text/template\"><p>{{x}}</p></script>\n  <script src=\"x.js\"></script>\n</div>";
        assert_eq!(pretty(input).unwrap(), expected);
    }

    #[test]
    fn json_script_and_comments() {
        let input = "<body><!-- note --><script type=\"application/ld+json\">{\"@type\":\"Thing\"}</script></body>";
        assert_eq!(
            pretty(input).unwrap(),
            "<body>\n  <!-- note -->\n  <script type=\"application/ld+json\">\n    {\n      \"@type\": \"Thing\"\n    }\n  </script>\n</body>"
        );
    }

    #[test]
    fn tables_and_implied_ends() {
        let input = "<table><tr><td>a<td>b<tr><td>c</table><p>one<p>two<div>x</div>";
        assert_eq!(
            pretty(input).unwrap(),
            "<table>\n  <tr>\n    <td>a\n    <td>b\n  <tr>\n    <td>c\n</table>\n<p>one\n<p>two\n<div>x</div>"
        );
    }

    #[test]
    fn attributes_with_gt_and_stray_tags() {
        let input = "<div data-x=\"a>b\" title='it''s'><span>x</span></i></div></div>";
        let out = pretty(input).unwrap();
        assert!(
            out.starts_with("<div data-x=\"a>b\" title='it''s'>"),
            "{out}"
        );
        assert!(out.contains("<span>x</span></i>"), "{out}");
        assert_eq!(pretty("a < b and c > d").unwrap(), "a < b and c > d");
        assert_eq!(pretty("  "), None);
        assert_eq!(
            pretty("<svg><path d=\"M0 0\"/><g><rect/></g></svg>").unwrap(),
            "<svg>\n  <path d=\"M0 0\"/>\n  <g>\n    <rect/>\n  </g>\n</svg>"
        );
    }

    #[test]
    fn unclosed_script_is_verbatim_to_end() {
        let out = pretty("<p>x</p><script>var a = '<div>'").unwrap();
        assert!(out.contains("var a = '<div>'"), "{out}");
    }

    #[test]
    fn deep_nesting_is_iterative() {
        let deep = "<div>".repeat(5_000) + &"</div>".repeat(5_000);
        assert!(pretty(&deep).is_some());
        // Very deep: no stack overflow; the output budget refuses the ~40× expansion.
        let deeper = "<div>".repeat(50_000) + &"</div>".repeat(50_000);
        assert_eq!(pretty(&deeper), None);
    }

    #[test]
    fn unclosed_and_stray_tag_soup_is_fast() {
        // Regression: each start tag scanned up to 512 open elements comparing strings, and
        // each stray end tag did the same.
        let start = std::time::Instant::now();
        let n = 100_000;
        for s in [
            "<div>".repeat(n),
            "<ul><li>".repeat(n),
            "<div>".repeat(n) + &"</span>".repeat(n),
            "<p>".to_string() + &"<span>".repeat(600) + &"<div>".repeat(n),
            // flat (no indentation growth): must format
            "<p>x</p><span>".repeat(n) + &"</i>".repeat(n),
        ] {
            let _ = pretty(&s);
        }
        let flat = "<br><p>a<li>b</li></p>".repeat(n);
        assert!(pretty(&flat).is_some());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(20),
            "took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn implied_ends_still_respect_scope() {
        // `<p>` inside a table cell isn't closed by a `<div>` outside the cell's scope rules.
        assert_eq!(
            pretty("<p>a<table><tr><td><div>x</div></td></tr></table>").unwrap(),
            "<p>a\n<table>\n  <tr>\n    <td>\n      <div>x</div>\n    </td>\n  </tr>\n</table>"
        );
        // Nested lists: an inner `<li>` doesn't close the outer one.
        assert_eq!(
            pretty("<ul><li>a<ul><li>b<li>c</ul><li>d</ul>").unwrap(),
            "<ul>\n  <li>a\n    <ul>\n      <li>b\n      <li>c\n    </ul>\n  <li>d\n</ul>"
        );
    }
}
