import { describe, expect, test } from 'bun:test';
import { base64ToText, textToBase64 } from '../../lib/encoding';
import { collectionSize, detectCollection, importCollection, importHar, importPostman, parseCollectionText, postmanRequestToDraft } from './importers';
import { composeKeys, shortUrlLabel, suggestedRequestName, urlLabel } from './labels';
import { collectionFolders } from './library';
import type { SavedNode } from './model';
import { sampleFromSchema, importOpenApi } from './openapi';
import { detectPaste, powershellToEditable, splitPowerShellCommands } from './paste';
import { buildRequest } from './request';

describe('labels', () => {
  test('urlLabel drops scheme, query and fragment', () => {
    expect(urlLabel('https://api.example.com/v1/users?x=1#top')).toBe('api.example.com/v1/users');
    expect(urlLabel('  ')).toBe('');
  });

  test('shortUrlLabel prefers the path, shortened from the left', () => {
    expect(shortUrlLabel('https://api.example.com/v1/users?page=2')).toBe('/v1/users');
    expect(shortUrlLabel('https://api.example.com/')).toBe('api.example.com');
    expect(shortUrlLabel('https://api.example.com')).toBe('api.example.com');
    expect(shortUrlLabel('https://api.github.com/repos/akev-io/akev-io/issues')).toBe('…/akev-io/issues');
    expect(shortUrlLabel('https://api.github.com/repos/akev/akev/issues', 22)).toBe('…/akev/akev/issues');
    expect(shortUrlLabel('{{baseUrl}}/users/{{id}}')).toBe('/users/{{id}}');
    expect(shortUrlLabel('localhost:3000/health')).toBe('/health');
    // A single very long segment is kept whole (CSS truncates it).
    expect(shortUrlLabel('https://x.dev/a/averyveryveryverylongsegmentname')).toBe('…/averyveryveryverylongsegmentname');
    expect(shortUrlLabel('')).toBe('');
  });

  test('suggested names use the path (the method is shown as a badge)', () => {
    expect(suggestedRequestName('https://api.example.com/v1/users?x=1')).toBe('/v1/users');
    expect(suggestedRequestName('https://api.example.com/')).toBe('api.example.com');
    expect(suggestedRequestName('{{baseUrl}}/health')).toBe('/health');
    expect(suggestedRequestName('')).toBe('Untitled request');
  });

  test('browser builds avoid the reserved Ctrl/⌘ T and W', () => {
    expect(composeKeys(true)).toEqual({ newTab: 'mod+t', closeTab: 'mod+w', reopenTab: 'mod+shift+t' });
    expect(composeKeys(false).newTab).toBe('alt+t');
    expect(composeKeys(false).closeTab).toBe('alt+w');
  });
});

const HAR = {
  log: {
    version: '1.2',
    entries: [
      {
        request: {
          method: 'POST',
          url: 'https://api.example.com/v1/users?team=core',
          httpVersion: 'HTTP/2',
          headers: [
            { name: ':authority', value: 'api.example.com' },
            { name: 'content-type', value: 'application/json' },
            { name: 'authorization', value: 'Bearer abc.def' },
          ],
          postData: { mimeType: 'application/json', text: '{"name":"Ada"}' },
        },
        response: { status: 201 },
      },
      { request: { method: 'GET', url: 'https://cdn.example.com/app.js', headers: [] } },
      { request: { method: 'POST', url: 'https://example.com/login', headers: [], postData: { mimeType: 'application/x-www-form-urlencoded', params: [{ name: 'user', value: 'a b' }] } } },
      { request: { method: 'CONNECT', url: 'example.com:443', headers: [] } },
      { nothing: true },
    ],
  },
};

