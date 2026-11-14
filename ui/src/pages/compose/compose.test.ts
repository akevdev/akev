import { describe, expect, test } from 'bun:test';
import { makeRow } from '../../components/ui/KeyValueEditor';
import { base64ToText, textToBase64 } from '../../lib/encoding';
import {
  addFolder,
  addHistory,
  addSaved,
  filterHistory,
  flattenSaved,
  groupHistory,
  historyTimeLabel,
  moveNode,
  moveToFolder,
  moveWithinParent,
  projectDrop,
  removeNode,
  renameNode,
  uniqueName,
} from './library';
import {
  canonicalDraft,
  draftHash,
  emptyDraft,
  historyDraft,
  HISTORY_BODY_LIMIT,
  isDirty,
  isPristineBlank,
  newTab,
  persistDraft,
  persistHistory,
  sanitizeDraft,
  sanitizeEnvState,
  sanitizeHistory,
  sanitizeSaved,
  sanitizeTab,
  type HistoryEntry,
  type ParamRow,
  type RequestDraft,
  type SavedFolder,
  type SavedNode,
  type SavedRequest,
} from './model';
import { detectPaste, fetchToEditable, looksLikeCurl, parseFetch, splitCurlCommands, splitFetchCalls } from './paste';
import {
  autoHeaders,
  bodyContentType,
  buildMultipart,
  buildRequest,
  checkJson,
  draftFromEditable,
  encodeFormComponent,
  formatJson,
  normalizeUrl,
  parseMultipart,
  serializeBody,
} from './request';
import {
  closeOthers,
  closeTab,
  closeToRight,
  duplicateTab,
  initialTabs,
  moveTab,
  openTab,
  reopenClosed,
  selectIndex,
  selectRelative,
  updateDraft,
} from './tabs';
import { buildQueryFromRows, encodeQueryComponent, mergeParamsFromUrl, paramsFromUrl, rowsFromUrl, urlWithParams } from './urlParams';
import { urlSpans } from './highlight';
import { applyVarCompletion, referencedVars, resolveEnvVars, substitute, tokenizeVars, unresolvedIn, varCompletionAt } from './variables';

const names = (rows: readonly ParamRow[]) => rows.map((r) => [r.enabled, r.name, r.value]);

// =====================================================================================
describe('URL ↔ params sync', () => {
  test('parses decoded params, keeping order, duplicates and raw encoding', () => {
    const p = paramsFromUrl('https://a.com/?tag=a&tag=b&q=hello%20world&plus=a+b&flag&empty=');
    expect(p.map((x) => [x.name, x.value])).toEqual([
      ['tag', 'a'],
      ['tag', 'b'],
      ['q', 'hello world'],
      ['plus', 'a b'],
      ['flag', ''],
      ['empty', ''],
    ]);
    expect(p[2]?.rawValue).toBe('hello%20world');
    expect(p[4]?.noEquals).toBe(true);
    expect(p[5]?.noEquals).toBe(false);
  });

  test('round trip: untouched params keep their exact encoding', () => {
    const url = 'https://a.com/search?q=hello+world&path=%2Fa%2Fb&x=%7B%7D&flag&n=1#top';
    const rows = rowsFromUrl(url);
    expect(urlWithParams(url, rows)).toBe(url);
  });

  test('editing a value re-encodes only that param', () => {
    const url = 'https://a.com/?a=x+y&b=1';
    const rows = rowsFromUrl(url);
    const edited = rows.map((r) => (r.name === 'b' ? { ...r, value: 'two words&more' } : r));
    expect(urlWithParams(url, edited)).toBe('https://a.com/?a=x+y&b=two%20words%26more');
  });

  test('new rows are minimally encoded and variables are never encoded', () => {
    expect(encodeQueryComponent('café & crème')).toBe('café%20%26%20crème');
    expect(encodeQueryComponent('{{token}} x')).toBe('{{token}}%20x');
    expect(encodeQueryComponent('a=b', true)).toBe('a%3Db');
    expect(encodeQueryComponent('50%')).toBe('50%25');
    const rows: ParamRow[] = [makeRow('q', '{{term}}'), makeRow('page', '2')];
    expect(urlWithParams('{{baseUrl}}/search', rows)).toBe('{{baseUrl}}/search?q={{term}}&page=2');
  });

  test('disabled and empty rows are not in the URL; no params drops the "?"', () => {
    const rows: ParamRow[] = [makeRow('a', '1', false), makeRow('', ''), makeRow('b', '2')];
    expect(buildQueryFromRows(rows)).toBe('b=2');
    expect(urlWithParams('https://a.com/p?old=1#h', [makeRow('a', '1', false)])).toBe('https://a.com/p#h');
  });

  test('merge keeps ids of enabled rows, keeps disabled rows in place and appends new params', () => {
    const rows: ParamRow[] = [makeRow('a', '1'), makeRow('off', 'x', false), makeRow('b', '2')];
    const ids = rows.map((r) => r.id);
    const merged = mergeParamsFromUrl(rows, 'https://a.com/?a=10&b=2&c=3');
    expect(names(merged)).toEqual([
      [true, 'a', '10'],
      [false, 'off', 'x'],
      [true, 'b', '2'],
      [true, 'c', '3'],
    ]);
    expect(merged.slice(0, 3).map((r) => r.id)).toEqual(ids);
  });

  test('merge drops surplus enabled rows and returns the same array when unchanged', () => {
    const rows = rowsFromUrl('https://a.com/?a=1&b=2');
    expect(mergeParamsFromUrl(rows, 'https://a.com/?a=1&b=2')).toBe(rows);
    expect(names(mergeParamsFromUrl(rows, 'https://a.com/?a=1'))).toEqual([[true, 'a', '1']]);
    expect(mergeParamsFromUrl(rows, 'https://a.com/')).toEqual([]);
  });
});

