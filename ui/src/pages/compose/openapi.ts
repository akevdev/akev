/**
 * OpenAPI 3.x / Swagger 2.0 (JSON) → requests: one per operation, grouped by first tag,
 * `{{baseUrl}}` from the first server, path parameters as `{{variables}}`, example query
 * parameters (optional ones unchecked), example bodies (JSON / form / multipart) and
 * security schemes mapped to Auth or an API-key header.
 */

import { makeRow, type KeyValueRow } from '../../components/ui/KeyValueEditor';
import { emptyDraft, type MultipartRow, type RequestDraft } from './model';
import { rowsFromUrl, urlWithParams } from './urlParams';
import type { ImportedCollection, ImportedFolder, ImportedRequest } from './importers';

type Json = Record<string, unknown>;

function isRecord(v: unknown): v is Json {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}
const str = (v: unknown): string => (typeof v === 'string' ? v : '');

const METHODS = ['get', 'post', 'put', 'patch', 'delete', 'head', 'options', 'trace'] as const;

/** OpenAPI 3.x or Swagger 2.0 with `paths`. */
export function isOpenApi(json: unknown): boolean {
  if (!isRecord(json) || !isRecord(json.paths)) return false;
  return (typeof json.openapi === 'string' && json.openapi.startsWith('3')) || json.swagger === '2.0';
}

/** Resolve local `$ref`s (`#/components/...`, `#/definitions/...`). */
function makeResolver(root: Json): (v: unknown) => unknown {
  return (v) => {
    let cur = v;
    for (let guard = 0; guard < 16 && isRecord(cur) && typeof cur.$ref === 'string'; guard++) {
      const ref = cur.$ref;
      if (!ref.startsWith('#/')) return {};
      let node: unknown = root;
      for (const part of ref.slice(2).split('/')) {
        const key = part.replace(/~1/g, '/').replace(/~0/g, '~');
        node = isRecord(node) ? node[key] : undefined;
      }
      cur = node ?? {};
    }
    return cur;
  };
}

/** A plausible sample value for a JSON schema (examples first, depth-limited, cycle-safe). */
export function sampleFromSchema(schema: unknown, resolve: (v: unknown) => unknown, depth = 0, seen: Set<unknown> = new Set()): unknown {
  const s = resolve(schema);
  if (!isRecord(s)) return null;
  if (s.example !== undefined) return s.example;
  if (Array.isArray(s.examples) && s.examples.length) return s.examples[0];
  if (s.default !== undefined) return s.default;
  if (Array.isArray(s.enum) && s.enum.length) return s.enum[0];
  if (seen.has(s) || depth > 5) return null;
  const next = new Set(seen).add(s);
  for (const key of ['allOf', 'oneOf', 'anyOf'] as const) {
    const list = s[key];
    if (Array.isArray(list) && list.length) {
      if (key === 'allOf') {
        const merged: Json = {};
        for (const part of list) {
          const v = sampleFromSchema(part, resolve, depth + 1, next);
          if (isRecord(v)) Object.assign(merged, v);
        }
        return merged;
      }
      return sampleFromSchema(list[0], resolve, depth + 1, next);
    }
  }
  const type = Array.isArray(s.type) ? str(s.type.find((t) => t !== 'null')) : str(s.type) || (isRecord(s.properties) ? 'object' : s.items ? 'array' : '');
  switch (type) {
    case 'object': {
      const out: Json = {};
      if (isRecord(s.properties)) for (const [k, v] of Object.entries(s.properties)) out[k] = sampleFromSchema(v, resolve, depth + 1, next);
      return out;
    }
    case 'array':
      return [sampleFromSchema(s.items, resolve, depth + 1, next)].filter((x) => x !== null);
    case 'integer':
    case 'number':
      return typeof s.minimum === 'number' ? s.minimum : 0;
    case 'boolean':
      return true;
    case 'string':
      switch (s.format) {
        case 'date-time':
          return '2026-01-01T00:00:00Z';
        case 'date':
          return '2026-01-01';
        case 'email':
          return 'user@example.com';
        case 'uuid':
          return '00000000-0000-4000-8000-000000000000';
        case 'uri':
        case 'url':
          return 'https://example.com';
        default:
          return 'string';
      }
    default:
      return null;
  }
}

function scalar(v: unknown): string {
  if (v === null || v === undefined) return '';
  return typeof v === 'object' ? JSON.stringify(v) : String(v);
}

interface Param {
  name: string;
  in: string;
  required: boolean;
  example: string;
}

function collectParams(lists: unknown[], resolve: (v: unknown) => unknown): Param[] {
  const byKey = new Map<string, Param>();
  for (const list of lists) {
    if (!Array.isArray(list)) continue;
    for (const raw of list) {
      const p = resolve(raw);
      if (!isRecord(p) || !p.name) continue;
      const example = p.example !== undefined ? p.example : isRecord(p.examples) ? (Object.values(p.examples).map(resolve).find(isRecord)?.value ?? undefined) : sampleFromSchema(p.schema ?? { type: p.type, enum: p.enum, default: p.default }, resolve);
      // Operation-level parameters override path-level ones with the same name + location.
      byKey.set(`${str(p.in)}:${str(p.name)}`, { name: str(p.name), in: str(p.in), required: p.required === true, example: scalar(example) });
    }
  }
  return [...byKey.values()];
}

