/** Rule hit statistics helpers and rule search. */

import type { Rule, RuleStats } from '../../api/types';
import { summarize } from './describe';

export interface LimitState {
  limit: number;
  used: number;
  remaining: number;
  exhausted: boolean;
}

/** Match-limit state (`maxMatches`), from the daemon's real counter. */
export function limitState(rule: Pick<Rule, 'maxMatches'>, stats: RuleStats | undefined): LimitState | null {
  if (rule.maxMatches == null) return null;
  const used = Math.min(stats?.matches ?? 0, rule.maxMatches);
  return { limit: rule.maxMatches, used, remaining: rule.maxMatches - used, exhausted: used >= rule.maxMatches };
}

/** Compact hit count: `12×`, `1.2k×`. */
export function formatHits(n: number): string {
  if (n < 1000) return `${n}×`;
  if (n < 10_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, '')}k×`;
  if (n < 1_000_000) return `${Math.round(n / 1000)}k×`;
  return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, '')}M×`;
}

/** Case-insensitive search over a rule's title, auto-summary and note (all words must match). */
export function ruleMatchesQuery(rule: Rule, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const hay = `${rule.title ?? ''}\n${summarize(rule)}\n${rule.note ?? ''}\n${rule.action.type}`.toLowerCase();
  return words.every((w) => hay.includes(w));
}
