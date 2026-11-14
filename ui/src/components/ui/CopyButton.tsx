import { Check, Copy } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { copyText } from '../../lib/platform';
import type { ControlSize } from './Button';
import { IconButton } from './IconButton';
import { toast } from './toast';

export interface CopyButtonProps {
  /** Text to copy, or a function producing it lazily (e.g. a large body). */
  value: string | (() => string | Promise<string>);
  label?: string;
  size?: ControlSize;
  /** Toast message on success (omit for silent). */
  toastMessage?: string;
  className?: string;
  tooltipSide?: 'top' | 'right' | 'bottom' | 'left';
  disabled?: boolean;
}

/** Icon button that copies text and flips to a ✓ for 1.2 s. */
export function CopyButton({ value, label = 'Copy', size = 'sm', toastMessage, className, tooltipSide, disabled }: CopyButtonProps) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);

  const onClick = async () => {
    try {
      const text = typeof value === 'function' ? await value() : value;
      const ok = await copyText(text);
      if (!ok) throw new Error('Clipboard unavailable');
      setCopied(true);
      if (toastMessage) toast.success(toastMessage);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopied(false), 1200);
    } catch (e) {
      toast.error('Copy failed', { description: e instanceof Error ? e.message : undefined });
    }
  };

  return (
    <IconButton
      label={copied ? 'Copied' : label}
      icon={copied ? Check : Copy}
      size={size}
      onClick={onClick}
      className={className}
      tooltipSide={tooltipSide}
      disabled={disabled}
      style={copied ? { color: 'var(--success)' } : undefined}
    />
  );
}
