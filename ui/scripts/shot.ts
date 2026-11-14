#!/usr/bin/env bun
/**
 * Headless-Chrome screenshots of the UI for visual review (see ui/README.md → Screenshots).
 *
 *   bun run shot                                   # traffic, dark, 1440×900 → /tmp/akev-shot.png
 *   bun run shot -- --route _kitchen-sink --theme light --section data --out /tmp/ks.png
 *   bun run shot -- --route connect --width 1280 --height 800 --no-mock
 *
 * Options:
 *   --url <base>      dev/preview server (default http://localhost:5173/)
 *   --route <path>    hash route without `#/` (default traffic)
 *   --theme <name>    dark | light | midnight | contrast (session override, not persisted)
 *   --section <id>    kitchen-sink section to scroll to (tokens, buttons, inputs, …)
 *   --width/--height  window size (default 1440×900)
 *   --scale <n>       device pixel ratio (default 1; 2 for high-DPI images)
 *   --wait <ms>       virtual time budget before capture (default 4000)
 *   --out <file>      output PNG (default /tmp/akev-shot.png)
 *   --no-mock         don't add `?mock` (screenshot against a real daemon)
 *   --media <f=v>     emulate a media feature, e.g. `forced-colors=active`,
 *                     `prefers-reduced-motion=reduce` (repeatable; uses a CDP session)
 *   --chrome <bin>    browser binary (default: $CHROME, google-chrome, chromium)
 *
 * Interactions (switch to a Chrome DevTools Protocol session; applied in order, ~350 ms apart):
 *   --press <combo>   key press, e.g. `mod+k`, `shift+?`, `escape`, `arrowdown`
 *   --type <text>     insert text into the focused element
 *   --click <css>     click the centre of the first element matching the selector
 *   --rclick <css>    right-click (context menus)
 *   --hover <css>     move the pointer over an element (tooltips)
 *   --sleep <ms>      extra pause
 *   --eval <js>       evaluate JavaScript in the page (e.g. `__akevMock.drop(60000)`)
 *
 *   bun run shot -- --press mod+k --type theme --out /tmp/palette.png
 */

import { spawnSync } from 'node:child_process';

interface Options {
  url: string;
  route: string;
  theme: string | null;
  section: string | null;
  width: number;
  height: number;
  scale: number;
  wait: number;
  out: string;
  mock: boolean;
  chrome: string | null;
  media: { name: string; value: string }[];
  actions: Action[];
}

type Action =
  | { kind: 'press'; combo: string }
  | { kind: 'type'; text: string }
  | { kind: 'click' | 'rclick' | 'hover'; selector: string }
  | { kind: 'sleep'; ms: number }
  | { kind: 'eval'; code: string };

