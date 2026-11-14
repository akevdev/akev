import { Play } from 'lucide-react';
import { useMemo } from 'react';
import type { TerminalShell } from '../../../api/types';
import { Button, CopyButton, QrCode } from '../../../components/ui';
import { activateInterceptor, clearActivationResult, useActivationResult, usePending } from '../actions';
import { CommandBlock } from '../CommandBlock';
import { metaStepNodes, Steps, TrustView } from '../GuideView';
import { parseActivation } from '../metadata';
import { SHELLS } from '../snippets';
import { Callout, PanelSection, ResultBlock } from './kit';
import type { PanelProps } from './types';
import styles from './panels.module.css';

/** Fallback for interceptors without a tailored panel: renders whatever metadata provides. */
export function GenericPanel({ info, meta }: PanelProps) {
  const busy = usePending(info.id);
  const result = useActivationResult(info.id);
  const details = useMemo(() => (result ? parseActivation(result.metadata) : null), [result]);
  const snippets = Object.entries(meta.snippets) as [TerminalShell, string][];
  const hasContent = !!(meta.instructions || meta.setupUrl || meta.steps.length || snippets.length || meta.command || meta.trust || meta.notes.length);

  return (
    <>
      {!info.available && info.unavailableReason && <Callout tone="warning">{info.unavailableReason}</Callout>}
      {meta.instructions && <p className={styles.lead}>{meta.instructions}</p>}
      {meta.setupUrl && (
        <div className={styles.qrRow}>
          <QrCode value={meta.setupUrl} size={120} className={styles.qr} label={`QR code for ${meta.setupUrl}`} />
          <div className={styles.qrText}>
            <span className={styles.qrTitle}>Setup page</span>
            <span className={styles.row}>
              <span className={styles.addr}>{meta.setupUrl}</span>
              <CopyButton value={meta.setupUrl} label="Copy setup page URL" toastMessage="Setup page URL copied" />
            </span>
          </div>
        </div>
      )}
      {meta.steps.length > 0 && (
        <PanelSection title="Steps">
          <Steps steps={metaStepNodes(meta.steps)} />
        </PanelSection>
      )}
      {(snippets.length > 0 || meta.command) && (
        <PanelSection title="Commands">
          {meta.command && <CommandBlock code={meta.command} />}
          {snippets.map(([shell, code]) => (
            <CommandBlock key={shell} title={meta.shellLabels[shell] ?? SHELLS.find((s) => s.id === shell)?.label ?? shell} code={code} />
          ))}
        </PanelSection>
      )}
      {meta.trust && (
        <PanelSection title="Trust the certificate">
          <TrustView trust={meta.trust} />
        </PanelSection>
      )}
      <PanelSection>
        {!hasContent && <p className={styles.lead}>{info.description}</p>}
        <Button variant="primary" icon={Play} loading={busy} disabled={!info.available} onClick={() => void activateInterceptor(info, undefined, { quiet: true })} className={styles.selfStart}>
          {info.active ? 'Activate again' : 'Activate'}
        </Button>
        {result && (
          <ResultBlock message={result.message ?? `${info.name} is active.`} onDismiss={() => clearActivationResult(info.id)}>
            {details?.command && <CommandBlock code={details.command} />}
          </ResultBlock>
        )}
        {meta.notes.map((n) => (
          <p key={n} className={styles.footnote}>
            {n}
          </p>
        ))}
      </PanelSection>
    </>
  );
}
