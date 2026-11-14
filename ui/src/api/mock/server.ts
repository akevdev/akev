/**
 * In-browser mock of the Akev daemon: implements every REST endpoint of the HTTP API reference and the
 * live stream (hello → batched `exchanges` at ≤30 Hz with per-connection server-side
 * filtering, `messages`, `paused`/`resumed`, `stats`, `rule-stats`, ...).
 *
 * It plugs into the real API client as a {@link Transport}, so the UI exercises exactly the
 * same code paths as against the daemon. Dev helpers are exposed as `window.__akevMock`.
 */

import { base64ToBytes, bytesToBase64, textToBase64, utf8Decode, utf8Encode } from '../../lib/encoding';
import { statusText } from '../../lib/http';
import type { Transport, WebSocketLike } from '../client';
import { MockWireguard } from './wireguard';
import type { WireguardPatch, WireguardSettings } from '../types';
import type {
  ActivationResult,
  BodyView,
  CaInfo,
  TrustReport,
  ClientMessage,
  Diagnostics,
  EditableRequest,
  EditableResponse,
  ExchangeDetail,
  ExchangeSummary,
  Http3ListenerStatus,
  HttpVersion,
  InterceptorInfo,
  PausedExchange,
  ProtoMapping,
  ProtoUploadFile,
  ProxyRebindRequest,
  RequestResume,
  ResponseResume,
  RestoredSession,
  RuleItem,
  RuleSet,
  RuleStats,
  SaveSessionRequest,
  SendOptions,
  SendResult,
  ServerMessage,
  SessionInfo,
  SessionOpenMode,
  Settings,
  Status,
  StatusStats,
} from '../types';
import type { TransparentPlanRequest } from '../types';
import { BATCH_LIMIT, REPEAT_LIMITS, type RepeatParams } from '../types';
import { buildBodyView, wireSizeFor } from './bodies';
import { generateSnippet } from './codegen';
import { mockJvmActivate, mockJvmDeactivate, withMockJvm } from './jvm';
import { CODEGEN_TARGETS, FILTER_DOCS, mockCaInfo, mockInterceptors, mockRules, mockSessions, mockSettings } from './data';
import { MockTransparent } from './transparent';
import { compileFilter, suggestFilter, type FilterSubject, type Predicate } from './filter';
import { advance, createRecord, resume, scenarioIndex, type MockRecord } from './generator';
import { LLM_FIXTURES, llmView } from './llm';
import { MockProtoRegistry, mockGrpcTrailers } from './protos';
import { Rng } from './rng';
import { MockWsTools } from './wsTools';
import { mockImpersonationAvailable, mockTlsProfiles } from './tlsProfiles';
import { MockLicense, MockLicenseError, mockLicenseModeFromQuery, type MockLicenseMode } from './license';

// =====================================================================================
// Socket
// =====================================================================================

class MockSocket implements WebSocketLike {
  readyState = 0;
  onopen: ((ev: Event) => void) | null = null;
  onmessage: ((ev: MessageEvent) => void) | null = null;
  onclose: ((ev: CloseEvent) => void) | null = null;
  onerror: ((ev: Event) => void) | null = null;
  /** `undefined` = no filter ever set; `null` predicate = matches everything. */
  filter: Predicate | null | undefined = undefined;
  matching: Set<number> | null = null;
  /** Bumped per `hello`: older `hello-more` chunks of a superseded snapshot stop. */
  helloToken = 0;

  constructor(private readonly server: MockServer) {
    setTimeout(() => {
      if (this.readyState !== 0) return;
      if (this.server.refuseConnections) {
        this.readyState = 3;
        this.onclose?.({ code: 1006, reason: '', wasClean: false } as CloseEvent);
        return;
      }
      this.readyState = 1;
      this.onopen?.({ type: 'open' } as Event);
      this.server.handleOpen(this);
    }, 40);
  }

  send(data: string): void {
    if (this.readyState !== 1) throw new Error('socket not open');
    this.server.handleClientMessage(this, data);
  }

  close(code = 1000, reason = ''): void {
    if (this.readyState >= 2) return;
    this.readyState = 3;
    this.server.handleClose(this);
    setTimeout(() => this.onclose?.({ code, reason, wasClean: code === 1000 } as CloseEvent), 0);
  }

  /** Server-side drop (simulates the daemon going away). */
  drop(): void {
    if (this.readyState >= 2) return;
    this.readyState = 3;
    this.server.handleClose(this);
    this.onclose?.({ code: 1006, reason: 'connection lost', wasClean: false } as CloseEvent);
  }

  deliver(msg: ServerMessage): void {
    if (this.readyState !== 1) return;
    // Round-trip through JSON exactly like the real wire.
    this.onmessage?.({ data: JSON.stringify(msg) } as MessageEvent);
  }
}

// =====================================================================================
// HTTP plumbing
// =====================================================================================

interface MockResponse {
  status?: number;
  json?: unknown;
  text?: string;
  bytes?: Uint8Array;
  /** A streamed body (`application/x-ndjson` unless `contentType` says otherwise). */
  stream?: ReadableStream<Uint8Array>;
  contentType?: string;
}

class HttpError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    /** Extra fields of the error object (e.g. `feature` for `pro-required`). */
    readonly details: Record<string, unknown> = {},
  ) {
    super(message);
  }
}

type Handler = (m: RegExpExecArray, url: URL, body: unknown, raw: Uint8Array | null) => MockResponse | Promise<MockResponse>;

const MAX_RECORDS = 3000;
/** Newest summaries in `hello`; older ones follow in `hello-more` chunks (as the daemon pages). */
const HELLO_FIRST_PAGE = 2000;
const HELLO_CHUNK = 5000;
/** `GET /messages` page size: default and max (as the daemon). */
const MESSAGES_PAGE = 1000;
const MESSAGES_PAGE_MAX = 10_000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function toResponse(r: MockResponse): Response {
  const status = r.status ?? 200;
  if (status === 204) return new Response(null, { status });
  if (r.stream) return new Response(r.stream, { status, headers: { 'content-type': r.contentType ?? 'application/x-ndjson' } });
  if (r.bytes) return new Response(r.bytes.slice().buffer as ArrayBuffer, { status, headers: { 'content-type': r.contentType ?? 'application/octet-stream' } });
  if (r.text != null) return new Response(r.text, { status, headers: { 'content-type': r.contentType ?? 'text/plain; charset=utf-8' } });
  return new Response(JSON.stringify(r.json ?? null), { status, headers: { 'content-type': 'application/json' } });
}

function ruleTitle(rules: RuleSet, id: string): string | undefined {
  const walk = (items: RuleItem[]): string | undefined => {
    for (const i of items) {
      if (i.type === 'rule' && i.id === id) return i.title;
      if (i.type === 'group') {
        const t = walk(i.items);
        if (t) return t;
      }
    }
    return undefined;
  };
  return walk(rules.items);
}

function firstRuleId(rules: RuleSet): string | null {
  for (const i of rules.items) {
    if (i.type === 'rule' && i.enabled) return i.id;
    if (i.type === 'group' && i.enabled) {
      const inner = firstRuleId({ version: 1, items: i.items });
      if (inner) return inner;
    }
  }
  return null;
}

// =====================================================================================
// Server
// =====================================================================================

export class MockServer implements Transport {
  /** When true, new stream sockets fail (simulates the daemon being down). */
  refuseConnections = false;

  private readonly rng = new Rng(0x0a4e70);
  private readonly records = new Map<number, MockRecord>();
  private order: number[] = [];
  private nextId = 1;
  private readonly live = new Set<number>();
  private readonly sockets = new Set<MockSocket>();
  private readonly pendingAdded = new Set<number>();
  private readonly pendingUpdated = new Set<number>();
  private rules: RuleSet = mockRules();
  private readonly ruleStats = new Map<string, RuleStats>();
  private readonly dirtyRuleStats = new Set<string>();
  private settings: Settings = mockSettings();
  private interceptors: InterceptorInfo[];
  private recording = true;
  private readonly paused = new Map<number, PausedExchange>();
  private readonly startedAt = Date.now();
  private ca: CaInfo = mockCaInfo(Date.now());
  private readonly platform: 'linux' | 'macos' | 'windows';
  private totalConnections = 0;
  /** Exchanges evicted by the record cap (`StatusStats.evicted`). */
  private evictedCount = 0;
  private readonly helloPage: number;
  private readonly helloChunk: number;
  private intervals: ReturnType<typeof setInterval>[] = [];
  private genTimer: ReturnType<typeof setTimeout> | null = null;
  private routes: [string, RegExp, Handler][] = [];
  private readonly wsTools = new MockWsTools({
    record: (id) => this.records.get(id),
    broadcast: (msg) => this.broadcast(msg),
    touched: (id) => {
      if (!this.pendingAdded.has(id)) this.pendingUpdated.add(id);
    },
  });
  private objectUrls: string[] = [];
  private readonly protos = new MockProtoRegistry(Date.now());
  private sessions: SessionInfo[] = mockSessions(Date.now());
  /** Set with `?restored` (shows the startup "restored" toast). */
  private restoredSession: RestoredSession | undefined;
  /** Live proxy port when moved with `/api/proxy/rebind` (otherwise the settings' port). */
  private livePort: number | null = null;
  private readonly wireguard = new MockWireguard(() => this.settings.proxy.wireguard ?? { enabled: false, port: 51820 });
  /** Akev Pro license (`?pro`, `?trial`, `?trialEnded`, `?updatesEnded`, `?invalidLicense`). */
  private readonly license: MockLicense;
  private trialTimer: ReturnType<typeof setTimeout> | null = null;
  /** `?still`: no new traffic (screenshots; pair with a fixed clock so nothing in flight moves). */
  private readonly still: boolean = typeof window !== 'undefined' && /[?&]still(=|&|$)/.test(`${window.location.search}&${window.location.hash.split('?')[1] ?? ''}`);
  /** `?empty`: a fresh install (no history, nothing active) until an interceptor is launched. */
  private readonly empty: boolean = typeof window !== 'undefined' && /[?&]empty(=|&|$)/.test(`${window.location.search}&${window.location.hash.split('?')[1] ?? ''}`);
  /** Transparent capture listener (`?transparent-active` simulates redirected traffic). */
  private readonly transparent = new MockTransparent(typeof window !== 'undefined' && /[?&]transparent-active(=|&|$)/.test(`${window.location.search}&${window.location.hash.split('?')[1] ?? ''}`));

