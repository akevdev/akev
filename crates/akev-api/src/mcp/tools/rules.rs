//! Tools for the rule set: read, replace, add a mock, test.

use super::*;

pub(super) fn get_rules(app: &Arc<App>) -> ToolResult {
    Ok(Out::Json(json!({
        "rules": app.rules.rules(),
        "stats": app.rules.stats(),
        "errors": app.rules.errors(),
    })))
}

pub(super) async fn set_rules(app: &Arc<App>, args: &Value) -> ToolResult {
    let raw = args.get("rules").ok_or("`rules` is required")?;
    let rules: RuleSet = serde_json::from_value(raw.clone()).map_err(|e| {
        format!(
            "invalid rule set: {e}. Fetch the current set with akev_get_rules for the exact format."
        )
    })?;
    let count = rules.walk().len();
    app.mcp_files.check_rules(&rules, &app.rules.rules())?;
    let errors = app.set_rules_blocking(rules).await.map_err(|e| e.message)?;
    Ok(Out::Json(
        json!({ "applied": true, "rules": count, "errors": errors }),
    ))
}

pub(super) fn url_pattern(pattern: &str, mode: &str) -> Result<StringMatch, String> {
    let (op, value) = match mode {
        "contains" => (MatchOp::Contains, pattern.to_string()),
        "equals" | "exact" => (MatchOp::Equals, pattern.to_string()),
        "starts-with" | "prefix" => (MatchOp::StartsWith, pattern.to_string()),
        "glob" => (MatchOp::Glob, pattern.to_string()),
        "regex" => (MatchOp::Regex, pattern.to_string()),
        "auto" => {
            if pattern.len() > 2 && pattern.starts_with('/') && pattern.ends_with('/') {
                (MatchOp::Regex, pattern[1..pattern.len() - 1].to_string())
            } else if pattern.contains('*') {
                (MatchOp::Glob, pattern.to_string())
            } else {
                (MatchOp::Contains, pattern.to_string())
            }
        }
        other => return Err(format!("unknown match mode {other:?}")),
    };
    if value.is_empty() {
        return Err("`urlPattern` must not be empty".into());
    }
    Ok(StringMatch {
        op,
        value,
        case_sensitive: op == MatchOp::Regex,
        negate: false,
    })
}

/// Build the rule for `akev_add_mock`.
pub fn build_mock_rule(args: &Value) -> Result<Rule, String> {
    let pattern = arg_str(args, "urlPattern")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or("`urlPattern` is required")?;
    let mode = arg_str(args, "match").unwrap_or("auto");
    let mut matchers = vec![Matcher::Url {
        pattern: url_pattern(pattern, mode)?,
    }];
    if let Some(method) = arg_str(args, "method")
        .map(str::trim)
        .filter(|m| !m.is_empty() && *m != "*")
    {
        matchers.insert(
            0,
            Matcher::Method {
                methods: vec![method.to_ascii_uppercase()],
            },
        );
    }
    let status = arg_u64(args, "status", 200, 999)?;
    if status < 100 {
        return Err("`status` must be between 100 and 999".into());
    }
    let headers = arg_headers(args, "headers")?.unwrap_or_default();
    let body = match args.get("body") {
        None | Some(Value::Null) => BodySource::Empty,
        Some(Value::String(s)) => BodySource::Text { text: s.clone() },
        Some(other) => BodySource::Json {
            value: other.clone(),
        },
    };
    let max_matches = match args.get("times") {
        None | Some(Value::Null) => None,
        Some(_) => Some(arg_u64(args, "times", 1, u32::MAX as u64)?.max(1) as u32),
    };
    let title = arg_str(args, "title")
        .map(str::to_string)
        .or_else(|| Some(format!("Mock {status} for {pattern}")));
    Ok(Rule {
        id: akev_core::new_uuid(),
        title,
        enabled: true,
        matchers,
        action: Action::Mock {
            status: status as u16,
            status_message: None,
            headers,
            body,
        },
        max_matches,
        note: Some("Added by an AI agent via MCP".into()),
    })
}

pub(super) async fn add_mock(app: &Arc<App>, args: &Value) -> ToolResult {
    let rule = build_mock_rule(args)?;
    let id = rule.id.clone();
    let rule_json = to_value(&rule);
    let errors: Vec<_> = app
        .update_rules_blocking(move |rules| rules.items.insert(0, RuleItem::Rule(rule)))
        .await
        .map_err(|e| e.message)?
        .into_iter()
        .filter(|(rid, _)| *rid == id)
        .collect();
    if !errors.is_empty() {
        return Err(format!(
            "the mock rule was added but failed to compile: {errors:?}"
        ));
    }
    Ok(Out::Json(json!({
        "ruleId": id,
        "rule": rule_json,
        "note": "The mock is active now (top priority). Remove it with akev_set_rules.",
    })))
}

pub(super) async fn test_rules(app: &Arc<App>, args: &Value) -> ToolResult {
    let mut body = crate::routes::rules::TestBody::default();
    if args.get("exchangeId").is_some() {
        body.exchange_id = Some(arg_id(args, "exchangeId")?);
    } else if let Some(r) = args.get("request") {
        let url = arg_str(r, "url")
            .ok_or("`request.url` is required")?
            .to_string();
        body.request = Some(EditableRequest {
            method: arg_str(r, "method").unwrap_or("GET").to_ascii_uppercase(),
            url,
            headers: arg_headers(r, "headers")?.unwrap_or_default(),
            body: Bytes::from(arg_str(r, "body").unwrap_or("").to_string()),
        });
    } else {
        return Err("provide `exchangeId` or `request`".into());
    }
    if let Some(rules) = args.get("rules").filter(|v| !v.is_null()) {
        let rules: RuleSet =
            serde_json::from_value(rules.clone()).map_err(|e| format!("invalid rule set: {e}"))?;
        app.mcp_files.check_rules(&rules, &app.rules.rules())?;
        body.rules = Some(rules);
    }
    let out = crate::routes::rules::dry_run(app, body)
        .await
        .map_err(|e| e.message)?;
    Ok(Out::Json(to_value(&out)))
}