// =====================================================================================
describe('variables', () => {
  const env = sanitizeEnvState({
    globals: [{ name: 'host', value: 'global.example' }, { name: 'scheme', value: 'https' }],
    envs: [{ id: 'dev', name: 'Dev', vars: [{ name: 'host', value: 'localhost:3000' }, { name: 'baseUrl', value: '{{scheme}}://{{host}}' }, { name: 'off', value: 'x', enabled: false }] }],
    activeId: 'dev',
  });

  test('active environment overrides globals; disabled vars are ignored', () => {
    const vars = resolveEnvVars(env);
    expect(vars.get('host')).toBe('localhost:3000');
    expect(vars.has('off')).toBe(false);
    expect(resolveEnvVars({ ...env, activeId: null }).get('host')).toBe('global.example');
  });

  test('substitutes nested variables and reports unresolved ones', () => {
    const vars = resolveEnvVars(env);
    const r = substitute('{{baseUrl}}/users/{{ id }}?t={{token}}&again={{id}}', vars);
    expect(r.text).toBe('https://localhost:3000/users/{{ id }}?t={{token}}&again={{id}}');
    expect(r.unresolved).toEqual(['id', 'token']);
  });

  test('cycles stop expanding and are reported', () => {
    const vars = new Map([
      ['a', '{{b}}'],
      ['b', '{{a}}'],
    ]);
    const r = substitute('{{a}}', vars);
    expect(r.unresolved.length).toBeGreaterThan(0);
  });

  test('dynamic variables are generated per occurrence', () => {
    let n = 0;
    const r = substitute('{{$uuid}}-{{$uuid}}', new Map(), (name) => (name === '$uuid' ? `u${++n}` : undefined));
    expect(r.text).toBe('u1-u2');
    expect(substitute('{{$timestamp}}', new Map()).text).toMatch(/^\d{10}$/);
    expect(unresolvedIn(['{{$uuid}} {{missing}}'], new Map())).toEqual(['missing']);
  });

  test('tokenizes for highlighting with resolution status', () => {
    const vars = new Map([['base', 'https://a.com']]);
    const segs = tokenizeVars('{{base}}/x/{{nope}}?t={{$timestamp}}', vars);
    expect(segs.map((s) => s.variable?.status ?? s.text)).toEqual(['resolved', '/x/', 'unresolved', '?t=', 'dynamic']);
    expect(segs[0]?.variable?.value).toBe('https://a.com');
    expect(referencedVars('{{a}} {{b}} {{a}}')).toEqual(['a', 'b']);
  });
});

