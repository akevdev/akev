import { describe, expect, test } from 'bun:test';
import { renderToStaticMarkup } from 'react-dom/server';
import type { ReactNode } from 'react';
import type { BodyView, SendResponse } from '../../api/types';
import { BodyViewer } from '../../components/exchange/BodyViewer';
import { HtmlPreview } from '../../components/exchange/body/Previews';
import { PREVIEW_CSP } from '../../components/exchange/model/body';
import { TooltipProvider } from '../../components/ui/Tooltip';
import { composeBodySource } from './ResponsePane';

const render = (node: ReactNode) => renderToStaticMarkup(<TooltipProvider>{node}</TooltipProvider>);

/** A gzip-encoded SVG response as Compose receives it from `POST /api/send`. */
const gzipSvg: BodyView = {
  kind: 'image',
  size: 41,
  wireSize: 30,
  encodings: ['gzip'],
  text: '<svg xmlns="http://www.w3.org/2000/svg"/>',
  formatted: null,
  structured: { format: 'svg', width: 1, height: 1 },
  decodeError: null,
  truncated: false,
  captureTruncated: false,
};
const result: Pick<SendResponse, 'id' | 'body' | 'recorded'> = { id: 9, body: gzipSvg, recorded: true };

describe('Compose response body', () => {
  test('previews and downloads use the decoded bytes of the recorded exchange (regression)', () => {
    const source = composeBodySource(result);
    expect(source).toEqual({ view: gzipSvg, exchangeId: 9, side: 'response' });
    const html = render(<BodyViewer {...source} contentType="image/svg+xml" />);
    expect(html).toContain('src="/api/exchanges/9/body/response/raw?decoded=1"');
    // Content-encoded bodies get the decoded download plus an "as received" option.
    expect(html).toContain('aria-label="Download body"');
    // The old wiring (a bare raw URL) served the gzip bytes to <img>.
    const old = render(<BodyViewer view={gzipSvg} rawUrl="/api/exchanges/9/body/response/raw" contentType="image/svg+xml" />);
    expect(old).toContain('src="/api/exchanges/9/body/response/raw"');
  });
});

describe('HTML preview iframe markup', () => {
  test('is sandboxed with no permissions and carries the embedder CSP', () => {
    const html = renderToStaticMarkup(<HtmlPreview html={'<!-- <head> --><img src="http://attacker.example/x">'} />);
    expect(html).toContain('sandbox=""');
    expect(html).toContain(`csp="${PREVIEW_CSP.replace(/'/g, '&#x27;')}"`);
    expect(html).toMatch(/referrerpolicy="no-referrer"/i);
    expect(html).not.toMatch(/allow-(scripts|same-origin)/);
    const srcdoc = /srcDoc="([^"]*)"/i.exec(html)?.[1] ?? '';
    expect(srcdoc.startsWith('&lt;meta http-equiv=&quot;Content-Security-Policy&quot;')).toBe(true);
  });
});
