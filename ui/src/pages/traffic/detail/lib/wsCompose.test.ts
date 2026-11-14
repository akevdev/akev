import { describe, expect, test } from 'bun:test';
import type { PausedMessage } from '../../../../api/types';
import { textToBase64 } from '../../../../lib/encoding';
import { composedPreview, editablePaused, encodeComposed, formatHex, forwardResume, jsonError, looksLikeJson, parseBase64, parseHex } from './wsCompose';

const paused = (kind: 'text' | 'binary', payload: string): Pick<PausedMessage, 'kind' | 'payload'> => ({ kind, payload });

describe('payload parsing', () => {
  test('hex accepts common separators and prefixes', () => {
    expect(parseHex('de ad be ef')).toEqual(new Uint8Array([0xde, 0xad, 0xbe, 0xef]));
    expect(parseHex('0xDE,0xAD\n00:ff-01')).toEqual(new Uint8Array([0xde, 0xad, 0x00, 0xff, 0x01]));
    expect(parseHex('')).toEqual(new Uint8Array());
    expect(parseHex('abc')).toMatch(/even number/);
    expect(parseHex('zz')).toMatch(/0-9 and a-f/);
  });
  test('hex formatting: 16 bytes per line', () => {
    const bytes = new Uint8Array(20).map((_, i) => i);
    expect(formatHex(bytes)).toBe('00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f\n10 11 12 13');
    expect(parseHex(formatHex(bytes))).toEqual(bytes);
  });
  test('base64 (whitespace ignored)', () => {
    expect(parseBase64('3q2+\n7w==')).toEqual(new Uint8Array([0xde, 0xad, 0xbe, 0xef]));
    expect(parseBase64('not base64!')).toBe('Not valid base64');
    expect(parseBase64('A')).toBe('Not valid base64');
  });
  test('JSON', () => {
    expect(jsonError('{"a":1}')).toBeNull();
    expect(jsonError('{"a":')).toMatch(/^Invalid JSON/);
    expect(jsonError('  ')).toBe('Enter a JSON value');
    expect(looksLikeJson(' [1, 2] ')).toBe(true);
    expect(looksLikeJson('42')).toBe(false);
    expect(looksLikeJson('{oops')).toBe(false);
  });
});

describe('encodeComposed', () => {
  test('text & JSON go as text, exactly as typed', () => {
    expect(encodeComposed('text', 'héllo')).toEqual({ ok: true, body: { kind: 'text', text: 'héllo' }, size: 6 });
    expect(encodeComposed('json', '{ "a" : 1 }')).toEqual({ ok: true, body: { kind: 'text', text: '{ "a" : 1 }' }, size: 11 });
    expect(encodeComposed('json', '{')).toMatchObject({ ok: false });
  });
  test('hex & base64 go as binary', () => {
    expect(encodeComposed('hex', 'ff 00')).toEqual({ ok: true, body: { kind: 'binary', base64: '/wA=' }, size: 2 });
    expect(encodeComposed('base64', '/w A=')).toEqual({ ok: true, body: { kind: 'binary', base64: '/wA=' }, size: 2 });
    expect(encodeComposed('hex', 'f')).toMatchObject({ ok: false });
  });
});

describe('paused message editing', () => {
  test('text (JSON highlighted, never reformatted) and binary as hex', () => {
    expect(editablePaused(paused('text', textToBase64('{"type":"auth"}')))).toEqual({ mode: 'json', input: '{"type":"auth"}' });
    expect(editablePaused(paused('text', textToBase64('hello')))).toEqual({ mode: 'text', input: 'hello' });
    expect(editablePaused(paused('binary', '3q2+7w=='))).toEqual({ mode: 'hex', input: 'de ad be ef' });
  });
  test('untouched content forwards the original; edits carry the new payload', () => {
    const text = paused('text', textToBase64('{"a":1}'));
    expect(forwardResume(text, 'json', '{"a":1}')).toEqual({ ok: true, resume: { action: 'forward' }, changed: false });
    expect(forwardResume(text, 'json', '{"a":2}')).toEqual({ ok: true, resume: { action: 'forward', kind: 'text', text: '{"a":2}' }, changed: true });
    // Broken JSON may be sent on purpose.
    expect(forwardResume(text, 'json', '{"a":')).toMatchObject({ ok: true, changed: true });
    const bin = paused('binary', '3q2+7w==');
    expect(forwardResume(bin, 'hex', 'DE AD BE EF')).toEqual({ ok: true, resume: { action: 'forward' }, changed: false });
    expect(forwardResume(bin, 'hex', 'de ad')).toEqual({ ok: true, resume: { action: 'forward', kind: 'binary', base64: '3q0=' }, changed: true });
    expect(forwardResume(bin, 'hex', 'xyz')).toMatchObject({ ok: false });
    // Same bytes as another kind is a change.
    expect(forwardResume(paused('binary', textToBase64('hi')), 'text', 'hi')).toMatchObject({ changed: true, resume: { kind: 'text' } });
  });
  test('previews', () => {
    expect(composedPreview('text', '  a\n  b ')).toBe('a b');
    expect(composedPreview('hex', '')).toBe('(no bytes)');
    expect(composedPreview('text', 'x'.repeat(200), 10)).toBe(`${'x'.repeat(10)}…`);
  });
});
