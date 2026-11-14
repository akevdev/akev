import { ArrowRight } from 'lucide-react';
import type { Settings, TransparentSettings } from '../../../api/types';
import { navigate } from '../../../app/router';
import { ProCallout } from '../../../components/pro';
import { Button, Input, NumberInput, Switch } from '../../../components/ui';
import { useFeature } from '../../../state/license';
import { formatMark, listenerSummary, parseMark, TRANSPARENT_ID, validateMarkText, withTransparent } from '../../connect/transparent';
import { useTransparentInfo } from '../../connect/useTransparent';
import { useEdit } from '../context';
import { SettingRow, SettingsCard } from '../SettingRow';
import { useTextField } from '../useTextField';
import styles from './sections.module.css';

const patch =
  (p: Partial<TransparentSettings>) =>
  (d: Settings): Settings =>
    withTransparent(d, p);

/** Settings → Proxy → Transparent capture (Linux): listener on/off, port, upstream socket mark. */
export function TransparentCard({ draft }: { draft: Settings }) {
  const edit = useEdit();
  const t = draft.proxy.transparent;
  const { info } = useTransparentInfo(3000);
  const listener = info?.listener ?? null;
  const summary = listenerSummary(listener);
  const mark = useTextField({
    value: formatMark(t.mark),
    validate: validateMarkText,
    onCommit: (text) => {
      const m = parseMark(text);
      if (m != null && m !== t.mark) void edit(patch({ mark: m }));
    },
  });
  const portConflict = t.enabled && t.port !== 0 && t.port === draft.proxy.port;
  const unsupported = info != null && !info.supported;
  const licensed = useFeature('transparent');

  return (
    <SettingsCard
      id="transparent"
      title="Transparent capture (Linux)"
      description="A second port for connections the firewall redirects to Akev — for apps that ignore proxy settings. Akev never changes firewall rules itself."
    >
      {!licensed && !unsupported && <ProCallout feature="transparent">{t.enabled ? 'The transparent listener is paused until Pro is active. Your settings are kept.' : undefined}</ProCallout>}
      <SettingRow
        id="transparent"
        title="Transparent listener"
        description={unsupported ? (info?.unsupportedReason ?? 'Not supported on this system.') : t.enabled ? `${summary.title}.${listener?.addresses.length ? ` Listening on ${listener.addresses.join(', ')}.` : ''}` : 'Off: the main proxy port keeps working as usual.'}
        error={t.enabled && listener?.error ? listener.error : undefined}
        // Turning it off always works; turning it on needs Pro.
        control={<Switch checked={t.enabled} disabled={!licensed && !t.enabled} onCheckedChange={(v) => void edit(patch({ enabled: v }))} aria-label="Transparent listener" />}
      />
      <SettingRow
        id="transparent-port"
        title="Transparent port"
        description={listener?.port != null && listener.port !== t.port && t.port !== 0 ? `Port ${t.port} is busy, so it listens on ${listener.port}: generate the commands again.` : 'Where the redirect rules send connections. Separate from the proxy port, which keeps explicit-proxy behaviour.'}
        error={portConflict ? 'Must differ from the proxy port' : undefined}
        control={<NumberInput value={t.port} onValueChange={(v) => v != null && v !== t.port && void edit(patch({ port: v }))} min={0} max={65535} aria-label="Transparent port" className={styles.numberInput} />}
      />
      <SettingRow
        id="transparent-mark"
        title="Firewall mark"
        description="Marks Akev's own upstream connections (SO_MARK) so machine-wide rules can skip them. Needs CAP_NET_ADMIN. 0 = off."
        error={mark.error ?? (t.mark !== 0 && listener?.markError ? listener.markError : undefined)}
        control={<Input {...mark.inputProps} mono placeholder="0x1a4b" aria-label="Firewall mark" className={styles.numberInput} />}
      />
      <SettingRow
        id="transparent-setup"
        title="Redirect rules"
        description="Generate the commands for one app, a user account or the whole machine — and check that redirected connections arrive."
        control={
          <Button size="sm" iconRight={ArrowRight} onClick={() => navigate(`/connect/${TRANSPARENT_ID}`)}>
            Set up
          </Button>
        }
      />
    </SettingsCard>
  );
}
