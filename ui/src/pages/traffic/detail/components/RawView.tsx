import { TriangleAlert, WrapText } from 'lucide-react';
import { useMemo } from 'react';
import type { ExchangeId, RequestHead, ResponseHead, Side } from '../../../../api/types';
import { CodeEditor, CopyButton, IconButton, Skeleton } from '../../../../components/ui';
import { formatBytes } from '../../../../lib/format';
import { usePagePref } from '../../../../state/ui';
import { useBodyView } from '../hooks/useBodyView';
import { buildRawRequest, buildRawResponse, type RawBody } from '../lib/rawMessage';
import styles from './views.module.css';

export interface RawViewProps {
  exchangeId: ExchangeId;
  side: Side;
  head: RequestHead | ResponseHead;
  /** Pre-modification body (Original view). */
  original?: boolean;
  /** Refetch the body when this changes (exchange still streaming). */
  refreshKey?: string | number;
  /** Declared body size (skips the fetch when 0). */
  bodySize: number;
}

/** The full HTTP/1.1-style message text with `http` highlighting (DESIGN.md §6.4 Raw tab). */
export function RawView({ exchangeId, side, head, original, refreshKey, bodySize }: RawViewProps) {
  const [wrap, setWrap] = usePagePref('traffic.raw.wrap', true);
  const { view, loading, error } = useBodyView(exchangeId, side, original, refreshKey, bodySize > 0);

  const body: RawBody | null = useMemo(() => {
    if (bodySize === 0) return null;
    if (!view) return null;
    return { text: view.text, size: view.size, truncated: view.truncated, captureTruncated: view.captureTruncated };
  }, [view, bodySize]);

  const build = (eol: string) => ('method' in head ? buildRawRequest(head, body, eol) : buildRawResponse(head as ResponseHead, body, eol));
  // biome-ignore lint/correctness/useExhaustiveDependencies: rebuilt only when the head or body changes
  const text = useMemo(() => build('\n'), [head, body]);
  const waitingForBody = bodySize > 0 && !view && loading;

  return (
    <div className={styles.fill}>
      <div className={styles.editorBar}>
        <span className={styles.editorBarText}>
          {error ? (
            <span className={styles.warnText}>
              <TriangleAlert size={12} strokeWidth={2} aria-hidden />
              Body unavailable — {error}
            </span>
          ) : (
            <>Wire order · {bodySize > 0 ? `body ${formatBytes(view?.size ?? bodySize)}` : 'no body'}</>
          )}
        </span>
        <IconButton size="sm" label={wrap ? "Don't wrap lines" : 'Wrap lines'} icon={WrapText} active={wrap} onClick={() => setWrap(!wrap)} />
        <CopyButton value={() => build('\r\n')} label="Copy raw message (CRLF)" toastMessage="Raw message copied" />
      </div>
      {waitingForBody ? (
        <div className={styles.skeletonPad}>
          <Skeleton lines={6} height={10} />
        </div>
      ) : (
        <CodeEditor value={text} language="http" readOnly fill wrap={wrap} contentId={`raw:${exchangeId}:${side}:${original ? 1 : 0}`} aria-label={`Raw ${side}`} />
      )}
    </div>
  );
}
