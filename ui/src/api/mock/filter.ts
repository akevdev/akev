/**
 * A faithful-enough implementation of the Akev filter language (see akev-filter docs) for
 * the mock backend, so the filter bar, validation errors and autocompletion can be built and
 * reviewed without the daemon. Error and suggestion offsets are UTF-8 byte offsets, exactly
 * like the real API.
 */

import { utf8Length } from '../../lib/encoding';
import type { ExchangeSummary, FilterError, Headers, Suggestion } from '../types';
import { FILTER_DOCS } from './data';

/** What the evaluator can see of an exchange. */
export interface FilterSubject {
  summary: ExchangeSummary;
  url: string;
  requestHeaders: Headers;
  responseHeaders?: Headers;
  requestBody?: string;
  responseBody?: string;
  note?: string;
  clientIp?: string;
  ruleTitle?: string;
}

export type Predicate = (s: FilterSubject) => boolean;

type Token =
  | { t: 'lparen' | 'rparen' | 'or' | 'and' | 'not'; start: number; end: number }
  | { t: 'term'; text: string; start: number; end: number };

class ParseFailure extends Error {
  constructor(
    message: string,
    readonly start: number,
    readonly end: number,
  ) {
    super(message);
  }
}

function tokenize(q: string): Token[] {
  const out: Token[] = [];
  let i = 0;
  while (i < q.length) {
    const c = q[i] as string;
    if (/\s/.test(c)) {
      i++;
      continue;
    }
    if (c === '(') {
      out.push({ t: 'lparen', start: i, end: i + 1 });
      i++;
      continue;
    }
    if (c === ')') {
      out.push({ t: 'rparen', start: i, end: i + 1 });
      i++;
      continue;
    }
    if ((c === '-' || c === '!') && i + 1 < q.length && !/\s/.test(q[i + 1] as string)) {
      out.push({ t: 'not', start: i, end: i + 1 });
      i++;
      continue;
    }
    if (c === '|' || (c === '&' && q[i + 1] === '&')) {
      const len = q[i + 1] === c ? 2 : 1;
      out.push({ t: c === '|' ? 'or' : 'and', start: i, end: i + len });
      i += len;
      continue;
    }
    // term: read until whitespace or paren, respecting quotes and /regex/
    const start = i;
    let quote: string | null = null;
    while (i < q.length) {
      const ch = q[i] as string;
      if (quote) {
        if (ch === '\\') i++;
        else if (ch === quote) quote = null;
      } else if (ch === '"' || ch === "'") quote = ch;
      else if (/\s/.test(ch) || ch === '(' || ch === ')') break;
      i++;
    }
    if (quote) throw new ParseFailure(`Unterminated ${quote === '"' ? 'double' : 'single'} quote`, start, q.length);
    const text = q.slice(start, i);
    const upper = text.toUpperCase();
    if (upper === 'OR') out.push({ t: 'or', start, end: i });
    else if (upper === 'AND') out.push({ t: 'and', start, end: i });
    else if (upper === 'NOT') out.push({ t: 'not', start, end: i });
    else out.push({ t: 'term', text, start, end: i });
  }
  return out;
}

const FIELD_ALIASES: Record<string, string> = { domain: 'host', protocol: 'scheme', 'content-type': 'type', time: 'duration', 'llm-model': 'model', vendor: 'provider' };
const FIELDS = new Set(FILTER_DOCS.map((d) => d.name));
const FLAGS = ['pending', 'complete', 'error', 'failed', 'aborted', 'paused', 'pinned', 'mocked', 'modified', 'websocket', 'ws', 'sse', 'tls-failure', 'tunnel', 'http', 'https', 'highlighted', 'llm', 'ai'];
const HAS = ['note', 'body', 'reqbody', 'resbody', 'cookie', 'auth'];
const OP_RE = /^([a-zA-Z][\w-]*(?:\.[\w-]+)?)(!=|\^=|\$=|\*=|>=|<=|:|=|~|>|<)(.*)$/s;

