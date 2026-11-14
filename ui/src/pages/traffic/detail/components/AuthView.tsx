import clsx from 'clsx';
import { Eye, EyeOff, KeyRound, ShieldCheck, UserRound } from 'lucide-react';
import { useState } from 'react';
import { Badge, CodeEditor, CopyButton, IconButton } from '../../../../components/ui';
import type { DecodedJwt } from '../../../../lib/http';
import { describeClaims, describeExpiry, jwtValidity, type AuthItem } from '../lib/auth';
import styles from './views.module.css';

export interface AuthViewProps {
  items: readonly AuthItem[];
  /** Exchange id (resets revealed secrets when switching). */
  exchangeId: number;
}

/** Credentials carried by the request: Basic (decoded), bearer/JWT (decoded claims), API keys. */
export function AuthView({ items, exchangeId }: AuthViewProps) {
  return (
    <div className={styles.stack}>
      {items.map((item, i) => (
        <AuthCard key={`${exchangeId}:${i}`} item={item} />
      ))}
      <p className={styles.hint}>Tokens are decoded locally for inspection — signatures are not verified.</p>
    </div>
  );
}

function AuthCard({ item }: { item: AuthItem }) {
  switch (item.kind) {
    case 'basic':
      return <BasicCard item={item} />;
    case 'bearer':
      return item.jwt ? <JwtCard source={item.source} title="Bearer token" token={item.token} jwt={item.jwt} /> : <OpaqueCard source={item.source} title="Bearer token" value={item.token} />;
    case 'jwt':
      return <JwtCard source={item.source} title="JSON Web Token" token={item.token} jwt={item.jwt} />;
    case 'api-key':
      return <OpaqueCard source={item.source} title="API key" value={item.value} />;
    case 'scheme':
      return <OpaqueCard source={item.source} title={`${item.scheme} credentials`} value={item.credentials} />;
  }
}

function BasicCard({ item }: { item: Extract<AuthItem, { kind: 'basic' }> }) {
  const [reveal, setReveal] = useState(false);
  return (
    <section className={styles.card}>
      <header className={styles.cardHead}>
        <span className={styles.cardTitle}>
          <UserRound size={14} strokeWidth={1.75} aria-hidden />
          Basic authentication
        </span>
        <span className={styles.cardSource}>{item.source}</span>
      </header>
      <div className={styles.cardBody}>
        {item.decoded ? (
          <dl className={styles.dl}>
            <dt className={styles.dt}>Username</dt>
            <dd className={styles.dd}>
              <span className={clsx(styles.mono, 'selectable')}>{item.username || <span className={styles.dim}>empty</span>}</span>
              <span className={styles.inlineActions}>
                <CopyButton value={item.username} label="Copy username" />
              </span>
            </dd>
            <dt className={styles.dt}>Password</dt>
            <dd className={styles.dd}>
              <span className={clsx(styles.mono, 'selectable', !reveal && styles.secret)}>{reveal ? item.password || '(empty)' : '•'.repeat(Math.min(12, Math.max(6, item.password.length)))}</span>
              <span className={styles.inlineActions}>
                <IconButton size="sm" label={reveal ? 'Hide password' : 'Show password'} icon={reveal ? EyeOff : Eye} onClick={() => setReveal((r) => !r)} />
                <CopyButton value={item.password} label="Copy password" />
              </span>
            </dd>
          </dl>
        ) : (
          <p className={styles.hint}>The credentials aren't valid base64-encoded UTF-8.</p>
        )}
        <code className={clsx(styles.rawCode, 'selectable')}>Basic {item.raw}</code>
      </div>
    </section>
  );
}

function OpaqueCard({ source, title, value }: { source: string; title: string; value: string }) {
  return (
    <section className={styles.card}>
      <header className={clsx(styles.cardHead, styles.cardHeadSplit)}>
        <span className={styles.cardHeadMain}>
          <span className={styles.cardTitle}>
            <KeyRound size={14} strokeWidth={1.75} aria-hidden />
            {title}
          </span>
          <span className={styles.cardSource}>{source}</span>
        </span>
        <CopyButton value={value} label="Copy value" />
      </header>
      <div className={styles.cardBody}>
        <code className={clsx(styles.rawCode, 'selectable')}>{value || '(empty)'}</code>
      </div>
    </section>
  );
}

function JwtCard({ source, title, token, jwt }: { source: string; title: string; token: string; jwt: DecodedJwt }) {
  const [now] = useState(() => Date.now());
  const validity = jwtValidity(jwt, now);
  const claims = describeClaims(jwt.payload, now);
  const alg = typeof jwt.header.alg === 'string' ? jwt.header.alg : null;
  return (
    <section className={styles.card}>
      <header className={clsx(styles.cardHead, styles.cardHeadSplit)}>
        <span className={styles.cardHeadMain}>
          <span className={styles.cardTitle}>
            <ShieldCheck size={14} strokeWidth={1.75} aria-hidden />
            {title}
          </span>
          <Badge size="sm" variant="accent">
            JWT
          </Badge>
          {alg && (
            <Badge size="sm" mono>
              {alg}
            </Badge>
          )}
          <Badge size="sm" dot variant={validity === 'expired' || validity === 'not-yet-valid' ? 'danger' : validity === 'valid' ? 'success' : 'neutral'}>
            {describeExpiry(jwt, now)}
          </Badge>
          <span className={styles.cardSource}>{source}</span>
        </span>
        <CopyButton value={token} label="Copy token" />
      </header>
      <div className={styles.cardBody}>
        {claims.length > 0 && (
          <dl className={styles.dl}>
            {claims.map((c) => (
              <ClaimRow key={c.claim} label={c.label} claim={c.claim} value={c.value} hint={c.hint} registered={c.registered} />
            ))}
          </dl>
        )}
        <div className={styles.jsonGrid}>
          <div className={styles.jsonBox}>
            <span className={styles.caps}>Header</span>
            <CodeEditor value={JSON.stringify(jwt.header, null, 2)} language="json" readOnly lineNumbers={false} folding={false} maxHeight={220} contentId={`jwt-h:${token.slice(0, 32)}`} aria-label="JWT header" />
          </div>
          <div className={styles.jsonBox}>
            <span className={styles.caps}>Payload</span>
            <CodeEditor value={JSON.stringify(jwt.payload, null, 2)} language="json" readOnly lineNumbers={false} folding={false} maxHeight={320} contentId={`jwt-p:${token.slice(0, 32)}`} aria-label="JWT payload" />
          </div>
        </div>
        <div className={styles.jsonBox}>
          <span className={styles.caps}>Signature</span>
          <span className={clsx(styles.signature, 'selectable')}>{jwt.signature || '(none — unsigned token)'}</span>
        </div>
      </div>
    </section>
  );
}

function ClaimRow({ label, claim, value, hint, registered }: { label: string; claim: string; value: string; hint?: string; registered: boolean }) {
  return (
    <>
      <dt className={styles.dt} title={registered ? claim : undefined}>
        {label}
      </dt>
      <dd className={styles.dd}>
        <span className={clsx(styles.mono, 'selectable')}>{value}</span>
        {hint && <span className={styles.hint}>{hint}</span>}
      </dd>
    </>
  );
}
