/**
 * TypeScript mirrors of the Akev wire types.
 *
 * Source of truth: `crates/akev-core/src/*.rs` (serde: camelCase fields, kebab-case enum
 * tags) and the HTTP API reference (payload shapes, plus a few types owned by other crates:
 * `CaInfo` from akev-ca, `BodyView` from akev-content/akev-api, `SendOptions`/`SendResult`
 * from akev-proxy, `TargetInfo` from akev-codegen, `Suggestion`/`FieldDoc` from akev-filter).
 *
 * Conventions:
 * - Rust `Option<T>` fields with `skip_serializing_if` are optional (`field?: T`); `Option<T>`
 *   fields serialized without skipping are `T | null`.
 * - `Headers` keeps order, duplicates and casing: `[name, value][]`.
 * - Rust `u64` ids/sizes are plain JS numbers (safe up to 2^53).
 * - `UnixMicros` is microseconds since the epoch; `ExchangeSummary.startedAt` is unix **ms**.
 */

// =====================================================================================
// ids & time (akev-core ids.rs, time.rs)
// =====================================================================================

/** Monotonic per-daemon exchange id; sorting by id == sorting by start order. */
export type ExchangeId = number;
/** Client TCP connection id. */
export type ConnectionId = number;
/** Rule / group id (UUID string). */
export type RuleId = string;
/** Microseconds since the Unix epoch. */
export type UnixMicros = number;

// =====================================================================================
// http.rs
// =====================================================================================

export type HttpVersion = 'HTTP/0.9' | 'HTTP/1.0' | 'HTTP/1.1' | 'HTTP/2' | 'HTTP/3';

/** Ordered multi-map of headers preserving duplicates, order and original casing. */
export type Headers = [name: string, value: string][];

export interface Header {
  name: string;
  value: string;
}

export interface RequestHead {
  method: string;
  /** Absolute URL as the client intended it, e.g. `https://example.com:8443/a?b=c`. */
  url: string;
  /** `http` / `https` / `ws` / `wss`. */
  scheme: string;
  /** Hostname (no port), lowercase, IPv6 without brackets. */
  host: string;
  port: number;
  /** Path including query string, as sent on the wire. */
  path: string;
  httpVersion: HttpVersion;
  headers: Headers;
}

export interface ResponseHead {
  status: number;
  /** Reason phrase as sent (HTTP/1), or the canonical one for HTTP/2. */
  statusMessage: string;
  httpVersion: HttpVersion;
  headers: Headers;
}

// =====================================================================================
// body.rs
// =====================================================================================

export type Side = 'request' | 'response';

export interface BodyMeta {
  /** Bytes on the wire (encoded). */
  wireSize: number;
  /** Bytes actually stored (may be < wireSize when truncated). */
  capturedSize: number;
  truncated: boolean;
  /** Content-Encoding chain, e.g. `["gzip"]`. */
  encodings: string[];
  /** Size after decoding, when known. */
  decodedSize?: number;
  /** Content-Type essence (lowercase, no params). */
  contentType?: string;
  /** The bytes were discarded to respect a limit (spill disk cap, overloaded capture); only the sizes remain. */
  dropped?: boolean;
}

/** Kind of content a body holds; drives the default viewer. */
export type ContentKind =
  | 'json'
  | 'json-lines'
  | 'xml'
  | 'html'
  | 'javascript'
  | 'css'
  | 'markdown'
  | 'yaml'
  | 'csv'
  | 'text'
  | 'form-url-encoded'
  | 'multipart'
  | 'image'
  | 'audio'
  | 'video'
  | 'font'
  | 'pdf'
  | 'wasm'
  | 'protobuf'
  | 'grpc'
  | 'grpc-web'
  | 'msg-pack'
  | 'cbor'
  | 'event-stream'
  | 'graph-ql'
  | 'binary'
  | 'unknown';

/** Every {@link ContentKind}, in declaration order. */
export const CONTENT_KINDS: readonly ContentKind[] = [
  'json',
  'json-lines',
  'xml',
  'html',
  'javascript',
  'css',
  'markdown',
  'yaml',
  'csv',
  'text',
  'form-url-encoded',
  'multipart',
  'image',
  'audio',
  'video',
  'font',
  'pdf',
  'wasm',
  'protobuf',
  'grpc',
  'grpc-web',
  'msg-pack',
  'cbor',
  'event-stream',
  'graph-ql',
  'binary',
  'unknown',
];

// =====================================================================================
// exchange.rs
// =====================================================================================

export type ExchangeKind = 'http' | 'web-socket' | 'event-stream' | 'tls-failure' | 'tunnel' | 'client-error';

export type ExchangeState =
  | 'requesting'
  | 'waiting'
  | 'responding'
  | 'paused'
  | 'complete'
  | 'aborted'
  | 'streaming';

export type BreakpointStage = 'request' | 'response';

export type ErrorKind =
  | 'client-aborted'
  | 'dns-failed'
  | 'connection-refused'
  | 'connection-reset'
  | 'host-unreachable'
  | 'timeout'
  | 'upstream-cert-invalid'
  | 'upstream-tls-failed'
  | 'upstream-protocol-error'
  | 'client-protocol-error'
  | 'client-tls-rejected'
  | 'rule-closed'
  | 'rule-reset'
  | 'rule-timeout'
  | 'breakpoint-aborted'
  | 'upstream-proxy-failed'
  | 'internal';

export interface ExchangeError {
  kind: ErrorKind;
  /** Human readable message from the underlying error. */
  message: string;
  /** Raw error code (e.g. `ECONNRESET`, a TLS alert name). */
  code?: string;
}

/** Fine-grained timings: `start` is absolute (µs); everything else is ms since `start`. */
export interface Timing {
  start: UnixMicros;
  requestHeaders?: number;
  requestBody?: number;
  dnsDuration?: number;
  connectDuration?: number;
  tlsDuration?: number;
  requestSent?: number;
  responseHeaders?: number;
  responseBody?: number;
  aborted?: number;
  /** Total time paused at breakpoints (ms). */
  pausedDuration?: number;
}

export interface TlsInfo {
  sni?: string;
  alpnOffered?: string[];
  alpn?: string;
  version?: string;
  cipher?: string;
  ja3?: string;
  ja4?: string;
}

export interface CertificateSummary {
  subject: string;
  issuer: string;
  san: string[];
  notBefore: string;
  notAfter: string;
  serial: string;
  sha256Fingerprint: string;
}

export interface UpstreamInfo {
  remoteAddr?: string;
  tlsVersion?: string;
  alpn?: string;
  cipher?: string;
  /** The TLS ClientHello (and HTTP/2 preface) fingerprint Akev presented upstream. */
  fingerprint?: UpstreamFingerprint;
  certificate?: CertificateSummary;
  /** The whole chain the server presented and what Akev's trust decision rested on. */
  chain?: CertificateChain;
  reused: boolean;
  viaProxy?: string;
}

/** akev-core `CertificateChain`: the certificates in the order the server sent them (leaf first). */
export interface CertificateChain {
  certificates: CertificateSummary[];
  trust: ChainTrust;
}

