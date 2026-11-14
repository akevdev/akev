import { describe, expect, test } from 'bun:test';
import type { BodyView, ExchangeDetail, Timing } from '../../../api/types';
import {
  analyzeBody,
  bodyInfo,
  contentDispositionFilename,
  downloadFilename,
  extensionFor,
  formatJson,
  formatJsonLines,
  grpcMessages,
  grpcStatus,
  hardenHtmlPreview,
  PREVIEW_CSP,
  PREVIEW_PREAMBLE,
  kindFromContentType,
  parseCsv,
  parseJsonLines,
  parseSse,
  resolveMode,
  sseEvents,
  formFields,
} from './body';
import { certValidity, dnField, formatFingerprint, parseDn, tlsVersionInfo, validityLabel } from './cert';
import { bodyText, compareTiming, formatDelta, headersText, requestLineText, statusLineText } from './compare';
import { classifyHeader, decodeBasicAuth, headersToText, orderHeaders, parseHeaderDate, summarizeJwt } from './headers';
import { inlineText, parseInline, parseMarkdown, safeHref } from './markdown';
import { buildWaterfall, niceTicks, tickLabel } from './timing';
import { htmlPreviewFrameProps } from '../body/Previews';

function view(p: Partial<BodyView>): BodyView {
  return { kind: 'json', size: 10, wireSize: 10, encodings: [], text: '{}', formatted: null, structured: null, decodeError: null, truncated: false, captureTruncated: false, ...p };
}

