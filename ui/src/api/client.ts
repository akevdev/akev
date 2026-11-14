/**
 * Typed client for every endpoint in the HTTP API reference (akev.dev/docs/api).
 *
 * ```ts
 * import { api, ApiError } from '@/api/client';
 * const detail = await api.exchanges.get(42);
 * try { await api.rules.put(ruleSet); } catch (e) { if (e instanceof ApiError) toast.error(e.message); }
 * ```
 *
 * All I/O goes through a {@link Transport}. The browser transport uses `fetch`/`WebSocket`;
 * the in-browser mock backend (`?mock`) installs its own transport, so pages exercise exactly
 * the same code paths in mock mode as against the real daemon.
 */

import { mayHaveInexactNumbers, parseJsonLossless } from '../lib/json';
import type { ClientConfig } from './auth';
import type {
  ActivationResult,
  BodyView,
  CaInfo,
  TrustReport,
  UpdateInfo,
  CaptureState,
  CodegenTarget,
  EditableRequest,
  EditableResponse,
  ExchangeDetail,
  ExchangeId,
  ExchangePatch,
  ExchangeSummary,
  FieldDoc,
  FilterValidation,
  IdsResult,
  InterceptorInfo,
  PausedExchange,
  ProtoMapping,
  ProtoRegistry,
  ProtoUploadFile,
  ProxyRebindRequest,
  ProxyStatus,
  RequestResume,
  ResponseResume,
  RuleSet,
  RuleStats,
  RulesPutResult,
  RulesTestRequest,
  RulesTestResult,
  SaveSessionRequest,
  SendOptions,
  SendResponse,
  SessionInfo,
  SessionOpenMode,
  Settings,
  Side,
  Snippet,
  Status,
  StreamMessageWithPayload,
  Suggestion,
  TargetInfo,
  TerminalShell,
} from './types';
import type { InjectMessageRequest, InjectMessageResult, LlmExchange, MessageResume, PausedMessage } from './types';
import type { WireguardInfo, WireguardPatch } from './types';
import type { ExchangeTiming, HeaderValues, RepeatDone, RepeatParams, RepeatResult } from './types';
import type { TransparentInfo, TransparentPlan, TransparentPlanRequest } from './transparentTypes';
import type { TlsProfiles } from './types';
import type { Feature, LicenseStatus } from './types';
import type { Diagnostics } from './types';

// =====================================================================================
// Transport
// =====================================================================================

/** Minimal WebSocket surface used by the stream client (implemented by the mock too). */
export interface WebSocketLike {
  readonly readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: Event) => void) | null;
  onmessage: ((ev: MessageEvent) => void) | null;
  onclose: ((ev: CloseEvent) => void) | null;
  onerror: ((ev: Event) => void) | null;
}

export interface Transport {
  fetch(url: string, init: RequestInit): Promise<Response>;
  openSocket(url: string): WebSocketLike;
  /**
   * Optionally rewrite a resource URL used directly by the DOM (`<img src>`, `<audio>`,
   * download links). The mock returns `data:`/`blob:` URLs here.
   */
  resolveUrl?(url: string): string;
}

export const browserTransport: Transport = {
  fetch: (url, init) => fetch(url, init),
  openSocket: (url) => new WebSocket(url),
};

// =====================================================================================
// Errors
// =====================================================================================

/**
 * Error thrown by every API call. `status` is the HTTP status (0 for network failures and
 * aborts); `code` is the daemon's machine code (`not-found`, `invalid-request`, ...) or one of
 * the client-side codes `network`, `aborted`, `timeout`, `bad-response`.
 */
export class ApiError extends Error {
  override readonly name = 'ApiError';

  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly method?: string,
    readonly path?: string,
    /** Extra fields of the daemon's error object besides `code`/`message` (e.g. `feature`). */
    readonly details: Readonly<Record<string, unknown>> = {},
  ) {
    super(message);
  }

  /** `pro-required` (HTTP 402): the endpoint needs Akev Pro. */
  get isProRequired(): boolean {
    return this.code === 'pro-required';
  }
  /** The Pro feature a `pro-required` error names. */
  get feature(): Feature | undefined {
    const f = this.details.feature;
    return typeof f === 'string' ? (f as Feature) : undefined;
  }

  get isNetworkError(): boolean {
    return this.code === 'network';
  }
  get isAborted(): boolean {
    return this.code === 'aborted';
  }
  get isUnauthorized(): boolean {
    return this.status === 401 || this.status === 403;
  }
  get isNotFound(): boolean {
    return this.status === 404;
  }
}

/** Type guard usable in `catch` blocks. */
export function isApiError(e: unknown): e is ApiError {
  return e instanceof ApiError;
}

/** Human-readable message for any thrown value (for toasts). */
export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === 'string') return e;
  return 'Unknown error';
}

// =====================================================================================
// Core request machinery
// =====================================================================================