// =====================================================================================
describe('paste detection', () => {
  test('detects cURL commands (with prompts, .exe) but not URLs', () => {
    expect(looksLikeCurl("curl 'https://a.com'")).toBe(true);
    expect(looksLikeCurl('  $ curl -X POST https://a.com')).toBe(true);
    expect(looksLikeCurl('curl.exe https://a.com')).toBe(true);
    expect(looksLikeCurl('https://curl.se/docs')).toBe(false);
    expect(looksLikeCurl('curl')).toBe(false);
    expect(detectPaste('fetch("https://a.com")')).toBe('fetch');
    expect(detectPaste('await fetch("https://a.com", {})')).toBe('fetch');
    expect(detectPaste('https://a.com')).toBe(null);
  });

  test('splits "Copy all as cURL" output into commands, honouring continuations', () => {
    const text = `curl 'https://a.com/1' \\\n  -H 'accept: */*' ;\ncurl 'https://a.com/2' -X POST --data-raw 'x' ;\n\ncurl 'https://a.com/3'`;
    const cmds = splitCurlCommands(text);
    expect(cmds).toHaveLength(3);
    expect(cmds[0]).toContain("-H 'accept: */*'");
    expect(cmds[0]?.endsWith(';')).toBe(false);
    expect(cmds[1]).toContain('--data-raw');
  });

  test('a continuation line starting with "curl" is not a new command', () => {
    expect(splitCurlCommands("curl \\\n  'https://a.com' \\\n  -H 'x: y'")).toHaveLength(1);
  });

  test('parses Chrome "Copy as fetch" output', () => {
    const text = `fetch("https://api.example.com/v1/items?x=1", {
  "headers": {
    "accept": "application/json",
    "content-type": "application/json"
  },
  "referrer": "https://example.com/",
  "body": "{\\"name\\":\\"Akev\\"}",
  "method": "POST",
  "mode": "cors",
  "credentials": "include"
});`;
    const f = parseFetch(text);
    expect(f.method).toBe('POST');
    expect(f.url).toBe('https://api.example.com/v1/items?x=1');
    expect(f.headers).toEqual([
      ['accept', 'application/json'],
      ['content-type', 'application/json'],
    ]);
    expect(f.body).toBe('{"name":"Akev"}');
    expect(base64ToText(fetchToEditable(text).body)).toBe('{"name":"Akev"}');
  });

  test('parses loose JS object literals and defaults the method', () => {
    expect(parseFetch(`fetch('https://a.com/x', { headers: { 'X-Key': 'k', }, body: 'a=1', })`)).toEqual({ method: 'POST', url: 'https://a.com/x', headers: [['X-Key', 'k']], body: 'a=1' });
    expect(parseFetch('fetch(`https://a.com`)').method).toBe('GET');
    expect(() => parseFetch('fetch(url)')).toThrow();
    expect(splitFetchCalls('fetch("https://a.com/1");\nfetch("https://a.com/2", {"method":"DELETE"});')).toHaveLength(2);
  });
});

// =====================================================================================
describe('body mode → headers', () => {
  const draftWith = (patch: (d: RequestDraft) => void): RequestDraft => {
    const d = emptyDraft();
    d.url = 'https://api.example.com:8443/v1';
    patch(d);
    return d;
  };

  test('content type per mode', () => {
    const d = emptyDraft();
    expect(bodyContentType(d.body)).toBe(null);
    expect(bodyContentType({ ...d.body, mode: 'json' })).toBe('application/json');
    expect(bodyContentType({ ...d.body, mode: 'text', textType: 'application/xml' })).toBe('application/xml');
    expect(bodyContentType({ ...d.body, mode: 'form' })).toBe('application/x-www-form-urlencoded');
    expect(bodyContentType({ ...d.body, mode: 'multipart', boundary: 'XYZ' })).toBe('multipart/form-data; boundary=XYZ');
    expect(bodyContentType({ ...d.body, mode: 'binary' })).toBe(null);
    expect(bodyContentType({ ...d.body, mode: 'binary', binary: { name: 'a.png', type: 'image/png', size: 1, data: 'AA==' } })).toBe('image/png');
  });

  test('auto headers: host, auth, content type, length; user headers override; toggles', () => {
    const d = draftWith((x) => {
      x.body.mode = 'json';
      x.body.json = '{"a":1}';
      x.auth = { type: 'bearer', username: '', password: '', token: 'abc' };
    });
    const auto = autoHeaders(d, { bodySize: 7 });
    expect(auto.map((a) => [a.name, a.value, a.sent])).toEqual([
      ['Host', 'api.example.com:8443', false],
      ['Authorization', 'Bearer abc', true],
      ['Content-Type', 'application/json', true],
      ['Content-Length', '7', false],
    ]);
    const overridden = autoHeaders({ ...d, headers: [makeRow('content-type', 'application/vnd.x+json')] });
    expect(overridden.find((a) => a.key === 'content-type')?.overridden).toBe(true);
    const off = autoHeaders({ ...d, disabledAuto: ['content-type'] });
    expect(off.find((a) => a.key === 'content-type')?.disabled).toBe(true);
  });

  test('buildRequest: substitution, auto headers first, user headers win, disabled skipped', () => {
    const d = draftWith((x) => {
      x.method = 'post';
      x.url = '{{base}}/items?id={{id}}';
      x.headers = [makeRow('X-Trace', '{{trace}}'), makeRow('X-Off', '1', false), makeRow('Content-Type', 'application/merge-patch+json')];
      x.body.mode = 'json';
      x.body.json = '{"name":"{{name}}"}';
      x.auth = { type: 'basic', username: 'u', password: '{{pw}}', token: '' };
    });
    const vars = new Map([
      ['base', 'localhost:3000'],
      ['id', '7'],
      ['name', 'Akev'],
      ['pw', 'secret'],
    ]);
    const built = buildRequest(d, vars);
    expect(built.request.method).toBe('POST');
    expect(built.url).toBe('http://localhost:3000/items?id=7');
    expect(built.request.headers).toEqual([
      ['Authorization', `Basic ${textToBase64('u:secret')}`],
      ['X-Trace', '{{trace}}'],
      ['Content-Type', 'application/merge-patch+json'],
    ]);
    expect(built.unresolved).toEqual(['trace']);
    expect(base64ToText(built.request.body)).toBe('{"name":"Akev"}');
    expect(built.bodySize).toBe(15);
  });

  test('form and multipart serialisation', () => {
    const d = emptyDraft();
    d.body.mode = 'form';
    d.body.form = [makeRow('q', 'a b&c'), makeRow('off', 'x', false), makeRow('n', '{{n}}')];
    expect(base64ToText(serializeBody(d.body, (s) => s.replace('{{n}}', '5')).base64)).toBe('q=a+b%26c&n=5');
    expect(encodeFormComponent('ü x')).toBe('%C3%BC+x');

    const rows = [
      { ...makeRow('field', 'value'), kind: 'text' as const, file: null },
      { ...makeRow('upload', ''), kind: 'file' as const, file: { name: 'a.txt', type: 'text/plain', size: 2, data: textToBase64('hi') } },
    ];
    const bytes = buildMultipart(rows, 'B');
    const text = new TextDecoder().decode(bytes);
    expect(text).toBe('--B\r\nContent-Disposition: form-data; name="field"\r\n\r\nvalue\r\n--B\r\nContent-Disposition: form-data; name="upload"; filename="a.txt"\r\nContent-Type: text/plain\r\n\r\nhi\r\n--B--\r\n');
    const parsed = parseMultipart(bytes, 'B');
    expect(parsed?.map((r) => [r.kind, r.name, r.value, r.file?.name])).toEqual([
      ['text', 'field', 'value', undefined],
      ['file', 'upload', '', 'a.txt'],
    ]);
  });

  test('normalizeUrl adds http for local hosts and https otherwise', () => {
    expect(normalizeUrl('localhost:3000/x')).toBe('http://localhost:3000/x');
    expect(normalizeUrl('192.168.1.4:8080')).toBe('http://192.168.1.4:8080');
    expect(normalizeUrl('api.example.com/v1')).toBe('https://api.example.com/v1');
    expect(normalizeUrl('http://a.com')).toBe('http://a.com');
    expect(normalizeUrl('  ')).toBe('');
  });
});

