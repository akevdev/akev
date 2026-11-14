import { afterEach, describe, expect, mock, test } from 'bun:test';
import { api } from '../api/client';
import type { PausedMessage } from '../api/types';
import { clearInjectedHistory, HISTORY_LIMIT, injectedSeqs, injectMessage, resumeAllPausedMessages, resumePausedMessage, useWsToolsStore, wsToolsReducers } from './wsTools';

const paused = (pauseId: number, id = 1): PausedMessage => ({ id, pauseId, direction: 'sent', kind: 'text', payload: 'aGk=', at: 0, rule: 'r' });

const original = { ...api.breakpoints, sendMessage: api.exchanges.sendMessage };

afterEach(() => {
  useWsToolsStore.setState({ paused: [], history: {} });
  api.breakpoints.resumeMessage = original.resumeMessage;
  api.breakpoints.resumeAllMessages = original.resumeAllMessages;
  api.exchanges.sendMessage = original.sendMessage;
});

describe('paused message reducers', () => {
  test('hello replaces, paused upserts in pause order, resumed removes', () => {
    wsToolsReducers.hello([paused(3), paused(1)]);
    expect(useWsToolsStore.getState().paused.map((p) => p.pauseId)).toEqual([1, 3]);
    wsToolsReducers.paused(paused(2, 5));
    wsToolsReducers.paused(paused(2, 5));
    expect(useWsToolsStore.getState().paused.map((p) => p.pauseId)).toEqual([1, 2, 3]);
    const before = useWsToolsStore.getState().paused;
    wsToolsReducers.resumed(99);
    expect(useWsToolsStore.getState().paused).toBe(before);
    wsToolsReducers.resumed(1);
    expect(useWsToolsStore.getState().paused.map((p) => p.pauseId)).toEqual([2, 3]);
    wsToolsReducers.hello([]);
    expect(useWsToolsStore.getState().paused).toEqual([]);
  });

  test('resuming calls the API and removes the message(s) right away', async () => {
    const resume = mock(async () => undefined);
    const resumeAll = mock(async () => ({ resumed: 2 }));
    api.breakpoints.resumeMessage = resume as typeof api.breakpoints.resumeMessage;
    api.breakpoints.resumeAllMessages = resumeAll as typeof api.breakpoints.resumeAllMessages;
    wsToolsReducers.hello([paused(1, 7), paused(2, 7), paused(3, 8)]);
    await resumePausedMessage(1, { action: 'drop' });
    expect(resume).toHaveBeenCalledWith(1, { action: 'drop' });
    expect(useWsToolsStore.getState().paused.map((p) => p.pauseId)).toEqual([2, 3]);
    expect(await resumeAllPausedMessages(7, 'forward')).toBe(2);
    expect(resumeAll).toHaveBeenCalledWith({ id: 7, action: 'forward' });
    expect(useWsToolsStore.getState().paused.map((p) => p.pauseId)).toEqual([3]);
  });
});

describe('injection history', () => {
  test('records injected messages per exchange, bounded, with their seq', async () => {
    let seq = 10;
    api.exchanges.sendMessage = mock(async () => ({ seq: seq++ })) as typeof api.exchanges.sendMessage;
    await injectMessage(4, { direction: 'sent', kind: 'text', text: 'a' }, 'text', 'a');
    const entry = await injectMessage(4, { direction: 'received', kind: 'binary', base64: 'AA==' }, 'hex', '00');
    expect(entry).toMatchObject({ direction: 'received', mode: 'hex', input: '00', seq: 11 });
    expect(injectedSeqs(useWsToolsStore.getState().history[4] ?? [])).toEqual(new Set([10, 11]));
    for (let i = 0; i < HISTORY_LIMIT + 5; i++) await injectMessage(5, { direction: 'sent', text: String(i) }, 'text', String(i));
    const list = useWsToolsStore.getState().history[5] ?? [];
    expect(list.length).toBe(HISTORY_LIMIT);
    expect(list.at(-1)?.input).toBe(String(HISTORY_LIMIT + 4));
    clearInjectedHistory(5);
    expect(useWsToolsStore.getState().history[5]).toBeUndefined();
    wsToolsReducers.forget('all');
    expect(useWsToolsStore.getState().history).toEqual({});
  });

  test('a still-queued message has no seq', async () => {
    api.exchanges.sendMessage = mock(async () => ({ seq: null })) as typeof api.exchanges.sendMessage;
    const entry = await injectMessage(1, { direction: 'sent', text: 'x' }, 'text', 'x');
    expect(entry.seq).toBeNull();
    expect(injectedSeqs([entry]).size).toBe(0);
  });
});
