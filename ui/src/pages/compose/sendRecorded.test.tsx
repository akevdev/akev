import { afterEach, describe, expect, test } from 'bun:test';
import { renderToStaticMarkup } from 'react-dom/server';
import { api, browserTransport, type Transport } from '../../api/client';
import type { BodyView, SendResponse } from '../../api/types';
import { TooltipProvider } from '../../components/ui/Tooltip';
import { emptyDraft } from './model';
import { ResponsePane, composeBodySource } from './ResponsePane';
import { sendTab, storedExchangeId, tabsActions, useComposeStore } from './store';

const view: BodyView = {
  kind: 'json',
  size: 11,
  wireSize: 11,
  encodings: [],
  text: '{"ok":true}',
  formatted: null,
  structured: null,
  decodeError: null,
  truncated: false,
  captureTruncated: false,
};

function sendResponse(id: number, recorded: boolean): SendResponse {
  return {
    id,
    response: { status: 200, statusMessage: 'OK', httpVersion: 'HTTP/1.1', headers: [['content-type', 'application/json']] },
    error: null,
    timing: { start: Date.now() * 1000, responseBody: 12 },
    body: view,
    recorded,
  } as SendResponse;
}

/** Answer `POST /api/send` with the queued results; record every request URL. */
function fakeDaemon(results: SendResponse[]): { transport: Transport; urls: string[] } {
  const urls: string[] = [];
  return {
    urls,
    transport: {
      fetch: async (url) => {
        urls.push(url);
        const next = results.shift();
        return new Response(JSON.stringify(next ?? {}), { status: next ? 200 : 404, headers: { 'content-type': 'application/json' } });
      },
      openSocket: () => {
        throw new Error('unused');
      },
    },
  };
}

afterEach(() => api.setTransport(browserTransport));

describe('Compose sends while recording is paused (recorded: false)', () => {
  test('storedExchangeId only trusts recorded exchanges', () => {
    expect(storedExchangeId({ id: 4, recorded: true })).toBe(4);
    expect(storedExchangeId({ id: 4, recorded: false })).toBeUndefined();
    expect(storedExchangeId(undefined)).toBeUndefined();
  });

  test('an unstored response sets no lastExchangeId / history id and keeps the last stored one for comparing', async () => {
    const { transport, urls } = fakeDaemon([sendResponse(7, true), sendResponse(8, false), sendResponse(9, true)]);
    api.setTransport(transport);
    const tabId = tabsActions.newTab({ ...emptyDraft(), method: 'GET', url: 'http://example.test/a' });
    const tab = () => useComposeStore.getState().tabs.find((t) => t.id === tabId);
    const latestHistory = () => useComposeStore.getState().history[0];

    expect((await sendTab(tabId)).ok).toBe(true);
    expect(tab()?.lastExchangeId).toBe(7);
    expect(latestHistory()?.exchangeId).toBe(7);

    expect((await sendTab(tabId)).ok).toBe(true);
    expect(tab()?.lastExchangeId).toBeUndefined();
    expect(latestHistory()?.exchangeId).toBeUndefined();
    const unstored = useComposeStore.getState().responses[tabId];
    expect(unstored?.result?.recorded).toBe(false);
    expect(unstored?.previousId).toBe(7);

    // The next stored send compares with #7, not the unfetchable #8.
    expect((await sendTab(tabId)).ok).toBe(true);
    expect(useComposeStore.getState().responses[tabId]?.previousId).toBe(7);
    expect(tab()?.lastExchangeId).toBe(9);
    // Nothing was fetched by the unstored id.
    expect(urls.every((u) => u.endsWith('/api/send'))).toBe(true);
    tabsActions.close(tabId);
  });

  test('the response pane hides "Open in Traffic", compare and detail tabs, and says why', () => {
    const response = { status: 'done' as const, startedAt: 0, finishedAt: 12, url: 'http://example.test/a', result: sendResponse(8, false), previousId: 7 };
    const render = (r: typeof response) =>
      renderToStaticMarkup(
        <TooltipProvider>
          <ResponsePane tabId="t" responseTab="body" response={r} layout="bottom" onToggleLayout={() => {}} onSend={() => {}} onCancel={() => {}} onRetryWith={() => {}} onFocusUrl={() => {}} />
        </TooltipProvider>,
      );
    const html = render(response);
    expect(html).not.toContain('Open in Traffic');
    expect(html).not.toContain('Compare with the previous response');
    expect(html).not.toContain('>Timing<');
    expect(html).toMatch(/Recording is paused — this response wasn't stored|Not stored — the capture settings ignore this host/);
    // The body is shown from the view alone (no exchange to fetch raw bytes from).
    expect(composeBodySource(response.result)).toEqual({ view, side: 'response' });

    const stored = render({ ...response, result: sendResponse(9, true) });
    expect(stored).toContain('Open in Traffic');
    expect(stored).toContain('>Timing<');
    expect(stored).not.toContain("this response wasn't stored");
  });
});
