import { describe, expect, it } from 'bun:test';
import type { ExchangeSummary, RequestHead, ResponseHead, StreamMessageWithPayload } from '../../../../api/types';
import { textToBase64 } from '../../../../lib/encoding';
import { asJwt, decodeBasic, describeClaims, describeExpiry, detectAuth, jwtValidity } from './auth';
import {
  decodeEditableBody,
  editableBodySize,
  encodeEditableBody,
  formatJsonText,
  invalidHeaderNames,
  isValidHeaderName,
  languageForContentType,
  nextStatusMessage,
  requestChanged,
  syncContentLength,
  validateMethod,
  validateStatus,
  validateUrl,
} from './breakpoint';
import {
  countHeaders,
  countRequestCookies,
  countSetCookies,
  describeSize,
  headsFor,
  pausedSince,
  protocolLabel,
  requestTabs,
  resolveTab,
  responseTabGroup,
  responseTabs,
  sizeInfo,
  splitUrl,
  tlsLabel,
} from './detail';
import {
  closeCodeMeaning,
  countDirections,
  filterMessages,
  formatPayload,
  hexDump,
  mergeMessages,
  messagePreview,
  parseCloseFrame,
} from './messages';
import { computeMultiStats, statusBucket } from './multiStats';
import { extractQuery, queryParamsToText } from './query';
import { buildRawMessage, buildRawRequest, buildRawResponse, rawHeaders, requestStartLine, responseStartLine } from './rawMessage';

// -------------------------------------------------------------------------------------- fixtures

function req(over: Partial<RequestHead> = {}): RequestHead {
  return {
    method: 'POST',
    url: 'https://api.example.com/v1/users?page=2',
    scheme: 'https',
    host: 'api.example.com',
    port: 443,
    path: '/v1/users?page=2',
    httpVersion: 'HTTP/1.1',
    headers: [
      ['Host', 'api.example.com'],
      ['Content-Type', 'application/json'],
    ],
    ...over,
  };
}

function res(over: Partial<ResponseHead> = {}): ResponseHead {
  return { status: 201, statusMessage: 'Created', httpVersion: 'HTTP/1.1', headers: [['Content-Type', 'application/json']], ...over };
}

function summary(over: Partial<ExchangeSummary> = {}): ExchangeSummary {
  return {
    id: 1,
    kind: 'http',
    state: 'complete',
    startedAt: 1_000,
    method: 'GET',
    scheme: 'https',
    host: 'a.test',
    port: 443,
    path: '/',
    httpVersion: 'HTTP/1.1',
    contentKind: 'json',
    requestSize: 10,
    responseSize: 100,
    status: 200,
    duration: 50,
    ...over,
  };
}

function msg(seq: number, over: Partial<StreamMessageWithPayload> = {}): StreamMessageWithPayload {
  return { seq, direction: 'received', kind: 'text', at: 1_000_000 + seq * 1000, size: 5, text: `m${seq}`, base64: null, ...over };
}

