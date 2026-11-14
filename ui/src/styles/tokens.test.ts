/**
 * Design-token guard rails:
 * - every theme defines the same token names (themes inherit the dark base, so overrides
 *   must only touch existing names);
 * - text-like tokens meet WCAG AA (4.5:1) on every surface, in every theme;
 * - white text on solid accent / danger buttons meets AA.
 */

import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

type Rgba = [number, number, number, number];

const css = readFileSync(join(import.meta.dir, 'tokens.css'), 'utf8');

function block(selector: string): Record<string, string> {
  const start = css.indexOf(selector);
  if (start < 0) throw new Error(`selector ${selector} not found`);
  const open = css.indexOf('{', start);
  const close = css.indexOf('}', open);
  const body = css.slice(open + 1, close).replace(/\/\*[\s\S]*?\*\//g, '');
  const out: Record<string, string> = {};
  for (const m of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) out[m[1] as string] = (m[2] as string).trim();
  return out;
}

const base = block(':root {');
const dark = block(":root,\n[data-theme='dark']");
const THEMES: Record<string, Record<string, string>> = {
  dark,
  midnight: { ...dark, ...block("[data-theme='midnight']") },
  contrast: { ...dark, ...block("[data-theme='contrast']") },
  light: { ...dark, ...block("[data-theme='light']") },
};

function resolve(tokens: Record<string, string>, value: string, depth = 0): string {
  const m = /^var\((--[\w-]+)\)$/.exec(value);
  if (!m || depth > 5) return value;
  const ref = tokens[m[1] as string];
  if (ref == null) throw new Error(`unresolved ${value}`);
  return resolve(tokens, ref, depth + 1);
}

function parseColor(value: string): Rgba {
  const hex = /^#([0-9a-f]{6})$/i.exec(value);
  if (hex) {
    const n = Number.parseInt(hex[1] as string, 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 1];
  }
  const rgba = /^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*(?:,\s*([\d.]+)\s*)?\)$/.exec(value);
  if (rgba) return [Number(rgba[1]), Number(rgba[2]), Number(rgba[3]), rgba[4] == null ? 1 : Number(rgba[4])];
  throw new Error(`cannot parse colour ${value}`);
}

function over(top: Rgba, bottom: Rgba): Rgba {
  const a = top[3];
  return [top[0] * a + bottom[0] * (1 - a), top[1] * a + bottom[1] * (1 - a), top[2] * a + bottom[2] * (1 - a), 1];
}

function luminance([r, g, b]: Rgba): number {
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

export function contrast(fg: Rgba, bg: Rgba): number {
  const a = luminance(fg);
  const b = luminance(bg);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

function color(theme: Record<string, string>, token: string): Rgba {
  const v = theme[token];
  if (v == null) throw new Error(`missing ${token}`);
  return parseColor(resolve(theme, v));
}

const SURFACES = ['--bg-app', '--bg-panel', '--bg-subtle', '--bg-elevated'];

/** Tokens rendered as (small) text somewhere in the UI. */
const TEXT_TOKENS = [
  '--text',
  '--text-muted',
  '--text-subtle',
  '--accent-text',
  '--success',
  '--warning',
  '--danger',
  '--info',
  ...['get', 'post', 'put', 'patch', 'delete', 'head', 'options', 'connect', 'other'].map((m) => `--m-${m}`),
  ...['1xx', '2xx', '3xx', '4xx', '5xx', 'err'].map((s) => `--s-${s}`),
  ...['json', 'html', 'js', 'css', 'image', 'media', 'font', 'xml', 'text', 'ws', 'grpc', 'form'].map((k) => `--k-${k}`),
  ...['key', 'string', 'number', 'bool', 'keyword', 'tag', 'fn', 'type', 'comment', 'punct'].map((s) => `--syn-${s}`),
];

describe('design tokens', () => {
  test('themes only override existing token names', () => {
    for (const name of ['midnight', 'contrast', 'light']) {
      const own = block(`[data-theme='${name}']`);
      const unknown = Object.keys(own).filter((k) => !(k in dark) && !(k in base));
      expect({ theme: name, unknown }).toEqual({ theme: name, unknown: [] });
    }
  });

  for (const [name, theme] of Object.entries(THEMES)) {
    test(`${name}: text tokens meet WCAG AA (4.5:1) on every surface`, () => {
      const failures: string[] = [];
      for (const surface of SURFACES) {
        const bg = color(theme, surface);
        for (const token of TEXT_TOKENS) {
          const fg = over(color(theme, token), bg);
          const ratio = contrast(fg, bg);
          if (ratio < 4.5) failures.push(`${token} on ${surface}: ${ratio.toFixed(2)}`);
        }
      }
      expect(failures).toEqual([]);
    });

    test(`${name}: primary text stays readable on hover/selected rows`, () => {
      const failures: string[] = [];
      for (const surface of ['--bg-panel', '--bg-elevated']) {
        for (const tint of ['--bg-hover', '--bg-active', '--bg-selected', '--bg-selected-strong', '--accent-soft']) {
          const bg = over(color(theme, tint), color(theme, surface));
          for (const token of ['--text', '--text-muted', '--accent-text']) {
            const ratio = contrast(over(color(theme, token), bg), bg);
            if (ratio < 4.5) failures.push(`${token} on ${tint}/${surface}: ${ratio.toFixed(2)}`);
          }
        }
      }
      expect(failures).toEqual([]);
    });

    test(`${name}: semantic text on its soft badge background meets AA`, () => {
      const failures: string[] = [];
      for (const kind of ['success', 'warning', 'danger', 'info']) {
        for (const surface of SURFACES) {
          const bg = over(color(theme, `--${kind}-soft`), color(theme, surface));
          const ratio = contrast(color(theme, `--${kind}`), bg);
          if (ratio < 4.5) failures.push(`--${kind} on --${kind}-soft/${surface}: ${ratio.toFixed(2)}`);
        }
      }
      expect(failures).toEqual([]);
    });

    test(`${name}: on-accent text on solid buttons meets AA`, () => {
      const white = color(theme, '--text-on-accent');
      for (const solid of ['--accent', '--accent-hover', '--accent-active', '--danger-solid']) {
        expect({ solid, ok: contrast(white, color(theme, solid)) >= 4.5 }).toEqual({ solid, ok: true });
      }
    });
  }

  test('contrast() sanity', () => {
    expect(contrast([0, 0, 0, 1], [255, 255, 255, 1])).toBeCloseTo(21, 0);
    expect(contrast([118, 118, 118, 1], [255, 255, 255, 1])).toBeGreaterThan(4.5);
  });
});