function serverUrl(root: Json): string {
  if (Array.isArray(root.servers) && isRecord(root.servers[0])) {
    const s = root.servers[0];
    let url = str(s.url);
    if (isRecord(s.variables)) for (const [k, v] of Object.entries(s.variables)) url = url.split(`{${k}}`).join(isRecord(v) ? scalar(v.default) : '');
    return url.replace(/\/+$/, '');
  }
  if (typeof root.host === 'string') {
    const scheme = Array.isArray(root.schemes) && typeof root.schemes[0] === 'string' ? root.schemes[0] : 'https';
    return `${scheme}://${root.host}${str(root.basePath)}`.replace(/\/+$/, '');
  }
  return '';
}

type Security = { kind: 'bearer' } | { kind: 'basic' } | { kind: 'header'; name: string } | { kind: 'query'; name: string };

function securitySchemes(root: Json): Map<string, Security> {
  const out = new Map<string, Security>();
  const schemes = isRecord(root.components) && isRecord(root.components.securitySchemes) ? root.components.securitySchemes : isRecord(root.securityDefinitions) ? root.securityDefinitions : {};
  for (const [name, raw] of Object.entries(schemes)) {
    if (!isRecord(raw)) continue;
    const type = str(raw.type);
    const scheme = str(raw.scheme).toLowerCase();
    if ((type === 'http' && scheme === 'bearer') || type === 'oauth2' || type === 'openIdConnect') out.set(name, { kind: 'bearer' });
    else if ((type === 'http' && scheme === 'basic') || type === 'basic') out.set(name, { kind: 'basic' });
    else if (type === 'apiKey' && raw.in === 'header') out.set(name, { kind: 'header', name: str(raw.name) || 'X-API-Key' });
    else if (type === 'apiKey' && raw.in === 'query') out.set(name, { kind: 'query', name: str(raw.name) || 'api_key' });
  }
  return out;
}

function firstSecurity(list: unknown, schemes: Map<string, Security>): Security | null {
  if (!Array.isArray(list)) return null;
  for (const req of list) {
    if (!isRecord(req)) continue;
    for (const name of Object.keys(req)) {
      const s = schemes.get(name);
      if (s) return s;
    }
  }
  return null;
}

/** Path template `/users/{id}` → `/users/{{id}}`. */
export function templatePath(path: string): string {
  return path.replace(/\{([^{}]+)\}/g, '{{$1}}');
}

function applyBody(draft: RequestDraft, op: Json, swaggerParams: Param[], rawParams: unknown[], resolve: (v: unknown) => unknown): void {
  // OpenAPI 3: requestBody.content
  const body = resolve(op.requestBody);
  if (isRecord(body) && isRecord(body.content)) {
    const content = body.content;
    const types = Object.keys(content);
    const pick = types.find((t) => t.includes('json')) ?? types.find((t) => t === 'application/x-www-form-urlencoded') ?? types.find((t) => t === 'multipart/form-data') ?? types[0];
    if (!pick) return;
    const media = resolve(content[pick]);
    if (!isRecord(media)) return;
    const example = media.example !== undefined ? media.example : isRecord(media.examples) ? Object.values(media.examples).map(resolve).find(isRecord)?.value : sampleFromSchema(media.schema, resolve);
    if (pick.includes('json')) {
      draft.body.mode = 'json';
      draft.body.json = JSON.stringify(example ?? {}, null, 2);
      if (pick !== 'application/json') draft.headers.push(makeRow('Content-Type', pick));
    } else if (pick === 'application/x-www-form-urlencoded' || pick === 'multipart/form-data') {
      const schema = resolve(media.schema);
      const props = isRecord(schema) && isRecord(schema.properties) ? schema.properties : {};
      const values = isRecord(example) ? example : {};
      const required = new Set(isRecord(schema) && Array.isArray(schema.required) ? schema.required.map(str) : []);
      if (pick === 'multipart/form-data') {
        draft.body.mode = 'multipart';
        draft.body.multipart = Object.entries(props).map(([k, v]): MultipartRow => {
          const p = resolve(v);
          const binary = isRecord(p) && (p.format === 'binary' || p.format === 'base64');
          return binary ? { ...makeRow(k, '', true), kind: 'file', file: null } : { ...makeRow(k, scalar(values[k] ?? sampleFromSchema(v, resolve)), true), kind: 'text', file: null };
        });
      } else {
        draft.body.mode = 'form';
        draft.body.form = Object.entries(props).map(([k, v]) => makeRow(k, scalar(values[k] ?? sampleFromSchema(v, resolve)), required.size === 0 || required.has(k)));
      }
    } else if (typeof example === 'string') {
      draft.body.mode = 'text';
      draft.body.text = example;
    }
    return;
  }
  // Swagger 2: `in: body` / `in: formData` parameters.
  const bodyParam = rawParams.map(resolve).find((p) => isRecord(p) && p.in === 'body');
  if (isRecord(bodyParam)) {
    draft.body.mode = 'json';
    draft.body.json = JSON.stringify(sampleFromSchema(bodyParam.schema, resolve) ?? {}, null, 2);
    return;
  }
  const formParams = swaggerParams.filter((p) => p.in === 'formData');
  if (formParams.length) {
    const consumes = Array.isArray(op.consumes) ? op.consumes.map(str) : [];
    const rawForm = rawParams.map(resolve).filter((p): p is Json => isRecord(p) && p.in === 'formData');
    if (consumes.includes('multipart/form-data') || rawForm.some((p) => p.type === 'file')) {
      draft.body.mode = 'multipart';
      draft.body.multipart = formParams.map((p): MultipartRow => {
        const isFile = rawForm.find((r) => r.name === p.name)?.type === 'file';
        return isFile ? { ...makeRow(p.name, '', true), kind: 'file', file: null } : { ...makeRow(p.name, p.example, true), kind: 'text', file: null };
      });
    } else {
      draft.body.mode = 'form';
      draft.body.form = formParams.map((p) => makeRow(p.name, p.example, p.required));
    }
  }
}