/** akev-core `ChainTrust`. */
export interface ChainTrust {
  /** `not-verified`: certificate errors are ignored for this host or request. `unknown`: verified, but the path couldn't be rebuilt. */
  status: 'verified' | 'not-verified' | 'unknown';
  /** Indices into `certificates` of the verified path, leaf first. */
  path?: number[];
  /** The trusted root the verified path ends at. */
  anchor?: TrustAnchorInfo;
}

/** akev-core `TrustAnchorInfo`. */
export interface TrustAnchorInfo {
  subject: string;
  /** Index into `certificates` when the server sent the root itself. */
  index?: number;
  /** `built-in`: Mozilla's roots shipped with Akev; `system`: the OS trust store; `custom`: a trusted CA file from the settings. */
  source: 'built-in' | 'system' | 'custom';
}

/** akev-core `UpstreamFingerprint`: what Akev's own upstream ClientHello looked like. */
export interface UpstreamFingerprint {
  /** Profile actually used: `native`, `chrome`, `firefox`, `safari` or `edge`. */
  profile: string;
  /** Profile asked for, when it differs (`mirror-client`, or a browser in a build without impersonation). */
  requested?: string;
  /** Why `requested` wasn't used as is. */
  note?: string;
  /** JA3 hash (md5 hex). */
  ja3?: string;
  /** Full JA3 string. */
  ja3Text?: string;
  ja4?: string;
  /** Akamai HTTP/2 fingerprint (`SETTINGS|WINDOW_UPDATE|PRIORITY|pseudo-header order`), HTTP/2 only. */
  http2?: string;
}

export interface ProcessInfo {
  pid: number;
  name: string;
  path?: string;
}

export type ProxyMode = 'http-proxy' | 'connect' | 'socks' | 'direct' | 'composer' | 'wireguard' | 'transparent';

export interface ClientInfo {
  /** Client socket address `ip:port`. */
  remoteAddr: string;
  connectionId: ConnectionId;
  process?: ProcessInfo;
  /** Interceptor that launched/configured the client (`chrome`, `terminal`, ...). */
  interceptor?: string;
  via: ProxyMode;
  tls?: TlsInfo;
}

export type Direction = 'sent' | 'received';
export type MessageKind = 'text' | 'binary' | 'ping' | 'pong' | 'close' | 'event';

/** A WebSocket or SSE message (without payload). */
export interface StreamMessage {
  seq: number;
  direction: Direction;
  kind: MessageKind;
  at: UnixMicros;
  size: number;
  event?: string;
  eventId?: string;
  modified?: boolean;
}

/** Compact row model for the traffic list. */
export interface ExchangeSummary {
  id: ExchangeId;
  kind: ExchangeKind;
  state: ExchangeState;
  /** Start time (unix ms, fractional). */
  startedAt: number;
  method: string;
  scheme: string;
  host: string;
  port: number;
  /** Path + query. */
  path: string;
  httpVersion: HttpVersion;
  status?: number;
  /** Response content kind, for the icon/colour. */
  contentKind: ContentKind;
  contentType?: string;
  requestSize: number;
  responseSize: number;
  /** Total duration in ms when finished. */
  duration?: number;
  error?: ErrorKind;
  paused?: BreakpointStage;
  /** Display name of the client source (process name or interceptor). */
  source?: string;
  rule?: RuleId;
  modified?: boolean;
  mocked?: boolean;
  pinned?: boolean;
  /** User colour tag. */
  color?: TagColor;
  messageCount?: number;
  hasNote?: boolean;
  /** Set when the exchange is a recognised LLM API call (provider + requested model). */
  llm?: LlmTag;
}

/** Provider and model of an LLM API call (`ExchangeSummary.llm`). */
export interface LlmTag {
  /** `openai`, `anthropic`, `google`, `vertex-ai`, `azure-openai`, `ollama`, `openai-compatible`… */
  provider: string;
  model?: string;
}

/** User colour tags accepted by `ExchangeSummary.color` and highlight rules. */
export type TagColor = 'red' | 'orange' | 'yellow' | 'green' | 'blue' | 'purple' | 'grey';
export const TAG_COLORS: readonly TagColor[] = ['red', 'orange', 'yellow', 'green', 'blue', 'purple', 'grey'];

export interface TunnelStats {
  bytesSent: number;
  bytesReceived: number;
  reason: string;
}

/** Full detail for one exchange (everything except body bytes and message payloads). */
export interface ExchangeDetail {
  summary: ExchangeSummary;
  request: RequestHead;
  requestBody: BodyMeta;
  requestTrailers?: Headers;
  response?: ResponseHead;
  responseBody: BodyMeta;
  responseTrailers?: Headers;
  originalRequest?: RequestHead;
  originalResponse?: ResponseHead;
  timing: Timing;
  client: ClientInfo;
  upstream?: UpstreamInfo;
  error?: ExchangeError;
  tunnel?: TunnelStats;
  note?: string;
  matchedRules?: RuleId[];
  /**
   * Why Akev handled this exchange differently from usual: a limit was reached (body too large
   * for rules, too many paused exchanges), capture was overloaded, or a breakpoint released itself.
   */
  notices?: string[];
}

// =====================================================================================
// rules.rs
// =====================================================================================

export const RULESET_VERSION = 1;

export interface RuleSet {
  version: number;
  items: RuleItem[];
}

export type RuleItem = ({ type: 'rule' } & Rule) | ({ type: 'group' } & RuleGroup);

export interface RuleGroup {
  id: RuleId;
  title: string;
  enabled: boolean;
  collapsed: boolean;
  items: RuleItem[];
}

export interface Rule {
  id: RuleId;
  /** Optional title; the UI auto-describes the rule when absent. */
  title?: string;
  enabled: boolean;
  /** All must match (AND). Empty = every request. */
  matchers: Matcher[];
  action: Action;
  /** Stop matching after this many matches. Absent = unlimited. */
  maxMatches?: number;
  note?: string;
}

export type MatchOp = 'equals' | 'contains' | 'starts-with' | 'ends-with' | 'glob' | 'regex' | 'exists';

export interface StringMatch {
  op: MatchOp;
  value: string;
  caseSensitive: boolean;
  negate?: boolean;
}

export interface KeyValueMatch {
  key: string;
  /** Absent = key merely present. */
  value?: StringMatch;
}

export type Matcher =
  | { type: 'method'; methods: string[] }
  | { type: 'url'; pattern: StringMatch }
  | { type: 'host'; pattern: StringMatch }
  | { type: 'port'; ports: number[] }
  | { type: 'path'; pattern: StringMatch }
  | { type: 'protocol'; protocols: string[] }
  | { type: 'query'; params: KeyValueMatch[] }
  | { type: 'header'; headers: KeyValueMatch[] }
  | { type: 'cookie'; cookies: KeyValueMatch[] }
  | { type: 'body'; pattern: StringMatch }
  | { type: 'json-body'; value: unknown; partial: boolean }
  | { type: 'form'; fields: KeyValueMatch[] }
  | { type: 'graphql-operation'; pattern: StringMatch }
  | { type: 'source'; pattern: StringMatch }
  | { type: 'http-version'; versions: string[] }
  | { type: 'not'; matcher: Matcher }
  | { type: 'any'; matchers: Matcher[] }
  | { type: 'all'; matchers: Matcher[] };

