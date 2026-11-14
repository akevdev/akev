/**
 * Lazily loaded CodeMirror language support. Each language is a dynamic import so only the
 * grammars actually viewed are downloaded (DESIGN.md §9 performance budget).
 */

import type { StreamParser } from '@codemirror/language';
import type { Extension } from '@codemirror/state';

/** Languages the CodeEditor understands (`text` = no highlighting). */
export type CodeLanguage =
  | 'text'
  | 'json'
  | 'javascript'
  | 'typescript'
  | 'html'
  | 'xml'
  | 'css'
  | 'markdown'
  | 'yaml'
  | 'shell'
  | 'http'
  | 'python'
  | 'go'
  | 'rust'
  | 'java'
  | 'kotlin'
  | 'csharp'
  | 'dart'
  | 'ruby'
  | 'swift'
  | 'powershell'
  | 'protobuf';

/** Languages whose syntax tree supports folding (others fold by indentation only). */
export const FOLDABLE: ReadonlySet<CodeLanguage> = new Set(['json', 'javascript', 'typescript', 'html', 'xml', 'css', 'markdown', 'yaml', 'rust', 'go', 'java', 'kotlin', 'csharp', 'dart', 'swift']);

async function legacy(load: () => Promise<StreamParser<unknown>>): Promise<Extension> {
  const [{ StreamLanguage }, parser] = await Promise.all([import('@codemirror/language'), load()]);
  return StreamLanguage.define(parser);
}

const LOADERS: Record<Exclude<CodeLanguage, 'text'>, () => Promise<Extension>> = {
  json: async () => (await import('@codemirror/lang-json')).json(),
  javascript: async () => (await import('@codemirror/lang-javascript')).javascript({ jsx: true }),
  typescript: async () => (await import('@codemirror/lang-javascript')).javascript({ typescript: true, jsx: true }),
  html: async () => (await import('@codemirror/lang-html')).html({ autoCloseTags: false }),
  xml: async () => (await import('@codemirror/lang-xml')).xml(),
  css: async () => (await import('@codemirror/lang-css')).css(),
  markdown: async () => (await import('@codemirror/lang-markdown')).markdown(),
  yaml: async () => (await import('@codemirror/lang-yaml')).yaml(),
  http: async () => (await import('./http')).http,
  shell: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/shell')).shell),
  python: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/python')).python),
  go: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/go')).go),
  rust: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/rust')).rust),
  java: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/clike')).java),
  kotlin: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/clike')).kotlin),
  csharp: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/clike')).csharp),
  dart: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/clike')).dart),
  ruby: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/ruby')).ruby),
  swift: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/swift')).swift),
  powershell: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/powershell')).powerShell),
  protobuf: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/protobuf')).protobuf),
};

const cache = new Map<CodeLanguage, Promise<Extension>>();

/** Load (once) the extension for a language; `text` resolves to no extension. */
export function loadLanguage(lang: CodeLanguage): Promise<Extension> {
  if (lang === 'text') return Promise.resolve([]);
  let p = cache.get(lang);
  if (!p) {
    const loader = LOADERS[lang];
    p = loader().catch((e: unknown) => {
      cache.delete(lang);
      console.warn(`[akev] failed to load ${lang} highlighting`, e);
      return [] as Extension;
    });
    cache.set(lang, p);
  }
  return p;
}

/**
 * Normalise loose language names (codegen `language` ids, content kinds, file extensions) to
 * a {@link CodeLanguage}.
 */
export function normalizeLanguage(name: string | null | undefined): CodeLanguage {
  const n = (name ?? '').toLowerCase();
  switch (n) {
    case 'json':
    case 'json-lines':
    case 'jsonl':
    case 'graph-ql':
    case 'graphql':
      return 'json';
    case 'js':
    case 'javascript':
    case 'node':
      return 'javascript';
    case 'ts':
    case 'typescript':
      return 'typescript';
    case 'html':
    case 'htm':
      return 'html';
    case 'xml':
    case 'svg':
      return 'xml';
    case 'css':
      return 'css';
    case 'md':
    case 'markdown':
      return 'markdown';
    case 'yaml':
    case 'yml':
      return 'yaml';
    case 'sh':
    case 'bash':
    case 'shell':
    case 'zsh':
    case 'curl':
      return 'shell';
    case 'http':
    case 'raw-http':
      return 'http';
    case 'py':
    case 'python':
      return 'python';
    case 'go':
      return 'go';
    case 'rs':
    case 'rust':
      return 'rust';
    case 'java':
      return 'java';
    case 'kt':
    case 'kotlin':
      return 'kotlin';
    case 'cs':
    case 'csharp':
      return 'csharp';
    case 'dart':
      return 'dart';
    case 'rb':
    case 'ruby':
      return 'ruby';
    case 'swift':
      return 'swift';
    case 'ps1':
    case 'powershell':
      return 'powershell';
    case 'proto':
    case 'protobuf':
      return 'protobuf';
    default:
      return 'text';
  }
}
