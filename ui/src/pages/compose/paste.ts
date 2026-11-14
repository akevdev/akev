/**
 * Paste detection for the URL bar: cURL commands (converted by the daemon via
 * `POST /api/import/curl`) and `fetch(…)` calls (parsed here — Chrome/Firefox "Copy as fetch").
 * Multiple commands ("Copy all as cURL") open one tab each.
 */

import { textToBase64 } from '../../lib/encoding';
import type { EditableRequest } from '../../api/types';

const CURL_START = /^\s*(?:\$\s+)?curl(?:\.exe)?\s+\S/i;
const FETCH_START = /^\s*(?:await\s+)?fetch\s*\(/;

/** Whether pasted text is a cURL command. */
export function looksLikeCurl(text: string): boolean {
  return CURL_START.test(text);
}

/** Whether pasted text is a `fetch(…)` call. */
export function looksLikeFetch(text: string): boolean {
  return FETCH_START.test(text);
}

const PS_START = /^\s*(?:\$session\s*=|(?:Invoke-WebRequest|Invoke-RestMethod|iwr|irm)\s)/i;
const PS_INVOKE = /(?:^|[\s;(])(?:Invoke-WebRequest|Invoke-RestMethod|iwr|irm)\s/i;

/** Whether pasted text is a PowerShell `Invoke-WebRequest` (DevTools "Copy as PowerShell"). */
export function looksLikePowerShell(text: string): boolean {
  return PS_START.test(text) && PS_INVOKE.test(text);
}

export type PasteKind = 'curl' | 'fetch' | 'powershell' | null;

export function detectPaste(text: string): PasteKind {
  if (looksLikeCurl(text)) return 'curl';
  if (looksLikeFetch(text)) return 'fetch';
  if (looksLikePowerShell(text)) return 'powershell';
  return null;
}

/** Human label for a paste kind ("cURL command"). */
export function pasteKindLabel(kind: Exclude<PasteKind, null>, count = 1): string {
  const base = kind === 'curl' ? 'cURL command' : kind === 'fetch' ? 'fetch() call' : 'PowerShell command';
  return count === 1 ? base : `${base}s`;
}

/** Strip a leading shell prompt and trailing separators (`;`, `&&`). */
function cleanCommand(cmd: string): string {
  return cmd
    .trim()
    .replace(/^\$\s+/, '')
    .replace(/\s*(?:;|&&)\s*$/, '')
    .trim();
}

/**
 * Split text holding one or more cURL commands (bash `\` / cmd `^` / PowerShell `` ` ``
 * continuations, `;`/`&&` separators, one command per line) into individual commands.
 */
export function splitCurlCommands(text: string): string[] {
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  const commands: string[] = [];
  let current: string[] = [];
  let continued = false;
  for (const line of lines) {
    const startsNew = CURL_START.test(line) && !continued;
    if (startsNew && current.length) {
      commands.push(current.join('\n'));
      current = [];
    }
    if (current.length || startsNew || line.trim()) current.push(line);
    continued = /(?:\\|\^|`)\s*$/.test(line);
  }
  if (current.length) commands.push(current.join('\n'));
  return commands.map(cleanCommand).filter((c) => looksLikeCurl(c));
}

/** Split text holding one or more `fetch(…)` calls. */
export function splitFetchCalls(text: string): string[] {
  const out: string[] = [];
  const re = /(?:^|[;\n])\s*((?:await\s+)?fetch\s*\()/g;
  const starts: number[] = [];
  for (const m of text.matchAll(re)) starts.push((m.index ?? 0) + m[0].indexOf(m[1] as string));
  starts.forEach((s, i) => out.push(cleanCommand(text.slice(s, starts[i + 1] ?? text.length))));
  return out.filter(looksLikeFetch);
}

// ---------------------------------------------------------------------------------- fetch

/** Read a JS string literal starting at `i` (quote char at `text[i]`). Returns [value, endIndex]. */
function readString(text: string, i: number): [string, number] | null {
  const quote = text[i];
  if (quote !== '"' && quote !== "'" && quote !== '`') return null;
  let out = '';
  for (let j = i + 1; j < text.length; j++) {
    const ch = text[j] as string;
    if (ch === '\\') {
      const next = text[j + 1] ?? '';
      const map: Record<string, string> = { n: '\n', r: '\r', t: '\t', b: '\b', f: '\f', v: '\v', '0': '\0' };
      if (next === 'u') {
        const braced = /^\{([0-9a-fA-F]+)\}/.exec(text.slice(j + 2));
        if (braced) {
          out += String.fromCodePoint(parseInt(braced[1] as string, 16));
          j += 2 + braced[0].length - 1;
        } else {
          out += String.fromCharCode(parseInt(text.slice(j + 2, j + 6), 16));
          j += 5;
        }
      } else if (next === 'x') {
        out += String.fromCharCode(parseInt(text.slice(j + 2, j + 4), 16));
        j += 3;
      } else if (next === '\n') {
        j += 1;
      } else {
        out += map[next] ?? next;
        j += 1;
      }
      continue;
    }
    if (ch === quote) return [out, j];
    out += ch;
  }
  return null;
}

/** Find the index of the bracket closing the one at `open` (string-aware). */
function matchBracket(text: string, open: number): number {
  const pairs: Record<string, string> = { '{': '}', '[': ']', '(': ')' };
  const stack: string[] = [];
  for (let i = open; i < text.length; i++) {
    const ch = text[i] as string;
    if (ch === '"' || ch === "'" || ch === '`') {
      const s = readString(text, i);
      if (!s) return -1;
      i = s[1];
      continue;
    }
    if (pairs[ch]) stack.push(pairs[ch] as string);
    else if (ch === stack[stack.length - 1]) {
      stack.pop();
      if (stack.length === 0) return i;
    }
  }
  return -1;
}

/** Convert a JS object literal (unquoted keys, single quotes, trailing commas) to a value. */
function parseLooseObject(src: string): unknown {
  try {
    return JSON.parse(src);
  } catch {
    // Fall through to the tolerant path.
  }
  let json = '';
  for (let i = 0; i < src.length; i++) {
    const ch = src[i] as string;
    if (ch === '"' || ch === "'" || ch === '`') {
      const s = readString(src, i);
      if (!s) throw new Error('Unterminated string');
      json += JSON.stringify(s[0]);
      i = s[1];
    } else if (/[A-Za-z_$]/.test(ch)) {
      let j = i;
      while (j < src.length && /[\w$-]/.test(src[j] as string)) j++;
      const word = src.slice(i, j);
      const rest = src.slice(j).trimStart();
      if (rest.startsWith(':')) json += JSON.stringify(word);
      else if (word === 'true' || word === 'false' || word === 'null') json += word;
      else if (word === 'undefined') json += 'null';
      else throw new Error(`Unsupported expression "${word}"`);
      i = j - 1;
    } else json += ch;
  }
  json = json.replace(/,(\s*[}\]])/g, '$1');
  return JSON.parse(json);
}

export interface ParsedFetch {
  method: string;
  url: string;
  headers: [string, string][];
  body: string | null;
}

/** Parse `fetch("url", { method, headers, body })`. Throws with a readable message. */
export function parseFetch(text: string): ParsedFetch {
  const m = FETCH_START.exec(text);
  if (!m) throw new Error('Not a fetch() call');
  let i = m[0].length;
  while (/\s/.test(text[i] ?? '')) i++;
  const urlLit = readString(text, i);
  if (!urlLit) throw new Error('The first fetch() argument must be a string URL');
  const url = urlLit[0];
  i = urlLit[1] + 1;
  while (/\s/.test(text[i] ?? '')) i++;
  let init: Record<string, unknown> = {};
  if (text[i] === ',') {
    i++;
    while (/\s/.test(text[i] ?? '')) i++;
    if (text[i] === '{') {
      const end = matchBracket(text, i);
      if (end < 0) throw new Error('Unbalanced braces in fetch() options');
      const parsed = parseLooseObject(text.slice(i, end + 1));
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) init = parsed as Record<string, unknown>;
    }
  }
  const headers: [string, string][] = [];
  const h = init.headers;
  if (Array.isArray(h)) {
    for (const pair of h) if (Array.isArray(pair) && pair.length >= 2) headers.push([String(pair[0]), String(pair[1])]);
  } else if (h && typeof h === 'object') {
    for (const [k, v] of Object.entries(h as Record<string, unknown>)) headers.push([k, String(v)]);
  }
  const body = typeof init.body === 'string' ? init.body : init.body == null ? null : JSON.stringify(init.body);
  const method = typeof init.method === 'string' && init.method ? init.method.toUpperCase() : body != null ? 'POST' : 'GET';
  return { method, url, headers, body };
}

/** `fetch(…)` → EditableRequest. */
export function fetchToEditable(text: string): EditableRequest {
  const f = parseFetch(text);
  return { method: f.method, url: f.url, headers: f.headers, body: f.body != null ? textToBase64(f.body) : '' };
}

// ---------------------------------------------------------------------------------- PowerShell

/**
 * Split "Copy all as PowerShell" output: each command starts at `$session = New-Object …`
 * (which owns the following Invoke-WebRequest) or at a bare Invoke-WebRequest.
 */
export function splitPowerShellCommands(text: string): string[] {
  const src = text.replace(/\r\n?/g, '\n');
  const starts: number[] = [];
  let sessionOpen = false;
  for (const m of src.matchAll(/^[ \t]*(\$session\s*=\s*New-Object|Invoke-WebRequest|Invoke-RestMethod|iwr\s|irm\s)/gim)) {
    if ((m[1] as string).startsWith('$')) {
      starts.push(m.index ?? 0);
      sessionOpen = true;
    } else {
      if (!sessionOpen) starts.push(m.index ?? 0);
      sessionOpen = false;
    }
  }
  return starts.map((st, i) => src.slice(st, starts[i + 1] ?? src.length).trim()).filter(looksLikePowerShell);
}

const PS_ESCAPES: Readonly<Record<string, string>> = { '0': '\0', a: '\x07', b: '\b', f: '\f', n: '\n', r: '\r', t: '\t', v: '\v' };

/** Read a PowerShell string literal at `i` ("…" with backtick escapes, '…' with '' escapes). */
function readPsString(text: string, i: number): [string, number] | null {
  const q = text[i];
  if (q !== '"' && q !== "'") return null;
  let out = '';
  for (let j = i + 1; j < text.length; j++) {
    const ch = text[j] as string;
    if (q === '"' && ch === '`') {
      const next = text[j + 1] ?? '';
      out += PS_ESCAPES[next] ?? next;
      j++;
      continue;
    }
    if (ch === q) {
      if (text[j + 1] === q) {
        out += q;
        j++;
        continue;
      }
      return [out, j];
    }
    out += ch;
  }
  return null;
}

type PsToken = { kind: 'param'; name: string } | { kind: 'string'; value: string } | { kind: 'hash'; entries: [string, string][] } | { kind: 'word'; value: string };

/** Index of the bracket closing the one at `open`, skipping string literals. */
function closePs(text: string, open: number, openCh: string, closeCh: string): number {
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    const ch = text[i] as string;
    if (ch === '"' || ch === "'") {
      const s = readPsString(text, i);
      if (!s) return -1;
      i = s[1];
      continue;
    }
    if (ch === openCh) depth++;
    else if (ch === closeCh && --depth === 0) return i;
  }
  return -1;
}

