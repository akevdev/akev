/**
 * Mock `jvm` interceptor (`?mock`): a catalogue entry shaped like the daemon's (captured from a
 * real `GET /api/interceptors`, demo processes) and attach / restore per `options.pid`.
 */
import type { ActivationResult, InterceptorInfo } from '../types';

const NOTES = [
  "Intercepted after attaching: HttpURLConnection (and what's built on it: Spring RestTemplate's default, Feign, Jersey…), java.net.http.HttpClient, and clients created from then on with the JVM's default proxy & trust settings (OkHttp, Apache HttpClient, Netty with default trust).",
  "Clients created before attaching with their own trust settings (e.g. an OkHttpClient built at startup) keep connecting directly; Akev then leaves the JVM-wide proxy properties alone so they don't break.",
  'Requests to localhost stay direct unless "Include localhost" is on. Restore puts the JVM\'s original settings back; quitting Akev restores every attached JVM.',
];

interface MockJvm {
  pid: number;
  name: string;
  mainClass: string;
  commandLine: string;
  javaVersion: string;
  javaHome: string;
  attached: boolean;
  active: boolean;
  attachedPort: number | null;
  attachedAt: number | null;
  interceptLoopback: boolean | null;
}

function jvm(pid: number, name: string, mainClass: string, commandLine: string, javaVersion: string, javaHome: string): MockJvm {
  return { pid, name, mainClass, commandLine, javaVersion, javaHome, attached: false, active: false, attachedPort: null, attachedAt: null, interceptLoopback: null };
}

export const MOCK_JVM_INTERCEPTOR: InterceptorInfo = {
  id: 'jvm',
  name: 'Running JVM',
  description:
    'Attach to a running Java, Kotlin or Scala app without restarting it: its HTTP(S) clients go through Akev and trust the Akev CA. Restore puts its original settings back.',
  category: 'runtime',
  tags: ['jvm', 'java', 'kotlin', 'scala', 'clojure', 'groovy', 'spring', 'quarkus', 'micronaut', 'gradle', 'maven', 'attach', 'agent', 'process', 'running'],
  icon: 'java',
  supported: true,
  available: true,
  active: false,
  version: '21.0.5',
  metadata: {
    processes: [
      jvm(48213, 'BillingServiceApplication', 'com.acme.billing.BillingServiceApplication', 'java -Xmx1g -cp build/classes:build/libs/* com.acme.billing.BillingServiceApplication --spring.profiles.active=dev', '21.0.5', '/usr/lib/jvm/java-21-openjdk'),
      jvm(51877, 'inventory-api.jar', '/srv/inventory/inventory-api.jar', 'java -jar /srv/inventory/inventory-api.jar --port 8081', '17.0.13', '/usr/lib/jvm/java-17-openjdk'),
      jvm(60210, 'MainKt', 'dev.acme.cli.MainKt', 'java -cp acme-cli.jar dev.acme.cli.MainKt sync --watch', '21.0.5', '/usr/lib/jvm/java-21-openjdk'),
    ],
    discovery: 'jps',
    jps: '/usr/lib/jvm/java-21-openjdk/bin/jps',
    jpsError: null,
    java: '/usr/lib/jvm/java-21-openjdk/bin/java',
    notes: NOTES,
    options: {
      pid: 'process id of the JVM (from processes[].pid)',
      interceptLoopback: 'also intercept requests to localhost (default false)',
      proxyProperties: 'auto (default) | always | never: set the http(s).proxyHost/Port system properties',
    },
  },
  needsConfig: true,
};

function processesOf(info: InterceptorInfo): MockJvm[] {
  const meta = info.metadata as { processes?: MockJvm[] } | undefined;
  return Array.isArray(meta?.processes) ? meta.processes : [];
}

function withProcesses(info: InterceptorInfo, processes: MockJvm[]): InterceptorInfo {
  return { ...info, active: processes.some((p) => p.active), metadata: { ...(info.metadata as object), processes } };
}

function pidOf(options: unknown): number | null {
  const raw = typeof options === 'object' && options !== null ? (options as { pid?: unknown }).pid : undefined;
  const pid = typeof raw === 'number' ? raw : typeof raw === 'string' ? Number(raw) : NaN;
  return Number.isInteger(pid) && pid > 0 ? pid : null;
}

/** Attach to `options.pid` (the Spring app reports the OkHttp limitation, like a real one would). */
export function mockJvmActivate(info: InterceptorInfo, options: unknown, port: number): { info: InterceptorInfo; result: ActivationResult } {
  const pid = pidOf(options);
  const target = processesOf(info).find((p) => p.pid === pid);
  if (pid == null || !target) {
    return { info, result: { success: false, message: pid == null ? 'Choose a JVM: options.pid is required (see metadata.processes)' : `No JVM with PID ${pid} to attach to: it may have exited, run as another user or in a container` } };
  }
  const loopback = typeof options === 'object' && options !== null && (options as { interceptLoopback?: unknown }).interceptLoopback === true;
  const processes = processesOf(info).map((p) => (p.pid === pid ? { ...p, attached: true, active: true, attachedPort: port, attachedAt: Date.now(), interceptLoopback: loopback } : p));
  const okhttp = target.mainClass.includes('billing');
  const warnings = okhttp
    ? ["Clients created before attaching that use the JDK's built-in proxy selector keep connecting directly (okhttp3.OkHttpClient is loaded); clients created from now on are intercepted."]
    : [];
  return {
    info: withProcesses(info, processes),
    result: {
      success: true,
      message: `Attached to ${target.name} (PID ${pid}): its HTTP(S) traffic now goes through Akev.${warnings[0] ? ` ${warnings[0]}` : ''}`,
      metadata: {
        pid,
        name: target.name,
        java: `${target.javaHome}/bin/java`,
        agent: {
          action: 'apply',
          proxy: `127.0.0.1:${port}`,
          interceptLoopback: String(loopback),
          proxySelector: 'installed',
          defaultSslContext: 'installed',
          httpsUrlConnection: 'installed',
          existingDefaultContext: 'patched',
          proxyProperties: okhttp ? 'skipped: okhttp3.OkHttpClient is loaded' : 'set',
          result: 'ok',
          agentVersion: '1.0.0',
          javaVersion: target.javaVersion,
          warnings,
        },
      },
    },
  };
}

/** Restore `options.pid`, or every attached JVM without one. */
export function mockJvmDeactivate(info: InterceptorInfo, options: unknown): InterceptorInfo {
  const pid = pidOf(options);
  const processes = processesOf(info).map((p) => (pid == null || p.pid === pid ? { ...p, attached: false, active: false, attachedPort: null, attachedAt: null, interceptLoopback: null } : p));
  return withProcesses(info, processes);
}

/** The catalogue with the JVM entry after Electron (the daemon's order). */
export function withMockJvm(list: InterceptorInfo[]): InterceptorInfo[] {
  if (list.some((i) => i.id === 'jvm')) return list;
  const at = list.findIndex((i) => i.id === 'electron');
  const out = [...list];
  out.splice(at < 0 ? out.length : at + 1, 0, structuredClone(MOCK_JVM_INTERCEPTOR));
  return out;
}
