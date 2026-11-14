import { Download, Wifi } from 'lucide-react';
import { useMemo } from 'react';
import { api } from '../../../api/client';
import type { TerminalShell } from '../../../api/types';
import { AppIcon, Button, CopyButton, SegmentedControl, TabList, Tabs, TabsContent, Tooltip } from '../../../components/ui';
import { downloadUrl } from '../../../lib/platform';
import { useLastTab, usePagePref } from '../../../state/ui';
import { proxyUrl } from '../addresses';
import { GuideView } from '../GuideView';
import { caFilePath, defaultShell, manualGuides } from '../snippets';
import { useLanToggle } from '../useProxyInfo';
import { PanelSection } from './kit';
import type { PanelProps } from './types';
import styles from './panels.module.css';

type Flavor = 'posix' | 'powershell' | 'cmd';

const FLAVORS: { value: Flavor; label: string; shell: TerminalShell }[] = [
  { value: 'posix', label: 'bash / zsh', shell: 'bash' },
  { value: 'powershell', label: 'PowerShell', shell: 'powershell' },
  { value: 'cmd', label: 'cmd', shell: 'cmd' },
];

function StepTitle({ n, children }: { n: number; children: string }) {
  return (
    <span className={styles.stepHead}>
      <span className={styles.stepNum}>{n}</span>
      {children}
    </span>
  );
}

/** Manual setup: proxy addresses, CA download, per-tool instructions with copyable snippets. */
export function ManualPanel({ meta, proxy }: PanelProps) {
  const lan = useLanToggle();
  const platformFlavor: Flavor = proxy.platform === 'windows' ? 'powershell' : 'posix';
  const [storedFlavor, setFlavor] = usePagePref<Flavor>('connect.manualShell', platformFlavor);
  const flavor = FLAVORS.some((f) => f.value === storedFlavor) ? storedFlavor : platformFlavor;
  const shell = flavor === 'posix' ? (defaultShell(proxy.platform) === 'zsh' ? 'zsh' : 'bash') : (FLAVORS.find((f) => f.value === flavor)?.shell ?? 'bash');
  const [tool, setTool] = useLastTab('connect.manualTool', 'env');

  const local = proxy.local ?? '127.0.0.1:8000';
  const guides = useMemo(
    () =>
      manualGuides({
        proxy: local,
        lanProxy: proxy.primaryLan,
        shell,
        caPath: caFilePath(shell, meta.caPath),
        providedCaPath: meta.caPath,
        bundlePath: meta.bundlePath,
        envVars: meta.envVars,
        setupUrl: proxy.setupUrl,
      }),
    [local, proxy.primaryLan, shell, meta.caPath, meta.bundlePath, meta.envVars, proxy.setupUrl],
  );
  const guide = guides.find((g) => g.id === tool) ?? guides[0];
  const shellAware = guide && !['ios', 'android'].includes(guide.id);
  const lanAddress = proxy.lan[0];

  return (
    <>
      <PanelSection title={<StepTitle n={1}>Send traffic through the proxy</StepTitle>}>
        <div className={styles.addrList}>
          <div className={styles.addrRow}>
            <span className={styles.addrLabel}>Local</span>
            <span className={`${styles.addr} ${styles.grow}`}>{proxyUrl(local)}</span>
            <CopyButton value={proxyUrl(local)} label="Copy proxy URL" toastMessage="Proxy URL copied" />
          </div>
          {lanAddress && (
            <div className={styles.addrRow}>
              <span className={styles.addrLabel}>Network</span>
              <Tooltip content={lan.checked ? undefined : 'Turn on “Allow other devices” to use this address'}>
                <span className={`${styles.addr} ${styles.grow} ${lan.checked ? '' : styles.addrMuted}`} tabIndex={lan.checked ? undefined : 0}>
                  {proxyUrl(lanAddress)}
                </span>
              </Tooltip>
              {lan.checked ? (
                <CopyButton value={proxyUrl(lanAddress)} label="Copy network proxy URL" toastMessage="Network proxy URL copied" />
              ) : (
                <Button size="sm" variant="ghost" icon={Wifi} loading={lan.pending} disabled={lan.disabled} onClick={() => lan.toggle(true)}>
                  Allow
                </Button>
              )}
            </div>
          )}
          {meta.pacUrl && (
            <div className={styles.addrRow}>
              <Tooltip content="Proxy auto-config script, for tools that only take a PAC URL">
                {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip */}
                <span className={styles.addrLabel} tabIndex={0}>
                  PAC
                </span>
              </Tooltip>
              <span className={`${styles.addr} ${styles.grow}`}>{meta.pacUrl}</span>
              <CopyButton value={meta.pacUrl} label="Copy PAC URL" toastMessage="PAC URL copied" />
            </div>
          )}
        </div>
      </PanelSection>

      <PanelSection
        title={<StepTitle n={2}>Trust the Akev certificate</StepTitle>}
        description={meta.caPath ? 'Clients must trust the Akev CA to accept HTTPS responses. Point them at the file below, or download a copy.' : 'Clients must trust the Akev CA to accept HTTPS responses. Save it where your tool can read it.'}
      >
        {meta.caPath && (
          <div className={styles.addrList}>
            <div className={styles.addrRow}>
              <span className={styles.addrLabel}>PEM file</span>
              <span className={`${styles.addr} ${styles.grow}`} title={meta.caPath}>
                {meta.caPath}
              </span>
              <CopyButton value={meta.caPath} label="Copy certificate path" toastMessage="Certificate path copied" />
            </div>
            {meta.bundlePath && (
              <div className={styles.addrRow}>
                <Tooltip content="Your system's trusted roots plus the Akev CA — for SSL_CERT_FILE and similar variables that replace the trusted roots">
                  {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip */}
                  <span className={styles.addrLabel} tabIndex={0}>
                    Bundle
                  </span>
                </Tooltip>
                <span className={`${styles.addr} ${styles.grow}`} title={meta.bundlePath}>
                  {meta.bundlePath}
                </span>
                <CopyButton value={meta.bundlePath} label="Copy bundle path" toastMessage="Bundle path copied" />
              </div>
            )}
          </div>
        )}
        <div className={styles.buttons}>
          <Button size="sm" icon={Download} onClick={() => void downloadUrl(api.ca.pemUrl(), 'akev-ca.pem')}>
            Download PEM
          </Button>
          <Button size="sm" icon={Download} onClick={() => void downloadUrl(api.ca.derUrl(), 'akev-ca.crt')}>
            Download DER (.crt)
          </Button>
        </div>
      </PanelSection>

      <PanelSection title={<StepTitle n={3}>Configure your tool</StepTitle>}>
        <Tabs value={guide?.id ?? 'env'} onValueChange={setTool}>
          <TabList
            size="sm"
            aria-label="Tool"
            className={styles.toolTabs}
            items={guides.map((g) => ({ value: g.id, label: <span className={styles.row}><AppIcon name={g.icon} size={13} />{g.label}</span> }))}
          />
          {guides.map((g) => (
            <TabsContent key={g.id} value={g.id} className={styles.toolBody}>
              <GuideView
                guide={g}
                header={
                  shellAware ? (
                    <div className={styles.shellPicker}>
                      <span className={styles.shellLabel}>Snippets for</span>
                      <SegmentedControl<Flavor> value={flavor} onValueChange={setFlavor} items={FLAVORS.map((f) => ({ value: f.value, label: f.label }))} size="sm" aria-label="Shell" />
                    </div>
                  ) : undefined
                }
              />
            </TabsContent>
          ))}
        </Tabs>
      </PanelSection>
    </>
  );
}
