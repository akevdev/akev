import { describe, expect, test } from 'bun:test';
import type { RuleSet } from '../../api/types';
import { History } from './history';
import { formatHits, limitState, ruleMatchesQuery } from './stats';
import { findRule, moveItem, positionOf, removeItem, replaceRule, setItemEnabled } from './tree';

describe('History', () => {
  test('undo / redo walk the stack', () => {
    const h = new History<number>();
    h.push({ label: 'a', before: 0, after: 1 }, 0);
    h.push({ label: 'b', before: 1, after: 2 }, 10_000);
    expect(h.canUndo).toBe(true);
    expect(h.canRedo).toBe(false);
    expect(h.peekUndo()?.label).toBe('b');
    const u = h.undo();
    expect(u?.before).toBe(1);
    expect(h.peekRedo()?.label).toBe('b');
    expect(h.undo()?.before).toBe(0);
    expect(h.undo()).toBeNull();
    expect(h.redo()?.after).toBe(1);
    expect(h.redo()?.after).toBe(2);
    expect(h.redo()).toBeNull();
  });

  test('a new push clears redo', () => {
    const h = new History<number>();
    h.push({ label: 'a', before: 0, after: 1 }, 0);
    h.undo();
    h.push({ label: 'c', before: 0, after: 5 }, 100);
    expect(h.canRedo).toBe(false);
    expect(h.size).toEqual({ undo: 1, redo: 0 });
  });

  test('same-key pushes coalesce within the window', () => {
    const h = new History<number>({ coalesceMs: 1000 });
    h.push({ label: 'Edit', before: 0, after: 1, key: 'edit:r1' }, 0);
    h.push({ label: 'Edit', before: 1, after: 2, key: 'edit:r1' }, 500);
    h.push({ label: 'Edit', before: 2, after: 3, key: 'edit:r1' }, 1400);
    expect(h.size.undo).toBe(1);
    expect(h.peekUndo()).toMatchObject({ before: 0, after: 3 });
    // Outside the window → new entry.
    h.push({ label: 'Edit', before: 3, after: 4, key: 'edit:r1' }, 5000);
    expect(h.size.undo).toBe(2);
    // Different key → new entry.
    h.push({ label: 'Edit', before: 4, after: 5, key: 'edit:r2' }, 5100);
    expect(h.size.undo).toBe(3);
  });

  test('undo/redo and seal break coalescing', () => {
    const h = new History<number>({ coalesceMs: 10_000 });
    h.push({ label: 'Edit', before: 0, after: 1, key: 'k' }, 0);
    h.undo();
    h.redo();
    h.push({ label: 'Edit', before: 1, after: 2, key: 'k' }, 1);
    expect(h.size.undo).toBe(2);
    h.seal();
    h.push({ label: 'Edit', before: 2, after: 3, key: 'k' }, 2);
    expect(h.size.undo).toBe(3);
  });

  test('limit, dropIfLatest, isLatest, clear, subscribe', () => {
    const h = new History<number>({ limit: 2 });
    let calls = 0;
    const off = h.subscribe(() => calls++);
    const a = h.push({ label: 'a', before: 0, after: 1 }, 0);
    h.push({ label: 'b', before: 1, after: 2 }, 1);
    const c = h.push({ label: 'c', before: 2, after: 3 }, 2);
    expect(h.size.undo).toBe(2);
    expect(h.isLatest(c.id)).toBe(true);
    expect(h.dropIfLatest(a.id)).toBe(false);
    expect(h.dropIfLatest(c.id)).toBe(true);
    expect(h.peekUndo()?.label).toBe('b');
    h.clear();
    expect(h.canUndo).toBe(false);
    expect(calls).toBe(5);
    off();
    h.push({ label: 'd', before: 0, after: 1 }, 3);
    expect(calls).toBe(5);
    expect(h.getVersion()).toBe(6);
  });
});