const POSTMAN = {
  info: { _postman_id: 'x', name: 'Akev API', schema: 'https://schema.getpostman.com/json/collection/v2.1.0/collection.json' },
  auth: { type: 'bearer', bearer: [{ key: 'token', value: '{{token}}', type: 'string' }] },
  variable: [
    { key: 'baseUrl', value: 'https://api.akev.dev' },
    { key: 'token', value: 'secret' },
  ],
  item: [
    { name: 'Health', request: { method: 'GET', url: { raw: '{{baseUrl}}/health', host: ['{{baseUrl}}'], path: ['health'] }, auth: { type: 'noauth' } } },
    {
      name: 'Users',
      item: [
        {
          name: 'Create user',
          request: {
            method: 'POST',
            header: [
              { key: 'Content-Type', value: 'application/json' },
              { key: 'X-Debug', value: '1', disabled: true },
            ],
            url: { raw: '{{baseUrl}}/users?notify=true', query: [{ key: 'notify', value: 'true' }, { key: 'dry', value: '1', disabled: true }] },
            body: { mode: 'raw', raw: '{"name":"{{$randomInt}}"}', options: { raw: { language: 'json' } } },
          },
        },
        {
          name: 'Admin',
          item: [
            {
              name: 'Upload avatar',
              request: {
                method: 'PUT',
                url: '{{baseUrl}}/users/1/avatar',
                auth: { type: 'basic', basic: [{ key: 'username', value: 'admin' }, { key: 'password', value: 'pw' }] },
                body: { mode: 'formdata', formdata: [{ key: 'caption', value: 'me', type: 'text' }, { key: 'file', type: 'file', src: '/home/me/a.png' }] },
              },
            },
            { name: 'Login', request: { method: 'POST', url: '{{baseUrl}}/login', body: { mode: 'urlencoded', urlencoded: [{ key: 'u', value: 'x' }] } } },
          ],
        },
      ],
    },
    { name: 'Broken' },
  ],
};

