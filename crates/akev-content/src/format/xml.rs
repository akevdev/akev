//! XML pretty-printing on top of the `quick-xml` event reader.
//!
//! Every markup event is copied verbatim from the source (attributes, namespaces, entity
//! references, CDATA sections, comments, processing instructions and the doctype keep their
//! exact spelling); only the whitespace between them changes. Elements that contain only text
//! stay on one line (`<a>text</a>`); whitespace-only text nodes are dropped; other text is
//! trimmed and placed on its own line. Parsing is lenient (mismatched end tags are accepted);
//! if the reader hits a syntax error, the unparsed remainder is appended verbatim so nothing is
//! ever lost.

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::util::push_indent;

#[derive(PartialEq, Clone, Copy)]
enum Last {
    /// Nothing written yet.
    Nothing,
    /// A start tag was just written (no children yet).
    Open,
    /// A start tag followed by inline text.
    InlineText,
    /// Anything else.
    Other,
}

pub(crate) fn pretty(text: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim().is_empty() {
        return None;
    }
    let mut reader = Reader::from_str(text);
    {
        let cfg = reader.config_mut();
        cfg.trim_text(false);
        cfg.check_end_names = false;
        cfg.allow_unmatched_ends = true;
        cfg.check_comments = false;
        cfg.expand_empty_elements = false;
    }
    let budget = crate::util::output_budget(text.len());
    let mut out = String::with_capacity(text.len() + text.len() / 4 + 16);
    let mut depth = 0usize;
    let mut last = Last::Nothing;
    let mut start = 0usize;

    let newline = |out: &mut String, depth: usize, last: Last| {
        if last != Last::Nothing {
            out.push('\n');
        }
        push_indent(out, depth);
    };

    // Start of the run of text, references and CDATA being collected: quick-xml reports
    // `a &amp; b` as three events, laid out as one.
    let mut text_from: Option<usize> = None;
    loop {
        let event = match reader.read_event() {
            Ok(e) => e,
            Err(_) => {
                let rest = text.get(text_from.unwrap_or(start)..).unwrap_or("").trim();
                if !rest.is_empty() {
                    newline(&mut out, depth, last);
                    out.push_str(rest);
                }
                break;
            }
        };
        let end = (reader.buffer_position() as usize).min(text.len());
        if matches!(
            event,
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_)
        ) {
            text_from.get_or_insert(start);
            start = end;
            continue;
        }
        if let Some(from) = text_from.take() {
            let t = text.get(from..start).unwrap_or("").trim();
            if !t.is_empty() {
                if last == Last::Open {
                    out.push_str(t);
                    last = Last::InlineText;
                } else {
                    newline(&mut out, depth, last);
                    out.push_str(t);
                    last = Last::Other;
                }
            }
        }
        let raw = text.get(start..end).unwrap_or("");
        match event {
            Event::Start(_) => {
                newline(&mut out, depth, last);
                out.push_str(raw.trim());
                depth += 1;
                last = Last::Open;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if !matches!(last, Last::Open | Last::InlineText) {
                    newline(&mut out, depth, last);
                }
                out.push_str(raw.trim());
                last = Last::Other;
            }
            // Collected above.
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_) => {}
            Event::Empty(_)
            | Event::Comment(_)
            | Event::Decl(_)
            | Event::PI(_)
            | Event::DocType(_) => {
                newline(&mut out, depth, last);
                out.push_str(raw.trim());
                last = Last::Other;
            }
            Event::Eof => break,
        }
        start = end;
        if out.len() > budget {
            return None;
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_layout() {
        let input = r#"<?xml version="1.0" encoding="UTF-8"?><root a="1"><child>text</child><empty/><nested><x y='2'>v</x></nested><e></e></root>"#;
        let expected = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root a=\"1\">\n  <child>text</child>\n  <empty/>\n  <nested>\n    <x y='2'>v</x>\n  </nested>\n  <e></e>\n</root>";
        assert_eq!(pretty(input).unwrap(), expected);
        assert_eq!(pretty(expected).unwrap(), expected);
    }

    #[test]
    fn cdata_comments_pis_namespaces() {
        let input = "<!DOCTYPE note SYSTEM \"Note.dtd\"><soap:Envelope xmlns:soap=\"http://www.w3.org/2003/05/soap-envelope\"><!-- a comment --><?pi target?><soap:Body><m:Get xmlns:m=\"urn:x\"><![CDATA[<raw> & stuff]]></m:Get><s>a &amp; b</s></soap:Body></soap:Envelope>";
        let out = pretty(input).unwrap();
        assert_eq!(
            out,
            "<!DOCTYPE note SYSTEM \"Note.dtd\">\n<soap:Envelope xmlns:soap=\"http://www.w3.org/2003/05/soap-envelope\">\n  <!-- a comment -->\n  <?pi target?>\n  <soap:Body>\n    <m:Get xmlns:m=\"urn:x\"><![CDATA[<raw> & stuff]]></m:Get>\n    <s>a &amp; b</s>\n  </soap:Body>\n</soap:Envelope>"
        );
    }

    #[test]
    fn mixed_content_and_whitespace() {
        let input = "<p>\n   hello <b>world</b> again\n</p>";
        assert_eq!(
            pretty(input).unwrap(),
            "<p>hello\n  <b>world</b>\n  again\n</p>"
        );
        assert_eq!(
            pretty("<a>\n\n  <b/>\n\n</a>").unwrap(),
            "<a>\n  <b/>\n</a>"
        );
    }

    #[test]
    fn lenient_and_lossless_on_errors() {
        // mismatched end tag accepted
        assert_eq!(pretty("<a><b></c></a>").unwrap(), "<a>\n  <b></c>\n</a>");
        // unclosed tag at the end: remainder appended verbatim
        let out = pretty("<a><b>text</b><c attr=\"x").unwrap();
        assert!(out.ends_with("<c attr=\"x"), "{out}");
        // plain text
        assert_eq!(pretty("just text").unwrap(), "just text");
        assert_eq!(pretty("   "), None);
        assert_eq!(pretty("\u{feff}<a/>").unwrap(), "<a/>");
    }

    #[test]
    fn deep_nesting_is_iterative() {
        let deep = "<a>".repeat(10_000) + &"</a>".repeat(10_000);
        let out = pretty(&deep).unwrap();
        assert!(out.starts_with("<a>\n  <a>"));
        // Very deep: no stack overflow (the output budget may refuse it).
        let deeper = "<a>".repeat(100_000) + &"</a>".repeat(100_000);
        let _ = pretty(&deeper);
        // Output is bounded: a 3 MB pile of unclosed tags would expand ~70×.
        assert_eq!(pretty(&"<a>".repeat(1 << 20)), None);
    }
}
