import { describe, expect, test } from 'bun:test';
import type { Diagnostics } from '../api/types';
import { formatDiagnostics, issueUrl, MAX_ISSUE_URL } from './diagnostics';

function diagnostics(patch: Partial<Diagnostics> = {}): Diagnostics {
  return {
    version: '0.1.0',
    releaseDate: '2026-11-11',
    os: 'Fedora Linux 45',
    arch: 'x86_64',
    platform: 'linux',
    desktop: false,
    uptimeSecs: 3725,
    repository: 'https://github.com/akevdev/akev',
    stats: {
      exchanges: 12,
      memoryBodyBytes: 1024,
      spilledBodyBytes: 0,
      activeConnections: 1,
      totalConnections: 3,
      evicted: 0,
      droppedBodies: 0,
      diskDroppedBodies: 0,
      passedThrough: 0,
      autoReleased: 0,
      spillDiskBytes: 0,
      refusedConnections: 0,
      authFailures: 0,
    },
    settings: { proxy: { port: 8000, http2: true }, capture: { maxExchanges: 200000 } },
    recentWarnings: [{ at: '2026-11-11T10:00:00Z', level: 'WARN', target: 'akev_proxy', message: 'upstream slow' }],
    crashReports: [],
    logFile: '~/.local/share/akev/logs/akev.log',
    ...patch,
  };
}

describe('diagnostics', () => {
  test('formats a readable report', () => {
    const text = formatDiagnostics(diagnostics());
    expect(text).toContain('Akev 0.1.0 (2026-11-11)');
    expect(text).toContain('OS: Fedora Linux 45 (x86_64)');
    expect(text).toContain('Runs as: akev command line');
    expect(text).toContain('Uptime: 1 h 2 min');
    expect(text).toContain('  proxy.port: 8000');
    expect(text).toContain('  capture.maxExchanges: 200000');
    expect(text).toContain('  exchanges: 12');
    expect(text).toContain('Crash reports: none');
    expect(text).toContain('2026-11-11T10:00:00Z WARN akev_proxy: upstream slow');
  });

  test('builds a pre-filled bug report URL', () => {
    const url = new URL(issueUrl(diagnostics({ desktop: true })));
    expect(url.origin + url.pathname).toBe('https://github.com/akevdev/akev/issues/new');
    expect(url.searchParams.get('template')).toBe('bug.yml');
    expect(url.searchParams.get('version')).toBe('0.1.0');
    expect(url.searchParams.get('app')).toBe('Desktop app');
    expect(url.searchParams.get('os')).toBe('Fedora Linux 45 (x86_64)');
    expect(url.searchParams.get('logs')).toContain('Recent warnings:');
  });

  test('cuts the diagnostics to keep the URL short', () => {
    const many = Array.from({ length: 400 }, (_, i) => ({ at: `t${i}`, level: 'WARN', target: 'akev', message: `warning number ${i} with some détails` }));
    const url = issueUrl(diagnostics({ recentWarnings: many }));
    expect(url.length).toBeLessThanOrEqual(MAX_ISSUE_URL);
    expect(new URL(url).searchParams.get('logs')).toContain('cut to fit');
  });
});
