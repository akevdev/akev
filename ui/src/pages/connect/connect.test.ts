import { describe, expect, test } from 'bun:test';
import type { InterceptorInfo, Status } from '../../api/types';
import { deviceSetupUrl, formatHostPort, isLanBind, lanAddresses, splitHostPort } from './addresses';
import { canUsePrimary, countByCategory, filterCatalog, iconKey, isDormant, panelKindFor, primaryActionFor, shortVersion, sortCatalog, splitDormant, statusOf } from './catalog';
import { describeDevice, describeDeviceState, parseActivation, parseMetadata, type AdbDevice } from './metadata';
import { rankByQuery, scoreFields, subsequenceScore, tokenMatch, withinOneEdit } from './search';
import { caFilePath, commonName, curlAuthConfig, defaultShell, dockerRunSnippet, envSnippet, manualGuides, terminalOneLiner, terminalResetCommand, terminalTestCommand, trustGuide } from './snippets';

function ic(p: Partial<InterceptorInfo> & Pick<InterceptorInfo, 'id' | 'name' | 'category'>): InterceptorInfo {
  return { description: '', tags: [], icon: p.id, supported: true, available: true, active: false, needsConfig: false, ...p };
}

const CATALOG: InterceptorInfo[] = [
  ic({ id: 'manual', name: 'Manual setup', category: 'manual', description: 'Configure anything by hand', needsConfig: true }),
  ic({ id: 'chrome', name: 'Chrome', category: 'browser', description: 'Launch a fresh Chrome window', tags: ['chromium', 'google'], version: '131.0.6778.86' }),
  ic({ id: 'firefox', name: 'Firefox', category: 'browser', description: 'Launch a fresh Firefox profile', tags: ['mozilla'] }),
  ic({ id: 'edge', name: 'Edge', category: 'browser', available: false, unavailableReason: 'Microsoft Edge is not installed' }),
  ic({ id: 'fresh-terminal', name: 'Fresh terminal', category: 'terminal', description: 'Open a new terminal window' }),
  ic({ id: 'existing-terminal', name: 'Existing terminal', category: 'terminal', needsConfig: true }),
  ic({ id: 'android-adb', name: 'Android device', category: 'mobile', needsConfig: true, description: 'Intercept an Android device via ADB' }),
  ic({ id: 'ios', name: 'iOS device', category: 'mobile', needsConfig: true }),
  ic({ id: 'docker', name: 'Docker container', category: 'container', needsConfig: true }),
  ic({ id: 'system-proxy', name: 'System proxy', category: 'system' }),
];

describe('search', () => {
  test('withinOneEdit covers substitution, insertion, deletion and transposition', () => {
    expect(withinOneEdit('android', 'android')).toBe(true);
    expect(withinOneEdit('andriod', 'android')).toBe(true); // transposition
    expect(withinOneEdit('dokcer', 'docker')).toBe(true);
    expect(withinOneEdit('chrom', 'chrome')).toBe(true); // insertion
    expect(withinOneEdit('chromee', 'chrome')).toBe(true); // deletion
    expect(withinOneEdit('chrxme', 'chrome')).toBe(true); // substitution
    expect(withinOneEdit('chrxxe', 'chrome')).toBe(false);
    expect(withinOneEdit('ab', 'abcd')).toBe(false);
  });

  test('subsequenceScore rewards tight matches', () => {
    expect(subsequenceScore('firefox', 'ffx')).toBeGreaterThan(0);
    expect(subsequenceScore('firefox', 'zz')).toBe(0);
    expect(subsequenceScore('abc', 'abc')).toBe(1);
  });

  test('tokenMatch prefers prefixes over substrings over typos', () => {
    const prefix = tokenMatch('chr', 'chrome');
    const word = tokenMatch('term', 'fresh terminal');
    const sub = tokenMatch('rom', 'chrome');
    const typo = tokenMatch('andriod', 'android device');
    expect(prefix).toBeGreaterThan(word);
    expect(word).toBeGreaterThan(sub);
    expect(sub).toBeGreaterThan(typo);
    expect(typo).toBeGreaterThan(0);
    expect(tokenMatch('ft', 'fresh terminal')).toBe(0.7); // acronym
    expect(tokenMatch('xyz', 'chrome')).toBe(0);
  });

  test('scoreFields requires every token to match some field', () => {
    const fields = [
      { text: 'Fresh terminal', weight: 1 },
      { text: 'shell bash', weight: 0.8 },
    ];
    expect(scoreFields('fresh bash', fields)).toBeGreaterThan(0);
    expect(scoreFields('fresh docker', fields)).toBe(0);
    expect(scoreFields('', fields)).toBe(1);
  });

  test('rankByQuery is stable and filters', () => {
    const items = ['alpha', 'beta', 'alpine', 'gamma'];
    expect(rankByQuery(items, 'al', (s) => [{ text: s, weight: 1 }])).toEqual(['alpha', 'alpine']);
    expect(rankByQuery(items, '', (s) => [{ text: s, weight: 1 }])).toEqual(items);
  });
});

