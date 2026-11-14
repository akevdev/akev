import { describe, expect, test } from 'bun:test';
import { parseBodyViewJson, parseSendResponseJson } from '../api/client';
import type { BodyView } from '../api/types';
import { childrenOf, copyValueText, primitiveText } from '../components/ui/JsonTree';
import { bodyText } from '../components/exchange/model/compare';
import { formatJson, formatJsonLines, parseJsonLines } from '../components/exchange/model/body';
import { formatJsonText } from '../pages/traffic/detail/lib/breakpoint';
import { formatPayload } from '../pages/traffic/detail/lib/messages';
import { formatJson as composeFormatJson } from '../pages/compose/request';
import { formatJsonLossless, isLosslessNumber, jsonEntries, jsonNumber, LosslessNumber, MAX_JSON_DEPTH, mayHaveInexactNumbers, parseJsonLossless, stringifyJson } from './json';

/** Small deterministic PRNG (mulberry32). */
function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function randomValue(r: () => number, depth = 0): unknown {
  const k = Math.floor(r() * (depth > 3 ? 5 : 7));
  switch (k) {
    case 0:
      return null;
    case 1:
      return r() < 0.5;
    case 2:
      return Math.floor((r() - 0.5) * 1e9);
    case 3:
      return (r() - 0.5) * 10 ** Math.floor(r() * 40 - 20);
    case 4: {
      const chars = ['a', 'é', '"', '\\', '\n', '\u0001', '😀', ' ', ' ', '/', '\ud800'];
      let s = '';
      const n = Math.floor(r() * 8);
      for (let i = 0; i < n; i++) s += chars[Math.floor(r() * chars.length)];
      return s;
    }
    case 5:
      return Array.from({ length: Math.floor(r() * 5) }, () => randomValue(r, depth + 1));
    default: {
      const o: Record<string, unknown> = {};
      const n = Math.floor(r() * 5);
      for (let i = 0; i < n; i++) o[['id', 'a b', '0', '10', 'x', '__proto__', 'é'][Math.floor(r() * 7)] as string] = randomValue(r, depth + 1);
      return o;
    }
  }
}

