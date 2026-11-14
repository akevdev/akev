import { describe, expect, test } from 'bun:test';
import type { RuleSet, Settings } from '../../api/types';
import { bindingId } from '../../app/shortcuts';
import { findConflicts, groupShortcuts, referenceDocs } from './shortcutCatalog';
import { SettingsSync } from './settingsSync';
import { addEntries, draftError, initialListState, makeListReducer, replaceEntry, splitEntries } from './stringList';
import { buildConfigExport, mergeSettings, omittedPassphrases, parseConfigImport } from './transfer';
import {
  bytesToMb,
  findDuplicate,
  isIpv4,
  isIpv6,
  listenerOverrides,
  mbToBytes,
  normalizeBind,
  normalizeHost,
  portWarning,
  validateAbsolutePath,
  validateBindAddress,
  validateDohUrl,
  validateHostPattern,
  validateIpAddress,
  validatePacUrl,
  parsePort,
  validatePort,
  validatePortText,
  validateProxyUrl,
  validateTimeout,
} from './validation';

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function settings(): Settings {
  return {
    proxy: {
      port: 8000,
      bindAddress: '127.0.0.1',
      http2: true,
      socks: true,
      tlsPassthrough: ['*.apple.com'],
      ignoreUpstreamCertErrors: [],
      trustedCaPaths: [],
      clientCertificates: [],
      upstreamProxy: { mode: 'direct', url: '', noProxy: ['localhost'] },
      dnsOverrides: [],
      dohUrl: null,
      connectTimeoutMs: 15000,
      responseTimeoutMs: 0,
      bodyIdleTimeoutMs: 300_000,
      ruleBodyBudgetBytes: 1024 ** 3,
      preserveHeaderCase: true,
      tlsProfile: { profile: 'native', overrides: [] },
      wireguard: { enabled: false, port: 51820 },
      transparent: { enabled: false, port: 8001, mark: 0 },
      http3: { upstream: 'off', listen: false, listenPort: 0 },
      access: { policy: 'private', allow: [], block: [], username: '', password: '', allowLoopbackTargets: false, trustProxyProtocol: false },
    },
    capture: {
      maxBodyBytes: 1024,
      memoryBudgetBytes: 2048,
      maxExchanges: 0,
      spillMaxBytes: 4 * 1024 ** 3,
      eventQueueBytes: 64 * 1024 ** 2,
      recordTunnels: true,
      recordTlsFailures: true,
      ignoreHosts: [],
      detectProcesses: true,
    },
    sessions: { autosave: true, autosaveIntervalSecs: 60, restoreOnStartup: true, autosaveMaxExchanges: 20_000, autosaveMaxBytes: 256 * 1024 ** 2 },
    updates: { checkAutomatically: true },
    breakpoints: { maxPaused: 100, autoReleaseSecs: 0 },
    ui: { theme: 'dark' },
  };
}

