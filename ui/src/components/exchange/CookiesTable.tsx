import clsx from 'clsx';
import { AlertTriangle, ChevronRight, Cookie } from 'lucide-react';
import { Fragment, memo, useMemo, useState, type KeyboardEvent } from 'react';
import type { Headers, Side } from '../../api/types';
import { formatBytes, formatDateTime, pluralize } from '../../lib/format';
import type { SetCookie } from '../../lib/http';
import { Badge, CopyButton, EmptyState, Tooltip } from '../ui';
import { cookieExpiry, cookieSize, cookieWarnings, decodedCookieValue, requestCookies, responseCookies } from './model/cookies';
import { relTime } from './model/time';
import { InfoGrid } from './parts/Parts';
import styles from './CookiesTable.module.css';

export interface CookiesTableProps {
  /** Request headers (parses `Cookie`) or response headers (parses `Set-Cookie`). */
  headers: Headers;
  side: Side;
  className?: string;
}

function ExpiresCell({ c }: { c: SetCookie }) {
  const exp = cookieExpiry(c);
  if (exp.kind === 'session') return <span className={styles.subtle}>Session</span>;
  if (exp.kind === 'invalid')
    return (
      <Tooltip content={`Unparseable Expires: ${exp.raw}`}>
        <span className={styles.expired}>Invalid date</span>
      </Tooltip>
    );
  const deletes = exp.source === 'max-age' && (c.maxAge ?? 0) <= 0;
  return (
    <Tooltip content={`${formatDateTime(exp.at)} local · from ${exp.source === 'max-age' ? `Max-Age=${c.maxAge}` : 'Expires'}`}>
      <span className={clsx('tnum', exp.kind === 'expired' ? styles.expired : styles.muted)}>{deletes ? 'Deletes cookie' : exp.kind === 'expired' ? `Expired ${relTime(exp.at)}` : relTime(exp.at)}</span>
    </Tooltip>
  );
}

function Flags({ c, warnings }: { c: SetCookie; warnings: string[] }) {
  const priority = c.other.find(([k]) => k.toLowerCase() === 'priority')?.[1];
  return (
    <span className={styles.flags}>
      {c.httpOnly && <Badge size="sm">HttpOnly</Badge>}
      {c.secure && <Badge size="sm">Secure</Badge>}
      {c.sameSite && (
        <Badge size="sm" variant={c.sameSite.toLowerCase() === 'none' ? 'info' : 'neutral'}>
          SameSite={c.sameSite}
        </Badge>
      )}
      {c.partitioned && <Badge size="sm">Partitioned</Badge>}
      {priority && <Badge size="sm">Priority={priority}</Badge>}
      {warnings.length > 0 && (
        <Tooltip content={warnings.join(' ')} multiline>
          <Badge size="sm" variant="warning" icon={AlertTriangle}>
            {warnings.length === 1 ? 'Issue' : `${warnings.length} issues`}
          </Badge>
        </Tooltip>
      )}
    </span>
  );
}

const SetCookieRow = memo(function SetCookieRow({ c, open, onToggle }: { c: SetCookie; open: boolean; onToggle: () => void }) {
  const warnings = useMemo(() => cookieWarnings(c), [c]);
  const decoded = decodedCookieValue(c.value);
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onToggle();
    }
  };
  const exp = cookieExpiry(c);
  return (
    <Fragment>
      <tr className={clsx(styles.row, open && styles.open)} tabIndex={0} aria-expanded={open} onClick={onToggle} onKeyDown={onKeyDown}>
        <td className={styles.name}>
          <span className={styles.nameInner}>
            <ChevronRight size={13} className={clsx(styles.chev, open && styles.chevOpen)} aria-hidden />
            <span className={styles.mono}>{c.name || '(no name)'}</span>
          </span>
        </td>
        <td className={styles.value}>
          <span className={clsx(styles.valueText, styles.mono)} title={c.value.length > 40 ? c.value : undefined}>
            {c.value || <span className={styles.subtle}>empty</span>}
          </span>
          {/* biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: only stops the click from reaching the row behind it; the controls inside are buttons */}
          <span className={styles.copy} onClick={(e) => e.stopPropagation()}>
            <CopyButton value={c.value} label="Copy value" />
          </span>
        </td>
        <td className={clsx(styles.mono, styles.muted, styles.wide)}>{c.domain ?? <span className={styles.subtle}>host only</span>}</td>
        <td className={clsx(styles.mono, styles.muted, styles.wide)}>{c.path ?? <span className={styles.subtle}>default</span>}</td>
        <td>
          <ExpiresCell c={c} />
        </td>
        <td className={clsx(styles.num, 'tnum', styles.wide)}>{formatBytes(cookieSize(c.name, c.value))}</td>
        <td>
          <Flags c={c} warnings={warnings} />
        </td>
      </tr>
      {open && (
        <tr className={styles.detail}>
          <td colSpan={7}>
            <div className={styles.detailBox}>
              <div className={styles.fullValue}>
                {c.value || '(empty value)'}
                {decoded && <span className={styles.decoded}>Decoded: {decoded}</span>}
              </div>
              <InfoGrid
                rows={[
                  { label: 'Domain', value: c.domain ?? 'Host only (no Domain attribute)', mono: !!c.domain },
                  { label: 'Path', value: c.path ?? 'Default (request directory)', mono: !!c.path },
                  {
                    label: 'Expires',
                    value: exp.kind === 'session' ? 'At the end of the browser session' : exp.kind === 'invalid' ? exp.raw : `${formatDateTime(exp.at)} (${relTime(exp.at)})`,
                    hint: exp.kind === 'expires' || exp.kind === 'expired' ? (exp.source === 'max-age' ? `Max-Age=${c.maxAge}${c.expires ? ' (overrides Expires)' : ''}` : 'Expires') : undefined,
                  },
                  { label: 'Size', value: formatBytes(cookieSize(c.name, c.value)) },
                  ...c.other.map(([k, v]) => ({ label: k, value: v || 'set', mono: true })),
                ]}
              />
              {warnings.map((w) => (
                <div key={w} className={styles.warn}>
                  <AlertTriangle size={12} strokeWidth={2.25} aria-hidden />
                  {w}
                </div>
              ))}
            </div>
          </td>
        </tr>
      )}
    </Fragment>
  );
});

