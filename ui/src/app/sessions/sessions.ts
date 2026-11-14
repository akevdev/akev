/**
 * Saved sessions (`/api/sessions`): the Sessions dialog's open state, the pure helpers it
 * and the startup "restored" toast use, and the save-scope resolution. No React here except
 * the tiny `useSessionsDialog` hook.
 */

import { useSyncExternalStore } from 'react';
import type { ExchangeId, RestoredSession, SaveSessionRequest, SessionInfo } from '../../api/types';
import { formatBytes, formatNumber, formatRelativeTime, pluralize } from '../../lib/format';

// =====================================================================================
// Dialog state
// =====================================================================================

export interface SessionsDialogState {
  open: boolean;
  /** Focus the "Save current traffic" form when opening. */
  focusSave: boolean;
}

let state: SessionsDialogState = { open: false, focusSave: false };
const listeners = new Set<() => void>();

function set(next: SessionsDialogState): void {
  state = next;
  for (const l of listeners) l();
}

/** Open the Sessions dialog (`save: true` focuses the save form). */
export function openSessionsDialog(options: { save?: boolean } = {}): void {
  set({ open: true, focusSave: options.save ?? false });
}

export function closeSessionsDialog(): void {
  set({ open: false, focusSave: false });
}

export function getSessionsDialogState(): SessionsDialogState {
  return state;
}

export function useSessionsDialog(): SessionsDialogState {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => state,
    () => state,
  );
}

// =====================================================================================
// Settings
// =====================================================================================

/** "every 60 s" / "every 5 min" for the autosave interval. */
export function formatInterval(secs: number): string {
  if (secs < 60 || secs % 60 !== 0) return `${formatNumber(secs)} s`;
  const min = secs / 60;
  if (min < 60 || min % 60 !== 0) return `${formatNumber(min)} min`;
  return pluralize(min / 60, 'hour');
}

// =====================================================================================
// Names & descriptions
// =====================================================================================

export const MAX_SESSION_NAME = 200;

/** Why a session name can't be used (`null` when it's fine). */
export function validateSessionName(name: string): string | null {
  const trimmed = name.trim();
  if (!trimmed) return 'Enter a name';
  if ([...trimmed].length > MAX_SESSION_NAME) return `Use at most ${MAX_SESSION_NAME} characters`;
  if (/[\u0000-\u001f\u007f]/.test(trimmed)) return "Line breaks and control characters aren't allowed";
  return null;
}

/** Suggested name for saving now: `Traffic 2026-11-09 15:30`. */
export function defaultSessionName(now: Date = new Date()): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `Traffic ${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())} ${p(now.getHours())}:${p(now.getMinutes())}`;
}

/** `1,234 exchanges · 4.8 MB · 2 min ago`. */
export function describeSession(s: Pick<SessionInfo, 'exchanges' | 'size' | 'updatedAt'>, now: number = Date.now()): string {
  return `${pluralize(s.exchanges, 'exchange')} · ${formatBytes(s.size)} · ${formatRelativeTime(s.updatedAt, now)}`;
}

/** Autosaves first (current run, then previous run), then saved sessions newest first. */
export function sortSessions(list: readonly SessionInfo[]): SessionInfo[] {
  const rank = (s: SessionInfo) => (s.kind !== 'autosave' ? 2 : s.id === 'autosave' ? 0 : 1);
  return [...list].sort((a, b) => rank(a) - rank(b) || b.updatedAt - a.updatedAt || a.name.localeCompare(b.name));
}

// =====================================================================================
// Saving
// =====================================================================================

export type SaveScope = 'all' | 'visible' | 'selected';

/** What the traffic list looks like when saving. */
export interface SaveContext {
  total: number;
  visible: readonly ExchangeId[];
  selected: readonly ExchangeId[];
  /** The composed filter query when one is active and valid. */
  filter: string | null;
}

/** Exchange count each scope would save. */
export function scopeCount(scope: SaveScope, ctx: SaveContext): number {
  if (scope === 'selected') return ctx.selected.length;
  if (scope === 'visible') return ctx.visible.length;
  return ctx.total;
}

