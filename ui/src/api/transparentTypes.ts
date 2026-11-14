/**
 * Transparent capture (Linux) — mirrors `akev_core::transparent` and
 * `akev_intercept::transparent` (HTTP API reference § Transparent capture). Re-exported by `types.ts`.
 */

/** `settings.proxy.transparent`. */
export interface TransparentSettings {
  /** Run the transparent listener (off by default). */
  enabled: boolean;
  /** Listener port the redirect rules point at (next free port when busy; 0 = any). */
  port: number;
  /** Firewall mark (SO_MARK) on Akev's upstream sockets, 0 = none. Needs CAP_NET_ADMIN. */
  mark: number;
}

/** One connection accepted on the transparent listener (newest first). */
export interface TransparentConnection {
  /** Unix ms. */
  at: number;
  /** `ip:port` of the client. */
  client: string;
  /** Where the client was really connecting (`ip:port`) — absent when it wasn't redirected. */
  originalDestination?: string;
  /** Detected local process. */
  process?: string;
}

/** Live listener status. */
export interface TransparentStatus {
  enabled: boolean;
  /** Original destinations can be read on this platform (Linux). */
  supported: boolean;
  listening: boolean;
  /** Port in use (absent while not listening). */
  port?: number;
  requestedPort: number;
  addresses: string[];
  /** Why it isn't listening. */
  error?: string;
  mark: number;
  /** Why upstream sockets can't be marked. */
  markError?: string;
  connections: number;
  redirected: number;
  /** Connections made to the port directly (handled by Host / SNI). */
  direct: number;
  /** Akev's own connections sent back by the rules (refused). */
  loops: number;
  /** Unix ms of the last redirected connection. */
  lastRedirectAt?: number;
  recent: TransparentConnection[];
}

/** What the commands depend on. */
export interface TransparentSystemFacts {
  platform: string;
  uid?: number;
  user?: string;
  cgroupV2: boolean;
  ownCgroup?: string;
  userManagerCgroup?: string;
  nft: boolean;
  iptables: boolean;
  systemdRun: boolean;
  executable?: string;
}

/** `GET /api/transparent`. */
export interface TransparentInfo {
  supported: boolean;
  unsupportedReason?: string;
  /** Redirected connections arrived in the last 10 minutes. */
  active: boolean;
  listener: TransparentStatus;
  system: TransparentSystemFacts;
  /** systemd slice used by per-app capture. */
  slice: string;
}

export type TransparentCaptureMode = 'app' | 'user' | 'machine';
export type TransparentPortScope = 'web' | 'all' | 'custom';
export type TransparentBackend = 'nft' | 'iptables';
export type TransparentExclusion = 'uid' | 'mark';
export type TransparentLauncher = 'systemd' | 'cgroupfs';

/** `POST /api/transparent/plan` body (all optional; daemon defaults shown). */
export interface TransparentPlanRequest {
  /** `app` (default). */
  mode?: TransparentCaptureMode;
  /** `web` = 80 + 443 (default). */
  ports?: TransparentPortScope;
  customPorts?: number[];
  /** `nft` (default). */
  backend?: TransparentBackend;
  /** Per-app: command line to launch (default: a new shell). */
  command?: string | null;
  /** Per-user: user name or uid. */
  user?: string | null;
  /** Machine-wide: exclude Akev by `uid` (default) or `mark`. */
  exclude?: TransparentExclusion;
  /** Per-app: `systemd` slice (default) or a hand-made `cgroupfs` cgroup. */
  launcher?: TransparentLauncher;
  /** IPs / CIDRs never redirected (besides loopback). */
  excludeDestinations?: string[];
  /** Per-app: CA trust environment variables on the launch command (default true). */
  trustCa?: boolean;
  /** Prefix root commands with `sudo` (default true). */
  sudo?: boolean;
}

export interface TransparentStep {
  title: string;
  /** Ready to paste (bash / zsh). */
  command: string;
  /** Needs root. */
  root: boolean;
  description?: string;
}

/** Generated commands (`POST /api/transparent/plan`, or the interceptor's activation metadata). */
export interface TransparentPlan {
  mode: TransparentCaptureMode;
  backend: TransparentBackend;
  port: number;
  summary: string;
  setup: TransparentStep[];
  launch?: TransparentStep;
  /** Prints 200 when redirection reaches Akev. */
  test: TransparentStep;
  teardown: TransparentStep[];
  /** The firewall rules alone. */
  rules: string;
  warnings: string[];
  notes: string[];
}
