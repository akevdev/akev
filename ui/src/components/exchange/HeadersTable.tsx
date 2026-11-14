// biome-ignore-all lint/a11y/useFocusableInteractive: row-focus grid: the grid moves focus between rows (roving tabindex); cells take none
import clsx from 'clsx';
import { AlertTriangle, BookOpen, ChevronDown, ClipboardCopy, Columns3, Copy, ExternalLink, Eye, EyeOff, Filter, KeyRound } from 'lucide-react';
import { memo, useCallback, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import type { Headers, HttpVersion, Side } from '../../api/types';
import { useShortcutDocs } from '../../app/shortcuts';
import { formatBytes, formatDateTime, formatNumber, formatRelativeTime, pluralize } from '../../lib/format';
import { lookupHeaderDoc, type HeaderDoc } from '../../lib/headerDocs';
import { copyText, openExternal } from '../../lib/platform';
import { usePagePref } from '../../state/ui';
import { Badge, CopyButton, IconButton, Popover, PopoverContent, PopoverTrigger, SegmentedControl, Tooltip, toast } from '../ui';
import { kindFromContentType } from './model/body';
import { cookieExpiry, cookieSize, cookieWarnings, decodedCookieValue } from './model/cookies';
import { classifyHeader, decodeBasicAuth, headersToText, JWT_CLAIMS, LONG_HEADER_VALUE, orderHeaders, parseHeaderDate, summarizeJwt, type HeaderEntry } from './model/headers';
import { parseCookieHeader, parseSetCookie } from '../../lib/http';
import { relTime } from './model/time';
import { KindIcon } from './parts/Parts';
import styles from './HeadersTable.module.css';

export interface HeadersTableProps {
  headers: Headers;
  httpVersion?: HttpVersion;
  side?: Side;
  /** "Filter by this header" action (name, value). */
  onFilter?: (name: string, value: string) => void;
  /** "Show as a list column" action (header name). */
  onShowColumn?: (name: string) => void;
  emptyText?: string;
  className?: string;
  /** Show the order toggle / count / copy-all bar (default true). */
  toolbar?: boolean;
}

const SORT_PREF = 'exchange.headers.sorted';

async function copy(text: string, what: string) {
  if (await copyText(text)) toast.success(`${what} copied`);
  else toast.error('Copy failed');
}

// =====================================================================================
// Special value renderers
// =====================================================================================

function DateValue({ value }: { value: string }) {
  const t = parseHeaderDate(value);
  const rel = formatRelativeTime(t);
  return (
    <Tooltip content={`${formatDateTime(t)} local · ${rel}`}>
      <span className={styles.text}>
        <span className={styles.dated}>{value}</span>
        <span className={styles.relative}>{rel}</span>
      </span>
    </Tooltip>
  );
}

function CookiePanel({ value }: { value: string }) {
  const cookies = parseCookieHeader(value);
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: only stops the click from reaching the row behind it; the controls inside are buttons
    <div className={styles.panel} onClick={(e) => e.stopPropagation()}>
      <div className={styles.panelTitle}>{pluralize(cookies.length, 'cookie')}</div>
      <table className={styles.miniTable}>
        <tbody>
          {cookies.map((c, i) => {
            const decoded = decodedCookieValue(c.value);
            return (
              <tr key={i}>
                <td>{c.name || '(no name)'}</td>
                <td>
                  {c.value}
                  {decoded && <span className={styles.hint}>= {decoded}</span>}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function SetCookieSummary({ value, open, onToggle }: { value: string; open: boolean; onToggle: () => void }) {
  const c = parseSetCookie(value);
  const exp = cookieExpiry(c);
  const warnings = cookieWarnings(c);
  return (
    <div className={styles.pills}>
      <button type="button" className={clsx(styles.pill, styles.accent)} onClick={onToggle} aria-expanded={open}>
        {c.name || 'cookie'}
        <ChevronDown size={10} strokeWidth={2.5} className={clsx(styles.chev, open && styles.chevOpen)} aria-hidden />
      </button>
      {c.httpOnly && <span className={styles.pill}>HttpOnly</span>}
      {c.secure && <span className={styles.pill}>Secure</span>}
      {c.sameSite && <span className={styles.pill}>SameSite={c.sameSite}</span>}
      {c.partitioned && <span className={styles.pill}>Partitioned</span>}
      <span className={clsx(styles.pill, exp.kind === 'expired' && styles.danger)}>
        {exp.kind === 'session' ? 'Session' : exp.kind === 'expired' ? (exp.source === 'max-age' && (c.maxAge ?? 0) <= 0 ? 'Deletes cookie' : `Expired ${relTime(exp.at)}`) : exp.kind === 'invalid' ? 'Invalid expiry' : `Expires ${relTime(exp.at)}`}
      </span>
      {warnings.length > 0 && (
        <Tooltip content={warnings.join(' ')} multiline>
          <span className={clsx(styles.pill, styles.warning)}>
            <AlertTriangle size={10} strokeWidth={2.5} aria-hidden />
            {warnings.length === 1 ? 'Issue' : `${warnings.length} issues`}
          </span>
        </Tooltip>
      )}
    </div>
  );
}

function SetCookiePanel({ value }: { value: string }) {
  const c = parseSetCookie(value);
  const exp = cookieExpiry(c);
  const warnings = cookieWarnings(c);
  const decoded = decodedCookieValue(c.value);
  const expires: ReactNode =
    exp.kind === 'session' ? (
      <span className={styles.hint}>end of session</span>
    ) : exp.kind === 'invalid' ? (
      exp.raw
    ) : (
      <>
        {formatDateTime(exp.at)}
        <span className={styles.hint}>
          {exp.kind === 'expired' ? 'expired ' : ''}
          {relTime(exp.at)} · from {exp.source === 'max-age' ? `Max-Age=${c.maxAge}` : 'Expires'}
        </span>
      </>
    );
  const rows: [string, ReactNode][] = [
    ['Name', c.name || '(none)'],
    [
      'Value',
      <>
        {c.value || '(empty)'}
        {decoded && <span className={styles.hint}>= {decoded}</span>}
      </>,
    ],
    ['Domain', c.domain ?? <span className={styles.hint}>host only</span>],
    ['Path', c.path ?? <span className={styles.hint}>request directory</span>],
    ['Expires', expires],
    ['Size', formatBytes(cookieSize(c.name, c.value))],
    ...c.other.map(([k, v]): [string, ReactNode] => [k, v || <span className={styles.hint}>(flag)</span>]),
  ];
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: only stops the click from reaching the row behind it; the controls inside are buttons
    <div className={styles.panel} onClick={(e) => e.stopPropagation()}>
      <table className={styles.miniTable}>
        <tbody>
          {rows.map(([k, v]) => (
            <tr key={k}>
              <td>{k}</td>
              <td>{v}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {warnings.map((w) => (
        <div key={w} className={styles.warnLine}>
          <AlertTriangle size={12} strokeWidth={2.25} aria-hidden />
          {w}
        </div>
      ))}
    </div>
  );
}

const JWT_STATUS: Record<string, { label: (exp?: number, nbf?: number) => string; tone: 'success' | 'danger' | 'warning' | 'neutral' }> = {
  valid: { label: (exp) => `Valid · expires ${relTime(exp ?? Date.now())}`, tone: 'success' },
  expired: { label: (exp) => `Expired ${relTime(exp ?? Date.now())}`, tone: 'danger' },
  'not-yet-valid': { label: (_e, nbf) => `Not valid yet · ${relTime(nbf ?? Date.now())}`, tone: 'warning' },
  'no-expiry': { label: () => 'No expiry', tone: 'neutral' },
};

function claimValue(key: string, v: unknown): ReactNode {
  if ((key === 'exp' || key === 'iat' || key === 'nbf') && typeof v === 'number') {
    return (
      <>
        {formatDateTime(v * 1000)}
        <span className={styles.hint}>{relTime(v * 1000)}</span>
      </>
    );
  }
  return typeof v === 'string' ? v : JSON.stringify(v);
}

function JwtSummary({ value, open, onToggle }: { value: string; open: boolean; onToggle: () => void }) {
  const s = summarizeJwt(value);
  if (!s) return null;
  const st = JWT_STATUS[s.status] ?? JWT_STATUS['no-expiry'];
  return (
    <div className={styles.pills}>
      <button type="button" className={clsx(styles.pill, styles.accent)} onClick={onToggle} aria-expanded={open} aria-label={open ? 'Hide decoded JWT' : 'Show decoded JWT'}>
        <KeyRound size={10} strokeWidth={2.5} aria-hidden />
        JWT{s.algorithm ? ` · ${s.algorithm}` : ''}
        <ChevronDown size={10} strokeWidth={2.5} className={clsx(styles.chev, open && styles.chevOpen)} aria-hidden />
      </button>
      <span className={clsx(styles.pill, st?.tone !== 'neutral' && styles[st?.tone ?? 'success'])}>{st?.label(s.jwt.expiresAt, s.notBefore)}</span>
      {typeof s.jwt.payload.sub === 'string' && <span className={styles.pill}>sub {s.jwt.payload.sub}</span>}
    </div>
  );
}

function JwtPanel({ value }: { value: string }) {
  const s = summarizeJwt(value);
  if (!s) return null;
  const claims = JWT_CLAIMS.filter(([k]) => s.jwt.payload[k] !== undefined);
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: only stops the click from reaching the row behind it; the controls inside are buttons
    <div className={styles.panel} onClick={(e) => e.stopPropagation()}>
      {claims.length > 0 && (
        <>
          <div className={styles.panelTitle}>Claims</div>
          <table className={styles.miniTable}>
            <tbody>
              {claims.map(([k, label]) => (
                <tr key={k}>
                  <td>
                    {label} <span className={styles.hint}>{k}</span>
                  </td>
                  <td>{claimValue(k, s.jwt.payload[k])}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      <div className={styles.panelTitle}>
        Header
        <span className={styles.spacer} />
        <CopyButton value={JSON.stringify(s.jwt.header, null, 2)} label="Copy JWT header" />
      </div>
      <pre className={styles.jsonBlock}>{JSON.stringify(s.jwt.header, null, 2)}</pre>
      <div className={styles.panelTitle}>
        Payload
        <span className={styles.spacer} />
        <CopyButton value={JSON.stringify(s.jwt.payload, null, 2)} label="Copy JWT payload" />
      </div>
      <pre className={styles.jsonBlock}>{JSON.stringify(s.jwt.payload, null, 2)}</pre>
      <div className={styles.panelNote}>Decoded locally — the signature is not verified.</div>
    </div>
  );
}

function BasicSummary({ value }: { value: string }) {
  const creds = decodeBasicAuth(value);
  const [reveal, setReveal] = useState(false);
  if (!creds) return null;
  return (
    <div className={styles.pills}>
      <span className={clsx(styles.pill, styles.accent)}>
        <KeyRound size={10} strokeWidth={2.5} aria-hidden />
        Basic
      </span>
      <span className={styles.pill}>
        user <span className={styles.pillValue}>{creds.user || '(empty)'}</span>
      </span>
      <span className={styles.pill}>
        password{' '}
        <span className={clsx(styles.pillValue, !reveal && styles.secret)}>
          {reveal ? creds.password || '(empty)' : '•'.repeat(Math.min(10, Math.max(4, creds.password.length)))}
        </span>
        <button type="button" className={styles.reveal} onClick={() => setReveal((r) => !r)} aria-label={reveal ? 'Hide password' : 'Reveal password'}>
          {reveal ? <EyeOff size={11} aria-hidden /> : <Eye size={11} aria-hidden />}
        </button>
      </span>
    </div>
  );
}

// =====================================================================================
// Docs popover
// =====================================================================================

function DocsContent({ doc }: { doc: HeaderDoc }) {
  return (
    <div className={styles.docs}>
      <div className={styles.docsHead}>
        <span className={styles.docsName}>{doc.name}</span>
        <Badge size="sm">{doc.side === 'req' ? 'Request' : doc.side === 'res' ? 'Response' : 'Request & response'}</Badge>
        {doc.category && <Badge size="sm" variant="accent">{doc.category}</Badge>}
      </div>
      <p className={styles.docsSummary}>{doc.summary}</p>
      {doc.url && (
        <button type="button" className={styles.docsLink} onClick={() => openExternal(doc.url as string)}>
          MDN reference <ExternalLink size={12} aria-hidden />
        </button>
      )}
    </div>
  );
}

// =====================================================================================
// Row
// =====================================================================================

interface RowProps {
  entry: HeaderEntry;
  focused: boolean;
  expanded: boolean;
  showAll: boolean;
  docsOpen: boolean;
  onToggle: (index: number) => void;
  onShowAll: (index: number) => void;
  onDocs: (index: number | null) => void;
  onFocusRow: (index: number) => void;
  onFilter?: (name: string, value: string) => void;
  onShowColumn?: (name: string) => void;
}

const HeaderRow = memo(function HeaderRow({ entry, focused, expanded, showAll, docsOpen, onToggle, onShowAll, onDocs, onFocusRow, onFilter, onShowColumn }: RowProps) {
  const { name, value, index, pseudo } = entry;
  const special = useMemo(() => classifyHeader(name, value), [name, value]);
  const doc = useMemo(() => lookupHeaderDoc(name), [name]);
  const long = value.length > LONG_HEADER_VALUE;
  const clamp = long && !showAll;
  const expandable = special === 'cookie' || special === 'set-cookie' || special === 'jwt';

  let main: ReactNode = <span className={clsx(styles.text, clamp && styles.clamp)}>{value}</span>;
  if (special === 'content-type') {
    main = (
      <span className={styles.inlineRow}>
        <KindIcon kind={kindFromContentType(value)} className={styles.kind} size={12} />
        <span className={styles.text}>{value}</span>
      </span>
    );
  } else if (special === 'date') {
    main = <DateValue value={value} />;
  }

  return (
    <div
      role="row"
      aria-rowindex={index + 1}
      tabIndex={focused ? 0 : -1}
      data-row={index}
      className={clsx(styles.row, pseudo && styles.pseudo)}
      onFocus={(e) => e.target === e.currentTarget && onFocusRow(index)}
      onMouseDown={() => onFocusRow(index)}
    >
      <div role="rowheader" className={styles.name}>
        {name}
      </div>
      <div role="gridcell" className={styles.value}>
        {main}
        {long && (
          <button type="button" className={styles.more} onClick={() => onShowAll(index)} tabIndex={-1}>
            {showAll ? 'Show less' : `Show all · ${formatNumber(value.length)} chars`}
          </button>
        )}
        {special === 'set-cookie' && <SetCookieSummary value={value} open={expanded} onToggle={() => onToggle(index)} />}
        {special === 'jwt' && <JwtSummary value={value} open={expanded} onToggle={() => onToggle(index)} />}
        {special === 'basic' && <BasicSummary value={value} />}
        {special === 'cookie' && (
          <div className={styles.pills}>
            <button type="button" className={clsx(styles.pill, styles.accent)} onClick={() => onToggle(index)} aria-expanded={expanded}>
              {pluralize(parseCookieHeader(value).length, 'cookie')}
              <ChevronDown size={10} strokeWidth={2.5} className={clsx(styles.chev, expanded && styles.chevOpen)} aria-hidden />
            </button>
          </div>
        )}
      </div>
      {expanded && expandable && special === 'cookie' && <CookiePanel value={value} />}
      {expanded && expandable && special === 'set-cookie' && <SetCookiePanel value={value} />}
      {expanded && expandable && special === 'jwt' && <JwtPanel value={value} />}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: keeps a press on the row actions from selecting the row */}
      <div className={styles.actions} data-open={docsOpen || undefined} onMouseDown={(e) => e.stopPropagation()}>
        <IconButton label="Copy value" icon={Copy} size="sm" shortcut="c" tabIndex={-1} onClick={() => void copy(value, 'Value')} />
        <IconButton label="Copy header line" icon={ClipboardCopy} size="sm" shortcut="shift+c" tabIndex={-1} onClick={() => void copy(`${name}: ${value}`, 'Header')} />
        {onFilter && !pseudo && <IconButton label="Filter traffic by this header" icon={Filter} size="sm" shortcut="f" tabIndex={-1} onClick={() => onFilter(name, value)} />}
        {onShowColumn && !pseudo && <IconButton label="Show as a traffic list column" icon={Columns3} size="sm" tabIndex={-1} onClick={() => onShowColumn(name)} />}
        {doc && (
          <Popover open={docsOpen} onOpenChange={(o) => onDocs(o ? index : null)}>
            <PopoverTrigger asChild>
              <IconButton label="Header documentation" icon={BookOpen} size="sm" shortcut="d" tabIndex={-1} tooltip={!docsOpen} />
            </PopoverTrigger>
            <PopoverContent width={340} side="bottom" align="end" onOpenAutoFocus={(e) => e.preventDefault()}>
              <DocsContent doc={doc} />
            </PopoverContent>
          </Popover>
        )}
      </div>
    </div>
  );
});

/**
 * Header list (DESIGN.md §6.4): wire order or sorted, HTTP/2 pseudo-headers dimmed on top, hover
 * actions (copy value / line, filter, docs), Cookie/Set-Cookie/JWT/Basic expanders, kind icons for
 * Content-Type and relative times for dates. Rows are a roving-tabindex grid (↑/↓, Enter, C, F, D).
 */
export function HeadersTable({ headers, httpVersion, side, onFilter, onShowColumn, emptyText, className, toolbar = true }: HeadersTableProps) {
  const [sortedPref, setSorted] = usePagePref<boolean>(SORT_PREF, false);
  const sorted = sortedPref === true;
  const entries = useMemo(() => orderHeaders(headers, sorted), [headers, sorted]);
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(new Set());
  const [showAll, setShowAll] = useState<ReadonlySet<number>>(new Set());
  const [docsFor, setDocsFor] = useState<number | null>(null);
  const [focusIndex, setFocusIndex] = useState<number | null>(null);
  const gridRef = useRef<HTMLDivElement | null>(null);

  useShortcutDocs(
    headers.length
      ? [
          { combo: 'arrowup or arrowdown', description: 'Move between headers', group: 'Headers' },
          { combo: 'enter', description: 'Expand cookies / JWT', group: 'Headers' },
          { combo: 'c', description: 'Copy header value', group: 'Headers' },
          { combo: 'shift+c', description: 'Copy “Name: value”', group: 'Headers' },
          { combo: 'f', description: 'Filter traffic by header', group: 'Headers' },
          { combo: 'd', description: 'Header documentation', group: 'Headers' },
        ]
      : [],
  );

  const toggle = useCallback((i: number) => setExpanded((s) => toggleIn(s, i)), []);
  const toggleShowAll = useCallback((i: number) => setShowAll((s) => toggleIn(s, i)), []);
  const firstIndex = entries[0]?.index ?? null;
  const activeIndex = focusIndex != null && entries.some((e) => e.index === focusIndex) ? focusIndex : firstIndex;

  const focusRow = (i: number) => {
    setFocusIndex(i);
    gridRef.current?.querySelector<HTMLElement>(`[data-row="${i}"]`)?.focus();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget && !(e.target as HTMLElement).matches('[role="row"]')) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const pos = entries.findIndex((x) => x.index === activeIndex);
    const entry = entries[pos];
    if (!entry) return;
    const move = (to: number) => {
      const t = entries[Math.max(0, Math.min(entries.length - 1, to))];
      if (t) focusRow(t.index);
    };
    switch (e.key) {
      case 'ArrowDown':
      case 'j':
        move(pos + 1);
        break;
      case 'ArrowUp':
      case 'k':
        move(pos - 1);
        break;
      case 'Home':
        move(0);
        break;
      case 'End':
        move(entries.length - 1);
        break;
      case 'Enter':
      case ' ': {
        const special = classifyHeader(entry.name, entry.value);
        if (special === 'cookie' || special === 'set-cookie' || special === 'jwt') toggle(entry.index);
        else if (entry.value.length > LONG_HEADER_VALUE) toggleShowAll(entry.index);
        else return;
        break;
      }
      case 'c':
        void copy(entry.value, 'Value');
        break;
      case 'C':
        void copy(`${entry.name}: ${entry.value}`, 'Header');
        break;
      case 'f':
        if (!onFilter || entry.pseudo) return;
        onFilter(entry.name, entry.value);
        break;
      case 'd':
        if (!lookupHeaderDoc(entry.name)) return;
        setDocsFor(entry.index);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const pseudoCount = entries.filter((e) => e.pseudo).length;
  const copyAll = headersToText(entries);

  return (
    <div className={clsx(styles.root, className)}>
      {toolbar && headers.length > 0 && (
        <div className={styles.toolbar}>
          <SegmentedControl
            size="sm"
            aria-label="Header order"
            value={sorted ? 'sorted' : 'wire'}
            onValueChange={(v) => setSorted(v === 'sorted')}
            items={[
              { value: 'wire', label: 'Wire order', 'aria-label': 'Wire order (as sent)' },
              { value: 'sorted', label: 'Sorted', 'aria-label': 'Sorted by name' },
            ]}
          />
          <span className={clsx(styles.count, 'tnum')}>
            {pluralize(headers.length - pseudoCount, 'header')}
            {pseudoCount > 0 && ` + ${pseudoCount} pseudo`}
            {httpVersion ? ` · ${httpVersion}` : ''}
          </span>
          <span className={styles.spacer} />
          <CopyButton value={copyAll} label={`Copy all ${side ? `${side} ` : ''}headers`} toastMessage={`Copied ${pluralize(headers.length - pseudoCount, 'header')}`} disabled={!copyAll} />
        </div>
      )}
      {headers.length === 0 ? (
        <div className={styles.empty}>{emptyText ?? 'No headers'}</div>
      ) : (
        <div ref={gridRef} role="grid" aria-label={`${side ? `${side[0]?.toUpperCase()}${side.slice(1)} ` : ''}headers`} aria-rowcount={entries.length} className={styles.grid} onKeyDown={onKeyDown}>
          {entries.map((entry, i) => (
            <div key={`${entry.index}`} role="presentation" style={{ display: 'contents' }}>
              {i === pseudoCount && pseudoCount > 0 && <div className={styles.pseudoDivider} role="separator" />}
              <HeaderRow
                entry={entry}
                focused={entry.index === activeIndex}
                expanded={expanded.has(entry.index)}
                showAll={showAll.has(entry.index)}
                docsOpen={docsFor === entry.index}
                onToggle={toggle}
                onShowAll={toggleShowAll}
                onDocs={setDocsFor}
                onFocusRow={setFocusIndex}
                onFilter={onFilter}
                onShowColumn={onShowColumn}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function toggleIn(s: ReadonlySet<number>, i: number): Set<number> {
  const n = new Set(s);
  if (n.has(i)) n.delete(i);
  else n.add(i);
  return n;
}
