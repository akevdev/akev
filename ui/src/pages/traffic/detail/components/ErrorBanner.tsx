import clsx from 'clsx';
import {
  Ban,
  CircleOff,
  Clock,
  FileWarning,
  Globe,
  Lightbulb,
  LockKeyhole,
  ShieldAlert,
  Unplug,
  Zap,
  type LucideIcon,
} from 'lucide-react';
import { useState, type ReactNode } from 'react';
import type { ErrorKind, ExchangeError, ExchangeSummary } from '../../../../api/types';
import { navigate } from '../../../../app/router';
import { Button } from '../../../../components/ui';
import { formatAuthority, describeError } from '../../../../lib/http';
import { useAppStore } from '../../../../state/app';
import { createEmptyMock } from '../../actions/exchangeActions';
import { isHostListed, setHostListed } from '../../actions/hostLists';
import styles from './Banners.module.css';

const ICONS: Partial<Record<ErrorKind, LucideIcon>> = {
  'client-aborted': CircleOff,
  'dns-failed': Globe,
  'connection-refused': Ban,
  'connection-reset': Zap,
  'host-unreachable': Unplug,
  timeout: Clock,
  'upstream-cert-invalid': ShieldAlert,
  'upstream-tls-failed': LockKeyhole,
  'client-tls-rejected': LockKeyhole,
  'upstream-protocol-error': FileWarning,
  'client-protocol-error': FileWarning,
  'rule-closed': Ban,
  'rule-reset': Zap,
  'rule-timeout': Clock,
  'breakpoint-aborted': CircleOff,
  'upstream-proxy-failed': Unplug,
};

/** Errors where answering locally (a mock) is a sensible way forward. */
const MOCKABLE = new Set<ErrorKind>(['dns-failed', 'connection-refused', 'host-unreachable', 'timeout', 'connection-reset']);
/** Errors caused by deliberate user choices: shown calmly (neutral tone). */
const DELIBERATE = new Set<ErrorKind>(['rule-closed', 'rule-reset', 'rule-timeout', 'breakpoint-aborted', 'client-aborted']);

export interface ErrorBannerProps {
  error: ExchangeError;
  summary: Pick<ExchangeSummary, 'method' | 'scheme' | 'host' | 'port' | 'path'>;
  /** Extra action buttons. */
  actions?: ReactNode;
  /** Offer one-click fixes (off when the surrounding layout already does). */
  fixes?: boolean;
  className?: string;
}

/**
 * Plain-language explanation of an aborted exchange (DESIGN.md §6.4): title, what happened, how
 * to fix it (with one-click fixes where Akev can apply them), and the raw error in mono.
 */
export function ErrorBanner({ error, summary, actions, fixes = true, className }: ErrorBannerProps) {
  const authority = formatAuthority(summary.scheme, summary.host, summary.port);
  const info = describeError(error.kind, error.kind === 'client-tls-rejected' ? summary.host : authority);
  const Icon = ICONS[error.kind] ?? Ban;
  const calm = DELIBERATE.has(error.kind);
  const raw = [error.code, error.message].filter(Boolean);

  return (
    <div className={clsx(styles.banner, calm ? styles.neutral : styles.error, className)} role="alert">
      <Icon size={16} strokeWidth={2} className={styles.icon} aria-hidden />
      <div className={styles.text}>
        <div className={styles.title}>{info.title}</div>
        <p className={styles.body}>{info.explanation}</p>
        {info.fix && (
          <p className={styles.fix}>
            <Lightbulb size={12} strokeWidth={2} aria-hidden />
            <span>{info.fix}</span>
          </p>
        )}
        {raw.length > 0 && (
          <p className={clsx(styles.raw, 'selectable')}>
            {error.code && <span className={styles.code}>{error.code}</span>}
            {error.message && error.message !== error.code && <span>{error.message}</span>}
          </p>
        )}
      </div>
      <div className={styles.actionsCol}>
        {fixes && <ErrorFixes kind={error.kind} summary={summary} />}
        {actions}
      </div>
    </div>
  );
}

function ErrorFixes({ kind, summary }: { kind: ErrorKind; summary: ErrorBannerProps['summary'] }) {
  const settings = useAppStore((s) => s.settings);
  const [busy, setBusy] = useState(false);
  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await fn();
    } finally {
      setBusy(false);
    }
  };

  if (kind === 'upstream-cert-invalid') {
    const listed = isHostListed(settings, 'ignoreUpstreamCertErrors', summary.host);
    return (
      <Button size="sm" loading={busy} disabled={listed} onClick={() => void run(() => setHostListed('ignoreUpstreamCertErrors', summary.host, true))}>
        {listed ? 'Certificate errors ignored' : 'Ignore for this host'}
      </Button>
    );
  }
  if (kind === 'client-tls-rejected') {
    const listed = isHostListed(settings, 'tlsPassthrough', summary.host);
    return (
      <Button size="sm" loading={busy} disabled={listed} onClick={() => void run(() => setHostListed('tlsPassthrough', summary.host, true))}>
        {listed ? 'In TLS passthrough' : 'Add to TLS passthrough'}
      </Button>
    );
  }
  if (kind === 'upstream-proxy-failed' || kind === 'dns-failed') {
    return (
      <>
        <Button size="sm" variant={kind === 'dns-failed' ? 'ghost' : 'secondary'} onClick={() => navigate('/settings/proxy')}>
          {kind === 'dns-failed' ? 'DNS settings' : 'Proxy settings'}
        </Button>
        {kind === 'dns-failed' && <MockButton summary={summary} />}
      </>
    );
  }
  if (MOCKABLE.has(kind)) return <MockButton summary={summary} />;
  return null;
}

function MockButton({ summary }: { summary: ErrorBannerProps['summary'] }) {
  const [busy, setBusy] = useState(false);
  return (
    <Button
      size="sm"
      loading={busy}
      onClick={async () => {
        setBusy(true);
        await createEmptyMock(summary);
        setBusy(false);
      }}
    >
      Mock this endpoint
    </Button>
  );
}
