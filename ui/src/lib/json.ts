/**
 * Lossless JSON for *displaying* captured data.
 *
 * `JSON.parse` silently rounds integers beyond ±2^53 (`18446744073709551615` becomes
 * `18446744073709552000`), turns `1e400` into `Infinity` and keeps only the last of duplicate
 * keys. That is fine for our own API objects but wrong for a debugging tool showing what went
 * over the wire. {@link parseJsonLossless} keeps every number whose value a JS `number` can't
 * represent exactly as a {@link LosslessNumber} (its source text), and remembers every
 * occurrence of duplicate keys; {@link stringifyJson} writes such values back out exactly.
 *
 * Numbers that are exact as `number`s stay plain numbers, so for ordinary documents the parsed
 * value is identical to `JSON.parse`'s.
 */

/** A JSON number literal kept as text because a JS `number` can't hold its value exactly. */
export class LosslessNumber {
  constructor(
    /** The literal exactly as it appeared in the JSON text (`18446744073709551615`). */
    readonly source: string,
  ) {}

  /** The nearest JS number (lossy — for sorting/arithmetic only). */
  valueOf(): number {
    return Number(this.source);
  }

  toString(): string {
    return this.source;
  }

  /**
   * Keeps `JSON.stringify` working on values that contain lossless numbers: the exact literal
   * where `JSON.rawJSON` exists, else the nearest number. Prefer {@link stringifyJson}.
   */
  toJSON(): unknown {
    const raw = (JSON as unknown as { rawJSON?: (text: string) => unknown }).rawJSON;
    if (typeof raw === 'function') {
      try {
        return raw(this.source);
      } catch {
        // fall through
      }
    }
    const n = Number(this.source);
    return Number.isFinite(n) ? n : this.source;
  }
}

export function isLosslessNumber(v: unknown): v is LosslessNumber {
  return v instanceof LosslessNumber;
}

/**
 * Non-enumerable property on parsed objects that had duplicate keys: every `[key, value]`
 * pair in document order (the object's own properties hold the last value of each key, as
 * with `JSON.parse`).
 */
export const JSON_ENTRIES: unique symbol = Symbol('akev.json.entries');

type WithEntries = { [JSON_ENTRIES]?: [string, unknown][] };

/** All entries of a parsed object in document order, duplicates included. */
export function jsonEntries(obj: object): [string, unknown][] {
  const dupes = (obj as WithEntries)[JSON_ENTRIES];
  return dupes ? dupes : Object.entries(obj);
}

/** Maximum nesting accepted by {@link parseJsonLossless} (keeps recursion safe). */
export const MAX_JSON_DEPTH = 1000;

const NUMBER_RE = /-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/y;
const CONTROL_RE = /[\u0000-\u001f]/;

/** Canonical decimal form (`-123e-2`) used to compare a literal's value with a number's. */
function canonicalDecimal(text: string): string {
  let s = text;
  let neg = false;
  if (s.charCodeAt(0) === 45 /* - */) {
    neg = true;
    s = s.slice(1);
  }
  let exp = 0;
  const e = s.search(/[eE]/);
  if (e >= 0) {
    exp = Number(s.slice(e + 1));
    s = s.slice(0, e);
  }
  const dot = s.indexOf('.');
  if (dot >= 0) {
    exp -= s.length - dot - 1;
    s = s.slice(0, dot) + s.slice(dot + 1);
  }
  let a = 0;
  while (a < s.length && s.charCodeAt(a) === 48) a++;
  let b = s.length;
  while (b > a && s.charCodeAt(b - 1) === 48) {
    b--;
    exp++;
  }
  const digits = s.slice(a, b);
  if (!digits) return '0';
  return `${neg ? '-' : ''}${digits}e${exp}`;
}

/**
 * The value for a JSON number literal: a plain `number` when that number prints back as the
 * same decimal value (`0.1`, `1.0`, `1e21`), else a {@link LosslessNumber} — so anything shown
 * from the parsed value equals what was received.
 */
export function jsonNumber(literal: string): number | LosslessNumber {
  const n = Number(literal);
  // Up to 15 significant digits without an exponent always round-trip through a double.
  let digits = literal.length;
  if (literal.charCodeAt(0) === 45) digits--;
  const hasDot = literal.indexOf('.') >= 0;
  if (hasDot) digits--;
  if (digits <= 15 && !/[eE]/.test(literal)) return n;
  if (!Number.isFinite(n)) return new LosslessNumber(literal);
  return canonicalDecimal(literal) === canonicalDecimal(String(n)) ? n : new LosslessNumber(literal);
}

class Parser {
  pos = 0;
  constructor(private readonly text: string) {}

  fail(message: string): never {
    throw new SyntaxError(`${message} at position ${this.pos}`);
  }

  ws(): void {
    const t = this.text;
    let p = this.pos;
    for (;;) {
      const c = t.charCodeAt(p);
      if (c === 32 || c === 10 || c === 13 || c === 9) p++;
      else break;
    }
    this.pos = p;
  }

