//! The `tls-profile` modifier action: JSON shape, stacking, and its decision field.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_core::http::{Headers, HttpVersion, RequestHead};
use akev_core::rules::{Action, Matcher, Rule, RuleItem, RuleSet, StringMatch};
use akev_core::settings::TlsProfile;
use akev_rules::{MatchContext, RulesEngine, describe};

fn head(host: &str, path: &str) -> RequestHead {
    RequestHead {
        method: "GET".into(),
        url: format!("https://{host}{path}"),
        scheme: "https".into(),
        host: host.into(),
        port: 443,
        path: path.into(),
        http_version: HttpVersion::Http11,
        headers: Headers(vec![("Host".into(), host.into())]),
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

fn engine(items: Vec<RuleItem>) -> RulesEngine {
    let e = RulesEngine::new(RuleSet { version: 1, items });
    assert!(e.errors().is_empty(), "{:?}", e.errors());
    e
}

fn decide(e: &RulesEngine, head: &RequestHead) -> akev_rules::Decision {
    e.evaluate(&MatchContext {
        head,
        body: None,
        source: None,
    })
}

#[test]
fn json_shape_round_trips() {
    let json = r#"{"type":"tls-profile","profile":"firefox"}"#;
    let action: Action = serde_json::from_str(json).unwrap();
    assert_eq!(
        action,
        Action::TlsProfile {
            profile: TlsProfile::Firefox
        }
    );
    assert_eq!(serde_json::to_string(&action).unwrap(), json);
    assert_eq!(action.type_name(), "tls-profile");
    assert!(!action.is_terminal(), "a modifier: later rules still run");

    // Missing / unknown profiles read as native instead of failing the whole rule file.
    let action: Action = serde_json::from_str(r#"{"type":"tls-profile"}"#).unwrap();
    assert_eq!(
        action,
        Action::TlsProfile {
            profile: TlsProfile::Native
        }
    );
    let action: Action =
        serde_json::from_str(r#"{"type":"tls-profile","profile":"netscape-4"}"#).unwrap();
    assert_eq!(
        action,
        Action::TlsProfile {
            profile: TlsProfile::Native
        }
    );
    let action: Action =
        serde_json::from_str(r#"{"type":"tls-profile","profile":"mirror-client"}"#).unwrap();
    assert_eq!(
        action,
        Action::TlsProfile {
            profile: TlsProfile::MirrorClient
        }
    );
}

#[test]
fn sets_the_decision_and_stacks_with_terminal_actions() {
    let e = engine(vec![
        rule(
            "chrome-for-shop",
            vec![Matcher::Host {
                pattern: StringMatch::equals("shop.example"),
            }],
            Action::TlsProfile {
                profile: TlsProfile::Chrome,
            },
        ),
        rule(
            "safari-for-api",
            vec![Matcher::Path {
                pattern: StringMatch::glob("/api/**"),
            }],
            Action::TlsProfile {
                profile: TlsProfile::Safari,
            },
        ),
        rule(
            "pass",
            vec![],
            Action::Passthrough {
                request: None,
                response: None,
            },
        ),
    ]);

    let d = decide(&e, &head("shop.example", "/"));
    assert_eq!(d.tls_profile, Some(TlsProfile::Chrome));
    assert_eq!(d.matched, vec!["chrome-for-shop", "pass"]);
    assert_eq!(d.terminal_rule.as_deref(), Some("pass"));
    assert!(!d.is_plain_passthrough());

    // Both match: the last tls-profile rule wins.
    let d = decide(&e, &head("shop.example", "/api/cart"));
    assert_eq!(d.tls_profile, Some(TlsProfile::Safari));

    let d = decide(&e, &head("other.example", "/"));
    assert_eq!(d.tls_profile, None);
}

#[test]
fn a_tls_profile_rule_alone_is_not_a_plain_passthrough() {
    let e = engine(vec![rule(
        "all",
        vec![],
        Action::TlsProfile {
            profile: TlsProfile::Edge,
        },
    )]);
    let d = decide(&e, &head("a.example", "/"));
    assert_eq!(d.tls_profile, Some(TlsProfile::Edge));
    assert_eq!(d.terminal_rule, None);
    assert!(!d.is_plain_passthrough());
    assert_eq!(e.stats()[0].matches, 1);
}

#[test]
fn describes_the_action() {
    let describe = |profile| describe::action(&Action::TlsProfile { profile });
    assert_eq!(
        describe(TlsProfile::Chrome),
        "impersonate Chrome upstream (TLS + HTTP/2)"
    );
    assert_eq!(
        describe(TlsProfile::Native),
        "use Akev's native TLS fingerprint upstream"
    );
    assert_eq!(
        describe(TlsProfile::MirrorClient),
        "mirror the client's browser TLS fingerprint upstream"
    );
}