type QueryValue = string | number | boolean | null | undefined | readonly (string | number)[];
export type Query = Record<string, QueryValue>;

/** A page of `GET /api/exchanges/:id/messages` (exclusive `seq` bounds). */
export interface MessagesPage {
  /** Only messages with `seq` > `after`. */
  after?: number;
  /** Only messages with `seq` < `before`. */
  before?: number;
  /** Page size (daemon default 1000, max 10000). */
  limit?: number;
  /** The newest `limit` messages of the range instead of the oldest. */
  tail?: boolean;
}

export interface RequestOptions {
  query?: Query;
  /** JSON-serialisable body, or raw bytes / Blob / string for uploads. */
  body?: unknown;
  /** Content-Type for raw bodies (default `application/octet-stream`). */
  contentType?: string;
  signal?: AbortSignal;
  /** Abort after this many ms (ApiError code `timeout`). */
  timeoutMs?: number;
  /** How to read the response (default: JSON, `undefined` for 204; `response`: the `Response` itself, for streaming). */
  responseType?: 'json' | 'text' | 'bytes' | 'none' | 'response';
  /** JSON parser for `responseType: 'json'` (default `JSON.parse`). */
  parseJson?: (text: string) => unknown;
}

function buildQueryString(query: Query | undefined): string {
  if (!query) return '';
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value == null || value === false) continue;
    if (value === true) params.set(key, '1');
    else if (Array.isArray(value)) params.set(key, value.join(','));
    else params.set(key, String(value));
  }
  const s = params.toString();
  return s ? `?${s}` : '';
}

type JsonObject = Record<string, unknown>;

function isJsonObject(v: unknown): v is JsonObject {
  return v != null && typeof v === 'object' && !Array.isArray(v);
}

/**
 * Parse JSON whose `BodyView.structured` (at `path`, e.g. `[]` for a body view or `['body']`
 * for a send result) holds decoded msgpack/CBOR/protobuf/gRPC payloads. Their 64-bit integers
 * would be rounded by `JSON.parse`, so when the text may contain such a number the payload is
 * re-read losslessly (inexact numbers become `LosslessNumber`s). Everything else keeps plain
 * `JSON.parse` values.
 */
function parseWithLosslessStructured(text: string, path: readonly string[]): unknown {
  const value: unknown = JSON.parse(text);
  let view: unknown = value;
  for (const k of path) view = isJsonObject(view) ? view[k] : undefined;
  if (!isJsonObject(view) || (view.structured == null && view.schema == null) || !mayHaveInexactNumbers(text)) return value;
  try {
    let exact: unknown = parseJsonLossless(text);
    for (const k of path) exact = isJsonObject(exact) ? exact[k] : undefined;
    if (isJsonObject(exact)) {
      view.structured = exact.structured;
      // Typed protobuf decodes (`schema.typed`) carry 64-bit integers too.
      if (exact.schema !== undefined) view.schema = exact.schema;
    }
  } catch {
    // Valid JSON always parses; keep the natively parsed value if it somehow doesn't.
  }
  return value;
}

/** Parse a `BodyView` response without rounding big integers in `structured`. */
export function parseBodyViewJson(text: string): unknown {
  return parseWithLosslessStructured(text, []);
}

/** Parse a `SendResponse` (`{…, body: BodyView}`) without rounding big integers in `body.structured`. */
export function parseSendResponseJson(text: string): unknown {
  return parseWithLosslessStructured(text, ['body']);
}

function isRawBody(body: unknown): body is BodyInit {
  return (
    typeof body === 'string' ||
    body instanceof Uint8Array ||
    body instanceof ArrayBuffer ||
    (typeof Blob !== 'undefined' && body instanceof Blob) ||
    (typeof FormData !== 'undefined' && body instanceof FormData)
  );
}

/**
 * The API client. Use the shared {@link api} instance; construct your own only in tests.
 */
export class ApiClient {
  private config: ClientConfig;
  private transport: Transport;
  private readonly errorListeners = new Set<(error: ApiError) => void>();

  constructor(config: ClientConfig = { baseUrl: '', token: null }, transport: Transport = browserTransport) {
    this.config = { ...config };
    this.transport = transport;
  }

  /** Update base URL / token (e.g. after reading `?token=`). */
  configure(config: Partial<ClientConfig>): void {
    this.config = { ...this.config, ...config };
  }

  getConfig(): Readonly<ClientConfig> {
    return this.config;
  }

  /** Swap the transport (the mock backend does this). */
  setTransport(transport: Transport): void {
    this.transport = transport;
  }

  getTransport(): Transport {
    return this.transport;
  }

  /**
   * Observe every error response from the daemon (not network failures or aborts) before it
   * is thrown to the caller — app-wide reactions such as opening the upgrade dialog on
   * `pro-required`. Returns an unsubscribe function.
   */
  onError(listener: (error: ApiError) => void): () => void {
    this.errorListeners.add(listener);
    return () => this.errorListeners.delete(listener);
  }

