import { describe, expect, test } from 'bun:test';
import type { LlmExchange, LlmMessage } from '../../api/types';
import { LLM_FIXTURES } from '../../api/mock/llmFixtures';
import {
  charCount,
  chatBreaks,
  conversationJson,
  conversationMarkdown,
  conversationStats,
  displayJson,
  formatTokens,
  formatTokensPerSecond,
  highlightJson,
  imageSrc,
  isLongText,
  messageMarkdown,
  messageText,
  messageTone,
  paramItems,
  parseJsonText,
  partText,
  prettyValue,
  roleLabel,
  sniffImageType,
  stopReasonInfo,
  toolCallNames,
  usageItems,
} from './conversation';
import { apiLabel, providerInfo, tintVar } from './providers';

function fixture(id: string): LlmExchange {
  const f = LLM_FIXTURES.find((x) => x.id === id);
  if (!f) throw new Error(`no fixture ${id}`);
  return f.llm;
}

const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==';

describe('providers', () => {
  test('known providers have names, marks and tints', () => {
    expect(providerInfo('anthropic')).toMatchObject({ name: 'Anthropic', mark: 'A', tint: 'orange' });
    expect(providerInfo('openai')).toMatchObject({ name: 'OpenAI', tint: 'green' });
    expect(providerInfo('ollama').local).toBe(true);
    expect(providerInfo('vertex-ai').name).toBe('Vertex AI');
    expect(tintVar('blue')).toBe('var(--tag-blue)');
  });

  test('unknown providers get a readable fallback', () => {
    expect(providerInfo('acme-llm')).toMatchObject({ name: 'Acme Llm', mark: 'AL', tint: 'grey' });
    expect(providerInfo('zed')).toMatchObject({ name: 'Zed', mark: 'ZE' });
    expect(providerInfo('').mark).toBe('AI');
  });

  test('API labels', () => {
    expect(apiLabel('anthropic-messages')).toBe('Messages API');
    expect(apiLabel('openai-chat')).toBe('Chat Completions');
    expect(apiLabel('gemini')).toBe('generateContent');
    expect(apiLabel('something-new')).toBe('something-new');
  });
});

describe('roles and tool calls', () => {
  test('roles', () => {
    expect(roleLabel('assistant')).toBe('Assistant');
    expect(roleLabel('developer')).toBe('Developer');
    expect(roleLabel('critic')).toBe('Critic');
    const user: LlmMessage = { role: 'user', content: [{ type: 'text', text: 'hi' }] };
    expect(messageTone(user)).toBe('user');
    expect(messageTone({ role: 'user', content: [{ type: 'tool-result', id: 'a', content: [] }] })).toBe('tool');
    expect(messageTone({ role: 'tool', content: [] })).toBe('tool');
    expect(messageTone({ role: 'developer', content: [] })).toBe('system');
    expect(messageTone({ role: 'model', content: [] })).toBe('assistant');
  });

  test('tool results find the name of their call', () => {
    const x = fixture('openai-answer');
    const names = toolCallNames(x);
    expect(names.get('call_Wx9fT2qLm4Hc')).toBe('get_weather');
    expect(names.get('call_Q7nB3vKs81Pz')).toBe('search_places');
    // Response tool calls count too.
    expect(toolCallNames(fixture('anthropic-agent')).get('toolu_01D7FvYq2nW8xR5tK3mJ6pLs')).toBe('edit_file');
  });

  test('stats', () => {
    expect(conversationStats(fixture('anthropic-agent'))).toEqual({ messages: 3, toolCalls: 2, images: 1 });
    expect(conversationStats(fixture('gemini-reviews'))).toEqual({ messages: 1, toolCalls: 0, images: 0 });
  });
});

