import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { api } from '../../../api/client';
import type { Suggestion } from '../../../api/types';
import { trafficStore } from '../../../state/traffic';
import { useUiStore } from '../../../state/ui';
import { act, render, screen, stub, summary, userEvent, waitFor, within } from '../../../test/render';
import { FilterBar } from './FilterBar';

let sent: string[] = [];

beforeEach(() => {
  sent = [];
  let seq = 0;
  trafficStore.setFilterSink((q) => {
    sent.push(q);
    return ++seq;
  });
  trafficStore.reset([summary(1), summary(2, { status: 404 }), summary(3, { status: 500 })]);
  trafficStore.flushNow();
  stub(api.filter, 'docs', async () => []);
  stub(api.filter, 'suggest', async () => []);
});

afterEach(() => {
  trafficStore.clearFilter();
  trafficStore.flushFilter();
  trafficStore.setFilterSink(null);
  useUiStore.setState({ recentFilters: [] });
});

const input = () => screen.getByRole('combobox', { name: 'Filter traffic' }) as HTMLInputElement;

/** The daemon's answer to the latest query sent. */
function answer(ids: number[] | null, error?: { message: string; start: number; end: number }) {
  act(() => {
    trafficStore.applyFilterResult(trafficStore.getSnapshot().filter.seq, ids, error);
    trafficStore.flushNow();
  });
}

describe('the traffic filter bar', () => {
  test('typing sends the query to Akev; Enter applies it now and remembers it', async () => {
    render(<FilterBar />);
    await userEvent.type(input(), 'status:>=400');
    await waitFor(() => expect(sent.at(-1)).toBe('status:>=400'));
    await userEvent.keyboard('{Enter}');
    expect(useUiStore.getState().recentFilters[0]).toBe('status:>=400');
  });

  test('shows how many exchanges match', async () => {
    render(<FilterBar />);
    await userEvent.type(input(), 'status:>=400');
    await waitFor(() => expect(sent.length).toBeGreaterThan(0));
    answer([2, 3]);
    expect(screen.getByText('2 matches')).toBeTruthy();
    expect(input().getAttribute('aria-invalid')).toBeNull();
  });

  test("marks the query invalid with the daemon's message", async () => {
    render(<FilterBar />);
    await userEvent.type(input(), 'stauts:200');
    await waitFor(() => expect(sent.length).toBeGreaterThan(0));
    answer(null, { message: 'Unknown field "stauts"', start: 0, end: 6 });
    expect(input().getAttribute('aria-invalid')).toBe('true');
    expect(screen.getByRole('img', { name: 'Filter error' })).toBeTruthy();
    expect(screen.getAllByText('Unknown field "stauts"').length).toBeGreaterThan(0);
  });

  test('suggestions from Akev complete with Tab', async () => {
    const suggestion: Suggestion = { label: 'status', detail: 'Response status code', insert: 'status:', replaceStart: 0, replaceEnd: 3, kind: 'field' };
    const asked = stub(api.filter, 'suggest', async () => [suggestion]);
    render(<FilterBar />);
    await userEvent.type(input(), 'sta');
    const list = await screen.findByRole('listbox', { name: 'Filter suggestions' });
    expect(within(list).getByRole('option', { name: /status/ })).toBeTruthy();
    expect(asked.at(-1)?.slice(0, 2)).toEqual(['sta', 3]);
    await userEvent.keyboard('{Tab}');
    expect(input().value).toBe('status:');
  });

  test('↑ in the empty field recalls recent filters', async () => {
    useUiStore.setState({ recentFilters: ['method:post', 'host:api.example.com'] });
    render(<FilterBar />);
    await userEvent.click(input());
    await userEvent.keyboard('{ArrowUp}');
    expect(input().value).toBe('method:post');
    await userEvent.keyboard('{ArrowUp}');
    expect(input().value).toBe('host:api.example.com');
  });

  test('the clear button empties the filter', async () => {
    render(<FilterBar />);
    await userEvent.type(input(), 'host:example.com');
    await userEvent.click(screen.getByRole('button', { name: 'Clear filter' }));
    expect(input().value).toBe('');
    expect(trafficStore.getSnapshot().filter.text).toBe('');
  });
});