  /**
   * Absolute URL for a path, with `?token=` appended in token mode — for downloads, `<img>`
   * sources and the WebSocket (where headers can't be set).
   */
  url(path: string, query?: Query, options: { withToken?: boolean } = {}): string {
    const q: Query = { ...query };
    if ((options.withToken ?? true) && this.config.token) q.token = this.config.token;
    const raw = `${this.config.baseUrl}${path}${buildQueryString(q)}`;
    return this.transport.resolveUrl ? this.transport.resolveUrl(raw) : raw;
  }

  /** `ws(s)://…/api/stream?token=…`. */
  streamUrl(): string {
    const q = this.config.token ? `?token=${encodeURIComponent(this.config.token)}` : '';
    const base = this.config.baseUrl || window.location.origin;
    return `${base.replace(/^http/, 'ws')}/api/stream${q}`;
  }

  openStreamSocket(): WebSocketLike {
    return this.transport.openSocket(this.streamUrl());
  }

  /** Perform a request. Throws {@link ApiError} on any failure. */
  async request<T>(method: string, path: string, options: RequestOptions = {}): Promise<T> {
    const url = `${this.config.baseUrl}${path}${buildQueryString(options.query)}`;
    const headers: Record<string, string> = { Accept: 'application/json' };
    if (this.config.token) headers.Authorization = `Bearer ${this.config.token}`;

    let body: BodyInit | undefined;
    if (options.body !== undefined) {
      if (isRawBody(options.body)) {
        body = options.body;
        if (!(typeof FormData !== 'undefined' && options.body instanceof FormData)) {
          headers['Content-Type'] = options.contentType ?? 'application/octet-stream';
        }
      } else {
        body = JSON.stringify(options.body);
        headers['Content-Type'] = 'application/json';
      }
    }

    const controller = new AbortController();
    const onAbort = () => controller.abort(options.signal?.reason);
    if (options.signal) {
      if (options.signal.aborted) throw new ApiError(0, 'aborted', 'Request aborted', method, path);
      options.signal.addEventListener('abort', onAbort, { once: true });
    }
    let timedOut = false;
    const timer =
      options.timeoutMs != null
        ? setTimeout(() => {
            timedOut = true;
            controller.abort();
          }, options.timeoutMs)
        : null;

    let response: Response;
    try {
      response = await this.transport.fetch(url, {
        method,
        headers,
        body,
        signal: controller.signal,
        // Only the Bearer token authenticates; the API never accepts cookies.
        credentials: 'omit',
      });
    } catch (e) {
      if (timedOut) throw new ApiError(0, 'timeout', `Request timed out after ${options.timeoutMs} ms`, method, path);
      if (controller.signal.aborted) throw new ApiError(0, 'aborted', 'Request aborted', method, path);
      throw new ApiError(0, 'network', `Couldn't reach Akev (${errorMessage(e)})`, method, path);
    } finally {
      if (timer != null) clearTimeout(timer);
      options.signal?.removeEventListener('abort', onAbort);
    }

    if (!response.ok) {
      const error = await this.toApiError(response, method, path);
      for (const l of this.errorListeners) {
        try {
          l(error);
        } catch (e) {
          console.error('[akev] API error listener failed', e);
        }
      }
      throw error;
    }

    const type = options.responseType ?? 'json';
    if (type === 'response') return response as T;
    if (response.status === 204 || type === 'none') return undefined as T;
    try {
      if (type === 'text') return (await response.text()) as T;
      if (type === 'bytes') return new Uint8Array(await response.arrayBuffer()) as T;
      const text = await response.text();
      return (text ? (options.parseJson ?? JSON.parse)(text) : undefined) as T;
    } catch (e) {
      throw new ApiError(response.status, 'bad-response', `Invalid response from ${path}: ${errorMessage(e)}`, method, path);
    }
  }

  private async toApiError(response: Response, method: string, path: string): Promise<ApiError> {
    let code = `http-${response.status}`;
    let message = response.statusText || `Request failed with status ${response.status}`;
    let details: Record<string, unknown> = {};
    let text = '';
    try {
      text = await response.text();
    } catch {
      // body unreadable: keep defaults
    }
    if (text) {
      let parsed: unknown;
      try {
        parsed = JSON.parse(text);
      } catch {
        parsed = undefined;
      }
      const err = parsed && typeof parsed === 'object' ? (parsed as { error?: { code?: unknown; message?: unknown } }).error : undefined;
      if (err && typeof err === 'object') {
        const { code: c, message: m, ...rest } = err as Record<string, unknown>;
        if (typeof c === 'string') code = c;
        if (typeof m === 'string') message = m;
        details = rest;
      } else {
        // Plain-text or HTML error page (e.g. from an intermediary): show its text.
        const plain = text.replace(/<[^>]*>/g, ' ').replace(/\s+/g, ' ').trim();
        if (plain) message = plain.slice(0, 300);
      }
    }
    if (response.status === 401 && code.startsWith('http-')) code = 'unauthorized';
    return new ApiError(response.status, code, message, method, path, details);
  }