describe('parseJsonLossless', () => {
  test('agrees with JSON.parse on ordinary documents (differential)', () => {
    const r = rng(42);
    for (let i = 0; i < 2000; i++) {
      const value = randomValue(r);
      for (const text of [JSON.stringify(value), JSON.stringify(value, null, 2)]) {
        expect(parseJsonLossless(text)).toEqual(JSON.parse(text));
        expect(stringifyJson(parseJsonLossless(text), 2)).toBe(JSON.stringify(JSON.parse(text), null, 2));
        expect(stringifyJson(parseJsonLossless(text), 0)).toBe(JSON.stringify(JSON.parse(text)));
      }
    }
  });

  test('rejects exactly what JSON.parse rejects (mutation corpus)', () => {
    const r = rng(7);
    const bits = ['{', '}', '[', ']', ',', ':', '"', '\\', '-', '0', '1', '.', 'e', 'E', '+', 't', 'n', 'u', ' ', '\n', '\u0000', 'x', '01', '1.', '.5', 'tru', 'nul'];
    let agreed = 0;
    for (let i = 0; i < 4000; i++) {
      const base = JSON.stringify(randomValue(r));
      const at = Math.floor(r() * (base.length + 1));
      const cut = r() < 0.5 ? 1 : 0;
      const text = base.slice(0, at) + (bits[Math.floor(r() * bits.length)] as string) + base.slice(at + cut);
      let native: unknown;
      let nativeOk = true;
      try {
        native = JSON.parse(text);
      } catch {
        nativeOk = false;
      }
      let ours: unknown;
      let oursOk = true;
      try {
        ours = parseJsonLossless(text);
      } catch (e) {
        oursOk = false;
        expect(e).toBeInstanceOf(SyntaxError);
      }
      expect({ text, ok: oursOk }).toEqual({ text, ok: nativeOk });
      // Numbers may legitimately differ (lossless); compare via the exact serialisation.
      if (nativeOk) expect(JSON.parse(stringifyJson(ours, 0) ?? 'null')).toEqual(native);
      agreed++;
    }
    expect(agreed).toBe(4000);
  });

  test('keeps 64-bit integers and other inexact numbers exactly', () => {
    const v = parseJsonLossless('{"id":18446744073709551615,"ts":1700000000123456789,"neg":-9223372036854775808,"safe":9007199254740991,"pi":3.14159265358979323846,"big":1e400,"tiny":1e-400,"f":0.1,"one":1.0,"e":1e21}') as Record<string, unknown>;
    expect(v.id).toBeInstanceOf(LosslessNumber);
    expect((v.id as LosslessNumber).source).toBe('18446744073709551615');
    expect((v.ts as LosslessNumber).source).toBe('1700000000123456789');
    expect((v.neg as LosslessNumber).source).toBe('-9223372036854775808');
    expect(v.safe).toBe(9007199254740991);
    expect((v.pi as LosslessNumber).source).toBe('3.14159265358979323846');
    expect((v.big as LosslessNumber).source).toBe('1e400');
    expect((v.tiny as LosslessNumber).source).toBe('1e-400');
    // Exact values stay plain numbers, even when written differently.
    expect(v.f).toBe(0.1);
    expect(v.one).toBe(1);
    expect(v.e).toBe(1e21);
    expect(stringifyJson(v, 0)).toBe('{"id":18446744073709551615,"ts":1700000000123456789,"neg":-9223372036854775808,"safe":9007199254740991,"pi":3.14159265358979323846,"big":1e400,"tiny":1e-400,"f":0.1,"one":1,"e":1e+21}');
  });

  test('jsonNumber boundary cases', () => {
    expect(jsonNumber('9007199254740992')).toBe(9007199254740992); // 2^53 is exact
    expect(isLosslessNumber(jsonNumber('9007199254740993'))).toBe(true);
    // 2^64 is an exact double, but it prints as 18446744073709552000: keep the literal.
    expect((jsonNumber('18446744073709551616') as LosslessNumber).source).toBe('18446744073709551616');
    expect(jsonNumber('100000000000000000000')).toBe(1e20);
    expect(jsonNumber('-0')).toBe(-0);
    expect(jsonNumber('0.30000000000000004')).toBe(0.30000000000000004);
    expect(isLosslessNumber(jsonNumber('0.30000000000000000001'))).toBe(true);
    expect(new LosslessNumber('12345678901234567890').valueOf()).toBe(12345678901234567890);
  });

  test('duplicate keys are all kept (last value wins for property access)', () => {
    const v = parseJsonLossless('{"role":"user","name":"x","role":"admin"}') as Record<string, unknown>;
    expect(v.role).toBe('admin');
    expect(Object.keys(v)).toEqual(['role', 'name']);
    expect(jsonEntries(v)).toEqual([
      ['role', 'user'],
      ['name', 'x'],
      ['role', 'admin'],
    ]);
    expect(stringifyJson(v, 0)).toBe('{"role":"user","name":"x","role":"admin"}');
    // Plain JSON.stringify keeps working (and sees what JSON.parse would).
    expect(JSON.stringify(v)).toBe('{"role":"admin","name":"x"}');
  });

  test('__proto__ is an own property, never the prototype', () => {
    const v = parseJsonLossless('{"__proto__":{"polluted":true},"a":1}') as Record<string, unknown>;
    expect(Object.getPrototypeOf(v)).toBe(Object.prototype);
    expect(Object.keys(v)).toEqual(['__proto__', 'a']);
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
    expect(v).toEqual(JSON.parse('{"__proto__":{"polluted":true},"a":1}'));
  });

  test('strings: escapes decoded, raw control characters rejected', () => {
    expect(parseJsonLossless('"a\\"b\\\\c\\n\\u00e9\\ud83d\\ude00"')).toBe('a"b\\c\né😀');
    expect(parseJsonLossless('"\\\\"')).toBe('\\');
    expect(parseJsonLossless('["\\\\", "x"]')).toEqual(['\\', 'x']);
    expect(() => parseJsonLossless('"a\tb"')).toThrow(SyntaxError);
    expect(() => parseJsonLossless('"\\x41"')).toThrow(SyntaxError);
    expect(() => parseJsonLossless('"abc')).toThrow(SyntaxError);
  });

  test('deep nesting fails with a clear error instead of overflowing the stack', () => {
    const deep = '['.repeat(MAX_JSON_DEPTH + 5) + ']'.repeat(MAX_JSON_DEPTH + 5);
    expect(() => parseJsonLossless(deep)).toThrow(/nested deeper/);
    const ok = '['.repeat(200) + ']'.repeat(200);
    expect(parseJsonLossless(ok)).toEqual(JSON.parse(ok));
  });

  test('stringifyJson mirrors JSON.stringify edge cases', () => {
    const d = new Date(0);
    const value = { a: undefined, b: () => 1, c: [undefined, NaN, Infinity, d], n: 10n, s: new String('x') };
    expect(stringifyJson(value, 2)).toBe(JSON.stringify({ ...value, n: 10 }, null, 2));
    expect(stringifyJson(undefined)).toBeUndefined();
    expect(stringifyJson([], 2)).toBe('[]');
    expect(stringifyJson({}, 2)).toBe('{}');
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(() => stringifyJson(cyclic)).toThrow(TypeError);
  });

  test('JSON.stringify of a LosslessNumber degrades gracefully', () => {
    const text = JSON.stringify({ id: new LosslessNumber('18446744073709551615') });
    // Exact where JSON.rawJSON exists, else the nearest number — never a crash.
    expect(text === '{"id":18446744073709551615}' || text === `{"id":${Number('18446744073709551615')}}`).toBe(true);
  });

  test('mayHaveInexactNumbers is a conservative pre-check', () => {
    expect(mayHaveInexactNumbers('{"a":123456789012345}')).toBe(false);
    expect(mayHaveInexactNumbers('{"a":1234567890123456}')).toBe(true);
    expect(mayHaveInexactNumbers('{"a":1e400}')).toBe(true);
  });
});

