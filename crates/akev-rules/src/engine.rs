//! Rule compilation, match counters and decision assembly.

use crate::matchers::{CMatcher, ReqView, Tri, compile_sorted, eval_all};
use crate::transform::validate_transforms;
use crate::ws::WsMessageFilter;
use crate::{Decision, MatchContext, Throttle};
use akev_core::rules::{Action, Rule, RuleSet, RuleStats};
use akev_core::{RuleId, UnixMicros};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-rule statistics, shared between successive snapshots while the rule id persists.
#[derive(Debug, Default)]
pub(crate) struct Counter {
    matches: AtomicU64,
    /// Unix microseconds of the last match (0 = never).
    last_matched_us: AtomicU64,
}

impl Counter {
    /// Record a match. With a budget, the hit is reserved atomically and `false` is returned
    /// when the budget is already exhausted (the rule must then be skipped).
    fn try_hit(&self, max: Option<u64>) -> bool {
        let reserved = match max {
            None => {
                self.matches.fetch_add(1, Ordering::Relaxed);
                true
            }
            Some(max) => {
                let mut current = self.matches.load(Ordering::Acquire);
                loop {
                    if current >= max {
                        break false;
                    }
                    match self.matches.compare_exchange_weak(
                        current,
                        current + 1,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => break true,
                        Err(actual) => current = actual,
                    }
                }
            }
        };
        if reserved {
            let now = u64::try_from(UnixMicros::now().0).unwrap_or(0);
            self.last_matched_us.fetch_max(now, Ordering::Relaxed);
        }
        reserved
    }

    fn exhausted(&self, max: Option<u64>) -> bool {
        max.is_some_and(|max| self.matches.load(Ordering::Acquire) >= max)
    }

    fn stats(&self, rule_id: &str) -> RuleStats {
        let last = self.last_matched_us.load(Ordering::Relaxed);
        RuleStats {
            rule_id: rule_id.to_string(),
            matches: self.matches.load(Ordering::Relaxed),
            last_matched_at: (last != 0).then(|| last as f64 / 1000.0),
        }
    }

    fn reset(&self) {
        self.matches.store(0, Ordering::Release);
        self.last_matched_us.store(0, Ordering::Relaxed);
    }
}

/// Action types (`Action::type_name`) that may not run, with the message reported for
/// enabled rules that use them (see [`crate::RulesEngine::set_locked_actions`]).
pub type LockedActions = HashMap<&'static str, String>;

/// A rule ready for evaluation.
pub(crate) struct CompiledRule {
    pub id: RuleId,
    /// Matchers ordered cheapest-first.
    matchers: Vec<CMatcher>,
    action: Action,
    terminal: bool,
    max_matches: Option<u64>,
    counter: Arc<Counter>,
    /// `ws-breakpoint` rules: the message filter (such rules only match WebSocket upgrades).
    ws: Option<Arc<WsMessageFilter>>,
}

impl CompiledRule {
    fn eval(&self, r: &ReqView<'_>) -> Tri {
        if self.ws.is_some() && !r.protocol().starts_with("ws") {
            return Tri::False;
        }
        eval_all(&self.matchers, r)
    }
}

/// An immutable compiled rule set.
pub(crate) struct Snapshot {
    pub rules: RuleSet,
    /// Enabled, valid rules (in evaluation order).
    compiled: Vec<CompiledRule>,
    pub errors: Vec<(RuleId, String)>,
    /// Counters of every rule in the set (walk order), for stats.
    counters: Vec<(RuleId, Arc<Counter>)>,
    /// Whether any enabled rule has a body-dependent matcher.
    any_body_matcher: bool,
    /// Message filters of the enabled `ws-breakpoint` rules, by rule id.
    ws_filters: HashMap<RuleId, Arc<WsMessageFilter>>,
}