const b64url = (o: unknown) => btoa(JSON.stringify(o)).replace(/=+$/, '').replace(/\+/g, '-').replace(/\//g, '_');
const NOW = 1_700_000_000_000;
const JWT = `${b64url({ alg: 'HS256', typ: 'JWT' })}.${b64url({ sub: 'user-42', iss: 'https://auth.test', exp: NOW / 1000 + 3600, iat: NOW / 1000 - 60, role: 'admin' })}.c2lnbmF0dXJl`;

// -------------------------------------------------------------------------------------- raw message

describe('raw message builder', () => {
  it('renders request line, headers in wire order and body text', () => {
    const text = buildRawRequest(req(), { text: '{"a":1}', size: 7 });
    expect(text).toBe('POST /v1/users?page=2 HTTP/1.1\nHost: api.example.com\nContent-Type: application/json\n\n{"a":1}');
  });

  it('uses CRLF when asked and omits an empty body', () => {
    expect(buildRawRequest(req({ method: 'GET' }), { text: '', size: 0 }, '\r\n')).toBe('GET /v1/users?page=2 HTTP/1.1\r\nHost: api.example.com\r\nContent-Type: application/json\r\n\r\n');
    expect(buildRawMessage('X', [], null)).toBe('X\n\n');
  });

  it('marks binary and truncated bodies', () => {
    expect(buildRawResponse(res(), { text: null, size: 1234 })).toEndWith('\n\n<1234 bytes binary>');
    expect(buildRawResponse(res(), { text: 'abc', size: 9_000_000, truncated: true })).toContain('<… truncated for display — 8.6 MB total>');
    expect(buildRawResponse(res(), { text: 'abc', size: 10, captureTruncated: true })).toContain('capture truncated');
  });

  it('folds HTTP/2 pseudo-headers into the start line and a Host header', () => {
    const head = req({
      httpVersion: 'HTTP/2',
      headers: [
        [':method', 'POST'],
        [':authority', 'api.example.com'],
        [':path', '/v1/users?page=2'],
        ['content-type', 'application/json'],
      ],
    });
    expect(rawHeaders(head.headers)).toEqual([
      ['Host', 'api.example.com'],
      ['content-type', 'application/json'],
    ]);
    expect(requestStartLine(head)).toBe('POST /v1/users?page=2 HTTP/2');
    expect(rawHeaders([[':authority', 'x'], ['host', 'y']])).toEqual([['host', 'y']]);
  });

  it('formats CONNECT and status lines', () => {
    expect(requestStartLine(req({ method: 'CONNECT', path: '', host: 'example.com', port: 443 }))).toBe('CONNECT example.com:443 HTTP/1.1');
    expect(requestStartLine(req({ method: 'connect', path: '', host: '::1', port: 8443 }))).toBe('CONNECT [::1]:8443 HTTP/1.1');
    expect(requestStartLine(req({ path: '' }))).toBe('POST / HTTP/1.1');
    expect(responseStartLine(res({ status: 404, statusMessage: '' }))).toBe('HTTP/1.1 404 Not Found');
    expect(responseStartLine(res({ status: 799, statusMessage: '' }))).toBe('HTTP/1.1 799');
  });
});

// -------------------------------------------------------------------------------------- query

describe('query tab', () => {
  it('keeps duplicates, skips empty segments and reports duplicate names', () => {
    const info = extractQuery('/s?a=1&&a=2&b=3#frag');
    expect(info.raw).toBe('a=1&&a=2&b=3');
    expect(info.params.map((p) => `${p.index}:${p.name}=${p.value}`)).toEqual(['0:a=1', '1:a=2', '2:b=3']);
    expect([...info.duplicates]).toEqual(['a']);
    expect(queryParamsToText(extractQuery('?a=1&flag').params)).toBe('a=1\nflag');
  });
});

// -------------------------------------------------------------------------------------- multi stats

describe('multi-select stats', () => {
  it('buckets statuses, errors and in-flight exchanges', () => {
    expect(statusBucket(summary({ status: 204 }))).toBe('2xx');
    expect(statusBucket(summary({ status: 101 }))).toBe('1xx');
    expect(statusBucket(summary({ status: 503 }))).toBe('5xx');
    expect(statusBucket(summary({ status: undefined, error: 'timeout', state: 'aborted' }))).toBe('err');
    expect(statusBucket(summary({ status: undefined, kind: 'tls-failure', state: 'complete' }))).toBe('err');
    expect(statusBucket(summary({ status: undefined, state: 'waiting' }))).toBe('pending');
    expect(statusBucket(summary({ status: undefined, state: 'complete', kind: 'tunnel' }))).toBe('other');
  });

  it('aggregates sizes, span, durations, buckets and top hosts', () => {
    const s = computeMultiStats([
      summary({ id: 1, host: 'a.test', startedAt: 1000, duration: 100, requestSize: 10, responseSize: 90 }),
      summary({ id: 2, host: 'b.test', startedAt: 1500, duration: 1000, status: 404, pinned: true }),
      summary({ id: 3, host: 'a.test', startedAt: 1200, duration: undefined, status: undefined, state: 'waiting', responseSize: 0 }),
      summary({ id: 4, host: 'c.test', startedAt: 900, duration: 20, status: undefined, error: 'dns-failed', state: 'aborted' }),
    ]);
    expect(s.count).toBe(4);
    expect(s.requestBytes).toBe(40);
    expect(s.responseBytes).toBe(90 + 100 + 0 + 100);
    expect(s.totalBytes).toBe(s.requestBytes + s.responseBytes);
    expect(s.firstStart).toBe(900);
    expect(s.lastEnd).toBe(2500);
    expect(s.spanMs).toBe(1600);
    expect(s.avgDuration).toBeCloseTo((100 + 1000 + 20) / 3);
    expect(s.maxDuration).toBe(1000);
    expect(s.buckets).toEqual([
      { bucket: '2xx', count: 1 },
      { bucket: '4xx', count: 1 },
      { bucket: 'err', count: 1 },
      { bucket: 'pending', count: 1 },
    ]);
    expect(s.topHosts[0]).toEqual({ host: 'a.test', count: 2, bytes: 100 + 10 });
    expect(s.hostCount).toBe(3);
    expect(s.pinned).toBe(1);
    expect(s.failed).toBe(1);
  });

  it('handles empty input and limits top hosts', () => {
    const empty = computeMultiStats([]);
    expect(empty).toMatchObject({ count: 0, firstStart: null, lastEnd: null, spanMs: 0, avgDuration: null, maxDuration: null, buckets: [], topHosts: [] });
    const many = computeMultiStats(Array.from({ length: 20 }, (_, i) => summary({ id: i, host: `h${i % 7}.test` })), 3);
    expect(many.topHosts).toHaveLength(3);
    expect(many.hostCount).toBe(7);
  });
});

// -------------------------------------------------------------------------------------- auth

describe('auth detection', () => {
  it('decodes Basic credentials', () => {
    expect(decodeBasic(btoa('ada:s3cr:et'))).toEqual({ username: 'ada', password: 's3cr:et', decoded: true });
    expect(decodeBasic('!!!').decoded).toBe(false);
  });

  it('detects bearer JWTs, cookies, api keys and query tokens', () => {
    const items = detectAuth(
      [
        ['Authorization', `Bearer ${JWT}`],
        ['Cookie', `theme=dark; session=${JWT}`],
        ['X-Api-Key', 'abc123'],
        ['X-Custom', JWT],
      ],
      [['access_token', 'opaque-token'], ['page', '2']],
      NOW,
    );
    expect(items.map((i) => `${i.kind}:${i.source}`)).toEqual([
      'bearer:Authorization',
      'api-key:X-Api-Key',
      'jwt:Cookie · session',
      'jwt:X-Custom',
      'api-key:Query · access_token',
    ]);
    const bearer = items[0];
    expect(bearer?.kind === 'bearer' && bearer.jwt?.payload.sub).toBe('user-42');
  });

  it('keeps unknown schemes and opaque bearer tokens', () => {
    const items = detectAuth([
      ['Authorization', 'Bearer opaque'],
      ['Proxy-Authorization', 'Digest username="x"'],
    ]);
    expect(items[0]).toMatchObject({ kind: 'bearer', token: 'opaque', jwt: null });
    expect(items[1]).toMatchObject({ kind: 'scheme', scheme: 'Digest', credentials: 'username="x"' });
    expect(detectAuth([['Accept', 'text/html']])).toEqual([]);
  });

  it('describes validity and claims', () => {
    const jwt = asJwt(JWT, NOW);
    expect(jwt).not.toBeNull();
    if (!jwt) return;
    expect(jwtValidity(jwt, NOW)).toBe('valid');
    expect(jwtValidity(jwt, NOW + 7_200_000)).toBe('expired');
    expect(describeExpiry(jwt, NOW)).toBe('Expires in 1h');
    expect(describeExpiry(jwt, NOW + 2 * 3_600_000)).toBe('Expired 1h ago');
    const rows = describeClaims(jwt.payload, NOW);
    expect(rows.map((r) => r.claim)).toEqual(['sub', 'iss', 'exp', 'iat', 'role']);
    expect(rows[2]).toMatchObject({ label: 'Expires', hint: 'in 1h', registered: true });
    expect(rows[4]).toMatchObject({ label: 'role', value: 'admin', registered: false });
    expect(asJwt('not.a.jwt')).toBeNull();
  });
});

// -------------------------------------------------------------------------------------- breakpoint

describe('breakpoint helpers', () => {
  it('decodes and encodes editable bodies', () => {
    expect(decodeEditableBody(textToBase64('héllo'))).toMatchObject({ text: 'héllo', size: 6 });
    expect(decodeEditableBody(btoa(String.fromCharCode(0xff, 0xfe, 0x00)))).toMatchObject({ text: null, size: 3 });
    expect(decodeEditableBody('')).toMatchObject({ text: '', size: 0, base64: '' });
    expect(encodeEditableBody('text', 'hé')).toEqual({ ok: true, base64: textToBase64('hé'), size: 3 });
    expect(encodeEditableBody('base64', ' aGk= ')).toEqual({ ok: true, base64: 'aGk=', size: 2 });
    expect(encodeEditableBody('base64', 'not base64!')).toEqual({ ok: false, error: 'Not valid base64' });
    expect(editableBodySize('text', '🚀')).toBe(4);
    expect(editableBodySize('base64', '%%')).toBeNull();
  });

  it('tracks Content-Length only while it matched the previous size', () => {
    const rows = [
      { name: 'Content-Type', value: 'application/json' },
      { name: 'content-length', value: '7' },
    ];
    expect(syncContentLength(rows, 7, 12)[1]?.value).toBe('12');
    expect(syncContentLength(rows, 5, 12)).toBe(rows);
    expect(syncContentLength(rows, 7, 7)).toBe(rows);
  });

  it('validates header names, URLs, methods and status codes', () => {
    expect(isValidHeaderName('X-Trace-Id')).toBe(true);
    expect(isValidHeaderName('Bad Header')).toBe(false);
    expect(invalidHeaderNames([{ name: 'ok', value: '' }, { name: 'no:pe', value: '' }, { name: 'skip me', value: '', enabled: false }])).toEqual(['no:pe']);
    expect(validateUrl('https://x.test/a')).toBeNull();
    expect(validateUrl('/relative')).toBe('Not a valid absolute URL');
    expect(validateUrl('ftp://x.test')).toBe('Use an http(s) or ws(s) URL');
    expect(validateUrl(' ')).toBe('URL is required');
    expect(validateMethod('PURGE')).toBeNull();
    expect(validateMethod('GE T')).toBe('Invalid method');
    expect(validateStatus(204)).toBeNull();
    expect(validateStatus(null)).toBe('Status is required');
    expect(validateStatus(42)).toBe('100–999');
  });

  it('keeps custom status messages but follows the default one', () => {
    expect(nextStatusMessage(200, 'OK', 404)).toBe('Not Found');
    expect(nextStatusMessage(200, '', 201)).toBe('Created');
    expect(nextStatusMessage(200, 'All good', 404)).toBe('All good');
  });

  it('detects request changes and editor languages', () => {
    const base = { method: 'GET', url: 'https://x.test', headers: [['a', '1']] as [string, string][], body: '' };
    expect(requestChanged(base, { ...base })).toBe(false);
    expect(requestChanged(base, { ...base, headers: [['a', '2']] })).toBe(true);
    expect(requestChanged(base, { ...base, body: 'aGk=' })).toBe(true);
    expect(languageForContentType('application/vnd.api+json; charset=utf-8')).toBe('json');
    expect(languageForContentType('text/html')).toBe('html');
    expect(languageForContentType('application/soap+xml')).toBe('xml');
    expect(languageForContentType(undefined)).toBe('text');
    expect(formatJsonText('{"a":[1]}')).toBe('{\n  "a": [\n    1\n  ]\n}');
    expect(formatJsonText('{oops')).toBeNull();
  });
});

// -------------------------------------------------------------------------------------- messages

describe('stream messages', () => {
  it('merges batches by seq', () => {
    const a = [msg(0), msg(1)];
    expect(mergeMessages(a, [])).toBe(a);
    expect(mergeMessages(a, [msg(2), msg(3)]).map((m) => m.seq)).toEqual([0, 1, 2, 3]);
    expect(mergeMessages(a, [msg(1, { text: 'dup' }), msg(-1)]).map((m) => `${m.seq}${m.text}`)).toEqual(['-1m-1', '0m0', '1dup']);
  });

  it('previews text, binary, empty and close frames', () => {
    expect(messagePreview(msg(0, { text: '  {"a":\n 1}  ' }))).toBe('{"a": 1}');
    expect(messagePreview(msg(0, { text: 'x'.repeat(300) }), 10)).toBe(`${'x'.repeat(10)}…`);
    expect(messagePreview(msg(0, { kind: 'binary', text: null, base64: btoa('\x82\x05\xab') }))).toBe('82 05 ab');
    expect(messagePreview(msg(0, { kind: 'ping', text: null }))).toBe('');
    expect(messagePreview(msg(0, { text: '' }))).toBe('(empty)');
    expect(messagePreview(msg(0, { kind: 'close', text: '1000 Normal closure' }))).toBe('1000 Normal closure');
  });

  it('decodes close frames from bytes or text', () => {
    expect(parseCloseFrame({ text: null, base64: btoa('\x03\xe9bye') })).toEqual({ code: 1001, reason: 'bye' });
    expect(parseCloseFrame({ text: '4001: kicked', base64: null })).toEqual({ code: 4001, reason: 'kicked' });
    expect(parseCloseFrame({ text: 'gone', base64: null })).toEqual({ code: null, reason: 'gone' });
    expect(closeCodeMeaning(1006)).toBe('Abnormal closure');
    expect(closeCodeMeaning(4404)).toBe('Application-defined');
    expect(closeCodeMeaning(null)).toBe('No close code');
  });

  it('filters by text and direction and counts directions', () => {
    const list = [msg(0, { text: 'hello', direction: 'sent' }), msg(1, { text: 'world' }), msg(2, { kind: 'event', event: 'metrics', text: '{}' })];
    expect(filterMessages(list, '', 'all')).toBe(list);
    expect(filterMessages(list, 'HEL', 'all').map((m) => m.seq)).toEqual([0]);
    expect(filterMessages(list, '', 'received').map((m) => m.seq)).toEqual([1, 2]);
    expect(filterMessages(list, 'metrics', 'all').map((m) => m.seq)).toEqual([2]);
    expect(countDirections(list)).toEqual({ all: 3, sent: 1, received: 2 });
  });

  it('formats payloads as JSON, prefixed JSON, text or hex', () => {
    expect(formatPayload(msg(0, { text: '{"a":1}' }))).toEqual({ language: 'json', text: '{\n  "a": 1\n}' });
    expect(formatPayload(msg(0, { text: '42["m",{"x":1}]' }))).toMatchObject({ language: 'json', prefix: '42' });
    expect(formatPayload(msg(0, { text: 'plain' }))).toEqual({ language: 'text', text: 'plain' });
    expect(formatPayload(msg(0, { kind: 'binary', text: null, base64: btoa('AB\x00') }))).toEqual({ language: 'hex', text: '0000  41 42 00' + ' '.repeat(40) + ' AB.' });
    expect(hexDump(new Uint8Array(17)).split('\n')).toHaveLength(2);
  });
});

// -------------------------------------------------------------------------------------- detail

describe('detail helpers', () => {
  it('builds tab sets and resolves remembered tabs', () => {
    expect(requestTabs({ hasAuth: false })).toEqual(['headers', 'query', 'body', 'cookies', 'raw', 'code']);
    expect(requestTabs({ hasAuth: true })).toContain('auth');
    expect(responseTabs({ stream: true })[0]).toBe('messages');
    expect(responseTabs({ stream: false })).not.toContain('messages');
    expect(resolveTab(['headers', 'body'], 'body')).toBe('body');
    expect(resolveTab(['headers', 'body'], 'auth')).toBe('headers');
    expect(resolveTab(['headers', 'body'], undefined)).toBe('headers');
    expect(responseTabGroup('web-socket')).toBe('traffic.response.stream');
    expect(responseTabGroup('http')).toBe('traffic.response');
    // Recognised LLM calls lead with the conversation view and remember their tab separately.
    expect(responseTabs({ stream: true, llm: true }).slice(0, 2)).toEqual(['llm', 'messages']);
    expect(responseTabs({ stream: false, llm: true })[0]).toBe('llm');
    expect(responseTabs({ stream: false })).not.toContain('llm');
    expect(responseTabGroup('event-stream', true)).toBe('traffic.response.llm');
  });

  it('counts cookies and headers', () => {
    expect(countRequestCookies([['Cookie', 'a=1; b=2'], ['cookie', 'c=3']])).toBe(3);
    expect(countSetCookies([['Set-Cookie', 'a=1'], ['set-cookie', 'b=2'], ['x', 'y']])).toBe(2);
    expect(countHeaders([[':status', '200'], ['a', '1']])).toBe(1);
  });

  it('describes sizes, protocols and TLS versions', () => {
    expect(describeSize(sizeInfo({ wireSize: 3174, capturedSize: 3174, truncated: false, encodings: ['gzip'], decodedSize: 13107 }))).toBe('3.1 KB → 12.8 KB');
    expect(describeSize(sizeInfo({ wireSize: 512, capturedSize: 512, truncated: false, encodings: [], decodedSize: 512 }))).toBe('512 B');
    expect(sizeInfo(undefined, 42)).toEqual({ wire: 42, decoded: null, encodings: [] });
    expect(protocolLabel('web-socket', 'HTTP/1.1')).toBe('WebSocket · HTTP/1.1');
    expect(protocolLabel('http', 'HTTP/2')).toBe('HTTP/2');
    expect(tlsLabel('TLSv1.3')).toBe('TLS 1.3');
    expect(tlsLabel('SSLv3')).toBe('SSLv3');
    expect(tlsLabel(undefined)).toBeNull();
  });

  it('splits URLs for emphasis', () => {
    expect(splitUrl({ scheme: 'https', host: 'api.test', port: 8443, path: '/a/b?x=1' })).toEqual({ scheme: 'https://', authority: 'api.test:8443', path: '/a/b', query: '?x=1' });
    expect(splitUrl({ scheme: 'http', host: 'x', port: 80, path: '/' })).toEqual({ scheme: 'http://', authority: 'x', path: '/', query: '' });
  });

  it('picks original heads and paused start times', () => {
    const detail = {
      request: req(),
      response: res(),
      originalRequest: req({ method: 'GET' }),
    } as Parameters<typeof headsFor>[0];
    expect(headsFor(detail, false).request.method).toBe('POST');
    expect(headsFor(detail, true)).toMatchObject({ original: true, request: { method: 'GET' }, response: { status: 201 } });
    expect(pausedSince({ timing: { start: 0, requestBody: 12 } }, { startedAt: 1000, paused: 'request' })).toBe(1012);
    expect(pausedSince({ timing: { start: 0, responseHeaders: 80 } }, { startedAt: 1000, paused: 'response' })).toBe(1080);
    expect(pausedSince(undefined, { startedAt: 1000, paused: 'request' })).toBe(1000);
  });
});

describe('filter clauses', () => {
  it('quotes values and appends clauses', async () => {
    const { appendFilterClause, headerFilterClause, quoteFilterValue } = await import('./detail');
    expect(quoteFilterValue('gzip')).toBe('gzip');
    expect(quoteFilterValue('text/html; charset=utf-8')).toBe('"text/html; charset=utf-8"');
    expect(quoteFilterValue('say "hi"')).toBe('"say \\"hi\\""');
    expect(headerFilterClause('Content-Encoding', 'br')).toBe('header.content-encoding=br');
    expect(headerFilterClause('X-Trace', '')).toBe('header:x-trace');
    expect(appendFilterClause('', 'a:b')).toBe('a:b');
    expect(appendFilterClause(' method:get ', 'a:b')).toBe('method:get a:b');
    expect(appendFilterClause('a:b', 'a:b')).toBe('a:b');
  });

  it('header clauses match the clicked value exactly, never as a wildcard (regression)', async () => {
    const { headerFilterClause } = await import('./detail');
    // `header.accept:*/*` was a glob matching any value with a slash; `=` with a quoted value is literal.
    expect(headerFilterClause('Accept', '*/*')).toBe('header.accept="*/*"');
    expect(headerFilterClause('Access-Control-Allow-Origin', '*')).toBe('header.access-control-allow-origin="*"');
    expect(headerFilterClause('If-None-Match', '*')).toBe('header.if-none-match="*"');
    // Values that would fuse with the operator or read as negation/regex are quoted.
    expect(headerFilterClause('X', '=y')).toBe('header.x="=y"');
    expect(headerFilterClause('X', '-1')).toBe('header.x="-1"');
    expect(headerFilterClause('X', '/api/')).toBe('header.x="/api/"');
    expect(headerFilterClause('X', 'a|b')).toBe('header.x="a|b"');
    expect(headerFilterClause('Link', '<https://a/?p=2>; rel="next"')).toBe('header.link="<https://a/?p=2>; rel=\\"next\\""');
    // Names a dotted subkey can't hold use the bracket form; odd names are quoted for presence.
    expect(headerFilterClause('X!Odd', 'v')).toBe('header[x!odd]=v');
    expect(headerFilterClause('x*y', '')).toBe('header:"x*y"');
  });

  it('a header clause applies to every OR alternative of the existing filter (regression)', async () => {
    const { appendFilterClause, headerFilterClause } = await import('./detail');
    const c = headerFilterClause('X', 'y');
    expect(appendFilterClause('path:/a | path:/b', c)).toBe('(path:/a | path:/b) header.x=y');
    expect(appendFilterClause('path:/a OR path:/b', c)).toBe('(path:/a OR path:/b) header.x=y');
    expect(appendFilterClause('(a | b) header.x=y', c)).toBe('(a | b) header.x=y');
    // An OR inside quotes or parentheses is not top-level.
    expect(appendFilterClause('host:"a | b"', c)).toBe('host:"a | b" header.x=y');
  });
});
