/**
 * Compose data model: request drafts, tabs, history, saved requests and environments.
 *
 * Everything here is plain JSON (persisted through UI page prefs → localStorage + the daemon's
 * `settings.ui`), so every reader goes through a `sanitize*` function: stored data may be old,
 * new, truncated or hand-edited.
 */

import type { HttpVersionChoice } from '../../api/types';
import type { KeyValueRow } from '../../components/ui';
import { makeRow } from '../../components/ui/KeyValueEditor';
import { mergeParamsFromUrl } from './urlParams';

// =====================================================================================
// Request draft
// =====================================================================================

export type BodyMode = 'none' | 'json' | 'text' | 'form' | 'multipart' | 'binary';
export const BODY_MODES: readonly BodyMode[] = ['none', 'json', 'text', 'form', 'multipart', 'binary'];

export type AuthType = 'none' | 'basic' | 'bearer';
export const AUTH_TYPES: readonly AuthType[] = ['none', 'basic', 'bearer'];

/** Query-string row. `raw*` keep the exact encoded form typed in the URL (round-trip fidelity). */
export interface ParamRow extends KeyValueRow {
  /** Encoded name as it appears in the URL (absent for rows created in the table). */
  rawName?: string;
  /** Encoded value as it appears in the URL. */
  rawValue?: string;
  /** `?flag` (no `=`) vs `?flag=`. */
  noEquals?: boolean;
}

/** An attached file (base64 data). */
export interface FileRef {
  name: string;
  type: string;
  size: number;
  /** Base64. Empty when the file was too large to persist and must be picked again. */
  data: string;
}

export interface MultipartRow extends KeyValueRow {
  kind: 'text' | 'file';
  file?: FileRef | null;
}

export interface BodyState {
  mode: BodyMode;
  /** Contents are kept per mode so switching modes never loses work. */
  json: string;
  text: string;
  /** Content-Type for the Text mode. */
  textType: string;
  form: KeyValueRow[];
  multipart: MultipartRow[];
  binary: FileRef | null;
  /** Stable multipart boundary (so previews/snippets don't change on every render). */
  boundary: string;
}

export interface AuthState {
  type: AuthType;
  username: string;
  password: string;
  token: string;
}

/** Mirrors `SendOptions` with every field present. */
export interface RequestOptions {
  applyRules: boolean;
  followRedirects: boolean;
  insecure: boolean;
  /** `auto`: the daemon's settings decide (HTTP/2 via ALPN, HTTP/3 per Settings → Proxy). */
  httpVersion: HttpVersionChoice;
  /** 0 = daemon default. */
  timeoutMs: number;
}

export const HTTP_VERSION_CHOICES: readonly HttpVersionChoice[] = ['auto', 'http1', 'http2', 'http3'];

export function sanitizeHttpVersion(v: Record<string, unknown>): HttpVersionChoice {
  const raw = v.httpVersion;
  return typeof raw === 'string' && (HTTP_VERSION_CHOICES as readonly string[]).includes(raw) ? (raw as HttpVersionChoice) : 'auto';
}

export const DEFAULT_OPTIONS: Readonly<RequestOptions> = {
  applyRules: true,
  followRedirects: false,
  insecure: false,
  httpVersion: 'auto',
  timeoutMs: 0,
};

export interface RequestDraft {
  method: string;
  /** Exactly as typed (may contain `{{vars}}`); enabled params mirror its query string. */
  url: string;
  params: ParamRow[];
  headers: KeyValueRow[];
  /** Lower-case names of auto headers the user switched off (e.g. `content-type`). */
  disabledAuto: string[];
  body: BodyState;
  auth: AuthState;
  options: RequestOptions;
}

export const TEXT_TYPES: readonly { value: string; label: string; language: string }[] = [
  { value: 'text/plain', label: 'Plain text', language: 'text' },
  { value: 'application/xml', label: 'XML', language: 'xml' },
  { value: 'text/html', label: 'HTML', language: 'html' },
  { value: 'application/javascript', label: 'JavaScript', language: 'javascript' },
  { value: 'text/css', label: 'CSS', language: 'css' },
  { value: 'application/yaml', label: 'YAML', language: 'yaml' },
  { value: 'application/graphql', label: 'GraphQL', language: 'text' },
  { value: 'text/csv', label: 'CSV', language: 'text' },
];

