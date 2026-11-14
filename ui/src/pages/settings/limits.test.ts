import { describe, expect, test } from 'bun:test';
import { mockSettings } from '../../api/mock/data';
import type { Settings, StatusStats } from '../../api/types';
import { setBreakpoints } from './context';
import { bytesToGb, gbToBytes, limitCounters, spillOnDisk } from './limits';
import { mergeSettings } from './transfer';

const stats = (patch: Partial<StatusStats> = {}): StatusStats => ({
  exchanges: 1,
  memoryBodyBytes: 0,
  spilledBodyBytes: 0,
  activeConnections: 0,
  totalConnections: 0,
  evicted: 0,
  droppedBodies: 0,
  diskDroppedBodies: 0,
  passedThrough: 0,
  autoReleased: 0,
  spillDiskBytes: 0,
  refusedConnections: 0,
  authFailures: 0,
  ...patch,
});

describe('resource limits', () => {
  test('editing one breakpoint limit keeps the other', () => {
    const s = mockSettings();
    expect(setBreakpoints('autoReleaseSecs', 30)(s).breakpoints).toEqual({ ...s.breakpoints, autoReleaseSecs: 30 });
  });

  test('GB conversions', () => {
    expect(bytesToGb(4 * 1024 ** 3)).toBe(4);
    expect(gbToBytes(1.5)).toBe(1.5 * 1024 ** 3);
  });

  test('only counters that moved are reported', () => {
    expect(limitCounters(null)).toEqual([]);
    expect(limitCounters(stats())).toEqual([]);
    const lines = limitCounters(stats({ evicted: 1200, droppedBodies: 3, diskDroppedBodies: 0, passedThrough: 1, autoReleased: 2 }));
    expect(lines).toEqual([
      '1,200 oldest exchanges removed (max exchanges)',
      '3 bodies not recorded (capture overloaded)',
      '1 exchange passed through without body rules (rule memory used up)',
      '2 breakpoints released automatically or skipped',
    ]);
    expect(spillOnDisk(stats())).toBeNull();
    expect(spillOnDisk(stats({ spillDiskBytes: 2048 }))).toContain('on disk');
  });

  test('config import keeps and type-checks the new limits', () => {
    const cur: Settings = mockSettings();
    const merged = mergeSettings(cur, {
      proxy: { bodyIdleTimeoutMs: 60_000, ruleBodyBudgetBytes: 'lots' },
      capture: { spillMaxBytes: 0, eventQueueBytes: 8 * 1024 * 1024 },
      breakpoints: { maxPaused: 5, autoReleaseSecs: -1 },
    });
    expect(merged.proxy.bodyIdleTimeoutMs).toBe(60_000);
    expect(merged.proxy.ruleBodyBudgetBytes).toBe(cur.proxy.ruleBodyBudgetBytes);
    expect(merged.capture.spillMaxBytes).toBe(0);
    expect(merged.capture.eventQueueBytes).toBe(8 * 1024 * 1024);
    expect(merged.breakpoints).toEqual({ maxPaused: 5, autoReleaseSecs: 0 });
  });
});