  private get<T>(path: string, options?: RequestOptions) {
    return this.request<T>('GET', path, options);
  }
  private post<T>(path: string, body?: unknown, options?: RequestOptions) {
    return this.request<T>('POST', path, { ...options, body });
  }
  private put<T>(path: string, body?: unknown, options?: RequestOptions) {
    return this.request<T>('PUT', path, { ...options, body });
  }
  private patch<T>(path: string, body?: unknown, options?: RequestOptions) {
    return this.request<T>('PATCH', path, { ...options, body });
  }

  // ===================================================================================
  // Status & lifecycle
  // ===================================================================================

  /** `GET /api/status`. */
  status(signal?: AbortSignal): Promise<Status> {
    return this.get('/api/status', { signal });
  }

  /** `GET /api/diagnostics` — version, OS, settings summary, recent warnings (no secrets, no traffic). */
  diagnostics(signal?: AbortSignal): Promise<Diagnostics> {
    return this.get('/api/diagnostics', { signal });
  }

  /** `POST /api/shutdown`. */
  shutdown(): Promise<void> {
    return this.post('/api/shutdown');
  }

  // ===================================================================================
  // Akev Pro license
  // ===================================================================================

  readonly license = {
    /** `GET /api/license`. */
    get: (signal?: AbortSignal): Promise<LicenseStatus> => this.get('/api/license', { signal }),
    /** `PUT /api/license` — install a key (400 `invalid-license` with the reason when it doesn't verify). */
    activate: (key: string): Promise<LicenseStatus> => this.put('/api/license', { key }),
    /** `DELETE /api/license` — forget the stored key (back to Free or the trial). */
    remove: (): Promise<LicenseStatus> => this.request('DELETE', '/api/license'),
    /** `POST /api/license/trial` — start the 14-day trial (409 `trial-unavailable` once used). */
    startTrial: (): Promise<LicenseStatus> => this.post('/api/license/trial'),
  };

  // ===================================================================================
  // Exchanges
  // ===================================================================================