let seq = 0;
/** Short unique id (tabs, history entries, saved nodes, environments). */
export function uid(prefix = 'c'): string {
  seq = (seq + 1) % 1_000_000;
  return `${prefix}${Date.now().toString(36)}${seq.toString(36)}${Math.floor(Math.random() * 1e6).toString(36)}`;
}

export function newBoundary(): string {
  let s = '';
  const chars = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789';
  for (let i = 0; i < 16; i++) s += chars[Math.floor(Math.random() * chars.length)];
  return `----AkevFormBoundary${s}`;
}

export function emptyBody(): BodyState {
  return { mode: 'none', json: '', text: '', textType: 'text/plain', form: [], multipart: [], binary: null, boundary: newBoundary() };
}

export function emptyDraft(): RequestDraft {
  return {
    method: 'GET',
    url: '',
    params: [],
    headers: [],
    disabledAuto: [],
    body: emptyBody(),
    auth: { type: 'none', username: '', password: '', token: '' },
    options: { ...DEFAULT_OPTIONS },
  };
}

// =====================================================================================
// Canonical form (dirty tracking)
// =====================================================================================

const rowKey = (r: KeyValueRow) => [r.enabled ? 1 : 0, r.name, r.value];
const fileKey = (f: FileRef | null | undefined) => (f ? [f.name, f.type, f.size, f.data.length, f.data.slice(0, 48), f.data.slice(-48)] : null);

/** Canonical serialisation of a draft, ignoring client-side row ids and the boundary. */
export function canonicalDraft(d: RequestDraft): string {
  return JSON.stringify([
    d.method.toUpperCase(),
    d.url,
    d.params.filter((p) => !p.enabled).map(rowKey),
    d.headers.filter((h) => h.name || h.value).map(rowKey),
    [...d.disabledAuto].sort(),
    d.body.mode,
    d.body.json,
    d.body.text,
    d.body.textType,
    d.body.form.filter((r) => r.name || r.value).map(rowKey),
    d.body.multipart.filter((r) => r.name || r.value || r.file).map((r) => [...rowKey(r), r.kind, fileKey(r.file)]),
    fileKey(d.body.binary),
    d.auth.type,
    d.auth.type === 'basic' ? [d.auth.username, d.auth.password] : d.auth.type === 'bearer' ? d.auth.token : null,
    d.options,
  ]);
}

/** FNV-1a 32-bit hash, base36. */
export function hashString(s: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(36);
}

const draftHashes = new WeakMap<RequestDraft, string>();
/** Memoised content hash of a draft (drafts are immutable, so identity caching is safe). */
export function draftHash(d: RequestDraft): string {
  let h = draftHashes.get(d);
  if (h === undefined) {
    h = hashString(canonicalDraft(d));
    draftHashes.set(d, h);
  }
  return h;
}

// =====================================================================================
// Tabs
// =====================================================================================

export type RequestTabId = 'params' | 'headers' | 'body' | 'auth' | 'code';
export type ResponseTabId = 'headers' | 'body' | 'cookies' | 'raw' | 'timing' | 'connection';

export interface ComposeTab {
  id: string;
  /** Custom title (rename); otherwise derived from the saved name or the URL. */
  title?: string;
  draft: RequestDraft;
  /** Hash of the draft when it was last opened/saved — differs → dirty dot. */
  baseline: string;
  /** Linked saved request. */
  savedId?: string;
  /** Captured exchange this request came from (Traffic → Edit in Compose). */
  sourceExchangeId?: number;
  /** Exchange recorded by the last send (restores the response after a reload). */
  lastExchangeId?: number;
  requestTab: RequestTabId;
  responseTab: ResponseTabId;
}

export function newTab(draft: RequestDraft = emptyDraft(), extra: Partial<ComposeTab> = {}): ComposeTab {
  return { id: uid('t'), draft, baseline: draftHash(draft), requestTab: draft.body.mode !== 'none' ? 'body' : 'params', responseTab: 'body', ...extra };
}

export function isDirty(tab: ComposeTab): boolean {
  return draftHash(tab.draft) !== tab.baseline;
}

/** A tab nobody would miss: blank and untouched. */
export function isPristineBlank(tab: ComposeTab): boolean {
  return !tab.savedId && tab.sourceExchangeId == null && tab.lastExchangeId == null && !tab.title && draftHash(tab.draft) === draftHash(emptyDraft()) && !isDirty(tab);
}

