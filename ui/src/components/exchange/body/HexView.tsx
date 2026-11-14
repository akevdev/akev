/** Hex mode of BodyViewer: raw bytes fetched lazily from the raw endpoint, in a HexViewer. */

import clsx from 'clsx';
import { Binary } from 'lucide-react';
import { useState } from 'react';
import type { ExchangeId, Side } from '../../../api/types';
import { formatBytes } from '../../../lib/format';
import { Button, EmptyState, HexViewer, SegmentedControl, Skeleton } from '../../ui';
import { Banner } from '../parts/Parts';
import { useBytes, type BytesSource } from './data';
import styles from './body.module.css';

const CONFIRM_ABOVE = 32 * 1024 * 1024;

export interface HexViewProps {
  exchangeId?: ExchangeId;
  side?: Side;
  original?: boolean;
  rawUrl?: string;
  text: string | null;
  encodings: string[];
  /** Decoded size (for the large-body confirmation). */
  size: number;
  wireSize: number;
  decodeFailed: boolean;
  refreshKey?: string | number;
}

export function HexView({ exchangeId, side, original, rawUrl, text, encodings, size, wireSize, decodeFailed, refreshKey }: HexViewProps) {
  const encoded = encodings.some((e) => e && e.toLowerCase() !== 'identity');
  const fromExchange = exchangeId != null && side != null && !original;
  const [wire, setWire] = useState(decodeFailed);
  const [confirmed, setConfirmed] = useState(false);

  let source: BytesSource | null = null;
  let note: string | null = null;
  if (fromExchange) source = { type: 'exchange', id: exchangeId, side, decoded: !wire && !decodeFailed };
  else if (rawUrl && !original) source = { type: 'url', url: rawUrl };
  else if (text != null) {
    source = { type: 'text', text };
    note = original ? "Raw bytes of the original (pre-modification) body aren't available — showing the UTF-8 bytes of its decoded text." : 'Showing the UTF-8 bytes of the decoded text.';
  }

  const expected = wire ? wireSize : size;
  const needsConfirm = expected > CONFIRM_ABOVE && !confirmed && source?.type !== 'text';
  const state = useBytes(source, !needsConfirm, refreshKey);

  if (!source) {
    return (
      <div className={styles.center}>
        <EmptyState icon={Binary} size="sm" title="Raw bytes unavailable" description="This body has no reachable raw data." />
      </div>
    );
  }

  return (
    <div className={styles.fill}>
      {note && <Banner tone="neutral">{note}</Banner>}
      {fromExchange && encoded && (
        <div className={styles.hexBar}>
          <SegmentedControl
            size="sm"
            aria-label="Bytes to show"
            value={wire || decodeFailed ? 'wire' : 'decoded'}
            onValueChange={(v) => setWire(v === 'wire')}
            items={[
              { value: 'decoded', label: 'Decoded', disabled: decodeFailed, 'aria-label': decodeFailed ? 'Decoded (decoding failed)' : `Decoded (after removing ${encodings.join(' + ')})` },
              { value: 'wire', label: `As received (${encodings.join(' + ')})`, 'aria-label': 'As received on the wire' },
            ]}
          />
          <span className={clsx('tnum')}>{formatBytes(expected)}</span>
        </div>
      )}
      {needsConfirm ? (
        <div className={styles.center}>
          <EmptyState
            icon={Binary}
            size="sm"
            title={`Load ${formatBytes(expected)} of bytes?`}
            description="Large bodies take a moment to download into the hex viewer."
            actions={
              <Button size="sm" variant="primary" onClick={() => setConfirmed(true)}>
                Load bytes
              </Button>
            }
          />
        </div>
      ) : state.error ? (
        <div className={styles.center}>
          <EmptyState
            icon={Binary}
            size="sm"
            tone="danger"
            title="Couldn't load the raw bytes"
            description={state.error}
            actions={
              <Button size="sm" onClick={state.retry}>
                Retry
              </Button>
            }
          />
        </div>
      ) : state.bytes ? (
        <HexViewer data={state.bytes} aria-label="Body bytes" />
      ) : (
        <div className={styles.loading} aria-busy>
          {Array.from({ length: 8 }, (_, i) => (
            <Skeleton key={i} height={12} width={`${92 - (i % 3) * 7}%`} />
          ))}
        </div>
      )}
    </div>
  );
}