describe('collection import', () => {
  test('detects HAR and Postman documents', () => {
    expect(detectCollection(HAR)).toBe('har');
    expect(detectCollection(POSTMAN)).toBe('postman');
    expect(detectCollection({ foo: 1 })).toBeNull();
    expect(detectCollection([1])).toBeNull();
    expect(parseCollectionText('  {"log":{"entries":[]}}')?.format).toBe('har');
    expect(parseCollectionText('curl https://x')).toBeNull();
    expect(parseCollectionText('{nope')).toBeNull();
    expect(() => importCollection({})).toThrow();
  });

  test('HAR entries become structured drafts; tunnels and junk are skipped', () => {
    const c = importHar(HAR, 'session.har');
    expect(c.name).toBe('session');
    expect(c.requests.map((r) => r.name)).toEqual(['/v1/users', '/app.js', '/login']);
    expect(c.skipped).toHaveLength(2);
    const first = c.requests[0]?.draft;
    expect(first?.body.mode).toBe('json');
    expect(first?.body.json).toBe('{"name":"Ada"}');
    expect(first?.auth).toMatchObject({ type: 'bearer', token: 'abc.def' });
    expect(first?.headers.some((h) => h.name.startsWith(':'))).toBe(false);
    expect(first?.params.map((p) => [p.name, p.value])).toEqual([['team', 'core']]);
    const form = c.requests[2]?.draft;
    expect(form?.body.mode).toBe('form');
    expect(form?.body.form.map((r) => [r.name, r.value])).toEqual([['user', 'a b']]);
    expect(collectionSize(c)).toBe(3);
  });

  test('Postman: folders flattened, auth inherited, bodies mapped, variables kept', () => {
    const c = importPostman(POSTMAN);
    expect(c.name).toBe('Akev API');
    expect(c.variables.map((v) => [v.name, v.value])).toEqual([
      ['baseUrl', 'https://api.akev.dev'],
      ['token', 'secret'],
    ]);
    expect(c.requests.map((r) => r.name)).toEqual(['Health']);
    expect(c.folders.map((f) => [f.name, f.requests.map((r) => r.name)])).toEqual([
      ['Users', ['Create user']],
      ['Users / Admin', ['Upload avatar', 'Login']],
    ]);
    expect(c.skipped).toEqual(['Broken: no request']);

    const health = c.requests[0]?.draft;
    expect(health?.url).toBe('{{baseUrl}}/health');
    expect(health?.auth.type).toBe('none'); // noauth overrides the collection's bearer

    const create = c.folders[0]?.requests[0]?.draft;
    expect(create?.method).toBe('POST');
    expect(create?.auth).toMatchObject({ type: 'bearer', token: '{{token}}' });
    expect(create?.body.mode).toBe('json');
    expect(create?.body.json).toBe('{"name":"{{$randomInt}}"}');
    // Content-Type implied by the JSON mode is dropped; the disabled header is kept unchecked.
    expect(create?.headers.map((h) => [h.enabled, h.name])).toEqual([[false, 'X-Debug']]);
    expect(create?.params.map((p) => [p.enabled, p.name, p.value])).toEqual([
      [true, 'notify', 'true'],
      [false, 'dry', '1'],
    ]);

    const upload = c.folders[1]?.requests[0]?.draft;
    expect(upload?.auth).toMatchObject({ type: 'basic', username: 'admin', password: 'pw' });
    expect(upload?.body.mode).toBe('multipart');
    expect(upload?.body.multipart.map((r) => [r.kind, r.name, r.file?.name ?? r.value])).toEqual([
      ['text', 'caption', 'me'],
      ['file', 'file', 'a.png'],
    ]);
    expect(c.folders[1]?.requests[1]?.draft.body.mode).toBe('form');
  });

  test('a Postman request builds the expected wire request with the collection variables', () => {
    const c = importPostman(POSTMAN);
    const draft = c.folders[0]?.requests[0]?.draft;
    if (!draft) throw new Error('missing');
    const vars = new Map(c.variables.map((v) => [v.name, v.value]));
    const built = buildRequest(draft, vars, (n) => (n === '$randomInt' ? '7' : undefined));
    expect(built.url).toBe('https://api.akev.dev/users?notify=true');
    expect(built.request.headers).toContainEqual(['Authorization', 'Bearer secret']);
    expect(built.request.headers).toContainEqual(['Content-Type', 'application/json']);
    expect(base64ToText(built.request.body)).toBe('{"name":"7"}');
    expect(built.unresolved).toEqual([]);
  });

  test('string URLs, GraphQL bodies and text bodies', () => {
    const gql = postmanRequestToDraft({ method: 'post', url: 'https://x.dev/graphql', body: { mode: 'graphql', graphql: { query: '{ me { id } }', variables: '{"a":1}' } } });
    expect(gql.method).toBe('POST');
    expect(JSON.parse(gql.body.json)).toEqual({ query: '{ me { id } }', variables: { a: 1 } });
    const xml = postmanRequestToDraft({ method: 'POST', url: 'https://x.dev', body: { mode: 'raw', raw: '<a/>', options: { raw: { language: 'xml' } } } });
    expect(xml.body.mode).toBe('text');
    expect(xml.body.textType).toBe('application/xml');
    const plain = postmanRequestToDraft('https://x.dev/ping');
    expect(plain.method).toBe('GET');
    expect(plain.url).toBe('https://x.dev/ping');
  });

  test('structured Postman URLs without raw are rebuilt', () => {
    const d = postmanRequestToDraft({ url: { protocol: 'https', host: ['api', 'x', 'dev'], port: '8443', path: ['v1', 'items'], query: [{ key: 'q', value: 'a' }] } });
    expect(d.url).toBe('https://api.x.dev:8443/v1/items?q=a');
  });

  test('imports become uniquely named saved folders', () => {
    const c = importPostman(POSTMAN);
    let n = 0;
    const makeId = (p: string) => `${p}${++n}`;
    const existing: SavedNode[] = [{ type: 'folder', id: 'old', name: 'Akev API', collapsed: false, children: [] }];
    const folders = collectionFolders(c, existing, makeId, 1);
    expect(folders.map((f) => [f.name, f.children.length])).toEqual([
      ['Akev API 2', 1],
      ['Akev API / Users', 1],
      ['Akev API / Users / Admin', 2],
    ]);
    const ids = folders.flatMap((f) => [f.id, ...f.children.map((r) => r.id)]);
    expect(new Set(ids).size).toBe(ids.length);
    expect(folders[0]?.children[0]).toMatchObject({ type: 'request', name: 'Health', updatedAt: 1 });
  });

  test('HAR bodies that are already base64 are kept as bytes', () => {
    const c = importHar({ log: { entries: [{ request: { method: 'PUT', url: 'https://x.dev/f.bin', headers: [{ name: 'content-type', value: 'application/octet-stream' }], postData: { mimeType: 'application/octet-stream', text: textToBase64('\u0000\u0001'), encoding: 'base64' } } }] } });
    const d = c.requests[0]?.draft;
    expect(d?.body.mode).toBe('binary');
    expect(d?.body.binary?.size).toBe(2);
  });
});