export type MatcherType = Matcher['type'];

export type BodySource =
  | { type: 'empty' }
  | { type: 'text'; text: string }
  | { type: 'base64'; data: string }
  | { type: 'json'; value: unknown }
  | { type: 'file'; path: string };

export interface MatchReplace {
  find: string;
  replace: string;
  regex: boolean;
  caseSensitive: boolean;
}

export type BodyTransform =
  | { type: 'replace'; body: BodySource }
  | { type: 'match-replace'; replacements: MatchReplace[] }
  | { type: 'json-merge'; patch: unknown }
  | { type: 'json-patch'; operations: unknown[] };

export interface RequestTransform {
  method?: string;
  urlReplace?: MatchReplace[];
  /** `null` value removes the param. */
  setQuery?: [name: string, value: string | null][];
  setHeaders?: Headers;
  addHeaders?: Headers;
  removeHeaders?: string[];
  body?: BodyTransform;
}

export interface ResponseTransform {
  status?: number;
  statusMessage?: string;
  setHeaders?: Headers;
  addHeaders?: Headers;
  removeHeaders?: string[];
  body?: BodyTransform;
}

export type Action =
  // terminal
  | { type: 'passthrough'; request?: RequestTransform; response?: ResponseTransform }
  | {
      type: 'forward';
      target: string;
      stripPrefix?: string;
      preserveHost: boolean;
      request?: RequestTransform;
      response?: ResponseTransform;
    }
  | { type: 'mock'; status: number; statusMessage?: string; headers: Headers; body: BodySource }
  | {
      type: 'map-local';
      path: string;
      stripPrefix?: string;
      index?: string;
      fallbackToUpstream: boolean;
      headers: Headers;
    }
  | { type: 'close' }
  | { type: 'reset' }
  | { type: 'timeout' }
  | { type: 'ws-echo' }
  | { type: 'ws-reject'; status: number; body: BodySource }
  // modifiers
  | { type: 'transform'; request?: RequestTransform; response?: ResponseTransform }
  | { type: 'breakpoint'; request: boolean; response: boolean }
  | { type: 'delay'; ms: number; jitterMs: number }
  | { type: 'throttle'; downloadBps: number; uploadBps: number; latencyMs: number }
  | { type: 'highlight'; color: string; note?: string }
  | { type: 'script'; source: string }
  /** Present a browser's TLS / HTTP/2 fingerprint upstream (overrides `proxy.tlsProfile`). */
  | { type: 'tls-profile'; profile: TlsProfile }
  /** Pause matching WebSocket messages (only WebSocket upgrades match such rules). */
  | { type: 'ws-breakpoint'; direction: WsBreakpointDirection; pattern?: StringMatch };

/** Which WebSocket messages a `ws-breakpoint` rule pauses. */
export type WsBreakpointDirection = 'sent' | 'received' | 'both';

export type ActionType = Action['type'];

/** Action types that stop rule evaluation (mirrors `Action::is_terminal`). */
export const TERMINAL_ACTIONS: ReadonlySet<ActionType> = new Set<ActionType>([
  'passthrough',
  'forward',
  'mock',
  'map-local',
  'close',
  'reset',
  'timeout',
  'ws-echo',
  'ws-reject',
]);

export interface RuleStats {
  ruleId: RuleId;
  matches: number;
  /** Unix ms of the last match. */
  lastMatchedAt?: number;
}

// =====================================================================================
// breakpoint.rs
// =====================================================================================

/** Editable request (body is decoded, base64-encoded for transport). */
export interface EditableRequest {
  method: string;
  url: string;
  headers: Headers;
  /** Base64. */
  body: string;
}

export interface EditableResponse {
  status: number;
  statusMessage: string;
  headers: Headers;
  /** Base64. */
  body: string;
}

export type RequestResume =
  | { action: 'continue'; request: EditableRequest }
  | { action: 'respond'; response: EditableResponse }
  | { action: 'close' }
  | { action: 'reset' };

export type ResponseResume =
  | { action: 'continue'; response: EditableResponse }
  | { action: 'close' }
  | { action: 'reset' };

export interface PausedExchange {
  id: ExchangeId;
  stage: BreakpointStage;
  /** When it started waiting (unix µs). */
  pausedAt: number;
  request: EditableRequest;
  response?: EditableResponse;
}

/** A WebSocket message held at a `ws-breakpoint` (only its direction waits). */
export interface PausedMessage {
  /** The WebSocket exchange. */
  id: ExchangeId;
  /** Resume with this id (`POST /api/breakpoints/messages/:pauseId/resume`). */
  pauseId: number;
  direction: Direction;
  kind: 'text' | 'binary';
  /** Complete payload, base64. */
  payload: string;
  /** Unix µs. */
  at: number;
  /** The `ws-breakpoint` rule that paused it. */
  rule: RuleId;
}

/** A WebSocket message payload in API requests: `text` (UTF-8) or `base64`. */
export interface WsMessageBody {
  kind?: 'text' | 'binary';
  text?: string;
  base64?: string;
}

/** `POST /api/breakpoints/messages/:pauseId/resume` (forward without a payload = unchanged). */
export type MessageResume = ({ action: 'forward' } & WsMessageBody) | { action: 'drop' };

/** `POST /api/exchanges/:id/messages` — `sent` = to the server, `received` = to the client. */
export type InjectMessageRequest = { direction: Direction } & WsMessageBody;

/** `seq` of the recorded message; `null` while it is still queued behind a long message. */
export interface InjectMessageResult {
  seq: number | null;
}

// =====================================================================================
// interceptor.rs
// =====================================================================================

export type InterceptorCategory = 'browser' | 'terminal' | 'runtime' | 'mobile' | 'container' | 'system' | 'manual';

export interface InterceptorInfo {
  /** Stable id: `chrome`, `firefox`, `fresh-terminal`, `existing-terminal`, `android-adb`, ... */
  id: string;
  name: string;
  description: string;
  category: InterceptorCategory;
  tags: string[];
  /** Icon key (`chrome`, `firefox`, `terminal`, `android`, `docker`, ...). */
  icon: string;
  supported: boolean;
  available: boolean;
  active: boolean;
  unavailableReason?: string;
  version?: string;
  /** Interceptor-specific metadata (adb devices, docker containers, JVMs...). */
  metadata?: unknown;
  needsConfig: boolean;
}

export interface ActivationResult {
  success: boolean;
  message?: string;
  metadata?: unknown;
}

// =====================================================================================
// settings.rs
// =====================================================================================

export interface Settings {
  proxy: ProxySettings;
  capture: CaptureSettings;
  /** Saved sessions & autosave. */
  sessions: SessionSettings;
  /** New-version checks. */
  updates: UpdateSettings;
  /** Limits on paused exchanges. */
  breakpoints: BreakpointSettings;
  /** Opaque UI preferences, owned by the UI (`state/ui.ts`). */
  ui: unknown;
}