function unquote(v: string): string {
  if ((v.startsWith('"') && v.endsWith('"')) || (v.startsWith("'") && v.endsWith("'"))) return v.slice(1, -1).replace(/\\(.)/g, '$1');
  return v;
}

function splitValues(raw: string): string[] {
  if (raw.startsWith('"') || raw.startsWith("'") || raw.startsWith('/')) return [raw];
  return raw.split(',').filter((v) => v.length > 0);
}

function globToRegex(glob: string): RegExp {
  const esc = glob.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*').replace(/\?/g, '.');
  return new RegExp(`^${esc}$`, 'i');
}

function stringMatcher(op: string, rawValue: string, start: number, end: number): (target: string | undefined) => boolean {
  const regexLit = /^\/(.*)\/([a-z]*)$/s.exec(rawValue);
  if (op === '~' || regexLit) {
    const src = regexLit ? (regexLit[1] ?? '') : unquote(rawValue);
    const flags = regexLit ? (regexLit[2] ?? '') : 'i';
    let re: RegExp;
    try {
      re = new RegExp(src, flags.includes('i') ? flags : `${flags}i`);
    } catch (e) {
      throw new ParseFailure(`Invalid regex: ${e instanceof Error ? e.message : String(e)}`, start, end);
    }
    return (t) => t != null && re.test(t);
  }
  const values = splitValues(rawValue).map((v) => unquote(v).toLowerCase());
  return (target) => {
    if (target == null) return false;
    const t = target.toLowerCase();
    return values.some((v) => {
      switch (op) {
        case '=':
          return t === v;
        case '!=':
          return t !== v;
        case '^=':
          return t.startsWith(v);
        case '$=':
          return t.endsWith(v);
        default:
          return v.includes('*') || v.includes('?') ? globToRegex(v).test(t) : t.includes(v);
      }
    });
  };
}

function parseNumber(raw: string, unit: 'size' | 'duration' | 'plain', start: number, end: number): number {
  const m = /^(\d+(?:\.\d+)?)([a-z]*)$/i.exec(raw.trim());
  if (!m) throw new ParseFailure(`Expected a number, got "${raw}"`, start, end);
  const n = Number(m[1]);
  const u = (m[2] ?? '').toLowerCase();
  if (unit === 'size') {
    const mult: Record<string, number> = { '': 1, b: 1, kb: 1024, k: 1024, mb: 1024 ** 2, m: 1024 ** 2, gb: 1024 ** 3, g: 1024 ** 3 };
    const f = mult[u];
    if (f == null) throw new ParseFailure(`Unknown size unit "${u}" (use b, kb, mb or gb)`, start, end);
    return n * f;
  }
  if (unit === 'duration') {
    const mult: Record<string, number> = { '': 1, ms: 1, s: 1000, m: 60_000 };
    const f = mult[u];
    if (f == null) throw new ParseFailure(`Unknown duration unit "${u}" (use ms or s)`, start, end);
    return n * f;
  }
  if (u) throw new ParseFailure(`Unexpected unit "${u}"`, start, end);
  return n;
}

function numberMatcher(op: string, raw: string, unit: 'size' | 'duration' | 'plain', start: number, end: number): (v: number | undefined) => boolean {
  const range = /^(\d+[a-z]*)-(\d+[a-z]*)$/i.exec(raw);
  if (range && (op === ':' || op === '=')) {
    const lo = parseNumber(range[1] ?? '', unit, start, end);
    const hi = parseNumber(range[2] ?? '', unit, start, end);
    return (v) => v != null && v >= lo && v <= hi;
  }
  let effOp = op;
  let valueText = raw;
  const inline = /^(>=|<=|>|<|!=|=)(.*)$/.exec(raw);
  if (op === ':' && inline) {
    effOp = inline[1] ?? ':';
    valueText = inline[2] ?? '';
  }
  const values = valueText.split(',').map((v) => parseNumber(v, unit, start, end));
  return (v) => {
    if (v == null) return false;
    return values.some((n) => {
      switch (effOp) {
        case '>':
          return v > n;
        case '>=':
          return v >= n;
        case '<':
          return v < n;
        case '<=':
          return v <= n;
        case '!=':
          return v !== n;
        default:
          return v === n;
      }
    });
  };
}

