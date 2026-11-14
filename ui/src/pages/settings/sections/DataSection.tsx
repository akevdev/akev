import { Archive, Download, RotateCcw, Trash2, Upload } from 'lucide-react';
import { formatInterval, openSessionsDialog } from '../../../app/sessions/sessions';
import { Button, NumberInput, Switch } from '../../../components/ui';
import { formatNumber } from '../../../lib/format';
import { useAppStore } from '../../../state/app';
import { useTraffic } from '../../../state/traffic';
import { setSessions, useDraft, useEdit, useSettingsSync } from '../context';
import { clearAllTraffic, exportConfig, importConfig, resetUiPrefs, useDataBusy } from '../dataActions';
import { SettingRow, SettingsCard } from '../SettingRow';
import { bytesToMb, mbToBytes } from '../validation';
import styles from './sections.module.css';

/** Saved sessions & autosave (`settings.sessions`). */
function SessionsCard() {
  const draft = useDraft();
  const edit = useEdit();
  const dataDir = useAppStore((s) => s.status?.dataDir ?? null);
  if (!draft) return null;
  const s = draft.sessions;
  const off = !s.autosave;
  return (
    <SettingsCard id="sessions" title="Sessions & autosave" description="Keep captured traffic across restarts, and save sessions to reopen later.">
      <SettingRow
        id="autosave"
        title="Autosave traffic"
        description={s.autosave ? `Captured traffic is saved to the Autosave session every ${formatInterval(s.autosaveIntervalSecs)} while it changes, and when Akev quits.` : 'Traffic is kept in memory only and is gone when Akev quits.'}
        control={<Switch checked={s.autosave} onCheckedChange={(v) => void edit(setSessions('autosave', v))} aria-label="Autosave traffic" />}
      />
      <SettingRow
        id="restore-on-startup"
        title="Restore last session on startup"
        description="Reopen the autosaved traffic when Akev starts, so you continue where you left off."
        control={<Switch checked={s.restoreOnStartup && s.autosave} disabled={off} onCheckedChange={(v) => void edit(setSessions('restoreOnStartup', v))} aria-label="Restore last session on startup" />}
      />
      <SettingRow
        id="autosave-interval"
        title="Save every"
        description="Saving runs in the background and never slows down the proxy."
        control={<NumberInput value={s.autosaveIntervalSecs} disabled={off} onValueChange={(v) => v != null && void edit(setSessions('autosaveIntervalSecs', Math.round(v)))} min={5} max={3600} step={15} unit="s" aria-label="Autosave interval in seconds" className={styles.numberInput} />}
      />
      <SettingRow
        id="autosave-size"
        title="Keep at most"
        description="Older exchanges beyond this are left out of the autosave. Pinned exchanges are always kept. 0 = no limit."
        control={<NumberInput value={bytesToMb(s.autosaveMaxBytes)} disabled={off} onValueChange={(v) => v != null && void edit(setSessions('autosaveMaxBytes', mbToBytes(Math.max(0, v))))} min={0} max={65536} step={256} precision={0} unit="MB" aria-label="Autosave size limit in MB" className={styles.numberInput} />}
      />
      <SettingRow
        id="saved-sessions"
        title="Saved sessions"
        description={dataDir ? `Stored in ${dataDir}/sessions. Open, rename, download or delete them.` : 'Open, rename, download or delete saved sessions and autosaves.'}
        control={
          <Button icon={Archive} onClick={() => openSessionsDialog()}>
            Manage…
          </Button>
        }
      />
    </SettingsCard>
  );
}

/** Settings → Data: export/import configuration, reset UI prefs, clear traffic. */
export function DataSection() {
  const sync = useSettingsSync();
  const total = useTraffic((s) => s.order.length);
  const { importing, clearing } = useDataBusy();

  return (
    <>
      <SessionsCard />
      <SettingsCard id="config" title="Configuration" description="One JSON file with your proxy & capture settings, rules and interface preferences.">
        <SettingRow
          id="export"
          title="Export configuration"
          description="Back up your setup or move it to another machine. Client certificate passphrases are left out."
          control={
            <Button icon={Download} onClick={() => void exportConfig(sync)}>
              Export…
            </Button>
          }
        />
        <SettingRow
          id="import"
          title="Import configuration"
          description="Akev exports, plain settings or rules files, and HTTP Toolkit rule exports."
          control={
            <Button icon={Upload} loading={importing} onClick={() => void importConfig(sync)}>
              Import…
            </Button>
          }
        />
      </SettingsCard>

      <SettingsCard id="reset" title="Reset & clean up">
        <SettingRow
          id="reset-ui"
          title="Reset interface preferences"
          description="Restore the default theme, density, columns, pane sizes and saved filters. Proxy settings and rules are kept."
          control={
            <Button icon={RotateCcw} onClick={resetUiPrefs}>
              Reset
            </Button>
          }
        />
        <SettingRow
          id="clear-traffic"
          title="Clear captured traffic"
          description={total > 0 ? `${formatNumber(total)} exchanges are stored right now. Pinned exchanges are kept.` : 'No traffic is stored right now.'}
          control={
            <Button variant="subtle-danger" icon={Trash2} disabled={total === 0} loading={clearing} onClick={() => void clearAllTraffic()}>
              Clear traffic…
            </Button>
          }
        />
      </SettingsCard>
    </>
  );
}
