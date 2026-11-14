import { describe, expect, test } from 'bun:test';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ExchangeDetail, ExchangeSummary } from '../../../../api/types';
import { TooltipProvider } from '../../../../components/ui/Tooltip';
import { GrpcCallChips } from './GrpcCall';

const summary = {
  id: 1,
  kind: 'http',
  state: 'complete',
  startedAt: 0,
  method: 'POST',
  scheme: 'https',
  host: 'grpc.example.com',
  port: 443,
  path: '/shop.v1.Catalog/GetItem',
  httpVersion: 'HTTP/2',
  status: 200,
  contentKind: 'grpc',
  requestSize: 5,
  responseSize: 5,
} as ExchangeSummary;

const detail = (trailers?: [string, string][], headers: [string, string][] = [['content-type', 'application/grpc']]) =>
  ({
    summary,
    request: { method: 'POST', url: 'https://grpc.example.com/shop.v1.Catalog/GetItem', scheme: 'https', host: 'grpc.example.com', port: 443, path: '/shop.v1.Catalog/GetItem', httpVersion: 'HTTP/2', headers: [['content-type', 'application/grpc']] },
    response: { status: 200, statusMessage: 'OK', httpVersion: 'HTTP/2', headers },
    responseTrailers: trailers,
  }) as unknown as ExchangeDetail;

const render = (s: ExchangeSummary, d: ExchangeDetail | null) => renderToStaticMarkup(<TooltipProvider><GrpcCallChips summary={s} detail={d} /></TooltipProvider>);

describe('gRPC header chips', () => {
  test('service / method and the status from the trailers', () => {
    const html = render(summary, detail([['grpc-status', '5'], ['grpc-message', 'item%20not%20found']]));
    expect(html).toContain('Catalog');
    expect(html).toContain('GetItem');
    expect(html).toContain('NOT_FOUND');
  });

  test('trailers-only responses carry the status in the headers', () => {
    const html = render(summary, detail(undefined, [['content-type', 'application/grpc'], ['grpc-status', '0']]));
    expect(html).toContain('OK');
  });

  test('no status yet, and nothing for non-gRPC exchanges', () => {
    const html = render(summary, null);
    expect(html).toContain('GetItem');
    expect(html).not.toContain('data-testid="grpc-status"');
    expect(render({ ...summary, contentKind: 'json', contentType: 'application/json' }, null)).toBe('');
    expect(render({ ...summary, path: '/health' }, null)).toBe('');
  });
});