describe('validation', () => {
  test('host patterns', () => {
    for (const ok of ['example.com', '*.example.com', 'api.*.internal', '*', 'localhost', 'localhost:3000', '10.0.0.1', '::1', '[::1]:8080', 'my_host.local', 'xn--bcher-kva.example']) {
      expect(validateHostPattern(ok)).toBeNull();
    }
    expect(validateHostPattern('')).toContain('Enter a hostname');
    expect(validateHostPattern('https://example.com')).toContain('scheme');
    expect(validateHostPattern('example.com/path')).toContain('path');
    expect(validateHostPattern('exa mple.com')).toContain('spaces');
    expect(validateHostPattern('example..com')).toContain('empty label');
    expect(validateHostPattern('-bad.com')).toContain('hyphen');
    expect(validateHostPattern('example.com:99999')).toContain('Port');
    expect(validateHostPattern('ex!ample.com')).toContain("aren't allowed");
    expect(validateHostPattern('user@host')).toContain('credentials');
  });

  test('ip addresses', () => {
    expect(isIpv4('192.168.1.1')).toBe(true);
    expect(isIpv4('256.1.1.1')).toBe(false);
    expect(isIpv4('01.1.1.1')).toBe(false);
    expect(isIpv6('::1')).toBe(true);
    expect(isIpv6('2001:db8::8a2e:370:7334')).toBe(true);
    expect(isIpv6('::ffff:192.168.1.1')).toBe(true);
    expect(isIpv6('1::2::3')).toBe(false);
    expect(isIpv6('12345::')).toBe(false);
    expect(validateIpAddress('1.2.3.4')).toBeNull();
    expect(validateIpAddress('nope')).toContain('IPv4');
    expect(validateBindAddress('0.0.0.0')).toBeNull();
    expect(validateBindAddress('localhost')).toBeNull();
    expect(validateBindAddress('example.com')).not.toBeNull();
  });

  test('ports and timeouts', () => {
    expect(validatePort(8000)).toBeNull();
    expect(validatePort(0)).not.toBeNull();
    expect(validatePort(70000)).not.toBeNull();
    expect(validatePort(null)).not.toBeNull();
    // Typed text is validated, never clamped.
    expect(parsePort(' 8001 ')).toBe(8001);
    expect(parsePort('80a')).toBeNull();
    expect(validatePortText('8001')).toBeNull();
    expect(validatePortText('99999')).toContain('between 1 and 65535');
    expect(validatePortText('0')).toContain('between 1 and 65535');
    expect(validatePortText('')).toContain('Enter a port');
    expect(validatePortText('80-90')).toContain('digits');
    expect(portWarning(80)).toContain('administrator');
    expect(portWarning(8080)).toBeNull();
    expect(validateTimeout(0, { allowZero: true })).toBeNull();
    expect(validateTimeout(0)).not.toBeNull();
    expect(validateTimeout(-1, { allowZero: true })).not.toBeNull();
    expect(validateTimeout(10, { max: 5 })).toContain('at most');
  });

  test('urls and paths', () => {
    expect(validateProxyUrl('http://proxy.local:3128')).toBeNull();
    expect(validateProxyUrl('socks5h://user:pw@10.0.0.1:1080')).toBeNull();
    expect(validateProxyUrl('proxy.local:3128')).toContain('scheme');
    expect(validateProxyUrl('ftp://proxy')).not.toBeNull();
    expect(validateProxyUrl('http://proxy/path')).toContain('path');
    expect(validateDohUrl('')).toBeNull();
    expect(validateDohUrl('https://cloudflare-dns.com/dns-query')).toBeNull();
    expect(validateDohUrl('http://insecure/dns-query')).toContain('https');
    expect(validatePacUrl('file:///etc/proxy.pac')).toBeNull();
    expect(validatePacUrl('http://wpad/wpad.dat')).toBeNull();
    expect(validateAbsolutePath('/etc/ssl/ca.pem')).toBeNull();
    expect(validateAbsolutePath('~/ca.pem')).toBeNull();
    expect(validateAbsolutePath('C:\\certs\\ca.pem')).toBeNull();
    expect(validateAbsolutePath('\\\\server\\share\\ca.pem')).toBeNull();
    expect(validateAbsolutePath('ca.pem')).toContain('absolute');
  });

  test('helpers', () => {
    expect(normalizeHost('  API.Example.COM. ')).toBe('api.example.com');
    // Pasted URLs become hosts (what people copy from a browser or the traffic list).
    expect(normalizeHost('https://API.example.com/v1/users?x=1#y')).toBe('api.example.com');
    expect(normalizeHost('http://user:pw@localhost:3000/')).toBe('localhost:3000');
    expect(normalizeHost('admin@intranet.corp')).toBe('intranet.corp');
    expect(normalizeHost('*.Example.com')).toBe('*.example.com');
    expect(normalizeHost('example.com/path')).toBe('example.com/path'); // no scheme: left for validation to explain
    expect(findDuplicate(['a.com', 'B.com'], 'b.com', normalizeHost)).toBe(1);
    expect(findDuplicate(['a.com', 'b.com'], 'b.com', normalizeHost, 1)).toBe(-1);
    expect(bytesToMb(32 * 1024 * 1024)).toBe(32);
    expect(mbToBytes(1.5)).toBe(1572864);
  });
});

