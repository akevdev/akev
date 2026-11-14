/**
 * Settings + rules export/import (Settings → Data). One JSON file holds the daemon settings,
 * the rule set and the UI preferences, so a setup moves between machines in one step.
 *
 * Import is forgiving: it accepts our export, a bare `Settings` object or a bare `RuleSet`,
 * and merges known settings keys over the current ones (type-checked), so files from older
 * or newer versions never corrupt the configuration.
 */

import type {
  BreakpointSettings,
  CaptureSettings,
  ClientCertificate,
  DnsOverride,
  Http3Settings,
  Http3UpstreamMode,
  ProxyAccessSettings,
  ProxySettings,
  RuleSet,
  SessionSettings,
  Settings,
  TlsProfile,
  TlsProfileOverride,
  TlsProfileSettings,
  TransparentSettings,
  UpdateSettings,
  UpstreamProxyMode,
  WireguardSettings,
} from '../../api/types';
import { isTlsProfile } from '../../lib/tlsProfiles';

export const CONFIG_EXPORT_KIND = 'akev-config';

export interface ConfigExport {
  kind: typeof CONFIG_EXPORT_KIND;
  version: 1;
  exportedAt: string;
  appVersion?: string;
  settings?: Omit<Settings, 'ui'>;
  rules?: RuleSet;
  ui?: unknown;
}

export interface ImportedConfig {
  /** Partial settings object (unknown shape until merged). */
  settings?: Record<string, unknown>;
  rules?: RuleSet;
  ui?: unknown;
}

export type ParseResult = { ok: true; config: ImportedConfig; summary: string[] } | { ok: false; error: string };

/**
 * Build the export file. Client-certificate passphrases are left out — the file is meant to
 * be copied around; importing it keeps the passphrases already configured on that machine.
 */
export function buildConfigExport(parts: { settings?: Settings | null; rules?: RuleSet | null; ui?: unknown; appVersion?: string }, now: Date = new Date()): ConfigExport {
  const out: ConfigExport = { kind: CONFIG_EXPORT_KIND, version: 1, exportedAt: now.toISOString() };
  if (parts.appVersion) out.appVersion = parts.appVersion;
  if (parts.settings) {
    const { ui: _ui, ...rest } = parts.settings;
    out.settings = { ...rest, proxy: { ...rest.proxy, clientCertificates: rest.proxy.clientCertificates.map(({ host, path }) => ({ host, path })) } };
  }
  if (parts.rules) out.rules = parts.rules;
  if (parts.ui !== undefined) out.ui = parts.ui;
  return out;
}

/** Number of client-certificate passphrases {@link buildConfigExport} leaves out. */
export function omittedPassphrases(settings: Settings | null | undefined): number {
  return settings ? settings.proxy.clientCertificates.filter((c) => !!c.passphrase).length : 0;
}

export function configFilename(now: Date = new Date()): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `akev-config_${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}.json`;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function isRuleSet(v: unknown): v is RuleSet {
  return isRecord(v) && typeof v.version === 'number' && Array.isArray(v.items);
}

/** Rule exports from other tools (e.g. HTTP Toolkit): an `items` array without our version. */
function isForeignRules(v: unknown): v is RuleSet {
  return isRecord(v) && Array.isArray(v.items) && typeof v.version !== 'number' && !looksLikeSettings(v);
}

function looksLikeSettings(v: unknown): v is Record<string, unknown> {
  return isRecord(v) && (isRecord(v.proxy) || isRecord(v.capture));
}

function countRules(items: unknown[]): number {
  let n = 0;
  for (const i of items) {
    if (!isRecord(i)) continue;
    if (i.type === 'group' && Array.isArray(i.items)) n += countRules(i.items);
    else n += 1;
  }
  return n;
}

/** Parse an imported file's text. */
export function parseConfigImport(text: string): ParseResult {
  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch {
    return { ok: false, error: "This file isn't valid JSON." };
  }
  const config: ImportedConfig = {};
  if (isRecord(json) && json.kind === CONFIG_EXPORT_KIND) {
    if (looksLikeSettings(json.settings)) config.settings = json.settings;
    if (isRuleSet(json.rules)) config.rules = json.rules;
    if (json.ui !== undefined && isRecord(json.ui)) config.ui = json.ui;
  } else if (isRuleSet(json) || isForeignRules(json)) {
    // Foreign rule sets are converted by the daemon (`/api/rules/import`) before applying.
    config.rules = json;
  } else if (looksLikeSettings(json)) {
    config.settings = json;
    if (isRecord(json.ui)) config.ui = json.ui;
  } else {
    return { ok: false, error: "This doesn't look like an Akev settings or rules export." };
  }
  const summary: string[] = [];
  if (config.settings) summary.push('proxy & capture settings');
  if (config.rules) {
    const n = countRules(config.rules.items);
    summary.push(`${n} rule${n === 1 ? '' : 's'}`);
  }
  if (config.ui) summary.push('appearance & layout preferences');
  if (summary.length === 0) return { ok: false, error: 'The file contains nothing Akev can import.' };
  return { ok: true, config, summary };
}

