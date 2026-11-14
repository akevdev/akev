/**
 * App-wide daemon state (zustand): connection, status & stats, settings, rules & rule stats,
 * interceptors, paused breakpoints, recording.
 *
 * Stream messages feed this store (see `state/connection.ts`). Mutating actions call the API
 * and update optimistically where safe; failures surface as thrown {@link ApiError}s so the
 * caller decides how to report them (usually a toast).
 */

import { create } from 'zustand';
import { useShallow } from 'zustand/react/shallow';
import { api } from '../api/client';
import type {
  ExchangeId,
  InterceptorInfo,
  PausedExchange,
  RequestResume,
  ResponseResume,
  RuleId,
  RuleSet,
  RuleStats,
  RulesPutResult,
  Settings,
  Status,
  StatusStats,
} from '../api/types';

export type ConnectionState =
  /** Not connected yet (initial attempts). */
  | 'connecting'
  /** Live. */
  | 'open'
  /** Lost the connection; retrying. */
  | 'reconnecting'
  /** The daemon rejected our token (needs a fresh `?token=` link). */
  | 'unauthorized'
  /** Stopped. */
  | 'closed';

export interface AppState {
  connection: ConnectionState;
  /** Whether a `hello` has ever been received this session. */
  hasConnected: boolean;
  /** Consecutive failed connection attempts (for the connecting screen). */
  failedAttempts: number;
  /** Unix ms of the next reconnect attempt, while reconnecting. */
  retryAt: number | null;
  /** Running against the in-browser mock backend. */
  mock: boolean;

  status: Status | null;
  stats: StatusStats | null;
  settings: Settings | null;
  rules: RuleSet | null;
  /** Rule stats keyed by rule id. */
  ruleStats: Record<RuleId, RuleStats>;
  interceptors: InterceptorInfo[];
  /** Paused exchanges in pause order. */
  paused: PausedExchange[];
  recording: boolean;

  // ---- actions
  /** Toggle (or set) server-side recording. Optimistic; reverts on failure. */
  setRecording(recording?: boolean): Promise<void>;
  /** Persist & apply settings (port changes rebind the proxy). */
  saveSettings(settings: Settings): Promise<Settings>;
  /** Persist & apply rules; per-rule errors are returned. */
  saveRules(rules: RuleSet): Promise<RulesPutResult>;
  refreshInterceptors(): Promise<InterceptorInfo[]>;
  resumeBreakpoint(id: ExchangeId, resume: RequestResume | ResponseResume): Promise<void>;
  resumeAllBreakpoints(): Promise<void>;
}

export const useAppStore = create<AppState>()((set, get) => ({
  connection: 'connecting',
  hasConnected: false,
  failedAttempts: 0,
  retryAt: null,
  mock: false,

  status: null,
  stats: null,
  settings: null,
  rules: null,
  ruleStats: {},
  interceptors: [],
  paused: [],
  recording: true,

  async setRecording(recording) {
    const next = recording ?? !get().recording;
    const prev = get().recording;
    set({ recording: next });
    try {
      const res = await api.capture.set(next);
      set({ recording: res.recording });
    } catch (e) {
      set({ recording: prev });
      throw e;
    }
  },

  async saveSettings(settings) {
    const saved = await api.settings.put(settings);
    set({ settings: saved });
    return saved;
  },

  async saveRules(rules) {
    const prev = get().rules;
    set({ rules });
    try {
      return await api.rules.put(rules);
    } catch (e) {
      set({ rules: prev });
      throw e;
    }
  },

  async refreshInterceptors() {
    const interceptors = await api.interceptors.list();
    set({ interceptors });
    return interceptors;
  },

  async resumeBreakpoint(id, resume) {
    await api.breakpoints.resume(id, resume);
    // The stream's `resumed` message also removes it; do it now for snappiness.
    set({ paused: get().paused.filter((p) => p.id !== id) });
  },

  async resumeAllBreakpoints() {
    await api.breakpoints.resumeAll();
    set({ paused: [] });
  },
}));

// =====================================================================================
// Stream reducers (called by state/connection.ts)
// =====================================================================================

export const appReducers = {
  hello(status: Status, paused: PausedExchange[], rules: RuleSet, settings: Settings): void {
    useAppStore.setState({
      status,
      stats: status.stats,
      paused,
      rules,
      settings,
      connection: 'open',
      hasConnected: true,
      failedAttempts: 0,
      retryAt: null,
    });
  },
  paused(exchange: PausedExchange): void {
    const list = useAppStore.getState().paused.filter((p) => p.id !== exchange.id);
    useAppStore.setState({ paused: [...list, exchange] });
  },
  resumed(id: ExchangeId): void {
    const list = useAppStore.getState().paused;
    if (list.some((p) => p.id === id)) useAppStore.setState({ paused: list.filter((p) => p.id !== id) });
  },
  ruleStats(stats: RuleStats[]): void {
    const next = { ...useAppStore.getState().ruleStats };
    for (const s of stats) next[s.ruleId] = s;
    useAppStore.setState({ ruleStats: next });
  },
  replaceRuleStats(stats: RuleStats[]): void {
    const next: Record<RuleId, RuleStats> = {};
    for (const s of stats) next[s.ruleId] = s;
    useAppStore.setState({ ruleStats: next });
  },
  stats(stats: StatusStats): void {
    const status = useAppStore.getState().status;
    useAppStore.setState({ stats, status: status ? { ...status, stats } : status });
  },
};

// =====================================================================================
// Selectors
// =====================================================================================

/** Active interceptors (for the status bar / Connect page). Shallow-compared. */
export function useActiveInterceptors(): InterceptorInfo[] {
  return useAppStore(useShallow((s) => s.interceptors.filter((i) => i.active)));
}

/** Primary proxy address (`127.0.0.1:8000`). */
export function proxyAddress(status: Status | null): string | null {
  if (!status) return null;
  const host = status.proxy.bindAddress === '0.0.0.0' || status.proxy.bindAddress === '::' ? '127.0.0.1' : status.proxy.bindAddress;
  return `${host}:${status.proxy.port}`;
}

/**
 * Why a sent or replayed exchange isn't in Traffic (`recorded: false` from `POST /api/send` /
 * `…/replay`): recording is paused, or the capture settings ignore the host.
 */
export function notStoredMessage(recording: boolean = useAppStore.getState().recording): string {
  return recording ? 'Not stored — the capture settings ignore this host' : "Recording is paused — this response wasn't stored";
}