// =====================================================================================
describe('import from EditableRequest', () => {
  test('JSON body, bearer auth, pseudo/framing headers dropped, params parsed', () => {
    const d = draftFromEditable({
      method: 'post',
      url: 'https://api.example.com/v1/items?x=1',
      headers: [
        [':authority', 'api.example.com'],
        ['host', 'api.example.com'],
        ['authorization', 'Bearer tok123'],
        ['content-type', 'application/json'],
        ['content-length', '9'],
        ['x-custom', 'yes'],
      ],
      body: textToBase64('{"a":1}'),
    });
    expect(d.method).toBe('POST');
    expect(d.params.map((p) => [p.name, p.value])).toEqual([['x', '1']]);
    expect(d.auth).toEqual({ type: 'bearer', username: '', password: '', token: 'tok123' });
    expect(d.body.mode).toBe('json');
    expect(d.body.json).toBe('{"a":1}');
    expect(d.headers.map((h) => [h.name, h.value])).toEqual([['x-custom', 'yes']]);
  });

  test('vendor JSON types keep their Content-Type header; forms become rows; basic auth decoded', () => {
    const d = draftFromEditable({ method: 'PATCH', url: 'https://a.com', headers: [['Content-Type', 'application/vnd.api+json'], ['Authorization', `Basic ${textToBase64('me:p:w')}`]], body: textToBase64('{}') });
    expect(d.body.mode).toBe('json');
    expect(d.headers.map((h) => h.name)).toEqual(['Content-Type']);
    expect(d.auth).toMatchObject({ type: 'basic', username: 'me', password: 'p:w' });
    const f = draftFromEditable({ method: 'POST', url: 'https://a.com', headers: [['content-type', 'application/x-www-form-urlencoded']], body: textToBase64('a=1&b=x+y') });
    expect(f.body.mode).toBe('form');
    expect(f.body.form.map((r) => [r.name, r.value])).toEqual([
      ['a', '1'],
      ['b', 'x y'],
    ]);
  });

  test('binary bodies become a file; Host that differs from the URL is kept', () => {
    const d = draftFromEditable({ method: 'PUT', url: 'https://10.0.0.1/upload/avatar.png', headers: [['Host', 'cdn.example.com'], ['Content-Type', 'image/png']], body: 'iVBORw0KGgoAAAA=' });
    expect(d.body.mode).toBe('binary');
    expect(d.body.binary?.name).toBe('avatar.png');
    expect(d.body.binary?.type).toBe('image/png');
    expect(d.headers.map((h) => h.name)).toEqual(['Host']);
  });

  test('a multipart body is parsed into rows with a fresh boundary', () => {
    const rows = [{ ...makeRow('f', 'v'), kind: 'text' as const, file: null }];
    const body = buildMultipart(rows, 'OLD');
    const d = draftFromEditable({ method: 'POST', url: 'https://a.com', headers: [['content-type', 'multipart/form-data; boundary=OLD']], body: Buffer.from(body).toString('base64') });
    expect(d.body.mode).toBe('multipart');
    expect(d.body.multipart.map((r) => [r.name, r.value])).toEqual([['f', 'v']]);
    expect(d.body.boundary).not.toBe('OLD');
    expect(d.headers).toEqual([]);
  });
});