function statusMatcher(op: string, raw: string, start: number, end: number): Predicate {
  const values = splitValues(raw);
  if (op === ':' && values.every((v) => /^[1-5]xx$/i.test(v))) {
    return (s) => s.summary.status != null && values.some((v) => Math.floor((s.summary.status ?? 0) / 100) === Number(v[0]));
  }
  const m = numberMatcher(op, raw, 'plain', start, end);
  return (s) => m(s.summary.status);
}

function headerValue(headers: Headers | undefined, name: string): string[] {
  if (!headers) return [];
  const lower = name.toLowerCase();
  return headers.filter(([k]) => k.toLowerCase() === lower).map(([, v]) => v);
}

function compileTerm(text: string, start: number, end: number): Predicate {
  const m = OP_RE.exec(text);
  if (!m || text.startsWith('"') || text.startsWith("'")) {
    const needle = unquote(text).toLowerCase();
    return (s) => s.url.toLowerCase().includes(needle) || s.summary.method.toLowerCase() === needle || String(s.summary.status ?? '') === needle;
  }
  let field = (m[1] ?? '').toLowerCase();
  const op = m[2] ?? ':';
  const raw = m[3] ?? '';
  let sub: string | undefined;
  if (field.includes('.')) {
    const [f, s] = field.split('.', 2);
    field = f ?? field;
    sub = s;
  }
  field = FIELD_ALIASES[field] ?? field;
  const valueStart = start + (m[1]?.length ?? 0) + op.length;
  if (!FIELDS.has(field)) {
    throw new ParseFailure(`Unknown field "${m[1]}". Try method, status, host, path, type, size, duration, header, body, source, is:, has:`, start, start + (m[1]?.length ?? 0));
  }
  if (raw === '' && field !== 'header' && field !== 'reqheader' && field !== 'resheader') {
    throw new ParseFailure(`Missing value for "${field}"`, start, end);
  }
  const str = (target: (s: FilterSubject) => string | undefined): Predicate => {
    const match = stringMatcher(op, raw, valueStart, end);
    return (s) => match(target(s));
  };
  switch (field) {
    case 'method':
      return str((s) => s.summary.method);
    case 'status':
      return statusMatcher(op, raw, valueStart, end);
    case 'host':
      return str((s) => s.summary.host);
    case 'port': {
      const n = numberMatcher(op, raw, 'plain', valueStart, end);
      return (s) => n(s.summary.port);
    }
    case 'path':
      return str((s) => s.summary.path.split('?')[0]);
    case 'url':
      return str((s) => s.url);
    case 'query':
      return str((s) => s.summary.path.split('?')[1] ?? '');
    case 'scheme':
      return str((s) => s.summary.scheme);
    case 'version': {
      const match = stringMatcher(op, raw.replace(/^h1$/i, 'HTTP/1').replace(/^h2$/i, 'HTTP/2').replace(/^h3$/i, 'HTTP/3'), valueStart, end);
      return (s) => match(s.summary.httpVersion);
    }
    case 'type': {
      const match = stringMatcher(op === ':' ? '=' : op, raw, valueStart, end);
      const loose = stringMatcher(':', raw, valueStart, end);
      return (s) => match(s.summary.contentKind) || (op === ':' && loose(s.summary.contentType));
    }
    case 'size': {
      const n = numberMatcher(op, raw, 'size', valueStart, end);
      return (s) => n(s.summary.responseSize);
    }
    case 'reqsize': {
      const n = numberMatcher(op, raw, 'size', valueStart, end);
      return (s) => n(s.summary.requestSize);
    }
    case 'duration': {
      const n = numberMatcher(op, raw, 'duration', valueStart, end);
      return (s) => n(s.summary.duration);
    }
    case 'header':
    case 'reqheader':
    case 'resheader': {
      const pick = (s: FilterSubject): Headers => (field === 'reqheader' ? s.requestHeaders : field === 'resheader' ? (s.responseHeaders ?? []) : [...s.requestHeaders, ...(s.responseHeaders ?? [])]);
      if (sub) {
        const match = stringMatcher(op, raw, valueStart, end);
        return (s) => headerValue(pick(s), sub).some((v) => match(v));
      }
      const names = splitValues(raw).map((v) => unquote(v).toLowerCase());
      return (s) => names.every((n) => pick(s).some(([k]) => k.toLowerCase() === n));
    }
    case 'cookie': {
      const match = stringMatcher(':', raw, valueStart, end);
      return (s) => headerValue(s.requestHeaders, 'cookie').some((v) => v.split(';').some((c) => match(c.split('=')[0]?.trim())));
    }
    case 'body':
      return str((s) => `${s.requestBody ?? ''}\n${s.responseBody ?? ''}`);
    case 'reqbody':
      return str((s) => s.requestBody);
    case 'resbody':
      return str((s) => s.responseBody);
    case 'source':
      return str((s) => s.summary.source);
    case 'rule':
      return str((s) => (s.summary.rule ? `${s.summary.rule} ${s.ruleTitle ?? ''}` : undefined));
    case 'id': {
      const n = numberMatcher(op, raw, 'plain', valueStart, end);
      return (s) => n(s.summary.id);
    }
    case 'note':
      return str((s) => s.note);
    case 'color':
      return str((s) => s.summary.color);
    case 'error':
      return str((s) => s.summary.error);
    case 'ip':
      return str((s) => s.clientIp);
    case 'model':
      return str((s) => s.summary.llm?.model);
    case 'provider': {
      // Whole-value match like the daemon's enumerated fields (`*` globs still work).
      if (op === ':' && !raw.includes('*') && !raw.startsWith('/')) {
        const wanted = splitValues(raw).map((v) => v.toLowerCase());
        return (s) => s.summary.llm != null && wanted.includes(s.summary.llm.provider.toLowerCase());
      }
      return str((s) => s.summary.llm?.provider);
    }
    case 'is': {
      const flags = splitValues(raw).map((v) => v.toLowerCase());
      for (const f of flags) if (!FLAGS.includes(f)) throw new ParseFailure(`Unknown flag "is:${f}". Known: ${FLAGS.join(', ')}`, valueStart, end);
      return (s) =>
        flags.some((f) => {
          const x = s.summary;
          switch (f) {
            case 'pending':
              return x.state === 'requesting' || x.state === 'waiting' || x.state === 'responding';
            case 'complete':
              return x.state === 'complete';
            case 'error':
            case 'failed':
              return x.error != null || (x.status != null && x.status >= 400);
            case 'aborted':
              return x.state === 'aborted';
            case 'paused':
              return x.paused != null;
            case 'pinned':
              return !!x.pinned;
            case 'mocked':
              return !!x.mocked;
            case 'modified':
              return !!x.modified;
            case 'websocket':
            case 'ws':
              return x.kind === 'web-socket';
            case 'sse':
              return x.kind === 'event-stream';
            case 'tls-failure':
              return x.kind === 'tls-failure';
            case 'tunnel':
              return x.kind === 'tunnel';
            case 'http':
              return x.scheme === 'http' || x.scheme === 'ws';
            case 'https':
              return x.scheme === 'https' || x.scheme === 'wss';
            case 'highlighted':
              return x.color != null;
            case 'llm':
            case 'ai':
              return x.llm != null;
            default:
              return false;
          }
        });
    }
    case 'has': {
      const things = splitValues(raw).map((v) => v.toLowerCase());
      for (const t of things) if (!HAS.includes(t)) throw new ParseFailure(`Unknown "has:${t}". Known: ${HAS.join(', ')}`, valueStart, end);
      return (s) =>
        things.every((t) => {
          switch (t) {
            case 'note':
              return !!s.summary.hasNote;
            case 'body':
              return s.summary.requestSize > 0 || s.summary.responseSize > 0;
            case 'reqbody':
              return s.summary.requestSize > 0;
            case 'resbody':
              return s.summary.responseSize > 0;
            case 'cookie':
              return headerValue(s.requestHeaders, 'cookie').length > 0;
            case 'auth':
              return headerValue(s.requestHeaders, 'authorization').length > 0;
            default:
              return false;
          }
        });
    }
    default:
      throw new ParseFailure(`Field "${field}" is not supported here`, start, end);
  }
}