describe('catalog', () => {
  test('panelKindFor maps daemon and mock ids', () => {
    expect(panelKindFor({ id: 'android-adb', needsConfig: true })).toBe('android');
    expect(panelKindFor({ id: 'manual-setup', needsConfig: true })).toBe('manual');
    expect(panelKindFor({ id: 'ios-manual', needsConfig: true })).toBe('ios');
    expect(panelKindFor({ id: 'system-proxy', needsConfig: false })).toBe('system-proxy');
    expect(panelKindFor({ id: 'chrome', needsConfig: false })).toBeNull();
    expect(panelKindFor({ id: 'something-new', needsConfig: true })).toBe('generic');
  });

  test('primary action labels', () => {
    expect(primaryActionFor(CATALOG[1] as InterceptorInfo).label).toBe('Launch');
    expect(primaryActionFor({ ...(CATALOG[6] as InterceptorInfo) }).label).toBe('Set up');
    expect(primaryActionFor({ ...(CATALOG[6] as InterceptorInfo), active: true }).label).toBe('Manage');
    expect(primaryActionFor({ id: 'vpn', needsConfig: false, category: 'system', active: false }).label).toBe('Activate');
    expect(primaryActionFor({ id: 'vpn', needsConfig: false, category: 'system', active: true }).label).toBe('Deactivate');
  });

  test('statusOf explains every state', () => {
    expect(statusOf(CATALOG[1] as InterceptorInfo)).toEqual({ tone: 'ready', label: 'Available · 131.0', tooltip: 'Version 131.0.6778.86' });
    expect(statusOf(CATALOG[3] as InterceptorInfo)).toMatchObject({ tone: 'missing', label: 'Not installed', tooltip: 'Microsoft Edge is not installed' });
    expect(statusOf({ ...(CATALOG[1] as InterceptorInfo), active: true }).tone).toBe('active');
    expect(statusOf({ ...(CATALOG[1] as InterceptorInfo), supported: false }, 'linux')).toMatchObject({ tone: 'unsupported', label: 'Not on Linux' });
    expect(statusOf({ ...(CATALOG[1] as InterceptorInfo), available: false, unavailableReason: 'Docker daemon is not running' }).label).toBe('Not running');
  });

  test('shortVersion', () => {
    expect(shortVersion('131.0.6778.86')).toBe('131.0');
    expect(shortVersion('1.73')).toBe('1.73');
    expect(shortVersion('nightly')).toBe('nightly');
  });

  test('canUsePrimary: unavailable launchers are disabled, panels still open', () => {
    expect(canUsePrimary(CATALOG[3] as InterceptorInfo)).toBe(false);
    expect(canUsePrimary({ ...(CATALOG[6] as InterceptorInfo), available: false })).toBe(true);
    expect(canUsePrimary({ ...(CATALOG[6] as InterceptorInfo), supported: false })).toBe(false);
  });

  test('sortCatalog orders by category and keeps daemon order within', () => {
    expect(sortCatalog(CATALOG).map((i) => i.id)).toEqual([
      'chrome',
      'firefox',
      'edge',
      'fresh-terminal',
      'existing-terminal',
      'android-adb',
      'ios',
      'docker',
      'system-proxy',
      'manual',
    ]);
  });

  test('filterCatalog: category and fuzzy query', () => {
    const sorted = sortCatalog(CATALOG);
    expect(filterCatalog(sorted, 'browser', '').map((i) => i.id)).toEqual(['chrome', 'firefox', 'edge']);
    expect(filterCatalog(sorted, 'all', 'chr')[0]?.id).toBe('chrome');
    expect(filterCatalog(sorted, 'all', 'andriod')[0]?.id).toBe('android-adb');
    expect(filterCatalog(sorted, 'all', 'iphone')[0]?.id).toBe('ios');
    expect(filterCatalog(sorted, 'all', 'phone').map((i) => i.id)).toEqual(expect.arrayContaining(['android-adb', 'ios']));
    expect(filterCatalog(sorted, 'all', 'fresh term')[0]?.id).toBe('fresh-terminal');
    expect(filterCatalog(sorted, 'all', 'python')[0]?.id).toBeDefined();
    expect(filterCatalog(sorted, 'all', 'qqqzzz')).toEqual([]);
    expect(filterCatalog(sorted, 'mobile', 'chrome')).toEqual([]);
  });

  test("sortCatalog puts interceptors this OS can't run last in their category", () => {
    const list = [
      ic({ id: 'safari', name: 'Safari', category: 'browser', supported: false }),
      ic({ id: 'opera', name: 'Opera', category: 'browser', available: false }),
      ic({ id: 'chrome', name: 'Chrome', category: 'browser' }),
      ic({ id: 'android-adb', name: 'Android', category: 'mobile', available: false }),
      ic({ id: 'ios', name: 'iOS', category: 'mobile' }),
      ic({ id: 'fresh-terminal', name: 'T', category: 'terminal' }),
    ];
    // Not-installed browsers sink below installed ones; other categories keep daemon order.
    expect(sortCatalog(list).map((i) => i.id)).toEqual(['chrome', 'opera', 'safari', 'fresh-terminal', 'android-adb', 'ios']);
  });

  test('splitDormant tucks away not-installed browsers and unsupported interceptors', () => {
    const list = [
      ic({ id: 'chrome', name: 'Chrome', category: 'browser' }),
      ic({ id: 'opera', name: 'Opera', category: 'browser', available: false }),
      ic({ id: 'arc', name: 'Arc', category: 'browser', supported: false }),
      ic({ id: 'android-adb', name: 'Android', category: 'mobile', available: false }), // a phone may be plugged in any moment
      ic({ id: 'edge', name: 'Edge', category: 'browser', available: false, active: true }), // still running from before
    ];
    const { main, dormant } = splitDormant(list);
    expect(main.map((i) => i.id)).toEqual(['chrome', 'android-adb', 'edge']);
    expect(dormant.map((i) => i.id)).toEqual(['opera', 'arc']);
    expect(isDormant(list[1] as InterceptorInfo)).toBe(true);
  });

  test('iconKey maps daemon icon keys AppIcon lacks', () => {
    expect(iconKey('desktop')).toBe('globe');
    expect(iconKey('ios')).toBe('apple');
    expect(iconKey('chrome')).toBe('chrome');
  });

  test('countByCategory', () => {
    const c = countByCategory(CATALOG);
    expect(c.all).toBe(10);
    expect(c.browser).toBe(3);
    expect(c.runtime).toBe(0);
  });
});

