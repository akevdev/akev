/**
 * Static fixtures for the mock backend: interceptors, settings, rules, CA info, codegen
 * targets and filter docs. All values are type-checked against `api/types.ts`.
 */
import { REAL_INTERCEPTORS } from './interceptors';
import { TRANSPARENT_INTERCEPTOR } from './transparentFixtures';

import type { CaInfo, FieldDoc, InterceptorInfo, RuleSet, SessionInfo, Settings, TargetInfo } from '../types';

const DAY = 86_400_000;

export function mockCaInfo(now: number): CaInfo {
  return {
    subject: 'CN=Akev CA, O=Akev',
    sha256Fingerprint: '3F:9A:1C:77:0B:E2:54:9D:A8:6E:11:C4:F0:2B:93:5D:7A:E8:66:0C:B1:4F:D2:39:85:A7:1E:C0:6B:F3:28:94',
    spkiFingerprint: 'q2YdE1x7b8vW1m0yQfTgK9pZtN4cR6oJ3sLhV5uXwA8=',
    subjectHashOld: 'c8750f0d',
    notBefore: new Date(now - 7 * DAY).toISOString(),
    notAfter: new Date(now + 358 * DAY).toISOString(),
    daysRemaining: 358,
    expiry: 'valid',
  };
}

export function mockSettings(): Settings {
  return {
    proxy: {
      port: 8000,
      bindAddress: '127.0.0.1',
      http2: true,
      socks: true,
      tlsPassthrough: ['*.apple.com', 'updates.example.com'],
      ignoreUpstreamCertErrors: [],
      trustedCaPaths: [],
      clientCertificates: [],
      tlsProfile: { profile: 'mirror-client', overrides: [{ host: '*.shop.example', profile: 'chrome' }] },
      upstreamProxy: { mode: 'direct', url: '', noProxy: ['localhost', '127.0.0.1'] },
      dnsOverrides: [{ host: 'api.local.test', address: '127.0.0.1' }],
      dohUrl: null,
      connectTimeoutMs: 15_000,
      responseTimeoutMs: 120_000,
      bodyIdleTimeoutMs: 300_000,
      ruleBodyBudgetBytes: 1024 * 1024 * 1024,
      preserveHeaderCase: true,
      wireguard: { enabled: false, port: 51820 },
      transparent: { enabled: true, port: 8001, mark: 0 },
      http3: { upstream: 'off', listen: false, listenPort: 0 },
      access: { policy: 'private', allow: [], block: [], username: '', password: '', allowLoopbackTargets: false, trustProxyProtocol: false },
    },
    capture: {
      maxBodyBytes: 32 * 1024 * 1024,
      memoryBudgetBytes: 512 * 1024 * 1024,
      maxExchanges: 200_000,
      spillMaxBytes: 4 * 1024 * 1024 * 1024,
      eventQueueBytes: 64 * 1024 * 1024,
      recordTunnels: true,
      recordTlsFailures: true,
      ignoreHosts: ['*.telemetry.mozilla.org'],
      detectProcesses: true,
    },
    sessions: {
      autosave: true,
      autosaveIntervalSecs: 60,
      restoreOnStartup: true,
      autosaveMaxExchanges: 20_000,
      autosaveMaxBytes: 256 * 1024 * 1024,
    },
    updates: { checkAutomatically: true },
    breakpoints: { maxPaused: 100, autoReleaseSecs: 0 },
    ui: {},
  };
}

export const RULE_IDS = {
  flags: '0d9f4c1e-7a2b-4c55-9e1f-1b2c3d4e5f60',
  login: '6a1b2c3d-4e5f-4a7b-8c9d-0e1f2a3b4c5d',
  staging: 'b7c8d9e0-f1a2-4b3c-8d4e-5f6a7b8c9d0e',
  stripe: 'c1d2e3f4-a5b6-4c7d-8e9f-0a1b2c3d4e5f',
  slow: 'd4e5f6a7-b8c9-4d0e-9f1a-2b3c4d5e6f7a',
  analytics: 'e7f8a9b0-c1d2-4e3f-a4b5-c6d7e8f9a0b1',
  authHeader: 'f0a1b2c3-d4e5-4f6a-b7c8-d9e0f1a2b3c4',
  wsPause: 'a3b4c5d6-e7f8-4a9b-8c0d-1e2f3a4b5c6d',
} as const;

