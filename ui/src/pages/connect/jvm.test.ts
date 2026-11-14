import { describe, expect, test } from 'bun:test';
import { MOCK_JVM_INTERCEPTOR, mockJvmActivate, mockJvmDeactivate, withMockJvm } from '../../api/mock/jvm';
import { MockServer } from '../../api/mock/server';
import type { ActivationResult, InterceptorInfo } from '../../api/types';
import { opensSheet, panelKindFor, primaryActionFor } from './catalog';
import { agentWarnings, jvmSubtitle, shortProcessName } from './jvm';
import { parseMetadata } from './metadata';

describe('JVM metadata', () => {
  test("parses the daemon's process list", () => {
    const m = parseMetadata({
      processes: [
        { pid: 4242, name: 'Main', mainClass: 'org.acme.Main', javaVersion: '21.0.5', commandLine: 'java -cp app org.acme.Main', attached: true, active: true, attachedPort: 8000 },
        { pid: 77, name: 'app.jar', mainClass: '/srv/app.jar', attached: true, active: false, attachedPort: 9000 },
        { pid: 5, name: 'Tool', attached: false, active: false, attachedPort: null, javaVersion: null },
        { pid: -1 },
        { pid: '12', name: 'not a number' },
        'garbage',
      ],
      notes: ['one', 'two'],
    });
    expect(m.processes).toEqual([
      { pid: 4242, name: 'Main', active: true, attached: true, mainClass: 'org.acme.Main', javaVersion: '21.0.5', commandLine: 'java -cp app org.acme.Main', attachedPort: 8000 },
      { pid: 77, name: 'app.jar', active: false, attached: true, mainClass: '/srv/app.jar', javaVersion: undefined, commandLine: undefined, attachedPort: 9000 },
      { pid: 5, name: 'Tool', active: false, attached: false, mainClass: undefined, javaVersion: undefined, commandLine: undefined, attachedPort: undefined },
    ]);
    expect(m.notes).toEqual(['one', 'two']);
  });

  test('the jvm interceptor opens its own panel', () => {
    const info = { id: 'jvm', needsConfig: true, category: 'runtime' as const, active: false };
    expect(panelKindFor(info)).toBe('jvm');
    expect(opensSheet(info)).toBe(true);
    expect(primaryActionFor(info)).toEqual({ kind: 'setup', label: 'Set up' });
    expect(primaryActionFor({ ...info, active: true })).toEqual({ kind: 'manage', label: 'Manage' });
  });
});

describe('JVM panel helpers', () => {
  test('short names', () => {
    expect(shortProcessName('org.acme.billing.BillingServiceApplication')).toBe('BillingServiceApplication');
    expect(shortProcessName('/srv/inventory/inventory-api.jar')).toBe('inventory-api.jar');
    expect(shortProcessName('C:\\apps\\tool.WAR')).toBe('tool.WAR');
    expect(shortProcessName('acme.app/org.acme.Main --flag')).toBe('Main');
    expect(shortProcessName('MainKt')).toBe('MainKt');
  });

  test('subtitles', () => {
    expect(jvmSubtitle({ pid: 1, name: 'Main', mainClass: 'org.acme.Main', javaVersion: '21.0.5' })).toBe('PID 1 · Java 21.0.5 · org.acme.Main');
    expect(jvmSubtitle({ pid: 2, name: 'MainKt', mainClass: 'MainKt' })).toBe('PID 2');
    expect(jvmSubtitle({ pid: 3, name: 'app.jar', mainClass: '/srv/app.jar' })).toBe('PID 3 · /srv/app.jar');
  });

  test('agent warnings', () => {
    expect(agentWarnings({ agent: { warnings: ['a', '', 3, 'b'] } })).toEqual(['a', 'b']);
    for (const bad of [null, undefined, 'x', {}, { agent: null }, { agent: { warnings: 'a' } }]) expect(agentWarnings(bad)).toEqual([]);
  });
});

describe('mock jvm interceptor', () => {
  const processes = (i: InterceptorInfo) => parseMetadata(i.metadata).processes;

  test('sits after Electron, once', () => {
    const list = withMockJvm([{ ...MOCK_JVM_INTERCEPTOR, id: 'electron' }, { ...MOCK_JVM_INTERCEPTOR, id: 'manual' }]);
    expect(list.map((i) => i.id)).toEqual(['electron', 'jvm', 'manual']);
    expect(withMockJvm(list)).toBe(list);
  });

  test('attach and restore one process', () => {
    const pid = processes(MOCK_JVM_INTERCEPTOR)[1]?.pid ?? 0;
    const { info, result } = mockJvmActivate(MOCK_JVM_INTERCEPTOR, { pid, interceptLoopback: true }, 8001);
    expect(result.success).toBe(true);
    expect(result.message).toContain(`(PID ${pid})`);
    expect(info.active).toBe(true);
    const attached = processes(info).find((p) => p.pid === pid);
    expect(attached).toMatchObject({ active: true, attached: true, attachedPort: 8001 });
    expect(processes(info).filter((p) => p.active)).toHaveLength(1);
    const restored = mockJvmDeactivate(info, { pid });
    expect(restored.active).toBe(false);
    expect(processes(restored).some((p) => p.attached)).toBe(false);
    // The original fixture isn't mutated.
    expect(processes(MOCK_JVM_INTERCEPTOR).some((p) => p.attached)).toBe(false);
  });

  test('the Spring demo app reports the OkHttp limitation', () => {
    const pid = processes(MOCK_JVM_INTERCEPTOR)[0]?.pid ?? 0;
    const { result } = mockJvmActivate(MOCK_JVM_INTERCEPTOR, { pid: String(pid) }, 8000);
    expect(agentWarnings(result.metadata)).toHaveLength(1);
  });

  test('bad pids fail like the daemon', () => {
    expect(mockJvmActivate(MOCK_JVM_INTERCEPTOR, {}, 8000).result.success).toBe(false);
    expect(mockJvmActivate(MOCK_JVM_INTERCEPTOR, { pid: 999999 }, 8000).result).toMatchObject({ success: false });
  });

  test('through the mock server', async () => {
    const server = new MockServer({ license: 'pro' });
    server.start();
    try {
      const list = (await (await server.fetch('/api/interceptors', { method: 'GET' })).json()) as InterceptorInfo[];
      const jvm = list.find((i) => i.id === 'jvm');
      expect(jvm?.needsConfig).toBe(true);
      const pid = processes(jvm as InterceptorInfo)[2]?.pid;
      const res = (await (await server.fetch('/api/interceptors/jvm/activate', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ options: { pid } }) })).json()) as ActivationResult;
      expect(res.success).toBe(true);
      let after = ((await (await server.fetch('/api/interceptors', { method: 'GET' })).json()) as InterceptorInfo[]).find((i) => i.id === 'jvm');
      expect(after?.active).toBe(true);
      expect(processes(after as InterceptorInfo).find((p) => p.pid === pid)?.active).toBe(true);
      const stop = await server.fetch('/api/interceptors/jvm/deactivate', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ options: { pid } }) });
      expect(stop.status).toBe(204);
      after = ((await (await server.fetch('/api/interceptors', { method: 'GET' })).json()) as InterceptorInfo[]).find((i) => i.id === 'jvm');
      expect(after?.active).toBe(false);
    } finally {
      server.stop();
    }
  });
});