  readonly exchanges = {
    /** `GET /api/exchanges?filter=&limit=` — summaries in id order. */
    list: (params: { filter?: string; limit?: number } = {}, signal?: AbortSignal): Promise<ExchangeSummary[]> =>
      this.get('/api/exchanges', { query: { filter: params.filter || undefined, limit: params.limit }, signal }),

    /** `GET /api/exchanges/:id`. */
    get: (id: ExchangeId, signal?: AbortSignal): Promise<ExchangeDetail> => this.get(`/api/exchanges/${id}`, { signal }),

    /** `GET /api/exchanges/:id/body/:side` (`original` = pre-modification body). */
    body: (id: ExchangeId, side: Side, options: { original?: boolean } = {}, signal?: AbortSignal): Promise<BodyView> =>
      this.get(`/api/exchanges/${id}/body/${side}`, { query: { original: options.original }, signal, parseJson: parseBodyViewJson }),

    /** URL of the raw body bytes, for `<img>`/`<audio>`/downloads. */
    rawUrl: (id: ExchangeId, side: Side, options: { decoded?: boolean; download?: boolean } = {}): string =>
      this.url(`/api/exchanges/${id}/body/${side}/raw`, { decoded: options.decoded, download: options.download }),

    /** Fetch raw body bytes (`decoded` removes content-encoding). */
    rawBytes: (id: ExchangeId, side: Side, options: { decoded?: boolean } = {}, signal?: AbortSignal): Promise<Uint8Array> =>
      this.get(`/api/exchanges/${id}/body/${side}/raw`, { query: { decoded: options.decoded }, responseType: 'bytes', signal }),

    /**
     * `GET /api/exchanges/:id/messages?after=&before=&limit=&tail=` — one page of WebSocket/SSE
     * messages with payloads, ascending `seq` (a number is `after`). See {@link MessagesPage}.
     */
    messages: (id: ExchangeId, page?: number | MessagesPage, signal?: AbortSignal): Promise<StreamMessageWithPayload[]> => {
      const p: MessagesPage = typeof page === 'number' ? { after: page } : (page ?? {});
      return this.get(`/api/exchanges/${id}/messages`, { query: { after: p.after, before: p.before, limit: p.limit, tail: p.tail }, signal });
    },

    /** `POST /api/exchanges/:id/messages` — inject a message into a live WebSocket (409 when it isn't open). */
    sendMessage: (id: ExchangeId, message: InjectMessageRequest): Promise<InjectMessageResult> => this.post(`/api/exchanges/${id}/messages`, message),

    /** `GET /api/exchanges/:id/llm` — the conversation view of an LLM API call (404 `not-llm` otherwise). */
    llm: (id: ExchangeId, signal?: AbortSignal): Promise<LlmExchange> => this.get(`/api/exchanges/${id}/llm`, { signal }),

    /** `GET /api/exchanges/:id/snippet?target=`. */
    snippet: (id: ExchangeId, target: CodegenTarget, signal?: AbortSignal): Promise<Snippet> =>
      this.get(`/api/exchanges/${id}/snippet`, { query: { target }, signal }),

    /** `POST /api/exchanges/delete`. */
    delete: (ids: ExchangeId[]): Promise<void> => this.post('/api/exchanges/delete', { ids }),

    /** `POST /api/exchanges/clear`. */
    clear: (options: { keepPinned?: boolean } = {}): Promise<void> => this.post('/api/exchanges/clear', options),

    /** `PATCH /api/exchanges` — pin, colour tag, note. */
    patch: (patch: ExchangePatch): Promise<void> => this.patch('/api/exchanges', patch),

    /** `POST /api/exchanges/:id/replay` (omit `request` to resend as captured). */
    replay: (id: ExchangeId, body: { request?: EditableRequest; options?: SendOptions } = {}): Promise<SendResponse> =>
      this.post(`/api/exchanges/${id}/replay`, body, { parseJson: parseSendResponseJson }),

    /** `GET /api/exchanges/:id/editable/request`. */
    editableRequest: (id: ExchangeId, signal?: AbortSignal): Promise<EditableRequest> =>
      this.get(`/api/exchanges/${id}/editable/request`, { signal }),

    /** `GET /api/exchanges/:id/editable/response`. */
    editableResponse: (id: ExchangeId, signal?: AbortSignal): Promise<EditableResponse> =>
      this.get(`/api/exchanges/${id}/editable/response`, { signal }),

    /**
     * `POST /api/exchanges/headers` — the values of chosen request / response headers of many
     * exchanges (at most {@link BATCH_LIMIT}). Unknown ids are left out.
     */
    headerValues: (ids: readonly ExchangeId[], names: { request?: readonly string[]; response?: readonly string[] }, signal?: AbortSignal): Promise<{ exchanges: HeaderValues[] }> =>
      this.post('/api/exchanges/headers', { ids, request: names.request ?? [], response: names.response ?? [] }, { signal }),

    /** `POST /api/exchanges/timings` — phase timings of many exchanges (at most {@link BATCH_LIMIT}). */
    timings: (ids: readonly ExchangeId[], signal?: AbortSignal): Promise<{ timings: ExchangeTiming[] }> => this.post('/api/exchanges/timings', { ids }, { signal }),
  };

  // ===================================================================================
  // Filter language
  // ===================================================================================

  readonly filter = {
    /** `POST /api/filter/validate`. Error offsets are UTF-8 byte offsets. */
    validate: (query: string, signal?: AbortSignal): Promise<FilterValidation> =>
      this.post('/api/filter/validate', { query }, { signal }),
    /** `POST /api/filter/suggest` — `cursor` is a UTF-8 byte offset. */
    suggest: (query: string, cursor: number, signal?: AbortSignal): Promise<Suggestion[]> =>
      this.post('/api/filter/suggest', { query, cursor }, { signal }),
    /** `POST /api/filter/query`. */
    query: (query: string, signal?: AbortSignal): Promise<IdsResult> => this.post('/api/filter/query', { query }, { signal }),
    /** `GET /api/filter/docs`. */
    docs: (signal?: AbortSignal): Promise<FieldDoc[]> => this.get('/api/filter/docs', { signal }),
  };

  // ===================================================================================
  // Rules
  // ===================================================================================

  readonly rules = {
    get: (signal?: AbortSignal): Promise<RuleSet> => this.get('/api/rules', { signal }),
    /** Applies immediately & persists; per-rule validation errors are returned, not thrown. */
    put: (rules: RuleSet): Promise<RulesPutResult> => this.put('/api/rules', rules),
    stats: (signal?: AbortSignal): Promise<RuleStats[]> => this.get('/api/rules/stats', { signal }),
    /** Zero every rule's hit counter on the daemon (and re-arm match limits). */
    resetStats: (): Promise<RuleStats[]> => this.post('/api/rules/stats/reset', {}),
    /** Dry run against an exchange or an ad-hoc request. */
    test: (body: RulesTestRequest): Promise<RulesTestResult> => this.post('/api/rules/test', body),
    /** Download URL for `akev-rules.json`. */
    exportUrl: (): string => this.url('/api/rules/export'),
    /** Convert an Akev or HTTP Toolkit rules export (not applied). */
    import: (data: unknown): Promise<RuleSet> =>
      typeof data === 'string' || data instanceof Uint8Array
        ? this.post('/api/rules/import', data, { contentType: 'application/json' })
        : this.post('/api/rules/import', data),
  };