/** The `POST /api/sessions` body for a scope (a filter stays short for any number of rows). */
export function saveRequest(name: string, scope: SaveScope, ctx: SaveContext): SaveSessionRequest {
  const base = { name: name.trim() };
  if (scope === 'selected') return { ...base, ids: [...ctx.selected] };
  if (scope === 'visible') {
    if (ctx.filter) return { ...base, filter: ctx.filter };
    if (ctx.visible.length < ctx.total) return { ...base, ids: [...ctx.visible] };
  }
  return base;
}

// =====================================================================================
// Startup restore
// =====================================================================================

/** sessionStorage key remembering the daemon start whose restore was announced. */
export const RESTORE_ANNOUNCED_KEY = 'akev.restoreAnnounced';

/** `Restored autosaved session (1,234 exchanges)`. */
export function restoredTitle(r: Pick<RestoredSession, 'exchanges'>): string {
  return `Restored autosaved session (${pluralize(r.exchanges, 'exchange')})`;
}

/**
 * Whether to announce a restore: once per daemon run (`startedAt`), whether it's seen in
 * `hello` or arrives as `session-restored`, across reconnects and reloads of this tab.
 */
export function shouldAnnounceRestore(startedAt: number, announced: string | null): boolean {
  return announced !== String(startedAt);
}

// =====================================================================================
// Imports from other tools
// =====================================================================================

export type ForeignImport = 'charles' | 'mitmproxy' | 'fiddler';

/** File picker filters: Charles `.chlsj`, Fiddler `.saz`; mitmproxy flow files have no fixed extension. */
export function importAccept(kind: ForeignImport): string[] | undefined {
  if (kind === 'charles') return ['.chlsj', '.json', 'application/json'];
  if (kind === 'fiddler') return ['.saz', 'application/zip'];
  return undefined;
}

export const IMPORT_LABELS: Record<ForeignImport, string> = {
  charles: 'Charles session',
  mitmproxy: 'mitmproxy flows',
  fiddler: 'Fiddler sessions',
};

// =====================================================================================
// Any capture file (drag & drop, "Open capture file…", the desktop menu)
// =====================================================================================

export type CaptureKind = 'har' | 'session' | ForeignImport;

export const CAPTURE_LABELS: Record<CaptureKind, string> = { har: 'HAR file', session: 'Akev session', ...IMPORT_LABELS };

const MITMPROXY_EXTENSIONS = new Set(['mitm', 'flow', 'flows', 'dump', 'mitmdump']);
const ZSTD_MAGIC = [0x28, 0xb5, 0x2f, 0xfd];

function extensionOf(name: string): string {
  const base = name.slice(name.lastIndexOf('/') + 1);
  const dot = base.lastIndexOf('.');
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : '';
}

function startsWith(data: Uint8Array, bytes: readonly number[]): boolean {
  return bytes.every((b, i) => data[i] === b);
}

/** Guess from the first bytes: HAR / Charles JSON, a zstd `.akev`, a Fiddler zip, mitmproxy tnetstrings. */
function sniff(head: Uint8Array): CaptureKind | null {
  if (startsWith(head, ZSTD_MAGIC)) return 'session';
  if (startsWith(head, [0x50, 0x4b, 0x03, 0x04])) return 'fiddler';
  const text = new TextDecoder().decode(head.subarray(0, 512)).replace(/^﻿/, '').trimStart();
  if (text.startsWith('[')) return 'charles';
  if (text.startsWith('{')) return /"log"\s*:/.test(text) ? 'har' : 'session';
  if (/^\d+:/.test(text)) return 'mitmproxy';
  return null;
}

/**
 * What a dropped or opened file is, from its extension (like the desktop app's file
 * associations) or, for `.json` and files without one, its first bytes. `null`: not a capture
 * file Akev reads.
 */
export function captureKind(name: string, head: Uint8Array): CaptureKind | null {
  const ext = extensionOf(name);
  if (ext === 'har') return 'har';
  if (ext === 'akev') return 'session';
  if (ext === 'chlsj') return 'charles';
  if (ext === 'saz') return 'fiddler';
  if (MITMPROXY_EXTENSIONS.has(ext)) return 'mitmproxy';
  if (ext === '' || ext === 'json') return sniff(head);
  return null;
}

