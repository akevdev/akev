#!/usr/bin/env bun
/**
 * Bundle-size budget for the production build (`bun run build` first).
 *
 *   bun run size            # report, exit 1 when over budget
 *   bun run size -- --json  # machine-readable
 *
 * "Initial JS" is what the browser downloads before the first Traffic paint: the entry
 * script in `dist/index.html` plus everything it imports statically (Vite also lists those
 * as `modulepreload` links). Lazy chunks (pages, dialogs, CodeMirror, previews…) don't count.
 * Sizes are gzip level 9, like a typical server would send them.
 */

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { gzipSync } from 'node:zlib';

/**
 * Budgets in KB (gzip): the sizes measured when the budget was set (initial JS 218.4, initial
 * CSS 22.4, largest lazy chunk 87.7 — CodeMirror) plus ~10% headroom. Raise one only with a
 * reason in the commit message.
 */
const BUDGET = {
  initialJsKb: 238,
  initialCssKb: 25,
  /** Any single lazy chunk (catches a heavy dependency landing in one place). */
  largestChunkKb: 97,
};

const dist = join(import.meta.dir, '..', 'dist');
const assets = join(dist, 'assets');

function gzipKb(file: string): number {
  return gzipSync(readFileSync(file), { level: 9 }).length / 1024;
}

let html: string;
try {
  html = readFileSync(join(dist, 'index.html'), 'utf8');
} catch {
  console.error('dist/index.html not found — run `bun run build` first.');
  process.exit(2);
}

const entry = [...html.matchAll(/<script[^>]+type="module"[^>]+src="\.\/(assets\/[^"]+\.js)"/g)].map((m) => m[1] as string);
const preloads = [...html.matchAll(/<link[^>]+rel="modulepreload"[^>]+href="\.\/(assets\/[^"]+\.js)"/g)].map((m) => m[1] as string);
const css = [...html.matchAll(/<link[^>]+rel="stylesheet"[^>]+href="\.\/(assets\/[^"]+\.css)"/g)].map((m) => m[1] as string);

// Follow static imports from the entry too, in case a chunk is imported but not preloaded.
const initial = new Set<string>([...entry, ...preloads]);
const queue = [...initial];
while (queue.length) {
  const rel = queue.pop() as string;
  const src = readFileSync(join(dist, rel), 'utf8');
  for (const m of src.matchAll(/(?:^|[;\s}])import\s*(?:[\w${}\s,*]+from\s*)?["']\.\/([^"']+\.js)["']/g)) {
    const dep = `assets/${m[1]}`;
    if (!initial.has(dep)) {
      initial.add(dep);
      queue.push(dep);
    }
  }
}

const rows = [...initial].map((rel) => ({ file: rel, kb: gzipKb(join(dist, rel)) })).sort((a, b) => b.kb - a.kb);
const initialJs = rows.reduce((n, r) => n + r.kb, 0);
const initialCss = css.reduce((n, rel) => n + gzipKb(join(dist, rel)), 0);
const lazy = readdirSync(assets)
  .filter((f) => f.endsWith('.js') && !initial.has(`assets/${f}`))
  .map((f) => ({ file: `assets/${f}`, kb: gzipKb(join(assets, f)), raw: statSync(join(assets, f)).size }))
  .sort((a, b) => b.kb - a.kb);
const largest = lazy[0];

const checks = [
  { name: 'Initial JS (gzip)', kb: initialJs, budget: BUDGET.initialJsKb },
  { name: 'Initial CSS (gzip)', kb: initialCss, budget: BUDGET.initialCssKb },
  { name: `Largest lazy chunk (${largest?.file ?? '—'})`, kb: largest?.kb ?? 0, budget: BUDGET.largestChunkKb },
];

if (process.argv.includes('--json')) {
  console.log(JSON.stringify({ initial: rows, initialJsKb: initialJs, initialCssKb: initialCss, lazy, checks }, null, 2));
} else {
  console.log('Initial JS:');
  for (const r of rows) console.log(`  ${r.kb.toFixed(1).padStart(7)} KB  ${r.file}`);
  console.log('\nLargest lazy chunks:');
  for (const r of lazy.slice(0, 8)) console.log(`  ${r.kb.toFixed(1).padStart(7)} KB  ${r.file}`);
  console.log('');
  for (const c of checks) console.log(`${c.kb <= c.budget ? 'ok  ' : 'OVER'}  ${c.name}: ${c.kb.toFixed(1)} KB (budget ${c.budget} KB)`);
}

if (checks.some((c) => c.kb > c.budget)) {
  console.error('\nOver the bundle budget. Lazy-load what the first Traffic paint does not need, or raise the budget in scripts/size.ts with a reason.');
  process.exit(1);
}