function parse(tokens: Token[], q: string): Predicate {
  let pos = 0;
  const peek = () => tokens[pos];

  const parseOr = (): Predicate => {
    const parts = [parseAnd()];
    while (peek()?.t === 'or') {
      pos++;
      if (!peek() || peek()?.t === 'rparen') throw new ParseFailure('Expected an expression after OR', q.length, q.length);
      parts.push(parseAnd());
    }
    return parts.length === 1 ? (parts[0] as Predicate) : (s) => parts.some((p) => p(s));
  };
  const parseAnd = (): Predicate => {
    const parts = [parseUnary()];
    for (;;) {
      const t = peek();
      if (!t || t.t === 'or' || t.t === 'rparen') break;
      if (t.t === 'and') {
        pos++;
        continue;
      }
      parts.push(parseUnary());
    }
    return parts.length === 1 ? (parts[0] as Predicate) : (s) => parts.every((p) => p(s));
  };
  const parseUnary = (): Predicate => {
    const t = peek();
    if (!t) throw new ParseFailure('Unexpected end of query', q.length, q.length);
    if (t.t === 'not') {
      pos++;
      const inner = parseUnary();
      return (s) => !inner(s);
    }
    if (t.t === 'lparen') {
      pos++;
      const inner = parseOr();
      if (peek()?.t !== 'rparen') throw new ParseFailure('Missing closing parenthesis', t.start, q.length);
      pos++;
      return inner;
    }
    if (t.t === 'term') {
      pos++;
      return compileTerm(t.text, t.start, t.end);
    }
    throw new ParseFailure(`Unexpected "${q.slice(t.start, t.end)}"`, t.start, t.end);
  };

  const result = parseOr();
  const rest = peek();
  if (rest) throw new ParseFailure(`Unexpected "${q.slice(rest.start, rest.end)}"`, rest.start, rest.end);
  return result;
}

