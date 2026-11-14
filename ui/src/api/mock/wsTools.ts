/**
 * Mock daemon: WebSocket tools — injecting messages into live mock WebSockets and messages
 * paused at `ws-breakpoint` rules (`/api/exchanges/:id/messages` POST,
 * `/api/breakpoints/messages*`, `message-paused` / `message-resumed` on the stream).
 */

import { base64ToBytes, bytesToBase64, utf8DecodeStrict, utf8Encode } from '../../lib/encoding';
import type { Direction, MessageResume, PausedMessage, ServerMessage, StreamMessageWithPayload, WsMessageBody } from '../types';
import { RULE_IDS } from './data';
import type { MockRecord } from './generator';

export interface WsToolsHost {
  record(id: number): MockRecord | undefined;
  broadcast(msg: ServerMessage): void;
  /** A record's summary changed (re-send it in the next `exchanges` batch). */
  touched(id: number): void;
}

interface Reply {
  status?: number;
  json?: unknown;
}

type Route = (method: string, pattern: RegExp, handler: (m: RegExpExecArray, url: URL, body: unknown) => Reply) => void;

const fail = (status: number, code: string, message: string): Reply => ({ status, json: { error: { code, message } } });

/** The payload of an API message body, or an error reply. */
export function decodeBody(body: WsMessageBody, defaultKind: 'text' | 'binary'): { kind: 'text' | 'binary'; bytes: Uint8Array } | Reply | null {
  if (body.text != null && body.base64 != null) return fail(400, 'bad-request', 'give either `text` or `base64`, not both');
  if (body.text == null && body.base64 == null) return null;
  let bytes: Uint8Array;
  try {
    bytes = body.text != null ? utf8Encode(body.text) : base64ToBytes(body.base64 ?? '');
  } catch {
    return fail(400, 'bad-request', 'invalid base64');
  }
  const kind = body.kind ?? (body.text != null ? 'text' : defaultKind);
  if (kind !== 'text' && kind !== 'binary') return fail(400, 'bad-request', '`kind` must be `text` or `binary`');
  if (kind === 'text' && utf8DecodeStrict(bytes) == null) return fail(400, 'bad-request', 'a text message must be valid UTF-8');
  return { kind, bytes };
}

export class MockWsTools {
  private readonly pausedById = new Map<number, PausedMessage>();
  private nextPauseId = 1;

  constructor(private readonly host: WsToolsHost) {}

  /** Paused messages, oldest first. */
  paused(): PausedMessage[] {
    return [...this.pausedById.values()];
  }

  /** Pause a message on `rec` (a live mock WebSocket), as a `ws-breakpoint` rule would. */
  pause(rec: MockRecord, direction: Direction, text: string, rule: string, at: number, announce: boolean): PausedMessage {
    const p: PausedMessage = { id: rec.summary.id, pauseId: this.nextPauseId++, direction, kind: 'text', payload: bytesToBase64(utf8Encode(text)), at: Math.round(at * 1000), rule };
    this.pausedById.set(p.pauseId, p);
    if (announce) this.host.broadcast({ type: 'message-paused', message: p });
    return p;
  }

  /** The demo pause: a chat subscription held by the mock `ws-breakpoint` rule. */
  pauseSample(rec: MockRecord, at: number): PausedMessage {
    const text = JSON.stringify({ type: 'subscribe', channel: 'orders', token: 'eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhZGEifQ' });
    return this.pause(rec, 'sent', text, RULE_IDS.wsPause, at, false);
  }

  /** Forget paused messages of removed exchanges. */
  forget(ids: Iterable<number> | 'all'): void {
    for (const p of this.paused()) {
      if (ids === 'all' || [...ids].includes(p.id)) this.release(p, null);
    }
  }