export interface ProxySettings {
  port: number;
  bindAddress: string;
  http2: boolean;
  socks: boolean;
  tlsPassthrough: string[];
  ignoreUpstreamCertErrors: string[];
  trustedCaPaths: string[];
  clientCertificates: ClientCertificate[];
  /** Upstream TLS fingerprint. */
  tlsProfile: TlsProfileSettings;
  upstreamProxy: UpstreamProxy;
  dnsOverrides: DnsOverride[];
  /** DNS-over-HTTPS URL, `null` = system resolver. */
  dohUrl: string | null;
  connectTimeoutMs: number;
  /** How long to wait for the response headers (ms, 0 = no limit). */
  responseTimeoutMs: number;
  /** How long a response body may go without data before the exchange fails (ms, 0 = no limit). */
  bodyIdleTimeoutMs: number;
  /** Memory shared by all exchanges whose bodies are held in full for rules, scripts or breakpoints. */
  ruleBodyBudgetBytes: number;
  preserveHeaderCase: boolean;
  /** WireGuard mode. */
  wireguard: WireguardSettings;
  /** Transparent capture listener. */
  transparent: TransparentSettings;
  /** HTTP/3 over QUIC. */
  http3: Http3Settings;
  /** Who may use the proxy. */
  access: ProxyAccessSettings;
}

/** `settings.proxy.access`. */
export interface ProxyAccessSettings {
  /** `private`: this computer and private networks; `allowlist`: this computer and `allow`; `any`. */
  policy: 'private' | 'allowlist' | 'any';
  /** IP addresses / CIDR ranges for the `allowlist` policy. */
  allow: string[];
  /** IP addresses / CIDR ranges always refused (never this computer). */
  block: string[];
  /** Proxy username; empty = no credentials. Devices on other machines must log in when set. */
  username: string;
  password: string;
  /** Let devices on other machines reach this computer's localhost through the proxy. */
  allowLoopbackTargets: boolean;
  /** Honour PROXY protocol headers (a trusted load balancer in front of Akev). */
  trustProxyProtocol: boolean;
}

/** `settings.proxy.wireguard`. */
export interface WireguardSettings {
  enabled: boolean;
  /** UDP port. */
  port: number;
  /** UDP bind address; absent = all interfaces. */
  bindAddress?: string | null;
}

/** The WireGuard client as seen by Akev (`GET /api/wireguard` → `peer`). */
export interface WireguardPeer {
  publicKey: string;
  /** Where the device's packets come from (`ip:port`). */
  endpoint?: string;
  /** Unix ms of the current session's handshake. */
  lastHandshakeAt?: number;
  /** Unix ms of the last authenticated packet. */
  lastSeenAt?: number;
  rxBytes: number;
  txBytes: number;
  /** A session younger than WireGuard's 3-minute lifetime exists. */
  connected: boolean;
}

/** Counters of the running WireGuard server. */
export interface WireguardStats {
  listenAddr: string;
  peer: WireguardPeer;
  tcpActive: number;
  tcpTotal: number;
  tcpRejected: number;
  dnsQueries: number;
  udpFlows: number;
  udpRelayed: number;
  quicDropped: number;
  droppedPackets: number;
}

/** `GET /api/wireguard`. */
export interface WireguardInfo {
  enabled: boolean;
  running: boolean;
  /** UDP port written into the configuration. */
  port: number;
  bindAddress?: string;
  listenAddr?: string;
  /** Why the server isn't running although enabled (e.g. port in use). */
  error?: string;
  /** LAN addresses devices can reach; the first is the default endpoint host. */
  endpoints: string[];
  /** `host:port` in `config`. */
  endpoint: string;
  /** The client configuration (`.conf`). */
  config: string;
  /** What the QR code encodes (the configuration). */
  qrPayload: string;
  configFileName: string;
  serverPublicKey: string;
  clientAddress: string;
  dnsAddress: string;
  keysCreatedAt: number;
  peer: WireguardPeer;
  stats?: WireguardStats;
}

/** `PUT /api/wireguard`: fields of `settings.proxy.wireguard` to change. */
export interface WireguardPatch {
  enabled?: boolean;
  port?: number;
  /** `null` / `""` = all interfaces. */
  bindAddress?: string | null;
}

/** When Akev talks HTTP/3 to servers: never, to origins that advertised it with `Alt-Svc`
 *  (cached for its max-age, like browsers), or first for every HTTPS origin. A failed QUIC
 *  connection always falls back to TCP. */
export type Http3UpstreamMode = 'off' | 'alt-svc' | 'prefer';

/** `ProxySettings.http3`. */
export interface Http3Settings {
  upstream: Http3UpstreamMode;
  /** Accept HTTP/3 from clients on a QUIC (UDP) listener. */
  listen: boolean;
  /** UDP port of that listener; 0 = the proxy's port number. */
  listenPort: number;
}

export interface ClientCertificate {
  host: string;
  path: string;
  passphrase?: string;
}

/** Which TLS ClientHello / HTTP/2 preface Akev presents upstream. */
export type TlsProfile = 'native' | 'chrome' | 'firefox' | 'safari' | 'edge' | 'mirror-client';

/** `proxy.tlsProfile`. */
export interface TlsProfileSettings {
  profile: TlsProfile;
  /** Per-host overrides; the first matching host glob wins (rules win over both). */
  overrides: TlsProfileOverride[];
}

export interface TlsProfileOverride {
  host: string;
  profile: TlsProfile;
}

/** One entry of `GET /api/tls-profiles`. */
export interface TlsProfileInfo {
  id: TlsProfile;
  label: string;
  description: string;
  /** Browser version(s) reproduced. */
  imitates?: string;
  /** Expected JA4 with `h2` offered. */
  ja4?: string;
  /** Akamai HTTP/2 fingerprint. */
  http2?: string;
  /** Usable in this build (browser profiles need an impersonation build). */
  available: boolean;
}

/** `GET /api/tls-profiles`. */
export interface TlsProfiles {
  /** Whether browser profiles work in this build. */
  available: boolean;
  /** `BoringSSL` when compiled in. */
  backend?: string;
  /**
   * Whether this instance may use browser profiles (Akev Pro `tls-impersonation`). Absent from
   * daemons that predate licensing (treated as licensed).
   */
  licensed?: boolean;
  profiles: TlsProfileInfo[];
}

export interface DnsOverride {
  host: string;
  address: string;
}

export type UpstreamProxyMode = 'direct' | 'system' | 'manual' | 'pac';

export interface UpstreamProxy {
  mode: UpstreamProxyMode;
  url: string;
  noProxy: string[];
}

export interface CaptureSettings {
  maxBodyBytes: number;
  memoryBudgetBytes: number;
  /** Oldest unpinned exchanges are evicted past it. 0 = unlimited. */
  maxExchanges: number;
  /** Disk space for spilled bodies; the oldest are deleted past it (exchanges stay, marked). 0 = unlimited. */
  spillMaxBytes: number;
  /** Captured traffic waiting to be stored (bytes, min 1 MiB). */
  eventQueueBytes: number;
  recordTunnels: boolean;
  recordTlsFailures: boolean;
  ignoreHosts: string[];
  detectProcesses: boolean;
}

