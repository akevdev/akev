import { describe, expect, test } from 'bun:test';
import type { ExchangeSummary, LlmExchange } from '../types';
import { LLM_FIXTURES, llmTag, partialResponse } from './llm';
import { MockServer } from './server';

const get = async (server: MockServer, path: string) => server.fetch(path, { method: 'GET' });

describe('mock LLM fixtures', () => {
  test('every fixture is a parsed conversation with request and response bodies', () => {
    expect(LLM_FIXTURES.map((f) => f.id)).toEqual(['openai-tools', 'openai-answer', 'anthropic-agent', 'gemini-reviews', 'ollama-haiku', 'anthropic-overloaded']);
    for (const f of LLM_FIXTURES) {
      expect(() => JSON.parse(f.requestBody)).not.toThrow();
      expect(f.llm.request.messages.length).toBeGreaterThan(0);
      if (f.events) {
        expect(f.contentType).toStartWith('text/event-stream');
        expect(f.events.length).toBeGreaterThan(3);
        expect(f.firstTokenEvent).not.toBeNull();
      }
    }
  });

  test('summary tags carry the requested model (Gemini: from the path)', () => {
    const tag = (id: string) => llmTag(LLM_FIXTURES.find((f) => f.id === id) as (typeof LLM_FIXTURES)[number]);
    expect(tag('openai-tools')).toEqual({ provider: 'openai', model: 'gpt-4.1' });
    expect(tag('anthropic-agent')).toEqual({ provider: 'anthropic', model: 'claude-sonnet-4-5' });
    expect(tag('gemini-reviews')).toEqual({ provider: 'google', model: 'gemini-2.5-flash' });
    expect(tag('ollama-haiku')).toEqual({ provider: 'ollama', model: 'llama3.2:3b' });
  });

  test('a partial stream reveals the reply in order', () => {
    const r = (LLM_FIXTURES.find((f) => f.id === 'anthropic-agent') as (typeof LLM_FIXTURES)[number]).llm.response;
    if (!r) throw new Error('fixture has a response');
    expect(partialResponse(r, 0).messages).toEqual([]);
    const early = partialResponse(r, 0.2).messages[0]?.content ?? [];
    expect(early.map((p) => p.type)).toEqual(['thinking']);
    const most = partialResponse(r, 0.9).messages[0]?.content ?? [];
    expect(most.map((p) => p.type)).toEqual(['thinking', 'text']);
    expect(partialResponse(r, 1).messages).toEqual(r.messages);
    expect(partialResponse(r, 1).usage).toBeUndefined();
  });
});

describe('mock /api/exchanges/:id/llm', () => {
  test('LLM calls are tagged, filterable and served as conversations with timing', async () => {
    const server = new MockServer({ license: 'pro' });
    server.start();
    try {
      const list = (await (await get(server, '/api/exchanges?filter=is:llm')).json()) as ExchangeSummary[];
      expect(list.length).toBeGreaterThanOrEqual(6);
      expect(list.every((s) => s.llm != null)).toBe(true);
      const claude = (await (await get(server, '/api/exchanges?filter=model:claude-sonnet')).json()) as ExchangeSummary[];
      expect(claude.length).toBeGreaterThanOrEqual(1);
      const ollama = (await (await get(server, '/api/exchanges?filter=provider:ollama')).json()) as ExchangeSummary[];
      expect(ollama.every((s) => s.host === 'localhost' && s.port === 11434)).toBe(true);

      const anthropic = claude[0] as ExchangeSummary;
      expect(anthropic.kind).toBe('event-stream');
      const x = (await (await get(server, `/api/exchanges/${anthropic.id}/llm`)).json()) as LlmExchange;
      expect(x).toMatchObject({ provider: 'anthropic', api: 'anthropic-messages', stream: true });
      expect(x.response?.stopReason).toBe('tool_use');
      expect(x.timing.ttftMs).toBeGreaterThan(0);
      expect(x.timing.totalMs).toBeGreaterThan(x.timing.ttftMs ?? 0);
      expect(x.timing.tokensPerSecond).toBeGreaterThan(0);
      expect(x.timing.chunks).toBe(23);
      // The recorded SSE events are the fixture's.
      const messages = (await (await get(server, `/api/exchanges/${anthropic.id}/messages`)).json()) as { event?: string }[];
      expect(messages[0]?.event).toBe('message_start');

      const errors = (await (await get(server, '/api/exchanges?filter=is:llm status:529')).json()) as ExchangeSummary[];
      const failed = (await (await get(server, `/api/exchanges/${(errors[0] as ExchangeSummary).id}/llm`)).json()) as LlmExchange;
      expect(failed.response).toBeUndefined();
      expect(failed.error).toMatchObject({ status: 529, kind: 'overloaded_error' });

      const plain = (await (await get(server, '/api/exchanges?filter=-is:llm&limit=1')).json()) as ExchangeSummary[];
      const notLlm = await get(server, `/api/exchanges/${(plain[0] as ExchangeSummary).id}/llm`);
      expect(notLlm.status).toBe(404);
      expect((await notLlm.json()).error.code).toBe('not-llm');

      const suggestions = (await (await server.fetch('/api/filter/suggest', { method: 'POST', body: JSON.stringify({ query: 'model:', cursor: 6 }), headers: { 'content-type': 'application/json' } })).json()) as { label: string }[];
      expect(suggestions.map((s) => s.label)).toContain('claude-sonnet-4-5');
    } finally {
      server.stop();
    }
  });
});
