import { useCallback, useMemo, useState } from 'react';
import type { EditableRequest, EditableResponse, ExchangeId, PausedExchange } from '../../../../api/types';
import { headersToRows, rowsToHeaders, type KeyValueRow } from '../../../../components/ui';
import { useAppStore } from '../../../../state/app';
import {
  decodeEditableBody,
  editableBodySize,
  encodeEditableBody,
  invalidHeaderNames,
  requestChanged,
  responseChanged,
  syncContentLength,
  syncHostHeader,
  validateMethod,
  validateStatus,
  validateUrl,
  type BodyMode,
} from '../lib/breakpoint';

export interface BodyDraft {
  mode: BodyMode;
  value: string;
  /** The original body wasn't valid UTF-8 (edit as base64). */
  binary: boolean;
}

export interface RequestDraft {
  method: string;
  url: string;
  rows: KeyValueRow[];
  body: BodyDraft;
}

export interface ResponseDraft {
  status: number | null;
  message: string;
  rows: KeyValueRow[];
  body: BodyDraft;
}

export type DraftErrors = Partial<Record<'method' | 'url' | 'status' | 'headers' | 'body', string>>;

/** Drafts survive switching to another exchange and back while still paused. */
const requestDrafts = new Map<ExchangeId, RequestDraft>();
const responseDrafts = new Map<ExchangeId, ResponseDraft>();

let pruning = false;
/** Forget drafts of exchanges that are no longer paused (resumed elsewhere, timed out, deleted). */
function prunePausedDrafts(): void {
  if (pruning) return;
  pruning = true;
  useAppStore.subscribe((s, prev) => {
    if (s.paused === prev.paused) return;
    const live = new Set(s.paused.map((p) => p.id));
    for (const id of requestDrafts.keys()) if (!live.has(id)) requestDrafts.delete(id);
    for (const id of responseDrafts.keys()) if (!live.has(id)) responseDrafts.delete(id);
  });
}

function bodyDraft(b64: string): BodyDraft {
  const d = decodeEditableBody(b64);
  return d.text != null ? { mode: 'text', value: d.text, binary: false } : { mode: 'base64', value: d.base64, binary: true };
}

export function requestDraftFrom(req: EditableRequest): RequestDraft {
  return { method: req.method, url: req.url, rows: headersToRows(req.headers), body: bodyDraft(req.body) };
}

export function responseDraftFrom(res: EditableResponse): ResponseDraft {
  return { status: res.status, message: res.statusMessage, rows: headersToRows(res.headers), body: bodyDraft(res.body) };
}

function encode(body: BodyDraft): { base64: string } | { error: string } {
  const r = encodeEditableBody(body.mode, body.value);
  return r.ok ? { base64: r.base64 } : { error: r.error };
}

export function buildRequest(d: RequestDraft): { request: EditableRequest; errors: DraftErrors } {
  const errors: DraftErrors = {};
  const m = validateMethod(d.method);
  if (m) errors.method = m;
  const u = validateUrl(d.url);
  if (u) errors.url = u;
  const bad = invalidHeaderNames(d.rows);
  if (bad.length) errors.headers = `Invalid header name${bad.length > 1 ? 's' : ''}: ${bad.join(', ')}`;
  const body = encode(d.body);
  if ('error' in body) errors.body = body.error;
  return {
    request: { method: d.method.trim().toUpperCase(), url: d.url.trim(), headers: rowsToHeaders(d.rows), body: 'base64' in body ? body.base64 : '' },
    errors,
  };
}

export function buildResponse(d: ResponseDraft): { response: EditableResponse; errors: DraftErrors } {
  const errors: DraftErrors = {};
  const s = validateStatus(d.status);
  if (s) errors.status = s;
  const bad = invalidHeaderNames(d.rows);
  if (bad.length) errors.headers = `Invalid header name${bad.length > 1 ? 's' : ''}: ${bad.join(', ')}`;
  const body = encode(d.body);
  if ('error' in body) errors.body = body.error;
  return {
    response: { status: d.status ?? 200, statusMessage: d.message, headers: rowsToHeaders(d.rows), body: 'base64' in body ? body.base64 : '' },
    errors,
  };
}