const OPENAPI = {
  openapi: '3.0.3',
  info: { title: 'Pet Store', version: '1' },
  servers: [{ url: 'https://{env}.pets.dev/v1/', variables: { env: { default: 'api' } } }],
  security: [{ bearerAuth: [] }],
  components: {
    securitySchemes: { bearerAuth: { type: 'http', scheme: 'bearer' }, key: { type: 'apiKey', in: 'header', name: 'X-Key' } },
    schemas: {
      Pet: { type: 'object', required: ['name'], properties: { id: { type: 'integer', format: 'int64' }, name: { type: 'string', example: 'Rex' }, tags: { type: 'array', items: { type: 'string' } }, owner: { $ref: '#/components/schemas/Owner' } } },
      Owner: { type: 'object', properties: { email: { type: 'string', format: 'email' }, pets: { type: 'array', items: { $ref: '#/components/schemas/Pet' } } } },
    },
  },
  paths: {
    '/pets': {
      get: { tags: ['pets'], summary: 'List pets', parameters: [{ name: 'limit', in: 'query', required: true, schema: { type: 'integer', default: 20 } }, { name: 'q', in: 'query', schema: { type: 'string' } }] },
      post: { tags: ['pets'], operationId: 'createPet', requestBody: { content: { 'application/json': { schema: { $ref: '#/components/schemas/Pet' } } } } },
    },
    '/pets/{petId}': {
      parameters: [{ name: 'petId', in: 'path', required: true, schema: { type: 'string', example: 'p1' } }],
      delete: { tags: ['pets'], security: [{ key: [] }] },
      put: { tags: ['admin'], summary: 'Upload photo', security: [], requestBody: { content: { 'multipart/form-data': { schema: { type: 'object', properties: { caption: { type: 'string' }, photo: { type: 'string', format: 'binary' } } } } } } },
    },
    '/health': { get: { summary: 'Health' } },
  },
};

describe('OpenAPI import', () => {
  test('detected and grouped by tag with a baseUrl variable', () => {
    expect(detectCollection(OPENAPI)).toBe('openapi');
    expect(detectCollection({ swagger: '2.0', paths: {} })).toBe('openapi');
    const c = importCollection(OPENAPI);
    expect(c.format).toBe('openapi');
    expect(c.name).toBe('Pet Store');
    expect(c.requests.map((r) => r.name)).toEqual(['Health']);
    expect(c.folders.map((f) => [f.name, f.requests.map((r) => `${r.draft.method} ${r.name}`)])).toEqual([
      ['pets', ['GET List pets', 'POST createPet', 'DELETE DELETE /pets/{petId}']],
      ['admin', ['PUT Upload photo']],
    ]);
    const vars = Object.fromEntries(c.variables.map((v) => [v.name, v.value]));
    expect(vars).toMatchObject({ baseUrl: 'https://api.pets.dev/v1', petId: 'p1', token: '', apiKey: '' });
  });

  test('params, security and bodies', () => {
    const c = importOpenApi(OPENAPI);
    const [list, create, del] = c.folders[0]?.requests ?? [];
    expect(list?.draft.url).toBe('{{baseUrl}}/pets?limit=20');
    expect(list?.draft.params.map((p) => [p.enabled, p.name, p.value])).toEqual([
      [true, 'limit', '20'],
      [false, 'q', 'string'],
    ]);
    expect(list?.draft.auth).toMatchObject({ type: 'bearer', token: '{{token}}' });
    expect(create?.draft.body.mode).toBe('json');
    const body = JSON.parse(create?.draft.body.json ?? '{}');
    expect(body).toMatchObject({ id: 0, name: 'Rex', tags: ['string'], owner: { email: 'user@example.com' } });
    expect(del?.draft.url).toBe('{{baseUrl}}/pets/{{petId}}');
    expect(del?.draft.auth.type).toBe('none');
    expect(del?.draft.headers.map((h) => [h.name, h.value])).toEqual([['X-Key', '{{apiKey}}']]);
    const upload = c.folders[1]?.requests[0]?.draft;
    expect(upload?.auth.type).toBe('none'); // security: [] opts out
    expect(upload?.body.mode).toBe('multipart');
    expect(upload?.body.multipart.map((r) => [r.kind, r.name])).toEqual([
      ['text', 'caption'],
      ['file', 'photo'],
    ]);
  });

  test('swagger 2 bodies and recursive schemas terminate', () => {
    const c = importOpenApi({
      swagger: '2.0',
      info: { title: 'Old' },
      host: 'old.dev',
      basePath: '/api',
      schemes: ['http'],
      definitions: { Node: { type: 'object', properties: { child: { $ref: '#/definitions/Node' } } } },
      paths: {
        '/nodes': { post: { parameters: [{ name: 'body', in: 'body', schema: { $ref: '#/definitions/Node' } }] } },
        '/login': { post: { consumes: ['application/x-www-form-urlencoded'], parameters: [{ name: 'user', in: 'formData', type: 'string', required: true }] } },
      },
    });
    expect(c.variables.find((v) => v.name === 'baseUrl')?.value).toBe('http://old.dev/api');
    expect(c.requests[0]?.draft.body.mode).toBe('json');
    expect(JSON.parse(c.requests[0]?.draft.body.json ?? 'null')).toEqual({ child: null });
    expect(c.requests[1]?.draft.body.form.map((r) => [r.name, r.value])).toEqual([['user', 'string']]);
    expect(sampleFromSchema({ oneOf: [{ type: 'boolean' }, { type: 'string' }] }, (v) => v)).toBe(true);
    expect(sampleFromSchema({ allOf: [{ properties: { a: { type: 'integer', minimum: 3 } } }, { properties: { b: { enum: ['x'] } } }] }, (v) => v)).toEqual({ a: 3, b: 'x' });
  });
});

