/**
 * Action catalogue for the rule editor ("Then" section): labels, descriptions, whether the
 * action stops evaluation, defaults when switching action types, throttle presets and the
 * documented script template.
 */

import type { Action, ActionType, BodySource, Headers, RequestTransform, ResponseTransform } from '../../api/types';
import { TERMINAL_ACTIONS } from '../../api/types';
import { bytesToBase64, utf8DecodeStrict } from '../encoding';

export type ActionCategory = 'respond' | 'modify' | 'connection' | 'websocket';

export interface ActionInfo {
  type: ActionType;
  label: string;
  /** One-line explanation shown on the picker card. */
  description: string;
  category: ActionCategory;
  /** Icon key mapped to a lucide icon by the page. */
  icon: string;
  /** Stops evaluation (vs. modifiers that stack and continue). */
  terminal: boolean;
}

export const ACTION_CATEGORIES: readonly { id: ActionCategory; label: string; hint: string }[] = [
  { id: 'respond', label: 'Respond', hint: 'Stops evaluation — the first matching respond rule wins' },
  { id: 'modify', label: 'Modify', hint: 'Stacks with later rules — evaluation continues' },
  { id: 'connection', label: 'Fail', hint: 'Stops evaluation and breaks the connection' },
  { id: 'websocket', label: 'WebSocket', hint: 'Handle WebSocket upgrades' },
];

export const ACTIONS: readonly ActionInfo[] = [
  { type: 'mock', label: 'Mock', description: 'Respond with a fixed status, headers and body', category: 'respond', icon: 'mock', terminal: true },
  { type: 'forward', label: 'Map Remote', description: 'Forward to another host or base URL', category: 'respond', icon: 'forward', terminal: true },
  { type: 'map-local', label: 'Map Local', description: 'Serve a local file or folder', category: 'respond', icon: 'map-local', terminal: true },
  { type: 'passthrough', label: 'Pass through', description: 'Send upstream unchanged (stops later rules)', category: 'respond', icon: 'passthrough', terminal: true },
  { type: 'transform', label: 'Rewrite', description: 'Change the request and/or response', category: 'modify', icon: 'transform', terminal: false },
  { type: 'breakpoint', label: 'Breakpoint', description: 'Pause to inspect and edit by hand', category: 'modify', icon: 'breakpoint', terminal: false },
  { type: 'delay', label: 'Delay', description: 'Add latency before handling', category: 'modify', icon: 'delay', terminal: false },
  { type: 'throttle', label: 'Throttle', description: 'Limit bandwidth like a slow network', category: 'modify', icon: 'throttle', terminal: false },
  { type: 'highlight', label: 'Highlight', description: 'Colour matching rows in Traffic', category: 'modify', icon: 'highlight', terminal: false },
  { type: 'script', label: 'Script', description: 'Run JavaScript on the request/response', category: 'modify', icon: 'script', terminal: false },
  { type: 'tls-profile', label: 'TLS fingerprint', description: "Look like a browser to the server (TLS + HTTP/2)", category: 'modify', icon: 'tls-profile', terminal: false },
  { type: 'close', label: 'Close', description: 'Close the connection without a response', category: 'connection', icon: 'close', terminal: true },
  { type: 'reset', label: 'Reset', description: 'Abort with a TCP RST / RST_STREAM', category: 'connection', icon: 'reset', terminal: true },
  { type: 'timeout', label: 'Timeout', description: 'Accept and never respond', category: 'connection', icon: 'timeout', terminal: true },
  { type: 'ws-echo', label: 'Echo', description: 'Accept the WebSocket and echo messages back', category: 'websocket', icon: 'ws-echo', terminal: true },
  { type: 'ws-reject', label: 'Reject', description: 'Refuse the WebSocket upgrade with a status', category: 'websocket', icon: 'ws-reject', terminal: true },
  { type: 'ws-breakpoint', label: 'Pause messages', description: 'Hold messages to edit, drop or forward them', category: 'websocket', icon: 'ws-breakpoint', terminal: false },
];

const BY_TYPE = new Map<ActionType, ActionInfo>(ACTIONS.map((a) => [a.type, a]));