describe('body model', () => {
  test('kindFromContentType', () => {
    expect(kindFromContentType('application/json; charset=utf-8')).toBe('json');
    expect(kindFromContentType('application/vnd.api+json')).toBe('json');
    expect(kindFromContentType('application/x-ndjson')).toBe('json-lines');
    expect(kindFromContentType('application/grpc+proto')).toBe('grpc');
    expect(kindFromContentType('application/grpc-web+proto')).toBe('grpc-web');
    expect(kindFromContentType('application/x-protobuf')).toBe('protobuf');
    expect(kindFromContentType('image/svg+xml')).toBe('image');
    expect(kindFromContentType('application/rss+xml')).toBe('xml');
    expect(kindFromContentType('text/event-stream')).toBe('event-stream');
    expect(kindFromContentType('multipart/form-data; boundary=x')).toBe('multipart');
    expect(kindFromContentType('font/woff2')).toBe('font');
    expect(kindFromContentType('application/msgpack')).toBe('msg-pack');
    expect(kindFromContentType('text/plain')).toBe('text');
    expect(kindFromContentType('')).toBe('unknown');
  });

  test('json modes, auto and large bodies', () => {
    const m = analyzeBody(view({}), 'application/json', true);
    expect(m.modes).toEqual(['pretty', 'raw', 'tree', 'hex']);
    expect(m.auto).toBe('pretty');
    expect(resolveMode(m, 'tree')).toBe('tree');
    expect(resolveMode(m, 'preview')).toBe('pretty');
    expect(resolveMode(m, 'auto')).toBe('pretty');
    const big = analyzeBody(view({ text: 'x'.repeat(2 * 1024 * 1024 + 1) }), 'application/json', true);
    expect(big.large).toBe(true);
    expect(big.auto).toBe('raw');
  });

  test('preview kinds and media without bytes', () => {
    expect(analyzeBody(view({ kind: 'html', text: '<p>', formatted: '<p>\n' }), 'text/html', true).modes).toEqual(['pretty', 'raw', 'preview', 'hex']);
    const svg = analyzeBody(view({ kind: 'image', text: '<svg/>' }), 'image/svg+xml', true);
    expect(svg.preview).toBe('svg');
    expect(svg.modes).toEqual(['preview', 'raw', 'hex']);
    expect(svg.prefKey).toBe('svg');
    const png = analyzeBody(view({ kind: 'image', text: null }), 'image/png', false);
    expect(png.modes).toEqual([]);
    const video = analyzeBody(view({ kind: 'video', text: null }), 'video/mp4', true);
    expect(video.modes).toEqual(['preview', 'hex']);
    expect(video.auto).toBe('preview');
  });

  test('binary decoded formats, structured kinds and fallbacks', () => {
    expect(analyzeBody(view({ kind: 'grpc', text: null, structured: [{}] }), 'application/grpc', true).auto).toBe('decoded');
    expect(analyzeBody(view({ kind: 'protobuf', text: null, structured: null }), 'application/x-protobuf', true).auto).toBe('hex');
    expect(analyzeBody(view({ kind: 'multipart', text: null }), 'multipart/form-data', true).modes).toEqual(['parts', 'hex']);
    expect(analyzeBody(view({ kind: 'event-stream', text: 'data: 1\n\n' }), 'text/event-stream', true).auto).toBe('events');
    expect(analyzeBody(view({ kind: 'form-url-encoded', text: 'a=1' }), null, false).modes).toEqual(['form', 'raw', 'hex']);
    expect(analyzeBody(view({ kind: 'unknown', text: null }), 'application/octet-stream', true).modes).toEqual(['hex']);
  });

  test('refines unknown kinds from the Content-Type; decode failures offer raw/hex', () => {
    expect(analyzeBody(view({ kind: 'unknown', text: '{}' }), 'application/json', true).kind).toBe('json');
    const failed = analyzeBody(view({ decodeError: 'invalid gzip header', text: null }), 'application/json', true);
    expect(failed.decodeFailed).toBe(true);
    expect(failed.modes).toEqual(['hex']);
    expect(failed.auto).toBe('hex');
  });

  test('bodyInfo', () => {
    expect(bodyInfo({ size: 13107, wireSize: 3174, encodings: ['gzip'] }).text).toBe('gzip · 3.1 KB → 12.8 KB');
    expect(bodyInfo({ size: 13107, wireSize: 3174, encodings: ['gzip'] }).title).toContain('76% smaller');
    expect(bodyInfo({ size: 2048, wireSize: 2048, encodings: [] }).text).toBe('2 KB');
    expect(bodyInfo({ size: 10, wireSize: 10, encodings: ['identity'] }).text).toBe('10 B');
  });

  test('download filenames', () => {
    expect(contentDispositionFilename('attachment; filename="report.pdf"')).toBe('report.pdf');
    expect(contentDispositionFilename('attachment; filename=report.csv')).toBe('report.csv');
    expect(contentDispositionFilename("attachment; filename*=UTF-8''na%C3%AFve%20file.txt; filename=\"fallback.txt\"")).toBe('naïve file.txt');
    expect(contentDispositionFilename('attachment; filename="../../etc/passwd"')).toBe('passwd');
    expect(contentDispositionFilename('inline')).toBeNull();
    expect(downloadFilename({ path: '/npm/react@19/umd/react.min.js?v=1', kind: 'javascript' })).toBe('react.min.js');
    expect(downloadFilename({ path: '/api/users?page=2', contentType: 'application/json', kind: 'json' })).toBe('users.json');
    expect(downloadFilename({ path: '/', contentType: 'text/html', kind: 'html', side: 'response' })).toBe('response-body.html');
    expect(downloadFilename({ path: '/x', kind: 'json', headers: [['Content-Disposition', 'attachment; filename="a.json"']] })).toBe('a.json');
    expect(extensionFor('application/vnd.api+json', 'unknown')).toBe('json');
    expect(extensionFor(null, 'protobuf')).toBe('bin');
  });

  test('formatting fallbacks', () => {
    expect(formatJson('{"a":1}')).toEqual({ ok: true, text: '{\n  "a": 1\n}', value: { a: 1 } });
    const bad = formatJson('{"a":');
    expect(bad.ok).toBe(false);
    expect(parseJsonLines('{"a":1}\n\nnot json\n\u001e{"b":2}')).toEqual([{ a: 1 }, { $invalid: 'not json' }, { b: 2 }]);
    expect(formatJsonLines('{"a":1}\n{"b":2}')).toBe('{\n  "a": 1\n}\n\n{\n  "b": 2\n}');
  });

  test('CSV parsing', () => {
    expect(parseCsv('a,b\n1,"x, ""y"""\r\n2,3').rows).toEqual([
      ['a', 'b'],
      ['1', 'x, "y"'],
      ['2', '3'],
    ]);
    expect(parseCsv('a;b\n1;2').rows).toEqual([
      ['a', 'b'],
      ['1', '2'],
    ]);
    const capped = parseCsv('a\n1\n2\n3\n', 2);
    expect(capped.rows).toHaveLength(2);
    expect(capped.truncated).toBe(true);
  });

  test('SSE parsing follows the spec', () => {
    expect(parseSse('﻿id: 1\nevent: up\ndata: a\ndata:b\n: comment\n\ndata: tail')).toEqual([
      { event: 'up', id: '1', retry: null, data: 'a\nb' },
      { event: null, id: '1', retry: null, data: 'tail' },
    ]);
    expect(sseEvents(view({ kind: 'event-stream', text: 'data: x\r\n\r\n', structured: null }))).toHaveLength(1);
  });

  test('structured accessors are tolerant', () => {
    expect(formFields(view({ structured: [['a', '1']] }))).toEqual([['a', '1']]);
    expect(formFields(view({ structured: null, text: 'a=1&b=x+y' }))).toEqual([
      ['a', '1'],
      ['b', 'x y'],
    ]);
    expect(grpcMessages(view({ structured: { 1: 'x' } }))).toEqual([{ compressed: false, decoded: { 1: 'x' }, trailers: null }]);
    expect(grpcMessages(view({ structured: [{ compressed: true, decoded: { 1: 2 }, trailers: [['grpc-status', '0']] }] }))[0]?.compressed).toBe(true);
  });

  test('grpcStatus', () => {
    expect(grpcStatus(null, [['grpc-status', '5'], ['grpc-message', 'user%20not%20found']])).toEqual({ code: 5, name: 'NOT_FOUND', message: 'user not found', ok: false });
    expect(grpcStatus([['grpc-status', '0']])?.ok).toBe(true);
    expect(grpcStatus([['x', '1']])).toBeNull();
  });

  test('hardenHtmlPreview puts the CSP before any captured markup', () => {
    const csp = `<meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}">`;
    const out = hardenHtmlPreview('<!doctype html><html><head><title>x</title></head><body><script>alert(1)</script></body></html>');
    expect(out.startsWith(csp)).toBe(true);
    expect(out).toContain("default-src 'none'");
    expect(out.endsWith('<!doctype html><html><head><title>x</title></head><body><script>alert(1)</script></body></html>')).toBe(true);
    expect(hardenHtmlPreview('<p>hi</p>').startsWith(csp)).toBe(true);
  });

  test('hardenHtmlPreview: <head> look-alikes cannot move the CSP out of effect (regression)', () => {
    const csp = `<meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}">`;
    for (const html of [
      // CSP used to land inside the comment…
      '<!-- <head> --><img src="http://attacker.example/beacon"><link rel=stylesheet href="http://192.168.1.1/reboot">',
      // …inside an attribute value…
      '<div title="<head>"><img src="http://attacker.example/a"></div>',
      // …or in <body> after content (ignored there by browsers).
      '<p>x</p><head></head><img src="http://attacker.example/b">',
      // A resource before the real <head> used to load before the CSP applied.
      '<link rel=stylesheet href="http://attacker.example/c"><html><head></head></html>',
      '\ufeff<!doctype html><img src="http://attacker.example/d">',
    ]) {
      const out = hardenHtmlPreview(html);
      expect(out.startsWith(csp)).toBe(true);
      // Nothing from the captured document precedes the preamble.
      expect(out.indexOf('attacker.example')).toBeGreaterThan(PREVIEW_PREAMBLE.length - 1);
      expect(out.slice(0, PREVIEW_PREAMBLE.length)).toBe(PREVIEW_PREAMBLE);
    }
  });

  test('hardenHtmlPreview neutralises tags that request outside CSP (refresh, hints, base, frames)', () => {
    const out = hardenHtmlPreview(
      '<META\nHTTP-EQUIV="refresh" content="0;url=http://x/r"><meta http-equiv="&#114;efresh" content="0;url=http://x/r2"><meta http-equiv=Content-Security-Policy content="default-src *">' +
        '<link rel=preconnect href=http://x/p><LiNk\trel=dns-prefetch href=//x><base href=http://x/><iframe srcdoc="<meta http-equiv=refresh content=0;url=http://x/n>"></iframe></iframe>' +
        '<object data=http://x/o></object><embed src=http://x/e/><meta/>',
    );
    const rest = out.slice(PREVIEW_PREAMBLE.length);
    expect(rest).not.toMatch(/<(meta|link|base|iframe|object|embed)[\s/>]/i);
    expect(rest).not.toMatch(/<\/(iframe|object)[\s/>]/i);
    expect(rest).toContain('<x-inert-META\nHTTP-EQUIV="refresh"');
    expect(rest).toContain('<x-inert-iframe srcdoc="<x-inert-meta http-equiv=refresh');
    expect(rest).toContain('</x-inert-iframe>');
    // Look-alike tag names are left alone.
    expect(hardenHtmlPreview('<metadata><linkx><basefont>').endsWith('<metadata><linkx><basefont>')).toBe(true);
  });

  test('HTML preview iframe: sandboxed with no permissions, CSP imposed by the embedder too', () => {
    const p = htmlPreviewFrameProps('<p>x</p>');
    expect(p.sandbox).toBe('');
    expect(p.csp).toBe(PREVIEW_CSP);
    expect(p.referrerPolicy).toBe('no-referrer');
    expect(p.srcDoc.startsWith(PREVIEW_PREAMBLE)).toBe(true);
    // No relaxing keywords anywhere.
    expect(`${p.sandbox} ${p.csp}`).not.toMatch(/allow-|unsafe-eval|\*|https?:/);
  });
});

