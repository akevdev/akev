import clsx from 'clsx';
import type { ReactNode } from 'react';
import { CommandBlock } from './CommandBlock';
import type { MetaStep, TrustInstructions } from './metadata';
import type { Guide, GuideBlock } from './snippets';
import styles from './GuideView.module.css';

/** Daemon steps (`{ title, detail }`) as step nodes: bold title over the detail. */
export function metaStepNodes(steps: readonly MetaStep[]): ReactNode[] {
  return steps.map((s) =>
    s.title ? (
      <>
        <span className={styles.stepTitle}>{s.title}</span>
        {s.detail && <span className={styles.stepDetail}>{s.detail}</span>}
      </>
    ) : (
      s.detail
    ),
  );
}

/** Daemon-provided trust instructions for this machine (exact CA path, install + remove). */
export function TrustView({ trust, className }: { trust: TrustInstructions; className?: string }) {
  return (
    <div className={clsx(styles.guide, className)}>
      {trust.summary && <p className={styles.intro}>{trust.summary}</p>}
      {trust.options.map((o) => (
        <div key={o.title} className={styles.block}>
          <div className={styles.optionTitle}>{o.title}</div>
          {o.install.length > 0 && <CommandBlock code={o.install.join('\n')} aria-label={`${o.title}: install`} />}
          {o.steps.length > 0 && <Steps steps={o.steps} />}
          {o.remove.length > 0 && <CommandBlock code={o.remove.join('\n')} title="To remove it later" aria-label={`${o.title}: remove`} />}
        </div>
      ))}
      {trust.notes.map((n) => (
        <p key={n} className={styles.note}>
          {n}
        </p>
      ))}
    </div>
  );
}

/** Numbered steps list. */
export function Steps({ steps, className }: { steps: readonly ReactNode[]; className?: string }) {
  return (
    <ol className={clsx(styles.steps, className)}>
      {steps.map((s, i) => (
        <li key={i} className={styles.step}>
          <span className={clsx(styles.num, 'tnum')} aria-hidden>
            {i + 1}
          </span>
          <span className={styles.stepText}>{s}</span>
        </li>
      ))}
    </ol>
  );
}

function Block({ block }: { block: GuideBlock }) {
  return (
    <div className={styles.block}>
      {block.text && <p className={styles.text}>{block.text}</p>}
      {block.steps && <Steps steps={block.steps} />}
      {block.code && <CommandBlock code={block.code} title={block.title} />}
      {!block.code && block.title && <div className={styles.blockTitle}>{block.title}</div>}
      {block.note && <p className={styles.note}>{block.note}</p>}
    </div>
  );
}

/** Renders a {@link Guide}: intro + blocks (code with copy, steps, notes). */
export function GuideView({ guide, header, className }: { guide: Guide; header?: ReactNode; className?: string }) {
  return (
    <div className={clsx(styles.guide, className)}>
      {header}
      <p className={styles.intro}>{guide.intro}</p>
      {guide.blocks.map((b, i) => (
        <Block key={i} block={b} />
      ))}
    </div>
  );
}
