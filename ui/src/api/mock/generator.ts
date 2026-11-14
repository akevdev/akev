/**
 * Realistic traffic generation for the mock backend: a weighted catalog of scenarios (APIs,
 * CDNs, pages, websockets, SSE, gRPC, errors, TLS failures, tunnels, mocked/modified/paused
 * exchanges) and a small state machine that advances each exchange through its lifecycle.
 */

import { statusText } from '../../lib/http';
import { bytesToBase64, utf8Encode } from '../../lib/encoding';
import type {
  BreakpointStage,
  ClientInfo,
  ContentKind,
  ErrorKind,
  ExchangeError,
  ExchangeKind,
  ExchangeSummary,
  Headers,
  HttpVersion,
  RequestHead,
  ResponseHead,
  RuleId,
  StreamMessageWithPayload,
  TagColor,
  Timing,
  UpstreamInfo,
} from '../types';
import * as B from './bodies';
import { RULE_IDS } from './data';
import { llmTag, type LlmFixture } from './llm';
import type { Rng } from './rng';
import { mockCertificates } from './certs';
import { mockUpstreamFingerprint } from './tlsProfiles';

export type Phase = 'requesting' | 'waiting' | 'responding' | 'paused' | 'streaming' | 'done';

export interface MockRecord {
  summary: ExchangeSummary;
  request: RequestHead;
  response?: ResponseHead;
  timing: Timing;
  client: ClientInfo;
  upstream?: UpstreamInfo;
  error?: ExchangeError;
  tunnel?: { bytesSent: number; bytesReceived: number; reason: string };
  note?: string;
  matchedRules: RuleId[];
  originalRequest?: RequestHead;
  originalResponse?: ResponseHead;
  reqSpec: B.BodySpec | null;
  resSpec: B.BodySpec | null;
  reqData?: string | Uint8Array;
  resData?: string | Uint8Array;
  messages: StreamMessageWithPayload[];
  /** Seed for lazily generated bodies / messages. */
  seed: number;
  phase: Phase;
  /** Wall-clock ms of the next lifecycle step (null = waiting on the user, or done). */
  nextAt: number | null;
  plan: Plan;
  /** A recorded LLM API call (`/api/exchanges/:id/llm`); its SSE events stream as messages. */
  llm?: LlmFixture;
}

/** Errors that happen while opening the upstream connection. */
const CONNECT_ERRORS: ReadonlySet<string> = new Set(['connection-refused', 'dns-failed', 'host-unreachable', 'upstream-cert-invalid', 'upstream-tls-failed', 'upstream-proxy-failed']);

interface Plan {
  reqDone: number;
  /** Request fully written upstream (after DNS/connect/TLS on a fresh connection). */
  sent: number;
  ttfb: number;
  total: number;
  status?: number;
  resHeaders: Headers;
  error?: ExchangeError;
  /** Abort after this many ms (instead of responding). */
  abortAt?: number;
  pause?: BreakpointStage;
  stream?: 'ws' | 'sse';
  streamUntil?: number;
}

interface Scenario {
  w: number;
  host: string;
  port?: number;
  scheme?: 'http' | 'https' | 'ws' | 'wss';
  method: string;
  path: (r: Rng) => string;
  status?: number | ((r: Rng) => number);
  kind?: ExchangeKind;
  res?: (r: Rng, status: number) => B.BodySpec | null;
  req?: (r: Rng) => B.BodySpec | null;
  sources: readonly string[];
  latency: [number, number];
  version?: HttpVersion;
  error?: { kind: ErrorKind; message: string; code?: string; at?: 'connect' | 'response' };
  rule?: { id: RuleId; mocked?: boolean; modified?: boolean; color?: TagColor; pause?: BreakpointStage };
  stream?: 'ws' | 'sse';
  extraResHeaders?: (r: Rng) => Headers;
  llm?: LlmFixture;
}

/** A scenario replaying an LLM API fixture. */
function llmScenario(f: LlmFixture): Scenario {
  const sse = f.events != null;
  return {
    w: 0,
    host: f.host,
    port: f.port,
    scheme: f.scheme,
    method: 'POST',
    path: () => f.path,
    status: f.status,
    kind: sse ? 'event-stream' : undefined,
    stream: sse ? 'sse' : undefined,
    req: () => spec('json', 'application/json', () => f.requestBody),
    res: () => spec(sse ? 'event-stream' : 'json', f.contentType, () => f.responseBody),
    sources: [f.source],
    latency: f.latency,
    version: f.host === 'localhost' ? 'HTTP/1.1' : 'HTTP/2',
    extraResHeaders: (r) =>
      f.host === 'api.anthropic.com'
        ? [['request-id', `req_011CUw${r.int(1e6, 9e6).toString(36)}`], ['anthropic-ratelimit-requests-remaining', String(r.int(3000, 3999))]]
        : f.host === 'api.openai.com'
          ? [['x-request-id', `req_${r.int(1e8, 9e8).toString(16)}`], ['openai-processing-ms', String(r.int(200, 900))], ['x-ratelimit-remaining-tokens', String(r.int(780_000, 799_000))]]
          : [],
    llm: f,
  };
}

const spec = (kind: ContentKind, contentType: string, make: B.BodySpec['make'], extra: Partial<B.BodySpec> = {}): B.BodySpec => ({ kind, contentType, make, ...extra });
const jsonSpec = (make: (r: Rng) => string, encoding?: B.BodySpec['encoding']): B.BodySpec => spec('json', 'application/json', make, { encoding });

const BROWSER = ['Chrome', 'Chrome', 'Chrome', 'Firefox', 'Brave'] as const;
const CLI = ['curl', 'python3', 'node', 'Go-http-client'] as const;

