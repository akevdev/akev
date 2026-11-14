import clsx from 'clsx';
import { Workflow } from 'lucide-react';
import type { ExchangeDetail, ExchangeSummary } from '../../../../api/types';
import { Tooltip } from '../../../../components/ui';
import { grpcStatus } from '../../../../components/exchange/model/body';
import { getHeader } from '../../../../lib/http';
import { grpcCallFromPath, isGrpcContentType } from '../../../../lib/protobuf';
import headerStyles from './ExchangeHeader.module.css';
import styles from './GrpcCall.module.css';

/**
 * Detail header chips for gRPC / gRPC-Web calls: `Catalog / GetItem` (full service in the
 * tooltip) and the call's `grpc-status` from the trailers (or the headers of a trailers-only
 * response).
 */
export function GrpcCallChips({ summary, detail }: { summary: ExchangeSummary; detail: ExchangeDetail | null }) {
  const requestType = detail ? getHeader(detail.request.headers, 'content-type') : undefined;
  const grpc = summary.contentKind === 'grpc' || summary.contentKind === 'grpc-web' || isGrpcContentType(requestType) || isGrpcContentType(summary.contentType);
  const call = grpc ? grpcCallFromPath(detail?.request.path ?? summary.path) : null;
  if (!call) return null;
  const status = grpcStatus(detail?.responseTrailers, detail?.response?.headers);
  const statusTip = status ? `grpc-status ${status.code} (${status.name})${status.message ? ` — ${status.message}` : ''}` : '';
  return (
    <>
      <span className={headerStyles.chip}>
        <Tooltip content={`gRPC call ${call.service}/${call.method}`}>
          <span className={styles.call} data-testid="grpc-call">
            <Workflow size={11} strokeWidth={2} aria-hidden className={styles.icon} />
            <span className={styles.service}>{call.shortService}</span>
            <span className={styles.sep} aria-hidden>
              /
            </span>
            <span className={styles.method}>{call.method}</span>
          </span>
        </Tooltip>
      </span>
      {status && (
        <span className={headerStyles.chip}>
          <Tooltip content={statusTip} multiline>
            <span className={clsx(styles.status, status.ok ? styles.ok : styles.bad)} data-testid="grpc-status">
              <span className={styles.dot} aria-hidden />
              {status.name}
            </span>
          </Tooltip>
        </span>
      )}
    </>
  );
}
