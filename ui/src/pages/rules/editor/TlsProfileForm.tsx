import { Fingerprint, Info, TriangleAlert } from 'lucide-react';
import type { Action, TlsProfile } from '../../../api/types';
import { ProCallout } from '../../../components/pro';
import { Select } from '../../../components/ui';
import { needsImpersonation, profileAvailable, TLS_PROFILE_LABELS, tlsProfileOptions } from '../../../lib/tlsProfiles';
import { useFeature } from '../../../state/license';
import { useTlsProfiles } from '../../../state/tlsProfiles';
import { Block, Explain, useIssue } from './context';
import styles from './Editor.module.css';

/** Form of the `tls-profile` modifier action. */
export function TlsProfileForm({ action, onChange }: { action: Extract<Action, { type: 'tls-profile' }>; onChange: (a: Action) => void }) {
  const catalog = useTlsProfiles();
  const issue = useIssue('action.profile');
  const info = catalog?.profiles.find((p) => p.id === action.profile);
  const unavailable = !profileAvailable(action.profile, catalog);
  const licensed = useFeature('tls-impersonation') && catalog?.licensed !== false;
  return (
    <div className={styles.stack}>
      {!licensed && needsImpersonation(action.profile) && (
        <ProCallout feature="tls-impersonation" title="TLS fingerprint rules need Akev Pro">
          This rule is saved but inactive: matching requests use the native TLS stack until Pro is active.
        </ProCallout>
      )}
      <Block label="Present the server with" error={issue}>
        <Select<TlsProfile>
          value={action.profile}
          onValueChange={(profile) => onChange({ ...action, profile })}
          options={tlsProfileOptions(catalog).map((o) => ({ value: o.value, label: o.label, description: o.description }))}
          renderValue={(o) => (o ? TLS_PROFILE_LABELS[o.value] : action.profile)}
          aria-label="TLS fingerprint profile"
        />
      </Block>
      {info?.ja4 && (
        <Explain icon={<Fingerprint size={13} aria-hidden />}>
          {info.imitates ?? info.label}: JA4 <code>{info.ja4}</code>
          {info.http2 && (
            <>
              , HTTP/2 <code>{info.http2}</code>
            </>
          )}
        </Explain>
      )}
      {unavailable && (
        <Explain icon={<TriangleAlert size={13} aria-hidden />}>
          Requires an impersonation build of Akev (<code>make release-impersonate</code>). Until then matching requests use the native TLS stack, and the exchange's Connection tab says so.
        </Explain>
      )}
      <Explain icon={<Info size={13} aria-hidden />}>
        A modifier: later rules still run. It overrides <strong>Settings → Proxy → Upstream TLS fingerprint</strong> for matching requests (the last matching TLS fingerprint rule wins). Only the TLS handshake and HTTP/2 framing change — headers such as User-Agent are forwarded as the client sent them.
      </Explain>
    </div>
  );
}