describe('addresses', () => {
  test('splitHostPort / formatHostPort', () => {
    expect(splitHostPort('127.0.0.1:8000')).toEqual({ host: '127.0.0.1', port: 8000 });
    expect(splitHostPort('[::1]:8000')).toEqual({ host: '::1', port: 8000 });
    expect(splitHostPort('fe80::1')).toEqual({ host: 'fe80::1', port: null });
    expect(splitHostPort('localhost')).toEqual({ host: 'localhost', port: null });
    expect(formatHostPort('::1', 8000)).toBe('[::1]:8000');
    expect(formatHostPort('10.0.0.2', 8000)).toBe('10.0.0.2:8000');
  });

  test('lanAddresses filters loopback and ranks private IPv4 first', () => {
    const status = { proxy: { port: 8000, bindAddress: '0.0.0.0', addresses: ['127.0.0.1:8000', '[fe80::1]:8000', '172.17.0.1:8000', '192.168.1.23:8000', '192.168.1.23:8000', '[::1]:8000'] } } as Pick<Status, 'proxy'>;
    expect(lanAddresses(status)).toEqual(['192.168.1.23:8000', '[fe80::1]:8000', '172.17.0.1:8000']);
    expect(lanAddresses(null)).toEqual([]);
  });

  test('isLanBind / deviceSetupUrl', () => {
    expect(isLanBind('0.0.0.0')).toBe(true);
    expect(isLanBind('127.0.0.1')).toBe(false);
    expect(isLanBind('::1')).toBe(false);
    expect(isLanBind('192.168.1.5')).toBe(true);
    expect(deviceSetupUrl('192.168.1.23:8000')).toBe('http://192.168.1.23:8000/');
  });
});

