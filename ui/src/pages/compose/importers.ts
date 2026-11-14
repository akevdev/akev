/**
 * Collection importers: HAR files (browser DevTools "Save all as HAR", Akev/HTTP Toolkit
 * exports), Postman collections (v2.0 / v2.1) and OpenAPI 3 / Swagger 2 JSON (see openapi.ts).
 * Pure — the page turns the result into saved folders and an environment.
 *
 * Postman uses the same `{{variable}}` syntax as Akev, so URLs, headers and bodies are kept
 * verbatim and the collection's variables become an environment.
 */

import type { EditableRequest } from '../../api/types';
import { makeRow, type KeyValueRow } from '../../components/ui/KeyValueEditor';
import { textToBase64 } from '../../lib/encoding';
import { suggestedRequestName } from './labels';
import { importOpenApi, isOpenApi } from './openapi';
import { emptyDraft, TEXT_TYPES, type MultipartRow, type RequestDraft } from './model';
import { draftFromEditable } from './request';
import { rowsFromUrl } from './urlParams';

export interface ImportedRequest {
  name: string;
  draft: RequestDraft;
}

export interface ImportedFolder {
  name: string;
  requests: ImportedRequest[];
}

export interface ImportedCollection {
  format: CollectionFormat;
  /** Collection name (Postman `info.name`, or the file name for HAR). */
  name: string;
  /** Requests at the top level of the collection. */
  requests: ImportedRequest[];
  /** Folders (nested Postman folders are flattened to `Parent / Child`). */
  folders: ImportedFolder[];
  /** Collection variables (Postman). */
  variables: KeyValueRow[];
  /** Items that could not be imported (with a reason). */
  skipped: string[];
}

export type CollectionFormat = 'har' | 'postman' | 'openapi';

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}
const str = (v: unknown): string => (typeof v === 'string' ? v : typeof v === 'number' || typeof v === 'boolean' ? String(v) : '');

/** Recognise a parsed JSON document. */
export function detectCollection(json: unknown): CollectionFormat | null {
  if (!isRecord(json)) return null;
  if (isRecord(json.log) && Array.isArray(json.log.entries)) return 'har';
  if (isOpenApi(json)) return 'openapi';
  const info = json.info;
  if (isRecord(info) && Array.isArray(json.item) && (typeof info.schema === 'string' ? info.schema.includes('postman') : typeof info._postman_id === 'string' || typeof info.name === 'string')) return 'postman';
  return null;
}

/** Parse text that may be a HAR / Postman document. Null when it isn't JSON of either kind. */
export function parseCollectionText(text: string): { format: CollectionFormat; json: unknown } | null {
  const t = text.trim();
  if (!t.startsWith('{')) return null;
  try {
    const json: unknown = JSON.parse(t);
    const format = detectCollection(json);
    return format ? { format, json } : null;
  } catch {
    return null;
  }
}

export function importCollection(json: unknown, fallbackName = 'Imported'): ImportedCollection {
  const format = detectCollection(json);
  if (format === 'har') return importHar(json, fallbackName);
  if (format === 'postman') return importPostman(json);
  if (format === 'openapi') return importOpenApi(json);
  throw new Error('Not a HAR file, Postman collection or OpenAPI document');
}

/** Number of requests in an import. */
export function collectionSize(c: ImportedCollection): number {
  return c.requests.length + c.folders.reduce((n, f) => n + f.requests.length, 0);
}

// =====================================================================================
// HAR
// =====================================================================================