/** akev-core `SessionSettings` — autosave into the session library. */
export interface SessionSettings {
  /** Save the traffic to the "Autosave" session periodically (when changed) and on quit. */
  autosave: boolean;
  autosaveIntervalSecs: number;
  /** Load the autosave back when Akev starts (while autosave is on). */
  restoreOnStartup: boolean;
  /** Newest exchanges an autosave keeps (pinned always). 0 = unlimited. */
  autosaveMaxExchanges: number;
  /** Body bytes an autosave keeps (newest first, pinned always). 0 = unlimited. */
  autosaveMaxBytes: number;
}

/** akev-core `BreakpointSettings` (`settings.breakpoints`). */
export interface BreakpointSettings {
  /** Most exchanges (and WebSocket messages) paused at once; further matches continue without pausing. 0 = no limit. */
  maxPaused: number;
  /** Continue a paused exchange unchanged after this many seconds. 0 = wait for the user. */
  autoReleaseSecs: number;
}

// =====================================================================================
// Other crates' public types used by the API
// =====================================================================================

/** akev-ca `CaInfo`. */
export interface CaInfo {
  subject: string;
  /** Uppercase hex with colons. */
  sha256Fingerprint: string;
  /** Base64 SHA-256 of the SPKI. */
  spkiFingerprint: string;
  /** OpenSSL `subject_hash_old` (Android filename). */
  subjectHashOld: string;
  /** RFC3339. */
  notBefore: string;
  /** RFC3339. */
  notAfter: string;
  /** Negative once expired. */
  daysRemaining: number;
  /** `expiring` within 60 days of `notAfter`; the CA is never replaced automatically. */
  expiry?: 'valid' | 'expiring' | 'expired';
}

export interface UpdateSettings {
  /** Look for a new version in the background (at most once a day). */
  checkAutomatically: boolean;
}

/** `GET /api/update`: the last check for a newer release. */
export interface UpdateInfo {
  current: string;
  latest?: string;
  available: boolean;
  /** Release notes (Markdown). */
  notes?: string;
  publishedAt?: string;
  downloadUrl: string;
  /** Unix ms; absent before the first check. */
  checkedAt?: number;
  error?: string;
}

/** One operating-system certificate store (`GET /api/ca/trust`). */
export interface TrustStore {
  /** `user`, `system` or `nss`. */
  id: string;
  label: string;
  /** Who reads this store. */
  description: string;
  /** `null`: couldn't tell. */
  trusted: boolean | null;
  canChange: boolean;
  /** Changing it asks for an administrator password. */
  needsAdmin: boolean;
  unavailableReason?: string;
}

/** `GET /api/ca/trust`: whether the operating system trusts the Akev CA. */
export interface TrustReport {
  /** The main store's state (`null`: couldn't tell). */
  trusted: boolean | null;
  /** The main store first. */
  stores: TrustStore[];
}

/** `GET /api/exchanges/:id/body/:side` (HTTP API reference). */
export interface BodyView {
  kind: ContentKind;
  /** Decoded size. */
  size: number;
  wireSize: number;
  /** Content-encoding chain. */
  encodings: string[];
  /** Decoded text (null if binary or decode failed). */
  text: string | null;
  /** Pretty-printed text where applicable. */
  formatted: string | null;
  /**
   * protobuf/grpc/msgpack/cbor → JSON, multipart parts, SSE events, form fields, image info.
   * Numbers a JS `number` can't hold exactly (64-bit integers) arrive as `LosslessNumber`s
   * (`lib/json`) when read through `api.exchanges.body` / `api.send`.
   */
  structured: unknown;
  decodeError: string | null;
  /** Text truncated for transport (> 8 MB); use the raw endpoint for the full body. */
  truncated: boolean;
  /** The proxy only captured a prefix of the body. */
  captureTruncated: boolean;
  /**
   * `structured` holds only the first part of the decode: it hit the daemon's size limits
   * (very many values, or a lot of text). Sent only when true.
   */
  structuredTruncated?: boolean;
  /** The imported protobuf schema that decoded this body (Settings → Protobuf schemas). */
  schema?: SchemaInfo;
}

// ---- `BodyView.structured` payloads (akev-content). Narrow on `BodyView.kind`:
//   image → ImageInfo · protobuf → ProtobufJson · grpc/grpc-web → { messages: GrpcMessage[]; error?: string }
//   msg-pack/cbor → arbitrary JSON · multipart → MultipartPart[] · event-stream → SseEvent[]
//   form-url-encoded → [name, value][]

/** `structured` for images (akev-content `image::ImageInfo`). */
export interface ImageInfo {
  /** png, jpeg, gif, webp, avif, svg, ico, bmp */
  format: string;
  width: number | null;
  height: number | null;
}

/** A schemaless protobuf field value (akev-content `protobuf::Value`, tagged by `type`). */
export type ProtobufValue =
  | { type: 'varint'; value: number; signed: number }
  | { type: 'fixed64'; value: number; double: number }
  | { type: 'fixed32'; value: number; float: number }
  | { type: 'message'; fields: ProtobufField[] }
  | { type: 'string'; value: string }
  | { type: 'bytes'; base64: string };

/** akev-content `protobuf::Field`. */
export interface ProtobufField {
  number: number;
  wireType: number;
  value: ProtobufValue;
}

/**
 * Protobuf rendered as JSON (`protobuf::to_json`): field numbers as keys, repeated fields as
 * arrays, nested messages as objects.
 */
export type ProtobufJson = Record<string, unknown>;

/** One gRPC / gRPC-Web message (akev-content `grpc::GrpcMessage`; bytes are not sent). */
export interface GrpcMessage {
  compressed: boolean;
  /** Schemaless decode, if it parses. */
  decoded: unknown;
  /** gRPC-Web trailer frame parsed as headers. */
  trailers: Headers | null;
  /** Decode with the imported schema (`BodyView.schema` names the type), when it fit. */
  typed?: unknown;
  /** Why the message didn't decode with the schema. */
  typedError?: string;
  /** Fields on the wire the schema doesn't define (`7`, `items.9`). */
  unknownFields?: string[];
}

/** One part of a multipart body (akev-content `multipart::Part`). */
export interface MultipartPart {
  headers: Headers;
  name: string | null;
  filename: string | null;
  contentType: string | null;
  size: number;
  /** Text content when the part is textual & < 64 KB. */
  text: string | null;
}

/** One server-sent event (akev-content `sse::SseEvent`). */
export interface SseEvent {
  event: string | null;
  id: string | null;
  retry: number | null;
  data: string;
}

/** akev-proxy `SendOptions` (all fields defaulted server-side). */
export interface SendOptions {
  applyRules?: boolean;
  followRedirects?: boolean;
  timeoutMs?: number;
  insecure?: boolean;
  /** `auto` (settings decide), `http1`, `http2` (TCP, never HTTP/3) or `http3` (HTTP/3 first,
   *  falling back to TCP when QUIC fails). */
  httpVersion?: HttpVersionChoice;
}