describe('string list editor', () => {
  const opts = { validate: validateHostPattern, normalize: normalizeHost };

  test('splitEntries', () => {
    expect(splitEntries('a.com, b.com\nc.com  d.com;e.com')).toEqual(['a.com', 'b.com', 'c.com', 'd.com', 'e.com']);
    expect(splitEntries('   ')).toEqual([]);
  });

  test('addEntries validates, normalises and de-duplicates', () => {
    const r = addEntries(['a.com'], 'B.com a.com ex!ample.com https://x.com/path c.com', opts);
    expect(r.items).toEqual(['a.com', 'b.com', 'x.com', 'c.com']);
    expect(r.added).toEqual(['b.com', 'x.com', 'c.com']); // pasted URLs become hosts
    expect(r.rejected).toEqual(['a.com', 'ex!ample.com']);
    expect(r.error).toContain('2 of 5 entries skipped');
    const single = addEntries(['a.com'], 'a.com', opts);
    expect(single.error).toBe('Already in the list');
  });

  test('replaceEntry', () => {
    expect(replaceEntry(['a.com', 'b.com'], 0, 'z.com', opts)).toEqual({ items: ['z.com', 'b.com'], error: null });
    expect(replaceEntry(['a.com', 'b.com'], 0, 'b.com', opts).error).toBe('Already in the list');
    expect(replaceEntry(['a.com', 'b.com'], 0, 'bad host', opts).error).toContain('spaces');
    expect(replaceEntry(['a.com', 'b.com'], 0, '  ', opts).items).toEqual(['b.com']);
  });

  test('reducer: add, edit, cancel, remove, restore, reset', () => {
    const reduce = makeListReducer(opts);
    let s = initialListState(['a.com']);
    s = reduce(s, { type: 'draft', text: 'b.com' });
    s = reduce(s, { type: 'add' });
    expect(s.items).toEqual(['a.com', 'b.com']);
    expect(s.draft).toBe('');
    // A multi-entry paste adds the valid entries and keeps the rejected ones in the draft.
    s = reduce(s, { type: 'draft', text: 'ok.com bad!host' });
    s = reduce(s, { type: 'add' });
    expect(s.items).toEqual(['a.com', 'b.com', 'ok.com']);
    expect(s.draft).toBe('bad!host');
    expect(s.draftError).toContain('1 of 2 entries skipped');
    s = reduce(s, { type: 'draft', text: 'bad.com/path' });
    s = reduce(s, { type: 'add' });
    expect(s.draftError).toContain('path');
    expect(s.draft).toBe('bad.com/path');
    s = reduce(s, { type: 'edit-start', index: 0 });
    s = reduce(s, { type: 'edit-text', text: 'b.com' });
    s = reduce(s, { type: 'edit-commit' });
    expect(s.editing?.error).toBe('Already in the list');
    s = reduce(s, { type: 'edit-text', text: 'c.com' });
    s = reduce(s, { type: 'edit-commit' });
    expect(s.editing).toBeNull();
    expect(s.items[0]).toBe('c.com');
    s = reduce(s, { type: 'edit-start', index: 1 });
    s = reduce(s, { type: 'edit-cancel' });
    expect(s.editing).toBeNull();
    s = reduce(s, { type: 'remove', index: 0 });
    expect(s.items).toEqual(['b.com', 'ok.com']);
    s = reduce(s, { type: 'restore', index: 0, value: 'c.com' });
    expect(s.items).toEqual(['c.com', 'b.com', 'ok.com']);
    s = reduce(s, { type: 'reset', items: ['x.com'] });
    expect(s.items).toEqual(['x.com']);
  });

  test('draftError while typing', () => {
    expect(draftError(['a.com'], 'a.com', opts)).toBe('Already in the list');
    expect(draftError(['a.com'], 'b.com', opts)).toBeNull();
    expect(draftError(['a.com'], 'b.com/x', opts)).toContain('path');
    expect(draftError(['a.com'], 'https://A.com/x', opts)).toBe('Already in the list'); // URL → host
    expect(draftError(['a.com'], 'b.com c.com', opts)).toBeNull();
    expect(draftError([], '', opts)).toBeNull();
  });
});

