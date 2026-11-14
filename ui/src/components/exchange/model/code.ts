/** Pure helpers for CodeView: target grouping, snippet filenames and request fingerprints. No React. */

import type { CodegenTarget, EditableRequest, TargetInfo } from '../../../api/types';

export interface TargetGroup {
  label: string;
  targets: TargetInfo[];
}

/** Group targets by `group`, keeping the daemon's order of first appearance. */
export function groupTargets(targets: readonly TargetInfo[]): TargetGroup[] {
  const groups = new Map<string, TargetInfo[]>();
  for (const t of targets) {
    const g = t.group || 'Other';
    let list = groups.get(g);
    if (!list) groups.set(g, (list = []));
    list.push(t);
  }
  return [...groups].map(([label, list]) => ({ label, targets: list }));
}

/** The remembered target if the daemon offers it, else cURL, else the first target. */
export function pickTarget(targets: readonly TargetInfo[], remembered: string | null | undefined): TargetInfo | undefined {
  return targets.find((t) => t.id === remembered) ?? targets.find((t) => t.id === 'curl') ?? targets[0];
}

const EXTENSIONS: Readonly<Record<string, string>> = {
  shell: 'sh',
  bash: 'sh',
  powershell: 'ps1',
  javascript: 'js',
  typescript: 'ts',
  python: 'py',
  go: 'go',
  rust: 'rs',
  java: 'java',
  kotlin: 'kt',
  csharp: 'cs',
  php: 'php',
  ruby: 'rb',
  swift: 'swift',
  dart: 'dart',
  http: 'http',
};

/** Filename for saving a snippet: `request.py`, `request.sh`, `request.cmd` (cURL for Windows). */
export function snippetFilename(target: CodegenTarget, language: string): string {
  if (target === 'curl-windows') return 'request.cmd';
  return `request.${EXTENSIONS[language.toLowerCase()] ?? 'txt'}`;
}

/** Stable key of an editable request (for caching generated snippets). */
export function requestKey(r: EditableRequest): string {
  let h = 2166136261;
  const s = `${r.method}\u0000${r.url}\u0000${r.headers.map(([k, v]) => `${k}:${v}`).join('\u0001')}\u0000${r.body}`;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return `${s.length}:${(h >>> 0).toString(36)}`;
}
