import { NumberInput } from '../../../components/ui';
import type { CaptureSettings } from '../../../api/types';
import { useAppStore } from '../../../state/app';
import { setCapture, setProxy, useDraft, useEdit } from '../context';
import { bytesToGb, gbToBytes, limitCounters, MIN_EVENT_QUEUE_BYTES, spillOnDisk } from '../limits';
import { SettingRow } from '../SettingRow';
import { bytesToMb, mbToBytes } from '../validation';
import styles from './sections.module.css';

/** Settings → Capture → Limits: rule body memory, spill disk space, capture queue, and what the limits did. */
export function LimitRows({ capture }: { capture: CaptureSettings }) {
  const draft = useDraft();
  const edit = useEdit();
  const stats = useAppStore((s) => s.stats);
  if (!draft) return null;
  const { spillMaxBytes, eventQueueBytes } = capture;
  const { ruleBodyBudgetBytes } = draft.proxy;
  const counters = limitCounters(stats);
  const disk = spillOnDisk(stats);
  return (
    <>
      <SettingRow
        id="rule-body-budget"
        title="Memory for rule bodies"
        description="Shared by every exchange whose body a rule, script or breakpoint needs in full. When it's used up, further bodies pass through untouched and the exchange says why."
        control={<NumberInput value={bytesToMb(ruleBodyBudgetBytes)} onValueChange={(v) => v != null && void edit(setProxy('ruleBodyBudgetBytes', mbToBytes(v)))} min={16} max={65536} step={64} precision={0} unit="MB" aria-label="Memory for rule bodies in MB" className={styles.numberInput} />}
      />
      <SettingRow
        id="spill-max"
        title="Disk space for bodies"
        description={spillMaxBytes === 0 ? 'Unlimited. Bodies moved to disk are kept until their exchanges are removed.' : 'Bodies moved to disk may use this much space. Past it the oldest are deleted; their exchanges stay, marked. 0 = unlimited.'}
        control={<NumberInput value={bytesToGb(spillMaxBytes)} onValueChange={(v) => void edit(setCapture('spillMaxBytes', gbToBytes(Math.max(0, v ?? 0))))} min={0} max={4096} step={1} precision={1} unit="GB" aria-label="Disk space for bodies in GB (0 = unlimited)" className={styles.numberInput} />}
      >
        {disk && <p className={styles.hint}>{disk}</p>}
      </SettingRow>
      <SettingRow
        id="event-queue"
        title="Capture queue"
        description="Traffic waiting to be stored. When storing falls this far behind, new requests wait briefly, then bodies are dropped instead of using more memory."
        control={<NumberInput value={bytesToMb(eventQueueBytes)} onValueChange={(v) => v != null && void edit(setCapture('eventQueueBytes', Math.max(MIN_EVENT_QUEUE_BYTES, mbToBytes(v))))} min={1} max={4096} step={16} precision={0} unit="MB" aria-label="Capture queue in MB" className={styles.numberInput} />}
      />
      {counters.length > 0 && (
        <SettingRow id="limit-counters" title="Limits reached since Akev started" layout="stacked">
          <ul className={styles.hint}>
            {counters.map((c) => (
              <li key={c}>{c}</li>
            ))}
          </ul>
        </SettingRow>
      )}
    </>
  );
}