describe('SettingsSync', () => {
  test('applies edits to the draft immediately and saves', async () => {
    const saved: Settings[] = [];
    const sync = new SettingsSync<Settings>({ save: async (s) => (saved.push(s), s), savedLingerMs: 5 });
    sync.setServer(settings());
    const p = sync.edit((s) => ({ ...s, proxy: { ...s.proxy, http2: false } }));
    expect(sync.getSnapshot().draft?.proxy.http2).toBe(false);
    expect(sync.getSnapshot().phase).toBe('pending');
    await p;
    expect(saved).toHaveLength(1);
    expect(sync.getSnapshot().phase).toBe('saved');
    await sleep(15);
    expect(sync.getSnapshot().phase).toBe('idle');
  });

  test('debounced edits coalesce into one save', async () => {
    const saved: Settings[] = [];
    const sync = new SettingsSync<Settings>({ save: async (s) => (saved.push(s), s), debounceMs: 20 });
    sync.setServer(settings());
    sync.edit((s) => ({ ...s, proxy: { ...s.proxy, dohUrl: 'h' } }), { debounce: true });
    sync.edit((s) => ({ ...s, proxy: { ...s.proxy, dohUrl: 'ht' } }), { debounce: true });
    const last = sync.edit((s) => ({ ...s, proxy: { ...s.proxy, dohUrl: 'https://x' } }), { debounce: true });
    await last;
    expect(saved).toHaveLength(1);
    expect(saved[0]?.proxy.dohUrl).toBe('https://x');
  });

  test('serialises saves and sends the latest draft', async () => {
    const saved: Settings[] = [];
    let release: () => void = () => undefined;
    const sync = new SettingsSync<Settings>({
      save: (s) =>
        new Promise((resolve) => {
          saved.push(s);
          release = () => resolve(s);
        }),
    });
    sync.setServer(settings());
    const a = sync.edit((s) => ({ ...s, proxy: { ...s.proxy, http2: false } }));
    await sleep(5);
    expect(saved).toHaveLength(1);
    const b = sync.edit((s) => ({ ...s, proxy: { ...s.proxy, socks: false } }));
    const c = sync.edit((s) => ({ ...s, proxy: { ...s.proxy, preserveHeaderCase: false } }));
    await sleep(5);
    expect(saved).toHaveLength(1); // still in flight
    release();
    await a;
    await sleep(5);
    expect(saved).toHaveLength(2);
    expect(saved[1]?.proxy).toMatchObject({ http2: false, socks: false, preserveHeaderCase: false });
    release();
    await Promise.all([b, c]);
    expect(sync.getSnapshot().draft?.proxy.socks).toBe(false);
  });

  test('reverts to the server copy and rejects on failure', async () => {
    const errors: unknown[] = [];
    const sync = new SettingsSync<Settings>({ save: async () => {
        throw new Error('port in use');
      }, onError: (e) => errors.push(e) });
    const server = settings();
    sync.setServer(server);
    const p = sync.edit((s) => ({ ...s, proxy: { ...s.proxy, port: 9000 } }));
    await expect(p).rejects.toThrow('port in use');
    expect(sync.getSnapshot().draft?.proxy.port).toBe(8000);
    expect(sync.getSnapshot().phase).toBe('error');
    expect(sync.getSnapshot().error).toBe('port in use');
    expect(errors).toHaveLength(1);
  });

  test('server broadcasts replace the draft only when idle', async () => {
    let release: () => void = () => undefined;
    const sync = new SettingsSync<Settings>({ save: (s) => new Promise((resolve) => (release = () => resolve(s))) });
    sync.setServer(settings());
    const p = sync.edit((s) => ({ ...s, capture: { ...s.capture, recordTunnels: false } }));
    await sleep(1);
    const remote = settings();
    remote.capture.detectProcesses = false;
    sync.setServer(remote); // arrives mid-flight: local draft kept
    expect(sync.getSnapshot().draft?.capture.recordTunnels).toBe(false);
    release();
    await p;
    sync.setServer(remote); // idle: replaced
    expect(sync.getSnapshot().draft?.capture.detectProcesses).toBe(false);
  });

  test('prepare transforms the payload', async () => {
    const saved: Settings[] = [];
    const sync = new SettingsSync<Settings>({ save: async (s) => (saved.push(s), s), prepare: (s) => ({ ...s, ui: { fresh: true } }) });
    sync.setServer(settings());
    await sync.edit((s) => s);
    expect(saved[0]?.ui).toEqual({ fresh: true });
  });

  test('edit before load rejects', async () => {
    const sync = new SettingsSync<Settings>({ save: async (s) => s });
    await expect(sync.edit((s) => s)).rejects.toThrow('not loaded');
  });
});

