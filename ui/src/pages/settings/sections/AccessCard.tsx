import type { ProxyAccessSettings, Settings } from '../../../api/types';
import { Input, SegmentedControl, Switch } from '../../../components/ui';
import { setProxy, useEdit, useSettingsSync } from '../context';
import { SettingRow, SettingsCard } from '../SettingRow';
import { StringListEditor } from '../StringListEditor';
import { useTextField } from '../useTextField';
import { validateIpAddress } from '../validation';
import styles from './sections.module.css';

type Policy = ProxyAccessSettings['policy'];

const POLICY_HELP: Record<Policy, string> = {
  private: 'This computer and devices on private networks (home and office LANs, Docker, Tailscale) can connect.',
  allowlist: 'Only this computer and the devices listed below can connect.',
  any: 'Anyone who can reach the port can use the proxy. Set a username and password below.',
};

/** An IP address or CIDR range (`192.168.1.23`, `10.0.0.0/8`, `fd00::/8`). */
export function validateIpRange(raw: string): string | null {
  const s = raw.trim();
  if (!s) return 'Enter an IP address or range';
  const [addr = '', prefix, extra] = s.split('/');
  if (extra !== undefined) return 'Use one address and one prefix, e.g. 10.0.0.0/8';
  if (validateIpAddress(addr) !== null) return 'Enter an IP address or range, e.g. 192.168.1.23 or 10.0.0.0/8';
  if (prefix === undefined) return null;
  const max = addr.includes(':') ? 128 : 32;
  const n = Number(prefix);
  return /^\d+$/.test(prefix) && n <= max ? null : `The prefix length must be 0–${max}`;
}

function validateUsername(raw: string): string | null {
  return raw.includes(':') ? "The username can't contain “:”" : null;
}

/** Settings → Proxy → Who can connect: client policy, allow / block lists, credentials. */
export function AccessCard({ draft }: { draft: Settings }) {
  const edit = useEdit();
  const sync = useSettingsSync();
  const { access } = draft.proxy;
  const update = (patch: Partial<ProxyAccessSettings>, debounce = false) => void edit(setProxy('access', { ...access, ...patch }), { debounce });

  const username = useTextField({
    value: access.username,
    validate: validateUsername,
    onCommit: (t) => update({ username: t.trim() }, true),
    onSettle: () => void sync.flush(),
  });
  const password = useTextField({
    value: access.password,
    onCommit: (t) => update({ password: t }, true),
    onSettle: () => void sync.flush(),
  });

  return (
    <SettingsCard id="access" title="Who can connect" description="Matters when the proxy listens on your network. Refused devices are disconnected without an answer.">
      <SettingRow
        id="access-policy"
        title="Devices allowed"
        description={POLICY_HELP[access.policy]}
        control={
          <SegmentedControl<Policy>
            value={access.policy}
            onValueChange={(policy) => update({ policy })}
            items={[
              { value: 'private', label: 'Private networks' },
              { value: 'allowlist', label: 'Only listed' },
              { value: 'any', label: 'Anyone' },
            ]}
            aria-label="Devices allowed"
          />
        }
      />
      {access.policy === 'allowlist' && (
        <SettingRow id="access-allow" title="Allowed devices" description="IP addresses or ranges, e.g. 192.168.1.23 or 192.168.1.0/24." layout="stacked">
          <StringListEditor
            items={access.allow}
            onChange={(allow) => update({ allow: [...allow] })}
            validate={validateIpRange}
            placeholder="Add address, e.g. 192.168.1.23"
            noun="address"
            emptyText="Only this computer can connect."
            aria-label="Allowed devices"
          />
        </SettingRow>
      )}
      <SettingRow id="access-block" title="Blocked devices" description="Always refused, whatever the setting above. This computer is never blocked." layout="stacked">
        <StringListEditor
          items={access.block}
          onChange={(block) => update({ block: [...block] })}
          validate={validateIpRange}
          placeholder="Add address, e.g. 192.168.1.50"
          noun="address"
          emptyText="No devices are blocked."
          aria-label="Blocked devices"
        />
      </SettingRow>
      <SettingRow
        id="access-credentials"
        title="Username and password"
        description="When set, devices on other machines must log in to use the proxy (HTTP proxy authentication or SOCKS5). This computer, WireGuard and transparent capture need no login."
        error={username.error ?? undefined}
        control={
          <div className={styles.inlineGroup}>
            <Input {...username.inputProps} placeholder="No login" autoComplete="off" aria-label="Proxy username" className={styles.textInput} />
            <Input {...password.inputProps} type="password" placeholder="Password" autoComplete="new-password" disabled={!access.username} aria-label="Proxy password" className={styles.textInput} />
          </div>
        }
      />
      <SettingRow
        id="access-loopback"
        title="Let devices reach this computer's localhost"
        description="Other devices can then use the proxy to open services that only listen on this computer (dev servers, databases). Rules that forward to localhost work either way."
        control={<Switch checked={access.allowLoopbackTargets} onCheckedChange={(v) => update({ allowLoopbackTargets: v })} aria-label="Let devices reach this computer's localhost" />}
      />
      <SettingRow
        id="access-proxy-protocol"
        title="Trust PROXY protocol headers"
        description="Only when a load balancer that adds them sits in front of Akev: the address it declares is then treated as the client."
        control={<Switch checked={access.trustProxyProtocol} onCheckedChange={(v) => update({ trustProxyProtocol: v })} aria-label="Trust PROXY protocol headers" />}
      />
    </SettingsCard>
  );
}
