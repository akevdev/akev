import { describe, expect, test } from 'bun:test';
import { decodeQueryComponent, parseQueryString, rawQueryOf, splitUrl } from './query';

describe('splitUrl', () => {
  test('separates base, query and fragment', () => {
    expect(splitUrl('https://a.com/p?x=1&y=2#frag')).toEqual({ base: 'https://a.com/p', query: 'x=1&y=2', hash: '#frag' });
    expect(splitUrl('https://a.com/p#frag?notquery')).toEqual({ base: 'https://a.com/p', query: null, hash: '#frag?notquery' });
    expect(splitUrl('https://a.com/p?')).toEqual({ base: 'https://a.com/p', query: '', hash: '' });
    expect(splitUrl('{{base}}/users?q={{q}}')).toEqual({ base: '{{base}}/users', query: 'q={{q}}', hash: '' });
  });

  test('rawQueryOf ignores fragments', () => {
    expect(rawQueryOf('/a?b=1&c=2#frag')).toBe('b=1&c=2');
    expect(rawQueryOf('https://x.test/a')).toBe('');
    expect(rawQueryOf('/a#x?y')).toBe('');
  });
});

describe('parseQueryString', () => {
  test('decodes names and values while keeping the raw form', () => {
    const params = parseQueryString('?q=hello+world&tag=%F0%9F%9A%80&flag&empty=&bad=%E0%A4%A');
    expect(params.map((p) => [p.name, p.value])).toEqual([
      ['q', 'hello world'],
      ['tag', '🚀'],
      ['flag', ''],
      ['empty', ''],
      ['bad', '%E0%A4%A'],
    ]);
    expect(params[2]?.rawValue).toBeNull();
    expect(params[3]?.rawValue).toBe('');
    expect(params[1]?.rawValue).toBe('%F0%9F%9A%80');
  });

  test('keeps duplicates and order, skips empty segments', () => {
    expect(parseQueryString('a=1&&a=2&b=3').map((p) => `${p.name}=${p.value}`)).toEqual(['a=1', 'a=2', 'b=3']);
    expect(parseQueryString('')).toEqual([]);
    expect(parseQueryString(null)).toEqual([]);
  });
});

describe('decodeQueryComponent', () => {
  test('malformed escapes are kept verbatim, valid ones decoded', () => {
    expect(decodeQueryComponent('100%')).toBe('100%');
    expect(decodeQueryComponent('a%20b%zz')).toBe('a b%zz');
    expect(decodeQueryComponent('%41%zz%42')).toBe('A%zzB');
    expect(decodeQueryComponent('a+b')).toBe('a b');
  });
});
