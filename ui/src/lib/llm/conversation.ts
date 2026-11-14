/**
 * Pure helpers for the LLM conversation view (`GET /api/exchanges/:id/llm`): roles, plain-text
 * and Markdown renderings for copying, usage / parameter / stop-reason presentation, safe
 * inline images and a tiny JSON syntax highlighter for tool arguments.
 */

import type { LlmExchange, LlmMessage, LlmPart, LlmUsage } from '../../api/types';
import { formatNumber } from '../format';
import { apiLabel, providerInfo } from './providers';

// =====================================================================================
// Roles
// =====================================================================================

export type MessageTone = 'user' | 'assistant' | 'tool' | 'system';

const ROLE_LABELS: Record<string, string> = {
  user: 'User',
  assistant: 'Assistant',
  tool: 'Tool',
  system: 'System',
  developer: 'Developer',
  model: 'Model',
  function: 'Function',
};

/** "User", "Assistant", … (unknown roles are capitalised). */
export function roleLabel(role: string): string {
  return ROLE_LABELS[role] ?? (role ? role.charAt(0).toUpperCase() + role.slice(1) : 'Message');
}

/** How a message is drawn: messages that only carry tool results read as tool turns. */
export function messageTone(m: LlmMessage): MessageTone {
  if (m.role === 'system' || m.role === 'developer') return 'system';
  if (m.role === 'assistant' || m.role === 'model') return 'assistant';
  if (m.role === 'tool' || m.role === 'function') return 'tool';
  if (m.content.length > 0 && m.content.every((p) => p.type === 'tool-result')) return 'tool';
  return 'user';
}

/** Tool-call id → tool name, so results can say which call they answer. */
export function toolCallNames(x: LlmExchange): Map<string, string> {
  const out = new Map<string, string>();
  const visit = (messages: readonly LlmMessage[]) => {
    for (const m of messages) for (const p of m.content) if (p.type === 'tool-call' && p.id) out.set(p.id, p.name);
  };
  visit(x.request.messages);
  if (x.response) visit(x.response.messages);
  return out;
}

// =====================================================================================
// Text
// =====================================================================================

/** Pretty JSON of any value; a string holding a JSON object/array is pretty-printed too. */
export function prettyValue(v: unknown): string {
  if (typeof v === 'string') {
    const parsed = parseJsonText(v);
    return parsed === undefined ? v : JSON.stringify(parsed, null, 2);
  }
  if (v === undefined) return '';
  try {
    return JSON.stringify(v, null, 2) ?? String(v);
  } catch {
    return String(v);
  }
}

/** One-line JSON with spaces after `:` and `,` (`{ "a": 1, "b": [1, 2] }`). */
function inlineJson(v: unknown): string {
  if (v === null || typeof v !== 'object') return JSON.stringify(v) ?? String(v);
  if (Array.isArray(v)) return `[${v.map(inlineJson).join(', ')}]`;
  const entries = Object.entries(v);
  return entries.length === 0 ? '{}' : `{ ${entries.map(([k, e]) => `${JSON.stringify(k)}: ${inlineJson(e)}`).join(', ')} }`;
}

/**
 * JSON for display, Prettier-style: any object or array whose one-line form fits in `width`
 * columns (indentation and key included) stays on one line, bigger ones break over lines.
 * Strings holding JSON are parsed first; other strings are returned as they are.
 */