/** `SendOptions.httpVersion`. */
export type HttpVersionChoice = 'auto' | 'http1' | 'http2' | 'http3';

/** akev-proxy `SendResult`. */
export interface SendResult {
  /** Exchange id (in the traffic list with `via: composer` — only when `recorded`). */
  id: ExchangeId;
  response: ResponseHead | null;
  error: ExchangeError | null;
  timing: Timing;
}

/** Most ids one `POST /api/exchanges/headers` or `/timings` call may ask for. */
export const BATCH_LIMIT = 5000;

/** `POST /api/exchanges/headers`: header values by lowercase name (missing headers left out). */
export interface HeaderValues {
  id: ExchangeId;
  request?: Record<string, string>;
  response?: Record<string, string>;
}

/** `POST /api/exchanges/timings`. */
export type ExchangeTiming = Timing & { id: ExchangeId };

/** `POST /api/send/repeat` limits. */
export const REPEAT_LIMITS = { count: 10_000, concurrency: 64, delayMs: 60_000 } as const;

/** `POST /api/send/repeat` body: `request`, or `exchangeId` to resend as captured. */
export interface RepeatParams {
  request?: EditableRequest;
  exchangeId?: ExchangeId;
  options?: SendOptions;
  count: number;
  concurrency?: number;
  /** Pause between the requests of each worker (ms). */
  delayMs?: number;
}

/** One finished request of a repeat run. */
export interface RepeatResult {
  /** 0-based start order. */
  index: number;
  /** The exchange it made. */
  id: ExchangeId;
  status?: number;
  error?: ErrorKind;
  /** Request start → end of the response body (or the failure), ms. */
  durationMs?: number;
}

/** The last line of a repeat run. */
export interface RepeatDone {
  done: true;
  sent: number;
}

/** `POST /api/send` and `POST /api/exchanges/:id/replay`. */
export type SendResponse = SendResult & {
  body: BodyView;
  /**
   * Whether exchange `id` was stored in the traffic list. `false` while recording is paused
   * (`PUT /api/capture`) or when the capture settings ignore the host: the id then can't be
   * opened (`/api/exchanges/:id` answers 404).
   */
  recorded: boolean;
};

/** akev-codegen targets. */
export type CodegenTarget =
  | 'curl'
  | 'curl-windows'
  | 'wget'
  | 'httpie'
  | 'powershell'
  | 'js-fetch'
  | 'node-fetch'
  | 'node-axios'
  | 'python-requests'
  | 'python-httpx'
  | 'go-net-http'
  | 'rust-reqwest'
  | 'java-http-client'
  | 'kotlin-okhttp'
  | 'csharp-http-client'
  | 'php-curl'
  | 'ruby-net-http'
  | 'swift-url-session'
  | 'dart-http'
  | 'raw-http';

export interface TargetInfo {
  id: CodegenTarget;
  /** e.g. "cURL", "Python · requests". */
  name: string;
  /** Language id for highlighting (`shell`, `javascript`, `python`, `go`, `http`, ...). */
  language: string;
  /** UI grouping ("Shell", "JavaScript", "Python", ...). */
  group: string;
}

export interface Snippet {
  target: CodegenTarget;
  language: string;
  code: string;
}

/** akev-filter `Suggestion`. Offsets are UTF-8 **byte** offsets (see lib/encoding.ts). */
export interface Suggestion {
  label: string;
  detail: string;
  insert: string;
  replaceStart: number;
  replaceEnd: number;
  kind: 'field' | 'operator' | 'value' | 'flag' | 'example';
}

/** akev-filter `FieldDoc`. */
export interface FieldDoc {
  name: string;
  aliases: string[];
  description: string;
  examples: string[];
}

/** Filter parse error. `start`/`end` are UTF-8 byte offsets into the query. */
export interface FilterError {
  message: string;
  start: number;
  end: number;
}

export type FilterValidation = { ok: true } | { ok: false; error: FilterError };

// =====================================================================================
// HTTP API payloads
// =====================================================================================

export type Platform = 'linux' | 'macos' | 'windows';

export interface StatusStats {
  exchanges: number;
  memoryBodyBytes: number;
  spilledBodyBytes: number;
  activeConnections: number;
  totalConnections: number;
  /** Exchanges evicted by `capture.maxExchanges` since start. */
  evicted: number;
  /** Bodies not recorded because capture was overloaded. */
  droppedBodies: number;
  /** Spilled bodies deleted to stay under `capture.spillMaxBytes`. */
  diskDroppedBodies: number;
  /** Exchanges whose bodies were too large for the rule body budget (passed through untouched). */
  passedThrough: number;
  /** Breakpoints released automatically, or skipped because too many exchanges were paused. */
  autoReleased: number;
  /** Bytes the spill files occupy on disk. */
  spillDiskBytes: number;
  /** Connections refused by `proxy.access`. */
  refusedConnections: number;
  /** Requests refused for missing or wrong proxy credentials. */
  authFailures: number;
}

/** `GET /api/diagnostics` — what a bug report needs, without secrets or captured traffic. */
export interface Diagnostics {
  version: string;
  releaseDate?: string;
  /** Operating system name and version. */
  os: string;
  arch: string;
  platform: Platform | string;
  desktop: boolean;
  uptimeSecs: number;
  /** Source repository URL (issues are filed there). */
  repository: string;
  stats: StatusStats;
  /** Settings summary (no secrets). */
  settings: Record<string, unknown>;
  recentWarnings: DiagnosticsWarning[];
  /** File names of crash reports in the data directory. */
  crashReports: string[];
  /** The log file, `null` when logging to a file is off. */
  logFile: string | null;
}

export interface DiagnosticsWarning {
  at: string;
  level: string;
  target: string;
  message: string;
}

export interface Status {
  version: string;
  proxy: {
    port: number;
    bindAddress: string;
    /** host:port reachable addresses incl. LAN IPs. */
    addresses: string[];
    /** The HTTP/3 (QUIC) listener. */
    http3: Http3ListenerStatus;
  };
  apiUrl: string;
  dataDir: string;
  ca: CaInfo;
  /** Unix ms. */
  startedAt: number;
  stats: StatusStats;
  platform: Platform;
  /** Running inside the desktop shell. */
  desktop: boolean;
  /** The autosaved session restored at startup (once loaded). */
  restoredSession?: RestoredSession;
  /** Plan & license. */
  license: LicenseStatus;
}

// ---- Akev Pro license (`/api/license`)

/** A Pro capability the daemon gates on the license. */
export type Feature = 'scripting' | 'tls-impersonation' | 'wireguard' | 'transparent' | 'jvm-attach' | 'proto-schemas' | 'llm-lens' | 'mcp-write';

/** The license key's signed payload. Dates are `YYYY-MM-DD`. */
export interface LicenseInfo {
  id: string;
  email: string;
  name?: string;
  issued: string;
  /** Versions released up to this date are covered. */
  updatesUntil: string;
  /** People a team license covers (absent: one person). */
  seats?: number;
}

export type LicensePlan = 'free' | 'pro';

