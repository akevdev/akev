import { ArrowDown, ArrowUp, Fingerprint, Plus, TriangleAlert, X } from 'lucide-react';
import { useState } from 'react';
import type { Settings, TlsProfile, TlsProfileOverride, TlsProfiles, TlsProfileSettings } from '../../../api/types';
import { ProCallout } from '../../../components/pro';
import { Button, IconButton, Input, Select } from '../../../components/ui';
import { moveOverride, needsImpersonation, profileAvailable, removeOverride, setOverrideProfile, TLS_PROFILE_LABELS, tlsProfileOptions, upsertOverride } from '../../../lib/tlsProfiles';
import { openUpgrade, useFeature } from '../../../state/license';
import { useTlsProfiles } from '../../../state/tlsProfiles';
import { useEdit, type Mutator } from '../context';
import { SettingRow, SettingsCard } from '../SettingRow';
import { normalizeHost, validateHostPattern } from '../validation';
import styles from './sections.module.css';
import own from './TlsProfileCard.module.css';

const setTlsProfile =
  (patch: Partial<TlsProfileSettings>): Mutator =>
  (d) => ({ ...d, proxy: { ...d.proxy, tlsProfile: { ...d.proxy.tlsProfile, ...patch } } });

/**
 * Without Akev Pro, choosing a browser profile opens the upgrade dialog instead (existing
 * choices stay as they are, and switching back to native always works).
 */
function gateProfileChange(licensed: boolean, onChange: (p: TlsProfile) => void): (p: TlsProfile) => void {
  return (p) => (licensed || !needsImpersonation(p) ? onChange(p) : openUpgrade('tls-impersonation'));
}

function ProfileSelect({ value, onChange, catalog, label, size, licensed, gate = true }: { value: TlsProfile; onChange: (p: TlsProfile) => void; catalog: TlsProfiles | null; label: string; size?: 'sm' | 'md'; licensed: boolean; gate?: boolean }) {
  const gated = gateProfileChange(licensed, onChange);
  return (
    <Select<TlsProfile>
      value={value}
      onValueChange={gate ? gated : onChange}
      size={size}
      options={tlsProfileOptions(catalog).map((o) => ({ value: o.value, label: o.label, description: licensed || !needsImpersonation(o.value) ? o.description : `${o.description} · Akev Pro` }))}
      renderValue={(o) => (o ? TLS_PROFILE_LABELS[o.value] : value)}
      aria-label={label}
      className={size === 'sm' ? own.rowSelect : styles.wideSelect}
    />
  );
}

