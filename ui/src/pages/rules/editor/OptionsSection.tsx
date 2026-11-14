import { NumberInput, SegmentedControl, TextArea } from '../../../components/ui';
import { Block, FieldError, useIssue } from './context';
import styles from './Editor.module.css';

type Limit = 'always' | 'once' | 'n';

export function OptionsSection({ maxMatches, note, used, onChange }: { maxMatches: number | undefined; note: string | undefined; used: number; onChange: (patch: { maxMatches?: number; note?: string }) => void }) {
  const mode: Limit = maxMatches == null ? 'always' : maxMatches === 1 ? 'once' : 'n';
  const err = useIssue('maxMatches');
  return (
    <section className={styles.section} aria-labelledby="rule-options">
      <div className={styles.sectionHead}>
        <h3 id="rule-options" className={styles.sectionTitle}>
          Options
        </h3>
      </div>
      <div className={styles.optionsGrid}>
        <Block label="Match limit" hint={maxMatches != null ? `${Math.min(used, maxMatches)} of ${maxMatches} used` : 'Applies to every match'}>
          <div className={styles.limitRow}>
            <SegmentedControl<Limit>
              size="sm"
              value={mode}
              onValueChange={(m) => onChange({ maxMatches: m === 'always' ? undefined : m === 'once' ? 1 : Math.max(2, maxMatches ?? 3) })}
              items={[
                { value: 'always', label: 'Always' },
                { value: 'once', label: 'Once' },
                { value: 'n', label: 'N times' },
              ]}
              aria-label="Match limit"
            />
            {mode === 'n' && <NumberInput size="sm" value={maxMatches ?? 3} onValueChange={(v) => onChange({ maxMatches: v ?? 2 })} min={1} max={4_294_967_295} unit="times" className={styles.limitInput} aria-label="Number of matches" invalid={err ? true : undefined} />}
          </div>
          <FieldError>{err}</FieldError>
        </Block>
        <Block label="Note" hint="Only for you">
          <TextArea autoResize minRows={1} maxRows={6} value={note ?? ''} placeholder="Why this rule exists, ticket links…" onChange={(e) => onChange({ note: e.target.value || undefined })} aria-label="Note" />
        </Block>
      </div>
    </section>
  );
}
