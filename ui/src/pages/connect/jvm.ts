/** Pure helpers for the JVM attach panel (`panels/JvmPanel.tsx`). */

import type { JvmProcess } from './metadata';

/** `com.example.app.Main --flag` → `Main` (first token, last path/package segment); jars keep their file name. */
export function shortProcessName(name: string): string {
  const first = name.trim().split(/\s+/)[0] ?? name;
  const seg = first.split(/[\\/]/).pop() ?? first;
  return seg.includes('.') && !/\.(jar|war|exe)$/i.test(seg) ? (seg.split('.').pop() ?? seg) : seg;
}

/** `PID 4242 · Java 21.0.5 · org.acme.Main` (the main class only when it adds something). */
export function jvmSubtitle(p: Pick<JvmProcess, 'pid' | 'javaVersion' | 'mainClass' | 'name'>): string {
  const parts = [`PID ${p.pid}`];
  if (p.javaVersion) parts.push(`Java ${p.javaVersion}`);
  const main = p.mainClass ?? p.name;
  if (main && main !== shortProcessName(main)) parts.push(main);
  return parts.join(' · ');
}

/** Warnings the agent reported when attaching (`ActivationResult.metadata.agent.warnings`). */
export function agentWarnings(metadata: unknown): string[] {
  if (typeof metadata !== 'object' || metadata === null) return [];
  const agent = (metadata as { agent?: unknown }).agent;
  if (typeof agent !== 'object' || agent === null) return [];
  const w = (agent as { warnings?: unknown }).warnings;
  return Array.isArray(w) ? w.filter((x): x is string => typeof x === 'string' && x.trim() !== '') : [];
}