describe('metadata', () => {
  test('devices fall back to their id for a name and skip entries without one', () => {
    const a = parseMetadata({ devices: [{ id: 'emulator-5554', name: 'Pixel 8', state: 'device', emulator: true }, { id: 'R58M', state: 'unauthorized', emulator: false, active: true }, { name: 'no id' }, 'x'] });
    expect(a.devices).toEqual([
      { id: 'emulator-5554', name: 'Pixel 8', state: 'device', model: undefined, manufacturer: undefined, androidVersion: undefined, emulator: true, active: false },
      { id: 'R58M', name: 'R58M', state: 'unauthorized', model: undefined, manufacturer: undefined, androidVersion: undefined, emulator: false, active: true },
    ]);
  });

  test('parses containers, processes, steps and misc keys, skipping malformed entries', () => {
    const m = parseMetadata({
      containers: [{ id: 'f3a9c1d2e4b5aa', names: ['/api'], image: 'node:22', intercepted: true }, { nope: 1 }],
      processes: [{ pid: 42, name: 'gradle' }, { pid: 'x' }],
      steps: ['one', { title: 'two' }, 3],
      setupUrl: 'http://x/',
      ca: { path: '/home/me/.local/share/akev/ca.pem' },
    });
    expect(m.containers).toEqual([{ id: 'f3a9c1d2e4b5aa', name: 'api', image: 'node:22', status: undefined, state: undefined, engine: undefined, active: true }]);
    expect(m.processes).toEqual([{ pid: 42, name: 'gradle', active: false, attached: false }]);
    expect(m.steps).toEqual([{ detail: 'one' }, { detail: 'two' }]);
    expect(m.setupUrl).toBe('http://x/');
    expect(m.caPath).toBe('/home/me/.local/share/akev/ca.pem');
    expect(m.docker).toBeNull();
    expect(m.snippets).toEqual({});
    expect(m.shells).toEqual([]);
  });

  test('garbage in, empty out', () => {
    for (const raw of [null, undefined, 42, 'str', [], { devices: 'x', containers: 7, shells: 3, trust: [], steps: 'x', running: {} }]) {
      const m = parseMetadata(raw);
      expect(m.devices).toEqual([]);
      expect(m.containers).toEqual([]);
      expect(m.snippets).toEqual({});
      expect(m.trust).toBeNull();
      expect(m.running).toEqual([]);
      expect(m.steps).toEqual([]);
    }
  });

  // Shapes below are copied from crates/akev-intercept (the real daemon).
  test('daemon: existing terminal shells', () => {
    const m = parseMetadata({
      defaultShell: 'fish',
      shells: {
        powershell: { label: 'PowerShell', command: 'iex (…)', reset: 'akev-off' },
        cmd: { label: 'Command Prompt', command: 'curl … && call …', reset: 'akev-off' },
        bash: { label: 'Git Bash', command: 'eval "$(curl …)"', reset: 'akev-off' },
        ruby: { label: 'Ruby', command: 'nope' },
      },
      proxy: 'http://127.0.0.1:8000',
      note: 'Run the command in the terminal you want to intercept.',
    });
    expect(m.shells).toEqual(['powershell', 'cmd', 'bash']);
    expect(m.shellLabels.bash).toBe('Git Bash');
    expect(m.snippets.cmd).toBe('curl … && call …');
    expect(m.resetSnippets.powershell).toBe('akev-off');
    expect(m.defaultShell).toBe('fish');
    expect(m.proxy).toBe('127.0.0.1:8000');
    expect(m.notes).toEqual(['Run the command in the terminal you want to intercept.']);
  });

  test('daemon: docker containers, engines and snippets', () => {
    const m = parseMetadata({
      engines: [
        { kind: 'docker', path: '/usr/bin/docker', available: true, error: null },
        { kind: 'podman', path: '/usr/bin/podman', available: false, error: 'socket missing' },
      ],
      containers: [{ id: 'abcdef0123456789', shortId: 'abcdef012345', name: 'web', names: ['web'], image: 'nginx', state: 'running', status: 'Up 3 hours', engine: 'docker', intercepted: false }],
      snippets: { envFiles: [{ path: '/d/akev-docker.env', content: '…' }], run: { docker: 'docker run …', podman: 'podman run …' }, compose: 'services: …', runHostNetwork: 'docker run --network host …', notes: ['Recreate running containers.'] },
    });
    expect(m.engines).toEqual([
      { kind: 'docker', available: true, error: undefined },
      { kind: 'podman', available: false, error: 'socket missing' },
    ]);
    expect(m.containers[0]).toEqual({ id: 'abcdef0123456789', name: 'web', image: 'nginx', status: 'Up 3 hours', state: 'running', engine: 'docker', active: false });
    expect(m.docker).toEqual({
      run: [
        { engine: 'docker', command: 'docker run …' },
        { engine: 'podman', command: 'podman run …' },
      ],
      compose: 'services: …',
      runHostNetwork: 'docker run --network host …',
      notes: ['Recreate running containers.'],
    });
    expect(m.snippets).toEqual({}); // docker snippets aren't shell snippets
  });

  test('daemon: system proxy trust instructions', () => {
    const m = parseMetadata({
      backends: [{ id: 'gnome', name: 'GNOME' }],
      proxy: '127.0.0.1:8001',
      configuredProxy: '127.0.0.1:8000',
      active: true,
      trust: {
        summary: "Most Linux tools read the distribution's CA bundle.",
        options: [
          { title: 'Fedora (p11-kit)', install: ['sudo trust anchor --store /ca.pem'], remove: ['sudo trust anchor --remove /ca.pem'] },
          { title: 'GUI', steps: ['Open it', 'Trust it'] },
          { title: 'Empty' },
        ],
        notes: ['Firefox keeps its own store.'],
        caSha256: 'AB',
      },
    });
    expect(m.backends).toEqual([{ id: 'gnome', name: 'GNOME' }]);
    expect(m.configuredProxy).toBe('127.0.0.1:8000');
    expect(m.proxy).toBe('127.0.0.1:8001');
    expect(m.trust?.options.map((o) => o.title)).toEqual(['Fedora (p11-kit)', 'GUI']);
    expect(m.trust?.options[0]?.remove).toEqual(['sudo trust anchor --remove /ca.pem']);
    expect(m.trust?.notes).toEqual(['Firefox keeps its own store.']);
  });

  test('daemon: manual setup and iOS', () => {
    const manual = parseMetadata({
      proxy: { host: '127.0.0.1', port: 8000, url: 'http://127.0.0.1:8000' },
      pacUrl: 'http://127.0.0.1:8000/proxy.pac',
      setupUrl: 'http://127.0.0.1:8000/',
      setupUrlViaProxy: 'http://akev.cert/',
      ca: { path: '/data/ca.pem', bundlePath: '/data/bundle.pem', pemUrl: 'x', mobileconfigUrl: 'http://127.0.0.1:8000/ca.mobileconfig' },
      envVars: [{ name: 'HTTPS_PROXY', value: 'http://127.0.0.1:8000' }, { name: 'bad' }],
      steps: ['Point the client at the proxy.'],
      note: 'Other devices need LAN access.',
    });
    expect(manual.proxy).toBe('127.0.0.1:8000');
    expect(manual.caPath).toBe('/data/ca.pem');
    expect(manual.bundlePath).toBe('/data/bundle.pem');
    expect(manual.pacUrl).toBe('http://127.0.0.1:8000/proxy.pac');
    expect(manual.envVars).toEqual([{ name: 'HTTPS_PROXY', value: 'http://127.0.0.1:8000' }]);
    expect(manual.mobileconfigUrl).toBe('http://127.0.0.1:8000/ca.mobileconfig');
    expect(manual.notes).toEqual(['Other devices need LAN access.']);

    const ios = parseMetadata({
      host: '192.168.1.20',
      port: 8000,
      setupUrl: 'http://192.168.1.20:8000/',
      steps: [{ title: 'Set the Wi-Fi proxy', detail: 'Settings → Wi-Fi…' }, { title: 'Only a title' }],
      notes: ['Firewall must allow the port.'],
    });
    expect(ios.host).toBe('192.168.1.20');
    expect(ios.port).toBe(8000);
    expect(ios.steps).toEqual([{ title: 'Set the Wi-Fi proxy', detail: 'Settings → Wi-Fi…' }, { detail: 'Only a title' }]);
  });

  test('daemon: android devices and electron apps', () => {
    const m = parseMetadata({
      adbPath: '/usr/bin/adb',
      devices: [{ id: 'R58M', name: 'Galaxy S21', state: 'device', model: 'SM-G991B', manufacturer: 'samsung', androidVersion: '14', sdk: 34, emulator: false, active: true }],
      running: [{ pid: 4242, path: '/usr/share/code/code', name: 'code' }, { pid: 'x' }],
    });
    expect(m.devices[0]).toMatchObject({ id: 'R58M', manufacturer: 'samsung', androidVersion: '14', active: true });
    expect(describeDevice(m.devices[0] as AdbDevice)).toBe('samsung SM-G991B · Android 14');
    expect(describeDevice({ name: 'Pixel 8', model: 'Pixel 8', manufacturer: 'Google' })).toBe('Google');
    expect(m.running).toEqual([{ pid: 4242, path: '/usr/share/code/code', name: 'code' }]);
  });

  test('parseActivation: docker exec + android checks', () => {
    const docker = parseActivation({ containerId: 'abc', engine: 'docker', proxyHost: '172.17.0.1', caCopied: true, execCommand: 'docker exec -it -e … abc sh' });
    expect(docker.execCommand).toBe('docker exec -it -e … abc sh');
    expect(docker.caCopied).toBe(true);
    const android = parseActivation({
      deviceId: 'R58M',
      proxy: '127.0.0.1:8000',
      caPath: '/sdcard/Download/akev-ca.crt',
      steps: [{ step: 'push-ca', ok: true, detail: null }, { step: 'reverse-tunnel', ok: false, proxyHost: '10.0.2.2' }, { nope: 1 }],
      installSteps: ['Open Settings', 'Install a certificate'],
      manualRecovery: 'adb shell settings put global http_proxy :0',
    });
    expect(android.checks).toEqual([
      { step: 'push-ca', ok: true, detail: undefined },
      { step: 'reverse-tunnel', ok: false, detail: undefined },
    ]);
    expect(android.installSteps).toHaveLength(2);
    expect(android.manualRecovery).toContain(':0');
    expect(parseActivation(null)).toEqual({ execCommand: undefined, caCopied: undefined, proxy: undefined, caPath: undefined, installSteps: [], manualRecovery: undefined, checks: [], command: undefined });
  });

  test('describeDeviceState', () => {
    expect(describeDeviceState('device').ready).toBe(true);
    expect(describeDeviceState('unauthorized').hint).toContain('USB debugging');
    expect(describeDeviceState('recovery').label).toBe('Recovery');
  });
});