// =====================================================================================
// History & saved requests
// =====================================================================================

export interface HistoryEntry {
  id: string;
  /** Unix ms. */
  at: number;
  draft: RequestDraft;
  /** Resolved method/host/path at send time (for the list). */
  method: string;
  host: string;
  path: string;
  status?: number;
  /** Error kind or client error label (`Cancelled`, `Network`). */
  error?: string;
  durationMs?: number;
  exchangeId?: number;
  /** Body content was dropped because it was too large to keep. */
  bodyOmitted?: boolean;
}

export const HISTORY_LIMIT = 500;

export interface SavedRequest {
  type: 'request';
  id: string;
  name: string;
  draft: RequestDraft;
  updatedAt: number;
}

export interface SavedFolder {
  type: 'folder';
  id: string;
  name: string;
  collapsed: boolean;
  children: SavedRequest[];
}

export type SavedNode = SavedRequest | SavedFolder;

// =====================================================================================
// Environments
// =====================================================================================

export interface Environment {
  id: string;
  name: string;
  vars: KeyValueRow[];
}

export interface EnvState {
  /** Always applied, overridden by the active environment. */
  globals: KeyValueRow[];
  envs: Environment[];
  activeId: string | null;
}

export function emptyEnvState(): EnvState {
  return { globals: [], envs: [], activeId: null };
}

// =====================================================================================
// Sanitising
// =====================================================================================

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}
const str = (v: unknown, d = ''): string => (typeof v === 'string' ? v : d);
const bool = (v: unknown, d: boolean): boolean => (typeof v === 'boolean' ? v : d);
const num = (v: unknown, d: number): number => (typeof v === 'number' && Number.isFinite(v) ? v : d);

export function sanitizeRows(v: unknown): KeyValueRow[] {
  if (!Array.isArray(v)) return [];
  return v.filter(isRecord).map((r) => {
    const row = makeRow(str(r.name), str(r.value), bool(r.enabled, true));
    return row;
  });
}

function sanitizeParams(v: unknown): ParamRow[] {
  if (!Array.isArray(v)) return [];
  return v.filter(isRecord).map((r) => {
    const row: ParamRow = makeRow(str(r.name), str(r.value), bool(r.enabled, true));
    if (typeof r.rawName === 'string') row.rawName = r.rawName;
    if (typeof r.rawValue === 'string') row.rawValue = r.rawValue;
    if (r.noEquals === true) row.noEquals = true;
    return row;
  });
}

function sanitizeFile(v: unknown): FileRef | null {
  if (!isRecord(v)) return null;
  return { name: str(v.name, 'file'), type: str(v.type, 'application/octet-stream'), size: Math.max(0, num(v.size, 0)), data: str(v.data) };
}

function sanitizeBody(v: unknown): BodyState {
  const d = emptyBody();
  if (!isRecord(v)) return d;
  return {
    mode: BODY_MODES.includes(v.mode as BodyMode) ? (v.mode as BodyMode) : 'none',
    json: str(v.json),
    text: str(v.text),
    textType: str(v.textType, d.textType) || d.textType,
    form: sanitizeRows(v.form),
    multipart: Array.isArray(v.multipart)
      ? v.multipart.filter(isRecord).map((r) => ({ ...makeRow(str(r.name), str(r.value), bool(r.enabled, true)), kind: r.kind === 'file' ? 'file' : 'text', file: sanitizeFile(r.file) }))
      : [],
    binary: sanitizeFile(v.binary),
    boundary: str(v.boundary) || d.boundary,
  };
}

export function sanitizeOptions(v: unknown): RequestOptions {
  if (!isRecord(v)) return { ...DEFAULT_OPTIONS };
  return {
    applyRules: bool(v.applyRules, DEFAULT_OPTIONS.applyRules),
    followRedirects: bool(v.followRedirects, DEFAULT_OPTIONS.followRedirects),
    insecure: bool(v.insecure, DEFAULT_OPTIONS.insecure),
    httpVersion: sanitizeHttpVersion(v),
    timeoutMs: Math.max(0, Math.min(3_600_000, Math.round(num(v.timeoutMs, 0)))),
  };
}