// =====================================================================================
describe('JSON helpers', () => {
  test('checkJson allows bare variables and reports line/column', () => {
    expect(checkJson('{"id": {{id}}, "name": "{{name}}"}')).toEqual({ ok: true });
    const bad = checkJson('{\n  "a": 1,\n  "b": \n}');
    expect(bad.ok).toBe(false);
    if (!bad.ok) {
      expect(bad.line).toBe(4);
      expect(bad.message).not.toContain('position');
    }
    expect(checkJson('   ')).toEqual({ ok: true });
  });

  test('formatJson pretty prints and keeps variables', () => {
    expect(formatJson('{"id":{{id}},"a":[1,2]}')).toBe('{\n  "id": {{id}},\n  "a": [\n    1,\n    2\n  ]\n}');
    expect(formatJson('{oops')).toBe(null);
  });
});

// =====================================================================================
describe('tab reducers', () => {
  const urlTab = (url: string) => {
    const d = emptyDraft();
    d.url = url;
    return newTab(d);
  };

  test('initial state always has a tab', () => {
    const s = initialTabs([]);
    expect(s.tabs).toHaveLength(1);
    expect(s.activeId).toBe(s.tabs[0]?.id ?? '');
  });

  test('open replaces a pristine blank tab when asked', () => {
    let s = initialTabs([]);
    const a = urlTab('https://a.com');
    s = openTab(s, a, { replacePristine: true });
    expect(s.tabs).toHaveLength(1);
    expect(s.activeId).toBe(a.id);
    const b = urlTab('https://b.com');
    s = openTab(s, b, { replacePristine: true });
    expect(s.tabs.map((t) => t.id)).toEqual([a.id, b.id]);
  });

  test('close moves focus right (else left), keeps one tab, and can be reopened in place', () => {
    const [a, b, c] = [urlTab('https://a'), urlTab('https://b'), urlTab('https://c')];
    let s = initialTabs([a, b, c], b.id);
    s = closeTab(s, b.id);
    expect(s.activeId).toBe(c.id);
    s = closeTab(s, c.id);
    expect(s.activeId).toBe(a.id);
    s = reopenClosed(s);
    expect(s.tabs.map((t) => t.id)).toEqual([a.id, c.id]);
    s = reopenClosed(s);
    expect(s.tabs.map((t) => t.id)).toEqual([a.id, b.id, c.id]);
    expect(s.activeId).toBe(b.id);
    expect(reopenClosed(s)).toBe(s);
    s = closeTab(closeTab(closeTab(s, a.id), b.id), c.id);
    expect(s.tabs).toHaveLength(1);
    expect(isPristineBlank(s.tabs[0] as never)).toBe(true);
  });

  test('pristine blank tabs are not remembered as closed', () => {
    const s = initialTabs([newTab(), urlTab('https://a')]);
    const first = s.tabs[0]?.id ?? '';
    expect(closeTab(s, first).closed).toHaveLength(0);
  });

  test('close others / to the right, move, select relative & by index, duplicate', () => {
    const tabs = ['1', '2', '3', '4'].map((n) => urlTab(`https://${n}`));
    const [a, b, c, d] = tabs.map((t) => t.id) as [string, string, string, string];
    let s = initialTabs(tabs, b);
    expect(closeOthers(s, c).tabs.map((t) => t.id)).toEqual([c]);
    expect(closeToRight(s, b).tabs.map((t) => t.id)).toEqual([a, b]);
    expect(closeToRight(initialTabs(tabs, d), b).activeId).toBe(b);
    s = moveTab(s, 0, 3);
    expect(s.tabs.map((t) => t.id)).toEqual([b, c, d, a]);
    expect(selectRelative(s, -1).activeId).toBe(a);
    expect(selectRelative(s, 1).activeId).toBe(c);
    expect(selectIndex(s, 9).activeId).toBe(a);
    expect(selectIndex(s, 1).activeId).toBe(b);
    const dup = duplicateTab(s, b);
    expect(dup.tabs).toHaveLength(5);
    expect(dup.tabs[1]?.draft.url).toBe('https://2');
    expect(dup.activeId).toBe(dup.tabs[1]?.id ?? '');
  });

  test('dirty tracking follows draft content, not row ids', () => {
    const t = urlTab('https://a.com');
    expect(isDirty(t)).toBe(false);
    let s = initialTabs([t]);
    s = updateDraft(s, t.id, (d) => ({ ...d, url: 'https://b.com' }));
    expect(isDirty(s.tabs[0] as never)).toBe(true);
    s = updateDraft(s, t.id, (d) => ({ ...d, url: 'https://a.com' }));
    expect(isDirty(s.tabs[0] as never)).toBe(false);
    const d1 = { ...emptyDraft(), headers: [makeRow('a', '1')] };
    const d2 = { ...emptyDraft(), headers: [makeRow('a', '1')], body: { ...d1.body, boundary: 'other' } };
    expect(draftHash(d1)).toBe(draftHash(d2));
    expect(canonicalDraft(d1)).toBe(canonicalDraft(d2));
  });
});