const SCENARIOS: readonly Scenario[] = [
  // ---------------------------------------------------------------- api.github.com
  { w: 7, host: 'api.github.com', method: 'GET', path: (r) => `/repos/akev/${r.pick(['akev', 'hyper', 'tokio', 'vite'])}`, status: (r) => (r.chance(0.12) ? 304 : 200), res: (_r, s) => (s === 304 ? null : jsonSpec(B.repoJson, 'gzip')), sources: ['Chrome', 'node', 'curl'], latency: [60, 420], version: 'HTTP/2' },
  { w: 4, host: 'api.github.com', method: 'GET', path: () => '/repos/akev/akev/pulls?state=open&per_page=30', res: () => jsonSpec(B.pullsJson, 'gzip'), sources: ['Chrome', 'node'], latency: [90, 600], version: 'HTTP/2' },
  { w: 2, host: 'api.github.com', method: 'GET', path: () => '/user', status: (r) => (r.chance(0.35) ? 401 : 200), res: (_r, s) => jsonSpec(() => (s === 401 ? B.json({ message: 'Requires authentication', documentation_url: 'https://docs.github.com/rest' }) : B.json({ login: 'ada', id: 1000, name: 'Ada Lovelace', public_repos: 42 }))), sources: ['curl', 'python3'], latency: [40, 200], version: 'HTTP/2' },
  { w: 3, host: 'api.github.com', method: 'POST', path: () => '/graphql', req: () => spec('json', 'application/json', B.graphqlRequest), res: () => jsonSpec(B.graphqlResponse), sources: ['node', 'Chrome'], latency: [120, 700], version: 'HTTP/2' },
  { w: 1, host: 'api.github.com', method: 'GET', path: () => '/repos/akev/missing', status: 404, res: () => jsonSpec(() => B.json({ message: 'Not Found', documentation_url: 'https://docs.github.com/rest/repos/repos#get-a-repository' })), sources: ['curl'], latency: [40, 160], version: 'HTTP/2' },
  // ---------------------------------------------------------------- CDN / static assets
  { w: 5, host: 'cdn.jsdelivr.net', method: 'GET', path: (r) => `/npm/react@19.1.0/umd/react.production.min.js?v=${r.int(1, 9)}`, res: () => spec('javascript', 'application/javascript', B.jsBundle, { encoding: 'br' }), sources: BROWSER, latency: [20, 140], version: 'HTTP/2' },
  { w: 4, host: 'cdn.jsdelivr.net', method: 'GET', path: () => '/npm/bootstrap@5.3.3/dist/css/bootstrap.min.css', res: () => spec('css', 'text/css', B.cssBundle, { encoding: 'br' }), sources: BROWSER, latency: [15, 120], version: 'HTTP/2' },
  { w: 3, host: 'cdn.jsdelivr.net', method: 'GET', path: () => '/gh/akev/brand@1.0/logo.svg', res: () => spec('image', 'image/svg+xml', B.svgLogo), sources: BROWSER, latency: [15, 90], version: 'HTTP/2' },
  { w: 3, host: 'fonts.gstatic.com', method: 'GET', path: (r) => `/s/inter/v13/UcC73FwrK3iLTeHuS_${r.int(10, 99)}.woff2`, res: (r) => spec('font', 'font/woff2', (rr) => B.binaryBlob(rr, 2048, [0x77, 0x4f, 0x46, 0x32]), { reportedSize: r.int(18_000, 48_000) }), sources: BROWSER, latency: [10, 80], version: 'HTTP/2' },
  // ---------------------------------------------------------------- google
  { w: 3, host: 'www.google.com', method: 'GET', path: () => '/', res: () => spec('html', 'text/html; charset=UTF-8', () => B.htmlPage('Google'), { encoding: 'br' }), sources: BROWSER, latency: [80, 300], version: 'HTTP/2' },
  { w: 5, host: 'www.google.com', method: 'GET', path: (r) => `/complete/search?q=${encodeURIComponent(r.pick(['akev', 'http proxy', 'rust async']))}&client=chrome`, res: () => jsonSpec(B.suggestJson), sources: ['Chrome'], latency: [30, 160], version: 'HTTP/2' },
  { w: 3, host: 'www.google.com', method: 'GET', path: () => '/images/branding/googlelogo/2x/googlelogo_color_272x92dp.png', res: () => spec('image', 'image/png', B.pngBytes), sources: BROWSER, latency: [15, 90], version: 'HTTP/2' },
  { w: 2, host: 'www.google.com', method: 'POST', path: (r) => `/gen_204?atyp=i&ei=${r.int(100000, 999999)}`, status: 204, res: () => null, sources: ['Chrome'], latency: [20, 80], version: 'HTTP/2' },
  // ---------------------------------------------------------------- localhost dev server
  { w: 6, host: 'localhost', port: 3000, scheme: 'http', method: 'GET', path: (r) => `/api/users?page=${r.int(1, 5)}&limit=20`, res: () => jsonSpec((r) => B.usersJson(r, 8)), sources: ['Chrome', 'Firefox', 'node'], latency: [8, 90], rule: { id: RULE_IDS.authHeader, modified: true } },
  { w: 3, host: 'localhost', port: 3000, scheme: 'http', method: 'POST', path: () => '/api/login', status: (r) => (r.chance(0.3) ? 401 : 200), req: () => spec('json', 'application/json', B.loginRequest), res: (_r, s) => jsonSpec(() => B.loginResponse(s === 200)), sources: ['Chrome'], latency: [60, 300], rule: { id: RULE_IDS.login, pause: 'request' } },
  { w: 3, host: 'localhost', port: 3000, scheme: 'http', method: 'GET', path: (r) => `/api/orders/${r.int(10, 99)}`, res: () => jsonSpec(B.orderJson), sources: ['Chrome', 'python3'], latency: [12, 140] },
  { w: 2, host: 'localhost', port: 3000, scheme: 'http', method: 'PUT', path: (r) => `/api/users/${r.int(1000, 1010)}`, req: () => spec('json', 'application/json', () => B.json({ role: 'editor', active: true })), res: () => jsonSpec(() => B.json({ ok: true })), sources: ['Chrome'], latency: [20, 120] },
  { w: 2, host: 'localhost', port: 3000, scheme: 'http', method: 'PATCH', path: (r) => `/api/orders/${r.int(10, 99)}`, req: () => spec('json', 'application/json', () => B.json({ status: 'shipped' })), status: (r) => (r.chance(0.25) ? 422 : 200), res: (_r, s) => jsonSpec(() => (s === 422 ? B.errorJson(422, 'status: cannot transition from delivered to shipped') : B.json({ ok: true }))), sources: ['Chrome'], latency: [20, 120] },
  { w: 2, host: 'localhost', port: 3000, scheme: 'http', method: 'DELETE', path: (r) => `/api/sessions/${r.int(100000, 999999).toString(36)}`, status: 204, res: () => null, sources: ['Chrome', 'curl'], latency: [10, 60] },
  { w: 2, host: 'localhost', port: 3000, scheme: 'http', method: 'GET', path: () => '/api/feature-flags', res: () => jsonSpec(() => B.json({ newCheckout: true, darkMode: true, betaSearch: false })), sources: ['Chrome'], latency: [1, 4], rule: { id: RULE_IDS.flags, mocked: true } },
  { w: 2, host: 'localhost', port: 3000, scheme: 'http', method: 'GET', path: () => '/', res: () => spec('html', 'text/html', () => B.htmlPage('Acme Dashboard')), sources: ['Chrome', 'Firefox'], latency: [5, 40] },
  { w: 2, host: 'localhost', port: 3000, scheme: 'http', method: 'GET', path: () => '/src/main.tsx', res: () => spec('javascript', 'text/javascript', B.viteModule), sources: ['Chrome', 'Firefox'], latency: [3, 30] },
  { w: 1, host: 'localhost', port: 3000, scheme: 'http', method: 'GET', path: () => '/api/reports/export', status: 500, res: () => jsonSpec(() => B.errorJson(500, 'TypeError: Cannot read properties of undefined (reading "rows")')), sources: ['Chrome'], latency: [80, 400] },
  { w: 1, host: 'localhost', port: 3000, scheme: 'http', method: 'POST', path: () => '/api/upload', req: () => spec('multipart', 'multipart/form-data; boundary=----akev7MA4YWxkTrZu0gW', () => B.multipartBody('----akev7MA4YWxkTrZu0gW')), status: 201, res: () => jsonSpec(() => B.json({ id: 'upl_91x', size: 128 })), sources: ['Chrome'], latency: [120, 500] },
  { w: 1, host: 'localhost', port: 3000, scheme: 'ws', method: 'GET', path: () => '/socket.io/?EIO=4&transport=websocket', status: 101, kind: 'web-socket', stream: 'ws', sources: ['Chrome'], latency: [5, 20] },
  { w: 1, host: 'localhost', port: 3000, scheme: 'http', method: 'GET', path: () => '/api/events', kind: 'event-stream', stream: 'sse', res: () => spec('event-stream', 'text/event-stream', (r) => B.sseBody(r)), sources: ['Chrome'], latency: [5, 20] },
  // ---------------------------------------------------------------- payments & misc APIs
  { w: 2, host: 'api.stripe.com', method: 'POST', path: () => '/v1/payment_intents', req: () => spec('form-url-encoded', 'application/x-www-form-urlencoded', B.formBody), res: () => jsonSpec((r) => B.json({ id: `pi_${r.int(1e6, 9e6)}`, object: 'payment_intent', amount: 2999, currency: 'eur', status: 'requires_payment_method', client_secret: 'pi_secret_redacted' })), sources: ['node', 'python3'], latency: [180, 700], version: 'HTTP/2', rule: { id: RULE_IDS.stripe, color: 'purple' } },
  { w: 2, host: 'api.stripe.com', method: 'GET', path: () => '/v1/customers?limit=10', res: () => jsonSpec((r) => B.usersJson(r, 4)), sources: ['node'], latency: [150, 500], version: 'HTTP/2', rule: { id: RULE_IDS.stripe, color: 'purple' } },
  { w: 2, host: 'registry.npmjs.org', method: 'GET', path: (r) => `/${r.pick(['react', 'vite', 'zustand', 'cmdk'])}`, res: () => spec('json', 'application/vnd.npm.install-v1+json', B.npmPackageJson, { encoding: 'gzip' }), sources: ['node'], latency: [60, 300], version: 'HTTP/2' },
  { w: 2, host: 'grpc.example.com', method: 'POST', path: () => '/helloworld.Greeter/SayHello', req: () => spec('grpc', 'application/grpc', B.grpcFrame, { structured: () => ({ '1': 'Ada' }) }), res: () => spec('grpc', 'application/grpc', B.grpcFrame, { structured: (r) => ({ '1': 'akev', '2': r.int(1, 120), '3': 'Hello, Ada!', '4': 1 }) }), sources: ['Go-http-client', 'python3'], latency: [10, 80], version: 'HTTP/2' },
  { w: 2, host: 'telemetry.example.io', method: 'POST', path: () => '/v1/traces', req: () => spec('protobuf', 'application/x-protobuf', B.protobufBytes, { structured: (r) => ({ '1': 'akev', '2': r.int(1, 120), '3': 'Hello, Grace!', '4': 1 }) }), status: 202, res: () => null, sources: ['Slack', 'node'], latency: [40, 200], version: 'HTTP/2' },
  { w: 1, host: 'telemetry.example.io', method: 'POST', path: () => '/collect', req: () => spec('json-lines', 'application/x-ndjson', B.telemetryNdjson), status: 204, res: () => null, sources: ['Chrome'], latency: [20, 90], version: 'HTTP/2' },
  { w: 1, host: 'akev.dev', method: 'GET', path: () => '/releases.xml', res: () => spec('xml', 'application/rss+xml', B.xmlFeed), sources: ['curl'], latency: [40, 180] },
  { w: 1, host: 's3.eu-central-1.amazonaws.com', method: 'GET', path: () => '/acme-reports/q3-report.pdf', res: (r) => spec('pdf', 'application/pdf', (rr) => B.binaryBlob(rr, 1024, [0x25, 0x50, 0x44, 0x46, 0x2d, 0x31, 0x2e, 0x37]), { reportedSize: r.int(200_000, 2_400_000) }), sources: ['Chrome'], latency: [200, 1400] },
  { w: 1, host: 'media.example.com', method: 'GET', path: () => '/video/intro.mp4', status: 206, res: (r) => spec('video', 'video/mp4', (rr) => B.binaryBlob(rr, 1024, [0, 0, 0, 0x20, 0x66, 0x74, 0x79, 0x70]), { reportedSize: r.int(1_000_000, 8_000_000) }), sources: ['Chrome'], latency: [300, 2500], version: 'HTTP/2' },
  { w: 1, host: 'downloads.example.com', method: 'GET', path: () => '/agent/akev-agent-linux-x64.tar.gz', res: (r) => spec('binary', 'application/octet-stream', (rr) => B.binaryBlob(rr, 1536, [0x1f, 0x8b, 0x08, 0x00]), { reportedSize: r.int(4_000_000, 12_000_000) }), sources: ['curl', 'Go-http-client'], latency: [800, 6000] },
  { w: 1, host: 'echo.websocket.events', scheme: 'wss', method: 'GET', path: () => '/', status: 101, kind: 'web-socket', stream: 'ws', sources: ['node', 'Chrome'], latency: [60, 200] },
  { w: 1, host: 'okhttp.example.app', method: 'GET', path: () => '/v2/feed?cursor=eyJwIjoyfQ', res: () => jsonSpec((r) => B.usersJson(r, 3), 'gzip'), sources: ['okhttp'], latency: [90, 500], version: 'HTTP/2' },
  // ---------------------------------------------------------------- slow & failing
  { w: 1, host: 'slow.example.com', method: 'GET', path: () => '/api/report?range=90d', res: () => jsonSpec(() => B.json({ rows: 1204, generatedIn: '2.4s' })), sources: ['python3'], latency: [2600, 7000], rule: { id: RULE_IDS.slow } },
  { w: 1, host: 'api.down.example', port: 8443, method: 'GET', path: () => '/health', sources: CLI, latency: [2, 10], error: { kind: 'connection-refused', message: 'Connection refused (os error 111)', code: 'ECONNREFUSED', at: 'connect' } },
  { w: 1, host: 'expired.badssl.com', method: 'GET', path: () => '/', sources: ['curl', 'python3'], latency: [80, 200], error: { kind: 'upstream-cert-invalid', message: 'invalid peer certificate: Expired', code: 'CertificateExpired', at: 'connect' } },
  { w: 1, host: 'unknown-host.invalid', method: 'GET', path: () => '/v1/ping', sources: CLI, latency: [5, 40], error: { kind: 'dns-failed', message: 'failed to lookup address information: Name or service not known', code: 'ENOTFOUND', at: 'connect' } },
  { w: 1, host: 'flaky.example.net', method: 'GET', path: () => '/stream/chunks', sources: ['node'], latency: [200, 900], error: { kind: 'connection-reset', message: 'connection reset by peer', code: 'ECONNRESET', at: 'response' } },
  { w: 1, host: 'www.google.com', method: 'GET', path: () => '/xjs/_/js/k=xjs.s.en.cancelled', sources: ['Chrome'], latency: [100, 400], error: { kind: 'client-aborted', message: 'client closed the connection', at: 'response' } },
  { w: 1, host: 'mobile.bank.example', method: 'CONNECT', path: () => '', kind: 'tls-failure', sources: ['okhttp'], latency: [30, 90], error: { kind: 'client-tls-rejected', message: 'client rejected certificate: unknown_ca', code: 'unknown_ca', at: 'connect' } },
  { w: 1, host: 'updates.example.com', method: 'CONNECT', path: () => '', kind: 'tunnel', sources: ['Chrome'], latency: [200, 2000] },
];