export function sanitizeDraft(v: unknown): RequestDraft {
  const d = emptyDraft();
  if (!isRecord(v)) return d;
  const auth = isRecord(v.auth) ? v.auth : {};
  return {
    method: (str(v.method, 'GET').trim() || 'GET').toUpperCase().slice(0, 32),
    url: str(v.url),
    // The table must mirror the URL's query (stored params may be missing or stale).
    params: mergeParamsFromUrl(sanitizeParams(v.params), str(v.url)),
    headers: sanitizeRows(v.headers),
    disabledAuto: Array.isArray(v.disabledAuto) ? v.disabledAuto.filter((x): x is string => typeof x === 'string').map((x) => x.toLowerCase()) : [],
    body: sanitizeBody(v.body),
    auth: {
      type: AUTH_TYPES.includes(auth.type as AuthType) ? (auth.type as AuthType) : 'none',
      username: str(auth.username),
      password: str(auth.password),
      token: str(auth.token),
    },
    options: sanitizeOptions(v.options),
  };
}

const REQUEST_TABS: readonly RequestTabId[] = ['params', 'headers', 'body', 'auth', 'code'];
const RESPONSE_TABS: readonly ResponseTabId[] = ['headers', 'body', 'cookies', 'raw', 'timing', 'connection'];

export function sanitizeTab(v: unknown): ComposeTab | null {
  if (!isRecord(v) || typeof v.id !== 'string' || !v.id) return null;
  const draft = sanitizeDraft(v.draft);
  const tab: ComposeTab = {
    id: v.id,
    draft,
    baseline: str(v.baseline) || draftHash(draft),
    requestTab: REQUEST_TABS.includes(v.requestTab as RequestTabId) ? (v.requestTab as RequestTabId) : 'params',
    responseTab: RESPONSE_TABS.includes(v.responseTab as ResponseTabId) ? (v.responseTab as ResponseTabId) : 'body',
  };
  if (typeof v.title === 'string' && v.title.trim()) tab.title = v.title;
  if (typeof v.savedId === 'string') tab.savedId = v.savedId;
  if (typeof v.sourceExchangeId === 'number') tab.sourceExchangeId = v.sourceExchangeId;
  if (typeof v.lastExchangeId === 'number') tab.lastExchangeId = v.lastExchangeId;
  return tab;
}

export function sanitizeHistory(v: unknown): HistoryEntry[] {
  if (!Array.isArray(v)) return [];
  const out: HistoryEntry[] = [];
  for (const e of v) {
    if (!isRecord(e) || typeof e.id !== 'string') continue;
    const entry: HistoryEntry = {
      id: e.id,
      at: num(e.at, 0),
      draft: sanitizeDraft(e.draft),
      method: str(e.method, 'GET'),
      host: str(e.host),
      path: str(e.path, '/'),
    };
    if (typeof e.status === 'number') entry.status = e.status;
    if (typeof e.error === 'string') entry.error = e.error;
    if (typeof e.durationMs === 'number') entry.durationMs = e.durationMs;
    if (typeof e.exchangeId === 'number') entry.exchangeId = e.exchangeId;
    if (e.bodyOmitted === true) entry.bodyOmitted = true;
    out.push(entry);
    if (out.length >= HISTORY_LIMIT) break;
  }
  return out;
}

function sanitizeSavedRequest(v: unknown): SavedRequest | null {
  if (!isRecord(v) || typeof v.id !== 'string') return null;
  return { type: 'request', id: v.id, name: str(v.name, 'Untitled request') || 'Untitled request', draft: sanitizeDraft(v.draft), updatedAt: num(v.updatedAt, 0) };
}

export function sanitizeSaved(v: unknown): SavedNode[] {
  if (!Array.isArray(v)) return [];
  const out: SavedNode[] = [];
  const seen = new Set<string>();
  for (const n of v) {
    if (!isRecord(n) || typeof n.id !== 'string' || seen.has(n.id)) continue;
    if (n.type === 'folder') {
      seen.add(n.id);
      const children: SavedRequest[] = [];
      if (Array.isArray(n.children)) {
        for (const c of n.children) {
          const r = sanitizeSavedRequest(c);
          if (r && !seen.has(r.id)) {
            seen.add(r.id);
            children.push(r);
          }
        }
      }
      out.push({ type: 'folder', id: n.id, name: str(n.name, 'Folder') || 'Folder', collapsed: bool(n.collapsed, false), children });
    } else {
      const r = sanitizeSavedRequest(n);
      if (r) {
        seen.add(r.id);
        out.push(r);
      }
    }
  }
  return out;
}