export function mockRules(): RuleSet {
  return {
    version: 1,
    items: [
      {
        type: 'group',
        id: '9a8b7c6d-5e4f-4a3b-2c1d-0e9f8a7b6c5d',
        title: 'Local API',
        enabled: true,
        collapsed: false,
        items: [
          {
            type: 'rule',
            id: RULE_IDS.authHeader,
            title: 'Inject dev auth token',
            enabled: true,
            matchers: [{ type: 'host', pattern: { op: 'equals', value: 'localhost', caseSensitive: false } }],
            action: { type: 'transform', request: { setHeaders: [['Authorization', 'Bearer dev-token-123']] } },
          },
          {
            type: 'rule',
            id: RULE_IDS.flags,
            enabled: true,
            matchers: [
              { type: 'method', methods: ['GET'] },
              { type: 'path', pattern: { op: 'starts-with', value: '/api/feature-flags', caseSensitive: false } },
            ],
            action: {
              type: 'mock',
              status: 200,
              headers: [['content-type', 'application/json']],
              body: { type: 'json', value: { newCheckout: true, darkMode: true, betaSearch: false } },
            },
          },
          {
            type: 'rule',
            id: RULE_IDS.login,
            title: 'Pause logins',
            enabled: true,
            matchers: [
              { type: 'method', methods: ['POST'] },
              { type: 'path', pattern: { op: 'equals', value: '/api/login', caseSensitive: false } },
            ],
            action: { type: 'breakpoint', request: true, response: false },
            maxMatches: 3,
          },
          {
            type: 'rule',
            id: RULE_IDS.staging,
            enabled: false,
            matchers: [{ type: 'path', pattern: { op: 'glob', value: '/api/orders/**', caseSensitive: false } }],
            action: { type: 'forward', target: 'https://staging.example.com', preserveHost: false },
          },
        ],
      },
      {
        type: 'rule',
        id: RULE_IDS.stripe,
        title: 'Highlight payments',
        enabled: true,
        matchers: [{ type: 'host', pattern: { op: 'glob', value: '*.stripe.com', caseSensitive: false } }],
        action: { type: 'highlight', color: 'purple', note: 'Payments' },
      },
      {
        type: 'rule',
        id: RULE_IDS.slow,
        enabled: true,
        matchers: [{ type: 'host', pattern: { op: 'equals', value: 'slow.example.com', caseSensitive: false } }],
        action: { type: 'delay', ms: 2500, jitterMs: 500 },
      },
      {
        type: 'rule',
        id: RULE_IDS.analytics,
        title: 'Block analytics',
        enabled: false,
        matchers: [
          {
            type: 'any',
            matchers: [
              { type: 'host', pattern: { op: 'ends-with', value: 'google-analytics.com', caseSensitive: false } },
              { type: 'host', pattern: { op: 'ends-with', value: 'segment.io', caseSensitive: false } },
            ],
          },
        ],
        action: { type: 'close' },
      },
      {
        type: 'rule',
        id: RULE_IDS.wsPause,
        title: 'Pause chat subscriptions',
        enabled: true,
        matchers: [{ type: 'host', pattern: { op: 'equals', value: 'localhost', caseSensitive: false } }],
        action: { type: 'ws-breakpoint', direction: 'sent', pattern: { op: 'contains', value: '"subscribe"', caseSensitive: false } },
      },
    ],
  };
}

export function mockInterceptors(_platform: 'linux' | 'macos' | 'windows'): InterceptorInfo[] {
  // Mirrors the real daemon exactly (ids, names, categories, metadata shapes).
  return structuredClone([...REAL_INTERCEPTORS, TRANSPARENT_INTERCEPTOR]);
}

export const CODEGEN_TARGETS: TargetInfo[] = [
  { id: 'curl', name: 'cURL', language: 'shell', group: 'Shell' },
  { id: 'curl-windows', name: 'cURL (Windows cmd)', language: 'shell', group: 'Shell' },
  { id: 'wget', name: 'Wget', language: 'shell', group: 'Shell' },
  { id: 'httpie', name: 'HTTPie', language: 'shell', group: 'Shell' },
  { id: 'powershell', name: 'PowerShell', language: 'powershell', group: 'Shell' },
  { id: 'js-fetch', name: 'fetch', language: 'javascript', group: 'JavaScript' },
  { id: 'node-fetch', name: 'Node.js · fetch', language: 'javascript', group: 'JavaScript' },
  { id: 'node-axios', name: 'Node.js · axios', language: 'javascript', group: 'JavaScript' },
  { id: 'python-requests', name: 'Python · requests', language: 'python', group: 'Python' },
  { id: 'python-httpx', name: 'Python · httpx', language: 'python', group: 'Python' },
  { id: 'go-net-http', name: 'Go · net/http', language: 'go', group: 'Go' },
  { id: 'rust-reqwest', name: 'Rust · reqwest', language: 'rust', group: 'Rust' },
  { id: 'java-http-client', name: 'Java · HttpClient', language: 'java', group: 'JVM' },
  { id: 'kotlin-okhttp', name: 'Kotlin · OkHttp', language: 'kotlin', group: 'JVM' },
  { id: 'csharp-http-client', name: 'C# · HttpClient', language: 'csharp', group: '.NET' },
  { id: 'php-curl', name: 'PHP · cURL', language: 'php', group: 'PHP' },
  { id: 'ruby-net-http', name: 'Ruby · Net::HTTP', language: 'ruby', group: 'Ruby' },
  { id: 'swift-url-session', name: 'Swift · URLSession', language: 'swift', group: 'Swift' },
  { id: 'dart-http', name: 'Dart · http', language: 'dart', group: 'Dart' },
  { id: 'raw-http', name: 'Raw HTTP', language: 'http', group: 'Other' },
];

