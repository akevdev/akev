/**
 * Test preload, after `dom.ts`: unmount what component tests rendered and undo their stubs
 * (`stub()` / `afterThisTest()` in `render.tsx`) after every test.
 */
import { afterEach } from 'bun:test';
import { cleanup } from '@testing-library/react';
import { dismissConfirm } from '../components/ui/confirm';

const restores: (() => void)[] = [];

/** Queue `fn` to run after the current test. */
export function afterCurrentTest(fn: () => void): void {
  restores.push(fn);
}

afterEach(() => {
  dismissConfirm();
  cleanup();
  while (restores.length) restores.pop()?.();
});