export function sanitizeEnvState(v: unknown): EnvState {
  if (!isRecord(v)) return emptyEnvState();
  const envs: Environment[] = Array.isArray(v.envs)
    ? v.envs.filter(isRecord).filter((e) => typeof e.id === 'string').map((e) => ({ id: e.id as string, name: str(e.name, 'Environment') || 'Environment', vars: sanitizeRows(e.vars) }))
    : [];
  const activeId = typeof v.activeId === 'string' && envs.some((e) => e.id === v.activeId) ? v.activeId : null;
  return { globals: sanitizeRows(v.globals), envs, activeId };
}

// =====================================================================================
// Persistence shaping
// =====================================================================================

/**
 * Files larger than this are not persisted (they must be picked again after a reload): prefs
 * live in localStorage (a few MB) and are synced to the daemon on every change.
 */
export const PERSIST_FILE_LIMIT = 256 * 1024;

function persistFile(f: FileRef | null | undefined, limit: number): FileRef | null {
  if (!f) return null;
  return f.data.length > (limit * 4) / 3 ? { ...f, data: '' } : f;
}

const plainRow = (r: KeyValueRow) => ({ enabled: r.enabled, name: r.name, value: r.value });

/** JSON shape of a draft for storage: no row ids, big files dropped. */
export function persistDraft(d: RequestDraft, fileLimit = PERSIST_FILE_LIMIT): unknown {
  return {
    method: d.method,
    url: d.url,
    params: d.params.map((p) => ({ ...plainRow(p), rawName: p.rawName, rawValue: p.rawValue, noEquals: p.noEquals || undefined })),
    headers: d.headers.filter((h) => h.name || h.value).map(plainRow),
    disabledAuto: d.disabledAuto,
    body: {
      ...d.body,
      form: d.body.form.filter((r) => r.name || r.value).map(plainRow),
      multipart: d.body.multipart.filter((r) => r.name || r.value || r.file).map((r) => ({ ...plainRow(r), kind: r.kind, file: persistFile(r.file, fileLimit) })),
      binary: persistFile(d.body.binary, fileLimit),
    },
    auth: d.auth,
    options: d.options,
  };
}

/** Approximate size of a draft's body content (chars). */
export function draftBodyWeight(d: RequestDraft): number {
  const b = d.body;
  let n = b.json.length + b.text.length + (b.binary?.data.length ?? 0);
  for (const r of b.multipart) n += r.value.length + (r.file?.data.length ?? 0);
  for (const r of b.form) n += r.value.length;
  return n;
}

/** History keeps requests small: bodies over this many chars are dropped. */
export const HISTORY_BODY_LIMIT = 64 * 1024;

/** Total body characters persisted across the whole history (newest entries keep theirs). */
export const HISTORY_PERSIST_BUDGET = 512 * 1024;

/** Strip a draft's body content (files, JSON, text, multipart files). */
function withoutBody(d: RequestDraft): RequestDraft {
  return { ...d, body: { ...d.body, json: '', text: '', binary: d.body.binary ? { ...d.body.binary, data: '' } : null, multipart: d.body.multipart.map((r) => ({ ...r, file: r.file ? { ...r.file, data: '' } : r.file })) } };
}

/**
 * JSON shape of the history for storage: files dropped, and once the body budget is spent
 * (newest first) older entries keep only their request line, headers and params.
 */
export function persistHistory(history: readonly HistoryEntry[], budget = HISTORY_PERSIST_BUDGET): unknown[] {
  let left = budget;
  return history.map((h) => {
    const weight = draftBodyWeight(h.draft);
    if (weight <= left) {
      left -= weight;
      return { ...h, draft: persistDraft(h.draft, 0) };
    }
    return { ...h, bodyOmitted: true, draft: persistDraft(withoutBody(h.draft), 0) };
  });
}

/** Copy of a draft suitable for history (large bodies dropped). */
export function historyDraft(d: RequestDraft): { draft: RequestDraft; bodyOmitted: boolean } {
  if (draftBodyWeight(d) <= HISTORY_BODY_LIMIT) return { draft: d, bodyOmitted: false };
  return { draft: withoutBody(d), bodyOmitted: true };
}
