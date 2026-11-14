import clsx from 'clsx';
import { Eye, EyeOff, KeyRound, ShieldCheck, ShieldX } from 'lucide-react';
import { memo, useMemo, useState } from 'react';
import { CopyButton, EmptyState, Field, IconButton, Input, SegmentedControl, TextArea } from '../../components/ui';
import { formatRelativeTime } from '../../lib/format';
import { decodeJwt } from '../../lib/http';
import type { AuthState, AuthType } from './model';
import { authHeaderValue } from './request';
import { substitute, type VarMap } from './variables';
import styles from './RequestEditor.module.css';

export const AUTH_LABEL: Record<AuthType, string> = { none: 'None', basic: 'Basic', bearer: 'Bearer' };

export interface AuthEditorProps {
  auth: AuthState;
  vars: VarMap;
  onAuth: (auth: AuthState) => void;
}

/** Auth tab: None / Basic / Bearer → an `Authorization` header (previewed, variables resolved). */
export const AuthEditor = memo(function AuthEditor({ auth, vars, onAuth }: AuthEditorProps) {
  const [reveal, setReveal] = useState(false);
  const subst = (s: string) => substitute(s, vars).text;
  const header = authHeaderValue(auth, subst);
  const jwt = useMemo(() => (auth.type === 'bearer' ? decodeJwt(substitute(auth.token, vars).text) : null), [auth.type, auth.token, vars]);

  return (
    <div className={clsx(styles.scrollPad, styles.authRoot)}>
      <SegmentedControl
        size="sm"
        value={auth.type}
        onValueChange={(type) => onAuth({ ...auth, type })}
        items={(['none', 'basic', 'bearer'] as const).map((t) => ({ value: t, label: AUTH_LABEL[t] }))}
        aria-label="Authorization type"
      />
      {auth.type === 'none' && (
        <EmptyState size="sm" icon={KeyRound} title="No authorization" description="Choose Basic or Bearer to have Akev build the Authorization header. Variables like {{token}} work in every field." />
      )}
      {auth.type === 'basic' && (
        <div className={styles.authFields}>
          <Field label="Username">
            <Input mono value={auth.username} onChange={(e) => onAuth({ ...auth, username: e.target.value })} placeholder="user" autoComplete="off" />
          </Field>
          <Field label="Password">
            <Input
              mono
              type={reveal ? 'text' : 'password'}
              value={auth.password}
              onChange={(e) => onAuth({ ...auth, password: e.target.value })}
              placeholder="••••••"
              autoComplete="new-password"
              trailing={<IconButton size="sm" label={reveal ? 'Hide password' : 'Show password'} icon={reveal ? EyeOff : Eye} onClick={() => setReveal((r) => !r)} />}
            />
          </Field>
        </div>
      )}
      {auth.type === 'bearer' && (
        <div className={styles.authFields}>
          <Field label="Token" hint="Sent as “Authorization: Bearer <token>”.">
            <TextArea mono autoResize minRows={2} maxRows={8} value={auth.token} onChange={(e) => onAuth({ ...auth, token: e.target.value })} placeholder="eyJhbGciOi… or {{token}}" spellCheck={false} />
          </Field>
          {jwt && (
            <div className={styles.jwtCard}>
              <div className={styles.jwtHead}>
                <span className={clsx(styles.jwtBadge, jwt.expired ? styles.jwtExpired : styles.jwtValid)}>
                  {jwt.expired ? <ShieldX size={12} strokeWidth={2} aria-hidden /> : <ShieldCheck size={12} strokeWidth={2} aria-hidden />}
                  JWT
                </span>
                <span className={styles.jwtMeta}>
                  {String(jwt.header.alg ?? 'unknown alg')}
                  {jwt.expiresAt != null && <> · {jwt.expired ? `expired ${formatRelativeTime(jwt.expiresAt)}` : `expires ${formatRelativeTime(jwt.expiresAt)}`}</>}
                  {typeof jwt.payload.sub === 'string' && <> · sub {jwt.payload.sub}</>}
                </span>
              </div>
              <pre className={styles.jwtPayload}>{JSON.stringify(jwt.payload, null, 2)}</pre>
            </div>
          )}
        </div>
      )}
      {header && (
        <div className={styles.authPreview}>
          <span className={styles.authPreviewLabel}>Header</span>
          <code className={styles.authPreviewValue}>
            <span className={styles.authPreviewName}>Authorization:</span> {header}
          </code>
          <CopyButton value={`Authorization: ${header}`} label="Copy header" />
        </div>
      )}
    </div>
  );
});
