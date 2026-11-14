import { describe, expect, test } from 'bun:test';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

/**
 * CSS Modules rename keyframe names referenced from module files, so the global keyframes in
 * styles/global.css must be referenced through their `--anim-*` variables. A direct reference
 * silently disables the animation — and Radix Presence then never unmounts closed overlays.
 */
const GLOBAL_KEYFRAMES = [...readFileSync(join(import.meta.dir, 'global.css'), 'utf8').matchAll(/@keyframes\s+([\w-]+)/g)].map((m) => m[1]);

function moduleFiles(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) moduleFiles(p, out);
    else if (name.endsWith('.module.css')) out.push(p);
  }
  return out;
}

describe('global keyframes', () => {
  test('are never referenced by name from CSS modules', () => {
    const offenders: string[] = [];
    for (const file of moduleFiles(join(import.meta.dir, '..'))) {
      const css = readFileSync(file, 'utf8');
      const local = new Set([...css.matchAll(/@keyframes\s+([\w-]+)/g)].map((m) => m[1]));
      for (const decl of css.matchAll(/animation(?:-name)?\s*:([^;]+);/g)) {
        for (const name of GLOBAL_KEYFRAMES) {
          if (!local.has(name) && new RegExp(`(^|[\\s,])${name}(?![\\w-])`).test(decl[1] ?? '')) offenders.push(`${file}: ${decl[0].trim()}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});