describe('markdown', () => {
  test('safeHref only allows http(s), mailto and anchors', () => {
    expect(safeHref('https://akev.dev')).toBe('https://akev.dev');
    expect(safeHref('mailto:a@b.c')).toBe('mailto:a@b.c');
    expect(safeHref('#usage')).toBe('#usage');
    expect(safeHref('javascript:alert(1)')).toBeNull();
    expect(safeHref('java\tscript:alert(1)')).toBeNull();
    expect(safeHref(' JAVASCRIPT:alert(1)')).toBeNull();
    expect(safeHref('data:text/html,<script>')).toBeNull();
    expect(safeHref('/relative')).toBeNull();
  });

  test('inline formatting', () => {
    expect(parseInline('**bold** and *em* and `code` and ~~gone~~')).toEqual([
      { t: 'strong', c: [{ t: 'text', v: 'bold' }] },
      { t: 'text', v: ' and ' },
      { t: 'em', c: [{ t: 'text', v: 'em' }] },
      { t: 'text', v: ' and ' },
      { t: 'code', v: 'code' },
      { t: 'text', v: ' and ' },
      { t: 'del', c: [{ t: 'text', v: 'gone' }] },
    ]);
    expect(parseInline('snake_case_name stays')).toEqual([{ t: 'text', v: 'snake_case_name stays' }]);
    expect(parseInline('2 * 3 * 4')).toEqual([{ t: 'text', v: '2 * 3 * 4' }]);
    expect(parseInline('\\*not em\\*')).toEqual([{ t: 'text', v: '*not em*' }]);
  });

  test('links, autolinks, images and raw HTML', () => {
    expect(parseInline('[docs](https://x.dev "Title")')).toEqual([{ t: 'link', href: 'https://x.dev', c: [{ t: 'text', v: 'docs' }] }]);
    expect(parseInline('[bad](javascript:alert(1))')).toEqual([{ t: 'link', href: null, c: [{ t: 'text', v: 'bad' }] }]);
    expect(parseInline('see https://a.dev/x.')).toEqual([
      { t: 'text', v: 'see ' },
      { t: 'link', href: 'https://a.dev/x', c: [{ t: 'text', v: 'https://a.dev/x' }] },
      { t: 'text', v: '.' },
    ]);
    expect(parseInline('![logo](https://x/logo.png)')).toEqual([{ t: 'image', alt: 'logo', src: 'https://x/logo.png' }]);
    expect(parseInline('<img src=x onerror=alert(1)>')).toEqual([{ t: 'text', v: '<img src=x onerror=alert(1)>' }]);
  });

  test('blocks', () => {
    const md = '# Title\n\nPara one\ncontinues.\n\n- a\n- [x] b\n\n1. one\n2. two\n\n> quote\n\n```js\nconst a = 1;\n```\n\n---\n\n| A | B |\n|:--|--:|\n| 1 | 2 |\n\nSetext\n===';
    const blocks = parseMarkdown(md);
    expect(blocks.map((b) => b.t)).toEqual(['heading', 'paragraph', 'list', 'list', 'quote', 'code', 'hr', 'table', 'heading']);
    const list = blocks[2];
    expect(list?.t === 'list' && list.items[1]?.task).toBe(true);
    const ol = blocks[3];
    expect(ol?.t === 'list' && ol.ordered).toBe(true);
    const code = blocks[5];
    expect(code?.t === 'code' && code.lang === 'js' && code.v === 'const a = 1;').toBe(true);
    const table = blocks[7];
    expect(table?.t === 'table' && table.align).toEqual(['left', 'right']);
    const p = blocks[1];
    expect(p?.t === 'paragraph' && inlineText(p.c)).toBe('Para one continues.');
  });

  test('does not blow up on adversarial nesting', () => {
    const deep = '> '.repeat(200) + 'x';
    expect(() => parseMarkdown(deep)).not.toThrow();
    expect(() => parseInline('['.repeat(5000) + 'x' + ']'.repeat(5000))).not.toThrow();
  });
});