// ---- type-checked merge

const num = (v: unknown, fallback: number, min = 0, max = Number.MAX_SAFE_INTEGER) =>
  typeof v === 'number' && Number.isFinite(v) && v >= min && v <= max ? Math.round(v) : fallback;
const bool = (v: unknown, fallback: boolean) => (typeof v === 'boolean' ? v : fallback);
const strList = (v: unknown, fallback: string[]) => (Array.isArray(v) ? v.filter((s): s is string => typeof s === 'string' && s.trim() !== '') : fallback);
const str = (v: unknown, fallback: string) => (typeof v === 'string' ? v : fallback);
const oneOf = <T extends string>(v: unknown, values: readonly T[], fallback: T): T => (values.includes(v as T) ? (v as T) : fallback);

function mergeProxy(cur: ProxySettings, raw: unknown): ProxySettings {
  if (!isRecord(raw)) return cur;
  const up = isRecord(raw.upstreamProxy) ? raw.upstreamProxy : {};
  const modes: UpstreamProxyMode[] = ['direct', 'system', 'manual', 'pac'];
  return {
    port: num(raw.port, cur.port, 1, 65535),
    bindAddress: str(raw.bindAddress, cur.bindAddress) || cur.bindAddress,
    http2: bool(raw.http2, cur.http2),
    socks: bool(raw.socks, cur.socks),
    tlsPassthrough: strList(raw.tlsPassthrough, cur.tlsPassthrough),
    ignoreUpstreamCertErrors: strList(raw.ignoreUpstreamCertErrors, cur.ignoreUpstreamCertErrors),
    trustedCaPaths: strList(raw.trustedCaPaths, cur.trustedCaPaths),
    clientCertificates: Array.isArray(raw.clientCertificates)
      ? raw.clientCertificates
          .filter(isRecord)
          .filter((c) => typeof c.host === 'string' && typeof c.path === 'string')
          .map((c): ClientCertificate => {
            // Exports omit passphrases: keep the one already set for the same certificate.
            const passphrase = typeof c.passphrase === 'string' ? c.passphrase : cur.clientCertificates.find((x) => x.host === c.host && x.path === c.path)?.passphrase;
            return { host: c.host as string, path: c.path as string, ...(passphrase ? { passphrase } : {}) };
          })
      : cur.clientCertificates,
    upstreamProxy: {
      mode: modes.includes(up.mode as UpstreamProxyMode) ? (up.mode as UpstreamProxyMode) : cur.upstreamProxy.mode,
      url: str(up.url, cur.upstreamProxy.url),
      noProxy: strList(up.noProxy, cur.upstreamProxy.noProxy),
    },
    dnsOverrides: Array.isArray(raw.dnsOverrides)
      ? raw.dnsOverrides.filter(isRecord).filter((d) => typeof d.host === 'string' && typeof d.address === 'string').map((d): DnsOverride => ({ host: d.host as string, address: d.address as string }))
      : cur.dnsOverrides,
    dohUrl: raw.dohUrl === null ? null : typeof raw.dohUrl === 'string' ? raw.dohUrl || null : cur.dohUrl,
    connectTimeoutMs: num(raw.connectTimeoutMs, cur.connectTimeoutMs),
    responseTimeoutMs: num(raw.responseTimeoutMs, cur.responseTimeoutMs),
    bodyIdleTimeoutMs: num(raw.bodyIdleTimeoutMs, cur.bodyIdleTimeoutMs),
    ruleBodyBudgetBytes: num(raw.ruleBodyBudgetBytes, cur.ruleBodyBudgetBytes),
    preserveHeaderCase: bool(raw.preserveHeaderCase, cur.preserveHeaderCase),
    tlsProfile: mergeTlsProfile(cur.tlsProfile, raw.tlsProfile),
    wireguard: mergeWireguard(cur.wireguard, raw.wireguard),
    transparent: mergeTransparent(cur.transparent, raw.transparent),
    http3: mergeHttp3(cur.http3, raw.http3),
    access: mergeAccess(cur.access, raw.access),
  };
}

function mergeTlsProfile(cur: TlsProfileSettings, raw: unknown): TlsProfileSettings {
  if (!isRecord(raw)) return cur;
  return {
    profile: typeof raw.profile === 'string' && isTlsProfile(raw.profile) ? raw.profile : cur.profile,
    overrides: Array.isArray(raw.overrides)
      ? raw.overrides
          .filter(isRecord)
          .filter((o) => typeof o.host === 'string' && o.host.trim() !== '' && typeof o.profile === 'string' && isTlsProfile(o.profile))
          .map((o): TlsProfileOverride => ({ host: o.host as string, profile: o.profile as TlsProfile }))
      : cur.overrides,
  };
}

