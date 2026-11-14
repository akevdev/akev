/**
 * `{{variable}}` substitution for Compose (environments + globals + dynamic variables).
 *
 * - Names are any run of non-brace, non-space characters, optionally padded: `{{ baseUrl }}`.
 * - Values may reference other variables (`baseUrl = {{scheme}}://{{host}}`), expanded up to
 *   {@link MAX_DEPTH} levels (cycles stop there and are reported as unresolved).
 * - Dynamic variables (`{{$uuid}}`, `{{$timestamp}}`, …) are generated per occurrence.
 */

import type { KeyValueRow } from '../../components/ui/KeyValueEditor';
import type { EnvState } from './model';

export const VAR_PATTERN = /\{\{\s*([^{}\s]+)\s*\}\}/g;
const MAX_DEPTH = 6;

export interface DynamicVar {
  name: string;
  description: string;
  generate: () => string;
}

export const DYNAMIC_VARS: readonly DynamicVar[] = [
  { name: '$uuid', description: 'Random UUID v4', generate: () => crypto.randomUUID() },
  { name: '$guid', description: 'Random UUID v4 (alias)', generate: () => crypto.randomUUID() },
  { name: '$timestamp', description: 'Unix time in seconds', generate: () => String(Math.floor(Date.now() / 1000)) },
  { name: '$timestampMs', description: 'Unix time in milliseconds', generate: () => String(Date.now()) },
  { name: '$isoTimestamp', description: 'Current time, ISO 8601', generate: () => new Date().toISOString() },
  { name: '$randomInt', description: 'Random integer 0–1000', generate: () => String(Math.floor(Math.random() * 1001)) },
];

const DYNAMIC = new Map(DYNAMIC_VARS.map((d) => [d.name, d]));

export function isDynamicVar(name: string): boolean {
  return DYNAMIC.has(name);
}

export type VarMap = ReadonlyMap<string, string>;

/** Variables from enabled rows (later rows win). */
export function rowsToVars(rows: readonly KeyValueRow[], into = new Map<string, string>()): Map<string, string> {
  for (const r of rows) {
    const name = r.name.trim();
    if (r.enabled && name) into.set(name, r.value);
  }
  return into;
}

/** Effective variables: globals, overridden by the active environment. */
export function resolveEnvVars(env: EnvState): Map<string, string> {
  const vars = rowsToVars(env.globals);
  const active = env.envs.find((e) => e.id === env.activeId);
  if (active) rowsToVars(active.vars, vars);
  return vars;
}

export interface SubstituteResult {
  text: string;
  /** Names that had no value (in order of first appearance). */
  unresolved: string[];
}

/**
 * Replace `{{name}}` occurrences. Unresolved variables are left verbatim and reported.
 * `generate` overrides dynamic variable generation (tests).
 */
export function substitute(text: string, vars: VarMap, generate?: (name: string) => string | undefined): SubstituteResult {
  if (!text.includes('{{')) return { text, unresolved: [] };
  const unresolved: string[] = [];
  const expand = (input: string, depth: number): string =>
    input.replace(VAR_PATTERN, (match, name: string) => {
      const dyn = generate?.(name) ?? DYNAMIC.get(name)?.generate();
      if (dyn !== undefined) return dyn;
      const value = vars.get(name);
      if (value === undefined || depth >= MAX_DEPTH) {
        if (!unresolved.includes(name)) unresolved.push(name);
        return match;
      }
      return value.includes('{{') ? expand(value, depth + 1) : value;
    });
  return { text: expand(text, 0), unresolved };
}

/** Names referenced in a text (deduplicated, in order). */
export function referencedVars(text: string): string[] {
  const out: string[] = [];
  if (!text.includes('{{')) return out;
  for (const m of text.matchAll(VAR_PATTERN)) {
    const name = m[1] as string;
    if (!out.includes(name)) out.push(name);
  }
  return out;
}

export type VarStatus = 'resolved' | 'unresolved' | 'dynamic';

export interface TextSegment {
  text: string;
  /** Set for `{{var}}` segments. */
  variable?: { name: string; status: VarStatus; value?: string };
}

/** Split a text into plain and `{{var}}` segments (for highlighting). */
export function tokenizeVars(text: string, vars: VarMap): TextSegment[] {
  const out: TextSegment[] = [];
  let last = 0;
  for (const m of text.matchAll(VAR_PATTERN)) {
    const start = m.index ?? 0;
    if (start > last) out.push({ text: text.slice(last, start) });
    const name = m[1] as string;
    const value = vars.get(name);
    const status: VarStatus = DYNAMIC.has(name) ? 'dynamic' : value !== undefined ? 'resolved' : 'unresolved';
    out.push({ text: m[0], variable: { name, status, value: status === 'resolved' ? substitute(value as string, vars).text : undefined } });
    last = start + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

/** Unresolved variable names across several texts. */
export function unresolvedIn(texts: readonly string[], vars: VarMap): string[] {
  const out: string[] = [];
  for (const t of texts) {
    for (const name of substitute(t, vars, (n) => (DYNAMIC.has(n) ? '' : undefined)).unresolved) if (!out.includes(name)) out.push(name);
  }
  return out;
}

// =====================================================================================
// Completion (typing `{{` in the URL bar)
// =====================================================================================

export interface VarCompletion {
  /** Index of the opening `{{`. */
  start: number;
  /** Partial name typed so far. */
  partial: string;
}

/** The variable being typed at `caret` (`…{{bas|`), or null. */
export function varCompletionAt(text: string, caret: number): VarCompletion | null {
  const m = /\{\{\s*([^{}\s]*)$/.exec(text.slice(0, caret));
  if (!m) return null;
  return { start: m.index, partial: m[1] ?? '' };
}

/** Insert a completed `{{name}}`, replacing the partial (and a trailing `rest}}` if present). */
export function applyVarCompletion(text: string, caret: number, c: VarCompletion, name: string): { text: string; caret: number } {
  const rest = text.slice(caret).replace(/^[^{}\s]*\s*\}\}/, '');
  const inserted = `{{${name}}}`;
  return { text: `${text.slice(0, c.start)}${inserted}${rest}`, caret: c.start + inserted.length };
}
