import { BookOpen, Bug, ClipboardCopy, CodeXml, ExternalLink, Globe } from 'lucide-react';
import { useState } from 'react';
import packages from 'virtual:licenses';
import { api, errorMessage } from '../../../api/client';
import type { Diagnostics } from '../../../api/types';
import { Logo } from '../../../app/Logo';
import { navigate } from '../../../app/router';
import { Badge, Button, CopyButton, toast } from '../../../components/ui';
import { formatDiagnostics, issueUrl } from '../../../lib/diagnostics';
import { formatNumber, formatRelativeTime } from '../../../lib/format';
import { licenseeName, planLabel } from '../../../lib/license';
import { copyText, isDesktop, openExternal } from '../../../lib/platform';
import { useAppStore } from '../../../state/app';
import { useLicense } from '../../../state/license';
import { PLATFORM_NAME } from '../../connect/catalog';
import { SettingRow, SettingsCard } from '../SettingRow';
import styles from './sections.module.css';
import { UpdatesCard } from './UpdatesCard';

const HOMEPAGE = 'https://akev.dev';
const REPOSITORY = 'https://github.com/akevdev/akev';

/** Artwork that isn't an npm package (the npm packages come from `virtual:licenses`). */
const ARTWORK: readonly { name: string; license: string }[] = [
  { name: 'Firefox logo (Mozilla Protocol assets)', license: 'MPL-2.0' },
  { name: 'Brave logo (Brave Software)', license: 'MPL-2.0' },
  { name: 'Microsoft Edge logo (Microsoft)', license: 'MIT' },
  { name: 'Vivaldi logo (Vivaldi Technologies, scaled)', license: 'CC-BY-4.0' },
  { name: 'Tux (Larry Ewing, Simon Budig, Garrett LeSage)', license: 'Attribution' },
  { name: 'WireGuard logo (trace of the official mark)', license: 'Public domain' },
];

function ValueWithCopy({ value, label }: { value: string; label: string }) {
  return (
    <span className={styles.inlineGroup}>
      <span className={styles.addr}>{value}</span>
      <CopyButton value={value} label={label} toastMessage={`${label.replace(/^Copy /, '').replace(/^./, (c) => c.toUpperCase())} copied`} />
    </span>
  );
}

/** "Copy diagnostics" and "Report a problem" (a pre-filled GitHub issue the user reviews before submitting). */
function DiagnosticsRows() {
  const [busy, setBusy] = useState<'copy' | 'report' | null>(null);
  const fetchDiagnostics = async (): Promise<Diagnostics | null> => {
    try {
      return await api.diagnostics();
    } catch (e) {
      toast.error(`Couldn't collect diagnostics: ${errorMessage(e)}`);
      return null;
    }
  };
  const copy = async () => {
    setBusy('copy');
    try {
      const d = await fetchDiagnostics();
      if (!d) return;
      if (await copyText(formatDiagnostics(d))) toast.success('Diagnostics copied — paste them into your report');
      else toast.error("Couldn't copy to the clipboard");
    } finally {
      setBusy(null);
    }
  };
  const report = async () => {
    setBusy('report');
    try {
      const d = await fetchDiagnostics();
      // Without diagnostics (they couldn't be collected) the form still opens, just not pre-filled.
      openExternal(d ? issueUrl(d) : `${REPOSITORY}/issues/new?template=bug.yml`);
    } finally {
      setBusy(null);
    }
  };
  return (
    <>
      <SettingRow
        id="copy-diagnostics"
        title="Copy diagnostics"
        description="Version, operating system, settings summary and recent warnings. No tokens and no captured traffic."
        control={
          <Button icon={ClipboardCopy} loading={busy === 'copy'} onClick={() => void copy()}>
            Copy diagnostics
          </Button>
        }
      />
      <SettingRow
        id="report-problem"
        title="Report a problem"
        description="Opens a new GitHub issue filled in with the diagnostics. You can review and edit everything before submitting."
        control={
          <Button icon={Bug} iconRight={ExternalLink} loading={busy === 'report'} onClick={() => void report()}>
            Report a problem
          </Button>
        }
      />
    </>
  );
}