/** Apply a body edit and keep Content-Length in sync with the new byte size. */
export function withBody<T extends { rows: KeyValueRow[]; body: BodyDraft }>(draft: T, body: BodyDraft): T {
  const prev = editableBodySize(draft.body.mode, draft.body.value);
  const next = editableBodySize(body.mode, body.value);
  const rows = prev != null && next != null ? syncContentLength(draft.rows, prev, next) : draft.rows;
  return { ...draft, rows, body };
}

/** Apply a URL edit and keep a Host header that tracked the old URL in sync with the new one. */
export function withUrl(draft: RequestDraft, url: string): RequestDraft {
  return { ...draft, url, rows: syncHostHeader(draft.rows, draft.url, url) };
}

export interface BreakpointDraftState {
  stage: 'request' | 'response';
  request: RequestDraft | null;
  response: ResponseDraft | null;
  setRequest: (update: (d: RequestDraft) => RequestDraft) => void;
  setResponse: (update: (d: ResponseDraft) => ResponseDraft) => void;
  /** Discard edits (back to the paused original). */
  revert: () => void;
  /** Number of edited parts (method, URL, headers, body / status, headers, body). */
  changes: number;
  errors: DraftErrors;
  /** Payload to resume with, or `null` when invalid. */
  build: () => { kind: 'request'; request: EditableRequest } | { kind: 'response'; response: EditableResponse } | null;
  /** Forget the stored draft (after resuming). */
  clear: () => void;
}

/** Editable state for a paused exchange, persisted per id until it is resumed. */
export function useBreakpointDraft(paused: PausedExchange): BreakpointDraftState {
  prunePausedDrafts();
  const id = paused.id;
  const stage = paused.stage;
  const [, force] = useState(0);
  const rerender = useCallback(() => force((n) => n + 1), []);

  let request: RequestDraft | null = null;
  let response: ResponseDraft | null = null;
  if (stage === 'request') {
    request = requestDrafts.get(id) ?? null;
    if (!request) {
      request = requestDraftFrom(paused.request);
      requestDrafts.set(id, request);
    }
  } else if (paused.response) {
    response = responseDrafts.get(id) ?? null;
    if (!response) {
      response = responseDraftFrom(paused.response);
      responseDrafts.set(id, response);
    }
  }

  const setRequest = useCallback(
    (update: (d: RequestDraft) => RequestDraft) => {
      const cur = requestDrafts.get(id);
      if (!cur) return;
      requestDrafts.set(id, update(cur));
      rerender();
    },
    [id, rerender],
  );
  const setResponse = useCallback(
    (update: (d: ResponseDraft) => ResponseDraft) => {
      const cur = responseDrafts.get(id);
      if (!cur) return;
      responseDrafts.set(id, update(cur));
      rerender();
    },
    [id, rerender],
  );

  const revert = useCallback(() => {
    if (stage === 'request') requestDrafts.set(id, requestDraftFrom(paused.request));
    else if (paused.response) responseDrafts.set(id, responseDraftFrom(paused.response));
    rerender();
  }, [id, stage, paused, rerender]);

  const clear = useCallback(() => {
    requestDrafts.delete(id);
    responseDrafts.delete(id);
  }, [id]);

  const computed = useMemo(() => {
    if (request) {
      const { request: built, errors } = buildRequest(request);
      const o = paused.request;
      let changes = 0;
      if (built.method !== o.method) changes++;
      if (built.url !== o.url) changes++;
      if (requestChanged({ ...o, method: built.method, url: built.url, body: built.body }, built)) changes++;
      if (requestChanged({ ...built, body: o.body }, built)) changes++;
      return { errors, changes, payload: { kind: 'request' as const, request: built } };
    }
    if (response && paused.response) {
      const { response: built, errors } = buildResponse(response);
      const o = paused.response;
      let changes = 0;
      if (built.status !== o.status || built.statusMessage !== o.statusMessage) changes++;
      if (responseChanged({ ...o, status: built.status, statusMessage: built.statusMessage, body: built.body }, built)) changes++;
      if (responseChanged({ ...built, body: o.body }, built)) changes++;
      return { errors, changes, payload: { kind: 'response' as const, response: built } };
    }
    return { errors: {} as DraftErrors, changes: 0, payload: null };
  }, [request, response, paused]);

  const build = useCallback(() => (Object.keys(computed.errors).length ? null : computed.payload), [computed]);

  return { stage, request, response, setRequest, setResponse, revert, changes: computed.changes, errors: computed.errors, build, clear };
}