export function displayJson(v: unknown, width = 76): string {
  const value = typeof v === 'string' ? (parseJsonText(v) ?? v) : v;
  if (typeof value === 'string') return value;
  const fmt = (x: unknown, indent: string, prefix: number, depth: number): string => {
    if (x === null || typeof x !== 'object') return JSON.stringify(x) ?? String(x);
    if (depth > 64) return JSON.stringify(x);
    const one = inlineJson(x);
    if (indent.length + prefix + one.length <= width) return one;
    const inner = `${indent}  `;
    if (Array.isArray(x)) {
      if (x.length === 0) return '[]';
      return `[\n${x.map((e) => inner + fmt(e, inner, 0, depth + 1)).join(',\n')}\n${indent}]`;
    }
    const entries = Object.entries(x);
    if (entries.length === 0) return '{}';
    return `{\n${entries
      .map(([k, e]) => {
        const key = `${JSON.stringify(k)}: `;
        return inner + key + fmt(e, inner, key.length, depth + 1);
      })
      .join(',\n')}\n${indent}}`;
  };
  try {
    return fmt(value, '', 0, 0);
  } catch {
    return prettyValue(value);
  }
}

/** The parsed value when `text` is a JSON object or array, else `undefined`. */
export function parseJsonText(text: string): unknown {
  const t = text.trim();
  if (!(t.startsWith('{') && t.endsWith('}')) && !(t.startsWith('[') && t.endsWith(']'))) return undefined;
  try {
    return JSON.parse(t) as unknown;
  } catch {
    return undefined;
  }
}

/** Plain-text rendering of one part (for copying). */
export function partText(p: LlmPart): string {
  switch (p.type) {
    case 'text':
      return p.text;
    case 'refusal':
      return `[refusal] ${p.text}`;
    case 'thinking':
      return p.redacted ? '[thinking redacted]' : `[thinking]\n${p.text}`;
    case 'image':
      return `[image${p.mediaType ? ` ${p.mediaType}` : ''}${p.url ? ` ${p.url}` : ''}]`;
    case 'file':
      return `[${p.label}${p.name ? ` ${p.name}` : ''}${p.mediaType ? ` (${p.mediaType})` : ''}]`;
    case 'tool-call':
      return `[tool call ${p.name}${p.id ? ` ${p.id}` : ''}]\n${prettyValue(p.arguments)}`;
    case 'tool-result':
      return `[tool result${p.name ? ` ${p.name}` : ''}${p.id ? ` ${p.id}` : ''}${p.isError ? ' error' : ''}]\n${p.content.map(partText).join('\n\n')}`;
    case 'other':
      return `[${p.label}]\n${prettyValue(p.value)}`;
  }
}

/** Plain-text rendering of a message's content. */
export function messageText(m: LlmMessage): string {
  return m.content.map(partText).join('\n\n');
}

const BLOCK_START = /^\s{0,3}(?:[-*+]\s|\d{1,9}[.)]\s|#{1,6}(?:\s|$)|>|\||```|~~~|(?:[-*_]\s*){3,}$|[=-]{2,}\s*$)|^\s{4}/;

/**
 * Keep the line breaks of chat text when rendering it as Markdown (like chat UIs do): a single
 * newline between two plain prose lines becomes a hard break. Lists, headings, tables, quotes
 * and fenced code are left alone.
 */
