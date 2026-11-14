/**
 * Defensive parsing of `InterceptorInfo.metadata` / `ActivationResult.metadata`.
 *
 * Metadata is interceptor-specific JSON produced by the daemon (`crates/akev-intercept`).
 * Known keys are extracted when present; anything malformed is ignored rather than crashing a
 * panel.
 *
 * Shapes understood (daemon → field):
 * - adb: `devices[{id, name, state, model, manufacturer, androidVersion, sdk, emulator, active}]`
 * - docker: `containers[{id, shortId, name, names, image, state, status, engine, intercepted}]`,
 *   `engines[{kind, available, error}]`, `snippets{run{docker,podman}, compose, runHostNetwork, envFiles[], notes[]}`
 * - jvm: `processes[{pid, name, mainClass, javaVersion, commandLine, attached, active, attachedPort}]`
 * - existing terminal: `shells{bash:{label, command, reset}}`, `defaultShell`
 * - electron: `running[{pid, path, name}]`
 * - system proxy: `trust{summary, options[{title, install[], remove[], steps[]}], notes[]}`,
 *   `proxy`, `configuredProxy`, `backends[{id, name}]`
 * - manual: `proxy{url}`, `ca{path, bundlePath, mobileconfigUrl}`, `envVars[{name, value}]`, `pacUrl`, `setupUrlViaProxy`
 * - iOS: `host`, `port`, `setupUrl`, `qrPayload`, `mobileconfigUrl`, `steps[{title, detail}]`, `notes[]`
 * - any interceptor (rendered when present): `setupUrl`, `steps[]`, `note`, `notes[]`,
 *   `instructions`, `command`, `recentPaths[]`
 */

import type { TerminalShell } from '../../api/types';

export interface AdbDevice {
  id: string;
  name: string;
  /** adb state: `device` (ready), `unauthorized`, `offline`, … */
  state: string;
  model?: string;
  manufacturer?: string;
  androidVersion?: string;
  emulator: boolean;
  /** Already intercepted by this Akev instance. */
  active: boolean;
}

export interface DockerContainer {
  id: string;
  name: string;
  image: string;
  /** Human status (`Up 3 hours`). */
  status?: string;
  /** Engine state (`running`, `exited`, …). */
  state?: string;
  /** `docker` / `podman` when several engines are present. */
  engine?: string;
  active: boolean;
}

export interface DockerEngine {
  kind: string;
  available: boolean;
  error?: string;
}

export interface DockerSnippets {
  /** `docker run …` per engine, in daemon order. */
  run: { engine: string; command: string }[];
  compose?: string;
  /** Linux: `--network host` variant that works without LAN access. */
  runHostNetwork?: string;
  notes: string[];
}

export interface JvmProcess {
  pid: number;
  name: string;
  /** Attached to *this* Akev (traffic flows here). */
  active: boolean;
  /** The Akev agent is loaded and routing (possibly to another Akev session's port). */
  attached: boolean;
  /** Main class, `-jar` path or `module/class`. */
  mainClass?: string;
  javaVersion?: string;
  commandLine?: string;
  /** Proxy port the attached JVM points at. */
  attachedPort?: number;
}

export interface ElectronApp {
  pid: number;
  path: string;
  name: string;
}

export interface MetaStep {
  title?: string;
  detail?: string;
}

export interface TrustOption {
  title: string;
  install: string[];
  remove: string[];
  steps: string[];
}

export interface TrustInstructions {
  summary?: string;
  options: TrustOption[];
  notes: string[];
}

export interface EnvVar {
  name: string;
  value: string;
}