describe('JSON helpers', () => {
  test('parseJsonText only accepts objects and arrays', () => {
    expect(parseJsonText(' {"a":1} ')).toEqual({ a: 1 });
    expect(parseJsonText('[1,2]')).toEqual([1, 2]);
    expect(parseJsonText('42')).toBeUndefined();
    expect(parseJsonText('{nope')).toBeUndefined();
    expect(parseJsonText('hello')).toBeUndefined();
  });

  test('prettyValue', () => {
    expect(prettyValue({ a: 1 })).toBe('{\n  "a": 1\n}');
    expect(prettyValue('{"a":1}')).toBe('{\n  "a": 1\n}');
    expect(prettyValue('plain text')).toBe('plain text');
    expect(prettyValue(undefined)).toBe('');
  });

  test('displayJson keeps what fits on one line and breaks the rest', () => {
    expect(displayJson({ path: 'src/money.ts' })).toBe('{ "path": "src/money.ts" }');
    expect(displayJson([1, 'a', null])).toBe('[1, "a", null]');
    expect(displayJson({})).toBe('{}');
    expect(displayJson('{"q":"akev"}')).toBe('{ "q": "akev" }');
    expect(displayJson('not json')).toBe('not json');
    const weather = { city: 'Ljubljana', hourly: [{ time: '12:00', temp_c: 10, precip_prob: 35 }, { time: '13:00', temp_c: 9, precip_prob: 80 }], summary: 'Light rain from 13:00' };
    expect(displayJson(weather)).toBe(
      ['{', '  "city": "Ljubljana",', '  "hourly": [', '    { "time": "12:00", "temp_c": 10, "precip_prob": 35 },', '    { "time": "13:00", "temp_c": 9, "precip_prob": 80 }', '  ],', '  "summary": "Light rain from 13:00"', '}'].join('\n'),
    );
    // Narrow widths break everything; the output is always valid JSON.
    const narrow = displayJson(weather, 10);
    expect(JSON.parse(narrow)).toEqual(weather);
    // Deep nesting doesn't blow the stack.
    let deep: unknown = 1;
    for (let i = 0; i < 500; i++) deep = { a: deep };
    expect(JSON.parse(displayJson(deep, 20))).toEqual(deep);
  });

  test('highlightJson tokenises without losing characters', () => {
    const text = '{\n  "name": "get_weather", "n": -1.5e3, "ok": true, "x": null, "esc": "a\\"b"\n}';
    const tokens = highlightJson(text);
    expect(tokens.map((t) => t.v).join('')).toBe(text);
    expect(tokens.find((t) => t.v === '"name"')?.k).toBe('k');
    expect(tokens.find((t) => t.v === '"get_weather"')?.k).toBe('s');
    expect(tokens.find((t) => t.v === '-1.5e3')?.k).toBe('n');
    expect(tokens.filter((t) => t.k === 'b').map((t) => t.v)).toEqual(['true', 'null']);
    expect(tokens.find((t) => t.v === '"a\\"b"')?.k).toBe('s');
    // Garbage and unterminated strings never throw.
    for (const junk of ['"unterminated', '{{{', '-', 'trueish', '\u0000\u{1F600}', '"\\']) {
      expect(highlightJson(junk).map((t) => t.v).join('')).toBe(junk);
    }
  });
});

describe('text', () => {
  test('chatBreaks keeps prose line breaks but leaves Markdown blocks alone', () => {
    expect(chatBreaks('roses are red\nviolets are blue')).toBe('roses are red  \nviolets are blue');
    expect(chatBreaks('para one\n\npara two')).toBe('para one\n\npara two');
    expect(chatBreaks('- a\n- b')).toBe('- a\n- b');
    expect(chatBreaks('1. a\n2. b')).toBe('1. a\n2. b');
    expect(chatBreaks('Intro\n- a')).toBe('Intro\n- a');
    expect(chatBreaks('| a |\n|---|')).toBe('| a |\n|---|');
    expect(chatBreaks('```\nx\ny\n```')).toBe('```\nx\ny\n```');
    expect(chatBreaks('Title\n=====')).toBe('Title\n=====');
    expect(chatBreaks('already  \nbroken')).toBe('already  \nbroken');
  });

  test('long text detection and counts', () => {
    expect(isLongText('short')).toBe(false);
    expect(isLongText('x'.repeat(2000))).toBe(true);
    expect(isLongText('a\n'.repeat(30))).toBe(true);
    expect(charCount('abc')).toBe('3 chars');
    expect(charCount('a')).toBe('1 char');
    expect(charCount('x'.repeat(1234))).toBe('1.2k chars');
    expect(charCount('x'.repeat(12_345))).toBe('12k chars');
  });

  test('plain text of parts and messages', () => {
    expect(partText({ type: 'tool-call', id: 'c1', name: 'f', arguments: { a: 1 } })).toBe('[tool call f c1]\n{\n  "a": 1\n}');
    expect(partText({ type: 'thinking', text: '', redacted: true })).toBe('[thinking redacted]');
    expect(partText({ type: 'image', mediaType: 'image/png', data: PNG })).toBe('[image image/png]');
    expect(partText({ type: 'tool-result', id: 'c1', content: [{ type: 'text', text: 'ok' }], isError: true })).toBe('[tool result c1 error]\nok');
    expect(messageText({ role: 'user', content: [{ type: 'text', text: 'a' }, { type: 'refusal', text: 'no' }] })).toBe('a\n\n[refusal] no');
  });
});

describe('images', () => {
  test('inline raster images become data URLs; others are never loaded', () => {
    expect(imageSrc({ type: 'image', mediaType: 'image/png', data: PNG })).toBe(`data:image/png;base64,${PNG}`);
    // Undeclared or mislabelled types are sniffed.
    expect(imageSrc({ type: 'image', data: PNG })).toStartWith('data:image/png;base64,');
    expect(imageSrc({ type: 'image', mediaType: 'image/svg+xml', data: PNG })).toStartWith('data:image/png;');
    expect(imageSrc({ type: 'image', mediaType: 'image/svg+xml', data: 'PHN2Zz48L3N2Zz4=' })).toBeNull();
    expect(imageSrc({ type: 'image', url: 'https://example.com/a.png' })).toBeNull();
    expect(imageSrc({ type: 'image', mediaType: 'image/png', data: 'not base64 "><script>' })).toBeNull();
    expect(sniffImageType('/9j/4AAQ')).toBe('image/jpeg');
    expect(sniffImageType('R0lGODlh')).toBe('image/gif');
    expect(sniffImageType('UklGRh4A')).toBe('image/webp');
    expect(sniffImageType('AAAA')).toBeUndefined();
  });
});

