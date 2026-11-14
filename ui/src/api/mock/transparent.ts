/**
 * Mock backend for transparent capture (Linux): the listener follows `settings.proxy.transparent`,
 * plans come from fixtures captured from a real daemon, and redirect activity is simulated once
 * a plan was requested (or right away with `?transparent-active`).
 */
import type { InterceptorInfo, TransparentConnection, TransparentInfo, TransparentPlan, TransparentPlanRequest, TransparentSettings, TransparentStatus } from '../types';
import { TRANSPARENT_INTERCEPTOR, TRANSPARENT_PLANS, TRANSPARENT_SYSTEM } from './transparentFixtures';

const ACTIVE_WINDOW_MS = 10 * 60_000;
/** Simulated redirected connections arrive this long after the first plan request. */
const FIRST_REDIRECT_DELAY_MS = 6_000;

const DEMO_CONNECTIONS: Omit<TransparentConnection, 'at'>[] = [
  { client: '192.168.1.23:51544', originalDestination: '140.82.112.6:443', process: 'gh' },
  { client: '192.168.1.23:51530', originalDestination: '104.18.32.47:443', process: 'node' },
  { client: '192.168.1.23:51522', originalDestination: '34.117.237.239:443', process: 'slack' },
  { client: '192.168.1.23:51518', originalDestination: '192.0.2.1:80', process: 'curl' },
];

export interface MockReply {
  status?: number;
  json: unknown;
}

export class MockTransparent {
  /** When simulated redirects started (null = none yet). */
  private redirectingSince: number | null;

  constructor(startActive: boolean, now: number = Date.now()) {
    this.redirectingSince = startActive ? now - 90_000 : null;
  }

  status(t: TransparentSettings, now: number = Date.now()): TransparentStatus {
    const port = t.port === 0 ? 8001 : t.port;
    const base: TransparentStatus = {
      enabled: t.enabled,
      supported: true,
      listening: t.enabled,
      port: t.enabled ? port : undefined,
      requestedPort: t.port,
      addresses: t.enabled ? [`127.0.0.1:${port}`, `[::1]:${port}`] : [],
      mark: t.mark,
      connections: 0,
      redirected: 0,
      direct: 0,
      loops: 0,
      recent: [],
    };
    const since = this.redirectingSince;
    if (!t.enabled || since == null || now < since) return base;
    // One connection every ~20 s since redirection "started".
    const count = Math.max(1, Math.min(240, Math.floor((now - since) / 20_000) + 1));
    const recent = Array.from({ length: Math.min(count, DEMO_CONNECTIONS.length) }, (_, i) => ({ ...DEMO_CONNECTIONS[i % DEMO_CONNECTIONS.length]!, at: now - 3_000 - i * 17_000 }));
    return { ...base, connections: count + 1, redirected: count, direct: 1, lastRedirectAt: recent[0]?.at, recent };
  }

  info(t: TransparentSettings, now: number = Date.now()): TransparentInfo {
    const listener = this.status(t, now);
    return {
      supported: true,
      active: listener.listening && listener.lastRedirectAt != null && now - listener.lastRedirectAt <= ACTIVE_WINDOW_MS,
      listener,
      system: TRANSPARENT_SYSTEM,
      slice: 'akev-capture.slice',
    };
  }

  interceptor(t: TransparentSettings, now: number = Date.now()): InterceptorInfo {
    const info = this.info(t, now);
    return {
      ...structuredClone(TRANSPARENT_INTERCEPTOR),
      active: info.active,
      metadata: { listener: info.listener, system: info.system, slice: info.slice },
    };
  }

  plan(req: TransparentPlanRequest, t: TransparentSettings, now: number = Date.now()): MockReply {
    const mode = req.mode ?? 'app';
    const backend = req.backend ?? 'nft';
    if (!['app', 'user', 'machine'].includes(mode) || !['nft', 'iptables'].includes(backend)) return badRequest('invalid options');
    if (mode === 'user' && !req.user?.trim()) return badRequest('choose the user account to capture');
    if (mode === 'machine' && req.exclude === 'mark' && t.mark === 0) {
      return badRequest('set a firewall mark first (Settings → Proxy → Transparent capture), so Akev can mark its own connections');
    }
    if (req.ports === 'custom' && !(req.customPorts ?? []).length) return badRequest('choose at least one destination port');
    const fixture = TRANSPARENT_PLANS[`${mode}-${backend}`];
    if (!fixture) return badRequest('no plan');
    const port = t.port === 0 ? 8001 : t.port;
    let plan: TransparentPlan = JSON.parse(JSON.stringify(fixture).replaceAll(':8001', `:${port}`).replaceAll('--to-ports 8001', `--to-ports ${port}`).replaceAll('port 8001', `port ${port}`));
    if (mode === 'user' && req.user) plan = JSON.parse(JSON.stringify(plan).replaceAll('nobody', req.user.trim()));
    if (mode === 'app' && req.command?.trim()) {
      const launch = plan.launch;
      if (launch) plan.launch = { ...launch, command: launch.command.replace(/"\$SHELL"$/, req.command.trim()) };
    }
    if (req.sudo === false) plan = JSON.parse(JSON.stringify(plan).replaceAll('sudo ', ''));
    if (!t.enabled) plan.warnings = [`The transparent listener isn't running: redirected connections to port ${port} will fail until you turn it on.`, ...plan.warnings];
    // The user is setting things up: simulate the rules taking effect a few seconds later.
    if (this.redirectingSince == null) this.redirectingSince = now + FIRST_REDIRECT_DELAY_MS;
    return { json: plan };
  }
}

function badRequest(message: string): MockReply {
  return { status: 400, json: { error: { code: 'bad-request', message } } };
}
