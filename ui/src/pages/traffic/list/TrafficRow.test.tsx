import { beforeEach, describe, expect, test } from 'bun:test';
import type { ExchangeSummary } from '../../../api/types';
import { trafficStore } from '../../../state/traffic';
import type { TrafficColumnId } from '../../../state/ui';
import { act, render, screen, summary, within } from '../../../test/render';
import { TrafficRow } from './TrafficRow';

const COLUMNS: TrafficColumnId[] = ['indicator', 'method', 'status', 'host', 'path', 'type', 'size', 'time', 'source'];

function load(...list: ExchangeSummary[]) {
  act(() => {
    trafficStore.reset(list);
    trafficStore.flushNow();
  });
}

function renderRow(id: number) {
  render(
    <div role="grid">
      <TrafficRow id={id} index={0} top={0} columns={COLUMNS} monoPaths={false} />
    </div>,
  );
  return screen.getByRole('row');
}

const cells = (row: HTMLElement) => within(row).getAllByRole('gridcell').map((c) => c.textContent?.trim() ?? '');

beforeEach(() => {
  trafficStore.clearSelection();
});

describe('a traffic list row', () => {
  test('shows the exchange in the visible columns', () => {
    load(summary(7, { method: 'POST', status: 201, host: 'api.example.com', port: 8443, path: '/v1/orders?page=2', contentKind: 'json', responseSize: 2048, duration: 143, source: 'curl' }));
    const row = renderRow(7);
    const [, method, status, host, path, type, size, time, source] = cells(row);
    expect(method).toBe('POST');
    expect(status).toContain('201');
    expect(host).toBe('api.example.com:8443');
    expect(path).toBe('/v1/orders?page=2');
    expect(type).toBe('json');
    expect(size).toBe('2 KB');
    expect(time).toBe('143 ms');
    expect(source).toBe('curl');
    // The query string is styled apart from the path.
    expect(within(row).getByText('?page=2')).toBeTruthy();
  });

  test('follows selection and updates in place', () => {
    load(summary(3, { status: 200 }));
    const row = renderRow(3);
    expect(row.getAttribute('aria-selected')).toBe('false');
    act(() => trafficStore.select(3));
    expect(row.getAttribute('aria-selected')).toBe('true');
    act(() => {
      trafficStore.applyExchanges([], [summary(3, { status: 503 })]);
      trafficStore.flushNow();
    });
    expect(cells(screen.getByRole('row'))[2]).toContain('503');
  });

  test('marks paused, pinned, mocked and highlighted exchanges', () => {
    load(summary(1, { paused: 'request', status: undefined, state: 'waiting' }), summary(2, { pinned: true, mocked: true, color: 'red' }));
    const paused = renderRow(1);
    expect(within(paused).getByLabelText('Paused')).toBeTruthy();
    act(() => {
      trafficStore.select(2);
    });
    render(
      <div role="grid">
        <TrafficRow id={2} index={1} top={30} columns={COLUMNS} monoPaths={false} />
      </div>,
    );
    const pinned = screen.getAllByRole('row')[1] as HTMLElement;
    expect(within(pinned).getByLabelText('Pinned')).toBeTruthy();
    expect(within(pinned).getByLabelText('Mocked')).toBeTruthy();
    expect(within(pinned).getByLabelText('red highlight')).toBeTruthy();
  });

  test('explains TLS failures and tunnels instead of a path', () => {
    load(summary(4, { kind: 'tls-failure', status: undefined, state: 'aborted', path: '' }), summary(5, { kind: 'tunnel', status: undefined, host: 'pinned.example.com', path: '' }));
    expect(within(renderRow(4)).getByText('TLS handshake failed — client rejected certificate')).toBeTruthy();
    render(
      <div role="grid">
        <TrafficRow id={5} index={1} top={30} columns={COLUMNS} monoPaths={false} />
      </div>,
    );
    expect(screen.getByText(/Tunnel to pinned\.example\.com.*not intercepted/)).toBeTruthy();
  });

  test('a row whose exchange is gone renders empty', () => {
    load();
    const row = renderRow(99);
    expect(within(row).queryAllByRole('gridcell')).toHaveLength(0);
  });
});
