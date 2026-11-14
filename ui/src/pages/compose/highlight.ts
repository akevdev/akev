/**
 * Syntax spans for the URL bar: scheme / host / path / query names, values and separators /
 * fragment, plus `{{variables}}` coloured by resolution status. The URL bar renders these
 * behind a transparent input, so spans must reproduce the text exactly (no extra characters).
 */

import { isDynamicVar, VAR_PATTERN, type VarMap } from './variables';

export type UrlSpanKind =
  | 'scheme'
  | 'host'
  | 'path'
  | 'punct'
  | 'qname'
  | 'qvalue'
  | 'hash'
  | 'var-resolved'
  | 'var-unresolved'
  | 'var-dynamic';

export interface UrlSpan {
  text: string;
  kind: UrlSpanKind;
  /** Variable name for `var-*` spans. */
  name?: string;
}

type State = 'scheme' | 'host' | 'path' | 'qname' | 'qvalue' | 'hash';

/** Split a URL into highlight spans. Concatenating `text` gives back the input. */
export function urlSpans(url: string, vars: VarMap): UrlSpan[] {
  const spans: UrlSpan[] = [];
  const push = (text: string, kind: UrlSpanKind, name?: string) => {
    if (!text) return;
    const last = spans[spans.length - 1];
    if (last && last.kind === kind && !name && !last.name) last.text += text;
    else spans.push(name ? { text, kind, name } : { text, kind });
  };
  const schemeMatch = /^\s*[a-z][a-z0-9+.-]*:\/\//i.exec(url);
  let state: State = schemeMatch ? 'scheme' : 'host';
  const schemeEnd = schemeMatch ? schemeMatch[0].length : 0;

  // Variable ranges.
  const varAt = new Map<number, { end: number; name: string }>();
  for (const m of url.matchAll(VAR_PATTERN)) varAt.set(m.index ?? 0, { end: (m.index ?? 0) + m[0].length, name: m[1] as string });

  for (let i = 0; i < url.length; ) {
    const v = varAt.get(i);
    if (v) {
      const kind: UrlSpanKind = isDynamicVar(v.name) ? 'var-dynamic' : vars.has(v.name) ? 'var-resolved' : 'var-unresolved';
      push(url.slice(i, v.end), kind, v.name);
      if (state === 'scheme') state = 'host';
      i = v.end;
      continue;
    }
    const ch = url[i] as string;
    if (state === 'scheme') {
      push(ch, 'scheme');
      i++;
      if (i >= schemeEnd) state = 'host';
      continue;
    }
    if (state !== 'hash' && ch === '#') {
      state = 'hash';
      push(ch, 'hash');
      i++;
      continue;
    }
    switch (state) {
      case 'host':
        if (ch === '/') {
          state = 'path';
          push(ch, 'path');
        } else if (ch === '?') {
          state = 'qname';
          push(ch, 'punct');
        } else push(ch, 'host');
        break;
      case 'path':
        if (ch === '?') {
          state = 'qname';
          push(ch, 'punct');
        } else push(ch, 'path');
        break;
      case 'qname':
        if (ch === '=') {
          state = 'qvalue';
          push(ch, 'punct');
        } else if (ch === '&') push(ch, 'punct');
        else push(ch, 'qname');
        break;
      case 'qvalue':
        if (ch === '&') {
          state = 'qname';
          push(ch, 'punct');
        } else push(ch, 'qvalue');
        break;
      case 'hash':
        push(ch, 'hash');
        break;
    }
    i++;
  }
  return spans;
}