function parseHashtable(body: string): [string, string][] {
  const out: [string, string][] = [];
  let i = 0;
  const value = (): string => {
    while (/[ \t]/.test(body[i] ?? '')) i++;
    const s = readPsString(body, i);
    if (s) {
      i = s[1] + 1;
      return s[0];
    }
    const start = i;
    while (i < body.length && !/[\n;]/.test(body[i] as string)) i++;
    return body.slice(start, i).trim();
  };
  while (i < body.length) {
    while (/[\s;`]/.test(body[i] ?? '')) i++;
    if (i >= body.length) break;
    const key = value();
    while (/[ \t]/.test(body[i] ?? '')) i++;
    if (body[i] !== '=') {
      // Not `key = value`; skip to the next line.
      while (i < body.length && body[i] !== '\n') i++;
      continue;
    }
    i++;
    out.push([key.replace(/^['"]|['"]$/g, ''), value()]);
  }
  return out;
}

/** Tokenize the arguments of an Invoke-WebRequest call (stops at the end of the command). */
function tokenizePs(text: string): PsToken[] {
  const tokens: PsToken[] = [];
  let i = 0;
  while (i < text.length) {
    const ch = text[i] as string;
    if (ch === '`' && /^`[ \t]*\r?\n/.test(text.slice(i))) {
      i = text.indexOf('\n', i) + 1;
      continue;
    }
    if (ch === ' ' || ch === '\t') {
      i++;
      continue;
    }
    if (ch === '\n' || ch === ';' || ch === '|') break;
    if (ch === '-' && /[A-Za-z]/.test(text[i + 1] ?? '')) {
      const m = /^-([A-Za-z][\w-]*)/.exec(text.slice(i)) as RegExpExecArray;
      tokens.push({ kind: 'param', name: (m[1] as string).toLowerCase() });
      i += m[0].length;
      continue;
    }
    if (ch === '"' || ch === "'") {
      const s = readPsString(text, i);
      if (!s) throw new Error('Unterminated string in the PowerShell command');
      tokens.push({ kind: 'string', value: s[0] });
      i = s[1] + 1;
      continue;
    }
    if (ch === '@' && text[i + 1] === '{') {
      const end = closePs(text, i + 1, '{', '}');
      if (end < 0) throw new Error('Unbalanced @{ } in the PowerShell command');
      tokens.push({ kind: 'hash', entries: parseHashtable(text.slice(i + 2, end)) });
      i = end + 1;
      continue;
    }
    if (ch === '(') {
      // e.g. -Body ([System.Text.Encoding]::UTF8.GetBytes("…")): take its first string literal.
      const end = closePs(text, i, '(', ')');
      if (end < 0) throw new Error('Unbalanced parentheses in the PowerShell command');
      const inner = text.slice(i + 1, end);
      const q = inner.search(/["']/);
      const s = q >= 0 ? readPsString(inner, q) : null;
      tokens.push(s ? { kind: 'string', value: s[0] } : { kind: 'word', value: inner.trim() });
      i = end + 1;
      continue;
    }
    const m = /^[^\s;|]+/.exec(text.slice(i)) as RegExpExecArray;
    tokens.push({ kind: 'word', value: m[0] });
    i += m[0].length;
  }
  return tokens;
}

/** Chrome's PowerShell export writes HTTP/2 pseudo-headers as plain names. */
const PS_PSEUDO = new Set(['authority', 'method', 'path', 'scheme']);

/** Parse an `Invoke-WebRequest` / `Invoke-RestMethod` command (with its `$session` setup). */
export function powershellToEditable(text: string): EditableRequest {
  const src = text.replace(/\r\n?/g, '\n');
  const headers: [string, string][] = [];
  const ua = /\$session\.UserAgent\s*=\s*/.exec(src);
  if (ua) {
    const s = readPsString(src, (ua.index ?? 0) + ua[0].length);
    if (s) headers.push(['User-Agent', s[0]]);
  }
  const cookies: string[] = [];
  for (const m of src.matchAll(/System\.Net\.Cookie\(\s*/g)) {
    const at = (m.index ?? 0) + m[0].length;
    const name = readPsString(src, at);
    if (!name) continue;
    const rest = src.slice(name[1] + 1);
    const comma = /^\s*,\s*/.exec(rest);
    const value = comma ? readPsString(rest, comma[0].length) : null;
    if (value) cookies.push(`${name[0]}=${value[0]}`);
  }
  const invoke = /(?:Invoke-WebRequest|Invoke-RestMethod|\biwr|\birm)\s/i.exec(src);
  if (!invoke) throw new Error('No Invoke-WebRequest call found');
  const tokens = tokenizePs(src.slice((invoke.index ?? 0) + invoke[0].length));

  let url = '';
  let method = '';
  let body: string | null = null;
  for (let k = 0; k < tokens.length; k++) {
    const t = tokens[k] as PsToken;
    if (t.kind !== 'param') {
      if (!url && (t.kind === 'string' || t.kind === 'word')) url = t.value;
      continue;
    }
    const next = tokens[k + 1];
    if (!next || next.kind === 'param') continue; // a switch (-UseBasicParsing)
    k++;
    const val = next.kind === 'string' || next.kind === 'word' ? next.value : '';
    switch (t.name) {
      case 'uri':
        url = val;
        break;
      case 'method':
        method = val.toUpperCase();
        break;
      case 'contenttype':
        headers.push(['Content-Type', val]);
        break;
      case 'useragent':
        headers.push(['User-Agent', val]);
        break;
      case 'body':
        body = val;
        break;
      case 'headers':
        if (next.kind === 'hash') for (const [name, value] of next.entries) if (!PS_PSEUDO.has(name.toLowerCase())) headers.push([name, value]);
        break;
    }
  }
  if (!url) throw new Error('The PowerShell command has no -Uri');
  if (cookies.length && !headers.some(([n]) => n.toLowerCase() === 'cookie')) headers.push(['Cookie', cookies.join('; ')]);
  // A Content-Type given both as -ContentType and in -Headers: keep one.
  const seen = new Set<string>();
  const unique = headers.filter(([n]) => {
    const key = n.toLowerCase();
    if (key !== 'content-type' && key !== 'user-agent') return true;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
  return { method: method || (body != null ? 'POST' : 'GET'), url, headers: unique, body: body != null ? textToBase64(body) : '' };
}