describe('snippets', () => {
  const url = 'http://127.0.0.1:45456/api/setup/terminal?shell=bash';
  test('terminal one-liners per shell send the token in a header, not the URL', () => {
    // Same form as the daemon's snippets (crates/akev-intercept/src/terminal.rs).
    const token = "t o'k&=";
    expect(terminalOneLiner('bash', url, token)).toBe(
      `eval "$(printf '%s\\n' 'header = "Authorization: Bearer t o'\\''k&="' | curl -sS --noproxy '*' -K - '${url}')"`,
    );
    expect(terminalOneLiner('fish', url, token)).toBe(
      `printf '%s\\n' 'header = "Authorization: Bearer t o\\'k&="' | curl -sS --noproxy '*' -K - '${url}' | source`,
    );
    expect(terminalOneLiner('powershell', url, token)).toBe(
      `iex (& { $c = New-Object Net.WebClient -Property @{Proxy=$null}; $c.Headers['Authorization'] = 'Bearer t o''k&='; $c.DownloadString('${url}') })`,
    );
    expect(terminalOneLiner('cmd', url, token)).toBe(
      `>"%TEMP%\\akev-curl.cfg" echo header = "Authorization: Bearer t o'k&="&& curl -sS --noproxy "*" -K "%TEMP%\\akev-curl.cfg" -o "%TEMP%\\akev-setup.cmd" "${url}" && (del "%TEMP%\\akev-curl.cfg" & call "%TEMP%\\akev-setup.cmd") || del "%TEMP%\\akev-curl.cfg" 2>nul`,
    );
    for (const shell of ['bash', 'zsh', 'fish', 'powershell', 'cmd'] as const) expect(terminalOneLiner(shell, url, token)).not.toContain('token=');
    // Without a token (none needed) there's no header.
    expect(terminalOneLiner('bash', url)).toBe(`eval "$(curl -sS --noproxy '*' '${url}')"`);
    expect(terminalOneLiner('cmd', url)).toContain('call "%TEMP%\\akev-setup.cmd"');
  });

  test('curl config escapes quotes and backslashes', () => {
    expect(curlAuthConfig('abc')).toBe('header = "Authorization: Bearer abc"');
    expect(curlAuthConfig('a"b\\c\nd')).toBe('header = "Authorization: Bearer a\\"b\\\\cd"');
  });

  test('reset commands clear proxy and CA variables', () => {
    expect(terminalResetCommand('zsh')).toMatch(/^unset HTTP_PROXY .*SSL_CERT_FILE/);
    expect(terminalResetCommand('fish')).toMatch(/^set -e HTTP_PROXY/);
    expect(terminalResetCommand('powershell')).toContain('Env:HTTPS_PROXY');
    expect(terminalResetCommand('powershell')).not.toContain('Env:https_proxy');
    expect(terminalResetCommand('cmd')).toMatch(/^for %v in \(HTTP_PROXY/);
  });

  test('env snippets per shell', () => {
    const bash = envSnippet('bash', '127.0.0.1:8000', '$HOME/ca.pem');
    expect(bash).toContain('export HTTPS_PROXY="http://127.0.0.1:8000"');
    expect(bash).toContain('export https_proxy="http://127.0.0.1:8000"');
    expect(bash).toContain('export NODE_EXTRA_CA_CERTS="$HOME/ca.pem"');
    const ps = envSnippet('powershell', '127.0.0.1:8000', 'C:\\ca.pem');
    expect(ps).toContain('$env:HTTPS_PROXY = "http://127.0.0.1:8000"');
    expect(ps).not.toContain('https_proxy');
    expect(envSnippet('fish', '127.0.0.1:8000', '/ca.pem')).toContain('set -gx HTTP_PROXY "http://127.0.0.1:8000"');
    expect(envSnippet('cmd', '127.0.0.1:8000', 'C:\\ca.pem')).toContain('set "HTTP_PROXY=http://127.0.0.1:8000"');
  });

  test('defaults per platform', () => {
    expect(defaultShell('windows')).toBe('powershell');
    expect(defaultShell('macos')).toBe('zsh');
    expect(defaultShell('linux')).toBe('bash');
    expect(caFilePath('bash')).toBe('$HOME/Downloads/akev-ca.pem');
    expect(caFilePath('powershell')).toContain('$env:USERPROFILE');
    expect(caFilePath('cmd', '/abs/ca.pem')).toBe('/abs/ca.pem');
  });

  test('root-replacing variables use the CA bundle when known', () => {
    const bash = envSnippet('bash', '127.0.0.1:8000', '/data/ca.pem', undefined, '/data/bundle.pem');
    expect(bash).toContain('export SSL_CERT_FILE="/data/bundle.pem"');
    expect(bash).toContain('export REQUESTS_CA_BUNDLE="/data/bundle.pem"');
    expect(bash).toContain('export NODE_EXTRA_CA_CERTS="/data/ca.pem"'); // additive: CA only
    const guides = manualGuides({ proxy: '127.0.0.1:8000', shell: 'bash', caPath: '/data/ca.pem', providedCaPath: '/data/ca.pem', bundlePath: '/data/bundle.pem' });
    expect(guides.find((g) => g.id === 'python')?.blocks[0]?.code).toContain('SSL_CERT_FILE="/data/bundle.pem"');
    // The daemon's exact variable list wins for the env tab.
    const exact = manualGuides({ proxy: '127.0.0.1:8000', shell: 'fish', caPath: '/c', envVars: [{ name: 'HTTPS_PROXY', value: 'http://127.0.0.1:8000' }] });
    expect(exact[0]?.blocks[0]?.code).toBe('set -gx HTTPS_PROXY "http://127.0.0.1:8000"');
  });

  test('terminal test command per shell', () => {
    expect(terminalTestCommand('bash')).toContain('/dev/null');
    expect(terminalTestCommand('cmd')).toContain('NUL');
    expect(terminalTestCommand('powershell')).toMatch(/^Invoke-WebRequest/);
  });

  test('manual guides embed the proxy and CA path', () => {
    const guides = manualGuides({ proxy: '127.0.0.1:8000', lanProxy: '192.168.1.23:8000', shell: 'bash', caPath: '$HOME/ca.pem', setupUrl: 'http://192.168.1.23:8000/' });
    expect(guides.map((g) => g.id)).toEqual(['env', 'node', 'python', 'java', 'go', 'rust', 'curl', 'ios', 'android']);
    const java = guides.find((g) => g.id === 'java');
    expect(java?.blocks.map((b) => b.code ?? '').join('\n')).toContain('-Dhttps.proxyPort=8000');
    const ios = guides.find((g) => g.id === 'ios');
    expect(ios?.blocks[0]?.steps?.[0]).toContain('server 192.168.1.23, port 8000');
    const android = guides.find((g) => g.id === 'android');
    expect(android?.blocks.map((b) => b.code ?? '').join('\n')).toContain('http_proxy 192.168.1.23:8000');
  });

  test('docker snippet and trust guides', () => {
    expect(dockerRunSnippet(8001)).toContain('HTTPS_PROXY=http://host.docker.internal:8001');
    expect(commonName('CN=Akev CA (mock), O=Akev')).toBe('Akev CA (mock)');
    expect(commonName(undefined)).toBe('Akev CA');
    const mac = trustGuide('macos', { caName: 'Akev CA' });
    expect(mac.blocks[0]?.code).toContain('add-trusted-cert');
    expect(trustGuide('linux', { caName: 'X', providedCaPath: '/data/ca.pem' }).blocks[0]?.code).toContain('"/data/ca.pem"');
  });
});