/**
 * - `none`: Free, no key and no trial.
 * - `trial` / `trial-ended`: the 14-day trial (see `trialEndsAt`).
 * - `active`: a valid key covering this build.
 * - `updates-ended`: a genuine key, but this build was released after `updatesUntil` → runs as Free.
 * - `invalid`: the stored key doesn't verify (`error` says why).
 */
export type LicenseState = 'none' | 'trial' | 'trial-ended' | 'active' | 'updates-ended' | 'invalid';

/** `GET /api/license`, `Status.license` and the `license` stream message. */
export interface LicenseStatus {
  plan: LicensePlan;
  state: LicenseState;
  /** What this instance is entitled to right now. */
  features: Feature[];
  /** For `active`, `updates-ended` (and `invalid` when the payload parsed). */
  license?: LicenseInfo;
  /** Unix ms; for `trial` / `trial-ended`. */
  trialEndsAt?: number;
  trialAvailable: boolean;
  /** Why a stored key is invalid. */
  error?: string;
  /** This build's release date, `YYYY-MM-DD`. */
  releaseDate: string;
}

/** `Status.proxy.http3`. */
export interface Http3ListenerStatus {
  /** `settings.proxy.http3.listen`. */
  enabled: boolean;
  /** UDP `ip:port` it listens on (when running). */
  address?: string;
  port?: number;
  /** Why it isn't running although enabled (e.g. the UDP port is taken). */
  error?: string;
}

/** `Status.proxy` (also returned by `POST /api/proxy/rebind`). */
export type ProxyStatus = Status['proxy'];

// ---- Session library (`/api/sessions`)

export type SessionKind = 'saved' | 'autosave';

/** A saved session (`GET /api/sessions`). */
export interface SessionInfo {
  /** Stable id (`checkout-bug`, `autosave`). */
  id: string;
  name: string;
  kind: SessionKind;
  exchanges: number;
  /** File size in bytes (compressed). */
  size: number;
  /** Unix ms. */
  createdAt: number;
  /** Unix ms of the last content write. */
  updatedAt: number;
}

/** `POST /api/sessions` — all traffic, or `ids` and/or `filter` (intersected). */
export interface SaveSessionRequest {
  name: string;
  ids?: ExchangeId[];
  filter?: string;
}

/** `replace` removes the current traffic (pinned too) first; `append` adds to it. */
export type SessionOpenMode = 'replace' | 'append';

/** `Status.restoredSession` / the `session-restored` stream message. */
export interface RestoredSession {
  id: string;
  name: string;
  exchanges: number;
  /** When the session was saved (unix ms). */
  savedAt: number;
}

/** `POST /api/proxy/rebind`: move the proxy now (no settings change). */
export interface ProxyRebindRequest {
  port: number;
  /** Default: the bind address in effect. */
  bindAddress?: string;
}

export interface StreamMessageWithPayload extends StreamMessage {
  /** UTF-8 payload if text. */
  text: string | null;
  /** Payload when binary. */
  base64: string | null;
}

export interface ExchangePatch {
  ids: ExchangeId[];
  pinned?: boolean;
  color?: TagColor | null;
  note?: string | null;
}

export interface RulesPutResult {
  errors: [ruleId: RuleId, message: string][];
}

export type RulesTestRequest =
  | { exchangeId: ExchangeId; rules?: RuleSet }
  | { request: EditableRequest; rules?: RuleSet };

/** akev-api `TestOut`. Matching `script` rules run (sandboxed) during the dry run. */
export interface RulesTestResult {
  matched: RuleId[];
  terminalRule: RuleId | null;
  action: Action;
  /** Console output of the scripts that ran (`onRequest` then `onResponse`); absent when empty. */
  scriptLogs?: string[];
  /** First script error (the message includes the line number). */
  error?: string;
  /** What the scripts did; absent when no script ran (or `onRequest` failed). */
  scriptOutcome?: ScriptOutcome;
}

/** `RulesTestResult.scriptOutcome`. */
export type ScriptOutcome =
  /** The request continues upstream (as modified by the scripts). `response`: after `onResponse`, when the exchange has a captured response. */
  | { kind: 'continue'; method: string; url: string; headers: Headers; body: string; response?: ScriptResponse }
  /** A script answered the request itself. */
  | { kind: 'respond'; response: ScriptResponse };

export interface ScriptResponse {
  status: number;
  headers: Headers;
  body: string;
}

export interface IdsResult {
  ids: ExchangeId[];
}

export interface CaptureState {
  recording: boolean;
}

export type TerminalShell = 'bash' | 'zsh' | 'fish' | 'powershell' | 'cmd';

export type NoticeLevel = 'info' | 'warning' | 'error';

// =====================================================================================
// Live stream (GET /api/stream)
// =====================================================================================

export interface FilteredDelta {
  add: ExchangeId[];
  remove: ExchangeId[];
}

export type ServerMessage =
  | {
      type: 'hello';
      status: Status;
      summaries: ExchangeSummary[];
      paused: PausedExchange[];
      rules: RuleSet;
      settings: Settings;
      /** WebSocket messages paused at `ws-breakpoint` rules. */
      pausedMessages: PausedMessage[];
      /**
       * Older summaries still to come in `hello-more` chunks (0 = `summaries` is the whole
       * snapshot). `summaries` then holds only the newest exchanges.
       */
      remaining: number;
    }
  /** The next older chunk of a paged `hello` (id order; older than everything sent before). */
  | { type: 'hello-more'; summaries: ExchangeSummary[]; remaining: number }
  | { type: 'exchanges'; added: ExchangeSummary[]; updated: ExchangeSummary[]; filtered?: FilteredDelta }
  | { type: 'removed'; ids: ExchangeId[] }
  | { type: 'cleared' }
  | { type: 'messages'; id: ExchangeId; messages: StreamMessage[] }
  | { type: 'paused'; exchange: PausedExchange }
  | { type: 'resumed'; id: ExchangeId }
  | { type: 'rules'; rules: RuleSet }
  | { type: 'rule-stats'; stats: RuleStats[] }
  | { type: 'settings'; settings: Settings }
  | { type: 'interceptors'; interceptors: InterceptorInfo[] }
  | { type: 'stats'; stats: StatusStats }
  | { type: 'notice'; level: NoticeLevel; message: string }
  | { type: 'capture'; recording: boolean }
  | { type: 'session-restored'; session: RestoredSession }
  | { type: 'license'; license: LicenseStatus }
  | { type: 'filter-result'; seq: number; query: string; ids: ExchangeId[] | null; error?: FilterError }
  | { type: 'pong' }
  | { type: 'message-paused'; message: PausedMessage }
  | { type: 'message-resumed'; id: ExchangeId; pauseId: number }
  | { type: 'client-connected'; address: string };

export type ServerMessageType = ServerMessage['type'];
export type ServerMessageOf<T extends ServerMessageType> = Extract<ServerMessage, { type: T }>;

export type ClientMessage = { type: 'ping' } | { type: 'set-filter'; query: string; seq: number };

// =====================================================================================
// Desktop shell bridge (window.__AKEV__)
// =====================================================================================