// =====================================================================================
describe('persistence', () => {
  test('tabs survive a persist → sanitize round trip', () => {
    const d = emptyDraft();
    d.method = 'PUT';
    d.url = 'https://a.com/?x=%20';
    d.params = rowsFromUrl(d.url);
    d.headers = [makeRow('X-A', '1'), makeRow('X-B', '2', false)];
    d.body.mode = 'json';
    d.body.json = '{}';
    d.options.followRedirects = true;
    const tab = newTab(d, { title: 'Custom', savedId: 's1', lastExchangeId: 42, requestTab: 'headers' });
    const json = JSON.parse(JSON.stringify({ ...tab, draft: persistDraft(tab.draft) }));
    const back = sanitizeTab(json);
    expect(back).not.toBeNull();
    expect(back?.title).toBe('Custom');
    expect(back?.savedId).toBe('s1');
    expect(back?.lastExchangeId).toBe(42);
    expect(back?.requestTab).toBe('headers');
    expect(draftHash(back?.draft as RequestDraft)).toBe(draftHash(d));
    expect(back?.draft.params[0]?.rawValue).toBe('%20');
    expect(isDirty(back as never)).toBe(false);
  });

  test('sanitizers reject garbage and fill defaults', () => {
    expect(sanitizeTab({})).toBeNull();
    expect(sanitizeTab({ id: 'x', draft: 'nope', requestTab: 'bogus' })?.requestTab).toBe('params');
    const d = sanitizeDraft({ method: ' patch ', body: { mode: 'weird' }, options: { timeoutMs: -5, insecure: 'yes' } });
    expect(d.method).toBe('PATCH');
    expect(d.body.mode).toBe('none');
    expect(d.options.timeoutMs).toBe(0);
    expect(d.options.insecure).toBe(false);
    expect(sanitizeHistory([{ id: 'h', at: 1, method: 'GET' }, null, 4])).toHaveLength(1);
    expect(sanitizeSaved([{ type: 'folder', id: 'f', name: '', children: [{ id: 'r' }, { id: 'r' }] }, { id: 'r' }])).toEqual([
      { type: 'folder', id: 'f', name: 'Folder', collapsed: false, children: [expect.objectContaining({ id: 'r', name: 'Untitled request' })] },
    ]);
    expect(sanitizeEnvState({ envs: [{ id: 'e', name: 'E' }], activeId: 'missing' }).activeId).toBeNull();
  });

  test('large files are not persisted; history drops big bodies', () => {
    const d = emptyDraft();
    d.body.mode = 'binary';
    d.body.binary = { name: 'big.bin', type: 'application/octet-stream', size: 3_000_000, data: 'A'.repeat(4_000_000) };
    const p = persistDraft(d) as { body: { binary: { data: string; size: number } } };
    expect(p.body.binary.data).toBe('');
    expect(p.body.binary.size).toBe(3_000_000);
    const h = historyDraft(d);
    expect(h.bodyOmitted).toBe(true);
    expect(h.draft.body.binary?.data).toBe('');
    const small = emptyDraft();
    small.body.json = 'x'.repeat(HISTORY_BODY_LIMIT - 1);
    expect(historyDraft(small).draft).toBe(small);
  });
});