describe('headers model', () => {
  const hs: [string, string][] = [
    [':method', 'GET'],
    ['user-agent', 'x'],
    [':path', '/'],
    ['Accept', '*/*'],
    ['accept', 'text/html'],
  ];

  test('orderHeaders keeps pseudo-headers first; sorted is stable', () => {
    expect(orderHeaders(hs, false).map((e) => e.name)).toEqual([':method', ':path', 'user-agent', 'Accept', 'accept']);
    expect(orderHeaders(hs, true).map((e) => `${e.name}=${e.value}`)).toEqual([':method=GET', ':path=/', 'Accept=*/*', 'accept=text/html', 'user-agent=x']);
  });

  test('headersToText has no trailing newline and skips pseudo-headers', () => {
    const text = headersToText(orderHeaders(hs, false));
    expect(text).toBe('user-agent: x\nAccept: */*\naccept: text/html');
    expect(text.endsWith('\n')).toBe(false);
    expect(headersToText(orderHeaders(hs, false), true).split('\n')).toHaveLength(5);
  });

  test('classifyHeader and parseHeaderDate', () => {
    const jwt = 'Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhZGEiLCJleHAiOjE3NjI2MDMyMDB9.c2ln';
    expect(classifyHeader('Authorization', jwt)).toBe('jwt');
    expect(classifyHeader('Authorization', 'Bearer opaque-token')).toBe('bearer');
    expect(classifyHeader('authorization', 'Basic YWRhOnNlY3JldA==')).toBe('basic');
    expect(classifyHeader('Set-Cookie', 'a=1')).toBe('set-cookie');
    expect(classifyHeader('Date', 'Sun, 08 Nov 2026 10:00:00 GMT')).toBe('date');
    expect(classifyHeader('Retry-After', '120')).toBe('none');
    expect(classifyHeader('x-foo', 'bar')).toBe('none');
    expect(parseHeaderDate('nope')).toBeNaN();
  });

  test('decodeBasicAuth', () => {
    expect(decodeBasicAuth('Basic YWRhOnNlY3JldDpwYXNz')).toEqual({ user: 'ada', password: 'secret:pass' });
    expect(decodeBasicAuth('Basic !!!')).toBeNull();
    expect(decodeBasicAuth('Bearer x')).toBeNull();
  });

  test('summarizeJwt validity', () => {
    const exp = 1762603200; // 2025-11-08T12:00:00Z
    const token = (payload: object) => `Bearer eyJhbGciOiJIUzI1NiJ9.${btoa(JSON.stringify(payload)).replace(/=+$/, '')}.c2ln`;
    expect(summarizeJwt(token({ exp }), exp * 1000 - 1000)?.status).toBe('valid');
    expect(summarizeJwt(token({ exp }), exp * 1000 + 1000)?.status).toBe('expired');
    expect(summarizeJwt(token({ nbf: exp }), exp * 1000 - 1000)?.status).toBe('not-yet-valid');
    expect(summarizeJwt(token({ sub: 'x' }))?.status).toBe('no-expiry');
    expect(summarizeJwt(token({ sub: 'x' }))?.algorithm).toBe('HS256');
    expect(summarizeJwt('Bearer nope')).toBeNull();
  });
});

