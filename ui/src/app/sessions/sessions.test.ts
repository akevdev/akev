import { describe, expect, test } from 'bun:test';
import { ApiClient, type Transport, type WebSocketLike } from '../../api/client';
import { MockServer } from '../../api/mock/server';
import type { SessionInfo } from '../../api/types';
import {
  captureKind,
  closeSessionsDialog,
  defaultSessionName,
  describeSession,
  formatInterval,
  getSessionsDialogState,
  importAccept,
  openSessionsDialog,
  restoredTitle,
  saveRequest,
  scopeCount,
  shouldAnnounceRestore,
  sortSessions,
  validateSessionName,
  type SaveContext,
} from './sessions';

const session = (patch: Partial<SessionInfo>): SessionInfo => ({ id: 'x', name: 'X', kind: 'saved', exchanges: 1, size: 100, createdAt: 0, updatedAt: 0, ...patch });

describe('session names', () => {
  test('validation', () => {
    expect(validateSessionName('Checkout bug')).toBeNull();
    expect(validateSessionName('  ')).toBe('Enter a name');
    expect(validateSessionName('a\nb')).toContain('control characters');
    expect(validateSessionName('x'.repeat(201))).toContain('200');
    expect(validateSessionName('é'.repeat(200))).toBeNull();
  });

  test('default name from the current time', () => {
    expect(defaultSessionName(new Date(2026, 10, 9, 7, 5))).toBe('Traffic 2026-11-09 07:05');
  });

  test('description line', () => {
    const now = Date.UTC(2026, 10, 9, 12);
    expect(describeSession({ exchanges: 1234, size: 4_812_330, updatedAt: now - 120_000 }, now)).toBe('1,234 exchanges · 4.6 MB · 2m ago');
    expect(describeSession({ exchanges: 1, size: 0, updatedAt: now }, now)).toMatch(/^1 exchange · /);
  });

  test('autosaves first, then saved sessions newest first', () => {
    const list = [
      session({ id: 'old', name: 'Old', updatedAt: 1 }),
      session({ id: 'autosave-previous', kind: 'autosave', updatedAt: 50 }),
      session({ id: 'new', name: 'New', updatedAt: 9 }),
      session({ id: 'autosave', kind: 'autosave', updatedAt: 5 }),
    ];
    expect(sortSessions(list).map((s) => s.id)).toEqual(['autosave', 'autosave-previous', 'new', 'old']);
    expect(list[0]?.id).toBe('old'); // not mutated
  });
});

describe('saving', () => {
  const ctx: SaveContext = { total: 10, visible: [2, 4, 6], selected: [4, 6], filter: null };

  test('scope counts', () => {
    expect(scopeCount('all', ctx)).toBe(10);
    expect(scopeCount('visible', ctx)).toBe(3);
    expect(scopeCount('selected', ctx)).toBe(2);
  });

  test('request bodies: everything, ids, or the filter (short for any size)', () => {
    expect(saveRequest('  A  ', 'all', ctx)).toEqual({ name: 'A' });
    expect(saveRequest('A', 'selected', ctx)).toEqual({ name: 'A', ids: [4, 6] });
    expect(saveRequest('A', 'visible', ctx)).toEqual({ name: 'A', ids: [2, 4, 6] });
    expect(saveRequest('A', 'visible', { ...ctx, filter: 'status:500' })).toEqual({ name: 'A', filter: 'status:500' });
    // Nothing filtered: "visible" is everything.
    expect(saveRequest('A', 'visible', { ...ctx, visible: Array.from({ length: 10 }, (_, i) => i) })).toEqual({ name: 'A' });
  });
});