export interface InterceptorMeta {
  devices: AdbDevice[];
  containers: DockerContainer[];
  engines: DockerEngine[];
  docker: DockerSnippets | null;
  processes: JvmProcess[];
  running: ElectronApp[];
  /** Setup command per shell. */
  snippets: Partial<Record<TerminalShell, string>>;
  /** Command undoing {@link InterceptorMeta.snippets} per shell. */
  resetSnippets: Partial<Record<TerminalShell, string>>;
  /** Shells the daemon offers, in its order (empty = not specified). */
  shells: TerminalShell[];
  /** Display label per shell (`Git Bash`, `Command Prompt`). */
  shellLabels: Partial<Record<TerminalShell, string>>;
  defaultShell?: TerminalShell;
  setupUrl?: string;
  setupUrlViaProxy?: string;
  mobileconfigUrl?: string;
  pacUrl?: string;
  /** Device-facing host/port (iOS). */
  host?: string;
  port?: number;
  steps: MetaStep[];
  instructions?: string;
  /** `note` + `notes[]`. */
  notes: string[];
  command?: string;
  /** Path of the CA PEM on this machine. */
  caPath?: string;
  /** System roots + Akev CA bundle (best for SSL_CERT_FILE). */
  bundlePath?: string;
  recentPaths: string[];
  envVars: EnvVar[];
  trust: TrustInstructions | null;
  /** Proxy address the interceptor points clients at (`127.0.0.1:8000`). */
  proxy?: string;
  /** System proxy: what the OS is currently configured with (differs after a port change). */
  configuredProxy?: string;
  backends: { id: string; name: string }[];
}

const SHELLS: readonly TerminalShell[] = ['bash', 'zsh', 'fish', 'powershell', 'cmd'];

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v.trim() ? v : undefined;
}

function num(v: unknown): number | undefined {
  return typeof v === 'number' && Number.isFinite(v) ? v : undefined;
}

function bool(v: unknown): boolean {
  return v === true;
}

/** A string or a list of strings (blank ones dropped). */
function strings(v: unknown): string[] {
  if (typeof v === 'string') return v.trim() ? [v] : [];
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string' && x.trim() !== '') : [];
}

/** The objects of an array (anything else is skipped). */
function records(v: unknown): Record<string, unknown>[] {
  return Array.isArray(v) ? v.filter(isRecord) : [];
}

function isShell(key: string): key is TerminalShell {
  return (SHELLS as readonly string[]).includes(key);
}

function parseDevices(v: unknown): AdbDevice[] {
  return records(v)
    .map((d): AdbDevice | null => {
      const id = str(d.id);
      if (!id) return null;
      const model = str(d.model);
      return {
        id,
        name: str(d.name) ?? model ?? id,
        state: str(d.state) ?? 'device',
        model,
        manufacturer: str(d.manufacturer),
        androidVersion: str(d.androidVersion),
        emulator: bool(d.emulator),
        active: bool(d.active),
      };
    })
    .filter((d): d is AdbDevice => d != null);
}