export function actionInfo(type: ActionType): ActionInfo {
  return BY_TYPE.get(type) ?? { type, label: type, description: '', category: 'modify', icon: 'unknown', terminal: TERMINAL_ACTIONS.has(type) };
}

export const SCRIPT_TEMPLATE = `// Runs for every request this rule matches. Both hooks are optional.
//
// req: { method, url, httpVersion, headers: { name: value | [values] }, body }
// res: { status, statusMessage, httpVersion, headers, body }
// console.log() output is captured. Each call has a 2 s CPU / 64 MB budget.

function onRequest(req) {
  // req.headers["x-debug"] = "1";
  // To respond immediately without contacting the server:
  // return { status: 200, headers: { "content-type": "application/json" }, json: { ok: true } };
  return req;
}

function onResponse(req, res) {
  // res.body = res.body.replace("foo", "bar");
  return res;
}
`;

export const DEFAULT_MOCK_HEADERS: Headers = [['content-type', 'application/json']];

/** A fresh action of the given type. `prev` lets compatible settings carry over. */
export function defaultAction(type: ActionType, prev?: Action): Action {
  const transforms = (): { request?: RequestTransform; response?: ResponseTransform } => {
    if (prev && (prev.type === 'transform' || prev.type === 'passthrough' || prev.type === 'forward')) {
      return { ...(prev.request ? { request: prev.request } : {}), ...(prev.response ? { response: prev.response } : {}) };
    }
    return {};
  };
  const status = prev && (prev.type === 'mock' || prev.type === 'ws-reject') ? prev.status : undefined;
  const body: BodySource | undefined = prev && (prev.type === 'mock' || prev.type === 'ws-reject') ? prev.body : undefined;
  switch (type) {
    case 'passthrough':
      return { type, ...transforms() };
    case 'forward':
      return { type, target: 'http://localhost:3000', preserveHost: false, ...transforms() };
    case 'mock':
      return {
        type,
        status: status ?? 200,
        headers: prev?.type === 'map-local' && prev.headers.length ? prev.headers : DEFAULT_MOCK_HEADERS,
        body: body ?? { type: 'json', value: { ok: true } },
      };
    case 'map-local':
      return { type, path: '', fallbackToUpstream: false, headers: [] };
    case 'close':
    case 'reset':
    case 'timeout':
    case 'ws-echo':
      return { type };
    case 'ws-reject':
      return { type, status: status && status >= 400 ? status : 403, body: body ?? { type: 'empty' } };
    case 'transform': {
      const tr = transforms();
      return { type, ...(tr.request || tr.response ? tr : { request: { setHeaders: [['x-akev', '1']] } }) };
    }
    case 'breakpoint':
      return { type, request: true, response: false };
    case 'delay':
      return { type, ms: 1000, jitterMs: 0 };
    case 'throttle':
      return { type, ...presetValues('3g') };
    case 'highlight':
      return { type, color: 'yellow' };
    case 'script':
      return { type, source: SCRIPT_TEMPLATE };
    case 'ws-breakpoint':
      return { type, direction: 'both' };
    case 'tls-profile':
      return { type, profile: 'chrome' };
  }
}

// =====================================================================================
// throttle presets
// =====================================================================================

export type ThrottlePresetId = '3g' | '4g' | 'dsl' | 'wifi' | 'custom';

export interface ThrottlePreset {
  id: Exclude<ThrottlePresetId, 'custom'>;
  label: string;
  /** kbit/s. */
  downKbps: number;
  upKbps: number;
  latencyMs: number;
}

export const THROTTLE_PRESETS: readonly ThrottlePreset[] = [
  { id: '3g', label: '3G', downKbps: 780, upKbps: 330, latencyMs: 100 },
  { id: '4g', label: '4G / LTE', downKbps: 9000, upKbps: 3000, latencyMs: 50 },
  { id: 'dsl', label: 'DSL', downKbps: 2000, upKbps: 256, latencyMs: 20 },
  { id: 'wifi', label: 'Wi-Fi', downKbps: 30000, upKbps: 15000, latencyMs: 2 },
];

/** kbit/s → bytes/s (the wire unit of `throttle`). */
export function kbpsToBps(kbps: number): number {
  return Math.round((kbps * 1000) / 8);
}