describe('config transfer', () => {
  const rules: RuleSet = { version: 1, items: [{ type: 'group', id: 'g', title: 'G', enabled: true, collapsed: false, items: [{ type: 'rule', id: 'r', enabled: true, matchers: [], action: { type: 'close' } }] }] };

  test('export round-trips through import', () => {
    const exp = buildConfigExport({ settings: settings(), rules, ui: { theme: 'light' }, appVersion: '1.0' }, new Date('2026-01-02T03:04:05Z'));
    expect(exp.exportedAt).toBe('2026-01-02T03:04:05.000Z');
    expect(exp.settings).not.toHaveProperty('ui');
    const parsed = parseConfigImport(JSON.stringify(exp));
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.summary).toEqual(['proxy & capture settings', '1 rule', 'appearance & layout preferences']);
    expect(parsed.config.rules).toEqual(rules);
  });

  test('exports never contain passphrases; imports keep the local ones', () => {
    const cur = settings();
    cur.proxy.clientCertificates = [
      { host: '*.corp', path: '/c.p12', passphrase: 'hunter2' },
      { host: 'api.x', path: '/x.pem' },
    ];
    expect(omittedPassphrases(cur)).toBe(1);
    const exp = buildConfigExport({ settings: cur });
    expect(JSON.stringify(exp)).not.toContain('hunter2');
    expect(exp.settings?.proxy.clientCertificates).toEqual([
      { host: '*.corp', path: '/c.p12' },
      { host: 'api.x', path: '/x.pem' },
    ]);
    const merged = mergeSettings(cur, exp.settings as unknown as Record<string, unknown>);
    expect(merged.proxy.clientCertificates[0]).toEqual({ host: '*.corp', path: '/c.p12', passphrase: 'hunter2' });
    // An explicit passphrase in the file wins.
    const explicit = mergeSettings(cur, { proxy: { clientCertificates: [{ host: '*.corp', path: '/c.p12', passphrase: 'new' }] } });
    expect(explicit.proxy.clientCertificates[0]?.passphrase).toBe('new');
  });

  test('accepts bare settings or rules, rejects junk', () => {
    expect(parseConfigImport(JSON.stringify(rules))).toMatchObject({ ok: true, summary: ['1 rule'] });
    expect(parseConfigImport(JSON.stringify(settings()))).toMatchObject({ ok: true });
    expect(parseConfigImport('{nope')).toEqual({ ok: false, error: "This file isn't valid JSON." });
    expect(parseConfigImport('{"hello":1}').ok).toBe(false);
  });

  test('mergeSettings type-checks every key', () => {
    const cur = settings();
    const merged = mergeSettings(cur, {
      proxy: { port: 9001, http2: 'yes', tlsPassthrough: ['a.com', 3, ''], upstreamProxy: { mode: 'bogus', url: 'http://p:1' }, dohUrl: '', clientCertificates: [{ host: 'x', path: '/p' }, { host: 1 }] },
      capture: { maxExchanges: -5, recordTunnels: false },
    });
    expect(merged.proxy.port).toBe(9001);
    expect(merged.proxy.http2).toBe(true);
    expect(merged.proxy.tlsPassthrough).toEqual(['a.com']);
    expect(merged.proxy.upstreamProxy).toEqual({ mode: 'direct', url: 'http://p:1', noProxy: ['localhost'] });
    expect(merged.proxy.dohUrl).toBeNull();
    expect(merged.proxy.clientCertificates).toEqual([{ host: 'x', path: '/p' }]);
    expect(merged.capture.maxExchanges).toBe(0);
    expect(merged.capture.recordTunnels).toBe(false);
    expect(merged.ui).toEqual(cur.ui);
  });

  /** Settings with every section away from its default. */
  function customized(): Settings {
    const s = settings();
    s.proxy.tlsProfile = { profile: 'chrome', overrides: [{ host: '*.shop.example', profile: 'firefox' }] };
    s.proxy.wireguard = { enabled: true, port: 51999, bindAddress: '192.168.1.5' };
    s.proxy.transparent = { enabled: true, port: 8101, mark: 0x1a4b };
    s.proxy.http3 = { upstream: 'prefer', listen: true, listenPort: 8443 };
    s.proxy.access = { policy: 'allowlist', allow: ['10.0.0.0/8'], block: ['10.0.0.9'], username: 'me', password: 's3cret', allowLoopbackTargets: true, trustProxyProtocol: true };
    s.updates = { checkAutomatically: false };
    return s;
  }

  test('importing a file keeps every setting it does not contain (the result replaces them all)', () => {
    const cur = customized();
    // A file with only some settings, e.g. hand-written or from a smaller setup.
    const merged = mergeSettings(cur, { proxy: { port: 9001 }, capture: { maxExchanges: 5 } });
    expect(merged).toEqual({ ...cur, proxy: { ...cur.proxy, port: 9001 }, capture: { ...cur.capture, maxExchanges: 5 } });
    // Sections present but partial keep their other fields.
    const partial = mergeSettings(cur, { proxy: { access: { policy: 'any' }, http3: { listen: false }, wireguard: { port: 51820 } }, updates: {} });
    expect(partial.proxy.access).toEqual({ ...cur.proxy.access, policy: 'any' });
    expect(partial.proxy.http3).toEqual({ ...cur.proxy.http3, listen: false });
    expect(partial.proxy.wireguard).toEqual({ ...cur.proxy.wireguard, port: 51820 });
    expect(partial.updates).toEqual(cur.updates);
  });

  test('an export imported elsewhere brings every section, type-checked', () => {
    const exp = buildConfigExport({ settings: customized() });
    const merged = mergeSettings(settings(), exp.settings as unknown as Record<string, unknown>);
    expect(merged).toEqual({ ...customized(), ui: settings().ui });
    const bad = mergeSettings(customized(), {
      proxy: {
        tlsProfile: { profile: 'opera', overrides: [{ host: 'a', profile: 'safari' }, { host: 'b', profile: 'opera' }, { profile: 'edge' }] },
        wireguard: { enabled: 'yes', port: 70000, bindAddress: '' },
        transparent: { port: -1, mark: 2 ** 33 },
        http3: { upstream: 'always', listenPort: 443 },
        access: { policy: 'everyone', allow: ['1.2.3.4', 5], username: 7, trustProxyProtocol: false },
      },
      updates: { checkAutomatically: 'no' },
    });
    const cur = customized();
    expect(bad.proxy.tlsProfile).toEqual({ profile: 'chrome', overrides: [{ host: 'a', profile: 'safari' }] });
    expect(bad.proxy.wireguard).toEqual({ enabled: true, port: 51999 });
    expect(bad.proxy.transparent).toEqual(cur.proxy.transparent);
    expect(bad.proxy.http3).toEqual({ ...cur.proxy.http3, listenPort: 443 });
    expect(bad.proxy.access).toEqual({ ...cur.proxy.access, allow: ['1.2.3.4'], trustProxyProtocol: false });
    expect(bad.updates).toEqual(cur.updates);
  });
});