/** HAR (1.2) entries → requests, in file order. Response data is ignored. */
export function importHar(json: unknown, name = 'HAR import'): ImportedCollection {
  const out: ImportedCollection = { format: 'har', name: name.replace(/\.(har|json)$/i, '') || 'HAR import', requests: [], folders: [], variables: [], skipped: [] };
  const entries = isRecord(json) && isRecord(json.log) && Array.isArray(json.log.entries) ? json.log.entries : [];
  entries.forEach((entry, i) => {
    const req = isRecord(entry) && isRecord(entry.request) ? entry.request : null;
    const url = req ? str(req.url) : '';
    if (!req || !url) {
      out.skipped.push(`Entry ${i + 1}: no request URL`);
      return;
    }
    const method = (str(req.method) || 'GET').toUpperCase();
    // Skip CORS preflights and the CONNECT tunnels some tools record.
    if (method === 'CONNECT') {
      out.skipped.push(`Entry ${i + 1}: CONNECT tunnel`);
      return;
    }
    const headers: [string, string][] = Array.isArray(req.headers)
      ? req.headers.filter(isRecord).map((h) => [str(h.name), str(h.value)] as [string, string]).filter(([n]) => n && !n.startsWith(':'))
      : [];
    let body = '';
    const post = isRecord(req.postData) ? req.postData : null;
    if (post) {
      if (typeof post.text === 'string') body = post.encoding === 'base64' ? post.text : textToBase64(post.text);
      else if (Array.isArray(post.params)) {
        const text = post.params
          .filter(isRecord)
          .map((p) => `${encodeURIComponent(str(p.name))}=${encodeURIComponent(str(p.value))}`)
          .join('&');
        body = textToBase64(text);
        const mime = str(post.mimeType);
        if (mime && !headers.some(([n]) => n.toLowerCase() === 'content-type')) headers.push(['Content-Type', mime]);
      }
    }
    const editable: EditableRequest = { method, url, headers, body };
    out.requests.push({ name: suggestedRequestName(url), draft: draftFromEditable(editable) });
  });
  return out;
}

// =====================================================================================
// Postman
// =====================================================================================

interface PostmanKv {
  key: string;
  value: string;
  disabled: boolean;
  type: string;
  src: string;
}

function kvList(v: unknown): PostmanKv[] {
  if (!Array.isArray(v)) return [];
  return v.filter(isRecord).map((x) => ({ key: str(x.key ?? x.name), value: str(x.value), disabled: x.disabled === true, type: str(x.type), src: Array.isArray(x.src) ? str(x.src[0]) : str(x.src) }));
}

function postmanUrl(u: unknown): string {
  if (typeof u === 'string') return u;
  if (!isRecord(u)) return '';
  if (typeof u.raw === 'string' && u.raw) return u.raw;
  const protocol = str(u.protocol);
  const host = Array.isArray(u.host) ? u.host.map(str).join('.') : str(u.host);
  const port = str(u.port);
  const path = Array.isArray(u.path) ? u.path.map((p) => (isRecord(p) ? str(p.value) : str(p))).join('/') : str(u.path);
  const query = kvList(u.query)
    .filter((q) => !q.disabled)
    .map((q) => (q.value ? `${q.key}=${q.value}` : q.key))
    .join('&');
  return `${protocol ? `${protocol}://` : ''}${host}${port ? `:${port}` : ''}${path ? `/${path.replace(/^\//, '')}` : ''}${query ? `?${query}` : ''}`;
}

function authValue(list: PostmanKv[], key: string): string {
  return list.find((x) => x.key === key)?.value ?? '';
}

const LANGUAGE_TYPES: Readonly<Record<string, string>> = {
  text: 'text/plain',
  javascript: 'application/javascript',
  html: 'text/html',
  xml: 'application/xml',
};