export function chatBreaks(text: string): string {
  const lines = text.split('\n');
  let fence = false;
  for (let i = 0; i < lines.length - 1; i++) {
    const line = lines[i] as string;
    if (/^\s{0,3}(```|~~~)/.test(line)) fence = !fence;
    if (fence) continue;
    const next = lines[i + 1] as string;
    if (!line.trim() || !next.trim() || BLOCK_START.test(line) || BLOCK_START.test(next) || / {2}$/.test(line)) continue;
    lines[i] = `${line}  `;
  }
  return lines.join('\n');
}

/** Text worth collapsing behind "Show more". */
export function isLongText(text: string, maxChars = 1400, maxLines = 18): boolean {
  if (text.length > maxChars) return true;
  let lines = 1;
  for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10 && ++lines > maxLines) return true;
  return false;
}

/** `1.2k chars`, `312 chars`. */
export function charCount(text: string): string {
  const n = text.length;
  if (n < 1000) return `${n} char${n === 1 ? '' : 's'}`;
  return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0).replace(/\.0$/, '')}k chars`;
}

// =====================================================================================
// Images
// =====================================================================================

const RASTER = new Set(['image/png', 'image/jpeg', 'image/jpg', 'image/gif', 'image/webp', 'image/avif', 'image/bmp']);

/** Media type from the first base64 characters (PNG, JPEG, GIF, WebP). */
export function sniffImageType(base64: string): string | undefined {
  if (base64.startsWith('iVBORw0KGgo')) return 'image/png';
  if (base64.startsWith('/9j/')) return 'image/jpeg';
  if (base64.startsWith('R0lGOD')) return 'image/gif';
  if (base64.startsWith('UklGR')) return 'image/webp';
  return undefined;
}

/**
 * A `data:` URL for an inline image, or `null` (remote URLs are never loaded — privacy — and
 * only raster types are rendered; SVG could carry active content).
 */
export function imageSrc(p: Extract<LlmPart, { type: 'image' }>): string | null {
  if (!p.data || !/^[A-Za-z0-9+/=\s_-]+$/.test(p.data.slice(0, 4096))) return null;
  const sniffed = sniffImageType(p.data);
  const declared = p.mediaType?.toLowerCase();
  const type = declared && RASTER.has(declared) ? declared : sniffed;
  if (!type) return null;
  return `data:${type};base64,${p.data.replace(/\s+/g, '')}`;
}

// =====================================================================================
// Usage, parameters, stop reasons
// =====================================================================================

export interface UsageItem {
  key: keyof LlmUsage;
  label: string;
  value: number;
  hint: string;
}

/** Usage figures worth showing, in display order. */
export function usageItems(u: LlmUsage | undefined): UsageItem[] {
  if (!u) return [];
  const out: UsageItem[] = [];
  const add = (key: keyof LlmUsage, label: string, hint: string) => {
    const v = u[key];
    if (typeof v === 'number' && Number.isFinite(v)) out.push({ key, label, value: v, hint });
  };
  add('inputTokens', 'Input', 'Prompt tokens, cached ones included');
  add('outputTokens', 'Output', 'Generated tokens, reasoning included');
  add('cacheReadTokens', 'Cache read', "Prompt tokens served from the provider's cache");
  add('cacheWriteTokens', 'Cache write', 'Prompt tokens written to the cache');
  add('reasoningTokens', 'Reasoning', 'Output tokens spent thinking');
  return out;
}

export interface ParamItem {
  key: string;
  /** Short display value (`0.3`, `"auto"`, `{…}`). */
  text: string;
  /** Objects and arrays: show the pretty JSON on demand. */
  complex: boolean;
  value: unknown;
}

/** Request parameters as chips, scalars first then structured ones, each group alphabetical. */
export function paramItems(params: Record<string, unknown> | undefined): ParamItem[] {
  if (!params) return [];
  const items = Object.entries(params).map(([key, value]): ParamItem => {
    if (value === null) return { key, text: 'null', complex: false, value };
    if (typeof value === 'object') {
      const n = Array.isArray(value) ? value.length : Object.keys(value).length;
      return { key, text: Array.isArray(value) ? `[${n}]` : n === 0 ? '{}' : '{…}', complex: n > 0, value };
    }
    if (typeof value === 'string') return { key, text: value.length > 40 ? `${value.slice(0, 39)}…` : value, complex: value.length > 40, value };
    return { key, text: String(value), complex: false, value };
  });
  return items.sort((a, b) => Number(a.complex) - Number(b.complex) || a.key.localeCompare(b.key));
}

export type Tone = 'success' | 'info' | 'warning' | 'danger' | 'neutral';

/** Plain-language stop reason with a tone. */
export function stopReasonInfo(reason: string | undefined): { label: string; tone: Tone; raw: string } | null {
  if (!reason) return null;
  const r = reason.toLowerCase();
  if (['stop', 'end_turn', 'completed', 'stop_sequence', 'eos'].includes(r)) return { label: 'Finished', tone: 'success', raw: reason };
  if (['tool_use', 'tool_calls', 'function_call'].includes(r)) return { label: 'Tool call', tone: 'info', raw: reason };
  if (['length', 'max_tokens', 'max_output_tokens', 'model_context_window_exceeded'].includes(r)) return { label: 'Token limit', tone: 'warning', raw: reason };
  if (['content_filter', 'safety', 'refusal', 'recitation', 'blocklist', 'prohibited_content', 'spii'].includes(r)) return { label: 'Filtered', tone: 'danger', raw: reason };
  if (r === 'pause_turn') return { label: 'Paused', tone: 'info', raw: reason };
  if (r === 'incomplete' || r === 'failed' || r === 'cancelled') return { label: reason.charAt(0).toUpperCase() + reason.slice(1), tone: 'warning', raw: reason };
  return { label: reason, tone: 'neutral', raw: reason };
}

/** `42.1 tok/s` (whole numbers from 100). */
export function formatTokensPerSecond(tps: number | undefined): string {
  if (tps == null || !Number.isFinite(tps)) return '—';
  return `${tps >= 100 ? Math.round(tps) : tps.toFixed(1).replace(/\.0$/, '')} tok/s`;
}

/** `12,345` tokens; `1.2M` from a million. */
export function formatTokens(n: number | undefined): string {
  if (n == null || !Number.isFinite(n)) return '—';
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, '')}M`;
  return formatNumber(n);
}

/** Counts for the conversation divider. */
export function conversationStats(x: LlmExchange): { messages: number; toolCalls: number; images: number } {
  let toolCalls = 0;
  let images = 0;
  const all = [...x.request.messages, ...(x.response?.messages ?? [])];
  for (const m of all)
    for (const p of m.content) {
      if (p.type === 'tool-call') toolCalls++;
      if (p.type === 'image') images++;
    }
  return { messages: x.request.messages.length, toolCalls, images };
}

// =====================================================================================
// Copy as Markdown / JSON
// =====================================================================================

function fence(text: string, lang = ''): string {
  const longest = Math.max(2, ...Array.from(text.matchAll(/`+/g), (m) => m[0].length));
  const ticks = '`'.repeat(longest + 1);
  return `${ticks}${lang}\n${text}\n${ticks}`;
}