  register(route: Route): void {
    route('POST', /^\/api\/exchanges\/(\d+)\/messages$/, (m, _u, body) => {
      const rec = this.host.record(Number(m[1]));
      if (!rec) return fail(404, 'not-found', `exchange ${m[1]} not found`);
      if (rec.summary.kind !== 'web-socket' || rec.summary.state !== 'streaming') return fail(409, 'conflict', `exchange ${m[1]} is not an open WebSocket`);
      const b = (body ?? {}) as WsMessageBody & { direction?: Direction };
      if (b.direction !== 'sent' && b.direction !== 'received') return fail(400, 'bad-request', '`direction` must be `sent` or `received`');
      const decoded = decodeBody(b, 'binary');
      if (decoded == null) return fail(400, 'bad-request', '`text` or `base64` is required');
      if (!('bytes' in decoded)) return decoded;
      const msg = this.append(rec, b.direction, decoded.kind, decoded.bytes, true);
      return { status: 202, json: { seq: msg.seq } };
    });
    route('GET', /^\/api\/breakpoints\/messages$/, () => ({ json: this.paused() }));
    route('POST', /^\/api\/breakpoints\/messages\/resume-all$/, (_m, _u, body) => {
      const b = (body ?? {}) as { id?: number; action?: string };
      if (b.action != null && b.action !== 'forward' && b.action !== 'drop') return fail(400, 'bad-request', `unknown action ${JSON.stringify(b.action)}`);
      const targets = this.paused().filter((p) => b.id == null || p.id === b.id);
      for (const p of targets) this.release(p, b.action === 'drop' ? null : { action: 'forward' });
      return { json: { resumed: targets.length } };
    });
    route('POST', /^\/api\/breakpoints\/messages\/(\d+)\/resume$/, (m, _u, body) => {
      const p = this.pausedById.get(Number(m[1]));
      if (!p) return fail(404, 'not-found', `message ${m[1]} is not paused`);
      const resume = (body && Object.keys(body).length ? body : { action: 'forward' }) as MessageResume;
      if (resume.action !== 'forward' && resume.action !== 'drop') return fail(400, 'bad-request', 'expected `forward` or `drop`');
      if (resume.action === 'forward') {
        const decoded = decodeBody(resume, p.kind);
        if (decoded && !('bytes' in decoded)) return decoded;
      }
      this.release(p, resume.action === 'drop' ? null : resume);
      return { status: 204 };
    });
  }

  /** Resolve a pause: forward (possibly edited) or drop (`null`). */
  private release(p: PausedMessage, resume: MessageResume | null): void {
    this.pausedById.delete(p.pauseId);
    const rec = this.host.record(p.id);
    if (rec && resume?.action === 'forward') {
      const decoded = decodeBody(resume, p.kind);
      const edited = decoded && 'bytes' in decoded ? decoded : null;
      const original = base64ToBytes(p.payload);
      if (edited) this.append(rec, p.direction, edited.kind, edited.bytes, true);
      else this.append(rec, p.direction, p.kind, original, false);
    }
    this.host.broadcast({ type: 'message-resumed', id: p.id, pauseId: p.pauseId });
  }

  private append(rec: MockRecord, direction: Direction, kind: 'text' | 'binary', bytes: Uint8Array, modified: boolean): StreamMessageWithPayload {
    const text = kind === 'text' ? utf8DecodeStrict(bytes) : null;
    const msg: StreamMessageWithPayload = {
      seq: rec.messages.length,
      direction,
      kind,
      at: Date.now() * 1000,
      size: bytes.length,
      ...(modified ? { modified: true } : {}),
      text,
      base64: text == null && bytes.length ? bytesToBase64(bytes) : null,
    };
    rec.messages.push(msg);
    rec.summary = { ...rec.summary, messageCount: rec.messages.length, ...(modified ? { modified: true } : {}) };
    const { text: _t, base64: _b, ...meta } = msg;
    this.host.broadcast({ type: 'messages', id: rec.summary.id, messages: [meta] });
    this.host.touched(rec.summary.id);
    return msg;
  }
}