describe('timing model', () => {
  const base: Timing = { start: 1_700_000_000_000_000, requestHeaders: 1, requestBody: 2, dnsDuration: 10, connectDuration: 20, tlsDuration: 30, requestSent: 63, responseHeaders: 163, responseBody: 200 };

  test('lays out phases on a shared timeline', () => {
    const w = buildWaterfall(base, { state: 'complete' });
    expect(w.approximate).toBe(false);
    expect(w.phases.map((p) => [p.id, p.start, p.duration])).toEqual([
      ['received', 0, 2],
      ['dns', 1, 10],
      ['connect', 11, 20],
      ['tls', 31, 30],
      ['sent', 61, 2],
      ['wait', 63, 100],
      ['download', 163, 37],
    ]);
    expect(w.total).toBe(200);
    expect(w.ttfb).toBe(100);
    expect(w.startedAt).toBe(1_700_000_000_000);
    expect(w.pending).toBe(false);
  });

  test('tolerates missing and inconsistent phases', () => {
    const reused = buildWaterfall({ start: 0, requestHeaders: 0.5, requestSent: 1, responseHeaders: 40, responseBody: 50 }, { state: 'complete' });
    expect(reused.phases.map((p) => p.id)).toEqual(['received', 'sent', 'wait', 'download']);
    expect(reused.approximate).toBe(false);
    // requestSent before the connection was ready (inconsistent data): the absolute marks win,
    // setup bars are squeezed in front of them and keep their measured durations.
    const odd = buildWaterfall({ start: 0, requestHeaders: 1, requestBody: 5, dnsDuration: 20, connectDuration: 20, requestSent: 6, responseHeaders: 80, responseBody: 90 }, { state: 'complete' });
    expect(odd.approximate).toBe(true);
    expect(odd.phases.every((p) => p.duration > 0 && p.span >= 0)).toBe(true);
    expect(odd.phases.map((p) => [p.id, p.start, p.duration, p.span])).toEqual([
      ['received', 0, 5, 5],
      ['dns', 1, 20, 2.5],
      ['connect', 3.5, 20, 2.5],
      ['wait', 6, 74, 74],
      ['download', 80, 10, 10],
    ]);
    expect(odd.total).toBe(90);
    expect(odd.end).toBe(90);
    expect(buildWaterfall({ start: 0 }, { state: 'complete' }).phases).toEqual([]);
  });

  test('paused time is cut out and reported separately', () => {
    const w = buildWaterfall({ start: 0, requestHeaders: 1, requestBody: 2, requestSent: 5002, responseHeaders: 5100, responseBody: 5120, pausedDuration: 5000 }, { state: 'complete' });
    expect(w.paused).toBe(5000);
    expect(w.pauseAt).toBe(1);
    expect(w.total).toBe(120);
    expect(w.phases.find((p) => p.id === 'wait')?.start).toBe(2);
    expect(w.ttfb).toBe(98);
  });

  test('aborted and pending exchanges', () => {
    const ab = buildWaterfall({ start: 0, requestHeaders: 1, dnsDuration: 5, aborted: 30 }, { state: 'aborted' });
    expect(ab.abortedAt).toBe(30);
    expect(ab.total).toBe(30);
    const pend = buildWaterfall({ start: 1_000_000, requestHeaders: 1, requestSent: 3 }, { state: 'waiting', now: 1000 + 50 });
    expect(pend.pending).toBe(true);
    expect(pend.phases[pend.phases.length - 1]?.id).toBe('pending');
    expect(pend.phases[pend.phases.length - 1]?.label).toBe('Waiting for response…');
    expect(pend.total).toBeNull();
  });

  test('niceTicks and tickLabel', () => {
    expect(niceTicks(200)).toEqual([0, 50, 100, 150, 200]);
    expect(niceTicks(143)).toEqual([0, 50, 100, 150]);
    expect(niceTicks(0)).toEqual([0, 1]);
    expect(niceTicks(7)).toEqual([0, 2, 4, 6, 8]);
    expect(tickLabel(0)).toBe('0');
    expect(tickLabel(250)).toBe('250 ms');
    expect(tickLabel(1500)).toBe('1.5 s');
  });
});