function partMarkdown(p: LlmPart, names: Map<string, string>): string {
  switch (p.type) {
    case 'text':
      return p.text;
    case 'refusal':
      return `> **Refusal:** ${p.text}`;
    case 'thinking':
      return p.redacted ? '> _Thinking (redacted)_' : `<details><summary>Thinking</summary>\n\n${p.text}\n\n</details>`;
    case 'image':
      return `_[image${p.mediaType ? ` ${p.mediaType}` : ''}${p.url ? ` ${p.url}` : ''}]_`;
    case 'file':
      return `_[${p.label}${p.name ? ` ${p.name}` : ''}]_`;
    case 'tool-call':
      return `**Tool call** \`${p.name}\`${p.id ? ` (${p.id})` : ''}\n\n${fence(prettyValue(p.arguments), 'json')}`;
    case 'tool-result': {
      const name = p.name ?? (p.id ? names.get(p.id) : undefined);
      const body = p.content.map((c) => (c.type === 'text' ? prettyValue(c.text) : partText(c))).join('\n\n');
      const lang = p.content.length === 1 && p.content[0]?.type === 'text' && parseJsonText(p.content[0].text) !== undefined ? 'json' : '';
      return `**Tool result**${name ? ` \`${name}\`` : ''}${p.id ? ` (${p.id})` : ''}${p.isError ? ' — error' : ''}\n\n${fence(body, lang)}`;
    }
    case 'other':
      return `**${p.label}**\n\n${fence(prettyValue(p.value), 'json')}`;
  }
}

/** One message as Markdown (heading + content). */
export function messageMarkdown(m: LlmMessage, names: Map<string, string> = new Map(), heading = `## ${roleLabel(m.role)}`): string {
  const body = m.content.map((p) => partMarkdown(p, names)).join('\n\n');
  return `${heading}\n\n${body || '_(empty)_'}`;
}

/** The whole conversation as a Markdown transcript. */
export function conversationMarkdown(x: LlmExchange): string {
  const names = toolCallNames(x);
  const p = providerInfo(x.provider);
  const lines: string[] = [`# ${p.name} · ${x.model ?? 'unknown model'}`, '', `_${apiLabel(x.api)}${x.stream ? ' · streamed' : ''}_`];
  if (x.request.system) lines.push('', '## System', '', x.request.system);
  if (x.request.tools.length) {
    lines.push('', `## Tools (${x.request.tools.length})`, '');
    for (const t of x.request.tools) lines.push(`- \`${t.name}\`${t.kind ? ` _(${t.kind})_` : ''}${t.description ? ` — ${t.description}` : ''}`);
  }
  for (const m of x.request.messages) lines.push('', messageMarkdown(m, names));
  if (x.response) {
    for (const m of x.response.messages) lines.push('', messageMarkdown(m, names, `## ${roleLabel(m.role)} (response)`));
    const meta: string[] = [];
    if (x.response.stopReason) meta.push(`stop: ${x.response.stopReason}`);
    for (const u of usageItems(x.response.usage)) meta.push(`${u.label.toLowerCase()}: ${u.value}`);
    if (meta.length) lines.push('', `_${meta.join(' · ')}_`);
  }
  if (x.error) lines.push('', `> **Error${x.error.status ? ` ${x.error.status}` : ''}:** ${x.error.message}`);
  return `${lines.join('\n')}\n`;
}

/** The normalised conversation as pretty JSON. */
export function conversationJson(x: LlmExchange): string {
  return `${JSON.stringify(x, null, 2)}\n`;
}

// =====================================================================================
// JSON highlighting (tool arguments, results, schemas)
// =====================================================================================

/** `k` key, `s` string, `n` number, `b` true/false/null, `p` punctuation, `t` other text. */
export type JsonTokenKind = 'k' | 's' | 'n' | 'b' | 'p' | 't';
export interface JsonToken {
  k: JsonTokenKind;
  v: string;
}

/**
 * Tokenise (pretty-printed) JSON text for colouring. Never fails: anything unexpected
 * becomes plain text, so it is safe on arbitrary input. Linear time.
 */
export function highlightJson(text: string): JsonToken[] {
  const out: JsonToken[] = [];
  const push = (k: JsonTokenKind, v: string) => {
    const last = out[out.length - 1];
    if (last && last.k === k && (k === 't' || k === 'p')) last.v += v;
    else out.push({ k, v });
  };
  let i = 0;
  const n = text.length;
  while (i < n) {
    const c = text[i] as string;
    if (c === '"') {
      let j = i + 1;
      while (j < n && text[j] !== '"') j += text[j] === '\\' ? 2 : 1;
      j = Math.min(j + 1, n);
      let k = j;
      while (k < n && (text[k] === ' ' || text[k] === '\t')) k++;
      push(text[k] === ':' ? 'k' : 's', text.slice(i, j));
      i = j;
    } else if (c === '-' || (c >= '0' && c <= '9')) {
      const m = /^-?\d+(\.\d+)?([eE][+-]?\d+)?/.exec(text.slice(i, i + 64));
      const len = m ? m[0].length : 1;
      push(m ? 'n' : 't', text.slice(i, i + len));
      i += len;
    } else if (text.startsWith('true', i) || text.startsWith('null', i)) {
      push('b', text.slice(i, i + 4));
      i += 4;
    } else if (text.startsWith('false', i)) {
      push('b', 'false');
      i += 5;
    } else if ('{}[],:'.includes(c)) {
      push('p', c);
      i++;
    } else {
      push('t', c);
      i++;
    }
  }
  return out;
}
