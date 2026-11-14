//! `ws-breakpoint` rules: which messages of a WebSocket are paused.
//!
//! A `ws-breakpoint` rule is a modifier that only matches WebSocket upgrade requests (its
//! matchers and `maxMatches` apply to the upgrade, once per connection). The ids of the matched
//! rules end up in [`crate::Decision::ws_breakpoints`]; the proxy keeps them for the lifetime of
//! the connection and asks [`crate::RulesEngine::ws_message_breakpoint`] about every complete
//! text/binary message. That lookup goes through the *current* rule set, so disabling, deleting
//! or editing the rule takes effect on connections that are already open.

use crate::matchers::{PatOpts, Pattern};
use akev_core::rules::{StringMatch, WsBreakpointDirection};
use akev_core::{Direction, MessageKind};

/// The compiled message filter of a `ws-breakpoint` rule.
#[derive(Debug)]
pub struct WsMessageFilter {
    direction: WsBreakpointDirection,
    pattern: Option<Pattern>,
}

impl WsMessageFilter {
    pub(crate) fn compile(
        direction: WsBreakpointDirection,
        pattern: Option<&StringMatch>,
    ) -> Result<Self, String> {
        let pattern = pattern
            .map(|p| Pattern::compile(p, PatOpts::TEXT))
            .transpose()?;
        Ok(WsMessageFilter { direction, pattern })
    }

    /// Whether messages travelling in `direction` can be paused by this filter (before looking
    /// at their payload).
    pub fn applies_to(&self, direction: Direction) -> bool {
        matches!(
            (self.direction, direction),
            (WsBreakpointDirection::Both, _)
                | (WsBreakpointDirection::Sent, Direction::Sent)
                | (WsBreakpointDirection::Received, Direction::Received)
        )
    }

