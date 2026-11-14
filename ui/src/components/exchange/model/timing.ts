/**
 * Waterfall layout for the Timing tab. Converts the daemon's `Timing` (absolute start in µs,
 * everything else ms since start; DNS/connect/TLS as durations) into positioned phases on a
 * shared scale, tolerating missing or inconsistent phases. Paused breakpoint time is cut out of
 * the timeline and reported separately. No React.
 */

import type { ExchangeState, Timing } from '../../../api/types';

export type PhaseId = 'received' | 'dns' | 'connect' | 'tls' | 'sent' | 'wait' | 'download' | 'pending';

export interface Phase {
  id: PhaseId;
  label: string;
  description: string;
  /** Offset from the start of the exchange (ms, paused time removed). */
  start: number;
  /** Measured duration (ms) — what labels show. */
  duration: number;
  /** Visual width on the timeline (ms); differs from `duration` only when `approximate`. */
  span: number;
}

export interface Waterfall {
  phases: Phase[];
  /** Total active duration (ms, paused time excluded); null while unknown. */
  total: number | null;
  /** Rightmost point on the scale (ms). */
  end: number;
  /** Time spent paused at breakpoints (ms). */
  paused: number;
  /** Where the pause was cut out of the timeline (ms), if it could be placed. */
  pauseAt: number | null;
  /** Offset at which the exchange was aborted, if it was. */
  abortedAt: number | null;
  /** Still in progress (a live `pending` phase may be included). */
  pending: boolean;
  /** Absolute start (unix ms). */
  startedAt: number;
  /** Time to first byte: request sent → response head (ms, = the Waiting phase), when known. */
  ttfb: number | null;
  /**
   * The connection-setup durations didn't fit before the request was sent (inconsistent
   * timings): their bars were squeezed into the available window; durations are as measured.
   */
  approximate: boolean;
}

export const PHASE_INFO: Readonly<Record<PhaseId, { label: string; description: string }>> = {
  received: { label: 'Request received', description: 'Receiving the request from the client (headers and body)' },
  dns: { label: 'DNS lookup', description: 'Resolving the upstream hostname' },
  connect: { label: 'Connect', description: 'Opening the TCP connection to the server' },
  tls: { label: 'TLS handshake', description: 'Negotiating TLS with the server' },
  sent: { label: 'Request sent', description: 'Forwarding the request to the server' },
  wait: { label: 'Waiting (TTFB)', description: 'Waiting for the first byte of the response' },
  download: { label: 'Content download', description: 'Receiving the response body' },
  pending: { label: 'In progress', description: 'Still running' },
};

/** Bar colour of each phase (Timing tab and traffic timeline). */
export const PHASE_COLORS: Readonly<Record<PhaseId, string>> = {
  received: 'var(--text-subtle)',
  dns: 'var(--k-font)',
  connect: 'var(--m-put)',
  tls: 'var(--k-media)',
  sent: 'var(--k-ws)',
  wait: 'var(--success)',
  download: 'var(--info)',
  pending: 'var(--text-subtle)',
};

const MIN_VISIBLE = 0.001;

function phase(id: PhaseId, start: number, duration: number): Phase {
  return { id, label: PHASE_INFO[id].label, description: PHASE_INFO[id].description, start, duration, span: duration };
}

/**
 * Build the waterfall. `now` (unix ms) enables the live `pending` phase for unfinished
 * exchanges.
 */