describe('lossless JSON in the body viewers', () => {
  const MSGPACK_VIEW = '{"kind":"msg-pack","size":20,"wireSize":20,"encodings":[],"text":null,"formatted":null,"structured":{"id":18446744073709551615,"ts":1700000000123456789,"n":[1,2.5]},"decodeError":null,"truncated":false,"captureTruncated":false}';

  test('API body views keep 64-bit integers in `structured` (Decoded view)', () => {
    const view = parseBodyViewJson(MSGPACK_VIEW) as BodyView;
    const s = view.structured as Record<string, unknown>;
    expect((s.id as LosslessNumber).source).toBe('18446744073709551615');
    expect((s.ts as LosslessNumber).source).toBe('1700000000123456789');
    expect(s.n).toEqual([1, 2.5]);
    // Other fields stay plain JSON.parse values.
    expect(view.size).toBe(20);
    // Before the fix the Decoded view/copy showed rounded values.
    expect(JSON.stringify((JSON.parse(MSGPACK_VIEW) as BodyView).structured)).toContain('18446744073709552000');
    expect(stringifyJson(view.structured, 0)).toBe('{"id":18446744073709551615,"ts":1700000000123456789,"n":[1,2.5]}');
    // Compare view and "Copy" use the exact serialisation.
    expect(bodyText(view).text).toContain('"id": 18446744073709551615');
    const send = parseSendResponseJson(`{"id":3,"response":null,"error":null,"timing":{"start":1},"body":${MSGPACK_VIEW}}`) as { body: BodyView };
    expect(((send.body.structured as Record<string, unknown>).id as LosslessNumber).source).toBe('18446744073709551615');
    // Views without big numbers are parsed natively and unchanged.
    const plain = '{"kind":"json","size":1,"structured":{"a":1}}';
    expect(parseBodyViewJson(plain)).toEqual(JSON.parse(plain));
  });

  test('JSON Tree / Pretty fallbacks / breakpoint & compose formatters keep snowflake ids', () => {
    const text = '{"id":12345678901234567890,"items":[{"v":9007199254740993}]}';
    const tree = parseJsonLossless(text) as { id: unknown; items: { v: unknown }[] };
    expect(primitiveText(tree.id, 'number')).toBe('12345678901234567890');
    expect(copyValueText(tree)).toBe('{\n  "id": 12345678901234567890,\n  "items": [\n    {\n      "v": 9007199254740993\n    }\n  ]\n}');
    const pretty = '{\n  "id": 12345678901234567890,\n  "items": [\n    {\n      "v": 9007199254740993\n    }\n  ]\n}';
    expect(formatJson(text)).toMatchObject({ ok: true, text: pretty });
    expect(formatJsonText(text)).toBe(pretty);
    expect(formatJsonLossless(text)).toBe(pretty);
    expect(composeFormatJson('{"id":{{id}},"big":12345678901234567890}')).toBe('{\n  "id": {{id}},\n  "big": 12345678901234567890\n}');
    expect(formatPayload({ seq: 0, direction: 'received', kind: 'text', at: 0, size: 1, text: '{"id":12345678901234567890}', base64: null }).text).toBe('{\n  "id": 12345678901234567890\n}');
    expect(formatJsonLines('{"id":12345678901234567890}\n{"id":1}')).toBe('{\n  "id": 12345678901234567890\n}\n\n{\n  "id": 1\n}');
    expect((parseJsonLines('{"id":12345678901234567890}')[0] as { id: LosslessNumber }).id.source).toBe('12345678901234567890');
    expect(bodyText({ kind: 'json', size: 1, wireSize: 1, encodings: [], text, formatted: null, structured: null, decodeError: null, truncated: false, captureTruncated: false }).text).toBe(pretty);
  });

  test('JSON tree lists every occurrence of a duplicate key', () => {
    const v = parseJsonLossless('{"role":"user","role":"admin"}');
    const kids = childrenOf(v, '$', '$');
    expect(kids.map((k) => [k.key, k.value, k.shadowed, k.path])).toEqual([
      ['role', 'user', true, '$.role'],
      ['role', 'admin', false, '$.role'],
    ]);
    expect(new Set(kids.map((k) => k.id)).size).toBe(2);
    expect(copyValueText(v)).toBe('{\n  "role": "user",\n  "role": "admin"\n}');
    // Lossless numbers are leaves, not objects.
    expect(childrenOf(new LosslessNumber('1e400'), '$', '$')).toEqual([]);
  });
});