  /** `license` overrides the URL flags (tests); `helloPage`/`helloChunk` shrink the paged `hello` (tests). */
  constructor(options: { license?: MockLicenseMode; helloPage?: number; helloChunk?: number } = {}) {
    this.helloPage = options.helloPage ?? HELLO_FIRST_PAGE;
    this.helloChunk = options.helloChunk ?? HELLO_CHUNK;
    const ua = typeof navigator !== 'undefined' ? navigator.userAgent : '';
    this.platform = /Mac/.test(ua) ? 'macos' : /Windows/.test(ua) ? 'windows' : 'linux';
    this.interceptors = mockInterceptors(this.platform);
    this.interceptors = withMockJvm(this.interceptors);
    if (this.empty) {
      this.interceptors = this.interceptors.map((i) => ({ ...i, active: false }));
      if (this.settings.sessions) this.settings = { ...this.settings, sessions: { ...this.settings.sessions, autosave: false } };
    }
    const query = typeof window !== 'undefined' ? `${window.location.search}&${window.location.hash.split('?')[1] ?? ''}` : '';
    this.license = new MockLicense(options.license ?? mockLicenseModeFromQuery(query));
    // Free installs never had the transparent listener on (the seeded settings have it on).
    const transparent = this.settings.proxy.transparent;
    if (!this.license.entitled('transparent')) this.settings = { ...this.settings, proxy: { ...this.settings.proxy, transparent: { ...transparent, enabled: false } } };
    this.registerRoutes();
    this.registerSessionRoutes();
    this.registerLicenseRoutes();
    // `?fallback`: the proxy "fell back" from the stored port (Settings → Proxy "Use port").
    if (/[?&]fallback(=|&|$)/.test(query)) this.livePort = this.settings.proxy.port + 1;
    // `?wireguard`: WireGuard mode is on and a phone is connected.
    if (/[?&]wireguard(=|&|$)/.test(query)) this.setWireguard({ enabled: true, port: 51820 }, true);
    if (/[?&]restored(=|&|$)/.test(query)) {
      const autosave = this.sessions.find((x) => x.id === 'autosave');
      if (autosave) this.restoredSession = { id: autosave.id, name: autosave.name, exchanges: autosave.exchanges, savedAt: autosave.updatedAt };
    }
  }

  // ---------------------------------------------------------------- lifecycle

  /** Seed history and start generating live traffic. */
  start(): void {
    if (!this.empty) this.seedHistory();
    this.intervals.push(setInterval(() => this.tick(), 40));
    this.intervals.push(setInterval(() => this.flush(), 33));
    this.intervals.push(setInterval(() => this.broadcast({ type: 'stats', stats: this.stats() }), 2000));
    this.intervals.push(setInterval(() => this.flushRuleStats(), 500));
    if (!this.empty && !this.still) this.scheduleGeneration();
    this.scheduleTrialEnd();
  }

  /** Stop all timers and close sockets. */
  stop(): void {
    for (const i of this.intervals) clearInterval(i);
    this.intervals = [];
    if (this.genTimer) clearTimeout(this.genTimer);
    if (this.trialTimer) clearTimeout(this.trialTimer);
    for (const s of [...this.sockets]) s.drop();
    for (const u of this.objectUrls) URL.revokeObjectURL(u);
  }

  /** Start generating traffic (a `?empty` mock waits for an interceptor launch). */
  startTraffic(): void {
    if (!this.genTimer && !this.still) this.scheduleGeneration();
  }

  /** Drop every stream connection (dev helper to test reconnect UI). */
  dropConnections(refuseForMs = 0): void {
    if (refuseForMs > 0) {
      this.refuseConnections = true;
      setTimeout(() => (this.refuseConnections = false), refuseForMs);
    }
    for (const s of [...this.sockets]) s.drop();
  }

  private seedHistory(): void {
    const now = Date.now();
    const count = 240;
    const forced = [
      scenarioIndex((h, _m, k) => k === 'web-socket' && h === 'localhost'),
      scenarioIndex((_h, _m, k) => k === 'event-stream'),
      scenarioIndex((_h, _m, k) => k === 'tls-failure'),
      scenarioIndex((_h, _m, k) => k === 'tunnel'),
      scenarioIndex((h) => h === 'grpc.example.com'),
      scenarioIndex((h) => h === 'api.down.example'),
      scenarioIndex((h) => h === 'expired.badssl.com'),
    ];
    for (let i = 0; i < count; i++) {
      const start = now - (count - i) * 1500 - this.rng.int(0, 900);
      const f = i % 34 === 5 ? forced[(i / 34) | 0] : undefined;
      const rec = createRecord(this.nextId++, this.rng, start, f != null && f >= 0 ? { scenario: f } : {});
      // Historic breakpoints were resumed long ago, except the last login.
      if (rec.plan.pause && i < count - 12) rec.plan.pause = undefined;
      this.insert(rec, false);
      this.runUntil(rec, now);
      // Recorded LLM API calls (OpenAI, Anthropic, Gemini, Ollama) among the recent traffic.
      // Extra records: they don't draw from the shared RNG, so the rest of the history is stable.
      const llmSlot = i - (count - 40);
      const fixture = llmSlot >= 0 && llmSlot % 5 === 0 ? LLM_FIXTURES[llmSlot / 5] : undefined;
      if (fixture) {
        const extra = createRecord(this.nextId++, this.rng, start + 700, { llm: fixture });
        this.insert(extra, false);
        this.runUntil(extra, now);
      }
    }
    // Always start with one request paused at a breakpoint, so breakpoint UIs (nav badge,
    // paused banner, editors) can be built and screenshotted deterministically.
    if (this.paused.size === 0) {
      const login = scenarioIndex((h, m) => h === 'localhost' && m === 'POST');
      const rec = createRecord(this.nextId++, this.rng, now - 4000, login >= 0 ? { scenario: login } : {});
      rec.plan.pause = 'request';
      this.insert(rec, false);
      this.runUntil(rec, now);
    }
    // A live WebSocket with a message held by a ws-breakpoint rule (WebSocket tools UIs).
    const chat = scenarioIndex((h, _m, k) => k === 'web-socket' && h === 'localhost');
    if (chat >= 0) {
      const rec = createRecord(this.nextId++, this.rng, now - 9000, { scenario: chat });
      rec.plan.streamUntil = undefined;
      this.insert(rec, false);
      this.runUntil(rec, now);
      this.wsTools.pauseSample(rec, now - 2500);
    }
    // A couple of pinned / highlighted / noted exchanges for variety.
    const ids = this.order;
    for (const [idx, patch] of [
      [12, { pinned: true }],
      [40, { color: 'red' as const }],
      [41, { color: 'green' as const, hasNote: true }],
      [120, { pinned: true, color: 'blue' as const }],
      [200, { color: 'yellow' as const }],
    ] as const) {
      const id = ids[idx];
      const rec = id != null ? this.records.get(id) : undefined;
      if (rec) {
        rec.summary = { ...rec.summary, ...patch };
        if ('hasNote' in patch) rec.note = 'Investigate the slow TTFB here';
      }
    }
  }

  private runUntil(rec: MockRecord, now: number): void {
    let guard = 0;
    while (rec.nextAt != null && rec.nextAt <= now && guard++ < 200) {
      const at = rec.nextAt;
      const res = advance(rec, at, this.rng);
      if (res.paused) this.registerPause(rec, false);
    }
    if (rec.nextAt != null) this.live.add(rec.summary.id);
  }

  private scheduleGeneration(): void {
    const delay = this.rng.chance(0.06) ? this.rng.int(1200, 2600) : this.rng.int(120, 520);
    this.genTimer = setTimeout(() => {
      if (this.recording) {
        const burst = this.rng.chance(0.08) ? this.rng.int(4, 10) : 1;
        for (let i = 0; i < burst; i++) {
          const at = Date.now() + i * this.rng.int(5, 60);
          // Now and then an agent calls an LLM API (streams live into the LLM tab).
          const llm = this.rng.chance(0.025) ? this.rng.pick(LLM_FIXTURES) : undefined;
          const rec = createRecord(this.nextId++, this.rng, at, llm ? { llm } : {});
          this.insert(rec, true);
        }
      }
      this.scheduleGeneration();
    }, delay);
  }

  private insert(rec: MockRecord, live: boolean): void {
    const id = rec.summary.id;
    this.records.set(id, rec);
    this.order.push(id);
    this.totalConnections++;
    if (rec.summary.rule) this.bumpRule(rec.summary.rule, rec.summary.startedAt);
    if (live) {
      this.live.add(id);
      this.pendingAdded.add(id);
    }
    if (this.order.length > MAX_RECORDS) this.evict(200);
  }

  private evict(n: number): void {
    const victims: number[] = [];
    for (const id of this.order) {
      if (victims.length >= n) break;
      const rec = this.records.get(id);
      if (rec && !rec.summary.pinned && rec.phase === 'done') victims.push(id);
    }
    this.evictedCount += victims.length;
    this.removeIds(victims);
  }

  private removeIds(ids: number[]): void {
    if (ids.length === 0) return;
    const set = new Set(ids);
    for (const id of ids) {
      this.records.delete(id);
      this.live.delete(id);
      this.pendingAdded.delete(id);
      this.pendingUpdated.delete(id);
      if (this.paused.delete(id)) this.broadcast({ type: 'resumed', id });
    }
    this.wsTools.forget(ids);
    this.order = this.order.filter((id) => !set.has(id));
    for (const s of this.sockets) if (s.matching) for (const id of ids) s.matching.delete(id);
    this.broadcast({ type: 'removed', ids });
  }

  private bumpRule(ruleId: string, at: number): void {
    const prev = this.ruleStats.get(ruleId);
    this.ruleStats.set(ruleId, { ruleId, matches: (prev?.matches ?? 0) + 1, lastMatchedAt: at });
    this.dirtyRuleStats.add(ruleId);
  }

  private flushRuleStats(): void {
    if (this.dirtyRuleStats.size === 0) return;
    const stats = [...this.dirtyRuleStats].map((id) => this.ruleStats.get(id)).filter((s): s is RuleStats => s != null);
    this.dirtyRuleStats.clear();
    this.broadcast({ type: 'rule-stats', stats });
  }

