import { describe, expect, test } from 'bun:test';
import { defaultColumns, defaultPrefs, sanitizePrefs, TRAFFIC_COLUMNS, useUiStore } from './ui';

describe('UI preferences', () => {
  test('defaults are valid and complete', () => {
    const d = defaultPrefs();
    expect(sanitizePrefs(d)).toEqual(d);
    expect(d.columns.map((c) => c.id)).toEqual(TRAFFIC_COLUMNS.map((c) => c.id));
    expect(d.columns.find((c) => c.id === 'id')?.visible).toBe(false);
    expect(d.columns.find((c) => c.id === 'method')?.visible).toBe(true);
  });

  test('garbage input falls back to defaults field by field', () => {
    expect(sanitizePrefs(null)).toEqual(defaultPrefs());
    expect(sanitizePrefs('nope')).toEqual(defaultPrefs());
    const p = sanitizePrefs({
      theme: 'neon',
      density: 'comfortable',
      scale: 3,
      monoPaths: 'yes',
      splitRatios: { a: 0.3, b: 7, c: 'x' },
      lastTabs: { 'traffic.request': 'body', bad: 4 },
      recentFilters: ['a', '', 3, 'b'],
      savedFilters: [{ name: 'Errors', query: 'status:>=400' }, { name: 1 }],
      sort: { column: 'size', direction: 'sideways' },
      updatedAt: 'later',
    });
    expect(p.theme).toBe('system');
    expect(p.density).toBe('comfortable');
    expect(p.scale).toBe(1);
    expect(p.monoPaths).toBe(false);
    expect(p.splitRatios).toEqual({ a: 0.3 });
    expect(p.lastTabs).toEqual({ 'traffic.request': 'body' });
    expect(p.recentFilters).toEqual(['a', 'b']);
    expect(p.savedFilters).toEqual([{ name: 'Errors', query: 'status:>=400' }]);
    expect(p.sort).toEqual({ column: 'size', direction: 'asc' });
    expect(p.updatedAt).toBe(0);
  });

  test('columns: unknown/duplicate entries dropped, widths clamped, new columns inserted, indicator first', () => {
    const cols = sanitizePrefs({
      columns: [
        { id: 'path', visible: true, width: 10 }, // below min → clamped
        { id: 'indicator', visible: false, width: 6 }, // fixed → always visible
        { id: 'bogus', visible: true, width: 100 },
        { id: 'path', visible: false, width: 400 }, // duplicate
        { id: 'host', visible: false, width: 99999 },
      ],
    }).columns;
    expect(cols[0]).toEqual({ id: 'indicator', visible: true, width: 6 });
    expect(cols.find((c) => c.id === 'path')?.width).toBe(TRAFFIC_COLUMNS.find((c) => c.id === 'path')?.minWidth);
    expect(cols.find((c) => c.id === 'host')).toEqual({ id: 'host', visible: false, width: 2000 });
    expect(cols.some((c) => (c.id as string) === 'bogus')).toBe(false);
    expect(new Set(cols.map((c) => c.id)).size).toBe(TRAFFIC_COLUMNS.length);
  });

  test('store actions bump updatedAt and validate input', () => {
    const s = useUiStore.getState();
    const before = s.updatedAt;
    s.setTheme('midnight');
    expect(useUiStore.getState().theme).toBe('midnight');
    expect(useUiStore.getState().updatedAt).toBeGreaterThanOrEqual(before);
    s.setSplitRatio('x', 1.7);
    expect(useUiStore.getState().splitRatios.x).toBe(1);
    s.setSplitRatio('x', Number.NaN);
    expect(useUiStore.getState().splitRatios.x).toBe(1);
    s.setColumnWidth('size', 1);
    expect(useUiStore.getState().columns.find((c) => c.id === 'size')?.width).toBe(TRAFFIC_COLUMNS.find((c) => c.id === 'size')?.minWidth);
    s.setColumnVisible('indicator', false);
    expect(useUiStore.getState().columns.find((c) => c.id === 'indicator')?.visible).toBe(true);
    s.moveColumn(0, 3); // indicator can't move
    expect(useUiStore.getState().columns[0]?.id).toBe('indicator');
    s.moveColumn(2, 4);
    expect(useUiStore.getState().columns[4]?.id).toBe(defaultColumns()[2]?.id);
    s.resetColumns();
    expect(useUiStore.getState().columns).toEqual(defaultColumns());
  });

  test('recent filters are de-duplicated, newest first, capped at 20', () => {
    const s = useUiStore.getState();
    for (let i = 0; i < 25; i++) s.pushRecentFilter(`q${i}`);
    s.pushRecentFilter('q10');
    s.pushRecentFilter('   ');
    const recent = useUiStore.getState().recentFilters;
    expect(recent.length).toBe(20);
    expect(recent[0]).toBe('q10');
    expect(recent.filter((q) => q === 'q10').length).toBe(1);
  });

  test('cycleTheme goes system → dark → light → system', () => {
    const s = useUiStore.getState();
    s.setTheme('system');
    s.cycleTheme();
    expect(useUiStore.getState().theme).toBe('dark');
    s.cycleTheme();
    expect(useUiStore.getState().theme).toBe('light');
    s.cycleTheme();
    expect(useUiStore.getState().theme).toBe('system');
  });

  test('applyRemote: newer daemon prefs win, older ones are ignored', () => {
    const s = useUiStore.getState();
    s.setTheme('light');
    const localUpdated = useUiStore.getState().updatedAt;
    s.applyRemote({ ...defaultPrefs(), theme: 'contrast', updatedAt: localUpdated - 1000 });
    expect(useUiStore.getState().theme).toBe('light');
    s.applyRemote({ ...defaultPrefs(), theme: 'contrast', updatedAt: localUpdated + 1000 });
    expect(useUiStore.getState().theme).toBe('contrast');
    expect(useUiStore.getState().updatedAt).toBe(localUpdated + 1000);
  });

  test('page prefs are namespaced', () => {
    useUiStore.getState().setPagePref('connect.layout', 'grid');
    expect(useUiStore.getState().pages['connect.layout']).toBe('grid');
  });
});