const SOURCE_INFO: Record<string, { process: string; interceptor?: string; ua: string }> = {
  Chrome: { process: 'chrome', interceptor: 'chrome', ua: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36' },
  Firefox: { process: 'firefox', interceptor: 'firefox', ua: 'Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0' },
  Brave: { process: 'brave', interceptor: 'brave', ua: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36' },
  curl: { process: 'curl', interceptor: 'fresh-terminal', ua: 'curl/8.11.0' },
  python3: { process: 'python3', interceptor: 'fresh-terminal', ua: 'python-requests/2.32.3' },
  node: { process: 'node', interceptor: 'fresh-terminal', ua: 'node' },
  'Go-http-client': { process: 'api-gateway', interceptor: 'docker', ua: 'Go-http-client/2.0' },
  okhttp: { process: 'com.acme.app', interceptor: 'android-adb', ua: 'okhttp/4.12.0' },
  Slack: { process: 'slack', interceptor: 'electron', ua: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Slack/4.41.105 Chrome/130.0.0.0 Electron/33.2.0 Safari/537.36' },
};

function defaultPort(scheme: string): number {
  return scheme === 'http' || scheme === 'ws' ? 80 : 443;
}

function authority(scheme: string, host: string, port: number): string {
  return port === defaultPort(scheme) ? host : `${host}:${port}`;
}

function requestHeaders(r: Rng, sc: Scenario, source: string, host: string, reqSpec: B.BodySpec | null, size: number): Headers {
  const info = SOURCE_INFO[source] ?? SOURCE_INFO.curl;
  const ua = info?.ua ?? 'curl/8.11.0';
  const h2 = sc.version === 'HTTP/2';
  const headers: Headers = [];
  if (h2) headers.push([':authority', host], [':method', sc.method], [':scheme', sc.scheme ?? 'https'], [':path', '/']);
  else headers.push(['Host', host]);
  headers.push([h2 ? 'user-agent' : 'User-Agent', ua]);
  const browser = source === 'Chrome' || source === 'Firefox' || source === 'Brave';
  if (browser) {
    headers.push([h2 ? 'accept' : 'Accept', sc.res ? '*/*' : 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8']);
    headers.push([h2 ? 'accept-language' : 'Accept-Language', 'en-US,en;q=0.9,sl;q=0.8']);
    headers.push([h2 ? 'accept-encoding' : 'Accept-Encoding', 'gzip, deflate, br, zstd']);
    headers.push(['sec-fetch-site', r.pick(['same-origin', 'cross-site', 'same-site'])], ['sec-fetch-mode', r.pick(['cors', 'navigate', 'no-cors'])], ['sec-fetch-dest', r.pick(['empty', 'document', 'script', 'image'])]);
    if (source === 'Chrome') headers.push(['sec-ch-ua', '"Chromium";v="131", "Not_A Brand";v="24", "Google Chrome";v="131"'], ['sec-ch-ua-platform', '"Linux"']);
    if (r.chance(0.6)) headers.push([h2 ? 'cookie' : 'Cookie', `_ga=GA1.1.${r.int(1e8, 9e8)}.${r.int(1e9, 2e9)}; session=s%3A${r.int(1e6, 9e6).toString(36)}; theme=dark`]);
    if (sc.host.includes('localhost') || r.chance(0.2)) headers.push([h2 ? 'referer' : 'Referer', `http://localhost:3000/dashboard`]);
  } else {
    headers.push([h2 ? 'accept' : 'Accept', '*/*']);
  }
  if (sc.rule?.id === RULE_IDS.authHeader) headers.push(['Authorization', 'Bearer dev-token-123']);
  else if (sc.host === 'api.github.com' && r.chance(0.5)) headers.push(['authorization', 'Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJhZGEiLCJpYXQiOjE3NjI1OTk2MDAsImV4cCI6MTc2MjYwMzIwMH0.c2lnbmF0dXJl']);
  if (reqSpec) {
    headers.push([h2 ? 'content-type' : 'Content-Type', reqSpec.contentType]);
    headers.push([h2 ? 'content-length' : 'Content-Length', String(size)]);
  }
  if (r.chance(0.3)) headers.push(['x-request-id', `${r.int(1e7, 9e7).toString(16)}-${r.int(1e3, 9e3)}`]);
  if (h2) {
    const path = headers.find(([k]) => k === ':path');
    if (path) path[1] = sc.path(r);
  }
  return headers;
}

function responseHeaders(r: Rng, sc: Scenario, status: number, resSpec: B.BodySpec | null, size: number): Headers {
  const h2 = sc.version === 'HTTP/2';
  const k = (name: string) => (h2 ? name.toLowerCase() : name);
  const headers: Headers = [];
  if (h2) headers.push([':status', String(status)]);
  headers.push([k('Date'), new Date().toUTCString()]);
  if (resSpec) {
    headers.push([k('Content-Type'), resSpec.contentType.includes('json') && !resSpec.contentType.includes('charset') ? `${resSpec.contentType}; charset=utf-8` : resSpec.contentType]);
    if (resSpec.encoding) headers.push([k('Content-Encoding'), resSpec.encoding]);
    headers.push([k('Content-Length'), String(B.wireSizeFor(size, resSpec.encoding))]);
  } else if (status !== 204 && status !== 304 && status !== 101) {
    headers.push([k('Content-Length'), '0']);
  }
  if (sc.host === 'api.github.com') headers.push(['x-ratelimit-limit', '5000'], ['x-ratelimit-remaining', String(r.int(3000, 4999))], ['x-github-request-id', `${r.int(1e4, 9e4).toString(16).toUpperCase()}:${r.int(1e5, 9e5)}`]);
  if (sc.host.includes('cdn') || sc.host.includes('fonts')) headers.push([k('Cache-Control'), 'public, max-age=31536000, immutable'], [k('Age'), String(r.int(10, 90_000))]);
  else headers.push([k('Cache-Control'), r.pick(['no-cache', 'private, max-age=0', 'no-store'])]);
  if (r.chance(0.5)) headers.push([k('ETag'), `W/"${r.int(1e6, 9e6).toString(16)}"`]);
  headers.push([k('Server'), sc.host.includes('localhost') ? 'Vite' : sc.host.includes('google') ? 'gws' : r.pick(['nginx', 'cloudflare', 'GitHub.com', 'AmazonS3'])]);
  if (sc.host === 'localhost' && sc.method === 'POST' && status === 200) headers.push(['Set-Cookie', 'session=s%3Aabc123.xyz; Path=/; HttpOnly; SameSite=Lax; Max-Age=86400']);
  if (r.chance(0.3)) headers.push(['Vary', 'Accept-Encoding, Origin']);
  if (sc.host.includes('localhost')) headers.push(['Access-Control-Allow-Origin', 'http://localhost:3000']);
  if (status === 101) headers.push(['Upgrade', 'websocket'], ['Connection', 'Upgrade'], ['Sec-WebSocket-Accept', 's3pPLMBiTxaQ9kYGzzhZRbK+xOo=']);
  if (status === 206) headers.push(['Content-Range', `bytes 0-${size - 1}/${size * 4}`], ['Accept-Ranges', 'bytes']);
  if (sc.rule?.mocked) headers.push(['x-akev-mock', 'true']);
  if (sc.extraResHeaders) headers.push(...sc.extraResHeaders(r));
  return headers;
}

function sizeOf(spec: B.BodySpec | null, data: string | Uint8Array | undefined): number {
  if (!spec) return 0;
  if (spec.reportedSize) return spec.reportedSize;
  if (data == null) return 0;
  return typeof data === 'string' ? utf8Encode(data).length : data.length;
}

let connectionSeq = 1;

/** Create a fresh exchange starting at `start` (unix ms). */
export function createRecord(id: number, rng: Rng, start: number, options: { scenario?: number; llm?: LlmFixture } = {}): MockRecord {
  const sc = options.llm ? llmScenario(options.llm) : options.scenario != null ? (SCENARIOS[options.scenario] ?? SCENARIOS[0]!) : rng.weighted(SCENARIOS, (s) => s.w);
  const r = rng.fork(id);
  const seed = r.int(1, 2 ** 31 - 1);
  const scheme = sc.scheme ?? 'https';
  const port = sc.port ?? defaultPort(scheme);
  const path = sc.path(r);
  const source = r.pick(sc.sources);
  const info = SOURCE_INFO[source];
  const kind: ExchangeKind = sc.kind ?? 'http';
  const status = typeof sc.status === 'function' ? sc.status(r) : (sc.status ?? 200);
  const reqSpec = sc.req ? sc.req(r) : null;
  const resSpec = sc.res ? sc.res(r, status) : null;
  const bodyRng = r.fork(7);
  const reqData = reqSpec ? reqSpec.make(bodyRng) : undefined;
  const resData = resSpec && !resSpec.reportedSize ? resSpec.make(bodyRng) : undefined;
  const reqSize = sizeOf(reqSpec, reqData);
  const resDecoded = sizeOf(resSpec, resData);
  const host = authority(scheme, sc.host, port);
  const version: HttpVersion = sc.version ?? 'HTTP/1.1';
  const url = kind === 'tls-failure' || kind === 'tunnel' ? `${scheme}://${host}` : `${scheme}://${host}${path}`;
  const latency = r.range(sc.latency[0], sc.latency[1]);
  const ttfb = Math.max(1, latency * r.range(0.55, 0.85));
  const total = latency;
  const fresh = r.chance(0.35);

  const request: RequestHead = {
    method: sc.method,
    url,
    scheme,
    host: sc.host,
    port,
    path: kind === 'tls-failure' || kind === 'tunnel' ? '' : path,
    httpVersion: version,
    headers: kind === 'tls-failure' || kind === 'tunnel' ? [] : requestHeaders(r, sc, source, host, reqSpec, reqSize),
  };
  const resHeaders = kind === 'tls-failure' || kind === 'tunnel' ? [] : responseHeaders(r, sc, status, resSpec, resDecoded);
  const connId = connectionSeq++;
  const tls = scheme === 'https' || scheme === 'wss';
  const client: ClientInfo = {
    remoteAddr: source === 'okhttp' ? `192.168.1.${r.int(20, 60)}:${r.int(40000, 60000)}` : `127.0.0.1:${r.int(40000, 60999)}`,
    connectionId: connId,
    process: info && source !== 'okhttp' ? { pid: r.int(1200, 98000), name: info.process, path: `/usr/bin/${info.process}` } : undefined,
    interceptor: info?.interceptor,
    via: tls ? 'connect' : 'http-proxy',
    tls: tls
      ? {
          sni: sc.host,
          alpnOffered: version === 'HTTP/2' ? ['h2', 'http/1.1'] : ['http/1.1'],
          alpn: version === 'HTTP/2' ? 'h2' : 'http/1.1',
          version: 'TLSv1.3',
          cipher: 'TLS13_AES_128_GCM_SHA256',
          ja3: source === 'Chrome' ? 'cd08e31494f9531f560d64c695473da9' : 'e7d705a3286e19ea42f587b344ee6865',
          ja4: source === 'Chrome' ? 't13d1516h2_8daaf6152771_02713d6af862' : 't13d1715h2_5b57614c22b0_3d5424432f57',
        }
      : undefined,
  };
  const upstream: UpstreamInfo | undefined =
    kind === 'tls-failure' || sc.rule?.mocked
      ? undefined
      : {
          remoteAddr: sc.host === 'localhost' ? `127.0.0.1:${port}` : `${r.int(13, 185)}.${r.int(0, 255)}.${r.int(0, 255)}.${r.int(1, 254)}:${port}`,
          tlsVersion: tls ? 'TLSv1.3' : undefined,
          alpn: tls ? (version === 'HTTP/2' ? 'h2' : 'http/1.1') : undefined,
          cipher: tls ? 'TLS13_AES_256_GCM_SHA384' : undefined,
          fingerprint: tls ? mockUpstreamFingerprint(source, version) : undefined,
          ...(tls ? mockCertificates(r, sc.host, start, sc.host.endsWith('badssl.com')) : {}),
          // A connection that failed to open was never reused.
          reused: !fresh && !(sc.error && CONNECT_ERRORS.has(sc.error.kind)),
        };

  const summary: ExchangeSummary = {
    id,
    kind,
    state: 'requesting',
    startedAt: start,
    method: sc.method,
    scheme,
    host: sc.host,
    port,
    path: request.path,
    httpVersion: version,
    contentKind: 'unknown',
    requestSize: reqSize,
    responseSize: 0,
    source,
    ...(sc.rule ? { rule: sc.rule.id } : {}),
    ...(sc.rule?.mocked ? { mocked: true } : {}),
    ...(sc.rule?.modified ? { modified: true } : {}),
    ...(sc.rule?.color ? { color: sc.rule.color } : {}),
  };

  const timing: Timing = { start: Math.round(start * 1000), requestHeaders: r.range(0.1, 1.2) };
  if (fresh && !sc.rule?.mocked && kind !== 'tls-failure') {
    timing.dnsDuration = sc.host === 'localhost' ? 0.2 : r.range(2, 40);
    timing.connectDuration = r.range(3, 60);
    if (tls) timing.tlsDuration = r.range(10, 90);
  }

  // Keep phases causal: request sent after connection setup, first byte after that.
  const reqDone = Math.min(ttfb * 0.3, r.range(1, 15));
  const setup = (timing.dnsDuration ?? 0) + (timing.connectDuration ?? 0) + (timing.tlsDuration ?? 0);
  const sent = reqDone + setup + 0.4;
  const causalTtfb = Math.max(ttfb, sent + r.range(2, 40));
  const plan: Plan = {
    reqDone,
    sent,
    ttfb: causalTtfb,
    total: total + (causalTtfb - ttfb),
    status: sc.error ? undefined : status,
    resHeaders,
  };
  if (sc.error) {
    plan.error = { kind: sc.error.kind, message: sc.error.message, ...(sc.error.code ? { code: sc.error.code } : {}) };
    plan.abortAt = sc.error.at === 'response' ? total : Math.max(2, ttfb * 0.4);
  }
  if (sc.rule?.pause && r.chance(0.5)) plan.pause = sc.rule.pause;
  if (sc.stream) {
    plan.stream = sc.stream;
    plan.streamUntil = sc.llm ? undefined : start + r.range(8_000, 45_000);
    // LLM APIs answer with headers quickly, then spend their time generating tokens.
    if (sc.llm) plan.ttfb = plan.sent + r.range(140, 420);
  }

  const rec: MockRecord = {
    summary,
    request,
    timing,
    client,
    upstream,
    matchedRules: sc.rule ? [sc.rule.id] : [],
    reqSpec,
    resSpec,
    reqData,
    resData,
    messages: [],
    seed,
    phase: 'requesting',
    nextAt: start + plan.reqDone,
    plan,
  };
  if (sc.rule?.modified) {
    rec.originalRequest = { ...request, headers: request.headers.filter(([k]) => k.toLowerCase() !== 'authorization') };
  }
  if (kind === 'tunnel') {
    rec.tunnel = { bytesSent: 0, bytesReceived: 0, reason: 'Host is on the TLS passthrough list' };
  }
  if (sc.llm) {
    rec.llm = sc.llm;
    rec.summary = { ...rec.summary, llm: llmTag(sc.llm) };
  }
  return rec;
}

function setSummary(rec: MockRecord, patch: Partial<ExchangeSummary>): void {
  rec.summary = { ...rec.summary, ...patch };
}

function pushMessage(rec: MockRecord, rng: Rng, at: number, forced?: Partial<StreamMessageWithPayload>): StreamMessageWithPayload {
  const sse = rec.summary.kind === 'event-stream';
  const sent = !sse && rng.chance(0.4);
  const payload = sse
    ? B.json({ seq: rec.messages.length + 1, cpu: rng.int(1, 99), mem: rng.int(100, 900) })
    : sent
      ? B.json({ type: rng.pick(['subscribe', 'ping', 'typing', 'ack']), channel: rng.pick(['orders', 'presence', 'chat:general']), id: rec.messages.length + 1 })
      : rng.chance(0.85)
        ? `42${B.json(['message', { from: rng.pick(['ada', 'grace', 'linus']), text: rng.pick(['shipping it 🚀', 'LGTM', 'can you check the proxy logs?', 'tests are green']), ts: Math.round(at) }])}`
        : '';
  const binary = !sse && payload === '';
  const bytes = binary ? new Uint8Array([0x82, 0x05, rng.int(0, 255), rng.int(0, 255), rng.int(0, 255), 0x00, 0xff]) : utf8Encode(payload);
  const msg: StreamMessageWithPayload = {
    seq: rec.messages.length,
    direction: sse ? 'received' : sent ? 'sent' : 'received',
    kind: sse ? 'event' : binary ? 'binary' : 'text',
    at: Math.round(at * 1000),
    size: bytes.length,
    ...(sse ? { event: rng.pick(['metrics', 'update']), eventId: String(rec.messages.length + 1) } : {}),
    text: binary ? null : payload,
    base64: binary ? bytesToBase64(bytes) : null,
    ...forced,
  };
  rec.messages.push(msg);
  setSummary(rec, { messageCount: rec.messages.length });
  return msg;
}

export interface AdvanceResult {
  changed: boolean;
  /** New stream messages (without payloads) to broadcast. */
  messages?: StreamMessageWithPayload[];
  /** The exchange just paused at a breakpoint. */
  paused?: boolean;
}

/** Advance one exchange's lifecycle to `now`. */
export function advance(rec: MockRecord, now: number, rng: Rng): AdvanceResult {
  const start = rec.summary.startedAt;
  const p = rec.plan;
  const elapsed = now - start;
  switch (rec.phase) {
    case 'requesting': {
      if (rec.summary.kind === 'tunnel') {
        rec.phase = 'streaming';
        setSummary(rec, { state: 'streaming' });
        rec.nextAt = start + p.total;
        return { changed: true };
      }
      if (p.abortAt != null && p.abortAt <= p.reqDone + 1) return abort(rec, now);
      rec.timing = { ...rec.timing, requestBody: p.reqDone };
      if (p.pause === 'request') {
        rec.phase = 'paused';
        rec.nextAt = null;
        setSummary(rec, { state: 'paused', paused: 'request' });
        return { changed: true, paused: true };
      }
      rec.phase = 'waiting';
      setSummary(rec, { state: 'waiting' });
      rec.nextAt = start + (p.abortAt != null && p.abortAt < p.ttfb ? p.abortAt : p.ttfb);
      return { changed: true };
    }
    case 'waiting': {
      if (p.abortAt != null && elapsed >= p.abortAt && (p.abortAt < p.ttfb || p.status == null)) return abort(rec, now);
      const status = p.status ?? 200;
      rec.response = { status, statusMessage: statusText(status), httpVersion: rec.request.httpVersion, headers: p.resHeaders };
      rec.timing = { ...rec.timing, requestSent: p.sent, responseHeaders: p.ttfb };
      setSummary(rec, {
        state: p.stream ? 'streaming' : 'responding',
        status,
        contentKind: rec.resSpec?.kind ?? (p.stream === 'ws' ? 'unknown' : 'unknown'),
        ...(rec.resSpec ? { contentType: rec.resSpec.contentType.split(';')[0] } : {}),
      });
      if (p.stream) {
        rec.phase = 'streaming';
        rec.nextAt = now + (rec.llm ? rng.range(15, 45) : rng.range(600, 2400));
      } else {
        rec.phase = 'responding';
        rec.nextAt = start + (p.abortAt != null ? p.abortAt : p.total);
      }
      return { changed: true };
    }
    case 'responding': {
      if (p.abortAt != null && elapsed >= p.abortAt) return abort(rec, now);
      const size = rec.resSpec ? B.wireSizeFor(rec.resSpec.reportedSize ?? sizeOf(rec.resSpec, rec.resData), rec.resSpec.encoding) : 0;
      rec.timing = { ...rec.timing, responseBody: p.total };
      rec.phase = 'done';
      rec.nextAt = null;
      setSummary(rec, { state: 'complete', duration: p.total, responseSize: size });
      return { changed: true };
    }
    case 'streaming': {
      if (rec.summary.kind === 'tunnel') {
        rec.tunnel = { bytesSent: rng.int(2_000, 40_000), bytesReceived: rng.int(10_000, 900_000), reason: rec.tunnel?.reason ?? 'TLS passthrough' };
        rec.timing = { ...rec.timing, responseBody: p.total };
        rec.phase = 'done';
        rec.nextAt = null;
        setSummary(rec, { state: 'complete', duration: p.total, requestSize: rec.tunnel.bytesSent, responseSize: rec.tunnel.bytesReceived });
        return { changed: true };
      }
      if (rec.llm?.events) return advanceLlmStream(rec, rec.llm, now, rng);
      if (p.streamUntil != null && now >= p.streamUntil) {
        const closing = rec.summary.kind === 'web-socket' ? [pushMessage(rec, rng, now, { kind: 'close', direction: 'received', text: '1000 Normal closure', base64: null, size: 2 })] : [];
        rec.timing = { ...rec.timing, responseBody: now - start };
        rec.phase = 'done';
        rec.nextAt = null;
        setSummary(rec, { state: 'complete', duration: now - start, responseSize: rec.messages.reduce((a, m) => a + m.size, 0) });
        return { changed: true, messages: closing };
      }
      const batch = [pushMessage(rec, rng, now)];
      if (rng.chance(0.25)) batch.push(pushMessage(rec, rng, now + 3));
      setSummary(rec, { responseSize: rec.messages.reduce((a, m) => a + m.size, 0) });
      rec.nextAt = now + rng.range(500, 2600);
      return { changed: true, messages: batch };
    }
    default:
      return { changed: false };
  }
}

/** Deliver the next SSE event of an LLM fixture (a pause before the first token, then quick chunks). */
function advanceLlmStream(rec: MockRecord, f: LlmFixture, now: number, rng: Rng): AdvanceResult {
  const events = f.events ?? [];
  const start = rec.summary.startedAt;
  const i = rec.messages.length;
  if (i >= events.length) {
    rec.timing = { ...rec.timing, responseBody: now - start };
    rec.phase = 'done';
    rec.nextAt = null;
    setSummary(rec, { state: 'complete', duration: now - start, responseSize: utf8Encode(f.responseBody).length });
    return { changed: true };
  }
  const ev = events[i] as { event?: string; data: string };
  const size = utf8Encode(ev.data).length;
  const msg: StreamMessageWithPayload = { seq: i, direction: 'received', kind: 'event', at: Math.round(now * 1000), size, ...(ev.event ? { event: ev.event } : {}), text: ev.data, base64: null };
  rec.messages.push(msg);
  setSummary(rec, { messageCount: rec.messages.length, responseSize: (rec.summary.responseSize ?? 0) + size + 8 });
  // Time to first token, then chunks at a plausible generation speed (~45–110 tokens/s).
  const first = f.firstTokenEvent ?? 0;
  const output = f.llm.response?.usage?.outputTokens ?? events.length * 3;
  const perChunk = Math.min(400, Math.max(12, ((output / Math.max(1, events.length - first)) / rng.range(45, 110)) * 1000));
  rec.nextAt = now + (i + 1 === first ? rng.range(320, 900) : i + 1 < first ? rng.range(4, 25) : perChunk * rng.range(0.6, 1.4));
  return { changed: true, messages: [msg] };
}

function abort(rec: MockRecord, now: number): AdvanceResult {
  const at = now - rec.summary.startedAt;
  rec.error = rec.plan.error ?? { kind: 'internal', message: 'aborted' };
  rec.timing = { ...rec.timing, aborted: at };
  rec.phase = 'done';
  rec.nextAt = null;
  setSummary(rec, { state: 'aborted', error: rec.error.kind, duration: at });
  return { changed: true };
}

/** Resume a paused exchange (from the breakpoint API). */
export function resume(rec: MockRecord, now: number, outcome: 'continue' | 'respond' | 'close' | 'reset', response?: ResponseHead): void {
  const pausedFor = now - rec.summary.startedAt;
  rec.timing = { ...rec.timing, pausedDuration: (rec.timing.pausedDuration ?? 0) + pausedFor };
  const shift = pausedFor;
  rec.plan = { ...rec.plan, pause: undefined, sent: rec.plan.sent + shift, ttfb: rec.plan.ttfb + shift, total: rec.plan.total + shift };
  if (outcome === 'close' || outcome === 'reset') {
    rec.plan.error = { kind: 'breakpoint-aborted', message: outcome === 'close' ? 'Closed at breakpoint' : 'Reset at breakpoint' };
    abort(rec, now);
    rec.summary = { ...rec.summary, paused: undefined };
    return;
  }
  if (outcome === 'respond' && response) {
    rec.plan.status = response.status;
    rec.plan.resHeaders = response.headers;
    rec.summary = { ...rec.summary, mocked: true };
  }
  rec.phase = 'waiting';
  rec.nextAt = now + 30;
  rec.summary = { ...rec.summary, state: 'waiting', paused: undefined };
}

/** Number of catalog scenarios (for deterministic seeding of special cases). */
export const SCENARIO_COUNT = SCENARIOS.length;

/** Index of the first scenario matching a predicate (used to force examples). */
export function scenarioIndex(pred: (host: string, method: string, kind: ExchangeKind | undefined) => boolean): number {
  return SCENARIOS.findIndex((s) => pred(s.host, s.method, s.kind));
}
