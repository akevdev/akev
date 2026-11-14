import { afterEach, describe, expect, test } from 'bun:test';
import { useState } from 'react';
import type { Action } from '../../../api/types';
import { resetTauriBridgeForTests } from '../../../lib/platform';
import { render, screen, userEvent, waitFor } from '../../../test/render';
import { ActionForm } from './ActionForms';

/** The form with its action kept in state, like the rule editor; `changes` records every edit. */
function Harness({ initial, changes }: { initial: Action; changes: Action[] }) {
  const [action, setAction] = useState(initial);
  return (
    <ActionForm
      action={action}
      ruleId="r1"
      onTest={() => {}}
      onChange={(a) => {
        changes.push(a);
        setAction(a);
      }}
    />
  );
}

const mapLocal: Action = { type: 'map-local', path: '', fallbackToUpstream: false, headers: [] };

/** Pretend to be the desktop app: Tauri IPC answers the dialog plugin with `picked`. */
function desktopDialog(picked: string | null): unknown[][] {
  const calls: unknown[][] = [];
  (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {
    invoke: async (...args: unknown[]) => {
      calls.push(args);
      return picked;
    },
  };
  resetTauriBridgeForTests();
  return calls;
}

afterEach(() => {
  delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  resetTauriBridgeForTests();
});

describe("the rule editor's action forms", () => {
  test('Map Local: typing the path; no file picker in the browser', async () => {
    const changes: Action[] = [];
    render(<Harness initial={mapLocal} changes={changes} />);
    await userEvent.type(screen.getByRole('textbox', { name: 'File or folder' }), '/srv/site');
    expect(changes.at(-1)).toMatchObject({ type: 'map-local', path: '/srv/site' });
    expect(screen.queryByRole('button', { name: /Choose/ })).toBeNull();
  });

  test('Map Local: the desktop app picks a folder with the native dialog', async () => {
    const calls = desktopDialog('/Users/me/project/dist');
    const changes: Action[] = [];
    render(<Harness initial={mapLocal} changes={changes} />);
    await userEvent.click(screen.getByRole('button', { name: /Choose/ }));
    await userEvent.click(await screen.findByRole('menuitem', { name: 'A folder…' }));
    await waitFor(() => expect(changes.at(-1)).toMatchObject({ path: '/Users/me/project/dist' }));
    expect(calls[0]?.[0]).toBe('plugin:dialog|open');
    expect(calls[0]?.[1]).toMatchObject({ options: { directory: true, multiple: false } });
    expect((screen.getByRole('textbox', { name: 'File or folder' }) as HTMLInputElement).value).toBe('/Users/me/project/dist');
  });

  test('Map Local: cancelling the dialog keeps the path', async () => {
    desktopDialog(null);
    const changes: Action[] = [];
    render(<Harness initial={{ ...mapLocal, path: '/srv/old' } as Action} changes={changes} />);
    await userEvent.click(screen.getByRole('button', { name: /Choose/ }));
    await userEvent.click(await screen.findByRole('menuitem', { name: 'A file…' }));
    await new Promise((r) => setTimeout(r, 10));
    expect(changes).toHaveLength(0);
  });

  test('Map Local: the fallback switch', async () => {
    const changes: Action[] = [];
    render(<Harness initial={mapLocal} changes={changes} />);
    expect(screen.getByText('Missing files get a 404')).toBeTruthy();
    await userEvent.click(screen.getByRole('switch', { name: /Fall back to the server/ }));
    expect(changes.at(-1)).toMatchObject({ fallbackToUpstream: true });
    expect(screen.getByText('Missing files are fetched from the real server')).toBeTruthy();
  });

  test('Breakpoint: request and response checkboxes', async () => {
    const changes: Action[] = [];
    render(<Harness initial={{ type: 'breakpoint', request: true, response: false }} changes={changes} />);
    await userEvent.click(screen.getByRole('checkbox', { name: 'Pause the response' }));
    expect(changes.at(-1)).toEqual({ type: 'breakpoint', request: true, response: true });
    await userEvent.click(screen.getByRole('checkbox', { name: 'Pause the request' }));
    expect(changes.at(-1)).toEqual({ type: 'breakpoint', request: false, response: true });
  });

  test('Forward: previews where a request goes', async () => {
    const changes: Action[] = [];
    render(<Harness initial={{ type: 'forward', target: '', preserveHost: false } as Action} changes={changes} />);
    await userEvent.type(screen.getByRole('textbox', { name: /Target/ }), 'https://staging.example.com');
    expect(screen.getByText('https://staging.example.com/api/users')).toBeTruthy();
    await userEvent.type(screen.getAllByRole('textbox')[1] as HTMLElement, '/api');
    expect(changes.at(-1)).toMatchObject({ target: 'https://staging.example.com', stripPrefix: '/api' });
    expect(screen.getByText('https://staging.example.com/users')).toBeTruthy();
  });
});