describe('settings & restore', () => {
  test('intervals', () => {
    expect(formatInterval(45)).toBe('45 s');
    expect(formatInterval(60)).toBe('1 min');
    expect(formatInterval(90)).toBe('90 s');
    expect(formatInterval(600)).toBe('10 min');
    expect(formatInterval(7200)).toBe('2 hours');
  });

  test('restore toast: title and once per daemon run', () => {
    expect(restoredTitle({ exchanges: 1234 })).toBe('Restored autosaved session (1,234 exchanges)');
    expect(restoredTitle({ exchanges: 1 })).toBe('Restored autosaved session (1 exchange)');
    expect(shouldAnnounceRestore(1000, null)).toBe(true);
    expect(shouldAnnounceRestore(1000, '1000')).toBe(false);
    expect(shouldAnnounceRestore(2000, '1000')).toBe(true);
  });

  test('dialog state', () => {
    openSessionsDialog({ save: true });
    expect(getSessionsDialogState()).toEqual({ open: true, focusSave: true });
    openSessionsDialog();
    expect(getSessionsDialogState()).toEqual({ open: true, focusSave: false });
    closeSessionsDialog();
    expect(getSessionsDialogState().open).toBe(false);
  });

  test('import file filters', () => {
    expect(importAccept('charles')).toContain('.chlsj');
    expect(importAccept('mitmproxy')).toBeUndefined();
  });
});

describe('sessions API (client ⇄ mock daemon)', () => {
  function client(): { api: ApiClient; server: MockServer } {
    const server = new MockServer();
    const transport: Transport = { fetch: (u, i) => server.fetch(u, i), openSocket: () => ({}) as WebSocketLike };
    return { api: new ApiClient({ baseUrl: '', token: null }, transport), server };
  }

  test('list, save, rename, open, delete', async () => {
    const { api, server } = client();
    server.start();
    try {
      const list = await api.sessions.list();
      expect(list.length).toBeGreaterThan(1);
      expect(list.map((s) => s.updatedAt)).toEqual([...list.map((s) => s.updatedAt)].sort((a, b) => b - a));
      const saved = await api.sessions.save({ name: 'Checkout bug' });
      expect(saved.id).toBe('checkout-bug-2'); // the seed already has `checkout-bug`
      expect(saved.exchanges).toBeGreaterThan(100);
      const some = await api.sessions.save({ name: 'Two', ids: list.length ? [1, 2] : [] });
      expect(some.exchanges).toBe(2);
      await expect(api.sessions.save({ name: ' ' })).rejects.toMatchObject({ status: 400 });
      const renamed = await api.sessions.rename(saved.id, 'Payment flow');
      expect(renamed).toMatchObject({ id: saved.id, name: 'Payment flow' });
      // Keeping an autosave makes it a saved session with a new id.
      const kept = await api.sessions.rename('autosave', 'Monday');
      expect(kept).toMatchObject({ id: 'monday', kind: 'saved' });
      const opened = await api.sessions.open('monday', 'append');
      expect(opened.ids.length).toBeGreaterThan(0);
      await api.sessions.remove('monday');
      await expect(api.sessions.remove('monday')).rejects.toMatchObject({ status: 404 });
      expect(api.sessions.fileUrl('a b')).toContain('/api/sessions/a%20b');
    } finally {
      server.stop();
    }
  });

  test('imports and the explicit rebind', async () => {
    const { api, server } = client();
    server.start();
    try {
      const charles = await api.transfer.importCharles(JSON.stringify([{ host: 'a.example' }, { host: 'b.example' }]));
      expect(charles.ids.length).toBe(2);
      await expect(api.transfer.importCharles('{"log":{}}')).rejects.toMatchObject({ status: 400 });
      await expect(api.transfer.importMitmproxy(new TextEncoder().encode('not flows'))).rejects.toMatchObject({ status: 400 });
      const flows = await api.transfer.importMitmproxy(new TextEncoder().encode('12:4:type;4:http;}'));
      expect(flows.ids.length).toBeGreaterThan(0);
      const moved = await api.proxy.rebind({ port: 8123 });
      expect(moved.port).toBe(8123);
      expect((await api.status()).proxy.port).toBe(8123);
      await expect(api.proxy.rebind({ port: 9 })).rejects.toMatchObject({ status: 409, code: 'conflict' });
      await expect(api.proxy.rebind({ port: 0 })).rejects.toMatchObject({ status: 400 });
    } finally {
      server.stop();
    }
  });
});