  // ===================================================================================
  // Breakpoints
  // ===================================================================================

  readonly breakpoints = {
    list: (signal?: AbortSignal): Promise<PausedExchange[]> => this.get('/api/breakpoints', { signal }),
    resume: (id: ExchangeId, resume: RequestResume | ResponseResume): Promise<void> =>
      this.post(`/api/breakpoints/${id}/resume`, resume),
    resumeAll: (): Promise<void> => this.post('/api/breakpoints/resume-all'),
    /** WebSocket messages paused at `ws-breakpoint` rules. */
    messages: (signal?: AbortSignal): Promise<PausedMessage[]> => this.get('/api/breakpoints/messages', { signal }),
    resumeMessage: (pauseId: number, resume: MessageResume): Promise<void> => this.post(`/api/breakpoints/messages/${pauseId}/resume`, resume),
    /** Forward (or drop) every paused message, or only those of exchange `id`. */
    resumeAllMessages: (options: { id?: ExchangeId; action?: 'forward' | 'drop' } = {}): Promise<{ resumed: number }> =>
      this.post('/api/breakpoints/messages/resume-all', options),
  };

  // ===================================================================================
  // Compose
  // ===================================================================================

  /** `POST /api/send`. */
  send(request: EditableRequest, options?: SendOptions, signal?: AbortSignal): Promise<SendResponse> {
    return this.post('/api/send', { request, options }, { signal, parseJson: parseSendResponseJson });
  }

  /**
   * `POST /api/send/repeat` — send a request (or resend an exchange as captured) `count` times,
   * `concurrency` at a time. `onResult` is called as each request finishes; aborting `signal`
   * cancels the run on the daemon. Resolves with the final line.
   */
  async repeat(params: RepeatParams, onResult: (line: RepeatResult) => void, signal?: AbortSignal): Promise<RepeatDone> {
    const response = await this.request<Response>('POST', '/api/send/repeat', { body: params, signal, responseType: 'response' });
    const reader = response.body?.getReader();
    if (!reader) throw new ApiError(response.status, 'bad-response', 'Akev sent no repeat results', 'POST', '/api/send/repeat');
    const decoder = new TextDecoder();
    let buffered = '';
    let done: RepeatDone | null = null;
    const handle = (line: string) => {
      if (!line.trim()) return;
      const v = JSON.parse(line) as RepeatResult | RepeatDone;
      if ('done' in v) done = v;
      else onResult(v);
    };
    // Cancelling the body stops the run (a fetch abort does too, but not every transport's).
    const onAbort = () => void reader.cancel().catch(() => undefined);
    signal?.addEventListener('abort', onAbort, { once: true });
    try {
      for (;;) {
        const chunk = await reader.read();
        if (chunk.done) break;
        buffered += decoder.decode(chunk.value, { stream: true });
        let nl = buffered.indexOf('\n');
        while (nl >= 0) {
          handle(buffered.slice(0, nl));
          buffered = buffered.slice(nl + 1);
          nl = buffered.indexOf('\n');
        }
      }
      handle(buffered + decoder.decode());
    } catch (e) {
      if (signal?.aborted) throw new ApiError(0, 'aborted', 'Request aborted', 'POST', '/api/send/repeat');
      if (isApiError(e)) throw e;
      throw new ApiError(0, 'network', `Couldn't read the repeat results (${errorMessage(e)})`, 'POST', '/api/send/repeat');
    } finally {
      signal?.removeEventListener('abort', onAbort);
      reader.releaseLock();
    }
    if (signal?.aborted) throw new ApiError(0, 'aborted', 'Request aborted', 'POST', '/api/send/repeat');
    if (!done) throw new ApiError(0, 'network', 'The connection to Akev closed before the run finished', 'POST', '/api/send/repeat');
    return done;
  }

  /** `POST /api/import/curl`. */
  importCurl(command: string): Promise<EditableRequest> {
    return this.post('/api/import/curl', { command });
  }

  readonly codegen = {
    targets: (signal?: AbortSignal): Promise<TargetInfo[]> => this.get('/api/codegen/targets', { signal }),
    generate: (request: EditableRequest, target: CodegenTarget, signal?: AbortSignal): Promise<Snippet> =>
      this.post('/api/codegen', { request, target }, { signal }),
  };

  // ===================================================================================
  // Interceptors
  // ===================================================================================