const PS = `$session = New-Object Microsoft.PowerShell.Commands.WebRequestSession
$session.UserAgent = "Mozilla/5.0 (X11; Linux x86_64)"
$session.Cookies.Add((New-Object System.Net.Cookie("sid", "abc123", "/", "example.com")))
$session.Cookies.Add((New-Object System.Net.Cookie("theme", "dark", "/", "example.com")))
Invoke-WebRequest -UseBasicParsing -Uri "https://example.com/api/items?x=1" \`
-Method "POST" \`
-WebSession $session \`
-Headers @{
"authority"="example.com"
  "method"="POST"
  "accept"="application/json"
  "x-note"="it's \`"quoted\`""
} \`
-ContentType "application/json" \`
-Body "{\`"name\`":\`"Ada\`"}"`;

describe('PowerShell paste', () => {
  test('detects and splits DevTools "Copy as PowerShell" output', () => {
    expect(detectPaste(PS)).toBe('powershell');
    expect(detectPaste('Invoke-RestMethod https://x.dev')).toBe('powershell');
    expect(detectPaste('Write-Host hi')).toBeNull();
    expect(splitPowerShellCommands(`${PS}\n${PS}`)).toHaveLength(2);
    expect(splitPowerShellCommands('iwr https://a.dev\niwr https://b.dev')).toHaveLength(2);
  });

  test('parses URI, method, headers (pseudo dropped), cookies, user agent and body', () => {
    const r = powershellToEditable(PS);
    expect(r.method).toBe('POST');
    expect(r.url).toBe('https://example.com/api/items?x=1');
    expect(r.headers).toEqual([
      ['User-Agent', 'Mozilla/5.0 (X11; Linux x86_64)'],
      ['accept', 'application/json'],
      ['x-note', 'it\'s "quoted"'],
      ['Content-Type', 'application/json'],
      ['Cookie', 'sid=abc123; theme=dark'],
    ]);
    expect(base64ToText(r.body)).toBe('{"name":"Ada"}');
  });

  test('positional URIs, single quotes and byte-array bodies', () => {
    expect(powershellToEditable("iwr 'https://x.dev/a'")).toMatchObject({ method: 'GET', url: 'https://x.dev/a' });
    const r = powershellToEditable('Invoke-WebRequest -Uri "https://x.dev" -Method "PUT" -Body ([System.Text.Encoding]::UTF8.GetBytes("hé"))');
    expect(r.method).toBe('PUT');
    expect(base64ToText(r.body)).toBe('hé');
    expect(() => powershellToEditable('Invoke-WebRequest -Method GET')).toThrow();
  });
});
