/**
 * The mock daemon's LLM traffic lens: recorded LLM API exchanges (generated from the Rust
 * parser's fixtures, see `llmFixtures.ts`) and `GET /api/exchanges/:id/llm` for them — the
 * response revealed as far as the stream has got, timing from the mock clock (same definitions
 * as the daemon: measured from the moment the request was sent).
 */

import type { LlmExchange, LlmPart, LlmResponse, LlmTag } from '../types';
import type { MockRecord } from './generator';
import { LLM_FIXTURES, type LlmFixture } from './llmFixtures';

export { LLM_FIXTURES, type LlmFixture };

/** The summary tag the daemon would compute: provider and requested model. */
export function llmTag(f: LlmFixture): LlmTag {
  let model: string | undefined;
  try {
    const v = JSON.parse(f.requestBody) as { model?: unknown };
    if (typeof v.model === 'string') model = v.model;
  } catch {
    model = undefined;
  }
  model ??= /\/models\/([^/:]+):/.exec(f.path)?.[1];
  return { provider: f.llm.provider, ...(model ? { model } : {}) };
}

/** Weight of a part when revealing a partial stream (characters, tool calls count as a block). */
function weight(p: LlmPart): number {
  return p.type === 'text' || p.type === 'thinking' ? p.text.length : 60;
}

/** The response as far as `fraction` of its stream has arrived (parts revealed in order). */
export function partialResponse(r: LlmResponse, fraction: number): LlmResponse {
  const total = r.messages.reduce((a, m) => a + m.content.reduce((b, p) => b + weight(p), 0), 0);
  let budget = Math.floor(total * Math.max(0, Math.min(1, fraction)));
  const messages = r.messages.map((m) => {
    const content: LlmPart[] = [];
    for (const p of m.content) {
      if (budget <= 0) break;
      const w = weight(p);
      if ((p.type === 'text' || p.type === 'thinking') && w > budget) {
        content.push({ ...p, text: p.text.slice(0, budget) });
        budget = 0;
        break;
      }
      if (w > budget) break;
      content.push(p);
      budget -= w;
    }
    return { ...m, content };
  });
  return { ...(r.id ? { id: r.id } : {}), ...(r.model ? { model: r.model } : {}), messages: messages.filter((m) => m.content.length > 0) };
}

const round2 = (n: number) => Math.round(n * 100) / 100;

/** `GET /api/exchanges/:id/llm` for a mock record (`null` = not an LLM call). */
export function llmView(rec: MockRecord): LlmExchange | null {
  const f = rec.llm;
  if (!f) return null;
  const { response, error, ...base } = f.llm;
  const out: LlmExchange = { ...base, timing: {} };
  if (!rec.response) return out;
  const done = rec.phase === 'done';
  const events = f.events?.length ?? 0;
  if (response) out.response = f.events && !done ? partialResponse(response, events ? rec.messages.length / events : 0) : response;
  if (error) out.error = error;
  const start = rec.summary.startedAt;
  const sent = rec.timing.requestSent ?? rec.timing.requestBody ?? 0;
  if (rec.timing.responseBody != null) out.timing.totalMs = round2(Math.max(0, rec.timing.responseBody - sent));
  const firstAt = f.firstTokenEvent != null ? rec.messages[f.firstTokenEvent]?.at : undefined;
  if (firstAt != null) out.timing.ttftMs = round2(Math.max(0, firstAt / 1000 - start - sent));
  if (f.events) out.timing.chunks = rec.messages.length;
  const output = out.response?.usage?.outputTokens;
  const total = out.timing.totalMs;
  if (done && output && total != null) {
    const generating = out.timing.ttftMs != null ? total - out.timing.ttftMs : total;
    if (generating >= 10) out.timing.tokensPerSecond = round2(output / (generating / 1000));
  }
  return out;
}
