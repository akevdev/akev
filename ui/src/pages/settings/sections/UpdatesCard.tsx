import { Download, RefreshCw } from 'lucide-react';
import { Button, Switch } from '../../../components/ui';
import { formatRelativeTime } from '../../../lib/format';
import { tauriBridge } from '../../../lib/platform';
import { useUpdate } from '../../../state/update';
import { setUpdates, useDraft, useEdit } from '../context';
import { SettingRow, SettingsCard } from '../SettingRow';
import styles from './sections.module.css';

/** Settings → About → Updates. */
export function UpdatesCard() {
  const draft = useDraft();
  const edit = useEdit();
  const info = useUpdate((s) => s.info);
  const checking = useUpdate((s) => s.checking);
  const installing = useUpdate((s) => s.installing);
  const { checkNow, install } = useUpdate.getState();
  const desktop = tauriBridge()?.installUpdate != null;

  const state = info?.error
    ? `Couldn't check: ${info.error}`
    : info?.available && info.latest
      ? `Akev ${info.latest} is available (you have ${info.current}).`
      : info?.checkedAt
        ? `Akev ${info.current} is the latest version · checked ${formatRelativeTime(info.checkedAt)}`
        : 'Not checked yet.';

  return (
    <SettingsCard id="updates" title="Updates" description="Akev only contacts the release server to look for a new version; nothing about you or your traffic is sent.">
      <SettingRow
        id="update-status"
        title="Version"
        description={state}
        control={
          info?.available ? (
            <Button size="sm" variant="primary" icon={Download} loading={installing} onClick={() => void install()}>
              {desktop ? 'Update and restart' : 'Download'}
            </Button>
          ) : (
            <Button size="sm" icon={RefreshCw} loading={checking} onClick={() => void checkNow()}>
              Check now
            </Button>
          )
        }
      />
      {draft && (
        <SettingRow
          id="update-automatic"
          title="Check for updates automatically"
          description="Once a day, while Akev is open. Updates are never installed without you."
          control={
            <Switch checked={draft.updates.checkAutomatically} onCheckedChange={(v) => void edit(setUpdates('checkAutomatically', v))} aria-label="Check for updates automatically" />
          }
        />
      )}
      {info?.available && info.notes && (
        <SettingRow title="What's new" layout="stacked">
          <pre className={styles.releaseNotes}>{info.notes}</pre>
        </SettingRow>
      )}
    </SettingsCard>
  );
}
