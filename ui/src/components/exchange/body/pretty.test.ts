import { describe, expect, test } from 'bun:test';
import { prettyAsync } from './pretty';

describe('prettyAsync (worker)', () => {
  test('formats JSON off the main thread, keeping big numbers exact', async () => {
    const r = await prettyAsync('{"id":12345678901234567890,"a":[1,true]}', false);
    expect(r).toEqual({ ok: true, text: '{\n  "id": 12345678901234567890,\n  "a": [\n    1,\n    true\n  ]\n}' });
  });

  test('reports invalid JSON', async () => {
    const r = await prettyAsync('{oops', false);
    expect(r.ok).toBe(false);
  });

  test('formats JSON Lines', async () => {
    const r = await prettyAsync('{"a":1}\n{"b":2}', true);
    expect(r).toEqual({ ok: true, text: '{\n  "a": 1\n}\n\n{\n  "b": 2\n}' });
  });
});