impl Snapshot {
    /// Compile `rules`, reusing the counters of `previous` for persisting rule ids.
    ///
    /// Enabled rules whose action type is in `locked` are not compiled (a script's top-level
    /// code doesn't run) and are reported with the lock's message instead; the rule set
    /// itself is kept as given.
    pub(crate) fn compile(
        rules: RuleSet,
        previous: Option<&Snapshot>,
        locked: &LockedActions,
    ) -> Snapshot {
        let old: HashMap<&str, &Arc<Counter>> = previous
            .map(|p| p.counters.iter().map(|(id, c)| (id.as_str(), c)).collect())
            .unwrap_or_default();
        let mut compiled = Vec::new();
        let mut errors = Vec::new();
        let mut counters = Vec::new();
        let mut seen = HashSet::new();
        let mut any_body_matcher = false;
        let mut ws_filters = HashMap::new();
        for (rule, parents_enabled) in rules.walk() {
            if !seen.insert(rule.id.as_str()) {
                errors.push((
                    rule.id.clone(),
                    "duplicate rule id (rule skipped)".to_string(),
                ));
                continue;
            }
            let counter = old
                .get(rule.id.as_str())
                .map(|c| Arc::clone(c))
                .unwrap_or_default();
            counters.push((rule.id.clone(), Arc::clone(&counter)));
            if let Some(message) = locked.get(rule.action.type_name()) {
                if rule.enabled && parents_enabled {
                    errors.push((rule.id.clone(), message.clone()));
                }
                continue;
            }
            match compile_rule(rule, counter) {
                Ok(c) => {
                    if rule.enabled && parents_enabled {
                        any_body_matcher |= c.matchers.iter().any(CMatcher::needs_body);
                        if let Some(ws) = &c.ws {
                            ws_filters.insert(rule.id.clone(), Arc::clone(ws));
                        }
                        compiled.push(c);
                    }
                }
                Err(message) => errors.push((rule.id.clone(), message)),
            }
        }
        for (rule, message) in &errors {
            tracing::warn!(rule = %rule, error = %message, "invalid rule skipped");
        }
        tracing::debug!(
            rules = counters.len(),
            active = compiled.len(),
            invalid = errors.len(),
            "rule set compiled"
        );
        Snapshot {
            rules,
            compiled,
            errors,
            counters,
            any_body_matcher,
            ws_filters,
        }
    }

    /// The message filter of an enabled `ws-breakpoint` rule of this set.
    pub(crate) fn ws_filter(&self, rule: &str) -> Option<&Arc<WsMessageFilter>> {
        self.ws_filters.get(rule)
    }

    pub(crate) fn stats(&self) -> Vec<RuleStats> {
        self.counters.iter().map(|(id, c)| c.stats(id)).collect()
    }

    pub(crate) fn reset_stats(&self) {
        self.counters.iter().for_each(|(_, c)| c.reset());
    }

    /// See [`crate::RulesEngine::needs_body`].
    pub(crate) fn needs_body(&self, ctx: &MatchContext<'_>) -> bool {
        if !self.any_body_matcher {
            return false;
        }
        let view = ReqView::new(ctx.head, None, ctx.source);
        for rule in &self.compiled {
            if rule.counter.exhausted(rule.max_matches) {
                continue;
            }
            match rule.eval(&view) {
                Tri::False => {}
                // A terminal rule that definitely matches ends evaluation: later rules can't
                // need the body. Unless it has a `maxMatches` budget: concurrent requests may
                // use that up before `evaluate` runs, and evaluation then goes on to later
                // rules, which must not be missing the body they need.
                Tri::True if rule.terminal && rule.max_matches.is_none() => return false,
                Tri::True => {}
                Tri::Unknown => return true,
            }
        }
        false
    }

    /// Evaluate the rules; with `side_effects`, counters and budgets are updated.
    pub(crate) fn evaluate(&self, ctx: &MatchContext<'_>, side_effects: bool) -> Decision {
        let mut decision = Decision::passthrough();
        if self.compiled.is_empty() {
            return decision;
        }
        let view = ReqView::new(ctx.head, ctx.body, ctx.source);
        for rule in &self.compiled {
            if rule.counter.exhausted(rule.max_matches) || rule.eval(&view) != Tri::True {
                continue;
            }
            if side_effects && !rule.counter.try_hit(rule.max_matches) {
                // Lost the race for the last unit of budget.
                continue;
            }
            decision.matched.push(rule.id.clone());
            apply_action(&mut decision, rule);
            if rule.terminal {
                break;
            }
        }
        decision
    }
}

fn compile_rule(rule: &Rule, counter: Arc<Counter>) -> Result<CompiledRule, String> {
    let matchers = compile_sorted(&rule.matchers)?;
    validate_action(&rule.action)?;
    let ws = match &rule.action {
        Action::WsBreakpoint { direction, pattern } => Some(Arc::new(WsMessageFilter::compile(
            *direction,
            pattern.as_ref(),
        )?)),
        _ => None,
    };
    Ok(CompiledRule {
        id: rule.id.clone(),
        matchers,
        action: rule.action.clone(),
        terminal: rule.action.is_terminal(),
        max_matches: rule.max_matches.map(u64::from),
        counter,
        ws,
    })
}

