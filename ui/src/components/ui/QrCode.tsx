import clsx from 'clsx';
import qrcode from 'qrcode-generator';
import { useMemo } from 'react';
import styles from './QrCode.module.css';

export interface QrCodeProps {
  /** Text/URL to encode. */
  value: string;
  /** Rendered size in px (default 160). */
  size?: number;
  /** Error correction (default `M`). */
  level?: 'L' | 'M' | 'Q' | 'H';
  className?: string;
  /** Accessible description (defaults to the encoded value). */
  label?: string;
}

/** Build an SVG path for the dark modules, merging horizontal runs. Exported for tests. */
export function qrPath(value: string, level: 'L' | 'M' | 'Q' | 'H' = 'M'): { path: string; size: number } {
  const qr = qrcode(0, level);
  qr.addData(value, 'Byte');
  qr.make();
  const n = qr.getModuleCount();
  let path = '';
  for (let r = 0; r < n; r++) {
    let c = 0;
    while (c < n) {
      if (!qr.isDark(r, c)) {
        c++;
        continue;
      }
      const start = c;
      while (c < n && qr.isDark(r, c)) c++;
      path += `M${start} ${r}h${c - start}v1h-${c - start}z`;
    }
  }
  return { path, size: n };
}

/**
 * Crisp SVG QR code (always dark-on-white with a quiet zone, so phones scan it in any theme).
 * Used by the Connect page for the device setup URL.
 */
export function QrCode({ value, size = 160, level = 'M', className, label }: QrCodeProps) {
  const qr = useMemo(() => {
    try {
      return qrPath(value, level);
    } catch {
      return null; // value too long for a QR code
    }
  }, [value, level]);
  if (!qr) return <div className={clsx(styles.error, className)} style={{ width: size, height: size }}>Too long for a QR code</div>;
  const quiet = 2;
  const box = qr.size + quiet * 2;
  return (
    <svg
      className={clsx(styles.qr, className)}
      width={size}
      height={size}
      viewBox={`${-quiet} ${-quiet} ${box} ${box}`}
      shapeRendering="crispEdges"
      role="img"
      aria-label={label ?? `QR code: ${value}`}
    >
      <rect x={-quiet} y={-quiet} width={box} height={box} fill="#ffffff" />
      <path d={qr.path} fill="#0a0b0e" />
    </svg>
  );
}
