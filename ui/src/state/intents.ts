import { create } from 'zustand';
import type { EditableRequest, ExchangeId, RuleId } from '../api/types';
import { navigate } from '../app/router';

/**
 * Cross-page "intents": small hand-offs between pages (e.g. Traffic → Compose, anything →
 * Rules) without pages importing each other. A page that *receives* intents drains them
 * when it mounts and whenever new ones arrive.
 */
export interface ComposeIntent {
  request: EditableRequest;
  /** Tab title (defaults to the URL path). */
  title?: string;
  /** Captured exchange this request came from, if any. */
  sourceExchangeId?: ExchangeId;
}

interface IntentsState {
  /** Requests waiting to be opened as new Compose tabs (FIFO). */
  composeQueue: ComposeIntent[];
  /** Rule the Rules page should select & scroll into view (e.g. just created from Traffic). */
  revealRuleId: RuleId | null;
}

export const useIntents = create<IntentsState>()(() => ({
  composeQueue: [],
  revealRuleId: null,
}));

/** Open a request in a new Compose tab and navigate there. */
export function openInCompose(request: EditableRequest, opts: Omit<ComposeIntent, 'request'> = {}): void {
  useIntents.setState((s) => ({ composeQueue: [...s.composeQueue, { request, ...opts }] }));
  navigate('/compose');
}

/** Compose page: take (and clear) all queued requests. */
export function takeComposeQueue(): ComposeIntent[] {
  const q = useIntents.getState().composeQueue;
  if (q.length) useIntents.setState({ composeQueue: [] });
  return q;
}

/** Select a rule in the Rules page and navigate there (`#/rules/<id>`). */
export function revealRule(id: RuleId): void {
  useIntents.setState({ revealRuleId: id });
  navigate(`/rules/${encodeURIComponent(id)}`);
}

/** Rules page: take (and clear) the rule to reveal. */
export function takeRevealRule(): RuleId | null {
  const id = useIntents.getState().revealRuleId;
  if (id) useIntents.setState({ revealRuleId: null });
  return id;
}