function parseArgs(argv: string[]): Options {
  const opts: Options = {
    url: 'http://localhost:5173/',
    route: 'traffic',
    theme: 'dark',
    section: null,
    width: 1440,
    height: 900,
    scale: 1,
    wait: 4000,
    out: '/tmp/akev-shot.png',
    mock: true,
    chrome: process.env.CHROME ?? null,
    media: [],
    actions: [],
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const next = (): string => {
      const v = argv[++i];
      if (v === undefined) throw new Error(`Missing value for ${arg}`);
      return v;
    };
    switch (arg) {
      case '--url':
        opts.url = next();
        break;
      case '--route':
        opts.route = next().replace(/^#?\/?/, '');
        break;
      case '--theme':
        opts.theme = next();
        break;
      case '--section':
        opts.section = next();
        break;
      case '--width':
        opts.width = Number(next());
        break;
      case '--height':
        opts.height = Number(next());
        break;
      case '--scale':
        opts.scale = Number(next());
        break;
      case '--wait':
        opts.wait = Number(next());
        break;
      case '--out':
        opts.out = next();
        break;
      case '--no-mock':
        opts.mock = false;
        break;
      case '--chrome':
        opts.chrome = next();
        break;
      case '--media': {
        const [name = '', value = ''] = next().split('=');
        opts.media.push({ name, value });
        break;
      }
      case '--press':
        opts.actions.push({ kind: 'press', combo: next() });
        break;
      case '--type':
        opts.actions.push({ kind: 'type', text: next() });
        break;
      case '--click':
      case '--rclick':
      case '--hover':
        opts.actions.push({ kind: arg.slice(2) as 'click' | 'rclick' | 'hover', selector: next() });
        break;
      case '--sleep':
        opts.actions.push({ kind: 'sleep', ms: Number(next()) });
        break;
      case '--eval':
        opts.actions.push({ kind: 'eval', code: next() });
        break;
      case '--':
        break;
      default:
        throw new Error(`Unknown option ${arg}`);
    }
  }
  return opts;
}

function findChrome(preferred: string | null): string {
  const candidates = preferred ? [preferred] : ['google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser'];
  for (const bin of candidates) {
    const probe = spawnSync(bin, ['--version'], { encoding: 'utf8' });
    if (probe.status === 0) return bin;
  }
  throw new Error(`No Chrome/Chromium found (tried ${candidates.join(', ')}); pass --chrome <bin>`);
}

/** Build the page URL: search params go before the hash, the route's query after it. */
export function buildShotUrl(opts: Pick<Options, 'url' | 'route' | 'theme' | 'section' | 'mock'>): string {
  const base = new URL(opts.url);
  if (opts.mock) base.searchParams.set('mock', '');
  if (opts.theme) base.searchParams.set('theme', opts.theme);
  const search = base.searchParams.toString().replace(/=(?=&|$)/g, '');
  const hashQuery = opts.section ? `?section=${encodeURIComponent(opts.section)}` : '';
  return `${base.origin}${base.pathname}${search ? `?${search}` : ''}#/${opts.route}${hashQuery}`;
}

// =====================================================================================
// Chrome DevTools Protocol session (for interactions)
// =====================================================================================

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

class Cdp {
  private nextId = 1;
  private readonly pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  private readonly waiters: { method: string; resolve: () => void }[] = [];
  sessionId: string | null = null;

  private constructor(private readonly ws: WebSocket) {
    ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(String(ev.data)) as { id?: number; result?: unknown; error?: { message: string }; method?: string };
      if (msg.id != null) {
        const p = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (msg.error) p?.reject(new Error(msg.error.message));
        else p?.resolve(msg.result);
      } else if (msg.method) {
        for (let i = this.waiters.length - 1; i >= 0; i--) {
          const w = this.waiters[i];
          if (w && w.method === msg.method) {
            this.waiters.splice(i, 1);
            w.resolve();
          }
        }
      }
    });
  }

  static connect(url: string): Promise<Cdp> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      ws.addEventListener('open', () => resolve(new Cdp(ws)), { once: true });
      ws.addEventListener('error', () => reject(new Error(`Cannot connect to ${url}`)), { once: true });
    });
  }

  send<T = unknown>(method: string, params: Record<string, unknown> = {}, root = false): Promise<T> {
    const id = this.nextId++;
    const payload: Record<string, unknown> = { id, method, params };
    if (!root && this.sessionId) payload.sessionId = this.sessionId;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      this.ws.send(JSON.stringify(payload));
    });
  }

  waitFor(method: string, timeoutMs: number): Promise<void> {
    return Promise.race([new Promise<void>((resolve) => this.waiters.push({ method, resolve })), sleep(timeoutMs)]);
  }

  close(): void {
    this.ws.close();
  }
}

const KEY_CODES: Record<string, [key: string, code: string, vk: number]> = {
  escape: ['Escape', 'Escape', 27],
  enter: ['Enter', 'Enter', 13],
  tab: ['Tab', 'Tab', 9],
  arrowdown: ['ArrowDown', 'ArrowDown', 40],
  arrowup: ['ArrowUp', 'ArrowUp', 38],
  arrowleft: ['ArrowLeft', 'ArrowLeft', 37],
  arrowright: ['ArrowRight', 'ArrowRight', 39],
  backspace: ['Backspace', 'Backspace', 8],
  delete: ['Delete', 'Delete', 46],
  ',': [',', 'Comma', 188],
  '/': ['/', 'Slash', 191],
  '?': ['?', 'Slash', 191],
};

async function pressKey(cdp: Cdp, combo: string): Promise<void> {
  const parts = combo.toLowerCase().split('+');
  const keyName = parts.pop() ?? '';
  let modifiers = 0;
  for (const p of parts) {
    if (p === 'alt') modifiers |= 1;
    else if (p === 'ctrl' || p === 'mod') modifiers |= 2; // headless Linux: mod = Ctrl
    else if (p === 'meta' || p === 'cmd') modifiers |= 4;
    else if (p === 'shift') modifiers |= 8;
  }
  const known = KEY_CODES[keyName];
  const [key, code, vk] = known ?? [
    modifiers & 8 ? keyName.toUpperCase() : keyName,
    /^[a-z]$/.test(keyName) ? `Key${keyName.toUpperCase()}` : /^[0-9]$/.test(keyName) ? `Digit${keyName}` : keyName,
    keyName.toUpperCase().charCodeAt(0),
  ];
  const text = key.length === 1 && !(modifiers & 2) && !(modifiers & 4) ? key : undefined;
  const base = { modifiers, key, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk };
  await cdp.send('Input.dispatchKeyEvent', { type: text ? 'keyDown' : 'rawKeyDown', ...base, ...(text ? { text, unmodifiedText: text } : {}) });
  await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
}