export function buildWaterfall(timing: Timing, options: { state?: ExchangeState; now?: number } = {}): Waterfall {
  const t = timing;
  const startedAt = t.start / 1000;
  const paused = Math.max(0, t.pausedDuration ?? 0);
  const out: Phase[] = [];

  const recvEnd = t.requestBody ?? t.requestHeaders;
  if (recvEnd != null && recvEnd > 0) out.push(phase('received', 0, recvEnd));

  // Upstream connection setup starts once the request head is parsed.
  const setupStart = t.requestHeaders ?? 0;
  const setup: Phase[] = [];
  let cursor = setupStart;
  for (const [id, d] of [
    ['dns', t.dnsDuration],
    ['connect', t.connectDuration],
    ['tls', t.tlsDuration],
  ] as const) {
    if (d != null && d > 0) {
      setup.push(phase(id, cursor, d));
      cursor += d;
    }
  }
  // The request can't have been sent before the connection was ready. If the setup durations
  // overrun the first absolute mark, trust the marks (they define the total) and squeeze the
  // setup bars into the window before it, keeping the measured durations as labels.
  const firstMark = t.requestSent ?? t.responseHeaders ?? t.responseBody ?? t.aborted;
  let approximate = false;
  if (firstMark != null && setup.length && cursor > firstMark + 0.01) {
    approximate = true;
    const from = Math.min(setupStart, firstMark);
    const room = Math.max(0, firstMark - from);
    const sum = cursor - setupStart;
    let c = from;
    for (const p of setup) {
      p.start = c;
      p.span = sum > 0 ? (p.duration / sum) * room : 0;
      c += p.span;
    }
    cursor = c;
  }
  out.push(...setup);
  const connEnd = cursor;

  // Sequential chain from here, anchored on the absolute marks.
  const chain: Phase[] = [];
  const sendStart = connEnd;
  if (t.requestSent != null && t.requestSent > sendStart) chain.push(phase('sent', sendStart, t.requestSent - sendStart));
  let chainEnd = t.requestSent ?? sendStart;
  if (t.responseHeaders != null) {
    const ws = Math.min(chainEnd, t.responseHeaders);
    if (t.responseHeaders > ws) chain.push(phase('wait', ws, t.responseHeaders - ws));
    chainEnd = Math.max(chainEnd, t.responseHeaders);
  }
  if (t.responseBody != null && t.responseHeaders != null) {
    const ds = Math.min(chainEnd, t.responseBody);
    if (t.responseBody > ds) chain.push(phase('download', ds, t.responseBody - ds));
    chainEnd = Math.max(chainEnd, t.responseBody);
  }

  // Cut the paused time out of the phase that most plausibly contains it.
  let pauseAt: number | null = null;
  let removed = 0;
  if (paused > 0) {
    const candidates = chain.filter((p) => p.duration >= paused * 0.98).sort((a, b) => b.duration - a.duration);
    const host = candidates.find((p) => p.id === 'sent') ?? candidates.find((p) => p.id === 'download') ?? candidates[0];
    if (host) {
      removed = Math.min(paused, host.duration);
      pauseAt = host.start;
      host.duration -= removed;
      host.span = host.duration;
      for (const p of chain) if (p.start > host.start) p.start -= removed;
    }
  }
  out.push(...chain.filter((p) => p.duration > MIN_VISIBLE));

  const finished = t.responseBody ?? t.aborted;
  const abortedAt = t.aborted != null ? t.aborted - (pauseAt != null && t.aborted > pauseAt ? removed : 0) : null;
  let total: number | null = finished != null ? Math.max(0, finished - removed) : null;

  const done = options.state === 'complete' || options.state === 'aborted' || finished != null;
  let pending = false;
  if (!done) {
    pending = true;
    if (options.now != null) {
      const elapsed = Math.max(0, options.now - startedAt - paused);
      const last = out.reduce((m, p) => Math.max(m, p.start + p.span), 0);
      if (elapsed > last) {
        const id: PhaseId = 'pending';
        const label = t.responseHeaders != null ? 'Downloading…' : t.requestSent != null ? 'Waiting for response…' : 'In progress…';
        out.push({ id, label, description: PHASE_INFO.pending.description, start: last, duration: elapsed - last, span: elapsed - last });
      }
      total = null;
    }
  }

  const end = Math.max(total ?? 0, abortedAt ?? 0, ...out.map((p) => p.start + p.span));
  // TTFB as akev-core defines it: from the request being sent to the response head (the
  // Waiting phase), with any breakpoint pause removed.
  const wait = out.find((p) => p.id === 'wait');
  const ttfb = t.responseHeaders == null ? null : wait ? wait.duration : 0;
  return { phases: out, total, end, paused, pauseAt, abortedAt, pending, startedAt, ttfb, approximate };
}

/** Nice tick values (0, step, 2·step, …) covering `max` with about `target` intervals. */
export function niceTicks(max: number, target = 4): number[] {
  if (!(max > 0)) return [0, 1];
  const raw = max / target;
  const pow = 10 ** Math.floor(Math.log10(raw));
  const step = ([1, 2, 2.5, 5, 10].map((m) => m * pow).find((s) => s >= raw) ?? 10 * pow) as number;
  const ticks: number[] = [];
  for (let v = 0; v < max + step * 0.999; v += step) ticks.push(Number(v.toPrecision(12)));
  if ((ticks[ticks.length - 1] ?? 0) < max) ticks.push(Number((ticks.length * step).toPrecision(12)));
  return ticks;
}

/** Compact tick label: `0`, `250 ms`, `1.5 s`. */
export function tickLabel(ms: number): string {
  if (ms === 0) return '0';
  if (ms < 1) return `${Number(ms.toFixed(2))} ms`;
  if (ms < 1000) return `${Number(ms.toFixed(ms < 10 ? 1 : 0))} ms`;
  return `${Number((ms / 1000).toFixed(ms < 10_000 ? 2 : 1))} s`;
}
