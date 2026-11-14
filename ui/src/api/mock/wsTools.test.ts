import { describe, expect, test } from 'bun:test';
import type { PausedMessage, ServerMessage, StreamMessageWithPayload } from '../types';
import { MockServer } from './server';
import { decodeBody } from './wsTools';

const post = (server: MockServer, path: string, body: unknown) => server.fetch(path, { method: 'POST', body: JSON.stringify(body), headers: { 'content-type': 'application/json' } });

describe('mock WebSocket tools', () => {
  test('decodeBody mirrors the daemon rules', () => {
    expect(decodeBody({ text: 'hi' }, 'binary')).toEqual({ kind: 'text', bytes: new TextEncoder().encode('hi') });
    expect(decodeBody({ base64: '/w==' }, 'binary')).toEqual({ kind: 'binary', bytes: new Uint8Array([0xff]) });
    expect(decodeBody({}, 'text')).toBeNull();
    expect(decodeBody({ kind: 'text', base64: '/w==' }, 'binary')).toMatchObject({ status: 400 });
    expect(decodeBody({ text: 'a', base64: 'YQ==' }, 'text')).toMatchObject({ status: 400 });
  });

  test('a seeded paused message; forward edited, inject both ways, 409 on closed sockets', async () => {
    const server = new MockServer();
    server.start();
    try {
      const socket = server.openSocket();
      const stream: ServerMessage[] = [];
      socket.onmessage = (ev) => stream.push(JSON.parse(String(ev.data)) as ServerMessage);
      await new Promise((r) => setTimeout(r, 50));
      const hello = stream[0];
      expect(hello?.type === 'hello' && hello.pausedMessages?.length).toBe(1);
      const paused = (await (await server.fetch('/api/breakpoints/messages', { method: 'GET' })).json()) as PausedMessage[];
      const p = paused[0] as PausedMessage;
      expect(p).toMatchObject({ direction: 'sent', kind: 'text' });

      const r = await post(server, `/api/breakpoints/messages/${p.pauseId}/resume`, { action: 'forward', text: '{"edited":true}' });
      expect(r.status).toBe(204);
      expect(stream.some((m) => m.type === 'message-resumed' && m.pauseId === p.pauseId)).toBe(true);
      const again = await post(server, `/api/breakpoints/messages/${p.pauseId}/resume`, {});
      expect(again.status).toBe(404);

      const injected = await post(server, `/api/exchanges/${p.id}/messages`, { direction: 'received', kind: 'binary', base64: '3q2+7w==' });
      expect(injected.status).toBe(202);
      const { seq } = (await injected.json()) as { seq: number };
      const messages = (await (await server.fetch(`/api/exchanges/${p.id}/messages`, { method: 'GET' })).json()) as StreamMessageWithPayload[];
      const edited = messages.find((m) => m.text === '{"edited":true}');
      expect(edited).toMatchObject({ direction: 'sent', modified: true });
      expect(messages.find((m) => m.seq === seq)).toMatchObject({ direction: 'received', kind: 'binary', base64: '3q2+7w==', modified: true });
      expect((await post(server, `/api/exchanges/${p.id}/messages`, { direction: 'up', text: 'x' })).status).toBe(400);

      // Any non-WebSocket exchange is a conflict.
      const list = (await (await server.fetch('/api/exchanges?filter=method:post&limit=1', { method: 'GET' })).json()) as { id: number }[];
      expect((await post(server, `/api/exchanges/${list[0]?.id}/messages`, { direction: 'sent', text: 'x' })).status).toBe(409);
    } finally {
      server.stop();
    }
  });
});
