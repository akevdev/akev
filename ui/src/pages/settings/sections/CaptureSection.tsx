import { NumberInput, ProgressBar, Switch } from '../../../components/ui';
import { formatBytes, formatNumber } from '../../../lib/format';
import { useAppStore } from '../../../state/app';
import { setCapture, useDraft, useEdit } from '../context';
import { SettingRow, SettingsCard } from '../SettingRow';
import { StringListEditor } from '../StringListEditor';
import { bytesToMb, mbToBytes, normalizeHost, validateHostPattern } from '../validation';
import { LimitRows } from './LimitRows';
import styles from './sections.module.css';

function MemoryMeter({ budget }: { budget: number }) {
  const stats = useAppStore((s) => s.stats);
  if (!stats) return null;
  const used = stats.memoryBodyBytes;
  const ratio = budget > 0 ? used / budget : 0;
  return (
    <div className={styles.meter}>
      <ProgressBar value={Math.min(1, ratio)} variant={ratio > 0.9 ? 'warning' : 'accent'} size="sm" aria-label="Memory used by bodies" />
      <div className={styles.meterLabels}>
        <span className="tnum">
          {formatBytes(used)} of {formatBytes(budget)} in memory
        </span>
        {stats.spilledBodyBytes > 0 && <span className="tnum">{formatBytes(stats.spilledBodyBytes)} on disk</span>}
      </div>
    </div>
  );
}

/** Settings → Capture. */
export function CaptureSection() {
  const draft = useDraft();
  const edit = useEdit();
  const exchanges = useAppStore((s) => s.stats?.exchanges ?? null);
  if (!draft) return null;
  const c = draft.capture;
  return (
    <>
      <SettingsCard id="limits" title="Limits" description="Keep long sessions fast. Limits apply to new traffic.">
        <SettingRow
          id="max-body"
          title="Max body size"
          description="Larger bodies are cut off at this size; the exchange is still recorded and marked truncated."
          control={<NumberInput value={bytesToMb(c.maxBodyBytes)} onValueChange={(v) => v != null && void edit(setCapture('maxBodyBytes', mbToBytes(v)))} min={0.1} max={4096} step={1} precision={1} unit="MB" aria-label="Max body size in MB" className={styles.numberInput} />}
        />
        <SettingRow
          id="memory"
          title="Memory budget"
          description="Bodies beyond this are moved to disk, so memory stays flat however much you capture."
          control={<NumberInput value={bytesToMb(c.memoryBudgetBytes)} onValueChange={(v) => v != null && void edit(setCapture('memoryBudgetBytes', mbToBytes(v)))} min={16} max={65536} step={64} precision={0} unit="MB" aria-label="Memory budget in MB" className={styles.numberInput} />}
        >
          <MemoryMeter budget={c.memoryBudgetBytes} />
        </SettingRow>
        <SettingRow
          id="max-exchanges"
          title="Max exchanges"
          description={c.maxExchanges === 0 ? `Unlimited. ${exchanges != null ? `${formatNumber(exchanges)} captured right now.` : ''}` : 'The oldest unpinned exchanges are dropped beyond this. 0 = unlimited.'}
          control={<NumberInput value={c.maxExchanges} onValueChange={(v) => void edit(setCapture('maxExchanges', Math.max(0, Math.round(v ?? 0))))} min={0} max={10_000_000} step={1000} aria-label="Max exchanges (0 = unlimited)" className={styles.numberInput} />}
        />
        <LimitRows capture={c} />
      </SettingsCard>

      <SettingsCard id="recording" title="What to record">
        <SettingRow id="record-tunnels" title="Record tunnels" description="Show connections that weren't decrypted (TLS passthrough, non-HTTP protocols) as tunnel rows." control={<Switch checked={c.recordTunnels} onCheckedChange={(v) => void edit(setCapture('recordTunnels', v))} aria-label="Record tunnels" />} />
        <SettingRow id="record-tls-failures" title="Record TLS failures" description="Show handshakes that failed — usually a client that doesn't trust the Akev CA or pins certificates." control={<Switch checked={c.recordTlsFailures} onCheckedChange={(v) => void edit(setCapture('recordTlsFailures', v))} aria-label="Record TLS failures" />} />
        <SettingRow id="detect-processes" title="Detect processes" description="Identify which local app sent each request (shown in the Source column)." control={<Switch checked={c.detectProcesses} onCheckedChange={(v) => void edit(setCapture('detectProcesses', v))} aria-label="Detect processes" />} />
      </SettingsCard>

      <SettingsCard id="ignored" title="Ignored hosts" description="Traffic to these hosts is proxied and rules still apply, but it isn't recorded — perfect for telemetry noise.">
        <SettingRow id="ignore-hosts" title="Hosts" description="Globs work: *.telemetry.mozilla.org, *.sentry.io." layout="stacked">
          <StringListEditor
            items={c.ignoreHosts}
            onChange={(items) => void edit(setCapture('ignoreHosts', items))}
            validate={validateHostPattern}
            normalize={normalizeHost}
            placeholder="Add host, e.g. *.analytics.example.com"
            noun="host"
            emptyText="Everything is recorded."
            aria-label="Ignored hosts"
          />
        </SettingRow>
      </SettingsCard>
    </>
  );
}
