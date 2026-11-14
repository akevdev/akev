/**
 * Client-side rule validation, mirroring what the daemon rejects at compile time
 * (`akev-rules` engine: regexes, header names, statuses, forward targets, JSON patches,
 * script syntax…) plus a few UI-level checks that catch rules which can never match
 * (empty method lists, blank keys). Issues carry a dotted `path` into the rule so the editor
 * shows each message next to its field. The daemon remains authoritative: its per-rule
 * errors are shown too.
 */

import type { Action, BodySource, BodyTransform, KeyValueMatch, MatchReplace, Matcher, RequestTransform, ResponseTransform, Rule, StringMatch } from '../../api/types';
import { isTlsProfile } from '../tlsProfiles';

export interface ValidationIssue {
  /** Dotted path into the rule, e.g. `matchers.0.pattern.value`, `action.status`. */
  path: string;
  message: string;
}

// =====================================================================================
// field validators (exported for inline use)
// =====================================================================================

/**
 * Compile a Rust-`regex`-syntax pattern with the JS engine (best effort). Returns an error
 * message or `null`. Rust-only syntax (`(?P<name>…)`, leading `(?i)` flags, `\A`, `\z`) is
 * translated; JS-only features Rust lacks (look-around, back-references) are reported.
 */
export function regexError(source: string, caseInsensitive = false): string | null {
  if (/\(\?<?[=!]/.test(source)) return "Look-around ((?=…), (?!…), (?<=…)) isn't supported";
  if (/(^|[^\\])\\[1-9]/.test(source) || /\\k</.test(source)) return "Back-references aren't supported";
  let src = source.replace(/\(\?P</g, '(?<').replace(/\\A/g, '^').replace(/\\z/g, '$');
  let flags = caseInsensitive ? 'i' : '';
  const inline = /^\(\?([imsxU]+)\)/.exec(src);
  if (inline) {
    src = src.slice(inline[0].length);
    if (inline[1]?.includes('i') && !flags.includes('i')) flags += 'i';
    if (inline[1]?.includes('s')) flags += 's';
    if (inline[1]?.includes('m')) flags += 'm';
  }
  src = src
    .replace(/\[:alpha:\]/g, 'a-zA-Z')
    .replace(/\[:digit:\]/g, '0-9')
    .replace(/\[:alnum:\]/g, 'a-zA-Z0-9')
    .replace(/\[:space:\]/g, '\\s')
    .replace(/\[:upper:\]/g, 'A-Z')
    .replace(/\[:lower:\]/g, 'a-z')
    .replace(/\[:xdigit:\]/g, '0-9a-fA-F')
    .replace(/\[:punct:\]/g, '!-\\/:-@\\[-`{-~');
  try {
    new RegExp(src, /\\[pP]\{/.test(src) ? `${flags}u` : flags);
    return null;
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    // "Invalid regular expression: /x(/: Unterminated group" → "Unterminated group".
    const m = /: ([^:]+)$/.exec(msg);
    return `Invalid regex — ${(m?.[1] ?? msg).trim().replace(/^\w/, (c) => c.toLowerCase())}`;
  }
}

/** RFC 9110 token (header names, methods). */
export function isToken(s: string): boolean {
  return s.length > 0 && /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(s);
}

export function headerNameError(name: string): string | null {
  if (!name.trim()) return 'Header name is empty';
  return isToken(name) ? null : `“${name}” isn't a valid header name`;
}

export function headerValueError(value: string): string | null {
  return /[\r\n\0]/.test(value) ? "Header values can't contain line breaks" : null;
}

export function statusError(status: number | null | undefined): string | null {
  if (status == null || !Number.isFinite(status)) return 'Enter a status code';
  if (!Number.isInteger(status) || status < 100 || status > 999) return 'Status must be between 100 and 999';
  return null;
}

/** Mirrors `respond::parse_forward_target`. */
export function forwardTargetError(target: string): string | null {
  const t = target.trim();
  if (!t) return 'Enter a target URL or host';
  let authority: string;
  if (t.includes('://')) {
    const m = /^([a-zA-Z][a-zA-Z0-9+.-]*):\/\/([^/?#]*)/.exec(t);
    if (!m) return 'Not a valid URL';
    const scheme = (m[1] as string).toLowerCase();
    if (!['http', 'https', 'ws', 'wss'].includes(scheme)) return `Unsupported scheme “${scheme}” — use http, https, ws or wss`;
    authority = m[2] as string;
  } else {
    const end = t.search(/[/?]/);
    authority = end < 0 ? t : t.slice(0, end);
  }
  if (!authority) return 'The target needs a host';
  if (/[\s@/]/.test(authority)) return 'The host contains invalid characters';
  const v6 = /^\[([0-9a-fA-F:.]+)\](?::(\d*))?$/.exec(authority);
  const plain = /^([^:]+)(?::(\d*))?$/.exec(authority);
  const port = v6 ? v6[2] : plain ? plain[2] : undefined;
  if (!v6 && !plain) return 'The host contains invalid characters';
  if (port !== undefined && (port === '' || Number(port) > 65535)) return 'Invalid port';
  return null;
}

/** Mirrors `json::validate_patch` (RFC 6902 operation shape). */
export function jsonPatchError(ops: unknown): string | null {
  if (!Array.isArray(ops)) return 'A JSON patch is an array of operations';
  for (let i = 0; i < ops.length; i++) {
    const op = ops[i] as Record<string, unknown> | null;
    if (!op || typeof op !== 'object' || Array.isArray(op)) return `Operation #${i} is not an object`;
    const kind = op.op;
    if (typeof kind !== 'string') return `Operation #${i} has no "op"`;
    if (!['add', 'remove', 'replace', 'move', 'copy', 'test'].includes(kind)) return `Operation #${i}: unknown op "${kind}"`;
    if (typeof op.path !== 'string') return `Operation #${i} (${kind}) has no "path"`;
    if (op.path !== '' && !op.path.startsWith('/')) return `Operation #${i}: "path" must start with /`;
    if ((kind === 'move' || kind === 'copy') && typeof op.from !== 'string') return `Operation #${i} (${kind}) has no "from"`;
    if ((kind === 'move' || kind === 'copy') && typeof op.from === 'string' && op.from !== '' && !op.from.startsWith('/')) return `Operation #${i}: "from" must start with /`;
    if ((kind === 'add' || kind === 'replace' || kind === 'test') && !('value' in op)) return `Operation #${i} (${kind}) has no "value"`;
    if (kind === 'move' && typeof op.from === 'string' && op.path.startsWith(`${op.from}/`)) return `Operation #${i}: can't move a value into itself`;
  }
  return null;
}

export function base64Error(data: string): string | null {
  const s = data.replace(/\s+/g, '');
  if (s.length % 4 === 1 || !/^[A-Za-z0-9+/_-]*={0,2}$/.test(s)) return 'Not valid base64';
  return null;
}

/** Parse JSON text, returning the value or an error message with the line number. */
export function parseJson(text: string): { ok: true; value: unknown } | { ok: false; error: string } {
  if (!text.trim()) return { ok: false, error: 'Enter a JSON value' };
  try {
    return { ok: true, value: JSON.parse(text) as unknown };
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    const pos = /position (\d+)/.exec(msg);
    if (pos) {
      const offset = Number(pos[1]);
      const line = text.slice(0, offset).split('\n').length;
      return { ok: false, error: `Invalid JSON on line ${line} — ${msg.replace(/^JSON\.parse: /, '').replace(/ in JSON at position \d+.*$/, '').replace(/\s*\(line \d+ column \d+\)/, '')}` };
    }
    return { ok: false, error: `Invalid JSON — ${msg}` };
  }
}

/** False once the page's Content-Security-Policy has refused `new Function`. */
let canCompile = true;

/** Compile-check a script (syntax only; nothing runs) and require a hook. */
export function scriptError(source: string): string | null {
  if (!/\bonRequest\b|\bonResponse\b/.test(source)) return 'Define onRequest(req) and/or onResponse(req, res)';
  // The daemon's CSP forbids eval: it checks the syntax itself when the rule is saved. Don't
  // retry on every keystroke (each refusal logs a CSP violation).
  if (!canCompile) return null;
  try {
    // biome-ignore lint/security/noGlobalEval: compiles the user's own script rule to check its syntax
    new Function(source);
    return null;
  } catch (e) {
    if (e instanceof SyntaxError) return `Syntax error — ${e.message}`;
    canCompile = false;
    return null;
  }
}

// =====================================================================================
// rule validation
// =====================================================================================

function checkPattern(p: StringMatch, path: string, out: ValidationIssue[], requireValue = true): void {
  if (p.op === 'regex') {
    if (!p.value) out.push({ path: `${path}.value`, message: 'Enter a regular expression' });
    else {
      const err = regexError(p.value, !p.caseSensitive);
      if (err) out.push({ path: `${path}.value`, message: err });
    }
  } else if (requireValue && p.op !== 'exists' && p.value === '') {
    out.push({ path: `${path}.value`, message: 'Enter a value' });
  }
}

function checkKvs(kvs: readonly KeyValueMatch[], path: string, out: ValidationIssue[], header = false): void {
  if (kvs.length === 0) out.push({ path, message: 'Add at least one entry' });
  kvs.forEach((kv, i) => {
    if (!kv.key.trim()) out.push({ path: `${path}.${i}.key`, message: 'Enter a name' });
    else if (header && !isToken(kv.key)) out.push({ path: `${path}.${i}.key`, message: `“${kv.key}” isn't a valid header name` });
    if (kv.value) checkPattern(kv.value, `${path}.${i}.value`, out, false);
  });
}

export function validateMatcher(m: Matcher, path: string, out: ValidationIssue[] = []): ValidationIssue[] {
  switch (m.type) {
    case 'method':
      if (m.methods.length === 0) out.push({ path: `${path}.methods`, message: 'Select at least one method' });
      m.methods.forEach((x, i) => {
        if (!isToken(x)) out.push({ path: `${path}.methods.${i}`, message: `“${x}” isn't a valid method` });
      });
      break;
    case 'url':
    case 'host':
    case 'path':
    case 'body':
    case 'graphql-operation':
    case 'source':
      checkPattern(m.pattern, `${path}.pattern`, out);
      break;
    case 'port':
      if (m.ports.length === 0) out.push({ path: `${path}.ports`, message: 'Add at least one port' });
      if (m.ports.some((p) => !Number.isInteger(p) || p < 1 || p > 65535)) out.push({ path: `${path}.ports`, message: 'Ports are 1–65535' });
      break;
    case 'protocol':
      if (m.protocols.length === 0) out.push({ path: `${path}.protocols`, message: 'Select at least one protocol' });
      if (m.protocols.some((p) => !['http', 'https', 'ws', 'wss'].includes(p.trim().toLowerCase())))
        out.push({ path: `${path}.protocols`, message: 'Protocols are http, https, ws or wss' });
      break;
    case 'http-version':
      if (m.versions.length === 0) out.push({ path: `${path}.versions`, message: 'Select at least one version' });
      break;
    case 'query':
      checkKvs(m.params, `${path}.params`, out);
      break;
    case 'header':
      checkKvs(m.headers, `${path}.headers`, out, true);
      break;
    case 'cookie':
      checkKvs(m.cookies, `${path}.cookies`, out);
      break;
    case 'form':
      checkKvs(m.fields, `${path}.fields`, out);
      break;
    case 'json-body':
      break;
    case 'not':
      validateMatcher(m.matcher, `${path}.matcher`, out);
      break;
    case 'any':
    case 'all':
      if (m.matchers.length === 0) out.push({ path: `${path}.matchers`, message: 'Add at least one nested condition' });
      m.matchers.forEach((inner, i) => validateMatcher(inner, `${path}.matchers.${i}`, out));
      break;
  }
  return out;
}

function checkHeaders(headers: readonly [string, string][] | undefined, path: string, out: ValidationIssue[]): void {
  headers?.forEach(([k, v], i) => {
    const e = headerNameError(k) ?? headerValueError(v);
    if (e) out.push({ path: `${path}.${i}`, message: e });
  });
}

function checkBodySource(b: BodySource, path: string, out: ValidationIssue[]): void {
  if (b.type === 'base64') {
    const e = base64Error(b.data);
    if (e) out.push({ path: `${path}.data`, message: e });
  } else if (b.type === 'file' && !b.path.trim()) out.push({ path: `${path}.path`, message: 'Enter a file path' });
}

function checkReplacements(list: readonly MatchReplace[], path: string, out: ValidationIssue[]): void {
  list.forEach((mr, i) => {
    if (mr.find === '') out.push({ path: `${path}.${i}.find`, message: 'Enter text to find' });
    else if (mr.regex) {
      const e = regexError(mr.find, !mr.caseSensitive);
      if (e) out.push({ path: `${path}.${i}.find`, message: e });
    }
  });
}

function checkBodyTransform(b: BodyTransform, path: string, out: ValidationIssue[]): void {
  switch (b.type) {
    case 'replace':
      checkBodySource(b.body, `${path}.body`, out);
      break;
    case 'match-replace':
      if (b.replacements.length === 0) out.push({ path: `${path}.replacements`, message: 'Add at least one replacement' });
      checkReplacements(b.replacements, `${path}.replacements`, out);
      break;
    case 'json-patch': {
      const e = jsonPatchError(b.operations);
      if (e) out.push({ path: `${path}.operations`, message: e });
      break;
    }
    case 'json-merge':
      break;
  }
}

function checkRequestTransform(t: RequestTransform | undefined, path: string, out: ValidationIssue[]): void {
  if (!t) return;
  if (t.method != null && !isToken(t.method)) out.push({ path: `${path}.method`, message: t.method ? `“${t.method}” isn't a valid method` : 'Enter a method' });
  if (t.urlReplace) checkReplacements(t.urlReplace, `${path}.urlReplace`, out);
  t.setQuery?.forEach(([k], i) => {
    if (!k.trim()) out.push({ path: `${path}.setQuery.${i}`, message: 'Enter a parameter name' });
  });
  checkHeaders(t.setHeaders, `${path}.setHeaders`, out);
  checkHeaders(t.addHeaders, `${path}.addHeaders`, out);
  t.removeHeaders?.forEach((h, i) => {
    const e = headerNameError(h);
    if (e) out.push({ path: `${path}.removeHeaders.${i}`, message: e });
  });
  if (t.body) checkBodyTransform(t.body, `${path}.body`, out);
}

function checkResponseTransform(t: ResponseTransform | undefined, path: string, out: ValidationIssue[]): void {
  if (!t) return;
  if (t.status != null) {
    const e = statusError(t.status);
    if (e) out.push({ path: `${path}.status`, message: e });
  }
  if (t.statusMessage != null && /[\r\n\0]/.test(t.statusMessage)) out.push({ path: `${path}.statusMessage`, message: "The reason phrase can't contain line breaks" });
  checkHeaders(t.setHeaders, `${path}.setHeaders`, out);
  checkHeaders(t.addHeaders, `${path}.addHeaders`, out);
  t.removeHeaders?.forEach((h, i) => {
    const e = headerNameError(h);
    if (e) out.push({ path: `${path}.removeHeaders.${i}`, message: e });
  });
  if (t.body) checkBodyTransform(t.body, `${path}.body`, out);
}

export function validateAction(a: Action, out: ValidationIssue[] = []): ValidationIssue[] {
  const p = 'action';
  switch (a.type) {
    case 'passthrough':
    case 'transform':
      checkRequestTransform(a.request, `${p}.request`, out);
      checkResponseTransform(a.response, `${p}.response`, out);
      break;
    case 'forward': {
      const e = forwardTargetError(a.target);
      if (e) out.push({ path: `${p}.target`, message: e });
      if (a.stripPrefix != null && a.stripPrefix !== '' && !a.stripPrefix.startsWith('/')) out.push({ path: `${p}.stripPrefix`, message: 'The prefix should start with /' });
      checkRequestTransform(a.request, `${p}.request`, out);
      checkResponseTransform(a.response, `${p}.response`, out);
      break;
    }
    case 'mock': {
      const e = statusError(a.status);
      if (e) out.push({ path: `${p}.status`, message: e });
      if (a.statusMessage != null && /[\r\n\0]/.test(a.statusMessage)) out.push({ path: `${p}.statusMessage`, message: "The reason phrase can't contain line breaks" });
      checkHeaders(a.headers, `${p}.headers`, out);
      checkBodySource(a.body, `${p}.body`, out);
      break;
    }
    case 'map-local':
      if (!a.path.trim()) out.push({ path: `${p}.path`, message: 'Enter a file or folder path' });
      checkHeaders(a.headers, `${p}.headers`, out);
      break;
    case 'ws-reject': {
      const e = statusError(a.status);
      if (e) out.push({ path: `${p}.status`, message: e });
      checkBodySource(a.body, `${p}.body`, out);
      break;
    }
    case 'highlight':
      if (!a.color.trim()) out.push({ path: `${p}.color`, message: 'Pick a colour' });
      break;
    case 'script': {
      const e = scriptError(a.source);
      if (e) out.push({ path: `${p}.source`, message: e });
      break;
    }
    case 'delay':
      if (!Number.isInteger(a.ms) || a.ms < 0) out.push({ path: `${p}.ms`, message: 'Enter a delay of 0 ms or more' });
      if (!Number.isInteger(a.jitterMs) || a.jitterMs < 0) out.push({ path: `${p}.jitterMs`, message: 'Jitter must be 0 or more' });
      break;
    case 'throttle':
      for (const k of ['downloadBps', 'uploadBps', 'latencyMs'] as const) {
        if (!Number.isFinite(a[k]) || a[k] < 0) out.push({ path: `${p}.${k}`, message: 'Must be 0 or more' });
      }
      break;
    case 'breakpoint':
      if (!a.request && !a.response) out.push({ path: `${p}.request`, message: 'Pause the request, the response, or both' });
      break;
    case 'ws-breakpoint':
      if (a.pattern) checkPattern(a.pattern, `${p}.pattern`, out);
      break;
    case 'tls-profile':
      if (!isTlsProfile(a.profile)) out.push({ path: `${p}.profile`, message: 'Pick a TLS fingerprint profile' });
      break;
    case 'close':
    case 'reset':
    case 'timeout':
    case 'ws-echo':
      break;
  }
  return out;
}

/** Every problem with a rule (empty = valid, safe to apply). */
export function validateRule(rule: Rule): ValidationIssue[] {
  const out: ValidationIssue[] = [];
  rule.matchers.forEach((m, i) => validateMatcher(m, `matchers.${i}`, out));
  validateAction(rule.action, out);
  if (rule.maxMatches != null && (!Number.isInteger(rule.maxMatches) || rule.maxMatches < 1 || rule.maxMatches > 4_294_967_295)) {
    out.push({ path: 'maxMatches', message: 'Enter a positive whole number' });
  }
  return out;
}

/** Issues at `path` exactly or below it. */
export function issuesAt(issues: readonly ValidationIssue[], path: string): ValidationIssue[] {
  return issues.filter((i) => i.path === path || i.path.startsWith(`${path}.`));
}

/** First message at exactly `path` (or below it, with `deep`). */
export function issueAt(issues: readonly ValidationIssue[], path: string, deep = false): string | undefined {
  return (deep ? issuesAt(issues, path) : issues.filter((i) => i.path === path))[0]?.message;
}