  value(depth: number): unknown {
    this.ws();
    const t = this.text;
    const c = t.charCodeAt(this.pos);
    switch (c) {
      case 123 /* { */:
        return this.object(depth + 1);
      case 91 /* [ */:
        return this.array(depth + 1);
      case 34 /* " */:
        return this.string();
      case 116 /* t */:
        return this.literal('true', true);
      case 102 /* f */:
        return this.literal('false', false);
      case 110 /* n */:
        return this.literal('null', null);
      default:
        if (c === 45 || (c >= 48 && c <= 57)) return this.number();
        if (Number.isNaN(c)) this.fail('Unexpected end of JSON input');
        return this.fail(`Unexpected token ${JSON.stringify(t[this.pos])}`);
    }
  }

  literal<T>(word: string, value: T): T {
    if (!this.text.startsWith(word, this.pos)) this.fail(`Unexpected token ${JSON.stringify(this.text[this.pos])}`);
    this.pos += word.length;
    return value;
  }

  number(): number | LosslessNumber {
    NUMBER_RE.lastIndex = this.pos;
    const m = NUMBER_RE.exec(this.text);
    if (!m) return this.fail('Invalid number');
    this.pos += m[0].length;
    return jsonNumber(m[0]);
  }

  string(): string {
    const t = this.text;
    const start = this.pos;
    let from = start + 1;
    let escaped = false;
    let end: number;
    for (;;) {
      const q = t.indexOf('"', from);
      if (q < 0) {
        this.pos = t.length;
        return this.fail('Unterminated string in JSON');
      }
      // An odd run of backslashes before the quote escapes it.
      let k = q - 1;
      while (k > start && t.charCodeAt(k) === 92) k--;
      if ((q - 1 - k) % 2 === 0) {
        end = q;
        break;
      }
      escaped = true;
      from = q + 1;
    }
    const body = t.slice(start + 1, end);
    if (!escaped && body.indexOf('\\') < 0) {
      if (CONTROL_RE.test(body)) this.fail('Bad control character in string literal');
      this.pos = end + 1;
      return body;
    }
    let s: unknown;
    try {
      // Escapes: let the native parser decode (and validate) the literal.
      s = JSON.parse(t.slice(start, end + 1));
    } catch (e) {
      this.fail(e instanceof Error ? e.message : 'Bad string');
    }
    this.pos = end + 1;
    return s as string;
  }

  array(depth: number): unknown[] {
    if (depth > MAX_JSON_DEPTH) this.fail(`JSON is nested deeper than ${MAX_JSON_DEPTH} levels`);
    this.pos++; // [
    const out: unknown[] = [];
    this.ws();
    if (this.text.charCodeAt(this.pos) === 93 /* ] */) {
      this.pos++;
      return out;
    }
    for (;;) {
      out.push(this.value(depth));
      this.ws();
      const c = this.text.charCodeAt(this.pos);
      if (c === 44 /* , */) {
        this.pos++;
        continue;
      }
      if (c === 93 /* ] */) {
        this.pos++;
        return out;
      }
      if (Number.isNaN(c)) this.fail('Unexpected end of JSON input');
      this.fail(`Expected ',' or ']' after array element`);
    }
  }

  object(depth: number): Record<string, unknown> {
    if (depth > MAX_JSON_DEPTH) this.fail(`JSON is nested deeper than ${MAX_JSON_DEPTH} levels`);
    this.pos++; // {
    const out: Record<string, unknown> = {};
    let entries: [string, unknown][] | null = null;
    this.ws();
    if (this.text.charCodeAt(this.pos) === 125 /* } */) {
      this.pos++;
      return out;
    }
    for (;;) {
      this.ws();
      if (this.text.charCodeAt(this.pos) !== 34) {
        if (this.pos >= this.text.length) this.fail('Unexpected end of JSON input');
        this.fail('Expected a double-quoted property name');
      }
      const key = this.string();
      this.ws();
      if (this.text.charCodeAt(this.pos) !== 58 /* : */) this.fail(`Expected ':' after property name`);
      this.pos++;
      const value = this.value(depth);
      if (Object.hasOwn(out, key)) {
        // Duplicate key: remember every occurrence (the property keeps the last value).
        entries ??= Object.entries(out);
        entries.push([key, value]);
      } else if (entries) {
        entries.push([key, value]);
      }
      if (key === '__proto__') Object.defineProperty(out, key, { value, writable: true, enumerable: true, configurable: true });
      else out[key] = value;
      this.ws();
      const c = this.text.charCodeAt(this.pos);
      if (c === 44 /* , */) {
        this.pos++;
        continue;
      }
      if (c === 125 /* } */) {
        this.pos++;
        if (entries) Object.defineProperty(out, JSON_ENTRIES, { value: entries, enumerable: false });
        return out;
      }
      if (Number.isNaN(c)) this.fail('Unexpected end of JSON input');
      this.fail(`Expected ',' or '}' after property value`);
    }
  }
}