/** bytes/s → kbit/s (rounded to 1 decimal). */
export function bpsToKbps(bps: number): number {
  return Math.round(((bps * 8) / 1000) * 10) / 10;
}

export function presetValues(id: Exclude<ThrottlePresetId, 'custom'>): { downloadBps: number; uploadBps: number; latencyMs: number } {
  const p = THROTTLE_PRESETS.find((x) => x.id === id) ?? (THROTTLE_PRESETS[0] as ThrottlePreset);
  return { downloadBps: kbpsToBps(p.downKbps), uploadBps: kbpsToBps(p.upKbps), latencyMs: p.latencyMs };
}

/** Which preset a throttle action corresponds to (or `custom`). */
export function detectPreset(a: { downloadBps: number; uploadBps: number; latencyMs: number }): ThrottlePresetId {
  for (const p of THROTTLE_PRESETS) {
    const v = presetValues(p.id);
    if (v.downloadBps === a.downloadBps && v.uploadBps === a.uploadBps && v.latencyMs === a.latencyMs) return p.id;
  }
  return 'custom';
}

// =====================================================================================
// bodies
// =====================================================================================

/** Editor language for a content type (for the mock body editor). */
export function languageForContentType(contentType: string | undefined): string {
  const ct = (contentType ?? '').toLowerCase();
  if (ct.includes('json')) return 'json';
  if (ct.includes('html')) return 'html';
  if (ct.includes('xml') || ct.includes('svg')) return 'xml';
  if (ct.includes('css')) return 'css';
  if (ct.includes('javascript') || ct.includes('ecmascript')) return 'javascript';
  if (ct.includes('yaml')) return 'yaml';
  if (ct.includes('markdown')) return 'markdown';
  return 'text';
}

/** Content type for an editor language (when the user picks a language). */
export const LANGUAGE_CONTENT_TYPES: Readonly<Record<string, string>> = {
  json: 'application/json',
  html: 'text/html; charset=utf-8',
  xml: 'application/xml',
  css: 'text/css',
  javascript: 'text/javascript',
  yaml: 'application/yaml',
  markdown: 'text/markdown',
  text: 'text/plain; charset=utf-8',
};

// =====================================================================================
// loading a body from a file
// =====================================================================================

const EXTENSION_TYPES: Readonly<Record<string, string>> = {
  json: 'application/json',
  html: 'text/html; charset=utf-8',
  htm: 'text/html; charset=utf-8',
  xml: 'application/xml',
  svg: 'image/svg+xml',
  css: 'text/css',
  js: 'text/javascript',
  mjs: 'text/javascript',
  yaml: 'application/yaml',
  yml: 'application/yaml',
  md: 'text/markdown',
  txt: 'text/plain; charset=utf-8',
  csv: 'text/csv',
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  ico: 'image/x-icon',
  pdf: 'application/pdf',
  wasm: 'application/wasm',
  woff2: 'font/woff2',
  mp4: 'video/mp4',
};

/** Content type for a file name (by extension), if known. */
export function contentTypeForFileName(name: string): string | undefined {
  const ext = /\.([A-Za-z0-9]+)$/.exec(name)?.[1]?.toLowerCase();
  return ext ? EXTENSION_TYPES[ext] : undefined;
}

export interface LoadedBody {
  body: BodySource;
  /** Content type guessed from the file name. */
  contentType: string | undefined;
  /** Editor language for text bodies. */
  language: string;
}

/**
 * Turn a picked file into an inline body: UTF-8 text stays text (pretty JSON stays as typed),
 * anything else becomes base64. The content type is guessed from the extension.
 */
export function bodyFromFile(name: string, bytes: Uint8Array): LoadedBody {
  const contentType = contentTypeForFileName(name);
  const language = languageForContentType(contentType);
  const text = utf8DecodeStrict(bytes);
  const binaryType = contentType != null && /^(image\/(?!svg)|video\/|audio\/|font\/|application\/(pdf|wasm|zip|octet-stream))/.test(contentType);
  if (text != null && !binaryType && !text.includes('\0')) return { body: { type: 'text', text }, contentType, language };
  return { body: { type: 'base64', data: bytesToBase64(bytes) }, contentType, language: 'text' };
}
