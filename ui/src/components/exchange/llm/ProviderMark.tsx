import clsx from 'clsx';
import type { CSSProperties } from 'react';
import { providerInfo, tintVar } from '../../../lib/llm/providers';
import styles from './ProviderMark.module.css';

export interface ProviderMarkProps {
  /** Provider id (`openai`, `anthropic`, …). */
  provider: string;
  /** Square size in px (default 18). */
  size?: number;
  className?: string;
}

/** A provider's monogram in its tint — a text badge, never a brand asset. */
export function ProviderMark({ provider, size = 18, className }: ProviderMarkProps) {
  const info = providerInfo(provider);
  const style = {
    '--tint': tintVar(info.tint),
    width: size,
    height: size,
    fontSize: Math.round(size * (info.mark.length > 1 ? 0.44 : 0.56)),
  } as CSSProperties;
  return (
    <span className={clsx(styles.mark, className)} style={style} role="img" aria-label={info.name}>
      {info.mark}
    </span>
  );
}