describe('shortcut catalog', () => {
  const reference = referenceDocs({}, false);

  test('live docs win, groups are ordered, actions merge their keys, query filters', () => {
    const live = [
      { combo: 'mod+shift+x', description: 'Clear traffic (undoable)', group: 'Traffic', binding: bindingId('Traffic', 'Clear traffic (undoable)', 'mod+shift+x'), defaultCombo: 'mod+shift+x' },
      { combo: 'mod+1', description: 'Go to Traffic', group: 'Navigation' },
      { combo: 'shift+?', description: 'Keyboard shortcuts', group: 'General' },
    ];
    const groups = groupShortcuts(live, reference);
    expect(groups[0]?.group).toBe('General');
    expect(groups[1]?.group).toBe('Navigation');
    const traffic = groups.find((g) => g.group === 'Traffic');
    expect(traffic?.items.filter((e) => e.description === 'Clear traffic (undoable)')).toHaveLength(1);
    // Two registrations of one action are one row with both keys.
    const focus = traffic?.items.find((e) => e.description === 'Focus the filter');
    expect(focus?.bindings.map((b) => b.combo)).toEqual(['mod+f', '/']);
    const filtered = groupShortcuts(live, reference, 'compose send');
    expect(filtered).toEqual([{ group: 'Compose', items: [{ group: 'Compose', description: 'Send request', bindings: [{ combo: 'mod+enter', binding: bindingId('Compose', 'Send request', 'mod+enter'), defaultCombo: 'mod+enter' }] }] }]);
    expect(groupShortcuts(live, reference, 'Ctrl', (c) => c.replace('mod', 'Ctrl')).length).toBeGreaterThan(0);
  });

  test('reference entries carry their bindings and show rebound keys; list keys stay fixed', () => {
    const id = bindingId('Traffic list', 'Replay', 'mod+r');
    const docs = referenceDocs({ [id]: 'mod+shift+y' }, false);
    expect(docs.find((d) => d.binding === id)).toMatchObject({ combo: 'mod+shift+y', defaultCombo: 'mod+r' });
    const arrows = docs.find((d) => d.description === 'Move selection');
    expect(arrows?.binding).toBeUndefined();
    // Compose tab keys follow the platform (the browser keeps Ctrl+T for itself).
    expect(referenceDocs({}, true).find((d) => d.description === 'New request tab')?.combo).toBe('mod+t');
    expect(reference.find((d) => d.description === 'New request tab')?.combo).toBe('alt+t');
  });

  test('conflicts: same keys where both can be active', () => {
    const entries = groupShortcuts([{ combo: 'mod+k', description: 'Command palette', group: 'General', binding: bindingId('General', 'Command palette', 'mod+k'), defaultCombo: 'mod+k' }], reference).flatMap((g) => g.items);
    const replay = bindingId('Traffic list', 'Replay', 'mod+r');
    // A global shortcut conflicts everywhere; ctrl+k is mod+k off macOS.
    expect(findConflicts('ctrl+k', replay, 'Traffic list', entries, false).map((e) => e.description)).toEqual(['Command palette']);
    expect(findConflicts('ctrl+k', replay, 'Traffic list', entries, true)).toEqual([]);
    // Traffic shortcuts don't clash with Compose ones (different pages)…
    expect(findConflicts('mod+l', replay, 'Traffic list', entries, false)).toEqual([]);
    // …but body viewer keys are active on the Traffic page.
    expect(findConflicts('alt+z', replay, 'Traffic list', entries, false).map((e) => e.description)).toEqual(['Toggle line wrap']);
    // A shortcut never conflicts with itself.
    expect(findConflicts('mod+r', replay, 'Traffic list', entries, false)).toEqual([]);
    // Fixed list keys count too.
    expect(findConflicts('enter', replay, 'Traffic list', entries, false).map((e) => e.description)).toEqual(['Focus the detail pane']);
  });
});