/** Import an OpenAPI 3.x / Swagger 2.0 JSON document. */
export function importOpenApi(json: unknown): ImportedCollection {
  const root = isRecord(json) ? json : {};
  const info = isRecord(root.info) ? root.info : {};
  const resolve = makeResolver(root);
  const out: ImportedCollection = { format: 'openapi', name: str(info.title) || 'OpenAPI', requests: [], folders: [], variables: [], skipped: [] };
  const base = serverUrl(root);
  const vars = new Map<string, string>();
  vars.set('baseUrl', base || 'http://localhost');
  const schemes = securitySchemes(root);
  const globalSecurity = firstSecurity(root.security, schemes);
  const paths = isRecord(root.paths) ? root.paths : {};
  const folderByName = new Map<string, ImportedFolder>();

  for (const [path, rawItem] of Object.entries(paths)) {
    const item = resolve(rawItem);
    if (!isRecord(item)) continue;
    for (const method of METHODS) {
      const op = item[method];
      if (!isRecord(op)) continue;
      const rawParams = [...(Array.isArray(item.parameters) ? item.parameters : []), ...(Array.isArray(op.parameters) ? op.parameters : [])];
      const params = collectParams([item.parameters, op.parameters], resolve);
      const draft = emptyDraft();
      draft.method = method.toUpperCase();
      for (const p of params) if (p.in === 'path' && !vars.has(p.name)) vars.set(p.name, p.example === 'string' ? '' : p.example);
      const query = params.filter((p) => p.in === 'query').map((p) => ({ ...makeRow(p.name, p.example, p.required) }));
      draft.headers = params.filter((p) => p.in === 'header').map((p) => makeRow(p.name, p.example, p.required));
      const security = op.security !== undefined ? firstSecurity(op.security, schemes) : globalSecurity;
      if (security?.kind === 'bearer') {
        draft.auth = { type: 'bearer', username: '', password: '', token: '{{token}}' };
        if (!vars.has('token')) vars.set('token', '');
      } else if (security?.kind === 'basic') {
        draft.auth = { type: 'basic', username: '{{username}}', password: '{{password}}', token: '' };
        if (!vars.has('username')) vars.set('username', '');
        if (!vars.has('password')) vars.set('password', '');
      } else if (security?.kind === 'header') {
        draft.headers.push(makeRow(security.name, '{{apiKey}}'));
        if (!vars.has('apiKey')) vars.set('apiKey', '');
      } else if (security?.kind === 'query') {
        query.push(makeRow(security.name, '{{apiKey}}'));
        if (!vars.has('apiKey')) vars.set('apiKey', '');
      }
      // Enabled params form the URL; disabled ones stay in the table.
      const url = urlWithParams(`{{baseUrl}}${templatePath(path)}`, query);
      draft.url = url;
      draft.params = [...rowsFromUrl(url), ...query.filter((q) => !q.enabled)];
      applyBody(draft, op, params, rawParams, resolve);

      const name = str(op.summary).trim() || str(op.operationId) || `${draft.method} ${path}`;
      const request: ImportedRequest = { name: name.slice(0, 120), draft };
      const tag = Array.isArray(op.tags) && typeof op.tags[0] === 'string' ? op.tags[0] : '';
      if (!tag) out.requests.push(request);
      else {
        let folder = folderByName.get(tag);
        if (!folder) {
          folder = { name: tag, requests: [] };
          folderByName.set(tag, folder);
          out.folders.push(folder);
        }
        folder.requests.push(request);
      }
    }
  }
  out.variables = [...vars].map(([k, v]): KeyValueRow => makeRow(k, v));
  return out;
}