describe('cert model', () => {
  test('parseDn handles escapes and quotes', () => {
    expect(parseDn("CN=R11, O=Let's Encrypt, C=US")).toEqual([
      ['CN', 'R11'],
      ['O', "Let's Encrypt"],
      ['C', 'US'],
    ]);
    expect(parseDn('CN=a\\, b, O="Acme, Inc"')).toEqual([
      ['CN', 'a, b'],
      ['O', 'Acme, Inc'],
    ]);
    expect(dnField('CN=example.com', 'cn')).toBe('example.com');
  });

  test('certValidity and labels', () => {
    const now = Date.parse('2026-11-08T00:00:00Z');
    const v = certValidity({ notBefore: '2026-10-01T00:00:00Z', notAfter: '2026-12-28T00:00:00Z' }, now);
    expect(v.status).toBe('valid');
    expect(validityLabel(v, now)).toBe('Expires in 50 days');
    expect(certValidity({ notBefore: '2026-10-01T00:00:00Z', notAfter: '2026-11-15T00:00:00Z' }, now).status).toBe('expiring');
    const ex = certValidity({ notBefore: '2024-01-01T00:00:00Z', notAfter: '2026-11-05T00:00:00Z' }, now);
    expect(ex.status).toBe('expired');
    expect(validityLabel(ex, now)).toBe('Expired 3 days ago');
    expect(certValidity({ notBefore: 'x', notAfter: 'y' }, now).status).toBe('unknown');
  });

  test('formatFingerprint and TLS versions', () => {
    expect(formatFingerprint('abcd 12ef')).toBe('AB:CD:12:EF');
    expect(tlsVersionInfo('TLSv1.3')).toEqual({ label: 'TLS 1.3', tone: 'success' });
    expect(tlsVersionInfo('TLSv1.1')?.tone).toBe('warning');
    expect(tlsVersionInfo(undefined)).toBeNull();
  });
});

