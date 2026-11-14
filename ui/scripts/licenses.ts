#!/usr/bin/env bun
/**
 * Open-source licenses of everything the web UI ships: the runtime `dependencies` in
 * package.json and, recursively, theirs — read from `node_modules/<name>/package.json`.
 * Scoped families (`@radix-ui/*`, `@codemirror/*`…) are listed once with a package count.
 *
 * The Vite config serves the result as `virtual:licenses` (Settings → About), so the list is
 * computed at build time and can't drift from what is bundled. `bun run scripts/licenses.ts`
 * prints it.
 */

import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

export interface LicenseEntry {
  /** Package name, or `@scope/*` for a scoped family. */
  name: string;
  /** SPDX expression(s), e.g. `MIT` or `MIT, ISC` when a family mixes them. */
  license: string;
  /** Number of packages behind the entry. */
  packages: number;
}

interface Manifest {
  name: string;
  version?: string;
  license?: string | { type?: string };
  licenses?: { type?: string }[];
  dependencies?: Record<string, string>;
}

function readManifest(dir: string): Manifest | null {
  const file = join(dir, 'package.json');
  if (!existsSync(file)) return null;
  return JSON.parse(readFileSync(file, 'utf8')) as Manifest;
}

/** Resolve a dependency like Node does: `node_modules` next to the dependent, then upwards. */
function resolvePackage(name: string, fromDir: string, root: string): string | null {
  let dir = fromDir;
  for (;;) {
    const candidate = join(dir, 'node_modules', name);
    if (existsSync(join(candidate, 'package.json'))) return candidate;
    if (dir === root || dir === dirname(dir)) return null;
    dir = dirname(dir);
  }
}

function licenseOf(m: Manifest): string {
  if (typeof m.license === 'string') return m.license;
  if (m.license?.type) return m.license.type;
  const types = (m.licenses ?? []).map((l) => l.type).filter(Boolean);
  return types.length ? types.join(' OR ') : 'UNKNOWN';
}

/** Every runtime package bundled into the UI, grouped by scope. */
export function collectLicenses(root: string): LicenseEntry[] {
  const top = readManifest(root);
  if (!top) throw new Error(`No package.json in ${root}`);
  const seen = new Map<string, Manifest>();
  const queue: { name: string; from: string }[] = Object.keys(top.dependencies ?? {}).map((name) => ({ name, from: root }));
  while (queue.length) {
    const { name, from } = queue.shift() as { name: string; from: string };
    const dir = resolvePackage(name, from, root);
    if (!dir) continue; // optional / platform-specific dependency that isn't installed
    const key = dir;
    if (seen.has(key)) continue;
    const m = readManifest(dir);
    if (!m) continue;
    seen.set(key, m);
    for (const dep of Object.keys(m.dependencies ?? {})) queue.push({ name: dep, from: dir });
  }

  const groups = new Map<string, { licenses: Set<string>; names: Set<string> }>();
  for (const m of seen.values()) {
    const group = m.name.startsWith('@') ? `${m.name.split('/')[0]}/*` : m.name;
    const g = groups.get(group) ?? { licenses: new Set<string>(), names: new Set<string>() };
    g.licenses.add(licenseOf(m));
    g.names.add(m.name);
    groups.set(group, g);
  }
  return [...groups.entries()]
    .map(([name, g]) => ({ name, license: [...g.licenses].sort().join(', '), packages: g.names.size }))
    .sort((a, b) => a.name.replace(/^@/, '').localeCompare(b.name.replace(/^@/, '')));
}

if (import.meta.main) {
  const entries = collectLicenses(join(import.meta.dir, '..'));
  for (const e of entries) console.log(`${e.name.padEnd(36)} ${e.license.padEnd(14)} ${e.packages > 1 ? `${e.packages} packages` : ''}`);
  console.log(`\n${entries.reduce((n, e) => n + e.packages, 0)} packages`);
}
