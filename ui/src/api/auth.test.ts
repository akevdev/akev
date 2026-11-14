import { describe, expect, test } from 'bun:test';
import { checkStoredToken, resolveAuth, storedTokenWasStale, stripTokenFromUrl, tokenStorageKey, type AuthEnvironment } from './auth';

function env(href: string, extra: Partial<AuthEnvironment> = {}) {
  const store = new Map<string, string>();
  const replaced: string[] = [];
  const url = new URL(href);
  return {
    store,
    replaced,
    env: {
      location: { href, search: url.search, origin: url.origin },
      history: { state: null, replaceState: (_s: unknown, _t: string, u?: string | URL | null) => void replaced.push(String(u)) },
      getStored: (k: string) => store.get(k) ?? null,
      setStored: (k: string, v: string) => void store.set(k, v),
      ...extra,
    } satisfies AuthEnvironment,
  };
}

const DAEMON = 'http://127.0.0.1:45456';

describe('auth', () => {
  test('?token= is remembered for the daemon origin and stripped from the address bar', () => {
    const { env: e, store, replaced } = env(`${DAEMON}/?token=s3cret&mock#/traffic/12?x=1`);
    const auth = resolveAuth(e);
    expect(auth.config).toEqual({ baseUrl: '', token: 's3cret' });
    expect(auth.source).toBe('url');
    expect(store.get(tokenStorageKey(DAEMON))).toBe('s3cret');
    expect(replaced).toEqual([`${DAEMON}/?mock#/traffic/12?x=1`]);
  });

  test('a new tab or a bookmark uses the remembered token without touching the URL', () => {
    const first = env(`${DAEMON}/?token=abc`);
    resolveAuth(first.env);
    const tab = env(`${DAEMON}/#/rules`);
    for (const [k, v] of first.store) tab.store.set(k, v);
    const auth = resolveAuth(tab.env);
    expect(auth.config.token).toBe('abc');
    expect(auth.source).toBe('stored');
    expect(tab.replaced).toEqual([]);
  });

  test('tokens are remembered per daemon origin', () => {
    const a = env(`${DAEMON}/?token=one`);
    resolveAuth(a.env);
    const other = env('http://127.0.0.1:45999/');
    for (const [k, v] of a.store) other.store.set(k, v);
    expect(resolveAuth(other.env).config.token).toBeNull();
    // A dev server talking to a daemon elsewhere keys by the daemon, not the page.
    const dev = env('http://localhost:5173/', { envUrl: `${DAEMON}/` });
    dev.store.set(tokenStorageKey(DAEMON), 'one');
    expect(resolveAuth(dev.env).config).toEqual({ baseUrl: DAEMON, token: 'one' });
  });

  test('no token anywhere', () => {
    const { env: e } = env('http://localhost:5173/');
    expect(resolveAuth(e).config).toEqual({ baseUrl: '', token: null });
    expect(resolveAuth(e).source).toBeNull();
  });

  test('the desktop shell wins and provides the base URL (trailing slash trimmed)', () => {
    const { env: e, store } = env('tauri://localhost/?token=fromurl', { desktop: { token: 'desktop', apiUrl: `${DAEMON}/` } });
    expect(resolveAuth(e)).toMatchObject({ config: { baseUrl: DAEMON, token: 'desktop' }, source: 'desktop' });
    expect(store.size).toBe(0);
  });

  test('VITE_AKEV_URL overrides same-origin in dev', () => {
    const { env: e } = env('http://localhost:5173/', { envUrl: 'http://192.168.1.5:45456' });
    expect(resolveAuth(e).config.baseUrl).toBe('http://192.168.1.5:45456');
  });

  test('stripTokenFromUrl keeps other params and the hash', () => {
    expect(stripTokenFromUrl('http://h/?a=1&token=x&b#/rules/9')).toBe('http://h/?a=1&b#/rules/9');
    expect(stripTokenFromUrl('http://h/?token=x')).toBe('http://h/');
  });
});

describe('checkStoredToken', () => {
  const stored = { config: { baseUrl: '', token: 'old' }, source: 'stored' as const, storageKey: tokenStorageKey(DAEMON) };

  test('only remembered tokens are checked', async () => {
    let probed = false;
    const probe = async () => {
      probed = true;
      return true;
    };
    expect(await checkStoredToken({ ...stored, source: 'url' }, probe)).toBe('unknown');
    expect(await checkStoredToken({ ...stored, config: { baseUrl: '', token: null }, source: null }, probe)).toBe('unknown');
    expect(probed).toBe(false);
    expect(await checkStoredToken(stored, probe)).toBe('valid');
  });

  test("when Akev can't be reached the token is kept", async () => {
    const removed: string[] = [];
    const check = await checkStoredToken(
      stored,
      async () => {
        throw new Error('connection refused');
      },
      (k) => void removed.push(k),
    );
    expect(check).toBe('unknown');
    expect(removed).toEqual([]);
  });

  test('a rejected token is forgotten and reported as stale', async () => {
    const removed: string[] = [];
    expect(storedTokenWasStale()).toBe(false);
    expect(await checkStoredToken(stored, async () => false, (k) => void removed.push(k))).toBe('stale');
    expect(removed).toEqual([stored.storageKey]);
    expect(storedTokenWasStale()).toBe(true);
  });
});
