import type { Settings } from '../../../api/types';
import { NumberInput } from '../../../components/ui';
import { setBreakpoints, setProxy, useEdit } from '../context';
import { SettingRow, SettingsCard } from '../SettingRow';
import styles from './sections.module.css';

function secondsOf(ms: number): number {
  return Math.round(ms / 100) / 10;
}

/** Settings → Proxy → Timeouts: how long a response body may stall. */
export function BodyIdleTimeoutRow({ draft }: { draft: Settings }) {
  const edit = useEdit();
  const { bodyIdleTimeoutMs } = draft.proxy;
  return (
    <SettingRow
      id="body-idle-timeout"
      title="Body idle timeout"
      description={
        bodyIdleTimeoutMs === 0
          ? 'No limit — a response body may pause for as long as the server keeps the connection open.'
          : 'Fail the exchange when a response body sends nothing for this long. Streams that keep sending are never cut off. 0 = no limit.'
      }
      control={<NumberInput value={secondsOf(bodyIdleTimeoutMs)} onValueChange={(v) => void edit(setProxy('bodyIdleTimeoutMs', Math.round((v ?? 0) * 1000)))} min={0} max={86_400} step={30} precision={0} unit="s" aria-label="Body idle timeout in seconds" className={styles.numberInput} />}
    />
  );
}

/** Settings → Proxy → Breakpoints: how many exchanges may wait at once, and for how long. */
export function BreakpointsCard({ draft }: { draft: Settings }) {
  const edit = useEdit();
  const b = draft.breakpoints;
  return (
    <SettingsCard id="breakpoints" title="Breakpoints" description="Exchanges paused by breakpoint rules wait for you. These limits keep a forgotten breakpoint from piling up requests.">
      <SettingRow
        id="max-paused"
        title="Max paused at once"
        description={b.maxPaused === 0 ? 'No limit.' : 'Further matches continue without pausing, and each says why. 0 = no limit.'}
        control={<NumberInput value={b.maxPaused} onValueChange={(v) => void edit(setBreakpoints('maxPaused', Math.max(0, Math.round(v ?? 0))))} min={0} max={100_000} step={10} aria-label="Max paused exchanges (0 = no limit)" className={styles.numberInput} />}
      />
      <SettingRow
        id="auto-release"
        title="Continue automatically after"
        description={b.autoReleaseSecs === 0 ? 'Off — paused exchanges wait until you continue them.' : 'Paused exchanges continue unchanged after this long (each says why). 0 = wait for you.'}
        control={<NumberInput value={b.autoReleaseSecs} onValueChange={(v) => void edit(setBreakpoints('autoReleaseSecs', Math.max(0, Math.round(v ?? 0))))} min={0} max={86_400} step={30} unit="s" aria-label="Continue paused exchanges automatically after seconds (0 = off)" className={styles.numberInput} />}
      />
    </SettingsCard>
  );
}
