/**
 * Whether exports replace secrets (credential headers and cookies, secret query parameters,
 * tokens in bodies) with `[redacted]` (`?redact=1`). HAR files are usually shared, so HAR
 * exports redact by default; `.akev` downloads keep everything unless asked.
 */

import { storageGet, storageSet } from './storage';

export type ExportFormat = 'har' | 'session';

const KEYS: Record<ExportFormat, string> = {
  har: 'akev.export.redactHar',
  session: 'akev.export.redactSession',
};

const DEFAULTS: Record<ExportFormat, boolean> = { har: true, session: false };

export function redactExports(format: ExportFormat): boolean {
  const stored = storageGet(KEYS[format]);
  return stored == null ? DEFAULTS[format] : stored === '1';
}

export function setRedactExports(format: ExportFormat, on: boolean): void {
  storageSet(KEYS[format], on ? '1' : '0');
}