describe('usage, params and stop reasons', () => {
  test('usage items in display order, missing values skipped', () => {
    const items = usageItems({ inputTokens: 3942, outputTokens: 187, cacheReadTokens: 3918 });
    expect(items.map((i) => i.key)).toEqual(['inputTokens', 'outputTokens', 'cacheReadTokens']);
    expect(usageItems(undefined)).toEqual([]);
    expect(usageItems({ reasoningTokens: 5, cacheWriteTokens: 2 }).map((i) => i.label)).toEqual(['Cache write', 'Reasoning']);
  });

  test('params: scalars first, structured values on demand', () => {
    const items = paramItems({ tool_choice: { type: 'auto' }, temperature: 0.3, max_tokens: 1024, stop: [], user: 'x'.repeat(60), stream_options: {} });
    expect(items.map((i) => i.key)).toEqual(['max_tokens', 'stop', 'stream_options', 'temperature', 'tool_choice', 'user']);
    expect(items.find((i) => i.key === 'tool_choice')).toMatchObject({ text: '{…}', complex: true });
    expect(items.find((i) => i.key === 'stop')).toMatchObject({ text: '[0]', complex: false });
    expect(items.find((i) => i.key === 'stream_options')).toMatchObject({ text: '{}', complex: false });
    expect(items.find((i) => i.key === 'user')?.text.length).toBe(40);
    expect(paramItems(undefined)).toEqual([]);
  });

  test('stop reasons in plain words', () => {
    expect(stopReasonInfo('end_turn')).toMatchObject({ label: 'Finished', tone: 'success' });
    expect(stopReasonInfo('STOP')).toMatchObject({ label: 'Finished', tone: 'success' });
    expect(stopReasonInfo('tool_calls')).toMatchObject({ label: 'Tool call', tone: 'info' });
    expect(stopReasonInfo('max_tokens')).toMatchObject({ label: 'Token limit', tone: 'warning' });
    expect(stopReasonInfo('length')?.tone).toBe('warning');
    expect(stopReasonInfo('SAFETY')).toMatchObject({ label: 'Filtered', tone: 'danger' });
    expect(stopReasonInfo('weird_reason')).toMatchObject({ label: 'weird_reason', tone: 'neutral' });
    expect(stopReasonInfo(undefined)).toBeNull();
  });

  test('number formats', () => {
    expect(formatTokens(3942)).toBe('3,942');
    expect(formatTokens(2_500_000)).toBe('2.5M');
    expect(formatTokens(undefined)).toBe('—');
    expect(formatTokensPerSecond(68.62)).toBe('68.6 tok/s');
    expect(formatTokensPerSecond(40)).toBe('40 tok/s');
    expect(formatTokensPerSecond(241.7)).toBe('242 tok/s');
    expect(formatTokensPerSecond(undefined)).toBe('—');
  });
});

describe('copy as Markdown / JSON', () => {
  test('a full transcript of a tool-using agent turn', () => {
    const md = conversationMarkdown(fixture('anthropic-agent'));
    expect(md.startsWith('# Anthropic · claude-sonnet-4-5-20250929\n')).toBe(true);
    expect(md).toContain('_Messages API · streamed_');
    expect(md).toContain('## System\n\nYou are Forge');
    expect(md).toContain('- `edit_file` — Replace an exact string');
    expect(md).toContain('**Tool call** `read_file` (toolu_01A9mK2xQ7vR4tN8bH3cZ5wP)\n\n```json\n{\n  "path": "src/money.ts"\n}\n```');
    expect(md).toContain('**Tool result** `read_file` (toolu_01A9mK2xQ7vR4tN8bH3cZ5wP)');
    expect(md).toContain('## Assistant (response)');
    expect(md).toContain('<details><summary>Thinking</summary>');
    expect(md).toContain('_stop: tool_use · input: 3942 · output: 187 · cache read: 3918_');
    expect(md).toContain('_[image image/png]_');
  });

  test('fences grow past backticks in the content', () => {
    const m: LlmMessage = { role: 'tool', content: [{ type: 'tool-result', content: [{ type: 'text', text: 'a ``` b' }] }] };
    expect(messageMarkdown(m)).toContain('````\na ``` b\n````');
  });

  test('errors and JSON', () => {
    const x = fixture('anthropic-overloaded');
    expect(conversationMarkdown(x)).toContain('> **Error 529:** Overloaded');
    expect(JSON.parse(conversationJson(x))).toEqual(x);
  });
});
