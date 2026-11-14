import { ArrowRight } from 'lucide-react';
import { api } from '../../../api/client';
import type { TerminalShell } from '../../../api/types';
import { navigate } from '../../../app/router';
import { Button, SegmentedControl } from '../../../components/ui';
import { useAppStore } from '../../../state/app';
import { usePagePref } from '../../../state/ui';
import { CommandBlock } from '../CommandBlock';
import { defaultShell, SHELLS, terminalOneLiner, terminalResetCommand, terminalTestCommand } from '../snippets';
import { LiveStatus, PanelSection } from './kit';
import type { PanelProps } from './types';
import styles from './panels.module.css';

/**
 * Fallback one-liner when the daemon sent none for `shell`. The setup-script URL carries no
 * token: the one-liner sends it as a header (see {@link terminalOneLiner}).
 */
function fallbackOneLiner(shell: TerminalShell, apiUrl: string | null): string {
  const cfg = api.getConfig();
  const base = (cfg.baseUrl || apiUrl || window.location.origin).replace(/\/$/, '');
  const url = `${base}/api/setup/terminal?${new URLSearchParams({ shell }).toString()}`;
  return terminalOneLiner(shell, url, cfg.token);
}

function StepTitle({ n, children }: { n: number; children: string }) {
  return (
    <span className={styles.stepHead}>
      <span className={styles.stepNum}>{n}</span>
      {children}
    </span>
  );
}

/** Existing terminal: shell tabs, one-liner + copy, how to check it works, reset command. */
export function TerminalPanel({ info, meta, proxy }: PanelProps) {
  const apiUrl = useAppStore((s) => s.status?.apiUrl ?? null);
  // The daemon lists the shells it supports on this OS (Windows: PowerShell, cmd, Git Bash).
  const shells = meta.shells.length > 0 ? SHELLS.filter((s) => meta.shells.includes(s.id)) : SHELLS;
  const fallback = meta.defaultShell && shells.some((s) => s.id === meta.defaultShell) ? meta.defaultShell : (shells.find((s) => s.id === defaultShell(proxy.platform))?.id ?? shells[0]?.id ?? 'bash');
  const [stored, setShell] = usePagePref<TerminalShell>('connect.shell', fallback);
  const shell = shells.some((s) => s.id === stored) ? stored : fallback;

  const command = meta.snippets[shell] ?? ((shell === 'bash' || shell === 'zsh') && meta.command ? meta.command : fallbackOneLiner(shell, apiUrl));
  const reset = meta.resetSnippets[shell] ?? terminalResetCommand(shell);
  const shortReset = !reset.includes(' ');

  return (
    <>
      <PanelSection title={<StepTitle n={1}>Run this in the terminal you want to intercept</StepTitle>}>
        <SegmentedControl<TerminalShell>
          value={shell}
          onValueChange={setShell}
          items={shells.map((s) => ({ value: s.id, label: meta.shellLabels[s.id] ?? s.label }))}
          size="sm"
          aria-label="Shell"
          className={styles.selfStart}
        />
        <CommandBlock code={command} aria-label={`${shell} setup command`} copyToast="Command copied — paste it into your terminal" />
        <p className={styles.footnote}>Only that terminal session and the programs you start from it are affected. Nothing is installed.</p>
      </PanelSection>

      <PanelSection title={<StepTitle n={2}>Check that it works</StepTitle>}>
        {info.active ? (
          <LiveStatus active waitingText="" activeText="A terminal is connected — its requests appear in Traffic." />
        ) : (
          <>
            <p className={styles.lead}>Run any command that makes a request — it shows up in Traffic right away.</p>
            <CommandBlock code={terminalTestCommand(shell)} aria-label={`${shell} test command`} />
          </>
        )}
        <Button size="sm" iconRight={ArrowRight} onClick={() => navigate('/traffic')} className={styles.selfStart}>
          View traffic
        </Button>
      </PanelSection>

      <PanelSection
        title="When you're done"
        description={
          shortReset ? (
            <>
              Run <code className={styles.inlineCode}>{reset}</code> in the same terminal to stop intercepting it, or just close the terminal.
            </>
          ) : (
            'Run this in the same terminal to stop intercepting it, or just close the terminal.'
          )
        }
      >
        <CommandBlock code={reset} aria-label={`${shell} reset command`} />
      </PanelSection>
    </>
  );
}
