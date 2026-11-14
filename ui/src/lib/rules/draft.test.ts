import { describe, expect, test } from 'bun:test';
import type { Rule } from '../../api/types';
import { mergeExternalChange, normalizeRule, rebaseDraft } from './draft';

const base: Rule = { id: 'r1', enabled: true, matchers: [{ type: 'host', pattern: { op: 'equals', value: 'a.com', caseSensitive: false } }], action: { type: 'mock', status: 200, headers: [], body: { type: 'empty' } } };
const edited: Rule = { ...base, action: { type: 'mock', status: 42, headers: [], body: { type: 'empty' } } };

describe('rebaseDraft', () => {
  test('no stash → the rule', () => {
    expect(rebaseDraft(undefined, base)).toBe(base);
  });

  test('stash still applies → the stashed draft', () => {
    expect(rebaseDraft({ draft: edited, base }, { ...base })).toBe(edited);
  });

  test('only enabled changed elsewhere → draft with the new enabled flag', () => {
    const out = rebaseDraft({ draft: edited, base }, { ...base, enabled: false });
    expect(out.enabled).toBe(false);
    expect(out.action).toEqual(edited.action);
  });

  test('rule changed otherwise → the stash is dropped', () => {
    const changed: Rule = { ...base, title: 'Renamed elsewhere' };
    expect(rebaseDraft({ draft: edited, base }, changed)).toBe(changed);
  });
});

describe('mergeExternalChange', () => {
  test('clean draft follows the rule', () => {
    const next: Rule = { ...base, note: 'x' };
    expect(mergeExternalChange(base, base, next)).toBe(next);
  });

  test('enable toggled in the list keeps in-progress edits', () => {
    const out = mergeExternalChange(edited, base, { ...base, enabled: false });
    expect(out).toEqual({ ...edited, enabled: false });
  });

  test('same enabled flag returns the draft itself (no re-render churn)', () => {
    const d = { ...edited };
    expect(mergeExternalChange(d, base, { ...base })).toBe(d);
  });

  test('undo/redo (any other change) wins over local edits', () => {
    const undone: Rule = { ...base, matchers: [] };
    expect(mergeExternalChange(edited, base, undone)).toBe(undone);
  });
});

describe('normalizeRule', () => {
  test('drops empty optional fields', () => {
    const r = normalizeRule({ ...base, maxMatches: undefined, note: '', title: '  ' });
    expect('maxMatches' in r).toBe(false);
    expect('note' in r).toBe(false);
    expect('title' in r).toBe(false);
  });

  test('keeps meaningful values', () => {
    const r = normalizeRule({ ...base, maxMatches: 3, note: 'why', title: 'T' });
    expect(r).toMatchObject({ maxMatches: 3, note: 'why', title: 'T' });
  });
});
