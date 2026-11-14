import { afterEach, describe, expect, test } from 'bun:test';
import { canPickNativePath, externalLinkTarget, FILE_NAME_HEADER, listenDesktopEvent, pickNativePath, resetTauriBridgeForTests, tauriBridge, windowInBackground } from './platform';

type Call = { cmd: string; args: unknown; options?: { headers?: Record<string, string> } };

function fakeTauri(result: unknown = true): Call[] {
  const calls: Call[] = [];
  (globalThis as { window?: unknown }).window = {
    __TAURI_INTERNALS__: {
      invoke: async (cmd: string, args?: unknown, options?: Call['options']) => {
        calls.push({ cmd, args, options });
        return result;
      },
    },
  };
  resetTauriBridgeForTests();
  return calls;
}

// The test preload provides a DOM `window`; these tests swap in fakes and put it back.
const realWindow = (globalThis as { window?: unknown }).window;

afterEach(() => {
  if (realWindow === undefined) delete (globalThis as { window?: unknown }).window;
  else (globalThis as { window?: unknown }).window = realWindow;
  resetTauriBridgeForTests();
});

describe('desktop bridge over Tauri IPC', () => {
  test('absent outside Tauri', () => {
    (globalThis as { window?: unknown }).window = {};
    expect(tauriBridge()).toBeUndefined();
  });

  test('opens links with the opener plugin', async () => {
    const calls = fakeTauri();
    await tauriBridge()?.openExternal?.('https://akev.dev');
    expect(calls).toEqual([{ cmd: 'plugin:opener|open_url', args: { url: 'https://akev.dev' }, options: undefined }]);
  });

  test('saves data through the shell command, with the name in a header', async () => {
    const calls = fakeTauri(true);
    const saved = await tauriBridge()?.saveFile?.({ suggestedName: 'résumé v2.json', data: '{"a":1}' });
    expect(saved).toBe(true);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.cmd).toBe('save_file');
    expect(new TextDecoder().decode(calls[0]?.args as Uint8Array)).toBe('{"a":1}');
    expect(calls[0]?.options?.headers?.[FILE_NAME_HEADER]).toBe(encodeURIComponent('résumé v2.json'));
  });

  test('a cancelled save resolves false', async () => {
    fakeTauri(false);
    expect(await tauriBridge()?.saveFile?.({ suggestedName: 'x.bin', data: new Uint8Array([1, 2]) })).toBe(false);
  });

  test('notifications go to the notification plugin', () => {
    const calls = fakeTauri();
    tauriBridge()?.notify?.('Request paused', 'GET example.com/');
    expect(calls).toEqual([{ cmd: 'plugin:notification|notify', args: { options: { title: 'Request paused', body: 'GET example.com/' } }, options: undefined }]);
  });
});

describe('externalLinkTarget', () => {
  const base = 'http://127.0.0.1:45456/#/settings/about';
  test('other sites and mail links open outside', () => {
    expect(externalLinkTarget('https://akev.dev/docs', base)).toBe('https://akev.dev/docs');
    expect(externalLinkTarget('http://example.com', base)).toBe('http://example.com/');
    expect(externalLinkTarget('mailto:hi@example.com', base)).toBe('mailto:hi@example.com');
  });
  test('the app itself, anchors and other schemes stay', () => {
    expect(externalLinkTarget('#/traffic', base)).toBeNull();
    expect(externalLinkTarget('/api/export/har', base)).toBeNull();
    expect(externalLinkTarget('http://127.0.0.1:45456/x', base)).toBeNull();
    expect(externalLinkTarget('javascript:alert(1)', base)).toBeNull();
    expect(externalLinkTarget('file:///etc/passwd', base)).toBeNull();
    expect(externalLinkTarget('http://[bad', base)).toBeNull();
  });
});

describe('native pickers, events and notifications', () => {
  test('the native path picker is desktop-only', async () => {
    (globalThis as { window?: unknown }).window = {};
    expect(canPickNativePath()).toBe(false);
    expect(await pickNativePath({ directory: true })).toBeNull();
  });

  test('picks a folder or a file through the dialog plugin', async () => {
    const calls = fakeTauri('/home/me/site/dist');
    expect(canPickNativePath()).toBe(true);
    expect(await pickNativePath({ directory: true })).toBe('/home/me/site/dist');
    expect(calls[0]).toEqual({ cmd: 'plugin:dialog|open', args: { options: { directory: true, multiple: false } }, options: undefined });
    fakeTauri(null);
    expect(await pickNativePath({ directory: false, title: 'Pick' })).toBeNull();
  });

  test('desktop events go through the global event API, and unsubscribe', async () => {
    const handlers = new Map<string, (e: { payload: unknown }) => void>();
    let unlistened = 0;
    (globalThis as { window?: unknown }).window = {
      __TAURI__: {
        event: {
          listen: async (name: string, h: (e: { payload: unknown }) => void) => {
            handlers.set(name, h);
            return () => unlistened++;
          },
        },
      },
    };
    const got: unknown[] = [];
    const off = listenDesktopEvent('akev://menu', (p) => got.push(p));
    await Promise.resolve();
    handlers.get('akev://menu')?.({ payload: 'nav-rules' });
    expect(got).toEqual(['nav-rules']);
    off();
    expect(unlistened).toBe(1);
    // A browser has no event API: listening is a no-op.
    (globalThis as { window?: unknown }).window = {};
    expect(() => listenDesktopEvent('akev://menu', () => {})()).not.toThrow();
  });

  test('notify when the window is hidden or just not focused', () => {
    expect(windowInBackground({ visibilityState: 'hidden', hasFocus: () => false })).toBe(true);
    expect(windowInBackground({ visibilityState: 'visible', hasFocus: () => false })).toBe(true);
    expect(windowInBackground({ visibilityState: 'visible', hasFocus: () => true })).toBe(false);
  });
});