/**
 * Cookies (DESIGN.md §6.4): request `Cookie` pairs, or response `Set-Cookie` with domain, path,
 * expiry (relative, Max-Age aware), size and flag pills; rows expand to the full value,
 * attributes and spec warnings (SameSite=None without Secure, __Host-/__Secure- rules…).
 */
export function CookiesTable({ headers, side, className }: CookiesTableProps) {
  const reqCookies = useMemo(() => (side === 'request' ? requestCookies(headers) : []), [headers, side]);
  const resCookies = useMemo(() => (side === 'response' ? responseCookies(headers) : []), [headers, side]);
  const [open, setOpen] = useState<ReadonlySet<number>>(new Set());
  const count = side === 'request' ? reqCookies.length : resCookies.length;
  const total = side === 'request' ? reqCookies.reduce((n, c) => n + cookieSize(c.name, c.value), 0) : resCookies.reduce((n, c) => n + cookieSize(c.name, c.value), 0);

  if (count === 0) {
    return (
      <div className={clsx(styles.root, className)}>
        <EmptyState icon={Cookie} size="sm" title="No cookies" description={side === 'request' ? "This request didn't send any cookies." : "This response doesn't set any cookies."} />
      </div>
    );
  }

  const toggle = (i: number) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(i)) n.delete(i);
      else n.add(i);
      return n;
    });

  return (
    <div className={clsx(styles.root, className)}>
      <div className={styles.toolbar}>
        <span className="tnum">
          {pluralize(count, side === 'request' ? 'cookie' : 'Set-Cookie', side === 'request' ? 'cookies' : 'Set-Cookie headers')} · {formatBytes(total)}
        </span>
        <span className={styles.spacer} />
        {side === 'request' && <CopyButton value={reqCookies.map((c) => `${c.name}=${c.value}`).join('; ')} label="Copy as Cookie header value" toastMessage="Cookies copied" />}
      </div>
      <div className={styles.scroll}>
        {side === 'request' ? (
          <table className={styles.table} aria-label="Request cookies">
            <thead>
              <tr>
                <th>Name</th>
                <th>Value</th>
                <th className={styles.numHead}>Size</th>
              </tr>
            </thead>
            <tbody>
              {reqCookies.map((c, i) => {
                const decoded = decodedCookieValue(c.value);
                return (
                  <tr key={i} className={clsx(styles.row, styles.static)}>
                    <td className={clsx(styles.name, styles.mono)}>{c.name || <span className={styles.subtle}>(no name)</span>}</td>
                    <td className={clsx(styles.value, styles.reqValue)}>
                      <Tooltip content={decoded ? `Decoded: ${decoded}` : undefined} multiline>
                        <span className={clsx(styles.valueText, styles.mono)}>{c.value || <span className={styles.subtle}>empty</span>}</span>
                      </Tooltip>
                      <span className={styles.copy}>
                        <CopyButton value={c.value} label="Copy value" />
                      </span>
                    </td>
                    <td className={clsx(styles.num, 'tnum')}>{formatBytes(cookieSize(c.name, c.value))}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ) : (
          <table className={styles.table} aria-label="Response cookies">
            <thead>
              <tr>
                <th>Name</th>
                <th>Value</th>
                <th className={styles.wide}>Domain</th>
                <th className={styles.wide}>Path</th>
                <th>Expires</th>
                <th className={clsx(styles.wide, styles.numHead)}>Size</th>
                <th>Flags</th>
              </tr>
            </thead>
            <tbody>
              {resCookies.map((c, i) => (
                <SetCookieRow key={i} c={c} open={open.has(i)} onToggle={() => toggle(i)} />
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