function mergeWireguard(cur: WireguardSettings, raw: unknown): WireguardSettings {
  if (!isRecord(raw)) return cur;
  // `null` / `""` = all interfaces (the daemon leaves the field out then).
  const bindAddress = raw.bindAddress === null ? null : typeof raw.bindAddress === 'string' ? raw.bindAddress || null : cur.bindAddress;
  return {
    enabled: bool(raw.enabled, cur.enabled),
    port: num(raw.port, cur.port, 1, 65535),
    ...(bindAddress ? { bindAddress } : {}),
  };
}

function mergeTransparent(cur: TransparentSettings, raw: unknown): TransparentSettings {
  if (!isRecord(raw)) return cur;
  return {
    enabled: bool(raw.enabled, cur.enabled),
    port: num(raw.port, cur.port, 0, 65535),
    mark: num(raw.mark, cur.mark, 0, 0xffff_ffff),
  };
}

const HTTP3_UPSTREAM_MODES: readonly Http3UpstreamMode[] = ['off', 'alt-svc', 'prefer'];

function mergeHttp3(cur: Http3Settings, raw: unknown): Http3Settings {
  if (!isRecord(raw)) return cur;
  return {
    upstream: oneOf(raw.upstream, HTTP3_UPSTREAM_MODES, cur.upstream),
    listen: bool(raw.listen, cur.listen),
    listenPort: num(raw.listenPort, cur.listenPort, 0, 65535),
  };
}

const ACCESS_POLICIES: readonly ProxyAccessSettings['policy'][] = ['private', 'allowlist', 'any'];

function mergeAccess(cur: ProxyAccessSettings, raw: unknown): ProxyAccessSettings {
  if (!isRecord(raw)) return cur;
  return {
    policy: oneOf(raw.policy, ACCESS_POLICIES, cur.policy),
    allow: strList(raw.allow, cur.allow),
    block: strList(raw.block, cur.block),
    username: str(raw.username, cur.username),
    password: str(raw.password, cur.password),
    allowLoopbackTargets: bool(raw.allowLoopbackTargets, cur.allowLoopbackTargets),
    trustProxyProtocol: bool(raw.trustProxyProtocol, cur.trustProxyProtocol),
  };
}

function mergeBreakpoints(cur: BreakpointSettings, raw: unknown): BreakpointSettings {
  if (!isRecord(raw)) return cur;
  return {
    maxPaused: num(raw.maxPaused, cur.maxPaused, 0, 1_000_000),
    autoReleaseSecs: num(raw.autoReleaseSecs, cur.autoReleaseSecs, 0, 86_400 * 7),
  };
}

function mergeCapture(cur: CaptureSettings, raw: unknown): CaptureSettings {
  if (!isRecord(raw)) return cur;
  return {
    maxBodyBytes: num(raw.maxBodyBytes, cur.maxBodyBytes),
    memoryBudgetBytes: num(raw.memoryBudgetBytes, cur.memoryBudgetBytes),
    maxExchanges: num(raw.maxExchanges, cur.maxExchanges),
    spillMaxBytes: num(raw.spillMaxBytes, cur.spillMaxBytes),
    eventQueueBytes: num(raw.eventQueueBytes, cur.eventQueueBytes),
    recordTunnels: bool(raw.recordTunnels, cur.recordTunnels),
    recordTlsFailures: bool(raw.recordTlsFailures, cur.recordTlsFailures),
    ignoreHosts: strList(raw.ignoreHosts, cur.ignoreHosts),
    detectProcesses: bool(raw.detectProcesses, cur.detectProcesses),
  };
}

function mergeSessions(cur: SessionSettings, raw: unknown): SessionSettings {
  if (!isRecord(raw)) return cur;
  return {
    autosave: bool(raw.autosave, cur.autosave),
    autosaveIntervalSecs: num(raw.autosaveIntervalSecs, cur.autosaveIntervalSecs, 1, 86_400),
    restoreOnStartup: bool(raw.restoreOnStartup, cur.restoreOnStartup),
    autosaveMaxExchanges: num(raw.autosaveMaxExchanges, cur.autosaveMaxExchanges),
    autosaveMaxBytes: num(raw.autosaveMaxBytes, cur.autosaveMaxBytes),
  };
}

function mergeUpdates(cur: UpdateSettings, raw: unknown): UpdateSettings {
  if (!isRecord(raw)) return cur;
  return { checkAutomatically: bool(raw.checkAutomatically, cur.checkAutomatically) };
}

/**
 * Merge imported (untrusted) settings over the current ones, key by key: anything the file
 * doesn't contain (or gets wrong) keeps its current value. `ui` is kept. The result replaces
 * the whole settings object (`PUT /api/settings`), so every field must be carried through.
 */
export function mergeSettings(current: Settings, incoming: Record<string, unknown>): Settings {
  return {
    proxy: mergeProxy(current.proxy, incoming.proxy),
    capture: mergeCapture(current.capture, incoming.capture),
    sessions: mergeSessions(current.sessions, incoming.sessions),
    updates: mergeUpdates(current.updates, incoming.updates),
    breakpoints: mergeBreakpoints(current.breakpoints, incoming.breakpoints),
    ui: current.ui,
  };
}