// =====================================================================================
describe('history', () => {
  const entry = (id: string, at: number, url: string, status?: number): HistoryEntry => {
    const u = new URL(url);
    return { id, at, draft: { ...emptyDraft(), url }, method: 'GET', host: u.host, path: u.pathname, status };
  };

  test('add caps the list and puts newest first', () => {
    let h: HistoryEntry[] = [];
    for (let i = 0; i < 5; i++) h = addHistory(h, entry(`h${i}`, i, `https://a.com/${i}`), 3);
    expect(h.map((e) => e.id)).toEqual(['h4', 'h3', 'h2']);
  });

  test('filter matches all words across method/host/path/status', () => {
    const h = [entry('1', 1, 'https://api.github.com/repos', 200), entry('2', 2, 'https://localhost:3000/login', 401)];
    expect(filterHistory(h, 'github').map((e) => e.id)).toEqual(['1']);
    expect(filterHistory(h, 'get 401').map((e) => e.id)).toEqual(['2']);
    expect(filterHistory(h, '')).toBe(h);
  });

  test('groups by day with headers', () => {
    const now = new Date(2026, 10, 8, 12).getTime();
    const rows = groupHistory([entry('a', now - 1000, 'https://a.com'), entry('b', now - 2000, 'https://a.com'), entry('c', now - 86_400_000, 'https://a.com')], now);
    expect(rows.map((r) => (r.type === 'day' ? r.label : r.entry.id))).toEqual(['Today', 'a', 'b', 'Yesterday', 'c']);
  });
});

// =====================================================================================
describe('saved tree', () => {
  const req = (id: string, name = id): SavedRequest => ({ type: 'request', id, name, draft: emptyDraft(), updatedAt: 0 });
  const folder = (id: string, children: SavedRequest[], collapsed = false): SavedFolder => ({ type: 'folder', id, name: id, collapsed, children });
  const shape = (nodes: readonly SavedNode[]): unknown => nodes.map((n) => (n.type === 'folder' ? [n.id, n.children.map((c) => c.id)] : n.id));

  test('add, rename, remove', () => {
    let t: SavedNode[] = addFolder([], folder('F', []));
    t = addSaved(t, req('a'));
    t = addSaved(t, req('b'), 'F');
    expect(shape(t)).toEqual([['F', ['b']], 'a']);
    t = renameNode(t, 'b', '  Login  ');
    expect((t[0] as SavedFolder).children[0]?.name).toBe('Login');
    expect(renameNode(t, 'b', '   ')).toBe(t);
    expect(shape(removeNode(t, 'F'))).toEqual(['a']);
    expect(shape(removeNode(t, 'b'))).toEqual([['F', []], 'a']);
    expect(uniqueName('Folder', ['Folder', 'Folder 2'])).toBe('Folder 3');
  });

  test('flatten respects collapse and search', () => {
    const t: SavedNode[] = [folder('F', [req('a', 'Login'), req('b', 'Users')]), folder('G', [req('c')], true), req('d', 'Health')];
    expect(flattenSaved(t).map((r) => r.node.id)).toEqual(['F', 'a', 'b', 'G', 'd']);
    expect(flattenSaved(t, 'user').map((r) => r.node.id)).toEqual(['F', 'b']);
    expect(flattenSaved(t, '', 'F').map((r) => r.node.id)).toEqual(['F', 'G', 'd']);
  });

  test('projectDrop: into folders, between siblings, out-dent, folders stay top-level', () => {
    const t: SavedNode[] = [folder('F', [req('a'), req('b')]), req('c'), folder('G', [], true), req('d')];
    const rows = flattenSaved(t);
    // c dropped over a → lands between F header… actually after F's header (above = F)
    expect(projectDrop(rows, 'c', 'a')).toEqual({ parentId: 'F', afterId: null });
    // d dropped over b → after a inside F
    expect(projectDrop(rows, 'd', 'b')).toEqual({ parentId: 'F', afterId: 'a' });
    // a dragged down over c → above is c (root) → root after c
    expect(projectDrop(rows, 'a', 'c')).toEqual({ parentId: null, afterId: 'c' });
    // b is the last child; dragging it left out-dents it after F
    expect(projectDrop(rows, 'b', 'b', -40)).toEqual({ parentId: null, afterId: 'F' });
    expect(projectDrop(rows, 'b', 'b', 0)).toEqual({ parentId: 'F', afterId: 'a' });
    // dropping right under a collapsed folder header stays top-level
    expect(projectDrop(rows, 'c', 'G')).toEqual({ parentId: null, afterId: 'G' });
    // folder G dragged over a → after F at the top level
    const rowsG = flattenSaved(t, '', 'G');
    expect(projectDrop(rowsG, 'G', 'a')).toEqual({ parentId: null, afterId: 'F' });
    expect(projectDrop(rows, 'd', 'F')).toEqual({ parentId: null, afterId: null });
  });

  test('moveNode applies targets; folders cannot nest', () => {
    const t: SavedNode[] = [folder('F', [req('a'), req('b')]), req('c'), folder('G', [])];
    expect(shape(moveNode(t, 'c', { parentId: 'F', afterId: 'a' }))).toEqual([['F', ['a', 'c', 'b']], ['G', []]]);
    expect(shape(moveNode(t, 'a', { parentId: null, afterId: null }))).toEqual(['a', ['F', ['b']], 'c', ['G', []]]);
    expect(shape(moveNode(t, 'G', { parentId: null, afterId: null }))).toEqual([['G', []], ['F', ['a', 'b']], 'c']);
    expect(moveNode(t, 'G', { parentId: 'F', afterId: null })).toBe(t);
    expect(shape(moveToFolder(t, 'c', 'G'))).toEqual([['F', ['a', 'b']], ['G', ['c']]]);
    expect(shape(moveToFolder(t, 'a', null))).toEqual([['F', ['b']], 'c', ['G', []], 'a']);
    expect(shape(moveWithinParent(t, 'b', -1))).toEqual([['F', ['b', 'a']], 'c', ['G', []]]);
    expect(shape(moveWithinParent(t, 'F', -1))).toEqual(shape(t));
    expect(shape(moveWithinParent(t, 'c', 1))).toEqual([['F', ['a', 'b']], ['G', []], 'c']);
  });
});