describe('sessions API wire format', () => {
  test('methods, paths and bodies', async () => {
    const calls: { method: string; url: string; body: unknown; type: string | null }[] = [];
    const transport: Transport = {
      fetch: async (url, init) => {
        const headers = new Headers(init.headers);
        calls.push({ method: init.method ?? 'GET', url, body: typeof init.body === 'string' ? JSON.parse(init.body) : init.body, type: headers.get('content-type') });
        return init.method === 'DELETE' ? new Response(null, { status: 204 }) : new Response('{"ids":[]}', { status: 200 });
      },
      openSocket: () => ({}) as WebSocketLike,
    };
    const api = new ApiClient({ baseUrl: 'http://d', token: 't' }, transport);
    await api.sessions.list();
    await api.sessions.save({ name: 'N', filter: 'status:500' });
    await api.sessions.rename('a/b', 'M');
    await api.sessions.remove('x');
    await api.sessions.open('x', 'append');
    await api.transfer.importCharles('[]');
    await api.transfer.importMitmproxy(new Uint8Array([1]));
    await api.proxy.rebind({ port: 8000, bindAddress: '127.0.0.1' });
    expect(calls.map((c) => `${c.method} ${c.url.replace('http://d', '')}`)).toEqual([
      'GET /api/sessions',
      'POST /api/sessions',
      'PUT /api/sessions/a%2Fb',
      'DELETE /api/sessions/x',
      'POST /api/sessions/x/open',
      'POST /api/import/charles',
      'POST /api/import/mitmproxy',
      'POST /api/proxy/rebind',
    ]);
    expect(calls[1]?.body).toEqual({ name: 'N', filter: 'status:500' });
    expect(calls[2]?.body).toEqual({ name: 'M' });
    expect(calls[4]?.body).toEqual({ mode: 'append' });
    expect(calls[5]?.type).toBe('application/json');
    expect(calls[6]?.type).toBe('application/octet-stream');
    expect(calls[7]?.body).toEqual({ port: 8000, bindAddress: '127.0.0.1' });
    expect(api.sessions.fileUrl('x')).toBe('http://d/api/sessions/x?token=t');
  });
});

describe('capture files', () => {
  const bytes = (text: string) => new TextEncoder().encode(text);
  test('by extension, like the desktop file associations', () => {
    expect(captureKind('traffic.HAR', bytes(''))).toBe('har');
    expect(captureKind('exports/x.akev', bytes(''))).toBe('session');
    expect(captureKind('charles.chlsj', bytes(''))).toBe('charles');
    expect(captureKind('fiddler.saz', bytes(''))).toBe('fiddler');
    expect(captureKind('dump.flows', bytes(''))).toBe('mitmproxy');
    expect(captureKind('flows.mitm', bytes(''))).toBe('mitmproxy');
  });

  test('.json and files without an extension are sniffed', () => {
    expect(captureKind('export.json', bytes('﻿  {"log": {"version": "1.2"}}'))).toBe('har');
    expect(captureKind('export.json', bytes('[{"status":"COMPLETE"}]'))).toBe('charles');
    expect(captureKind('session', new Uint8Array([0x28, 0xb5, 0x2f, 0xfd, 1]))).toBe('session');
    expect(captureKind('session', bytes('{"akev":1}\n{"id":1}'))).toBe('session');
    expect(captureKind('archive', new Uint8Array([0x50, 0x4b, 0x03, 0x04, 0]))).toBe('fiddler');
    expect(captureKind('flows', bytes('1234:4:type;'))).toBe('mitmproxy');
  });

  test('anything else is refused', () => {
    expect(captureKind('photo.png', new Uint8Array([0x89, 0x50, 0x4e, 0x47]))).toBeNull();
    expect(captureKind('notes.txt', bytes('{"log":1}'))).toBeNull();
    expect(captureKind('blob', bytes('hello'))).toBeNull();
  });
});