export const FILTER_DOCS: FieldDoc[] = [
  { name: 'method', aliases: [], description: 'Request method', examples: ['method:post', 'method:get,head'] },
  { name: 'status', aliases: [], description: 'Response status code (supports 4xx, ranges and comparisons)', examples: ['status:404', 'status:4xx', 'status:>=500', 'status:200-299'] },
  { name: 'host', aliases: ['domain'], description: 'Hostname (contains; * for globs)', examples: ['host:github.com', 'host:*.example.com'] },
  { name: 'port', aliases: [], description: 'Destination port', examples: ['port:3000'] },
  { name: 'path', aliases: [], description: 'Path without the query string', examples: ['path:/api/', 'path^=/v1'] },
  { name: 'url', aliases: [], description: 'Full URL', examples: ['url:"?page=2"'] },
  { name: 'query', aliases: [], description: 'Query string', examples: ['query:token'] },
  { name: 'scheme', aliases: ['protocol'], description: 'URL scheme', examples: ['scheme:https', 'scheme:ws,wss'] },
  { name: 'version', aliases: [], description: 'HTTP version', examples: ['version:h2', 'version:h1'] },
  { name: 'type', aliases: ['content-type'], description: 'Content kind (json, html, image…) or MIME substring', examples: ['type:json', 'type:image', 'type:protobuf'] },
  { name: 'size', aliases: [], description: 'Response size (b, kb, mb, gb)', examples: ['size:>1mb', 'size:<100b'] },
  { name: 'reqsize', aliases: [], description: 'Request body size', examples: ['reqsize:>10kb'] },
  { name: 'duration', aliases: ['time'], description: 'Total duration (ms or s)', examples: ['duration:>1s', 'time:<50ms'] },
  { name: 'header', aliases: [], description: 'Header present (or header.<name>:value)', examples: ['header:x-trace-id', 'header.content-encoding:br'] },
  { name: 'reqheader', aliases: [], description: 'Request header', examples: ['reqheader:authorization'] },
  { name: 'resheader', aliases: [], description: 'Response header', examples: ['resheader:set-cookie'] },
  { name: 'cookie', aliases: [], description: 'Cookie name', examples: ['cookie:session'] },
  { name: 'body', aliases: [], description: 'Request or response body text', examples: ['body:"error"'] },
  { name: 'reqbody', aliases: [], description: 'Request body text', examples: ['reqbody:password'] },
  { name: 'resbody', aliases: [], description: 'Response body text', examples: ['resbody:"not found"'] },
  { name: 'source', aliases: [], description: 'Client process or interceptor', examples: ['source:chrome', 'source:curl'] },
  { name: 'rule', aliases: [], description: 'Rule id or title that handled the exchange', examples: ['rule:"Pause logins"'] },
  { name: 'id', aliases: [], description: 'Exchange id', examples: ['id:>1000'] },
  { name: 'note', aliases: [], description: 'User note text', examples: ['note:bug'] },
  { name: 'color', aliases: [], description: 'Highlight colour', examples: ['color:red'] },
  { name: 'error', aliases: [], description: 'Error kind', examples: ['error:timeout', 'error:connection-refused'] },
  { name: 'ip', aliases: [], description: 'Client IP address', examples: ['ip:192.168.'] },
  { name: 'model', aliases: ['llm-model'], description: 'Model of an LLM API call (contains; * for globs)', examples: ['model:gpt-4.1', 'model:claude*', 'is:llm -model:mini'] },
  { name: 'provider', aliases: ['vendor'], description: 'LLM provider: openai, anthropic, google, vertex-ai, azure-openai, openrouter, ollama, lmstudio, openai-compatible…', examples: ['provider:anthropic', 'provider:openai,azure-openai'] },
  { name: 'is', aliases: [], description: 'Flags: pending, complete, error, aborted, paused, pinned, mocked, modified, websocket/ws, sse, tls-failure, tunnel, http, https, highlighted, llm', examples: ['is:error', 'is:pinned', '-is:ws', 'is:llm'] },
  { name: 'has', aliases: [], description: 'Presence: note, body, reqbody, resbody, cookie, auth', examples: ['has:auth', 'has:note'] },
];

/** Seed sessions of the mock session library (`/api/sessions`), relative to `now`. */
export function mockSessions(now: number): SessionInfo[] {
  const min = 60_000;
  return [
    { id: 'autosave', name: 'Autosave', kind: 'autosave', exchanges: 1234, size: 4_812_330, createdAt: now - 3 * 24 * 60 * min, updatedAt: now - 2 * min },
    { id: 'checkout-bug', name: 'Checkout bug — total off by one', kind: 'saved', exchanges: 87, size: 312_904, createdAt: now - 26 * 60 * min, updatedAt: now - 26 * 60 * min },
    { id: 'autosave-previous', name: 'Autosave (previous run)', kind: 'autosave', exchanges: 9_802, size: 41_200_118, createdAt: now - 5 * 24 * 60 * min, updatedAt: now - 3 * 24 * 60 * min },
    { id: 'android-app-login', name: 'Android app login flow', kind: 'saved', exchanges: 342, size: 1_904_220, createdAt: now - 9 * 24 * 60 * min, updatedAt: now - 9 * 24 * 60 * min },
  ];
}