  private registerPause(rec: MockRecord, announce: boolean): void {
    const p: PausedExchange = { id: rec.summary.id, stage: rec.summary.paused ?? 'request', pausedAt: Date.now() * 1000, request: this.editableRequest(rec) };
    this.paused.set(rec.summary.id, p);
    if (announce) this.broadcast({ type: 'paused', exchange: p });
  }

  private tick(): void {
    const now = Date.now();
    for (const id of [...this.live]) {
      const rec = this.records.get(id);
      if (!rec) {
        this.live.delete(id);
        continue;
      }
      let guard = 0;
      while (rec.nextAt != null && rec.nextAt <= now && guard++ < 10) {
        const res = advance(rec, now, this.rng);
        if (res.changed && !this.pendingAdded.has(id)) this.pendingUpdated.add(id);
        if (res.messages?.length) {
          this.broadcast({ type: 'messages', id, messages: res.messages.map(({ text: _t, base64: _b, ...m }) => m) });
        }
        if (res.paused) this.registerPause(rec, true);
      }
      if (rec.nextAt == null && rec.phase !== 'paused') this.live.delete(id);
    }
  }

  private flush(): void {
    if (this.pendingAdded.size === 0 && this.pendingUpdated.size === 0) return;
    const added = [...this.pendingAdded].map((id) => this.records.get(id)).filter((r): r is MockRecord => r != null);
    const updated = [...this.pendingUpdated].filter((id) => !this.pendingAdded.has(id)).map((id) => this.records.get(id)).filter((r): r is MockRecord => r != null);
    this.pendingAdded.clear();
    this.pendingUpdated.clear();
    for (const s of this.sockets) {
      const msg: ServerMessage = { type: 'exchanges', added: added.map((r) => r.summary), updated: updated.map((r) => r.summary) };
      if (s.filter) {
        const add: number[] = [];
        const remove: number[] = [];
        const matching = s.matching ?? new Set<number>();
        for (const rec of [...added, ...updated]) {
          const id = rec.summary.id;
          const m = s.filter(this.subject(rec));
          if (m && !matching.has(id)) {
            matching.add(id);
            add.push(id);
          } else if (!m && matching.has(id)) {
            matching.delete(id);
            remove.push(id);
          }
        }
        s.matching = matching;
        msg.filtered = { add, remove };
      }
      s.deliver(msg);
    }
  }

  private broadcast(msg: ServerMessage): void {
    for (const s of this.sockets) s.deliver(msg);
  }

  // ---------------------------------------------------------------- stream protocol

  handleOpen(socket: MockSocket): void {
    this.sockets.add(socket);
    // Like the daemon: the newest summaries first, the older ones in `hello-more` chunks.
    const older = this.order.slice(0, Math.max(0, this.order.length - this.helloPage));
    const token = ++socket.helloToken;
    socket.deliver({
      type: 'hello',
      status: this.status(),
      summaries: this.summariesOf(this.order.slice(older.length)),
      paused: [...this.paused.values()],
      rules: this.rules,
      settings: this.settings,
      pausedMessages: this.wsTools.paused(),
      remaining: older.length,
    });
    const step = () => {
      if (!this.sockets.has(socket) || socket.helloToken !== token || older.length === 0) return;
      // Read at send time: removed exchanges are skipped, summaries are current.
      const chunk = older.splice(Math.max(0, older.length - this.helloChunk));
      socket.deliver({ type: 'hello-more', summaries: this.summariesOf(chunk), remaining: older.length });
      if (older.length) setTimeout(step, 33);
    };
    if (older.length) setTimeout(step, 33);
  }

  private summariesOf(ids: readonly number[]): ExchangeSummary[] {
    return ids.map((id) => this.records.get(id)?.summary).filter((s): s is ExchangeSummary => s != null);
  }

  handleClose(socket: MockSocket): void {
    this.sockets.delete(socket);
  }

  handleClientMessage(socket: MockSocket, data: string): void {
    let msg: ClientMessage;
    try {
      msg = JSON.parse(data) as ClientMessage;
    } catch {
      return;
    }
    if (msg.type === 'ping') {
      socket.deliver({ type: 'pong' });
    } else if (msg.type === 'set-filter') {
      const compiled = compileFilter(msg.query);
      if (!compiled.ok) {
        // Like the daemon: keep the previous filter and report its ids (null = unfiltered).
        const ids = socket.filter ? [...(socket.matching ?? [])].sort((a, b) => a - b) : null;
        socket.deliver({ type: 'filter-result', seq: msg.seq, query: msg.query, ids, error: compiled.error });
        return;
      }
      socket.filter = compiled.predicate;
      if (!compiled.predicate) {
        socket.matching = null;
        socket.deliver({ type: 'filter-result', seq: msg.seq, query: msg.query, ids: null });
        return;
      }
      const pred = compiled.predicate;
      const ids = this.order.filter((id) => {
        const rec = this.records.get(id);
        return rec != null && pred(this.subject(rec));
      });
      socket.matching = new Set(ids);
      socket.deliver({ type: 'filter-result', seq: msg.seq, query: msg.query, ids });
    }
  }

  // ---------------------------------------------------------------- derived data

  private materialize(rec: MockRecord, side: 'request' | 'response'): string | Uint8Array | null {
    const spec = side === 'request' ? rec.reqSpec : rec.resSpec;
    if (!spec) return null;
    if (side === 'request') {
      rec.reqData ??= spec.make(new Rng(rec.seed));
      return rec.reqData;
    }
    rec.resData ??= spec.make(new Rng(rec.seed + 1));
    return rec.resData;
  }

  private subject(rec: MockRecord): FilterSubject {
    const text = (d: string | Uint8Array | undefined) => (typeof d === 'string' ? d : undefined);
    return {
      summary: rec.summary,
      url: rec.request.url,
      requestHeaders: rec.request.headers,
      responseHeaders: rec.response?.headers,
      requestBody: text(rec.reqData),
      responseBody: text(rec.resData),
      note: rec.note,
      clientIp: rec.client.remoteAddr.split(':')[0],
      ruleTitle: rec.summary.rule ? ruleTitle(this.rules, rec.summary.rule) : undefined,
    };
  }

  private stats(): StatusStats {
    let mem = 0;
    for (const rec of this.records.values()) mem += rec.summary.requestSize + rec.summary.responseSize;
    return {
      exchanges: this.records.size,
      memoryBodyBytes: Math.min(mem, this.settings.capture.memoryBudgetBytes),
      spilledBodyBytes: Math.max(0, mem - this.settings.capture.memoryBudgetBytes),
      activeConnections: this.live.size + 3,
      totalConnections: this.totalConnections,
      evicted: this.evictedCount,
      droppedBodies: 0,
      diskDroppedBodies: 0,
      passedThrough: 0,
      autoReleased: 0,
      spillDiskBytes: Math.round(Math.max(0, mem - this.settings.capture.memoryBudgetBytes) / 3),
      refusedConnections: 0,
      authFailures: 0,
    };
  }

  /** `GET /api/diagnostics`: what a bug report needs, without secrets or traffic. */
  private diagnostics(): Diagnostics {
    const s = this.settings;
    return {
      version: '0.1.0-mock',
      releaseDate: '2026-11-11',
      os: this.platform === 'macos' ? 'macOS 15.2' : this.platform === 'windows' ? 'Windows 11' : 'Ubuntu 24.04',
      arch: 'x86_64',
      platform: this.platform,
      desktop: false,
      uptimeSecs: Math.round((Date.now() - this.startedAt) / 1000),
      repository: 'https://github.com/akevdev/akev',
      stats: this.stats(),
      settings: {
        proxy: { port: s.proxy.port, bindAddress: s.proxy.bindAddress, http2: s.proxy.http2, upstreamProxy: s.proxy.upstreamProxy.mode, responseTimeoutMs: s.proxy.responseTimeoutMs },
        capture: { maxBodyBytes: s.capture.maxBodyBytes, memoryBudgetBytes: s.capture.memoryBudgetBytes, maxExchanges: s.capture.maxExchanges },
        sessions: { autosave: s.sessions.autosave },
      },
      recentWarnings: [{ at: new Date(this.startedAt + 60_000).toISOString(), level: 'WARN', target: 'akev_proxy::upstream', message: 'upstream connect to api.example.com:443 timed out' }],
      crashReports: [],
      logFile: null,
    };
  }

  private status(): Status {
    return {
      version: '0.1.0-mock',
      proxy: { port: this.settings.proxy.port, bindAddress: this.settings.proxy.bindAddress, addresses: [`127.0.0.1:${this.settings.proxy.port}`, `192.168.1.23:${this.settings.proxy.port}`], http3: this.http3Status() },
      apiUrl: 'http://127.0.0.1:45456',
      dataDir: this.platform === 'macos' ? '~/Library/Application Support/akev' : this.platform === 'windows' ? '%APPDATA%\\akev' : '~/.local/share/akev',
      ca: this.ca,
      startedAt: this.startedAt,
      stats: this.stats(),
      platform: this.platform,
      desktop: false,
      license: this.license.status(),
      ...(this.restoredSession ? { restoredSession: this.restoredSession } : {}),
      ...(this.livePort != null ? { proxy: { port: this.livePort, bindAddress: this.settings.proxy.bindAddress, addresses: [`127.0.0.1:${this.livePort}`], http3: this.http3Status() } } : {}),
    };
  }

  /** `Status.proxy.http3`, as the daemon reports its QUIC listener. */
  private http3Status(): Http3ListenerStatus {
    const h3 = this.settings.proxy.http3;
    if (!h3.listen) return { enabled: false };
    const port = h3.listenPort || (this.livePort ?? this.settings.proxy.port);
    return { enabled: true, address: `${this.settings.proxy.bindAddress}:${port}`, port };
  }

  private detail(rec: MockRecord): ExchangeDetail {
    const meta = (side: 'request' | 'response') => {
      const spec = side === 'request' ? rec.reqSpec : rec.resSpec;
      if (!spec || (side === 'response' && !rec.response)) return { wireSize: 0, capturedSize: 0, truncated: false, encodings: [] };
      const data = this.materialize(rec, side);
      const size = spec.reportedSize ?? (data == null ? 0 : typeof data === 'string' ? utf8Encode(data).length : data.length);
      const wire = wireSizeFor(size, spec.encoding);
      return { wireSize: wire, capturedSize: wire, truncated: spec.captureTruncated ?? false, encodings: spec.encoding ? [spec.encoding] : [], decodedSize: size, contentType: spec.contentType.split(';')[0] };
    };
    return {
      summary: rec.summary,
      request: rec.request,
      requestBody: meta('request'),
      ...(rec.response ? { response: rec.response } : {}),
      responseBody: meta('response'),
      ...(rec.originalRequest ? { originalRequest: rec.originalRequest } : {}),
      ...(rec.originalResponse ? { originalResponse: rec.originalResponse } : {}),
      timing: rec.timing,
      client: rec.client,
      ...(rec.upstream ? { upstream: rec.upstream } : {}),
      ...(rec.error ? { error: rec.error } : {}),
      ...(rec.tunnel ? { tunnel: rec.tunnel } : {}),
      ...(rec.note ? { note: rec.note } : {}),
      ...(rec.matchedRules.length ? { matchedRules: rec.matchedRules } : {}),
      ...(mockGrpcTrailers(rec.summary) ? { responseTrailers: mockGrpcTrailers(rec.summary) } : {}),
    };
  }

