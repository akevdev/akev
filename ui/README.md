# Akev UI

The React web UI of Akev: served by the daemon (embedded with rust-embed) and loaded by the
Tauri desktop shell. Spec: [`docs/DESIGN.md`](../docs/DESIGN.md) (UI/UX) and
the [HTTP API reference](https://akev.dev/docs/api/) (the daemon contract).

```bash
bun install
bun run dev            # http://localhost:5173 — proxies /api + /mcp (incl. WebSocket) to 127.0.0.1:45456
bun run dev            # …then open http://localhost:5173/?mock to work without the daemon
bun run typecheck      # tsc --noEmit (strict)
bun test               # unit tests (bun:test)
bun run build          # → dist/ (relative asset paths: works under the daemon and tauri://)
bun run shot -- --route _kitchen-sink --theme light   # screenshot (see "Screenshots")
```

The dev server reads `VITE_` settings from the repository's `.env` (see `../.env.example`):
`VITE_AKEV_URL=http://127.0.0.1:45456` talks to a daemon directly instead of through the dev
proxy (the daemon must then allow the origin with `--allow-origin`, as `make daemon` does), and
`VITE_MOCK=1` always runs on the mock backend. Builds ignore `.env`.

---

## Folder layout

```
src/
  main.tsx               bootstrap: auth/base URL → optional mock → stream → render
  app/                   the shell (§4): App, NavRail, StatusBar, CommandPalette,
                         ShortcutsOverlay, ConnectionOverlay, ErrorBoundary, theme
    router.ts            hash router: useRoute(), useRouteParam(), navigate(), href()
    shortcuts.ts         global shortcut registry: useShortcut(), useShortcutDocs()
    commands.ts          command palette registry: useCommands(), useCommandProvider()
    commandScore.ts      palette ranking
    globalCommands.tsx   app-wide commands/shortcuts (navigation, recording, themes…)
    sessions/            Sessions dialog (save/open/rename/delete), session & import actions,
                         the startup "restored" toast
  api/
    types.ts             TS mirrors of every akev-core type + HTTP API payload
    client.ts            typed client for every endpoint (`api`), ApiError
    auth.ts              token / base URL resolution
    stream.ts            WebSocket stream client (reconnect, keepalive, filters)
    mock/                in-browser mock daemon (`?mock`)
  state/
    traffic.ts           TrafficStore — the high-performance external store for exchanges
    app.ts               zustand: connection, status, settings, rules, interceptors, paused…
    ui.ts                zustand: persisted UI prefs (theme, density, columns, splits, tabs…)
    connection.ts        wires the stream to the stores
    tlsProfiles.ts       the daemon's upstream TLS fingerprint catalogue (`useTlsProfiles`)
    license.ts           Akev Pro license status (`useLicense`, `useFeature`) + `openUpgrade(feature?)`
  components/ui/         primitives (§5) — import from `components/ui`
  components/pro/        Akev Pro: ProBadge, ProGate/ProCallout, license key form, UpgradeDialog
  components/            shared composites (BodyViewer, HeadersTable… — added by pages)
  pages/<page>/          one folder per page; the page component is the default export
  lib/                   pure helpers: format, http (status texts, header docs, cookies,
                         JWT, errors), encoding (UTF-8 byte offsets, base64), platform,
                         storage, timing, lru
  styles/                tokens.css (all design tokens), global.css, fonts.ts
scripts/
  shot.ts                headless-Chrome screenshots (optionally scripted via CDP)
  size.ts                bundle-size budget check (`bun run size`)
  licenses.ts            the open-source licences shown in Settings → About (read by vite.config.ts)
  llm-fixtures.ts        regenerates src/api/mock/llmFixtures.ts with the daemon's LLM parser
e2e/                     Playwright: daemon/ (against a real daemon), visual/ (screenshot baselines)
public/                  favicons (rendered by ../brand) and theme-boot.js
```

Rules of thumb: **pure logic goes in `lib/` or next to its store and gets a `*.test.ts`**;
components stay thin. Nothing in `lib/` imports React.

---

## Conventions

### Styling: CSS Modules + tokens only

* One `Component.module.css` next to each component; compose classes with `clsx`.
* **Never write raw colours, font sizes or shadows.** Use tokens from `styles/tokens.css`
  (`var(--bg-panel)`, `var(--text-muted)`, `var(--fs-sm)`, `var(--r-md)`, `var(--dur)`…). Token
  names are identical in every theme (dark, light, midnight, contrast), so a component that
  only uses tokens is automatically themed. `src/styles/tokens.test.ts` enforces WCAG AA for
  every text-like token on every surface — add new text colours to its list.
* Spacing sits on the 4 px grid (`--sp-1` … `--sp-12`). Controls are 24 / 28 / 34 px
  (`--control-h-sm`, `--control-h`, `--control-h-lg`); list rows use `--row-h` (density aware —
  in JS use `useRowHeight()`).
* Utility classes from `global.css`: `.tnum` (tabular numerals for ids/sizes/durations), `.mono`,
  `.truncate`, `.muted`, `.subtle`, `.caps`, `.sr-only`.
* Monospace text must never use ligatures (`://`, `>=` must render as captured): every
  `font-family: var(--font-mono)` rule also sets `font-variant-ligatures: none`.
* Motion: use `--dur-fast` / `--dur` / `--dur-slow` with `--ease`; `prefers-reduced-motion` is
  handled globally.
* Focus: rely on the global `:focus-visible` ring; don't remove outlines without a replacement.

### Copy

* Sentence case, plain words, and say what to do next ("Couldn't save the rule" + a Retry
  action, "Paste your license key.").
* Failures start with "Couldn't …". Straight apostrophes (`'`) everywhere; strings that contain
  one use double quotes. American spelling ("license").
* User-facing text says "Akev" (or "the Akev background service"), never "daemon".
* Numbers and dates go through `lib/format.ts`, which follows the system locale.

### State

| What | Where | How to read |
|---|---|---|
| Exchanges, filter, selection | `state/traffic.ts` (`trafficStore`) | `useTraffic(selector)`, `useExchangeSummary(id)`, `useIsSelected(id)` |
| Daemon state (status, settings, rules, rule stats, interceptors, paused, recording, connection) | `state/app.ts` (`useAppStore`) | `useAppStore((s) => s.rules)` |
| UI preferences (persisted to localStorage **and** `PATCH /api/settings/ui`) | `state/ui.ts` (`useUiStore`) | `useUiStore((s) => s.theme)`, `usePagePref(key, default)`, `useLastTab(group, default)` |
| Per-component ephemeral state | `useState` | — |

* The traffic store batches stream updates into **one notification per animation frame** and
  replaces (never mutates) summary objects, so rows can be `memo`ised on summary identity.
  Selection changes notify synchronously.
* Selectors must return stable references (`s.visible`, `s.summaries.get(id)`) or pass
  `shallowEqual` as the second argument to `useTraffic`.
* Filtering is server-side: `trafficStore.setFilterText(q)` / `setTypeChip(chip)` send
  `set-filter` (debounced 120 ms); `visible` follows the daemon's `filter-result` + deltas.
  Error spans in `filter.error` are UTF-8 **byte** offsets — convert with
  `byteOffsetToIndex()` from `lib/encoding.ts` (same for `/api/filter/suggest` ranges).
* Selection model: `select(id)`, `toggle(id)` (Ctrl/⌘-click), `extendTo(id, list?)`
  (Shift-click), `move(delta, extend?, list?)` (↑/↓/J/K/Home/End/PgUp/PgDn), `selectAll()`,
  `selectedInOrder()`. Pass `list` when the view is column-sorted.
* Page-specific preferences go through `usePagePref('rules.listWidth', 360)` — don't add
  top-level fields to `UiPrefs` unless several pages share them.

### Talking to the daemon

```ts
import { api, ApiError, errorMessage } from '../../api/client';

const detail = await api.exchanges.get(id);                    // ExchangeDetail
const body = await api.exchanges.body(id, 'response');         // BodyView
<img src={api.exchanges.rawUrl(id, 'response')} />             // URLs carry ?token= when needed
try {
  await useAppStore.getState().saveRules(next);                // optimistic, reverts on failure
} catch (e) {
  toast.error("Couldn't save rules", { description: errorMessage(e) });
}
```

Every endpoint in the HTTP API reference has a typed method (`api.exchanges.*`, `api.filter.*`, `api.rules.*`,
`api.breakpoints.*`, `api.send`, `api.codegen.*`, `api.interceptors.*`, `api.settings.*`,
`api.ca.*`, `api.transfer.*`, `api.capture.*`, `api.protos.*`, `api.sessions.*`, `api.proxy.*`). Failures throw `ApiError` with `status`, the
daemon's `code` (or `network` / `timeout` / `aborted` / `bad-response`) and a human message.
Live updates come from the stream (`state/connection.ts`), not polling. For WS/SSE messages
of one exchange use `trafficStore.onMessages(id, cb)` and fetch payloads with
`api.exchanges.messages(id, after)`.

Bodies are base64 in `EditableRequest`/`EditableResponse` — use `textToBase64` /
`base64ToText` / `base64ToBytes` from `lib/encoding.ts`.

Desktop integration (`window.__AKEV__.bridge`) is wrapped by `lib/platform.ts`:
`copyText`, `downloadUrl`, `saveData`, `pickFiles`, `openExternal`, `notifyNative` — always use
these instead of touching browser APIs directly. Desktop-only: `pickNativePath` (a native file
or folder dialog that returns the path; hide the button when `canPickNativePath()` is false),
`listenDesktopEvent` and `setDesktopWindowTitle`. `app/desktop/` handles the shell's events:
`akev://menu` (the macOS menu bar; `menu.ts` makes sure a menu accelerator and a UI shortcut
never both act on one keypress) and `akev://imported` (files opened with Akev), and mirrors
`document.title` into the window title. Capture files dropped on the Traffic page go through
`importCaptureFiles` (`app/sessions/actions.ts`), which detects the format per file.

---

## How to…

### Add a page

1. Create `src/pages/<name>/<Name>Page.tsx` exporting the component **as default** (pages
   are code-split) and a `<Name>Page.module.css`.
2. Add the route(s) to `ROUTES` in `app/router.ts` (and the `PageId` union), register the
   lazy import in `PAGES`/`TITLES` in `app/App.tsx`, and — for primary pages — an entry in
   `app/navItems.ts` (gives it a nav rail item, `Mod+N` shortcut and palette command).
3. Read params with `useRouteParam('id')`; navigate with `navigate('/rules/abc')`
   (`{ replace: true }` for selection-driven URL updates — writes are coalesced).
4. Page layout: a `Toolbar` on top, then the content area filling the rest
   (`display:flex; flex-direction:column; flex:1; min-height:0`).

### Add a command (palette)

```tsx
useCommands([
  { id: 'traffic.clear', title: 'Clear traffic', group: 'Traffic', icon: Trash2, shortcut: 'mod+shift+x', run: clear },
  { id: 'traffic.export-visible', title: 'Export visible as HAR', group: 'Traffic', icon: Download, disabled: visible.length === 0, run: exportVisible },
]);
```

Commands live while the component is mounted. Passing a fresh array every render is fine —
the registry only rebuilds when what the palette *displays* changes, and `run` always calls
the latest closure. For expensive lists use the factory form
`useCommands(() => build(items), [items])`. Results that depend on the typed text (e.g.
"jump to #123") use `useCommandProvider({ id, provide(search) { … } })`. Groups are ordered by
`GROUP_ORDER` in `app/commands.ts`.

### Add a keyboard shortcut

```tsx
useShortcut('mod+shift+x', clearTraffic, { description: 'Clear traffic', group: 'Traffic' });
useShortcut('mod+enter', resume, { allowInInputs: true, description: 'Resume', group: 'Breakpoints' });
useShortcutDocs([{ combo: 'j or arrowdown', description: 'Next exchange', group: 'Traffic list' }]);
```

* `mod` = ⌘ on macOS, Ctrl elsewhere. Display with `<Kbd combo="mod+k" />` (shows the keys the
  user rebound it to) or `shortcutLabel(displayCombo('mod+k'))`.
* Shortcuts with a `description` are rebindable (Settings › Shortcuts): their id is group +
  description + default combo (`bindingId`), the overrides live in `UiPrefs.shortcuts`. Page
  shortcuts are listed there from `pages/settings/shortcutCatalog.ts` while their page isn't
  open — keep its entries identical to the `useShortcut` calls.
* Shortcuts never fire while typing in inputs (opt in with `allowInInputs`) or inside open
  dialogs/menus (`allowInOverlays`), and ignore events a component already handled
  (`event.preventDefault()`).
* The most recently registered handler wins; return `false` from a handler to pass the event
  on. Anything with a `description` shows up in the `?` overlay; keys a component handles
  itself (list navigation) are documented with `useShortcutDocs`.
* A `shortcut` on a command or `IconButton` is display-only — bind it with `useShortcut`.

### Show feedback

```ts
toast.success('Proxy address copied', { description: address });
toast.error("Couldn't save rules", { description: e.message, action: { label: 'Retry', onClick: save } });
// Error toasts have a close button; with an action they stay until dismissed.
undoToast('Deleted 3 exchanges', restore);                      // prefer undo over confirm (§1.6)
if (await confirm({ title: 'Clear all traffic?', message: '3 pinned exchanges will be deleted too.', destructive: true, confirmLabel: 'Clear all' })) …
```

---

## Primitives (`components/ui`)

All primitives accept `className`, forward refs (React 19 `ref` prop) and are keyboard
accessible. Browse them live at **`#/_kitchen-sink`** (dev server only, with `?mock`), which
renders every primitive in every state — keep it up to date when you add or change one.
Production builds don't include the page.

```tsx
import { Button, IconButton, Input, SearchInput, Select, SegmentedControl, Switch, Tabs, TabList, TabsContent,
         Tooltip, Menu, ContextMenu, Dialog, Sheet, Badge, MethodBadge, StatusBadge, Kbd, KeyValueEditor,
         CodeEditor, HexViewer, JsonTree, EmptyState, SplitPane, Toolbar, Section, Field, CopyButton, AppIcon } from '../../components/ui';

<Button variant="primary" icon={Send} kbd="mod+enter" loading={sending} onClick={send}>Send</Button>
<IconButton label="Pause recording" icon={Circle} shortcut="mod+shift+r" active={!recording} onClick={toggle} />
<SearchInput value={q} onValueChange={setQ} shortcut="mod+f" mono placeholder="Filter…" />
<NumberInput value={ms} onValueChange={setMs} unit="ms" min={0} stepper />
<Select value={target} onValueChange={setTarget} options={[{ label: 'Shell', options: [{ value: 'curl', label: 'cURL' }] }]} aria-label="Target" />
<SegmentedControl value={mode} onValueChange={setMode} items={[{ value: 'pretty', label: 'Pretty' }, { value: 'raw', label: 'Raw' }]} />

<Tabs value={tab} onValueChange={setTab}>
  <TabList items={[{ value: 'headers', label: 'Headers', count: 12 }, { value: 'body', label: 'Body', badge: '2.4 KB' }]} />
  <TabsContent value="headers">…</TabsContent>
</Tabs>

<Menu trigger={<Button iconRight={ChevronDown}>Export</Button>} items={[
  { label: 'Export all as HAR', icon: Download, onSelect: exportAll },
  { type: 'separator' },
  { label: 'Delete', icon: Trash2, destructive: true, shortcut: 'delete', onSelect: remove },
]} />
<ContextMenu items={() => rowMenu(id)}><div role="row">…</div></ContextMenu>   {/* items evaluated on open */}

<Dialog open={open} onOpenChange={setOpen} title="Import rules" footer={<Button variant="primary">Import</Button>}>…</Dialog>
<Sheet open={open} onOpenChange={setOpen} title="Android device">…</Sheet>

<MethodBadge method="POST" fixedWidth />
<StatusBadge summary={summary} />                         {/* pending / paused / errors / WS handled */}
<CodeEditor value={text} language="json" readOnly fill contentId={`${id}:response`} />
<HexViewer data={bytes} />  <JsonTree data={value} />
<SplitPane id="traffic.detail" defaultRatio={0.55} minFirst={320}>{list}{detail}</SplitPane>
<KeyValueEditor rows={rows} onChange={setRows} nameSuggestions={KNOWN_HEADER_NAMES} />
```

Notes:

* **CodeEditor** lazy-loads CodeMirror and each language (`json`, `javascript`, `typescript`,
  `html`, `xml`, `css`, `markdown`, `yaml`, `shell`, `http`, `python`, `go`, `rust`, `java`,
  `kotlin`, `csharp`, `dart`, `ruby`, `swift`, `powershell`, `protobuf`); loose names
  (`curl`, `raw-http`, content kinds) go through `normalizeLanguage`. Pass `contentId` so
  switching exchanges resets scroll/selection. The theme is driven by CSS variables, so theme
  switches need no reconfiguration. Imperative handle: `openSearch()`, `foldAll()`, `focus()`.
* **Virtualised** primitives (`HexViewer`, `JsonTree`) fill their parent by default; pass
  `fill={false}` + `maxHeight` to size to content.
* `KeyValueEditor` keeps a trailing empty row, splits pasted `Name: value` lines into rows and
  has a bulk-text mode. Convert with `headersToRows` / `rowsToHeaders` (wire order preserved).
* `Tooltip` children must forward refs (all primitives do). `IconButton` already has one.
* `QrCode` renders dark-on-white regardless of theme (phones must scan it).

`lib/http.ts` has the domain helpers pages need: `statusText`, `statusClass`,
`methodColorVar`, `contentKindLabel/ColorVar`, `lookupHeaderDoc` (136 bundled MDN-style
header docs), `parseCookieHeader`, `parseSetCookie`, `decodeJwt`,
`describeError` (plain-language error titles/fixes per `ErrorKind`), `typeChipClause`,
`renderRawMessage`. `lib/query.ts` is the one query-string parser (`splitUrl`,
`parseQueryString` keeps decoded and raw forms). `lib/format.ts` formats bytes, durations,
times, numbers and dates (system locale). `lib/ruleFromExchange.ts` builds rules from a
captured exchange.

---

## Mock mode

On the dev server, open any URL with **`?mock`** (e.g. `http://localhost:5173/?mock#/traffic`)
to run the whole UI against an in-browser mock daemon (`src/api/mock/`). It implements every
REST endpoint and the stream through the same `Transport` the real client uses, so mock mode
exercises the real code paths. Production builds don't contain it; `VITE_MOCK=1 bun run build`
makes a build that always runs against the mock (for demos and screenshots of `dist/`).

What it generates:

* ~250 seeded exchanges plus a new one every ~300 ms (bursts included): api.github.com,
  cdn.jsdelivr.net, www.google.com, localhost:3000, gRPC, telemetry (protobuf), media,
  fonts, images; every method; 2xx–5xx; aborted exchanges (refused, timeout, DNS, cert,
  reset), TLS failures, tunnels, a paused breakpoint (always at least one), WebSockets and
  SSE with live messages, mocked/modified/pinned/highlighted/noted rows, and recorded LLM API
  calls (OpenAI streaming tool calls and Markdown answer, Anthropic streaming with thinking +
  tool use, Gemini, Ollama, an Anthropic 529) whose SSE events replay as messages. Their
  `GET /api/exchanges/:id/llm` conversations (`api/mock/llmFixtures.ts`) are generated by the
  daemon's own parser from `crates/akev-content/tests/fixtures/llm`
  (`bun run scripts/llm-fixtures.ts`).
* Bodies for JSON, HTML, JS, CSS, SVG/PNG, multipart, protobuf/gRPC (with `structured`
  decodes), SSE; `BodyView`s with formatting and encodings.
* Rules (groups, mocks, breakpoints, rewrites…), rule stats, 14 interceptors (browsers,
  terminals, Android, iOS, Docker, system proxy, manual, JVM, Electron) with
  available/unavailable/active states, settings, CA info, codegen for all targets, filter
  validation/suggestions/docs (a real implementation of the filter language).

Console helpers: `__akevMock.drop(5000)` drops the stream and refuses reconnects for 5 s
(exercises the reconnect UI); `__akevMock.stop()` stops generating traffic.

URL flags for states that need a particular daemon: `?mock&empty` is a fresh install (no
traffic, nothing intercepted, CA not trusted: the first-run setup shows when its "done"
preference isn't set; launching an interceptor starts the traffic), `?mock&still` generates no new
traffic (the visual tests pair it with a fixed clock), `?mock&restored` reports a restored
autosave at startup (the "Restored autosaved session" toast), `?mock&fallback` has the proxy
listening on the port after the stored one (Settings → Proxy → *Use port 8000*). Licensing: the
mock starts as **Free with the trial available**; `?pro` (active license), `?trial` (9 days left),
`?trialEnded`, `?updatesEnded` and `?invalidLicense` pick another state. Any key starting with
`AKEV1.` activates (`AKEV1.old…` = a key whose updates ended), Pro routes answer 402
`pro-required` without a license, and `__akevMock.expireTrial()` ends a running trial. The mock also
has a seeded session library (`/api/sessions`) and accepts Charles / mitmproxy imports.

## Screenshots

Visual review is part of the definition of done. With the dev server running
(`bun run dev`):

```bash
bun run shot                                                   # traffic, dark, 1440×900 → /tmp/akev-shot.png
bun run shot -- --route _kitchen-sink --section data --theme light --out /tmp/ks-data.png
bun run shot -- --route rules --theme midnight --width 1280 --height 800
bun run shot -- --press mod+k --type theme --out /tmp/palette.png              # scripted (CDP)
bun run shot -- --route _kitchen-sink --section overlays --click 'button[aria-haspopup="menu"]'
bun run shot -- --eval '__akevMock.drop(60000)' --sleep 2500                  # reconnecting state
```

`--theme` sets a session-only override (`?theme=dark|light|midnight|contrast`, also handy in
a normal browser). Interaction flags (`--press`, `--type`, `--click`, `--rclick`, `--hover`,
`--sleep`, `--eval`) run in order through the Chrome DevTools Protocol. Raw equivalent:

```bash
google-chrome --headless=new --disable-gpu --hide-scrollbars --window-size=1440,900 \
  --virtual-time-budget=4000 --screenshot=/tmp/akev-shot.png "http://localhost:5173/?mock#/traffic"
```

Check: 4 px alignment, consistent control heights, no overflow/ellipsis where text should
fit, contrast in **both** dark and light, and hover/focus states.

## Testing

`bun test` runs the unit tests (`src/**/*.test.ts`): TrafficStore (ingestion, filter deltas,
selection, 100k-row ingest), stream client (backoff, keepalive, resync), API client & auth,
router, shortcuts, palette ranking, UI prefs sanitising, formatting/HTTP helpers, token
contrast, CodeMirror HTTP grammar, and the mock backend. Put logic where it can be tested
without a DOM.

Component tests (`src/**/*.test.tsx`) render with Testing Library on happy-dom, which
`bunfig.toml` preloads (`src/test/dom.ts`, `src/test/cleanup.ts`). Import the helpers from
`src/test/render.tsx`: `render` (adds the tooltip and confirm-dialog providers), `screen`,
`userEvent`, `waitFor`, `stub(obj, key, impl)` (replaces an API method or store action for
one test and records its calls) and the `summary()` / `testStatus()` fixtures. Query by role
and accessible name, as a user would find things:

```tsx
stub(api.interceptors, 'activate', async () => ({ success: true }));
render(<InterceptorTile info={chrome} />);
await userEvent.click(screen.getByRole('button', { name: 'Launch Chrome' }));
```

Stores are module singletons: reset what a test changes (`useAppStore.setState(...)` in a
`beforeEach`). A test that replaces `window` must put the DOM's back afterwards.

### End-to-end and visual tests (Playwright)

```bash
bunx playwright install chromium   # once
bun run e2e                        # both projects (starts the servers it needs); or `make e2e`
AKEV_E2E_PROJECT=daemon bun run e2e -- -g breakpoint
bun run e2e -- --project visual --update-snapshots=all   # re-render the baselines
```

* `e2e/daemon/` drives the UI served by a real, ephemeral Akev (`cargo run -p akev-cli -- start
  --ephemeral …`, debug build, API on :45491 with a fixed token, proxy on a random port) and sends
  traffic through its proxy to a local upstream server: loading with the token, the list, the
  filter, the detail pane, breakpoints, a mock rule made from an exchange, Compose, HAR export,
  the license flows and the first-run setup. Build the UI first (`bun run build`): the debug
  daemon serves `ui/dist` from disk. The activation of a valid license only runs against a
  daemon built with `AKEV_LICENSE_PUBLIC_KEY` set to the test key and `AKEV_E2E_TEST_LICENSE=1`
  (CI does both; locally, uncomment both in `.env` and run `make e2e`). The test daemon never
  sees your own `AKEV_DATA_DIR`, `AKEV_TOKEN` or `AKEV_LICENSE`.
* `e2e/visual/` screenshots five screens in light and dark against a `VITE_MOCK=1` build
  (`?still` and a fixed clock keep them identical between runs) and compares them with
  `visual/__screenshots__/` (1.5% tolerance). The baselines are rendered in the
  `mcr.microsoft.com/playwright` image CI uses, so render new ones there:
  `podman run --rm --network host -v "$PWD/..":/work -w /work/ui -e AKEV_E2E_PROJECT=visual
  mcr.microsoft.com/playwright:v1.65.0-noble npx playwright test -c e2e --update-snapshots=all`.

## Performance budget (§9)

Initial JS — what loads before the first Traffic paint — is ≈ 218 KB gzip (initial CSS
≈ 22 KB). `bun run build && bun run size` reports it and fails over the budget in
`scripts/size.ts` (measured size + ~10%; CI runs it). What keeps it there:

* Every page but Traffic is lazy, and so are the Traffic detail views (loaded on first
  selection and prefetched when the page is idle), the compare view, the command palette,
  the shortcuts overlay, the Sessions dialog, CodeMirror and its languages, the LLM view,
  the header docs, the app-icon artwork and the rule builders.
* `package.json` declares the sources side-effect free (`sideEffects`), so importing one
  primitive from the `components/ui` barrel doesn't pull in the others.
* Big JSON bodies (over 512 KB) are pretty-printed in a Web Worker (`body/pretty.ts`), like
  large diffs (`diff/useTextDiff.ts`).

Lists must be virtualised (`@tanstack/react-virtual`), rows memoised on summary identity,
and per-row store subscriptions kept cheap (`useIsSelected(id)`).