/**
 * Parse JSON text like `JSON.parse` (same grammar, throws `SyntaxError`), but without losing
 * information: inexact numbers become {@link LosslessNumber}s and duplicate keys are kept
 * (see {@link jsonEntries}).
 */
export function parseJsonLossless(text: string): unknown {
  const p = new Parser(text);
  const value = p.value(0);
  p.ws();
  if (p.pos < text.length) p.fail(`Unexpected non-whitespace character after JSON`);
  return value;
}

/**
 * Whether `value` holds anything native `JSON.stringify` would write inexactly or reject:
 * lossless numbers, bigints, or objects with duplicate keys (cycles fall through to the exact
 * writer, which reports them like `JSON.stringify`).
 */
function needsExactWriter(value: unknown): boolean {
  const seen = new Set<object>();
  const stack: unknown[] = [value];
  while (stack.length) {
    const v = stack.pop();
    if (typeof v === 'bigint') return true;
    if (v == null || typeof v !== 'object') continue;
    if (v instanceof LosslessNumber) return true;
    if (seen.has(v)) return true;
    seen.add(v);
    if (Array.isArray(v)) {
      for (let i = 0; i < v.length; i++) stack.push(v[i]);
      continue;
    }
    if ((v as WithEntries)[JSON_ENTRIES]) return true;
    for (const k in v) if (Object.hasOwn(v, k)) stack.push((v as Record<string, unknown>)[k]);
  }
  return false;
}

/**
 * `JSON.stringify(value, null, indent)` that also writes {@link LosslessNumber}s and bigints as
 * exact number literals and every occurrence of duplicate keys. Returns `undefined` for the
 * same inputs `JSON.stringify` does (top-level `undefined`, functions, symbols).
 */
export function stringifyJson(value: unknown, indent: number | string = 2): string | undefined {
  const gap = typeof indent === 'number' ? ' '.repeat(Math.max(0, Math.min(10, Math.floor(indent)))) : indent.slice(0, 10);
  // Nothing JSON.stringify would get wrong: use the (much faster) native serialiser.
  if (!needsExactWriter(value)) return JSON.stringify(value, null, gap);
  const stack = new Set<object>();

  const write = (v: unknown, key: string, current: string): string | undefined => {
    if (v instanceof LosslessNumber) return v.source;
    if (v != null && typeof v === 'object' && typeof (v as { toJSON?: unknown }).toJSON === 'function') {
      v = (v as { toJSON: (k: string) => unknown }).toJSON(key);
      if (v instanceof LosslessNumber) return v.source;
    }
    switch (typeof v) {
      case 'string':
        return JSON.stringify(v);
      case 'number':
        return Number.isFinite(v) ? String(v) : 'null';
      case 'bigint':
        return v.toString();
      case 'boolean':
        return v ? 'true' : 'false';
      case 'undefined':
      case 'function':
      case 'symbol':
        return undefined;
    }
    if (v === null) return 'null';
    const obj = v as object;
    // Boxed primitives (new Number(1), …) serialise as their primitive.
    if (obj instanceof Number || obj instanceof String || obj instanceof Boolean) return write(obj.valueOf(), key, current);
    // JSON.rawJSON values (where supported) carry their text.
    if (typeof (JSON as unknown as { isRawJSON?: (o: unknown) => boolean }).isRawJSON === 'function' && (JSON as unknown as { isRawJSON: (o: unknown) => boolean }).isRawJSON(obj)) {
      return (obj as { rawJSON: string }).rawJSON;
    }
    if (stack.has(obj)) throw new TypeError('Converting circular structure to JSON');
    stack.add(obj);
    const inner = current + gap;
    const sep = gap ? `,\n${inner}` : ',';
    let out: string;
    if (Array.isArray(obj)) {
      if (obj.length === 0) out = '[]';
      else {
        const parts: string[] = [];
        for (let i = 0; i < obj.length; i++) parts.push(write(obj[i], String(i), inner) ?? 'null');
        out = gap ? `[\n${inner}${parts.join(sep)}\n${current}]` : `[${parts.join(',')}]`;
      }
    } else {
      const parts: string[] = [];
      for (const [k, child] of jsonEntries(obj)) {
        const s = write(child, k, inner);
        if (s !== undefined) parts.push(`${JSON.stringify(k)}:${gap ? ' ' : ''}${s}`);
      }
      out = parts.length === 0 ? '{}' : gap ? `{\n${inner}${parts.join(sep)}\n${current}}` : `{${parts.join(',')}}`;
    }
    stack.delete(obj);
    return out;
  };

  return write(value, '', '');
}

/** Pretty-print JSON text without losing numbers or duplicate keys (throws on invalid JSON). */
export function formatJsonLossless(text: string, indent: number | string = 2): string {
  return stringifyJson(parseJsonLossless(text), indent) ?? 'null';
}

/**
 * Whether JSON text might contain a number `JSON.parse` would round (16+ digit run, or an
 * exponent). A cheap pre-check before a lossless re-parse of an API response.
 */
export function mayHaveInexactNumbers(text: string): boolean {
  return /\d{16}|\d[eE][+-]?\d/.test(text);
}