/** Compile a query. Empty/whitespace = match everything (`predicate: null`). */
export function compileFilter(query: string): { ok: true; predicate: Predicate | null } | { ok: false; error: FilterError } {
  if (!query.trim()) return { ok: true, predicate: null };
  try {
    return { ok: true, predicate: parse(tokenize(query), query) };
  } catch (e) {
    if (e instanceof ParseFailure) {
      // Convert UTF-16 indices to UTF-8 byte offsets, like the daemon.
      return { ok: false, error: { message: e.message, start: utf8Length(query.slice(0, e.start)), end: utf8Length(query.slice(0, e.end)) } };
    }
    return { ok: false, error: { message: e instanceof Error ? e.message : 'Invalid filter', start: 0, end: utf8Length(query) } };
  }
}

/** Values known to the mock store, for value completion. */
export interface SuggestValues {
  hosts: string[];
  methods: string[];
  sources: string[];
  /** LLM models / providers seen in traffic. */
  models?: string[];
  providers?: string[];
}

const LLM_PROVIDERS = ['openai', 'anthropic', 'google', 'vertex-ai', 'azure-openai', 'aws-bedrock', 'openrouter', 'ollama', 'lmstudio', 'groq', 'mistral', 'deepseek', 'xai', 'together', 'fireworks', 'perplexity', 'openai-compatible'];