describe('stats helpers', () => {
  test('limit state', () => {
    expect(limitState({}, undefined)).toBeNull();
    expect(limitState({ maxMatches: 3 }, { ruleId: 'x', matches: 1 })).toEqual({ limit: 3, used: 1, remaining: 2, exhausted: false });
    expect(limitState({ maxMatches: 1 }, { ruleId: 'x', matches: 4 })).toEqual({ limit: 1, used: 1, remaining: 0, exhausted: true });
  });

  test('formatHits', () => {
    expect(formatHits(0)).toBe('0×');
    expect(formatHits(999)).toBe('999×');
    expect(formatHits(1200)).toBe('1.2k×');
    expect(formatHits(1000)).toBe('1k×');
    expect(formatHits(45_678)).toBe('46k×');
    expect(formatHits(2_000_000)).toBe('2M×');
  });

  test('search', () => {
    const r = { id: 'x', enabled: true, title: 'Pause logins', matchers: [{ type: 'host' as const, pattern: { op: 'equals' as const, value: 'api.example.com', caseSensitive: false } }], action: { type: 'breakpoint' as const, request: true, response: false } };
    expect(ruleMatchesQuery(r, '')).toBe(true);
    expect(ruleMatchesQuery(r, 'login')).toBe(true);
    expect(ruleMatchesQuery(r, 'EXAMPLE pause')).toBe(true);
    expect(ruleMatchesQuery(r, 'breakpoint')).toBe(true);
    expect(ruleMatchesQuery(r, 'mock')).toBe(false);
  });
});

// ---------------------------------------------------------------------------------------
// Integration: the Rules page records RuleSet snapshots for every kind of change.


describe('History over rule-set operations', () => {
  const sm = (value: string) => ({ op: 'equals' as const, value, caseSensitive: false });
  const initial: RuleSet = {
    version: 1,
    items: [
      { type: 'rule', id: 'a', enabled: true, matchers: [{ type: 'host', pattern: sm('a.com') }], action: { type: 'close' } },
      {
        type: 'group',
        id: 'g',
        title: 'G',
        enabled: true,
        collapsed: false,
        items: [{ type: 'rule', id: 'b', enabled: true, matchers: [], action: { type: 'delay', ms: 100, jitterMs: 0 } }],
      },
      { type: 'rule', id: 'c', enabled: true, matchers: [], action: { type: 'reset' } },
    ],
  };

  test('edits, enable toggles, moves and deletes undo and redo in order', () => {
    const h = new History<RuleSet>({ coalesceMs: 1000 });
    let cur = initial;
    const apply = (label: string, next: RuleSet, key?: string, at = 0) => {
      h.push({ label, before: cur, after: next, key }, at);
      cur = next;
    };

    // Two quick edits of the same rule coalesce into one step.
    const a = findRule(cur, 'a')!;
    apply('Edit rule', replaceRule(cur, { ...a, title: 'A' }), 'edit:a', 0);
    apply('Edit rule', replaceRule(cur, { ...a, title: 'A!' }), 'edit:a', 300);
    apply('Disable rule', setItemEnabled(cur, 'c', false), undefined, 5_000);
    apply('Move rule', moveItem(cur, 'c', { type: 'inside', targetId: 'g', at: 'start' })!, undefined, 6_000);
    apply('Delete rule', removeItem(cur, 'b')!.set, undefined, 7_000);

    expect(h.size.undo).toBe(4);
    expect(findRule(cur, 'b')).toBeUndefined();
    expect(positionOf(cur, 'c')).toEqual({ parentId: 'g', index: 0 });

    // Undo everything.
    cur = h.undo()!.before; // delete
    expect(findRule(cur, 'b')).toBeDefined();
    cur = h.undo()!.before; // move
    expect(positionOf(cur, 'c')).toEqual({ parentId: null, index: 2 });
    cur = h.undo()!.before; // disable
    expect(findRule(cur, 'c')?.enabled).toBe(true);
    cur = h.undo()!.before; // both edits at once
    expect(findRule(cur, 'a')?.title).toBeUndefined();
    expect(cur).toBe(initial);
    expect(h.undo()).toBeNull();

    // Redo everything back.
    for (let i = 0; i < 4; i++) cur = h.redo()!.after;
    expect(findRule(cur, 'a')?.title).toBe('A!');
    expect(findRule(cur, 'c')?.enabled).toBe(false);
    expect(positionOf(cur, 'c')).toEqual({ parentId: 'g', index: 0 });
    expect(findRule(cur, 'b')).toBeUndefined();
  });
});