  readonly interceptors = {
    list: (signal?: AbortSignal): Promise<InterceptorInfo[]> => this.get('/api/interceptors', { signal }),
    activate: (id: string, options?: unknown): Promise<ActivationResult> =>
      this.post(`/api/interceptors/${encodeURIComponent(id)}/activate`, { options }),
    deactivate: (id: string, options?: unknown): Promise<void> =>
      this.post(`/api/interceptors/${encodeURIComponent(id)}/deactivate`, { options }),
    /** `GET /api/setup/terminal?shell=` as text. */
    terminalScript: (shell: TerminalShell, signal?: AbortSignal): Promise<string> =>
      this.get('/api/setup/terminal', { query: { shell }, responseType: 'text', signal }),
    /**
     * URL of the terminal setup script for a copyable one-liner. Never carries the token: the
     * one-liner sends it as a header, so it stays off curl's command line.
     */
    terminalScriptUrl: (shell: TerminalShell): string => this.url('/api/setup/terminal', { shell }, { withToken: false }),
  };

  // ===================================================================================
  // Settings & certificate
  // ===================================================================================

  readonly settings = {
    get: (signal?: AbortSignal): Promise<Settings> => this.get('/api/settings', { signal }),
    put: (settings: Settings): Promise<Settings> => this.put('/api/settings', settings),
    /** Replaces `settings.ui` wholesale. */
    patchUi: (ui: unknown): Promise<void> => this.patch('/api/settings/ui', ui),
    /** `GET /api/tls-profiles` — upstream TLS fingerprint profiles and whether this build has them. */
    tlsProfiles: (signal?: AbortSignal): Promise<TlsProfiles> => this.get('/api/tls-profiles', { signal }),
  };

  readonly updates = {
    /** The last check (re-checked first when a day old and automatic checks are on). */
    get: (signal?: AbortSignal): Promise<UpdateInfo> => this.get('/api/update', { signal }),
    /** Check now. */
    check: (): Promise<UpdateInfo> => this.post('/api/update/check'),
  };

  readonly ca = {
    get: (signal?: AbortSignal): Promise<CaInfo> => this.get('/api/ca', { signal }),
    pemUrl: (): string => this.url('/api/ca/cert.pem'),
    derUrl: (): string => this.url('/api/ca/cert.der'),
    regenerate: (): Promise<CaInfo> => this.post('/api/ca/regenerate'),
    /** Whether the operating system's certificate stores trust the CA. */
    trust: (signal?: AbortSignal): Promise<TrustReport> => this.get('/api/ca/trust', { signal }),
    /** Install the CA in a store; the operating system asks the user to confirm. */
    trustInstall: (store: string): Promise<TrustReport> => this.post('/api/ca/trust', { store }),
    /** Remove the CA from a store; the operating system asks the user to confirm. */
    trustRemove: (store: string): Promise<TrustReport> => this.request('DELETE', '/api/ca/trust', { body: { store } }),
  };

  // ===================================================================================
  // Import / export
  // ===================================================================================

  readonly transfer = {
    /** HAR download URL (all, a selection, or a filter; `redact` replaces secrets). */
    harUrl: (params: { ids?: ExchangeId[]; filter?: string; redact?: boolean } = {}): string =>
      this.url('/api/export/har', { ids: params.ids?.length ? params.ids : undefined, filter: params.filter || undefined, redact: params.redact ? 1 : undefined }),
    /** Fetch a HAR export as text (`GET /api/export/har?ids=&filter=&redact=`), e.g. to merge chunks. */
    har: (params: { ids?: ExchangeId[]; filter?: string; redact?: boolean } = {}, signal?: AbortSignal): Promise<string> =>
      this.get('/api/export/har', {
        query: { ids: params.ids?.length ? params.ids : undefined, filter: params.filter || undefined, redact: params.redact ? 1 : undefined },
        responseType: 'text',
        signal,
      }),
    importHar: (data: Uint8Array | Blob | string): Promise<IdsResult> =>
      this.post('/api/import/har', data, { contentType: 'application/json' }),
    sessionUrl: (params: { ids?: ExchangeId[]; filter?: string; redact?: boolean } = {}): string =>
      this.url('/api/export/session', { ids: params.ids?.length ? params.ids : undefined, filter: params.filter || undefined, redact: params.redact ? 1 : undefined }),
    importSession: (data: Uint8Array | Blob): Promise<IdsResult> => this.post('/api/import/session', data),
    /** Charles Proxy JSON session (`.chlsj`). */
    importCharles: (data: Uint8Array | Blob | string): Promise<IdsResult> =>
      this.post('/api/import/charles', data, { contentType: 'application/json' }),
    /** mitmproxy flow file (`mitmdump -w`). */
    importMitmproxy: (data: Uint8Array | Blob): Promise<IdsResult> => this.post('/api/import/mitmproxy', data),
    /** Fiddler session archive (`.saz`). */
    importFiddler: (data: Uint8Array | Blob): Promise<IdsResult> => this.post('/api/import/fiddler', data),
  };

  // ===================================================================================
  // Session library
  // ===================================================================================