  private bodyB64(rec: MockRecord, side: 'request' | 'response'): string {
    const data = this.materialize(rec, side);
    if (data == null) return '';
    return typeof data === 'string' ? textToBase64(data) : bytesToBase64(data);
  }

  private editableRequest(rec: MockRecord): EditableRequest {
    return { method: rec.request.method, url: rec.request.url, headers: rec.request.headers.filter(([k]) => !k.startsWith(':')), body: this.bodyB64(rec, 'request') };
  }

  private editableResponse(rec: MockRecord): EditableResponse {
    const status = rec.response?.status ?? 200;
    return { status, statusMessage: rec.response?.statusMessage ?? statusText(status), headers: (rec.response?.headers ?? []).filter(([k]) => !k.startsWith(':')), body: this.bodyB64(rec, 'response') };
  }

  private record(id: string | undefined): MockRecord {
    const rec = this.records.get(Number(id));
    if (!rec) throw new HttpError(404, 'not-found', `Exchange ${id} not found`);
    return rec;
  }

  private bodyView(rec: MockRecord, side: 'request' | 'response'): BodyView {
    const spec = side === 'request' ? rec.reqSpec : rec.resSpec;
    const view = buildBodyView(spec, this.materialize(rec, side), new Rng(rec.seed + 2));
    return this.protos.decorate(view, { side, url: rec.request.url, path: rec.request.path, contentType: spec?.contentType ?? null });
  }

  /** `POST /api/send/repeat`: compose `count` times, `concurrency` at a time, streaming NDJSON. */
  private repeat(b: RepeatParams): MockResponse {
    const count = b.count ?? 0;
    const concurrency = Math.min(b.concurrency ?? 1, count);
    const delay = b.delayMs ?? 0;
    if (!(count >= 1 && count <= REPEAT_LIMITS.count)) throw new HttpError(400, 'invalid-request', `count must be between 1 and ${REPEAT_LIMITS.count}`);
    if (!(concurrency >= 1 && (b.concurrency ?? 1) <= REPEAT_LIMITS.concurrency)) throw new HttpError(400, 'invalid-request', `concurrency must be between 1 and ${REPEAT_LIMITS.concurrency}`);
    if (delay > REPEAT_LIMITS.delayMs) throw new HttpError(400, 'invalid-request', `delayMs must be at most ${REPEAT_LIMITS.delayMs}`);
    const req = b.request ?? (b.exchangeId != null ? this.editableRequest(this.record(String(b.exchangeId))) : null);
    if (!req) throw new HttpError(400, 'invalid-request', 'provide the request, or the exchangeId to resend');
    const encoder = new TextEncoder();
    let next = 0;
    let cancelled = false;
    const stream = new ReadableStream<Uint8Array>({
      start: (controller) => {
        const line = (v: unknown) => controller.enqueue(encoder.encode(`${JSON.stringify(v)}\n`));
        const worker = async () => {
          let first = true;
          while (!cancelled) {
            const index = next++;
            if (index >= count) return;
            if (!first && delay > 0) await sleep(delay);
            first = false;
            await sleep(this.rng.int(30, 240));
            if (cancelled) return;
            const { result } = this.compose(req, b.options);
            // Some failures and slow answers so the summary has something to show.
            const roll = this.rng.int(0, 99);
            const status = roll < 3 ? 503 : roll < 5 ? 429 : (result.response?.status ?? 200);
            const durationMs = (result.timing.responseBody ?? 0) * (roll > 95 ? 6 : 1);
            line(roll === 99 ? { index, id: result.id, error: 'timeout', durationMs: 30_000 } : { index, id: result.id, status, durationMs });
          }
        };
        void Promise.all(Array.from({ length: concurrency }, worker)).then(() => {
          if (cancelled) return;
          line({ done: true, sent: Math.min(next, count) });
          controller.close();
        });
      },
      cancel: () => {
        cancelled = true;
      },
    });
    return { stream };
  }

  /** Create a composer exchange for send / replay. Like the daemon, it is only stored
   *  (`recorded`) while recording is on. */
  private compose(req: EditableRequest, options?: SendOptions): { rec: MockRecord; result: SendResult; recorded: boolean } {
    const id = this.nextId++;
    const base = createRecord(id, this.rng, Date.now(), { scenario: scenarioIndex((h, m) => h === 'localhost' && m === 'GET') });
    let url: URL | null = null;
    try {
      url = new URL(req.url);
    } catch {
      url = null;
    }
    const scheme = url?.protocol.replace(':', '') ?? 'http';
    const host = url?.hostname ?? 'localhost';
    const port = url ? Number(url.port || (scheme === 'https' ? 443 : 80)) : 80;
    const path = url ? `${url.pathname}${url.search}` : req.url;
    const echo = JSON.stringify({ ok: true, method: req.method, url: req.url, headers: Object.fromEntries(req.headers), receivedAt: new Date().toISOString() });
    // Like the daemon: the protocol spoken upstream (HTTP/3 only for https:// URLs).
    const httpVersion: HttpVersion = options?.httpVersion === 'http3' && scheme === 'https' ? 'HTTP/3' : options?.httpVersion === 'http2' && scheme === 'https' ? 'HTTP/2' : 'HTTP/1.1';
    const now = Date.now();
    const duration = this.rng.int(30, 240);
    const rec: MockRecord = {
      ...base,
      request: { ...base.request, method: req.method, url: req.url, scheme, host, port, path, headers: req.headers },
      client: { ...base.client, via: 'composer', process: undefined, interceptor: undefined },
      reqSpec: req.body ? { kind: 'text', contentType: 'text/plain', make: () => base64ToBytes(req.body) } : null,
      resSpec: { kind: 'json', contentType: 'application/json', make: () => echo },
      reqData: req.body ? base64ToBytes(req.body) : undefined,
      resData: echo,
      matchedRules: [],
      phase: 'done',
      nextAt: null,
      response: { status: 200, statusMessage: 'OK', httpVersion, headers: [['content-type', 'application/json'], ['content-length', String(echo.length)]] },
      timing: { start: now * 1000, requestHeaders: 0.2, requestSent: 1, responseHeaders: duration * 0.8, responseBody: duration },
      summary: {
        ...base.summary,
        method: req.method,
        scheme,
        host,
        port,
        path,
        httpVersion,
        state: 'complete',
        status: 200,
        contentKind: 'json',
        contentType: 'application/json',
        requestSize: req.body ? base64ToBytes(req.body).length : 0,
        responseSize: echo.length,
        duration,
        source: 'Akev Compose',
        rule: undefined,
        mocked: undefined,
        modified: undefined,
        color: undefined,
      },
    };
    const recorded = this.recording;
    if (recorded) this.insert(rec, true);
    return { rec, result: { id, response: rec.response ?? null, error: null, timing: rec.timing }, recorded };
  }

  // ---------------------------------------------------------------- routes

  /** Store `settings.proxy.wireguard`, (re)start the mock server, update the interceptor card. */
  private setWireguard(next: WireguardSettings, connected = false): void {
    this.settings = { ...this.settings, proxy: { ...this.settings.proxy, wireguard: next } };
    this.wireguard.applied(next, connected);
    this.interceptors = this.interceptors.map((x) => (x.id === 'wireguard' ? { ...x, active: next.enabled } : x));
  }

  private registerWireguardRoutes(): void {
    const r = this.route.bind(this);
    r('GET', /^\/api\/wireguard$/, (_m, url) => ({ json: this.wireguard.info(url.searchParams.get('host')) }));
    r('PUT', /^\/api\/wireguard$/, (_m, _u, body) => {
      if ((body as WireguardPatch | undefined)?.enabled === true && !this.settings.proxy.wireguard?.enabled) this.license.require('wireguard');
      let next: WireguardSettings;
      try {
        next = this.wireguard.patch(this.settings.proxy.wireguard ?? { enabled: false, port: 51820 }, (body ?? {}) as WireguardPatch);
      } catch (e) {
        throw new HttpError(400, 'bad-request', e instanceof Error ? e.message : String(e));
      }
      this.setWireguard(next);
      this.broadcast({ type: 'settings', settings: this.settings });
      this.broadcast({ type: 'interceptors', interceptors: this.interceptors });
      return { json: this.wireguard.info() };
    });
    r('POST', /^\/api\/wireguard\/regenerate$/, async () => {
      await sleep(300);
      this.wireguard.regenerate();
      return { json: this.wireguard.info() };
    });
    r('GET', /^\/api\/wireguard\/config$/, (_m, url) => ({ text: this.wireguard.info(url.searchParams.get('host')).config, contentType: 'text/plain; charset=utf-8' }));
  }

  // ---------------------------------------------------------------- Akev Pro license

  /** Broadcast the license (and what depends on it) after a change. */
  private licenseChanged(): void {
    this.broadcast({ type: 'license', license: this.license.status() });
    this.scheduleTrialEnd();
  }

  /** Announce the trial ending at the moment it does (as the daemon does). */
  private scheduleTrialEnd(): void {
    if (this.trialTimer) clearTimeout(this.trialTimer);
    this.trialTimer = null;
    const end = this.license.trialEnd();
    // setTimeout overflows past ~24.8 days; trials are 14.
    if (end != null) this.trialTimer = setTimeout(() => this.licenseChanged(), Math.min(end - Date.now() + 50, 2_000_000_000));
  }

  /** Dev helper (`__akevMock.expireTrial()`): end a running trial now. */
  expireTrial(): boolean {
    const ended = this.license.expireTrial();
    if (ended) this.licenseChanged();
    return ended;
  }