    /// Whether a complete message is paused: text and binary messages in a matching direction
    /// whose payload matches the pattern (if any). The pattern sees the payload as UTF-8, binary
    /// payloads decoded lossily. Control frames (ping, pong, close) are never paused.
    pub fn matches(&self, direction: Direction, kind: MessageKind, payload: &[u8]) -> bool {
        if !matches!(kind, MessageKind::Text | MessageKind::Binary) || !self.applies_to(direction) {
            return false;
        }
        match &self.pattern {
            None => true,
            Some(p) => p.eval(Some(&String::from_utf8_lossy(payload))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::rules::MatchOp;

    fn filter(direction: WsBreakpointDirection, pattern: Option<StringMatch>) -> WsMessageFilter {
        WsMessageFilter::compile(direction, pattern.as_ref()).unwrap()
    }

    #[test]
    fn direction_and_kind() {
        let both = filter(WsBreakpointDirection::Both, None);
        assert!(both.matches(Direction::Sent, MessageKind::Text, b"x"));
        assert!(both.matches(Direction::Received, MessageKind::Binary, &[0xff]));
        assert!(both.matches(Direction::Sent, MessageKind::Text, b""));
        for kind in [MessageKind::Ping, MessageKind::Pong, MessageKind::Close] {
            assert!(!both.matches(Direction::Sent, kind, b"x"), "{kind:?}");
        }
        let sent = filter(WsBreakpointDirection::Sent, None);
        assert!(sent.applies_to(Direction::Sent) && !sent.applies_to(Direction::Received));
        assert!(!sent.matches(Direction::Received, MessageKind::Text, b"x"));
        let received = filter(WsBreakpointDirection::Received, None);
        assert!(received.matches(Direction::Received, MessageKind::Text, b"x"));
        assert!(!received.matches(Direction::Sent, MessageKind::Text, b"x"));
    }

    #[test]
    fn payload_patterns() {
        let contains = filter(
            WsBreakpointDirection::Both,
            Some(StringMatch::contains("\"type\":\"auth\"")),
        );
        assert!(contains.matches(
            Direction::Sent,
            MessageKind::Text,
            br#"{"TYPE":"AUTH","token":"x"}"#
        ));
        assert!(!contains.matches(Direction::Sent, MessageKind::Text, br#"{"type":"ping"}"#));
        // Binary payloads are matched lossily as text.
        assert!(contains.matches(
            Direction::Received,
            MessageKind::Binary,
            b"\x00\x01{\"type\":\"auth\"}\xff"
        ));
        let regex = filter(
            WsBreakpointDirection::Both,
            Some(StringMatch::regex(r#"^\d+\["chat""#)),
        );
        assert!(regex.matches(Direction::Sent, MessageKind::Text, br#"42["chat",{"m":1}]"#));
        assert!(!regex.matches(Direction::Sent, MessageKind::Text, br#"42["typing"]"#));
        let negated = filter(
            WsBreakpointDirection::Both,
            Some(StringMatch {
                op: MatchOp::Equals,
                value: "2".into(),
                case_sensitive: true,
                negate: true,
            }),
        );
        assert!(!negated.matches(Direction::Sent, MessageKind::Text, b"2"));
        assert!(negated.matches(Direction::Sent, MessageKind::Text, b"42[]"));
    }

    // ---- engine integration ------------------------------------------------------------

    use crate::{MatchContext, RulesEngine};
    use akev_core::rules::{Action, Matcher, Rule, RuleItem, RuleSet};
    use akev_core::{Headers, HttpVersion, RequestHead};

    fn head(path: &str, websocket: bool) -> RequestHead {
        let mut headers = Headers::new();
        headers.append("Host", "chat.test");
        if websocket {
            headers.append("Upgrade", "websocket");
            headers.append("Connection", "Upgrade");
        }
        RequestHead {
            method: "GET".into(),
            url: format!("http://chat.test{path}"),
            scheme: "http".into(),
            host: "chat.test".into(),
            port: 80,
            path: path.into(),
            http_version: HttpVersion::Http11,
            headers,
        }
    }

    fn rule(id: &str, matchers: Vec<Matcher>, action: Action) -> RuleItem {
        RuleItem::Rule(Rule {
            id: id.into(),
            title: None,
            enabled: true,
            matchers,
            action,
            max_matches: None,
            note: None,
        })
    }

    fn ws_break(direction: WsBreakpointDirection, pattern: Option<StringMatch>) -> Action {
        Action::WsBreakpoint { direction, pattern }
    }

    fn rules(items: Vec<RuleItem>) -> RuleSet {
        RuleSet { version: 1, items }
    }

    fn decide(e: &RulesEngine, head: &RequestHead) -> crate::Decision {
        e.evaluate(&MatchContext {
            head,
            body: None,
            source: None,
        })
    }

    #[test]
    fn ws_breakpoint_rules_only_match_websocket_upgrades() {
        let e = RulesEngine::new(rules(vec![rule(
            "wsb",
            vec![Matcher::Path {
                pattern: StringMatch::equals("/socket"),
            }],
            ws_break(WsBreakpointDirection::Both, None),
        )]));
        assert!(e.errors().is_empty());
        let plain = decide(&e, &head("/socket", false));
        assert!(plain.matched.is_empty() && plain.ws_breakpoints.is_empty());
        assert!(plain.is_plain_passthrough());
        let ws = decide(&e, &head("/socket", true));
        assert_eq!(ws.ws_breakpoints, vec!["wsb".to_string()]);
        assert_eq!(ws.matched, vec!["wsb".to_string()]);
        assert_eq!(ws.terminal_rule, None, "a modifier, not a terminal action");
        assert!(!ws.is_plain_passthrough());
        assert!(decide(&e, &head("/other", true)).ws_breakpoints.is_empty());
        // Counted once per connection (upgrade), not per message.
        assert_eq!(e.stats()[0].matches, 1);
    }

    #[test]
    fn ws_breakpoints_stack_and_stop_at_terminal_rules() {
        let e = RulesEngine::new(rules(vec![
            rule("a", vec![], ws_break(WsBreakpointDirection::Sent, None)),
            rule("b", vec![], ws_break(WsBreakpointDirection::Received, None)),
            rule("stop", vec![], Action::WsEcho),
            rule("never", vec![], ws_break(WsBreakpointDirection::Both, None)),
        ]));
        let d = decide(&e, &head("/", true));
        assert_eq!(d.ws_breakpoints, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(d.terminal_rule.as_deref(), Some("stop"));
    }

    #[test]
    fn message_checks_follow_the_live_rule_set() {
        let e = RulesEngine::new(rules(vec![rule(
            "wsb",
            vec![],
            ws_break(
                WsBreakpointDirection::Sent,
                Some(StringMatch::contains("secret")),
            ),
        )]));
        let ids = decide(&e, &head("/", true)).ws_breakpoints;
        assert!(e.ws_may_break(&ids, Direction::Sent));
        assert!(!e.ws_may_break(&ids, Direction::Received));
        assert_eq!(
            e.ws_message_breakpoint(&ids, Direction::Sent, MessageKind::Text, b"my secret"),
            Some("wsb".to_string())
        );
        assert_eq!(
            e.ws_message_breakpoint(&ids, Direction::Sent, MessageKind::Text, b"hello"),
            None
        );
        assert_eq!(
            e.ws_message_breakpoint(&ids, Direction::Received, MessageKind::Text, b"secret"),
            None
        );
        // Editing the rule applies to open connections (same rule id).
        e.set_rules(rules(vec![rule(
            "wsb",
            vec![],
            ws_break(WsBreakpointDirection::Both, None),
        )]));
        assert!(e.ws_may_break(&ids, Direction::Received));
        assert_eq!(
            e.ws_message_breakpoint(&ids, Direction::Received, MessageKind::Binary, b"x"),
            Some("wsb".to_string())
        );
        // Disabling (or turning it into another action) stops pausing.
        let mut disabled = rules(vec![rule(
            "wsb",
            vec![],
            ws_break(WsBreakpointDirection::Both, None),
        )]);
        if let RuleItem::Rule(r) = &mut disabled.items[0] {
            r.enabled = false;
        }
        e.set_rules(disabled);
        assert!(!e.ws_may_break(&ids, Direction::Sent));
        assert_eq!(
            e.ws_message_breakpoint(&ids, Direction::Sent, MessageKind::Text, b"x"),
            None
        );
        e.set_rules(rules(vec![rule("wsb", vec![], Action::Close)]));
        assert!(!e.ws_may_break(&ids, Direction::Sent));
    }

    #[test]
    fn invalid_pattern_is_reported_and_the_rule_skipped() {
        let e = RulesEngine::new(rules(vec![rule(
            "bad",
            vec![],
            ws_break(WsBreakpointDirection::Both, Some(StringMatch::regex("("))),
        )]));
        assert_eq!(e.errors().len(), 1);
        assert_eq!(e.errors()[0].0, "bad");
        assert!(decide(&e, &head("/", true)).ws_breakpoints.is_empty());
    }

    #[test]
    fn summaries() {
        let describe = |a: Action| crate::describe::action(&a);
        assert_eq!(
            describe(ws_break(WsBreakpointDirection::Both, None)),
            "pause WebSocket messages"
        );
        assert_eq!(
            describe(ws_break(
                WsBreakpointDirection::Sent,
                Some(StringMatch::contains("auth"))
            )),
            "pause sent WebSocket messages containing \"auth\""
        );
        assert_eq!(
            describe(ws_break(
                WsBreakpointDirection::Received,
                Some(StringMatch::regex("^4"))
            )),
            "pause received WebSocket messages matching /^4/"
        );
    }

    #[test]
    fn invalid_regex_is_an_error() {
        assert!(
            WsMessageFilter::compile(WsBreakpointDirection::Both, Some(&StringMatch::regex("(")))
                .is_err()
        );
    }
}