/** Autocomplete at a UTF-8 byte cursor (simplified akev-filter `suggest`). */
export function suggestFilter(query: string, byteCursor: number, values: SuggestValues): Suggestion[] {
  // Map the byte cursor back to a UTF-16 index.
  let cursor = 0;
  for (let bytes = 0; cursor < query.length && bytes < byteCursor; cursor++) bytes += utf8Length(query[cursor] as string);
  let start = cursor;
  while (start > 0 && !/[\s()]/.test(query[start - 1] as string)) start--;
  let word = query.slice(start, cursor);
  let negPrefix = '';
  if (word.startsWith('-') || word.startsWith('!')) {
    negPrefix = word[0] as string;
    word = word.slice(1);
  }
  const replaceStart = utf8Length(query.slice(0, start + negPrefix.length));
  const replaceEnd = utf8Length(query.slice(0, cursor));
  const out: Suggestion[] = [];
  const m = /^([a-zA-Z][\w-]*)(:|=|\^=|\$=|\*=|~|>=|<=|>|<)(.*)$/.exec(word);
  if (!m) {
    const prefix = word.toLowerCase();
    for (const d of FILTER_DOCS) {
      if (!d.name.startsWith(prefix) && !d.aliases.some((a) => a.startsWith(prefix))) continue;
      out.push({ label: `${d.name}:`, detail: d.description, insert: `${d.name}:`, replaceStart, replaceEnd, kind: d.name === 'is' || d.name === 'has' ? 'flag' : 'field' });
    }
    return out.slice(0, 30);
  }
  const field = FIELD_ALIASES[(m[1] ?? '').toLowerCase()] ?? (m[1] ?? '').toLowerCase();
  const partial = (m[3] ?? '').split(',').pop()?.toLowerCase() ?? '';
  const head = word.slice(0, word.length - partial.length);
  let candidates: [string, string][] = [];
  switch (field) {
    case 'method':
      candidates = values.methods.map((v) => [v.toLowerCase(), 'method']);
      break;
    case 'status':
      candidates = [
        ['2xx', 'Success'],
        ['3xx', 'Redirect'],
        ['4xx', 'Client error'],
        ['5xx', 'Server error'],
        ['>=400', 'Any error status'],
        ['200', 'OK'],
        ['404', 'Not Found'],
        ['500', 'Internal Server Error'],
      ];
      break;
    case 'host':
      candidates = values.hosts.map((h) => [h, 'host']);
      break;
    case 'source':
      candidates = values.sources.map((s) => [s.toLowerCase(), 'client']);
      break;
    case 'model':
      candidates = (values.models ?? []).map((v) => [v, 'Model (seen in traffic)']);
      break;
    case 'provider':
      candidates = [...new Set([...(values.providers ?? []), ...LLM_PROVIDERS])].map((v) => [v, values.providers?.includes(v) ? 'LLM provider (seen in traffic)' : 'LLM provider']);
      break;
    case 'type':
      candidates = ['json', 'html', 'javascript', 'css', 'image', 'font', 'xml', 'protobuf', 'grpc', 'form-url-encoded', 'multipart', 'event-stream', 'binary', 'video', 'pdf'].map((t) => [t, 'content kind']);
      break;
    case 'is':
      candidates = FLAGS.map((f) => [f, 'flag']);
      break;
    case 'has':
      candidates = HAS.map((h) => [h, 'presence']);
      break;
    case 'scheme':
      candidates = ['http', 'https', 'ws', 'wss'].map((s) => [s, 'scheme']);
      break;
    case 'version':
      candidates = [
        ['h1', 'HTTP/1.x'],
        ['h2', 'HTTP/2'],
      ];
      break;
    case 'size':
      candidates = [
        ['>1mb', 'Larger than 1 MB'],
        ['>100kb', 'Larger than 100 KB'],
        ['<1kb', 'Smaller than 1 KB'],
      ];
      break;
    case 'duration':
      candidates = [
        ['>1s', 'Slower than 1 second'],
        ['>500ms', 'Slower than 500 ms'],
        ['<50ms', 'Faster than 50 ms'],
      ];
      break;
    default:
      candidates = (FILTER_DOCS.find((d) => d.name === field)?.examples ?? []).map((e) => [e.split(/[:=~<>]+/).slice(1).join(':'), 'example']);
  }
  for (const [value, detail] of candidates) {
    if (!value || !value.toLowerCase().startsWith(partial)) continue;
    out.push({ label: value, detail, insert: `${head}${value}`, replaceStart, replaceEnd, kind: field === 'is' || field === 'has' ? 'flag' : 'value' });
  }
  return out.slice(0, 30);
}