  private registerLicenseRoutes(): void {
    const r = this.route.bind(this);
    r('GET', /^\/api\/license$/, () => ({ json: this.license.status() }));
    r('PUT', /^\/api\/license$/, async (_m, _u, body) => {
      await sleep(250);
      const status = this.license.activate((body as { key?: unknown } | undefined)?.key);
      this.licenseChanged();
      return { json: status };
    });
    r('DELETE', /^\/api\/license$/, () => {
      const status = this.license.remove();
      this.licenseChanged();
      return { json: status };
    });
    r('POST', /^\/api\/license\/trial$/, async () => {
      await sleep(200);
      const status = this.license.startTrial();
      this.licenseChanged();
      return { json: status };
    });
  }

  private caTrusted = !this.empty;

  private trustReport(): TrustReport {
    return {
      trusted: this.caTrusted,
      stores: [
        {
          id: 'user',
          label: 'macOS keychain (your account)',
          description: "Safari, Chrome, Edge, curl and apps that use the system's TLS",
          trusted: this.caTrusted,
          canChange: true,
          needsAdmin: true,
        },
      ],
    };
  }

  private route(method: string, pattern: RegExp, handler: Handler): void {
    this.routes.push([method, pattern, handler]);
  }

  private registerRoutes(): void {
    const r = this.route.bind(this);
    r('GET', /^\/api\/status$/, () => ({ json: this.status() }));
    r('GET', /^\/api\/diagnostics$/, () => ({ json: this.diagnostics() }));
    r('POST', /^\/api\/shutdown$/, () => ({ status: 204 }));

    // ---- exchanges
    r('GET', /^\/api\/exchanges$/, (_m, url) => {
      const compiled = compileFilter(url.searchParams.get('filter') ?? '');
      if (!compiled.ok) throw new HttpError(400, 'invalid-filter', compiled.error.message);
      const limit = Number(url.searchParams.get('limit') ?? 0) || Infinity;
      const out: ExchangeSummary[] = [];
      for (const id of this.order) {
        const rec = this.records.get(id);
        if (!rec || (compiled.predicate && !compiled.predicate(this.subject(rec)))) continue;
        out.push(rec.summary);
        if (out.length >= limit) break;
      }
      return { json: out };
    });
    r('GET', /^\/api\/exchanges\/(\d+)$/, (m) => ({ json: this.detail(this.record(m[1])) }));
    r('GET', /^\/api\/exchanges\/(\d+)\/body\/(request|response)$/, (m) => ({ json: this.bodyView(this.record(m[1]), m[2] as 'request' | 'response') }));
    r('GET', /^\/api\/exchanges\/(\d+)\/body\/(request|response)\/raw$/, (m) => {
      const rec = this.record(m[1]);
      const side = m[2] as 'request' | 'response';
      const data = this.materialize(rec, side);
      const spec = side === 'request' ? rec.reqSpec : rec.resSpec;
      return { bytes: data == null ? new Uint8Array() : typeof data === 'string' ? utf8Encode(data) : data, contentType: spec?.contentType ?? 'application/octet-stream' };
    });
    r('GET', /^\/api\/exchanges\/(\d+)\/messages$/, (m, url) => {
      // Exclusive seq bounds, ascending; `tail` = the newest `limit` of the range.
      const q = url.searchParams;
      const after = q.has('after') ? Number(q.get('after')) : -Infinity;
      const before = q.has('before') ? Number(q.get('before')) : Infinity;
      const limit = Math.max(1, Math.min(MESSAGES_PAGE_MAX, Number(q.get('limit') ?? MESSAGES_PAGE) || MESSAGES_PAGE));
      const tail = q.get('tail') === '1' || q.get('tail') === 'true';
      const range = this.record(m[1]).messages.filter((msg) => msg.seq > after && msg.seq < before);
      return { json: tail ? range.slice(Math.max(0, range.length - limit)) : range.slice(0, limit) };
    });
    r('GET', /^\/api\/exchanges\/(\d+)\/llm$/, (m) => {
      this.license.require('llm-lens');
      const view = llmView(this.record(m[1]));
      if (!view) throw new HttpError(404, 'not-llm', `exchange ${m[1]} is not a recognised LLM API call`);
      return { json: view };
    });
    r('GET', /^\/api\/exchanges\/(\d+)\/snippet$/, (m, url) => {
      const target = (url.searchParams.get('target') ?? 'curl') as Parameters<typeof generateSnippet>[1];
      if (!CODEGEN_TARGETS.some((t) => t.id === target)) throw new HttpError(400, 'invalid-target', `Unknown target "${target}"`);
      return { json: generateSnippet(this.editableRequest(this.record(m[1])), target) };
    });
    r('POST', /^\/api\/exchanges\/delete$/, (_m, _u, body) => {
      const ids = (body as { ids?: number[] })?.ids ?? [];
      this.removeIds(ids.filter((id) => this.records.has(id)));
      return { status: 204 };
    });
    r('POST', /^\/api\/exchanges\/clear$/, (_m, _u, body) => {
      const keepPinned = (body as { keepPinned?: boolean })?.keepPinned ?? false;
      const pinned = this.order.filter((id) => this.records.get(id)?.summary.pinned);
      if (keepPinned && pinned.length > 0) {
        this.removeIds(this.order.filter((id) => !this.records.get(id)?.summary.pinned));
      } else {
        this.records.clear();
        this.order = [];
        this.live.clear();
        this.pendingAdded.clear();
        this.pendingUpdated.clear();
        for (const id of this.paused.keys()) this.broadcast({ type: 'resumed', id });
        this.paused.clear();
        this.wsTools.forget('all');
        for (const s of this.sockets) if (s.matching) s.matching.clear();
        this.broadcast({ type: 'cleared' });
      }
      return { status: 204 };
    });
    r('PATCH', /^\/api\/exchanges$/, (_m, _u, body) => {
      const p = body as { ids: number[]; pinned?: boolean; color?: ExchangeSummary['color'] | null; note?: string | null };
      for (const id of p.ids ?? []) {
        const rec = this.records.get(id);
        if (!rec) continue;
        const s = { ...rec.summary };
        if (p.pinned !== undefined) s.pinned = p.pinned || undefined;
        if (p.color !== undefined) s.color = p.color ?? undefined;
        if (p.note !== undefined) {
          rec.note = p.note ?? undefined;
          s.hasNote = p.note ? true : undefined;
        }
        rec.summary = s;
        this.pendingUpdated.add(id);
      }
      return { status: 204 };
    });
    r('POST', /^\/api\/exchanges\/headers$/, (_m, _u, body) => {
      const b = (body ?? {}) as { ids?: number[]; request?: string[]; response?: string[] };
      const ids = b.ids ?? [];
      if (ids.length > BATCH_LIMIT) throw new HttpError(400, 'invalid-request', `at most ${BATCH_LIMIT} ids per request (got ${ids.length})`);
      const names = (list: string[] | undefined) => (list ?? []).map((n) => n.trim().toLowerCase());
      const [req, res] = [names(b.request), names(b.response)];
      if ([...req, ...res].some((n) => !/^[!#$%&'*+.^_`|~0-9a-z:-]+$/.test(n))) throw new HttpError(400, 'invalid-request', 'not a valid header name');
      const pick = (headers: readonly (readonly [string, string])[], wanted: string[]) => {
        const out: Record<string, string> = {};
        for (const n of wanted) {
          const values = headers.filter(([k]) => k.toLowerCase() === n).map(([, v]) => v);
          if (values.length) out[n] = values.join(', ');
        }
        return Object.keys(out).length ? out : undefined;
      };
      const exchanges = [];
      for (const id of ids) {
        const rec = this.records.get(id);
        if (!rec) continue;
        exchanges.push({ id, request: pick(rec.request.headers, req), response: pick(rec.response?.headers ?? [], res) });
      }
      return { json: { exchanges } };
    });
    r('POST', /^\/api\/exchanges\/timings$/, (_m, _u, body) => {
      const ids = (body as { ids?: number[] } | undefined)?.ids ?? [];
      if (ids.length > BATCH_LIMIT) throw new HttpError(400, 'invalid-request', `at most ${BATCH_LIMIT} ids per request (got ${ids.length})`);
      const timings = [];
      for (const id of ids) {
        const rec = this.records.get(id);
        if (rec) timings.push({ id, ...rec.timing });
      }
      return { json: { timings } };
    });
    r('POST', /^\/api\/exchanges\/(\d+)\/replay$/, async (m, _u, body) => {
      const original = this.record(m[1]);
      const b = body as { request?: EditableRequest; options?: SendOptions } | undefined;
      const req = b?.request ?? this.editableRequest(original);
      await sleep(this.rng.int(60, 250));
      const { rec, result, recorded } = this.compose(req, b?.options);
      return { json: { ...result, body: this.bodyView(rec, 'response'), recorded } };
    });
    r('GET', /^\/api\/exchanges\/(\d+)\/editable\/(request|response)$/, (m) => {
      const rec = this.record(m[1]);
      return { json: m[2] === 'request' ? this.editableRequest(rec) : this.editableResponse(rec) };
    });

    // ---- filter
    r('POST', /^\/api\/filter\/validate$/, (_m, _u, body) => {
      const c = compileFilter((body as { query?: string })?.query ?? '');
      return { json: c.ok ? { ok: true } : { ok: false, error: c.error } };
    });
    r('POST', /^\/api\/filter\/suggest$/, (_m, _u, body) => {
      const b = body as { query?: string; cursor?: number };
      const hosts = new Set<string>();
      const methods = new Set<string>(['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS']);
      const sources = new Set<string>();
      const models = new Set<string>();
      const providers = new Set<string>();
      for (const rec of this.records.values()) {
        hosts.add(rec.summary.host);
        methods.add(rec.summary.method);
        if (rec.summary.source) sources.add(rec.summary.source);
        if (rec.summary.llm) {
          providers.add(rec.summary.llm.provider);
          if (rec.summary.llm.model) models.add(rec.summary.llm.model);
        }
      }
      return { json: suggestFilter(b.query ?? '', b.cursor ?? 0, { hosts: [...hosts].sort(), methods: [...methods], sources: [...sources].sort(), models: [...models].sort(), providers: [...providers].sort() }) };
    });
    r('POST', /^\/api\/filter\/query$/, (_m, _u, body) => {
      const c = compileFilter((body as { query?: string })?.query ?? '');
      if (!c.ok) throw new HttpError(400, 'invalid-filter', c.error.message);
      const pred = c.predicate;
      return { json: { ids: this.order.filter((id) => !pred || pred(this.subject(this.records.get(id) as MockRecord))) } };
    });
    r('GET', /^\/api\/filter\/docs$/, () => ({ json: FILTER_DOCS }));

    // ---- rules
    r('GET', /^\/api\/rules$/, () => ({ json: this.rules }));
    r('PUT', /^\/api\/rules$/, (_m, _u, body) => {
      const rules = body as RuleSet;
      if (!rules || !Array.isArray(rules.items)) throw new HttpError(400, 'invalid-request', 'Expected a RuleSet');
      const errors: [string, string][] = [];
      const check = (items: RuleItem[]) => {
        for (const i of items) {
          if (i.type === 'group') check(i.items);
          else if (i.action.type === 'mock' && (i.action.status < 100 || i.action.status > 599)) errors.push([i.id, `Invalid status code ${i.action.status}`]);
          else if (i.action.type === 'forward' && !/^https?:\/\//.test(i.action.target)) errors.push([i.id, 'Forward target must be an http(s) URL']);
          else if (i.action.type === 'script' && !this.license.entitled('scripting')) errors.push([i.id, 'Inactive: script rules need Akev Pro. The rule is saved and runs again once Pro is active.']);
          else if (i.action.type === 'tls-profile' && i.action.profile !== 'native' && !this.license.entitled('tls-impersonation')) errors.push([i.id, 'Inactive: TLS fingerprint rules need Akev Pro. Matching requests use the native TLS stack until Pro is active.']);
        }
      };
      check(rules.items);
      this.rules = rules;
      this.broadcast({ type: 'rules', rules });
      return { json: { errors } };
    });
    r('GET', /^\/api\/rules\/stats$/, () => ({ json: [...this.ruleStats.values()] }));
    r('POST', /^\/api\/rules\/stats\/reset$/, () => {
      for (const [id, st] of this.ruleStats) this.ruleStats.set(id, { ...st, matches: 0, lastMatchedAt: undefined });
      const stats = [...this.ruleStats.values()];
      this.broadcast({ type: 'rule-stats', stats });
      return { json: stats };
    });
    r('POST', /^\/api\/rules\/test$/, (_m, _u, body) => {
      const b = body as { exchangeId?: number; rules?: RuleSet };
      const rules = b.rules ?? this.rules;
      const rec = b.exchangeId != null ? this.records.get(b.exchangeId) : undefined;
      const matched = rec?.summary.rule ? [rec.summary.rule] : [];
      const terminal = matched[0] ?? firstRuleId(rules);
      return { json: { matched, terminalRule: terminal, action: { type: 'passthrough' } } };
    });
    r('GET', /^\/api\/rules\/export$/, () => ({ text: JSON.stringify(this.rules, null, 2), contentType: 'application/json' }));
    r('POST', /^\/api\/rules\/import$/, (_m, _u, body, raw) => {
      const parsed = (body ?? (raw ? JSON.parse(utf8Decode(raw)) : null)) as Partial<RuleSet> | null;
      if (parsed && Array.isArray(parsed.items)) return { json: { version: 1, items: parsed.items } };
      return { json: { version: 1, items: [{ type: 'group', id: crypto.randomUUID(), title: 'Imported from HTTP Toolkit', enabled: true, collapsed: false, items: [] }] } };
    });

    // ---- breakpoints
    this.wsTools.register(r);
    r('GET', /^\/api\/breakpoints$/, () => ({ json: [...this.paused.values()] }));
    r('POST', /^\/api\/breakpoints\/(\d+)\/resume$/, (m, _u, body) => {
      const id = Number(m[1]);
      const rec = this.records.get(id);
      if (!rec || !this.paused.has(id)) throw new HttpError(404, 'not-found', `Exchange ${id} is not paused`);
      const resumeBody = body as RequestResume | ResponseResume;
      const response =
        resumeBody.action === 'respond'
          ? { status: resumeBody.response.status, statusMessage: resumeBody.response.statusMessage, httpVersion: rec.request.httpVersion, headers: resumeBody.response.headers }
          : undefined;
      if (resumeBody.action === 'continue' && 'request' in resumeBody) {
        rec.originalRequest = rec.request;
        rec.request = { ...rec.request, method: resumeBody.request.method, url: resumeBody.request.url, headers: resumeBody.request.headers };
        rec.summary = { ...rec.summary, modified: true, method: resumeBody.request.method };
      }
      resume(rec, Date.now(), resumeBody.action, response);
      this.paused.delete(id);
      this.live.add(id);
      this.pendingUpdated.add(id);
      this.broadcast({ type: 'resumed', id });
      return { status: 204 };
    });
    r('POST', /^\/api\/breakpoints\/resume-all$/, () => {
      for (const id of [...this.paused.keys()]) {
        const rec = this.records.get(id);
        if (rec) {
          resume(rec, Date.now(), 'continue');
          this.live.add(id);
          this.pendingUpdated.add(id);
        }
        this.paused.delete(id);
        this.broadcast({ type: 'resumed', id });
      }
      return { status: 204 };
    });

    // ---- compose
    r('POST', /^\/api\/send\/repeat$/, (_m, _u, body) => this.repeat((body ?? {}) as RepeatParams));
    r('POST', /^\/api\/send$/, async (_m, _u, body) => {
      const b = body as { request: EditableRequest; options?: SendOptions };
      if (!b?.request?.url) throw new HttpError(400, 'invalid-request', 'request.url is required');
      await sleep(this.rng.int(80, 400));
      const { rec, result, recorded } = this.compose(b.request, b.options);
      return { json: { ...result, body: this.bodyView(rec, 'response'), recorded } };
    });
    r('POST', /^\/api\/import\/curl$/, (_m, _u, body) => ({ json: parseCurl((body as { command?: string })?.command ?? '') }));
    r('GET', /^\/api\/codegen\/targets$/, () => ({ json: CODEGEN_TARGETS }));
    r('POST', /^\/api\/codegen$/, (_m, _u, body) => {
      const b = body as { request: EditableRequest; target: Parameters<typeof generateSnippet>[1] };
      return { json: generateSnippet(b.request, b.target) };
    });

    // ---- interceptors
    r('GET', /^\/api\/interceptors$/, () => ({ json: this.interceptors.map((i) => (i.id === 'transparent-linux' ? this.transparent.interceptor(this.settings.proxy.transparent) : i)) }));
    r('POST', /^\/api\/interceptors\/([^/]+)\/activate$/, async (m, _u, body) => {
      const id = decodeURIComponent(m[1] ?? '');
      const i = this.interceptors.find((x) => x.id === id);
      if (!i) throw new HttpError(404, 'not-found', `Unknown interceptor ${id}`);
      if (id === 'jvm') this.license.require('jvm-attach');
      if (id === 'wireguard') this.license.require('wireguard');
      if (id === 'transparent-linux') this.license.require('transparent');
      await sleep(this.rng.int(300, 900));
      if (!i.available) return { json: { success: false, message: i.unavailableReason ?? `${i.name} is not available` } satisfies ActivationResult };
      if (id === 'wireguard') this.setWireguard({ ...(this.settings.proxy.wireguard ?? { port: 51820 }), enabled: true });
      if (id === 'jvm') {
        const { info, result } = mockJvmActivate(i, (body as { options?: unknown } | undefined)?.options, this.settings.proxy.port);
        this.interceptors = this.interceptors.map((x) => (x.id === id ? info : x));
        this.broadcast({ type: 'interceptors', interceptors: this.interceptors });
        return { json: result };
      }
      this.interceptors = this.interceptors.map((x) => (x.id === id ? { ...x, active: true } : x));
      this.broadcast({ type: 'interceptors', interceptors: this.interceptors });
      // A fresh install starts seeing traffic once something is intercepted.
      this.startTraffic();
      const result: ActivationResult =
        id === 'existing-terminal'
          ? { success: true, message: 'Paste this into your terminal', metadata: { command: `eval "$(printf '%s\\n' 'header = "Authorization: Bearer mock-token"' | curl -sS --noproxy '*' -K - 'http://127.0.0.1:45456/api/setup/terminal?shell=bash')"` } }
          : { success: true, message: `${i.name} is now intercepted` };
      return { json: result };
    });
    r('POST', /^\/api\/interceptors\/([^/]+)\/deactivate$/, (m, _u, body) => {
      const id = decodeURIComponent(m[1] ?? '');
      if (id === 'wireguard') this.setWireguard({ ...(this.settings.proxy.wireguard ?? { port: 51820 }), enabled: false });
      const options = (body as { options?: unknown } | undefined)?.options;
      this.interceptors = this.interceptors.map((x) => (x.id !== id ? x : id === 'jvm' ? mockJvmDeactivate(x, options) : { ...x, active: false }));
      this.broadcast({ type: 'interceptors', interceptors: this.interceptors });
      return { status: 204 };
    });
    this.registerWireguardRoutes();
    r('GET', /^\/api\/transparent$/, () => ({ json: this.transparent.info(this.settings.proxy.transparent) }));
    r('POST', /^\/api\/transparent\/plan$/, (_m, _u, body) => {
      this.license.require('transparent');
      return this.transparent.plan((body ?? {}) as TransparentPlanRequest, this.settings.proxy.transparent);
    });
    r('GET', /^\/api\/setup\/terminal$/, (_m, url) => ({ text: terminalScript(url.searchParams.get('shell') ?? 'bash', this.settings.proxy.port) }));

    // ---- settings & CA
    r('GET', /^\/api\/settings$/, () => ({ json: this.settings }));
    r('PUT', /^\/api\/settings$/, (_m, _u, body) => {
      const next = body as Settings;
      if (!next?.proxy || !next.capture) throw new HttpError(400, 'invalid-request', 'Expected Settings');
      if (next.proxy.port < 1 || next.proxy.port > 65535) throw new HttpError(400, 'invalid-settings', 'proxy.port must be between 1 and 65535');
      if (next.proxy.transparent?.enabled && next.proxy.transparent.port === next.proxy.port) throw new HttpError(400, 'bad-request', 'proxy.transparent.port must differ from proxy.port: the main port keeps explicit-proxy semantics');
      if (next.proxy.transparent?.enabled && !this.settings.proxy.transparent?.enabled) this.license.require('transparent');
      this.settings = next;
      this.broadcast({ type: 'settings', settings: this.settings });
      return { json: this.settings };
    });
    r('PATCH', /^\/api\/settings\/ui$/, (_m, _u, body) => {
      this.settings = { ...this.settings, ui: body ?? {} };
      this.broadcast({ type: 'settings', settings: this.settings });
      return { status: 204 };
    });
    r('GET', /^\/api\/tls-profiles$/, () => ({ json: { ...mockTlsProfiles(mockImpersonationAvailable()), licensed: this.license.entitled('tls-impersonation') } }));
    r('GET', /^\/api\/ca$/, () => ({ json: this.ca }));
    r('GET', /^\/api\/ca\/cert\.pem$/, () => ({ text: MOCK_PEM, contentType: 'application/x-pem-file' }));
    r('GET', /^\/api\/ca\/cert\.der$/, () => ({ bytes: base64ToBytes(MOCK_PEM.split('\n').filter((l) => !l.startsWith('-----')).join('')), contentType: 'application/x-x509-ca-cert' }));
    r('GET', /^\/api\/update$/, () => ({ json: { current: '0.1.0', available: false, latest: '0.1.0', downloadUrl: 'https://github.com/akevdev/akev/releases/latest', checkedAt: Date.now() } }));
    r('POST', /^\/api\/update\/check$/, async () => {
      await sleep(400);
      return { json: { current: '0.1.0', available: false, latest: '0.1.0', downloadUrl: 'https://github.com/akevdev/akev/releases/latest', checkedAt: Date.now() } };
    });
    r('GET', /^\/api\/ca\/trust$/, () => ({ json: this.trustReport() }));
    r('POST', /^\/api\/ca\/trust$/, async () => {
      await sleep(600);
      this.caTrusted = true;
      return { json: this.trustReport() };
    });
    r('DELETE', /^\/api\/ca\/trust$/, async () => {
      await sleep(400);
      this.caTrusted = false;
      return { json: this.trustReport() };
    });
    r('POST', /^\/api\/ca\/regenerate$/, async () => {
      await sleep(500);
      this.ca = { ...mockCaInfo(Date.now()), daysRemaining: 365, sha256Fingerprint: this.ca.sha256Fingerprint.split(':').reverse().join(':') };
      return { json: this.ca };
    });

    // ---- import / export
    r('GET', /^\/api\/export\/har$/, (_m, url) => ({ text: JSON.stringify(this.har(url)), contentType: 'application/json' }));
    r('GET', /^\/api\/export\/session$/, () => ({ bytes: utf8Encode(JSON.stringify({ akevSession: 1, exchanges: this.order.length })), contentType: 'application/octet-stream' }));
    r('POST', /^\/api\/import\/(har|session)$/, async (m, _u, body, raw) => {
      let n = 5;
      if (m[1] === 'har') {
        try {
          const har = (body ?? (raw ? JSON.parse(utf8Decode(raw)) : null)) as { log?: { entries?: unknown[] } } | null;
          n = Math.min(500, har?.log?.entries?.length ?? 0);
        } catch {
          throw new HttpError(400, 'invalid-har', 'The file is not valid HAR JSON');
        }
      }
      await sleep(200);
      const ids: number[] = [];
      for (let i = 0; i < n; i++) {
        const rec = createRecord(this.nextId++, this.rng, Date.now() - (n - i) * 50);
        rec.plan.pause = undefined;
        this.runUntil(rec, Date.now() + 60_000);
        this.live.delete(rec.summary.id);
        this.insert(rec, true);
        ids.push(rec.summary.id);
      }
      return { json: { ids } };
    });

    // ---- protobuf schemas
    const protoCall = <T,>(f: () => T): T => {
      try {
        return f();
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        throw new HttpError((e as { notFound?: boolean }).notFound ? 404 : 400, (e as { notFound?: boolean }).notFound ? 'not-found' : 'bad-request', message);
      }
    };
    r('GET', /^\/api\/protos$/, () => ({ json: this.protos.info() }));
    r('POST', /^\/api\/protos$/, (_m, _u, body) => {
      this.license.require('proto-schemas');
      return { json: protoCall(() => this.protos.add((body as { files?: ProtoUploadFile[] })?.files ?? [])) };
    });
    r('DELETE', /^\/api\/protos$/, (_m, url) => {
      const name = url.searchParams.get('name');
      if (!name && !url.searchParams.get('all')) throw new HttpError(400, 'bad-request', 'name the file to delete with `?name=`, or delete every file with `?all=1`');
      return { json: protoCall(() => this.protos.remove(name)) };
    });
    r('GET', /^\/api\/protos\/file$/, (_m, url) => {
      const text = this.protos.source(url.searchParams.get('name') ?? '');
      if (text == null) throw new HttpError(404, 'not-found', 'no such schema file');
      return { text };
    });
    r('GET', /^\/api\/protos\/mappings$/, () => ({ json: this.protos.mappings }));
    r('PUT', /^\/api\/protos\/mappings$/, (_m, _u, body) => {
      this.license.require('proto-schemas');
      return { json: protoCall(() => this.protos.setMappings(body as ProtoMapping[])) };
    });

    // ---- capture
    r('GET', /^\/api\/capture$/, () => ({ json: { recording: this.recording } }));
    r('PUT', /^\/api\/capture$/, (_m, _u, body) => {
      this.recording = !!(body as { recording?: boolean })?.recording;
      this.broadcast({ type: 'capture', recording: this.recording });
      return { json: { recording: this.recording } };
    });
  }

  private har(url: URL): unknown {
    const idsParam = url.searchParams.get('ids');
    const ids = idsParam ? idsParam.split(',').map(Number) : this.order.slice(-500);
    return {
      log: {
        version: '1.2',
        creator: { name: 'Akev (mock)', version: '0.1.0' },
        entries: ids
          .map((id) => this.records.get(id))
          .filter((r): r is MockRecord => r != null)
          .map((rec) => ({
            startedDateTime: new Date(rec.summary.startedAt).toISOString(),
            time: rec.summary.duration ?? 0,
            request: { method: rec.request.method, url: rec.request.url, httpVersion: rec.request.httpVersion, headers: rec.request.headers.map(([name, value]) => ({ name, value })), queryString: [], cookies: [], headersSize: -1, bodySize: rec.summary.requestSize },
            response: { status: rec.response?.status ?? 0, statusText: rec.response?.statusMessage ?? '', httpVersion: rec.request.httpVersion, headers: (rec.response?.headers ?? []).map(([name, value]) => ({ name, value })), cookies: [], content: { size: rec.summary.responseSize, mimeType: rec.summary.contentType ?? '' }, redirectURL: '', headersSize: -1, bodySize: rec.summary.responseSize },
            cache: {},
            timings: { send: 0, wait: rec.timing.responseHeaders ?? 0, receive: Math.max(0, (rec.timing.responseBody ?? 0) - (rec.timing.responseHeaders ?? 0)) },
          })),
      },
    };
  }

  // ---------------------------------------------------------------- sessions, imports, rebind

  /** Remove every exchange (like `POST /api/exchanges/clear` without keeping pinned ones). */
  private clearAll(): void {
    this.records.clear();
    this.order = [];
    this.live.clear();
    this.pendingAdded.clear();
    this.pendingUpdated.clear();
    for (const id of this.paused.keys()) this.broadcast({ type: 'resumed', id });
    this.paused.clear();
    for (const s of this.sockets) if (s.matching) s.matching.clear();
    this.broadcast({ type: 'cleared' });
  }

  /** Insert `n` finished generated exchanges (imports and opened sessions). */
  private insertGenerated(n: number): number[] {
    const ids: number[] = [];
    for (let i = 0; i < n; i++) {
      const rec = createRecord(this.nextId++, this.rng, Date.now() - (n - i) * 50);
      rec.plan.pause = undefined;
      this.runUntil(rec, Date.now() + 60_000);
      this.live.delete(rec.summary.id);
      this.insert(rec, true);
      ids.push(rec.summary.id);
    }
    return ids;
  }

  private session(id: string): SessionInfo {
    const found = this.sessions.find((x) => x.id === id);
    if (!found) throw new HttpError(404, 'not-found', 'session not found');
    return found;
  }

  private registerSessionRoutes(): void {
    const r = this.route.bind(this);
    const sessionName = (raw: unknown): string => {
      const name = typeof raw === 'string' ? raw.trim() : '';
      if (!name) throw new HttpError(400, 'bad-request', 'the session name must not be empty');
      if (name.length > 200) throw new HttpError(400, 'bad-request', 'the session name must be at most 200 characters');
      return name;
    };
    const slug = (name: string): string => {
      const base = name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 48) || 'session';
      let id = base;
      for (let n = 2; id === 'autosave' || id === 'autosave-previous' || this.sessions.some((x) => x.id === id); n++) id = `${base}-${n}`;
      return id;
    };
    r('GET', /^\/api\/sessions$/, () => ({ json: [...this.sessions].sort((a, b) => b.updatedAt - a.updatedAt) }));
    r('POST', /^\/api\/sessions$/, async (_m, _u, body) => {
      const req = (body ?? {}) as Partial<SaveSessionRequest>;
      const name = sessionName(req.name);
      let ids = req.ids ? req.ids.filter((id) => this.records.has(id)) : this.order;
      if (req.ids && req.ids.length === 0) throw new HttpError(400, 'bad-request', 'no exchanges selected');
      if (req.filter) {
        const compiled = compileFilter(req.filter);
        if (!compiled.ok) throw new HttpError(400, 'invalid-filter', compiled.error.message);
        const pred = compiled.predicate;
        if (pred) ids = ids.filter((id) => {
          const rec = this.records.get(id);
          return rec ? pred(this.subject(rec)) : false;
        });
      }
      await sleep(300);
      const now = Date.now();
      const info: SessionInfo = { id: slug(name), name, kind: 'saved', exchanges: ids.length, size: 900 + ids.length * 3_700, createdAt: now, updatedAt: now };
      this.sessions.push(info);
      return { json: info };
    });
    r('PUT', /^\/api\/sessions\/([^/]+)$/, (m, _u, body) => {
      const current = this.session(decodeURIComponent(m[1] ?? ''));
      const name = sessionName((body as { name?: unknown } | undefined)?.name);
      const next: SessionInfo = current.kind === 'autosave' ? { ...current, id: slug(name), name, kind: 'saved' } : { ...current, name };
      this.sessions = this.sessions.map((x) => (x.id === current.id ? next : x));
      return { json: next };
    });
    r('DELETE', /^\/api\/sessions\/([^/]+)$/, (m) => {
      const current = this.session(decodeURIComponent(m[1] ?? ''));
      this.sessions = this.sessions.filter((x) => x.id !== current.id);
      return { status: 204 };
    });
    r('POST', /^\/api\/sessions\/([^/]+)\/open$/, async (m, _u, body) => {
      const current = this.session(decodeURIComponent(m[1] ?? ''));
      const mode: SessionOpenMode = (body as { mode?: SessionOpenMode } | undefined)?.mode ?? 'replace';
      await sleep(250);
      if (mode === 'replace') this.clearAll();
      return { json: { ids: this.insertGenerated(Math.min(400, current.exchanges)) } };
    });
    r('POST', /^\/api\/import\/fiddler$/, async (_m, _u, _body, raw) => {
      // A .saz is a ZIP archive: "PK".
      if (!raw || raw[0] !== 0x50 || raw[1] !== 0x4b) throw new HttpError(400, 'bad-request', 'could not import the Fiddler sessions: not a Fiddler .saz archive');
      await sleep(200);
      return { json: { ids: this.insertGenerated(6) } };
    });
    r('POST', /^\/api\/import\/(charles|mitmproxy)$/, async (m, _u, body, raw) => {
      let n = 6;
      if (m[1] === 'charles') {
        try {
          const doc = body ?? (raw ? JSON.parse(utf8Decode(raw)) : null);
          if (!Array.isArray(doc)) throw new Error('not an array');
          n = Math.min(500, doc.length);
        } catch {
          throw new HttpError(400, 'bad-request', 'could not import the Charles session: not a Charles JSON session: expected an array of transactions');
        }
      } else if (!raw || !/^\d+:/.test(utf8Decode(raw.slice(0, 16)))) {
        throw new HttpError(400, 'bad-request', 'could not import the mitmproxy flows: not a mitmproxy flow file');
      }
      await sleep(200);
      return { json: { ids: this.insertGenerated(n) } };
    });
    r('POST', /^\/api\/proxy\/rebind$/, async (_m, _u, body) => {
      const req = (body ?? {}) as Partial<ProxyRebindRequest>;
      const port = Number(req.port);
      if (!Number.isInteger(port) || port < 1 || port > 65535) throw new HttpError(400, 'bad-request', 'port must be between 1 and 65535');
      await sleep(150);
      // Like a port some other program holds: lets the UI's error path be exercised.
      if (port === 9) throw new HttpError(409, 'conflict', `could not move the proxy to 127.0.0.1:${port}: Address already in use (os error 98)`);
      this.livePort = port === this.settings.proxy.port ? null : port;
      return { json: this.status().proxy };
    });
  }

  // ---------------------------------------------------------------- Transport

  async fetch(input: string, init: RequestInit): Promise<Response> {
    const url = new URL(input, 'http://mock.akev');
    const method = (init.method ?? 'GET').toUpperCase();
    let raw: Uint8Array | null = null;
    let body: unknown;
    if (init.body != null) {
      if (typeof init.body === 'string') raw = utf8Encode(init.body);
      else if (init.body instanceof Uint8Array) raw = init.body;
      else if (init.body instanceof ArrayBuffer) raw = new Uint8Array(init.body);
      else if (init.body instanceof Blob) raw = new Uint8Array(await init.body.arrayBuffer());
      const ct = new Headers(init.headers).get('content-type') ?? '';
      if (raw && ct.includes('json')) {
        try {
          body = JSON.parse(utf8Decode(raw));
        } catch {
          body = undefined;
        }
      }
    }
    await sleep(this.rng.int(8, 45));
    if (init.signal?.aborted) throw new DOMException('Aborted', 'AbortError');
    for (const [m, re, handler] of this.routes) {
      if (m !== method) continue;
      const match = re.exec(url.pathname);
      if (!match) continue;
      try {
        return toResponse(await handler(match, url, body, raw));
      } catch (e) {
        if (e instanceof HttpError || e instanceof MockLicenseError) return toResponse({ status: e.status, json: { error: { code: e.code, message: e.message, ...e.details } } });
        return toResponse({ status: 500, json: { error: { code: 'internal', message: e instanceof Error ? e.message : String(e) } } });
      }
    }
    return toResponse({ status: 404, json: { error: { code: 'not-found', message: `No mock route for ${method} ${url.pathname}` } } });
  }

  openSocket(): WebSocketLike {
    return new MockSocket(this);
  }

  /** Synchronously resolve DOM-facing URLs (images, downloads) to data/blob URLs. */
  resolveUrl(input: string): string {
    let url: URL;
    try {
      url = new URL(input, 'http://mock.akev');
    } catch {
      return input;
    }
    const raw = /^\/api\/exchanges\/(\d+)\/body\/(request|response)\/raw$/.exec(url.pathname);
    if (raw) {
      const rec = this.records.get(Number(raw[1]));
      if (!rec) return input;
      const side = raw[2] as 'request' | 'response';
      const spec = side === 'request' ? rec.reqSpec : rec.resSpec;
      const data = this.materialize(rec, side);
      if (!spec || data == null) return 'data:,';
      const b64 = typeof data === 'string' ? textToBase64(data) : bytesToBase64(data);
      return `data:${spec.contentType.split(';')[0]};base64,${b64}`;
    }
    const blob = (text: string, type: string) => {
      const u = URL.createObjectURL(new Blob([text], { type }));
      this.objectUrls.push(u);
      if (this.objectUrls.length > 20) URL.revokeObjectURL(this.objectUrls.shift() as string);
      return u;
    };
    switch (url.pathname) {
      case '/api/ca/cert.pem':
        return `data:application/x-pem-file;base64,${textToBase64(MOCK_PEM)}`;
      case '/api/ca/cert.der':
        return `data:application/x-x509-ca-cert;base64,${MOCK_PEM.split('\n').filter((l) => !l.startsWith('-----')).join('')}`;
      case '/api/rules/export':
        return blob(JSON.stringify(this.rules, null, 2), 'application/json');
      case '/api/export/har':
        return blob(JSON.stringify(this.har(url), null, 2), 'application/json');
      case '/api/setup/terminal':
        return `data:text/plain;base64,${textToBase64(terminalScript(url.searchParams.get('shell') ?? 'bash', this.settings.proxy.port))}`;
      default: {
        const session = /^\/api\/sessions\/([^/]+)$/.exec(url.pathname);
        if (session) return blob(JSON.stringify({ akevSession: 1, id: decodeURIComponent(session[1] ?? '') }), 'application/octet-stream');
        return input;
      }
    }
  }
}

// =====================================================================================
// Helpers
// =====================================================================================

const MOCK_PEM = `-----BEGIN CERTIFICATE-----
MIIBszCCAVmgAwIBAgIUQWtldm9Nb2NrQ0FfMjAyNjExMDgwCgYIKoZIzj0EAwIw
LzEUMBIGA1UEAwwLQWtldm8gTW9jazEXMBUGA1UECgwOQWtldm8gKG1vY2spKTAe
Fw0yNjExMDEwMDAwMDBaFw0yNzExMDEwMDAwMDBaMC8xFDASBgNVBAMMC0FrZXZv
IE1vY2sxFzAVBgNVBAoMDkFrZXZvIChtb2NrKSkwWTATBgcqhkjOPQIBBggqhkjO
PQMBBwNCAAT7u0mOlH9oG0dQq2Q5c1v7zR3wKfJm3c1X2n4vYbqJf8b0bq6m7mZ9
k2VQe5y0pS0e9mH6o9o9Jt1kK1m8Q2a1o1MwUTAdBgNVHQ4EFgQUAKEVAKEVAK
EVOAKEVAKEVAkwHwYDVR0jBBgwFoAUAKEVAKEVAKEVAKEVAKEVAkwDwYD
VR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNIADBFAiEAqkzQ0n1c2Q==
-----END CERTIFICATE-----`;

function terminalScript(shell: string, port: number): string {
  const proxy = `http://127.0.0.1:${port}`;
  if (shell === 'fish') return `set -gx HTTP_PROXY ${proxy}\nset -gx HTTPS_PROXY ${proxy}\nset -gx NODE_EXTRA_CA_CERTS ~/.local/share/akev/ca.pem\necho "Akev interception enabled"`;
  if (shell === 'powershell') return `$env:HTTP_PROXY="${proxy}"\n$env:HTTPS_PROXY="${proxy}"\nWrite-Host "Akev interception enabled"`;
  return `export HTTP_PROXY=${proxy}\nexport HTTPS_PROXY=${proxy}\nexport NODE_EXTRA_CA_CERTS="$HOME/.local/share/akev/ca.pem"\nexport SSL_CERT_FILE="$HOME/.local/share/akev/ca.pem"\necho "Akev interception enabled"`;
}

/** Minimal cURL parser (URL, -X, -H, -d/--data*, -u). */
function parseCurl(command: string): EditableRequest {
  const tokens: string[] = [];
  const re = /'([^']*)'|"((?:\\.|[^"])*)"|(\S+)/g;
  const source = command.replace(/\\\r?\n/g, ' ').trim();
  for (let m = re.exec(source); m; m = re.exec(source)) tokens.push(m[1] ?? m[2]?.replace(/\\(.)/g, '$1') ?? m[3] ?? '');
  if (tokens[0] !== 'curl') throw new HttpError(400, 'not-curl', 'Not a curl command');
  let method = '';
  let url = '';
  const headers: [string, string][] = [];
  let data: string | null = null;
  for (let i = 1; i < tokens.length; i++) {
    const t = tokens[i] as string;
    const next = () => tokens[++i] ?? '';
    if (t === '-X' || t === '--request') method = next().toUpperCase();
    else if (t === '-H' || t === '--header') {
      const h = next();
      const c = h.indexOf(':');
      if (c > 0) headers.push([h.slice(0, c).trim(), h.slice(c + 1).trim()]);
    } else if (t === '-d' || t === '--data' || t === '--data-raw' || t === '--data-binary' || t === '--json') {
      data = next();
      if (t === '--json') headers.push(['Content-Type', 'application/json']);
    } else if (t === '-u' || t === '--user') headers.push(['Authorization', `Basic ${btoa(next())}`]);
    else if (!t.startsWith('-') && !url) url = t;
  }
  if (!url) throw new HttpError(400, 'missing-url', 'missing URL');
  return { method: method || (data != null ? 'POST' : 'GET'), url, headers, body: data != null ? textToBase64(data) : '' };
}