  readonly sessions = {
    /** Saved sessions, most recently updated first. */
    list: (signal?: AbortSignal): Promise<SessionInfo[]> => this.get('/api/sessions', { signal }),
    /** Save the current traffic (all, ids and/or filter) as a new session. */
    save: (request: SaveSessionRequest): Promise<SessionInfo> => this.post('/api/sessions', request),
    /** Rename (an autosave becomes a saved session with a new id: use the returned info). */
    rename: (id: string, name: string): Promise<SessionInfo> => this.put(`/api/sessions/${encodeURIComponent(id)}`, { name }),
    remove: (id: string): Promise<void> => this.request('DELETE', `/api/sessions/${encodeURIComponent(id)}`),
    /** Load into the traffic list, replacing it or adding to it. */
    open: (id: string, mode: SessionOpenMode): Promise<IdsResult> => this.post(`/api/sessions/${encodeURIComponent(id)}/open`, { mode }),
    /** Download URL of the `.akev` file. */
    fileUrl: (id: string): string => this.url(`/api/sessions/${encodeURIComponent(id)}`),
  };

  // ===================================================================================
  // Proxy listener
  // ===================================================================================

  readonly proxy = {
    /** Move the proxy to exactly this port (409 when it's busy); settings are unchanged. */
    rebind: (request: ProxyRebindRequest): Promise<ProxyStatus> => this.post('/api/proxy/rebind', request),
  };

  // ===================================================================================
  // Transparent capture (Linux)
  // ===================================================================================

  readonly transparent = {
    /** `GET /api/transparent`: listener status, redirect detection, system facts. */
    status: (signal?: AbortSignal): Promise<TransparentInfo> => this.get('/api/transparent', { signal }),
    /** `POST /api/transparent/plan`: the firewall commands for a capture mode (nothing is run). */
    plan: (request: TransparentPlanRequest, signal?: AbortSignal): Promise<TransparentPlan> => this.post('/api/transparent/plan', request, { signal }),
  };

  // ===================================================================================
  // Recording
  // ===================================================================================

  readonly capture = {
    get: (signal?: AbortSignal): Promise<CaptureState> => this.get('/api/capture', { signal }),
    set: (recording: boolean): Promise<CaptureState> => this.put('/api/capture', { recording }),
  };

  // ===================================================================================
  // WireGuard mode
  // ===================================================================================

  readonly wireguard = {
    /** Status, keys and the client configuration; `host` picks the endpoint written into it. */
    get: (host?: string, signal?: AbortSignal): Promise<WireguardInfo> => this.get('/api/wireguard', { query: { host: host || undefined }, signal }),
    /** Change `settings.proxy.wireguard` (persisted, applied live). */
    update: (patch: WireguardPatch): Promise<WireguardInfo> => this.put('/api/wireguard', patch),
    /** New key pairs: every configuration handed out before stops working. */
    regenerate: (): Promise<WireguardInfo> => this.post('/api/wireguard/regenerate'),
    /** Download URL of the `.conf` file. */
    configUrl: (host?: string): string => this.url('/api/wireguard/config', { host: host || undefined }),
  };

  // ===================================================================================
  // Protobuf schemas
  // ===================================================================================

  readonly protos = {
    /** `GET /api/protos` — files (with services, messages, compile errors), mappings, message types. */
    list: (signal?: AbortSignal): Promise<ProtoRegistry> => this.get('/api/protos', { signal }),
    /** `POST /api/protos` — add or replace files by name; files that don't compile are kept with their errors. */
    upload: (files: ProtoUploadFile[]): Promise<ProtoRegistry> => this.post('/api/protos', { files }),
    /** `DELETE /api/protos?name=` — one file. */
    remove: (name: string): Promise<ProtoRegistry> => this.request('DELETE', '/api/protos', { query: { name } }),
    /** `DELETE /api/protos?all=1` — every file (mappings are kept). */
    removeAll: (): Promise<ProtoRegistry> => this.request('DELETE', '/api/protos', { query: { all: true } }),
    /** `GET /api/protos/file?name=` — a `.proto` file's text. */
    source: (name: string, signal?: AbortSignal): Promise<string> => this.get('/api/protos/file', { query: { name }, responseType: 'text', signal }),
    /** `GET /api/protos/file?name=` — any file's bytes (e.g. to restore it after a delete). */
    bytes: (name: string, signal?: AbortSignal): Promise<Uint8Array> => this.get('/api/protos/file', { query: { name }, responseType: 'bytes', signal }),
    mappings: (signal?: AbortSignal): Promise<ProtoMapping[]> => this.get('/api/protos/mappings', { signal }),
    /** `PUT /api/protos/mappings` — replace the mappings (validated; ids filled in). */
    setMappings: (mappings: ProtoMapping[]): Promise<ProtoMapping[]> => this.put('/api/protos/mappings', mappings),
  };
}

/** The shared client instance used by the whole UI. */
export const api = new ApiClient();