function parseContainers(v: unknown): DockerContainer[] {
  return records(v)
    .map((c): DockerContainer | null => {
      const id = str(c.id);
      if (!id) return null;
      const rawName = str(c.name) ?? strings(c.names)[0] ?? str(c.shortId) ?? id.slice(0, 12);
      return {
        id,
        name: rawName.replace(/^\//, ''),
        image: str(c.image) ?? '',
        status: str(c.status),
        state: str(c.state),
        engine: str(c.engine),
        active: bool(c.intercepted),
      };
    })
    .filter((c): c is DockerContainer => c != null);
}

function parseEngines(v: unknown): DockerEngine[] {
  return records(v)
    .map((e): DockerEngine | null => {
      const kind = str(e.kind);
      if (!kind) return null;
      return { kind, available: bool(e.available), error: str(e.error) };
    })
    .filter((e): e is DockerEngine => e != null);
}

/** Docker `snippets{run, compose, runHostNetwork, notes}`. */
function parseDockerSnippets(v: unknown): DockerSnippets | null {
  if (!isRecord(v) || !('run' in v || 'compose' in v || 'runHostNetwork' in v)) return null;
  const run: DockerSnippets['run'] = [];
  if (isRecord(v.run)) {
    for (const [engine, command] of Object.entries(v.run)) {
      const c = str(command);
      if (c) run.push({ engine, command: c });
    }
  }
  return { run, compose: str(v.compose), runHostNetwork: str(v.runHostNetwork), notes: strings(v.notes) };
}

function parseProcesses(v: unknown): JvmProcess[] {
  return records(v)
    .map((p): JvmProcess | null => {
      const pid = num(p.pid);
      if (pid == null || !Number.isInteger(pid) || pid <= 0) return null;
      return {
        pid,
        name: str(p.name) ?? `PID ${pid}`,
        active: bool(p.active),
        attached: bool(p.attached),
        mainClass: str(p.mainClass),
        javaVersion: str(p.javaVersion),
        commandLine: str(p.commandLine),
        attachedPort: num(p.attachedPort),
      };
    })
    .filter((p): p is JvmProcess => p != null);
}

function parseRunning(v: unknown): ElectronApp[] {
  return records(v)
    .map((a): ElectronApp | null => {
      const pid = num(a.pid);
      const path = str(a.path) ?? '';
      if (pid == null || !Number.isInteger(pid) || pid <= 0) return null;
      return { pid, path, name: str(a.name) ?? (path.split(/[\\/]/).pop() || `PID ${pid}`) };
    })
    .filter((a): a is ElectronApp => a != null);
}

interface ShellInfo {
  snippets: Partial<Record<TerminalShell, string>>;
  resets: Partial<Record<TerminalShell, string>>;
  labels: Partial<Record<TerminalShell, string>>;
  order: TerminalShell[];
}

/** Existing terminal: `shells{ bash: { label, command, reset } }`, in the daemon's order. */
function parseShells(v: unknown): ShellInfo {
  const into: ShellInfo = { snippets: {}, resets: {}, labels: {}, order: [] };
  if (!isRecord(v)) return into;
  for (const [shell, value] of Object.entries(v)) {
    if (!isShell(shell) || !isRecord(value)) continue;
    const command = str(value.command);
    if (!command) continue;
    into.snippets[shell] = command;
    into.order.push(shell);
    const reset = str(value.reset);
    const label = str(value.label);
    if (reset) into.resets[shell] = reset;
    if (label) into.labels[shell] = label;
  }
  return into;
}

function parseSteps(v: unknown): MetaStep[] {
  if (!Array.isArray(v)) return [];
  return v
    .map((s): MetaStep | null => {
      if (typeof s === 'string') return s.trim() ? { detail: s } : null;
      if (!isRecord(s)) return null;
      const title = str(s.title);
      const detail = str(s.detail);
      if (!title && !detail) return null;
      // `{ title }` alone reads as the step itself.
      return detail ? (title ? { title, detail } : { detail }) : { detail: title };
    })
    .filter((s): s is MetaStep => s != null);
}

function parseTrust(v: unknown): TrustInstructions | null {
  if (!isRecord(v)) return null;
  const options = records(v.options)
    .map((o): TrustOption | null => {
      const title = str(o.title);
      if (!title) return null;
      const option = { title, install: strings(o.install), remove: strings(o.remove), steps: strings(o.steps) };
      return option.install.length || option.remove.length || option.steps.length ? option : null;
    })
    .filter((o): o is TrustOption => o != null);
  if (options.length === 0) return null;
  return { summary: str(v.summary), options, notes: strings(v.notes) };
}

function parseEnvVars(v: unknown): EnvVar[] {
  return records(v).flatMap((e) => {
    const name = str(e.name);
    return name && typeof e.value === 'string' ? [{ name, value: e.value }] : [];
  });
}

/** `host:port` from a proxy URL or address (`proxy`), or the manual setup's `proxy{url}`. */
function parseProxy(v: unknown): string | undefined {
  return str(isRecord(v) ? v.url : v)
    ?.replace(/^https?:\/\//, '')
    .replace(/\/$/, '');
}

/** Parse any metadata value into the known shape (missing keys → empty). */
export function parseMetadata(raw: unknown): InterceptorMeta {
  const meta = isRecord(raw) ? raw : {};
  const ca = isRecord(meta.ca) ? meta.ca : {};
  const shellInfo = parseShells(meta.shells);
  const defaultShell = typeof meta.defaultShell === 'string' && isShell(meta.defaultShell) ? meta.defaultShell : undefined;
  const notes = [...strings(meta.note), ...strings(meta.notes)];

  return {
    devices: parseDevices(meta.devices),
    containers: parseContainers(meta.containers),
    engines: parseEngines(meta.engines),
    docker: parseDockerSnippets(meta.snippets),
    processes: parseProcesses(meta.processes),
    running: parseRunning(meta.running),
    snippets: shellInfo.snippets,
    resetSnippets: shellInfo.resets,
    shells: shellInfo.order,
    shellLabels: shellInfo.labels,
    defaultShell,
    setupUrl: str(meta.setupUrl) ?? str(meta.qrPayload),
    setupUrlViaProxy: str(meta.setupUrlViaProxy),
    mobileconfigUrl: str(meta.mobileconfigUrl) ?? str(ca.mobileconfigUrl),
    pacUrl: str(meta.pacUrl),
    host: str(meta.host),
    port: num(meta.port),
    steps: parseSteps(meta.steps),
    instructions: str(meta.instructions),
    notes,
    command: str(meta.command),
    caPath: str(ca.path),
    bundlePath: str(ca.bundlePath),
    recentPaths: strings(meta.recentPaths),
    envVars: parseEnvVars(meta.envVars),
    trust: parseTrust(meta.trust),
    proxy: parseProxy(meta.proxy),
    configuredProxy: parseProxy(meta.configuredProxy),
    backends: records(meta.backends).flatMap((b) => {
      const id = str(b.id);
      return id ? [{ id, name: str(b.name) ?? id }] : [];
    }),
  };
}

// =====================================================================================
// Activation results
// =====================================================================================

export interface ActivationCheck {
  /** `push-ca`, `reverse-tunnel`, `http-proxy`, … */
  step: string;
  ok: boolean;
  detail?: string;
}

/** What a panel shows after a successful activation (`ActivationResult.metadata`). */
export interface ActivationInfo {
  /** Docker: command for an intercepted shell inside the container. */
  execCommand?: string;
  /** Docker: whether the CA could be copied into the container. */
  caCopied?: boolean;
  /** Android: proxy the device now uses (`127.0.0.1:8000`). */
  proxy?: string;
  /** Android: where the CA was pushed on the device. */
  caPath?: string;
  /** Android: how to install the pushed CA on the device. */
  installSteps: string[];
  /** Android: command that undoes the proxy setting if Akev crashes. */
  manualRecovery?: string;
  checks: ActivationCheck[];
  /** Terminal / generic: a command to run. */
  command?: string;
}

export function parseActivation(raw: unknown): ActivationInfo {
  const m = isRecord(raw) ? raw : {};
  const checks = Array.isArray(m.steps)
    ? m.steps.flatMap((s): ActivationCheck[] => {
        if (!isRecord(s) || !str(s.step)) return [];
        return [{ step: s.step as string, ok: bool(s.ok), detail: str(s.detail) }];
      })
    : [];
  return {
    execCommand: str(m.execCommand),
    caCopied: m.caCopied === undefined ? undefined : bool(m.caCopied),
    proxy: str(m.proxy),
    caPath: str(m.caPath),
    installSteps: strings(m.installSteps),
    manualRecovery: str(m.manualRecovery),
    checks,
    command: str(m.command),
  };
}

// =====================================================================================
// Display helpers
// =====================================================================================

/** Human description of an adb device state. */
export function describeDeviceState(state: string): { label: string; ready: boolean; hint?: string } {
  switch (state) {
    case 'device':
      return { label: 'Ready', ready: true };
    case 'unauthorized':
      return { label: 'Unauthorized', ready: false, hint: 'Unlock the device and accept the “Allow USB debugging” prompt.' };
    case 'offline':
      return { label: 'Offline', ready: false, hint: 'Reconnect the cable or restart adb (adb kill-server).' };
    case 'no permissions':
      return { label: 'No permission', ready: false, hint: 'Your user lacks USB permissions — add a udev rule for the device.' };
    default:
      return { label: state.charAt(0).toUpperCase() + state.slice(1), ready: false };
  }
}

/** `Google Pixel 8 · Android 15` (skips parts already in the device name). */
export function describeDevice(d: Pick<AdbDevice, 'name' | 'model' | 'manufacturer' | 'androidVersion'>): string | undefined {
  const hardware = [d.manufacturer, d.model].filter((x): x is string => !!x && !d.name.toLowerCase().includes(x.toLowerCase())).join(' ');
  const parts = [hardware || undefined, d.androidVersion ? `Android ${d.androidVersion}` : undefined].filter(Boolean);
  return parts.length ? parts.join(' · ') : undefined;
}