async function elementCenter(cdp: Cdp, selector: string): Promise<{ x: number; y: number }> {
  const res = await cdp.send<{ result: { value?: { x: number; y: number } | null } }>('Runtime.evaluate', {
    expression: `(() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) return null; el.scrollIntoView({ block: 'center' }); const r = el.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`,
    returnByValue: true,
  });
  const v = res.result.value;
  if (!v) throw new Error(`No element matches ${selector}`);
  return v;
}

async function runInteractive(chrome: string, opts: Options, url: string): Promise<void> {
  const profile = `${process.env.TMPDIR ?? '/tmp'}/akev-shot-${process.pid}`;
  const proc = Bun.spawn([chrome, '--headless=new', '--disable-gpu', '--hide-scrollbars', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], {
    stderr: 'pipe',
    stdout: 'ignore',
  });
  try {
    // Read "DevTools listening on ws://…" from stderr.
    const reader = proc.stderr.getReader();
    let buf = '';
    let wsUrl: string | null = null;
    const deadline = Date.now() + 15_000;
    while (!wsUrl && Date.now() < deadline) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += new TextDecoder().decode(value);
      wsUrl = /DevTools listening on (ws:\/\/\S+)/.exec(buf)?.[1] ?? null;
    }
    reader.releaseLock();
    if (!wsUrl) throw new Error('Chrome did not expose a DevTools endpoint');

    const cdp = await Cdp.connect(wsUrl);
    const { targetId } = await cdp.send<{ targetId: string }>('Target.createTarget', { url: 'about:blank' }, true);
    const { sessionId } = await cdp.send<{ sessionId: string }>('Target.attachToTarget', { targetId, flatten: true }, true);
    cdp.sessionId = sessionId;
    await cdp.send('Emulation.setDeviceMetricsOverride', { width: opts.width, height: opts.height, deviceScaleFactor: opts.scale, mobile: false });
    if (opts.media.length) await cdp.send('Emulation.setEmulatedMedia', { features: opts.media });
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    const loaded = cdp.waitFor('Page.loadEventFired', 20_000);
    await cdp.send('Page.navigate', { url });
    await loaded;
    await sleep(Math.min(opts.wait, 6000));

    for (const action of opts.actions) {
      switch (action.kind) {
        case 'press':
          await pressKey(cdp, action.combo);
          break;
        case 'type':
          await cdp.send('Input.insertText', { text: action.text });
          break;
        case 'sleep':
          await sleep(action.ms);
          break;
        case 'eval':
          await cdp.send('Runtime.evaluate', { expression: action.code, awaitPromise: true });
          break;
        default: {
          const { x, y } = await elementCenter(cdp, action.selector);
          await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
          if (action.kind !== 'hover') {
            const button = action.kind === 'rclick' ? 'right' : 'left';
            await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button, clickCount: 1 });
            await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button, clickCount: 1 });
          }
        }
      }
      await sleep(action.kind === 'hover' ? 700 : 350);
    }

    const shot = await cdp.send<{ data: string }>('Page.captureScreenshot', { format: 'png' });
    await Bun.write(opts.out, Buffer.from(shot.data, 'base64'));
    cdp.close();
  } finally {
    proc.kill();
    spawnSync('rm', ['-rf', profile]);
  }
}

async function main(): Promise<void> {
  const opts = parseArgs(process.argv.slice(2));
  const chrome = findChrome(opts.chrome);
  const url = buildShotUrl(opts);
  if (opts.actions.length > 0 || opts.media.length > 0) {
    await runInteractive(chrome, opts, url);
    process.stdout.write(`${opts.out}  ←  ${url} (${opts.actions.length} interactions)\n`);
    return;
  }
  const result = spawnSync(
    chrome,
    [
      '--headless=new',
      '--disable-gpu',
      '--hide-scrollbars',
      `--force-device-scale-factor=${opts.scale}`,
      `--window-size=${opts.width},${opts.height}`,
      `--virtual-time-budget=${opts.wait}`,
      `--screenshot=${opts.out}`,
      url,
    ],
    { encoding: 'utf8', timeout: opts.wait + 60_000 },
  );
  if (result.status !== 0) {
    process.stderr.write(result.stderr || `Chrome exited with ${result.status}\n`);
    process.exit(1);
  }
  process.stdout.write(`${opts.out}  ←  ${url}\n`);
}

if (import.meta.main) {
  main().catch((e: unknown) => {
    process.stderr.write(`${e instanceof Error ? e.message : String(e)}\n`);
    process.exit(1);
  });
}