/** Per-host overrides: ordered (first match wins), host glob → profile. */
function OverridesEditor({ overrides, onChange, catalog, licensed }: { overrides: readonly TlsProfileOverride[]; onChange: (next: TlsProfileOverride[]) => void; catalog: TlsProfiles | null; licensed: boolean }) {
  const [host, setHost] = useState('');
  const [profile, setProfile] = useState<TlsProfile>('chrome');
  const [touched, setTouched] = useState(false);
  const error = host.trim() ? validateHostPattern(normalizeHost(host)) : touched ? 'Enter a hostname, e.g. *.example.com' : null;
  const add = () => {
    setTouched(true);
    const h = normalizeHost(host);
    if (!h || validateHostPattern(h)) return;
    if (!licensed && needsImpersonation(profile)) return openUpgrade('tls-impersonation');
    onChange(upsertOverride(overrides, h, profile));
    setHost('');
    setTouched(false);
  };
  return (
    <div className={own.editor}>
      {overrides.length === 0 ? (
        <p className={own.empty}>No per-host profiles — every server gets the profile above.</p>
      ) : (
        <ul className={own.list} aria-label="Per-host TLS fingerprints">
          {overrides.map((o, i) => (
            <li key={`${o.host}-${i}`} className={own.row}>
              <span className={own.host} title={o.host}>
                {o.host}
              </span>
              <ProfileSelect size="sm" value={o.profile} onChange={(p) => onChange(setOverrideProfile(overrides, i, p))} catalog={catalog} licensed={licensed} label={`TLS fingerprint for ${o.host}`} />
              <span className={own.rowActions}>
                <IconButton icon={ArrowUp} size="sm" label="Move up" disabled={i === 0} onClick={() => onChange(moveOverride(overrides, i, -1))} />
                <IconButton icon={ArrowDown} size="sm" label="Move down" disabled={i === overrides.length - 1} onClick={() => onChange(moveOverride(overrides, i, 1))} />
                <IconButton icon={X} size="sm" label={`Remove ${o.host}`} onClick={() => onChange(removeOverride(overrides, i))} />
              </span>
            </li>
          ))}
        </ul>
      )}
      <div className={own.addRow}>
        <Input
          size="sm"
          mono
          value={host}
          placeholder="Add host, e.g. *.shop.example"
          onChange={(e) => setHost(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              add();
            }
          }}
          invalid={!!error}
          aria-label="Host for a per-host TLS fingerprint"
          className={own.hostInput}
        />
        {/* Picking is free; adding a browser profile is what's gated. */}
        <ProfileSelect size="sm" value={profile} onChange={setProfile} catalog={catalog} licensed={licensed} gate={false} label="TLS fingerprint for the new host" />
        <Button size="sm" icon={Plus} onClick={add} disabled={!host.trim()}>
          Add
        </Button>
      </div>
      {error && (
        <p className={own.error} role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/** Settings → Proxy → "Upstream TLS fingerprint". */
export function TlsProfileCard({ draft }: { draft: Settings }) {
  const edit = useEdit();
  const catalog = useTlsProfiles();
  const tls = draft.proxy.tlsProfile;
  const selected = catalog?.profiles.find((p) => p.id === tls.profile);
  const wantsBrowser = tls.profile !== 'native' || tls.overrides.some((o) => o.profile !== 'native');
  const unavailable = catalog != null && !catalog.available;
  const licensed = useFeature('tls-impersonation') && catalog?.licensed !== false;
  return (
    <SettingsCard
      id="tls-fingerprint"
      title="Upstream TLS fingerprint"
      description="What Akev's own connections to servers look like. Sites behind Cloudflare, Datadome or Akamai may challenge or block requests whose TLS handshake doesn't match the browser in their User-Agent."
    >
      {!licensed && (
        <ProCallout feature="tls-impersonation" title="Browser fingerprints are part of Akev Pro">
          {wantsBrowser ? 'The browser profiles below are saved but inactive: Akev uses its native TLS stack until Pro is active.' : "Make Akev's connections look like Chrome, Firefox or Safari to servers that check TLS fingerprints."}
        </ProCallout>
      )}
      {unavailable && (
        <p className={`${styles.callout} ${styles.calloutWarning}`} role="note">
          <TriangleAlert size={13} strokeWidth={1.9} aria-hidden />
          <span>
            This build of Akev doesn't include browser profiles{wantsBrowser ? ', so the native TLS stack is used' : ''}. The official downloads from akev.dev do; if you built Akev yourself, build it with <code>--features impersonate</code> (BoringSSL; needs cmake and clang).
          </span>
        </p>
      )}
      <SettingRow
        id="tls-profile"
        title="Profile"
        description={
          <>
            {selected?.description ?? "Akev's own TLS stack (rustls)."}
            {selected?.ja4 && (
              <span className={own.fingerprints}>
                <Fingerprint size={12} strokeWidth={1.9} aria-hidden />
                <span className={own.mono}>JA4 {selected.ja4}</span>
                {selected.http2 && <span className={own.mono}>HTTP/2 {selected.http2}</span>}
              </span>
            )}
          </>
        }
        warning={!licensed && tls.profile !== 'native' ? 'Inactive without Akev Pro — the native TLS stack is used' : unavailable && tls.profile !== 'native' ? 'Requires an impersonation build — the native TLS stack is used' : undefined}
        control={<ProfileSelect value={tls.profile} onChange={(profile) => void edit(setTlsProfile({ profile }))} catalog={catalog} licensed={licensed} label="Upstream TLS fingerprint" />}
      />
      <SettingRow
        id="tls-profile-overrides"
        title="Per-host profiles"
        description={
          <>
            The first matching host wins; globs like <code>*.example.com</code> and <code>host:8443</code> work. Rules with a <em>TLS fingerprint</em> action override both. Request headers (User-Agent…) are forwarded as the client sent them.
          </>
        }
        layout="stacked"
      >
        <OverridesEditor overrides={tls.overrides} onChange={(overrides) => void edit(setTlsProfile({ overrides }))} catalog={catalog} licensed={licensed} />
        {unavailable && tls.overrides.some((o) => !profileAvailable(o.profile, catalog)) && <p className={styles.hint}>Browser profiles in this list fall back to the native TLS stack in this build.</p>}
      </SettingRow>
    </SettingsCard>
  );
}