describe('compare model', () => {
  const d = (p: Partial<ExchangeDetail> = {}): ExchangeDetail =>
    ({
      summary: { state: 'complete' },
      request: { method: 'GET', url: 'https://a.dev/x?b=2&a=1#frag', httpVersion: 'HTTP/2', headers: [] },
      response: { status: 200, statusMessage: 'OK', httpVersion: 'HTTP/2', headers: [] },
      timing: { start: 0, requestSent: 1, responseHeaders: 50, responseBody: 60 },
      ...p,
    }) as unknown as ExchangeDetail;

  test('requestLineText splits query parameters per line', () => {
    expect(requestLineText(d())).toBe('GET https://a.dev/x HTTP/2\n?b=2\n&a=1');
  });

  test('statusLineText', () => {
    expect(statusLineText(d())).toBe('HTTP/2 200 OK');
    expect(statusLineText(d({ response: undefined, error: { kind: 'timeout', message: 'timed out' } }))).toBe('Failed: timeout\ntimed out');
  });

  test('headersText normalises case and order', () => {
    expect(headersText([['X-B', '2'], [':path', '/'], ['x-a', '1'], ['X-A', '0']])).toBe(':path: /\nx-a: 1\nx-a: 0\nx-b: 2');
    expect(headersText(undefined)).toBe('');
  });

  test('bodyText prefers pretty text and falls back sensibly', () => {
    expect(bodyText(view({ formatted: '{\n}' })).text).toBe('{\n}');
    expect(bodyText(view({ text: '{"a":1}' })).text).toBe('{\n  "a": 1\n}');
    expect(bodyText(view({ kind: 'protobuf', text: null, structured: { 1: 'x' } })).language).toBe('json');
    expect(bodyText(view({ kind: 'image', text: null, size: 2048 }))).toEqual({ text: '<2 KB img body>', binary: true, language: 'text' });
    expect(bodyText(null).text).toBe('');
  });

  test('compareTiming and formatDelta', () => {
    const rows = compareTiming(d(), d({ timing: { start: 0, requestSent: 1, responseHeaders: 150, responseBody: 170 } }));
    expect(rows[0]).toEqual({ id: 'total', label: 'Total', a: 60, b: 170, delta: 110 });
    expect(formatDelta(60, 170)).toBe('+110 ms (+183%)');
    expect(formatDelta(100, 100.2)).toBe('same');
    expect(formatDelta(null, 1)).toBe('—');
  });
});