/** One Postman request object → draft. */
export function postmanRequestToDraft(request: unknown): RequestDraft {
  const draft = emptyDraft();
  const r = typeof request === 'string' ? { url: request, method: 'GET' } : isRecord(request) ? request : {};
  draft.method = (str(r.method) || 'GET').toUpperCase();
  draft.url = postmanUrl(r.url);
  draft.params = rowsFromUrl(draft.url);
  // Disabled query params stay in the table (unchecked), like Postman.
  if (isRecord(r.url)) for (const q of kvList(r.url.query)) if (q.disabled) draft.params.push({ ...makeRow(q.key, q.value, false) });

  const headers = kvList(r.header);
  draft.headers = headers.filter((h) => h.key).map((h) => makeRow(h.key, h.value, !h.disabled));

  const body = isRecord(r.body) ? r.body : null;
  const ctHeader = headers.find((h) => !h.disabled && h.key.toLowerCase() === 'content-type')?.value.split(';')[0]?.trim().toLowerCase() ?? '';
  if (body && body.disabled !== true) {
    switch (body.mode) {
      case 'raw': {
        const raw = str(body.raw);
        const options = isRecord(body.options) && isRecord(body.options.raw) ? body.options.raw : {};
        const language = str(options.language);
        if (language === 'json' || ctHeader === 'application/json' || ctHeader.endsWith('+json')) {
          draft.body.mode = 'json';
          draft.body.json = raw;
        } else if (raw) {
          draft.body.mode = 'text';
          draft.body.text = raw;
          draft.body.textType = TEXT_TYPES.some((t) => t.value === ctHeader) ? ctHeader : (LANGUAGE_TYPES[language] ?? 'text/plain');
        }
        break;
      }
      case 'urlencoded':
        draft.body.mode = 'form';
        draft.body.form = kvList(body.urlencoded).map((f) => makeRow(f.key, f.value, !f.disabled));
        break;
      case 'formdata':
        draft.body.mode = 'multipart';
        draft.body.multipart = kvList(body.formdata).map(
          (f): MultipartRow =>
            f.type === 'file'
              ? { ...makeRow(f.key, '', !f.disabled), kind: 'file', file: f.src ? { name: f.src.split(/[\\/]/).pop() || 'file', type: 'application/octet-stream', size: 0, data: '' } : null }
              : { ...makeRow(f.key, f.value, !f.disabled), kind: 'text', file: null },
        );
        break;
      case 'graphql': {
        const g = isRecord(body.graphql) ? body.graphql : {};
        let variables: unknown = undefined;
        try {
          variables = typeof g.variables === 'string' && g.variables.trim() ? JSON.parse(g.variables) : undefined;
        } catch {
          variables = undefined;
        }
        draft.body.mode = 'json';
        draft.body.json = JSON.stringify({ query: str(g.query), ...(variables !== undefined ? { variables } : {}) }, null, 2);
        break;
      }
      case 'file':
        draft.body.mode = 'binary';
        draft.body.binary = null;
        break;
    }
  }
  // A Content-Type the body mode already implies is redundant.
  if (draft.body.mode === 'json' && ctHeader === 'application/json') draft.headers = draft.headers.filter((h) => h.name.toLowerCase() !== 'content-type');
  if (draft.body.mode === 'form' && ctHeader === 'application/x-www-form-urlencoded') draft.headers = draft.headers.filter((h) => h.name.toLowerCase() !== 'content-type');
  if (draft.body.mode === 'multipart' && ctHeader === 'multipart/form-data') draft.headers = draft.headers.filter((h) => h.name.toLowerCase() !== 'content-type');

  const auth = isRecord(r.auth) ? r.auth : null;
  if (auth?.type === 'bearer') {
    draft.auth = { type: 'bearer', username: '', password: '', token: authValue(kvList(auth.bearer), 'token') };
  } else if (auth?.type === 'basic') {
    const b = kvList(auth.basic);
    draft.auth = { type: 'basic', username: authValue(b, 'username'), password: authValue(b, 'password'), token: '' };
  }
  return draft;
}

/** Postman collection (v2.0/v2.1) → folders + requests + variables. */
export function importPostman(json: unknown): ImportedCollection {
  const root = isRecord(json) ? json : {};
  const info = isRecord(root.info) ? root.info : {};
  const out: ImportedCollection = { format: 'postman', name: str(info.name) || 'Postman collection', requests: [], folders: [], variables: [], skipped: [] };
  out.variables = kvList(root.variable)
    .filter((v) => v.key)
    .map((v) => makeRow(v.key, v.value, !v.disabled));

  // Collection-level auth applies to requests that inherit it.
  const rootAuth = isRecord(root.auth) ? root.auth : null;

  const walk = (items: unknown, path: string[], inheritedAuth: Record<string, unknown> | null) => {
    if (!Array.isArray(items)) return;
    for (const it of items) {
      if (!isRecord(it)) continue;
      const name = str(it.name) || 'Untitled';
      if (Array.isArray(it.item)) {
        const folderAuth = isRecord(it.auth) ? it.auth : inheritedAuth;
        walk(it.item, [...path, name], folderAuth);
        continue;
      }
      if (it.request === undefined) {
        out.skipped.push(`${[...path, name].join(' / ')}: no request`);
        continue;
      }
      const reqObj = isRecord(it.request) ? it.request : { url: it.request };
      const withAuth = reqObj.auth === undefined && inheritedAuth ? { ...reqObj, auth: inheritedAuth } : reqObj;
      const draft = postmanRequestToDraft(withAuth);
      const request: ImportedRequest = { name, draft };
      if (path.length === 0) out.requests.push(request);
      else {
        const folderName = path.join(' / ');
        let folder = out.folders.find((f) => f.name === folderName);
        if (!folder) {
          folder = { name: folderName, requests: [] };
          out.folders.push(folder);
        }
        folder.requests.push(request);
      }
    }
  };
  walk(root.item, [], rootAuth);
  return out;
}