/// Validate everything about an action that can be checked ahead of time (regexes, JSON
/// patches, header names, forward targets, script syntax...).
fn validate_action(action: &Action) -> Result<(), String> {
    match action {
        Action::Passthrough { request, response } | Action::Transform { request, response } => {
            validate_transforms(request.as_ref(), response.as_ref())
        }
        Action::Forward {
            target,
            request,
            response,
            ..
        } => {
            crate::respond::parse_forward_target(target)?;
            validate_transforms(request.as_ref(), response.as_ref())
        }
        Action::Mock {
            status,
            status_message,
            headers,
            body,
        } => {
            validate_status(*status)?;
            if let Some(m) = status_message {
                crate::transform::validate_header_value("status message", m)?;
            }
            crate::respond::validate_body_source(body)?;
            crate::transform::validate_headers(headers)
        }
        Action::MapLocal { path, headers, .. } => {
            if path.trim().is_empty() {
                return Err("map-local path is empty".to_string());
            }
            crate::transform::validate_headers(headers)
        }
        Action::WsReject { status, body } => {
            validate_status(*status)?;
            crate::respond::validate_body_source(body)
        }
        Action::Highlight { color, .. } => {
            if color.trim().is_empty() {
                Err("highlight colour is empty".to_string())
            } else {
                Ok(())
            }
        }
        Action::Script { source } => crate::script::check_syntax(source),
        // The message filter is compiled (and its pattern checked) by `compile_rule`.
        Action::WsBreakpoint { .. } => Ok(()),
        Action::Close
        | Action::Reset
        | Action::Timeout
        | Action::WsEcho
        | Action::Breakpoint { .. }
        | Action::Delay { .. }
        | Action::Throttle { .. }
        | Action::TlsProfile { .. } => Ok(()),
    }
}

fn validate_status(status: u16) -> Result<(), String> {
    if (100..=999).contains(&status) {
        Ok(())
    } else {
        Err(format!("invalid HTTP status {status}"))
    }
}

/// Fold one matched rule's action into the decision.
fn apply_action(d: &mut Decision, rule: &CompiledRule) {
    match &rule.action {
        Action::Transform { request, response } => {
            push_transforms(d, request.as_ref(), response.as_ref());
        }
        Action::Breakpoint { request, response } => {
            d.break_on_request |= *request;
            d.break_on_response |= *response;
        }
        Action::Delay { ms, jitter_ms } => {
            let jitter = if *jitter_ms > 0 {
                rand::random_range(0..=*jitter_ms)
            } else {
                0
            };
            d.delay_ms = d.delay_ms.saturating_add(*ms).saturating_add(jitter);
        }
        Action::Throttle {
            download_bps,
            upload_bps,
            latency_ms,
        } => {
            let t = d.throttle.get_or_insert_with(Throttle::default);
            t.download_bps = min_nonzero(t.download_bps, *download_bps);
            t.upload_bps = min_nonzero(t.upload_bps, *upload_bps);
            t.latency_ms = t.latency_ms.max(*latency_ms);
        }
        Action::Highlight { color, note } => d.highlight = Some((color.clone(), note.clone())),
        Action::Script { source } => d.scripts.push(source.clone()),
        Action::TlsProfile { profile } => d.tls_profile = Some(*profile),
        Action::WsBreakpoint { .. } => d.ws_breakpoints.push(rule.id.clone()),
        terminal => {
            d.action = terminal.clone();
            d.terminal_rule = Some(rule.id.clone());
            match terminal {
                Action::Passthrough { request, response }
                | Action::Forward {
                    request, response, ..
                } => {
                    push_transforms(d, request.as_ref(), response.as_ref());
                }
                _ => {}
            }
        }
    }
}

fn push_transforms(
    d: &mut Decision,
    request: Option<&akev_core::rules::RequestTransform>,
    response: Option<&akev_core::rules::ResponseTransform>,
) {
    if let Some(t) = request.filter(|t| !t.is_empty()) {
        d.request_transforms.push(t.clone());
    }
    if let Some(t) = response.filter(|t| !t.is_empty()) {
        d.response_transforms.push(t.clone());
    }
}

/// The more restrictive of two bandwidth limits where 0 means unlimited.
fn min_nonzero(a: u64, b: u64) -> u64 {
    match (a, b) {
        (0, x) | (x, 0) => x,
        (a, b) => a.min(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_nonzero_semantics() {
        assert_eq!(min_nonzero(0, 0), 0);
        assert_eq!(min_nonzero(0, 5), 5);
        assert_eq!(min_nonzero(7, 0), 7);
        assert_eq!(min_nonzero(7, 5), 5);
    }

    #[test]
    fn counter_budget() {
        let c = Counter::default();
        assert!(c.try_hit(Some(2)));
        assert!(c.try_hit(Some(2)));
        assert!(!c.try_hit(Some(2)));
        assert!(c.exhausted(Some(2)));
        assert_eq!(c.stats("r").matches, 2);
        assert!(c.stats("r").last_matched_at.is_some());
        c.reset();
        assert!(!c.exhausted(Some(2)));
        assert_eq!(c.stats("r").last_matched_at, None);
        assert!(c.try_hit(None));
    }
}