// =====================================================================================
describe('URL highlighting & variable completion', () => {
  test('spans reproduce the URL and classify parts', () => {
    const url = 'https://{{host}}/v1/users?q={{term}}&page=2#top';
    const spans = urlSpans(url, new Map([['host', 'a.com']]));
    expect(spans.map((s) => s.text).join('')).toBe(url);
    expect(spans.map((s) => [s.kind, s.text])).toEqual([
      ['scheme', 'https://'],
      ['var-resolved', '{{host}}'],
      ['path', '/v1/users'],
      ['punct', '?'],
      ['qname', 'q'],
      ['punct', '='],
      ['var-unresolved', '{{term}}'],
      ['punct', '&'],
      ['qname', 'page'],
      ['punct', '='],
      ['qvalue', '2'],
      ['hash', '#top'],
    ]);
  });

  test('URLs without a scheme start with the host', () => {
    expect(urlSpans('localhost:3000/x', new Map()).map((s) => s.kind)).toEqual(['host', 'path']);
    expect(urlSpans('', new Map())).toEqual([]);
  });

  test('completion context and insertion', () => {
    expect(varCompletionAt('https://{{ba', 12)).toEqual({ start: 8, partial: 'ba' });
    expect(varCompletionAt('https://{{base}}/x', 18)).toBeNull();
    expect(varCompletionAt('{{', 2)).toEqual({ start: 0, partial: '' });
    const c = varCompletionAt('{{ba/x', 4);
    expect(c).not.toBeNull();
    if (c) expect(applyVarCompletion('{{ba/x', 4, c, 'baseUrl')).toEqual({ text: '{{baseUrl}}/x', caret: 11 });
    const c2 = varCompletionAt('{{b}}/x', 3);
    if (c2) expect(applyVarCompletion('{{b}}/x', 3, c2, 'baseUrl').text).toBe('{{baseUrl}}/x');
  });
});

describe('hydration', () => {
  test('stored drafts get a Params table that mirrors the URL', () => {
    const d = sanitizeDraft({ method: 'get', url: 'https://x.dev/a?b=1&c=two', params: [{ name: 'old', value: 'x', enabled: false }] });
    expect(d.method).toBe('GET');
    expect(names(d.params)).toEqual([
      [false, 'old', 'x'],
      [true, 'b', '1'],
      [true, 'c', 'two'],
    ]);
  });
});

describe('history persistence budget', () => {
  test('newest entries keep their bodies until the budget is spent', () => {
    const entry = (id: string, json: string): HistoryEntry => {
      const draft = emptyDraft();
      draft.body.mode = 'json';
      draft.body.json = json;
      return { id, at: 1, draft, method: 'POST', host: 'x', path: '/' };
    };
    const out = persistHistory([entry('a', 'x'.repeat(60)), entry('b', 'y'.repeat(60)), entry('c', 'z'.repeat(10))], 100) as HistoryEntry[];
    const back = sanitizeHistory(out);
    expect(back.map((h) => [h.id, h.draft.body.json.length, !!h.bodyOmitted])).toEqual([
      ['a', 60, false],
      ['b', 0, true],
      ['c', 10, false],
    ]);
    expect(back[1]?.draft.body.mode).toBe('json');
  });
});

describe('history time labels', () => {
  test('relative today (never in the future), clock time on older days', () => {
    const now = new Date(2026, 10, 9, 12, 0, 0).getTime();
    expect(historyTimeLabel(now + 5000, now)).toBe(historyTimeLabel(now, now));
    expect(historyTimeLabel(now - 3 * 60_000, now)).toMatch(/3\s*m/);
    expect(historyTimeLabel(new Date(2026, 10, 7, 18, 5).getTime(), now)).toBe('18:05');
  });
});