/** Settings → About. */
export function AboutSection() {
  const version = useAppStore((s) => s.status?.version ?? null);
  const platform = useAppStore((s) => s.status?.platform ?? null);
  const dataDir = useAppStore((s) => s.status?.dataDir ?? null);
  const apiUrl = useAppStore((s) => s.status?.apiUrl ?? null);
  const startedAt = useAppStore((s) => s.status?.startedAt ?? null);
  const desktop = useAppStore((s) => s.status?.desktop ?? false) || isDesktop();
  const mock = useAppStore((s) => s.mock);
  const status = useLicense();
  const plan = planLabel(status);
  const license = status?.license ?? null;

  return (
    <>
      <div className={styles.hero} id="setting-version" data-setting="version">
        <span className={styles.heroMark}>
          <Logo size={34} />
        </span>
        <div className={styles.heroText}>
          <div className={styles.heroTitle}>
            Akev
            {version && (
              <Badge variant="accent" mono>
                v{version}
              </Badge>
            )}
            {plan && (
              <button type="button" className={styles.planButton} onClick={() => navigate('/settings/license')} title="Settings → License">
                <Badge variant={plan === 'Free' ? 'neutral' : 'accent'} dot={plan !== 'Free'}>
                  {plan}
                </Badge>
              </button>
            )}
            {mock && <Badge variant="warning">Mock data</Badge>}
          </div>
          <div className={styles.heroSub}>Fast, open-source HTTP(S) debugging proxy · AGPL-3.0</div>
          {license && (
            <div className={styles.heroLicensee}>
              Licensed to <span className={styles.heroLicenseeName}>{licenseeName(license)}</span>
              {license.name?.trim() && <span> · {license.email}</span>}
            </div>
          )}
        </div>
        <div className={styles.links}>
          <Button size="sm" icon={Globe} onClick={() => openExternal(HOMEPAGE)}>
            Website
          </Button>
          <Button size="sm" icon={CodeXml} onClick={() => openExternal(REPOSITORY)}>
            Source
          </Button>
        </div>
      </div>

      <UpdatesCard />

      <SettingsCard id="environment" title="Environment">
        <SettingRow title="Platform" control={<span className={styles.muted}>{platform ? `${PLATFORM_NAME[platform]} · ${desktop ? 'Desktop app' : 'Browser'}` : '—'}</span>} />
        <SettingRow id="data-dir" title="Data directory" description="Settings, rules, the CA and spilled bodies live here." control={dataDir ? <ValueWithCopy value={dataDir} label="Copy data directory" /> : '—'} />
        <SettingRow title="API" description="Also serves the MCP endpoint for AI agents at /mcp." control={apiUrl ? <ValueWithCopy value={apiUrl} label="Copy API URL" /> : '—'} />
        <SettingRow title="Running since" control={<span className={`${styles.muted} tnum`}>{startedAt ? `${new Date(startedAt).toLocaleString()} · ${formatRelativeTime(startedAt)}` : '—'}</span>} />
      </SettingsCard>

      <SettingsCard id="help" title="Help & feedback">
        <SettingRow
          title="Documentation"
          description="Guides for every interceptor, the filter language, rules and the API."
          control={
            <Button icon={BookOpen} iconRight={ExternalLink} onClick={() => openExternal(HOMEPAGE)}>
              Open docs
            </Button>
          }
        />
        <DiagnosticsRows />
      </SettingsCard>

      <SettingsCard id="licenses" title="Open-source licenses" description={`Akev's web interface is built on these ${formatNumber(packages.reduce((n, p) => n + p.packages, 0))} open-source packages and artwork.`}>
        <div id="setting-licenses" data-setting="licenses">
          <ul className={styles.licenses}>
            {packages.map((p) => (
              <li key={p.name} className={styles.license}>
                <span className={styles.licenseName} title={p.packages > 1 ? `${p.name} · ${p.packages} packages` : p.name}>
                  {p.name}
                  {p.packages > 1 && <span className={styles.licenseCount}> · {p.packages} packages</span>}
                </span>
                <span className={styles.licenseKind}>{p.license}</span>
              </li>
            ))}
            {ARTWORK.map((l) => (
              <li key={l.name} className={styles.license}>
                <span className={styles.licenseName}>{l.name}</span>
                <span className={styles.licenseKind}>{l.license}</span>
              </li>
            ))}
          </ul>
          <p className={styles.trademarks}>Product names and logos are trademarks of their owners; used to identify compatible software.</p>
        </div>
      </SettingsCard>
    </>
  );
}