describe('listener overrides (Status.proxy vs stored settings)', () => {
  const at = (port: number, bindAddress: string) => ({ port, bindAddress });
  test('bind addresses compare the way the daemon normalizes them', () => {
    expect(normalizeBind(' localhost ')).toBe('127.0.0.1');
    expect(normalizeBind('')).toBe('127.0.0.1');
    expect(normalizeBind('[::]')).toBe('::');
    expect(listenerOverrides(at(8000, 'localhost'), at(8000, '127.0.0.1'))).toEqual({ port: null, bindAddress: null });
    expect(listenerOverrides(at(8000, '[::]'), at(8000, '::'))).toEqual({ port: null, bindAddress: null });
  });
  test('a busy or --proxy-port port and a --proxy-bind address are reported separately', () => {
    expect(listenerOverrides(at(8000, '127.0.0.1'), at(8001, '127.0.0.1'))).toEqual({ port: 8000, bindAddress: null });
    expect(listenerOverrides(at(8000, '127.0.0.1'), at(8000, '0.0.0.0'))).toEqual({ port: null, bindAddress: '127.0.0.1' });
    expect(listenerOverrides(at(8000, '127.0.0.1'), at(9000, '0.0.0.0'))).toEqual({ port: 8000, bindAddress: '127.0.0.1' });
  });
  test('a stored port of 0 (any free port) is never an override', () => {
    expect(listenerOverrides(at(0, '127.0.0.1'), at(41234, '127.0.0.1')).port).toBeNull();
  });
});
