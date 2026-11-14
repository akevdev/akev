import { afterEach, describe, expect, test } from 'bun:test';
import { summary } from '../../../test/render';
import { trafficStore } from '../../../state/traffic';
import { useUiStore } from '../../../state/ui';
import { closeFirstRun, evaluateFirstRun, FIRST_RUN_DONE_PREF, firstNewExchange, openFirstRun, shouldAutoOpen, useFirstRun } from './firstRun';

afterEach(() => {
  useFirstRun.setState({ status: 'pending', step: 0, baseline: 0 });
  useUiStore.getState().setPagePref(FIRST_RUN_DONE_PREF, undefined);
  load([]);
});

/** A `hello` snapshot, published right away. */
function load(list: ReturnType<typeof summary>[]): void {
  trafficStore.reset(list);
  trafficStore.flushNow();
}

describe('first-run setup', () => {
  test('opens by itself only when never finished and nothing was captured', () => {
    expect(shouldAutoOpen({ pages: {} }, 0)).toBe(true);
    expect(shouldAutoOpen({ pages: {} }, 3)).toBe(false);
    expect(shouldAutoOpen({ pages: { [FIRST_RUN_DONE_PREF]: true } }, 0)).toBe(false);
  });

  test('decides once, after the first snapshot', () => {
    load([]);
    evaluateFirstRun();
    expect(useFirstRun.getState().status).toBe('open');
    // Later snapshots don't change the decision (e.g. the user clears traffic).
    closeFirstRun();
    evaluateFirstRun();
    expect(useFirstRun.getState().status).toBe('closed');
  });

  test('a restored session means it is not a first launch', () => {
    load([summary(1), summary(2)]);
    evaluateFirstRun();
    expect(useFirstRun.getState().status).toBe('closed');
  });

  test('finishing remembers it in the UI preferences', () => {
    load([]);
    evaluateFirstRun();
    closeFirstRun();
    expect(useUiStore.getState().pages[FIRST_RUN_DONE_PREF]).toBe(true);
    useFirstRun.setState({ status: 'pending' });
    evaluateFirstRun();
    expect(useFirstRun.getState().status).toBe('closed');
  });

  test('reopening waits for a request newer than the ones captured', () => {
    load([summary(4), summary(9)]);
    openFirstRun();
    expect(useFirstRun.getState()).toMatchObject({ status: 'open', step: 0, baseline: 9 });
  });

  test('finds the first exchange after the baseline', () => {
    expect(firstNewExchange([], 0)).toBeNull();
    expect(firstNewExchange([1, 2, 3], 0)).toBe(1);
    expect(firstNewExchange([1, 2, 3], 2)).toBe(3);
    expect(firstNewExchange([1, 2, 3], 3)).toBeNull();
    expect(firstNewExchange([2, 5, 8, 13], 6)).toBe(8);
  });
});