/**
 * Injected by the Tauri desktop shell before the UI loads (on the daemon's own origin). Native
 * integrations go over Tauri's IPC (`lib/platform.ts`), not through this object.
 */
export interface AkevDesktopGlobals {
  /** API session token. */
  token?: string;
  /** API base URL, e.g. `http://127.0.0.1:45456`. */
  apiUrl?: string;
  desktop?: boolean;
  platform?: Platform;
  /** How the desktop window is framed (see `lib/windowChrome.ts`). */
  windowChrome?: {
    /** The window started without system decorations: the UI draws the window buttons. */
    custom: boolean;
    /** GTK `gtk-decoration-layout` (Linux), e.g. `appmenu:minimize,maximize,close`. */
    layout?: string | null;
  };
}

export interface AkevDesktopBridge {
  openExternal?(url: string): Promise<void> | void;
  /** Save `data` (or download `url`) via a native dialog. Resolves false if cancelled. */
  saveFile?(options: { suggestedName: string; url?: string; data?: string | Uint8Array }): Promise<boolean>;
  notify?(title: string, body: string): void;
  /** Download, verify and install the newest release, then restart. False: nothing newer. */
  installUpdate?(): Promise<boolean>;
}

declare global {
  interface Window {
    __AKEV__?: AkevDesktopGlobals;
  }
}

// =====================================================================================
// Protobuf schemas (`/api/protos`, akev-api `protos` + akev-content `schema`)
// =====================================================================================

/** How the message type of a body was chosen. */
export type SchemaSource = 'grpc' | 'content-type' | 'mapping';

/** The gRPC method a request path names (akev-content `schema::GrpcMethodInfo`). */
export interface GrpcMethodInfo {
  /** Fully-qualified service (`shop.v1.Catalog`). */
  service: string;
  method: string;
  inputType: string;
  outputType: string;
  clientStreaming: boolean;
  serverStreaming: boolean;
}

/** `BodyView.schema` (akev-content `schema::SchemaView`). */
export interface SchemaInfo {
  /** Fully-qualified message type (`shop.v1.Item`). */
  messageType: string;
  source: SchemaSource;
  /** For `grpc`. */
  method?: GrpcMethodInfo;
  /** For `mapping`: the mapping's URL pattern. */
  mapping?: string;
  /** Typed decode of a protobuf body (gRPC bodies carry theirs per message). */
  typed?: unknown;
  /** Why typed decoding failed (the schemaless decode is still in `structured`). */
  error?: string;
  /** Fields on the wire the schema doesn't define. */
  unknownFields?: string[];
}

export type ProtoFileKind = 'proto' | 'descriptor-set';

/** A compile error (protox) with its location when known (1-based). */
export interface ProtoError {
  file: string;
  line: number | null;
  column: number | null;
  message: string;
}

export interface ProtoMethod {
  name: string;
  /** `/package.Service/Method`. */
  path: string;
  inputType: string;
  outputType: string;
  clientStreaming: boolean;
  serverStreaming: boolean;
}

export interface ProtoService {
  /** Fully-qualified name. */
  name: string;
  methods: ProtoMethod[];
}

/** One imported schema file and what it defines. */
export interface ProtoFileInfo {
  /** Relative path, e.g. `shop/v1/catalog.proto`. */
  name: string;
  kind: ProtoFileKind;
  size: number;
  /** Unix ms. */
  addedAt: number;
  /** The path it is compiled under when imports refer to it by another one. */
  importName?: string;
  package?: string;
  /** Descriptor sets: the `.proto` files inside. */
  contains?: string[];
  services: ProtoService[];
  messages: string[];
  enums: string[];
  /** Empty = compiled. */
  errors: ProtoError[];
}

export type ProtoMappingSide = 'request' | 'response' | 'both';

/** URL pattern → message type, for protobuf bodies that don't say what they are. */
export interface ProtoMapping {
  id: string;
  enabled: boolean;
  /** `*` wildcards; `/path`, `host/path`, `https://host/path` or a host. */
  pattern: string;
  side: ProtoMappingSide;
  messageType: string;
}

/** `GET /api/protos`. */
export interface ProtoRegistry {
  files: ProtoFileInfo[];
  mappings: ProtoMapping[];
  /** Every message type the files define. */
  messageTypes: string[];
}

/** One file of `POST /api/protos`: `.proto` text in `content`, or bytes as `base64`. */
export interface ProtoUploadFile {
  name: string;
  content?: string;
  base64?: string;
}

// =====================================================================================
// LLM traffic lens — `GET /api/exchanges/:id/llm`
// =====================================================================================

/** The wire API of a recognised LLM call. */
export type LlmApi = 'openai-chat' | 'openai-completions' | 'openai-responses' | 'anthropic-messages' | 'gemini' | 'ollama-chat' | 'ollama-generate' | 'bedrock-converse';

/** One piece of message content. */
export type LlmPart =
  | { type: 'text'; text: string }
  /** `data` is base64 without the `data:` prefix; `url` a remote image (never loaded). */
  | { type: 'image'; mediaType?: string; data?: string; url?: string; size?: number }
  | { type: 'file'; label: string; mediaType?: string; name?: string; url?: string; size?: number }
  /** `arguments` is the parsed JSON (the raw string when it wasn't valid JSON). */
  | { type: 'tool-call'; id?: string; name: string; arguments: unknown }
  | { type: 'tool-result'; id?: string; name?: string; content: LlmPart[]; isError?: boolean }
  | { type: 'thinking'; text: string; redacted?: boolean }
  | { type: 'refusal'; text: string }
  | { type: 'other'; label: string; value: unknown };

export interface LlmMessage {
  /** `system`, `developer`, `user`, `assistant` or `tool`. */
  role: string;
  name?: string;
  content: LlmPart[];
}

export interface LlmTool {
  name: string;
  description?: string;
  /** JSON Schema of the arguments. */
  schema?: unknown;
  /** Built-in tool type (`web_search_20250305`, `googleSearch`…); absent for function tools. */
  kind?: string;
}

/** `inputTokens` counts every prompt token (cached included); `outputTokens` includes reasoning. */
export interface LlmUsage {
  inputTokens?: number;
  outputTokens?: number;
  totalTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  reasoningTokens?: number;
}

export interface LlmResponse {
  id?: string;
  model?: string;
  messages: LlmMessage[];
  stopReason?: string;
  usage?: LlmUsage;
}

/** Durations are measured from the moment the request was sent upstream. */
export interface LlmTiming {
  ttftMs?: number;
  totalMs?: number;
  tokensPerSecond?: number;
  chunks?: number;
}

export interface LlmError {
  status?: number;
  kind?: string;
  code?: string;
  message: string;
}

/** A recognised LLM API call, normalised across providers. */
export interface LlmExchange {
  provider: string;
  api: LlmApi;
  model?: string;
  stream: boolean;
  request: {
    system?: string;
    messages: LlmMessage[];
    tools: LlmTool[];
    params: Record<string, unknown>;
  };
  response?: LlmResponse;
  timing: LlmTiming;
  error?: LlmError;
  warnings?: string[];
}

import type { TransparentSettings } from './transparentTypes';
export * from './transparentTypes';
