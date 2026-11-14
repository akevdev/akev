# Akev — UI & UX Design Specification

This is the blueprint for the Akev web UI (served by the daemon and embedded in the desktop
app). Every UI contributor follows it so the product feels like one coherent, premium tool.

North stars: **Proxyman's polish, Linear's speed and restraint, Raycast's keyboard-first flow,
Chrome DevTools' familiarity.** Everything must stay smooth with 100k+ exchanges.

---

## 1. Principles

1. **Instant.** Every interaction responds within a frame. Lists are virtualised, updates are
   batched per animation frame, heavy work (filtering, decoding, formatting) happens in Rust.
2. **Calm density.** Information-dense like a pro tool, but with generous rhythm: consistent
   4px grid, quiet borders, colour reserved for meaning (methods, statuses, states).
3. **Keyboard-first, mouse-perfect.** Every action has a shortcut or palette entry; every
   control has hover/focus/active/disabled states, tooltips with shortcut hints, and correct
   cursor feedback.
4. **No dead ends.** Empty states teach and offer the next action. Errors explain cause and fix
   ("The server's certificate expired 3 days ago — add this host to *Ignore certificate errors*").
5. **Never paywall the basics.** Every feature HTTP Toolkit gates behind Pro (rule persistence,
   snippets, HAR export, themes, settings, resend, body download) is free in Akev. Akev Pro
   ($49 once) covers power features only, never takes data away (without it, Pro settings and
   rules are kept, just inactive), and its locks explain themselves with a way forward (trial,
   key, pricing) instead of a dead end.
6. **Reversible.** Destructive actions are undoable (toast with Undo) rather than confirmed,
   except clearing everything with pinned items (confirm).

---

## 2. Tech stack (ui/)

* **React 19 + TypeScript (strict) + Vite**, package manager **bun**.
* **CSS Modules** (`*.module.css`) + global design tokens (`src/styles/tokens.css`) as CSS custom
  properties. No CSS-in-JS runtime. Use `clsx` for class composition.
* **Radix UI primitives** for accessible overlays/controls: `@radix-ui/react-dropdown-menu`,
  `react-context-menu`, `react-dialog`, `react-tooltip`, `react-popover`, `react-tabs`,
  `react-switch`, `react-select`, `react-toggle-group`, `react-scroll-area` (optional).
* **cmdk** for the command palette. **sonner** for toasts.
* **CodeMirror 6** for all code/text viewing & editing (`@codemirror/view`, `state`, `language`,
  `commands`, `search`, `lang-json`, `lang-html`, `lang-xml`, `lang-javascript`, `lang-css`,
  `lang-markdown`, `lang-yaml`, `legacy-modes` for shell/http/protobuf).
* **@tanstack/react-virtual** for virtual lists. **@dnd-kit** for rule drag & drop.
* **lucide-react** icons (stroke 1.75, size 16 default, 14 in dense rows, 18 in nav).
* **zustand** for app state; the traffic store is a custom high-performance external store
  (see §9) consumed via `useSyncExternalStore`.
* Fonts bundled offline: **Inter Variable** (`@fontsource-variable/inter`) for UI and
  **JetBrains Mono Variable** (`@fontsource-variable/jetbrains-mono`) for code/data.
* Routing: tiny hash router (`#/traffic`, `#/traffic/123`, `#/rules/<id>`, `#/compose`,
  `#/connect`, `#/settings/<section>`), works identically in browser & Tauri.

---

## 3. Design tokens (`src/styles/tokens.css`)

Themes are applied with `data-theme` on `<html>`: `dark` (default), `light`, `midnight`
(OLED black), `contrast` (high contrast). `system` follows `prefers-color-scheme`
(dark ↔ light). Token names are **identical across themes**; only values change.

### 3.1 Colour — dark (default)

```css
--bg-app:        #0a0b0e;   /* window background, behind panels */
--bg-panel:      #101216;   /* main surfaces (list, detail) */
--bg-subtle:     #14161b;   /* toolbars, headers, table header */
--bg-elevated:   #191c22;   /* popovers, menus, dialogs, cards */
--bg-input:      #0c0d11;
--bg-hover:      rgba(255,255,255,0.045);
--bg-active:     rgba(255,255,255,0.07);
--bg-selected:   rgba(124,92,255,0.16);
--bg-selected-strong: rgba(124,92,255,0.26);

--border:        rgba(255,255,255,0.07);
--border-strong: rgba(255,255,255,0.12);
--border-focus:  #7c5cff;

--text:          #e8eaef;
--text-muted:    #a0a6b3;
--text-subtle:   #6c7281;
--text-disabled: #4a4f5a;
--text-on-accent:#ffffff;

--accent:        #7c5cff;   /* Akev violet */
--accent-hover:  #8d71ff;
--accent-active: #6b4cf0;
--accent-soft:   rgba(124,92,255,0.14);
--accent-text:   #a996ff;   /* accent coloured text on dark bg */
--brand-gradient: linear-gradient(180deg, #8468ff 0%, #6a4cf2 100%);  /* one hue, never violet→blue */

--success: #2fcf8a;  --success-soft: rgba(47,207,138,0.14);
--warning: #f5b544;  --warning-soft: rgba(245,181,68,0.14);
--danger:  #ff5d6c;  --danger-soft:  rgba(255,93,108,0.14);
--info:    #4ea4ff;  --info-soft:    rgba(78,164,255,0.14);

/* HTTP methods */
--m-get: #4ea4ff; --m-post: #2fcf8a; --m-put: #f59f44; --m-patch: #e6c84f;
--m-delete: #ff5d6c; --m-head: #a78bfa; --m-options: #8b93a7; --m-connect: #22d3ee; --m-other: #a0a6b3;

/* status classes */
--s-1xx: #8b93a7; --s-2xx: #2fcf8a; --s-3xx: #4ea4ff; --s-4xx: #f5b544; --s-5xx: #ff5d6c; --s-err: #ff5d6c;

/* content kinds (type column icons) */
--k-json: #f5b544; --k-html: #f58a4e; --k-js: #e6d24f; --k-css: #4ea4ff; --k-image: #f06fb4;
--k-media: #a78bfa; --k-font: #34d3c0; --k-xml: #f58a4e; --k-text: #a0a6b3; --k-binary: #6c7281;
--k-ws: #22d3ee; --k-grpc: #7c9cff; --k-form: #8fd16a;

/* user highlight colours */
--tag-red: #ff5d6c; --tag-orange: #f59f44; --tag-yellow: #e6c84f; --tag-green: #2fcf8a;
--tag-blue: #4ea4ff; --tag-purple: #a78bfa; --tag-grey: #8b93a7;

--shadow-sm: 0 1px 2px rgba(0,0,0,0.4);
--shadow-md: 0 4px 16px rgba(0,0,0,0.45), 0 0 0 1px var(--border);
--shadow-lg: 0 16px 48px rgba(0,0,0,0.55), 0 0 0 1px var(--border-strong);
--scrim: rgba(5,6,8,0.6);
```

### 3.2 Colour — light

```css
--bg-app: #eceef2; --bg-panel: #ffffff; --bg-subtle: #f6f7f9; --bg-elevated: #ffffff;
--bg-input: #ffffff; --bg-hover: rgba(15,18,30,0.045); --bg-active: rgba(15,18,30,0.07);
--bg-selected: rgba(110,80,240,0.10); --bg-selected-strong: rgba(110,80,240,0.18);
--border: rgba(15,18,30,0.09); --border-strong: rgba(15,18,30,0.16); --border-focus: #6e50f0;
--text: #14161c; --text-muted: #4f5563; --text-subtle: #808694; --text-disabled: #b4b8c2;
--accent: #6e50f0; --accent-hover: #7d61f5; --accent-active: #5d40e0; --accent-soft: rgba(110,80,240,0.10);
--accent-text: #5b3ee0;
--success: #12a868; --warning: #c98a0b; --danger: #e5334a; --info: #1f7fe0;
--m-get: #1f7fe0; --m-post: #12a868; --m-put: #d9730d; --m-patch: #b8960a; --m-delete: #e5334a;
--m-head: #7c5cd6; --m-options: #6b7280; --m-connect: #0e9fb5;
--s-2xx: #12a868; --s-3xx: #1f7fe0; --s-4xx: #c98a0b; --s-5xx: #e5334a;
--shadow-md: 0 4px 16px rgba(15,18,30,0.10), 0 0 0 1px var(--border);
--shadow-lg: 0 16px 48px rgba(15,18,30,0.16), 0 0 0 1px var(--border);
--scrim: rgba(20,22,28,0.35);
```

`midnight`: dark with `--bg-app:#000; --bg-panel:#050506; --bg-subtle:#0a0a0c; --bg-elevated:#111114`.
`contrast`: dark with pure white text, `--border: rgba(255,255,255,0.28)`, saturated status colours,
focus rings 2px.

### 3.2.1 Brand mark

One mark everywhere: a capital A drawn as a chevron, its crossbar replaced by a single dot (the
request Akev is holding between the two sides of a connection). Sources live in `brand/` and
are generated by `brand/build.ts`; `brand/render.sh` rasterises them into the UI's PNG favicons,
the desktop icons and the installer art (`make brand` runs both).

* **App icon / favicon** (`akev-icon.svg`, `ui/public/favicon.svg`): ink squircle
  (`#22232b` → `#0d0e12`, 10% white hairline), white chevron, violet dot `#8a70ff`.
* **In-app mark** (`Logo.tsx`, `akev-mark.svg`): no tile. Legs `--text`, dot `--accent`, so it
  follows the theme.
* Flat only: no glow, gloss, drop shadow or two-hue gradient on the mark. Violet is the only
  brand colour; the dot is the only place the icon uses it.
* Wordmark: "Akev" in Inter, weight 620, letter-spacing −0.02em, set in `--text`.

### 3.3 Typography

```css
--font-ui:   "Inter Variable", "Inter", system-ui, -apple-system, "Segoe UI", sans-serif;
--font-mono: "JetBrains Mono Variable", "JetBrains Mono", ui-monospace, "SF Mono", Menlo, monospace;
--fs-2xs: 10.5px; --fs-xs: 11px; --fs-sm: 12px; --fs-base: 13px; --fs-md: 14px;
--fs-lg: 16px; --fs-xl: 20px; --fs-2xl: 26px;
--lh-tight: 1.25; --lh-base: 1.45;
--fw-regular: 450; --fw-medium: 530; --fw-semibold: 620;
```
Inter with `font-feature-settings: "cv11", "ss01", "tnum"` on numeric columns (tabular
numbers for sizes/durations/ids). UI base size 13px. A user setting scales via
`--ui-scale` (0.9 / 1 / 1.1 / 1.2) applied to `html { font-size }`, all sizes in px are
multiplied with `calc(var(--ui-scale) * Npx)` only for text tokens.

### 3.4 Spacing, radius, motion, sizing

```css
--sp-1: 4px; --sp-2: 8px; --sp-3: 12px; --sp-4: 16px; --sp-5: 20px; --sp-6: 24px; --sp-8: 32px;
--r-xs: 4px; --r-sm: 6px; --r-md: 8px; --r-lg: 12px; --r-full: 999px;
--dur-fast: 90ms; --dur: 150ms; --dur-slow: 240ms; --ease: cubic-bezier(.2,.7,.2,1);
--nav-width: 52px; --toolbar-h: 40px; --statusbar-h: 24px;
--row-h: 30px;   /* density: compact 26px, comfortable 34px */
--control-h: 28px; --control-h-sm: 24px; --control-h-lg: 34px;
```
`@media (prefers-reduced-motion: reduce)` disables transitions/animations except opacity.

### 3.5 Focus & selection

* Focus ring: `outline: 2px solid var(--border-focus); outline-offset: 1px;` only on
  `:focus-visible`.
* Text selection: `::selection { background: var(--accent-soft-strong, rgba(124,92,255,.35)); }`.
* Scrollbars: thin (8px), track transparent, thumb `var(--border-strong)` → hover `var(--text-subtle)`.

---

## 4. App shell

```
┌────┬──────────────────────────────────────────────────────────────────┐
│ ◆  │  <page>                                                          │
│    │                                                                  │
│ ⇅  │                                                                  │
│ ⚙︎  │                                                                  │
│ ✉  │                                                                  │
│ ⌁  │                                                                  │
│    │                                                                  │
│ ◐  │                                                                  │
├────┴──────────────────────────────────────────────────────────────────┤
│ ● Proxy 127.0.0.1:8000 ⧉ │ ● Recording │ 1,234 / 5,678 │ 2 paused │ 48 MB │ CA ✓ │
└───────────────────────────────────────────────────────────────────────┘
```

**Nav rail** (left, `--nav-width`, `--bg-app`, right border): Akev logo mark (chevron "A"
with an accent dot, 28px, tooltip shows version & ports) → nav items (36×36 icon buttons, radius `--r-md`,
active = `--bg-active` + 2px accent indicator bar at the left edge + icon `--text`; inactive icon
`--text-subtle`, hover `--text-muted`):

| Item | Icon | Route | Shortcut |
|---|---|---|---|
| Traffic | `ArrowLeftRight` | `#/traffic` | Ctrl/⌘ 1 |
| Rules | `Workflow` (or `SlidersHorizontal`) | `#/rules` | Ctrl/⌘ 2 |
| Compose | `Send` | `#/compose` | Ctrl/⌘ 3 |
| Connect | `Plug` | `#/connect` | Ctrl/⌘ 4 |
| — spacer — | | | |
| Breakpoints (only when ≥1 paused; amber badge with count; click → first paused exchange) | `CirclePause` | | |
| Theme toggle (cycles system/dark/light; tooltip shows current) | `SunMoon` | | |
| Settings | `Settings` | `#/settings` | Ctrl/⌘ , |

Tooltips appear to the right after 400ms (`Traffic  ⌘1`).

**Status bar** (bottom, `--statusbar-h`, `--bg-app`, top border, `--fs-xs`, `--text-muted`):
segments separated by 1px dividers, each a button with hover bg:
proxy status dot (green = listening, red = error with tooltip) + address (click: copy, toast
"Proxy address copied") · recording state (● Recording / ❚❚ Paused — click toggles) ·
`visible / total` exchange counts · paused breakpoints (amber, click jumps) · memory
(`48 MB` + spilled) · CA status (✓ trusted-looking / ⚠ expiring in N days) · right side:
active interceptors (`Chrome, Terminal`), the licensee while Pro is active (`Licensed to Ada
Lovelace`, name only; tooltip adds the email and updates-until; click → Settings → License)
and daemon connection state (if disconnected: red "Reconnecting…" pill).

**Command palette** (Ctrl/⌘ K, also Ctrl/⌘ Shift P): cmdk dialog, 640px wide, top 15vh,
`--bg-elevated`, `--shadow-lg`, radius `--r-lg`. Groups: *Navigation*, *Traffic* (clear,
pause/resume recording, export HAR, import, toggle detail layout), *Rules* (new rule from each
template, enable/disable all), *Compose* (new request, paste cURL), *Connect* (launch each
available interceptor), *Appearance* (themes, density), *Exchanges* (type `#123` or a URL
fragment to jump to an exchange). Each item: icon, label, right-aligned shortcut `Kbd`s.

**Toasts**: sonner, bottom-right, `--bg-elevated`, radius `--r-md`, max 3 visible; success
(green icon), error (red, persists until dismissed when actionable), with action buttons
(Undo / Open / Retry).

**Global keyboard shortcuts** (rebindable in Settings › Shortcuts; shown in palette & tooltips; never hijack
when focus is in a text input unless noted; on macOS `⌘` replaces Ctrl):

| Shortcut | Action |
|---|---|
| Ctrl 1–4, Ctrl , | navigate |
| Ctrl K | command palette |
| ? (outside inputs) | keyboard shortcuts overlay |
| Ctrl F / `/` | focus traffic filter |
| Ctrl Shift R | toggle recording |
| Ctrl Shift X | clear traffic (undoable) |
| Ctrl Shift D | toggle detail pane position (right / bottom) |
| Esc | close overlay → clear filter focus → deselect |

---

## 5. Primitives (`src/components/ui/`)

Build these first; every page uses them. All support `className`, forward refs, and keyboard
access. Sizes `sm` (24px) / `md` (28px, default) / `lg` (34px).

* **Button** — variants: `primary` (accent bg, white text), `secondary` (bg-elevated + border),
  `ghost` (transparent, hover bg-hover), `danger` (danger bg), `subtle-danger` (danger text). Icon
  slot left/right, `loading` spinner, `kbd` hint slot. Radius `--r-sm`. Medium weight.
* **IconButton** — square ghost button with required `label` (tooltip + aria-label), optional
  `shortcut` shown in tooltip, `active` state.
* **Input**, **TextArea**, **SearchInput** (leading icon, clear ×), **NumberInput** (with unit
  suffix), **Select** (Radix), **Combobox** (input + suggestions), **Switch**, **Checkbox**,
  **SegmentedControl** (toggle group, pill-style, used for view modes), **Slider**.
* **Tabs** — underline style for section tabs (accent 2px underline, text-muted → text);
  optional count badges; overflow scrolls horizontally with fade edges.
* **Tooltip** (400ms delay, 100ms skip delay, `--bg-elevated`, `--fs-xs`, shortcut keys
  rendered with `Kbd`), **Popover**, **DropdownMenu**, **ContextMenu** (Radix; items with
  icon, label, shortcut, submenus, separators, checkbox items, destructive red items).
* **Dialog** (centered, max 560px default, scrim blur 2px, enter animation scale .98→1 +
  fade), **Sheet** (side drawer from right, used for interceptor configs).
* **Badge/Pill** — tiny rounded labels: `neutral`, `accent`, `success`, `warning`, `danger`,
  `info`; **MethodBadge** (method text in method colour, mono, semibold, fixed width 56px in
  lists), **StatusBadge** (status code coloured by class, + short reason tooltip; errors show
  icon + short label e.g. "Refused", "TLS", "Timeout").
* **Kbd** — `<kbd>` with bg-subtle, border, radius 4px, mono 10.5px.
* **KeyValueEditor** — editable table of name/value rows (enable checkbox, name, value, delete),
  auto-appends an empty row, paste of `Name: value` lines splits into rows, bulk-edit text mode
  toggle. Used by Compose, breakpoints, rule forms.
* **CodeEditor** — CodeMirror 6 wrapper: `value`, `language`, `readOnly`, `onChange`,
  `wrap`, `lineNumbers`, `minHeight/maxHeight`/`fill`, theme from tokens (see §5.1),
  search panel styled, folding gutter for JSON/XML, `placeholder`.
* **HexViewer** — virtualised rows of 16 bytes: offset gutter (mono, subtle), hex pairs with
  per-byte hue colouring (subtle), ASCII column; selection syncs; copy as hex/base64.
* **JsonTree** — collapsible tree with type-coloured values, copy path/value, expand all,
  search highlight.
* **EmptyState** — icon in a soft circle, title (`--fs-md` semibold), description (muted), actions.
* **SplitPane** — resizable (pointer events, 1px visual divider with 7px hit area, cursor
  col/row-resize, double-click resets), min sizes, persisted ratio (per id in UI settings),
  collapsible side.
* **Spinner**, **Skeleton**, **ProgressBar**, **Toolbar** (40px row with gap 6px, left/right
  slots), **Section** (titled block in settings: title, description, children), **Field** (label,
  hint, inline error).
* **CopyButton** — icon button that flips to a ✓ for 1.2s.
* **ColorDot**, **Avatar/AppIcon** (process/interceptor icons: chrome, firefox, edge, brave,
  safari, terminal, node, python, curl, android, apple, docker, java, go, rust, unknown).

### 5.1 CodeMirror theme
Background `--bg-panel` (or transparent), gutter `--bg-panel` with `--text-disabled` numbers,
active line `rgba(255,255,255,0.025)`, selection `--accent-soft`, cursor `--accent`. Syntax:
keys/properties `#9cb4ff` (light: `#3553c4`), strings `#7dd3a8` (light `#0f8a55`), numbers
`#f5b544` (light `#b06d00`), booleans/null `#c792ea` (light `#8a3fc0`), tags `#ff8a7a`,
attributes `#f5b544`, comments `--text-subtle` italic, punctuation `--text-muted`. Font
`--font-mono` 12.5px, line-height 1.55.

---

## 6. Traffic page (`#/traffic`) — the heart of the product

### 6.1 Layout
```
┌──────────────────────────────────────────────────────────────────────────────┐
│ [🔍 filter: method:post status:>=400 …            ⌘F] [All|Fetch|Doc|JS|CSS|Img|Media|WS|Other] │ ⏺ ⌫ ⤓ ⫶ │
├─────────────────────────────────────────┬────────────────────────────────────┤
│ list header (sortable, resizable cols)  │  detail pane                       │
│ ▌GET  200 api.example.com /v1/users  … │                                    │
│ ▌POST 201 …                             │                                    │
│ …virtualised…                           │                                    │
│                         [↓ 23 new]      │                                    │
└─────────────────────────────────────────┴────────────────────────────────────┘
```
Detail pane position: right (default when window ≥ 1100px wide) or bottom (toggle,
remembered). Split ratio persisted. When nothing is selected the detail pane shows a
helpful placeholder (shortcut hints + live stats), or the multi-select summary.

### 6.2 Toolbar
* **Filter input** (flex 1, height 28px, mono 12px): token-level syntax highlighting (fields
  in accent-text, operators muted, values text, negation danger, errors with wavy underline +
  tooltip message from `/api/filter/validate`), autocomplete dropdown (`/api/filter/suggest`,
  debounced 60ms, ↑/↓/Tab/Enter to accept, shows `label`+`detail`), history (↑ in empty
  input recalls recent filters), `?` help popover listing all fields (`/api/filter/docs`) with
  clickable examples, × clear. Saved filters: star button saves current query with a name;
  dropdown lists saved filters.
* **Type segmented control**: All · Fetch/XHR (json/xml/protobuf/grpc/form) · Doc (html) · JS ·
  CSS · Img · Media (audio/video) · Font · WS (websocket/sse) · LLM (`is:llm`) · Other — combines with the filter
  (AND). Collapses into a dropdown when narrow.
* Right buttons (IconButtons): Record/Pause recording (red dot when recording, `Ctrl Shift R`),
  Clear (`Ctrl Shift X`, undo toast), Import/Export dropdown (Import HAR…, Import .akev…,
  Import Charles session (.chlsj)…, Import mitmproxy flows…, Export all as HAR, Export visible as
  HAR, Save as session…, Saved sessions…, Download .akev file), More (⋮): detail position,
  density, columns.
* **Sessions dialog** (also palette: "Sessions…", "Save traffic as session…"): save form (name +
  All / Visible / Selected), Autosave group and Saved group (icon, name, "N exchanges · size ·
  time ago", Open — a Replace / Add menu when traffic exists — and ⋯ Rename / Download / Delete),
  footer with the autosave state linking to Settings → Data. A restored autosave announces itself
  once per daemon run: "Restored autosaved session (1,234 exchanges)" with a *Sessions* action.

### 6.2.1 Hosts sidebar
Collapsible left sidebar (248px, `--bg-subtle`, hidden by default; toolbar button at the left of
the filter, `Ctrl/⌘ B`, palette; the open state is a UI pref). Header: Hosts · Apps
SegmentedControl, sort toggle (name / request count), hide button; a "Find a host…" search.
*Hosts* lists pinned hosts first (pin from a row's hover button or the traffic row menu *Pin host
to the sidebar*; persisted), then registrable domains (`api.example.co.uk` under
`example.co.uk`; a domain with one host is just that host) with chevrons and request counts for
the whole capture. *Apps* lists client apps (`source`) with counts. Clicking an item ANDs a clause
onto the user's filter text — `host=…`, `(host=d | host:*.d)` for a domain, `source=…` /
`-source:*` — replacing the item picked before; clicking it again removes it. While one is active
the sidebar shows an accent box "Only api.example.com ✕ — and your filter `status:4xx`" so the
composition stays visible. Counts are kept incrementally (only new ids are visited; deletions,
clears and resyncs recount) and refresh at most 4× a second; rows are virtualised.

### 6.3 Traffic list
* Virtualised (`@tanstack/react-virtual`), fixed row height `--row-h`, overscan 20, renders
  only visible rows. Row components are memoised on `(summary object identity, selected)`.
* Columns (show/hide via header context menu & ⋮ menu; drag to reorder; drag edges to resize;
  click header to sort asc/desc/none — default order = id/time; widths persisted):
  1. **Indicator** (6px): left colour stripe = user colour tag; pinned shows 📌 icon in the
     row's start; paused shows amber ❚❚; mocked shows a small "M" accent dot; modified a "✎" dot.
  2. **#** (id, subtle, tabular) — hidden by default.
  3. **Method** — MethodBadge.
  4. **Status** — StatusBadge; pending = animated subtle spinner dot; errors = icon + label.
  5. **Host** — `--text`, truncated with ellipsis (start of host kept), tooltip full.
  6. **Path** — path in `--text`, `?query` in `--text-subtle`; mono optional (setting).
  7. **Type** — kind icon in kind colour + short label (`json`, `html`, `img`…).
  8. **Size** — response size, human (`12.4 KB`), tabular, right-aligned, subtle when 0.
  9. **Time** — duration (`143 ms`, `1.2 s`), right-aligned; colour shifts to warning >1s,
     danger >5s; pending shows live elapsed.
  10. **Source** — app icon + process/interceptor name (`Chrome`, `curl`, `python3`).
  11. **Started** — `14:03:22.418` (hidden by default).
  12. **Protocol** — `h1`/`h2`/`ws` (hidden by default). 13. **Remote IP** (hidden). 14. **Rule** (hidden).
  15. **Model** (hidden) — provider mark (monogram in the provider's tag tint) + model of an
  LLM API call. LLM calls also show a ✦ **LLM** label (accent) in the Type column.
  16. **Header columns** (user-added, up to 8) — the value of one request (↑) or response (↓)
  header, mono, `—` when absent: added from *Columns ▸ Add header column…*, the palette or a
  Headers tab row (*Show as a traffic list column*), removed by unchecking. Values load for the
  rows on screen (`POST /api/exchanges/headers`, batched and cached), so they don't sort; in a
  narrow list they're left out after the minor built-in columns.
* Row states: hover `--bg-hover`; selected `--bg-selected` + 2px inset left accent; selected
  & focused list `--bg-selected-strong`; aborted rows: status in danger, text slightly muted;
  TLS-failure rows: lock-x icon, "TLS handshake failed — client rejected certificate"; tunnel
  rows: "Tunnel" pill + byte counts.
* Selection: click selects; Ctrl/⌘-click toggles; Shift-click range; ↑/↓ or J/K move;
  Shift+↑/↓ extend; Home/End; PageUp/Down; Ctrl/⌘ A selects all visible; Esc clears.
  Enter focuses detail pane. Delete/Backspace deletes selected (undo toast). Ctrl P pin/unpin.
  Ctrl R replay, Ctrl E open in Compose, Ctrl Shift C copy as cURL. Ctrl C copies URL(s).
* **Context menu** (single row): Copy URL · Copy as cURL · Copy as… (submenu of targets) ·
  Copy request/response body · ─ · Replay · Edit in Compose · ─ · Create rule ▸ (Mock this
  response · Breakpoint matching requests · Block this host · Map to local file · Rewrite…) ·
  ─ · Filter ▸ (Same host · Same path · Exclude host · Same source) · Highlight ▸ (colours) ·
  Pin · Add note… · ─ · Export as HAR · Download response body · ─ · Delete (danger).
  Multi-row: Copy URLs · Copy as cURL (all) · Export selected as HAR · Pin all · Highlight ▸ ·
  Compare (when exactly 2) · Delete selected.
* **Live tail**: if scrolled to bottom, new rows keep it pinned to bottom. If not, a floating
  pill "↓ N new" (accent) appears bottom-center; click scrolls down. Selection is never
  stolen by new traffic.
* **Pause view vs pause recording**: Recording toggle is server-side (stop storing).
  Additionally, when the user holds the list scrolled up, rendering of new rows above is stable.
* **Empty state** (no exchanges at all): large but calm "Waiting for traffic" with the proxy
  address card (copy), and a 3×2 grid of top interceptors (Chrome, Firefox, Terminal,
  Android, System proxy, Manual setup) with one-click launch; link "All integrations →".
  Empty because of filter: "No exchanges match `<filter>`" + Clear filter button.

### 6.3.1 Timeline
A second view of the list (toolbar button next to *View options*, `T`, palette, and *Timeline* in
the multi-selection pane; remembered): the visible exchanges — or the selection when several are
selected — as a waterfall on one wall-clock axis. A bar per row with the Timing tab's phases and
colours (DNS, connect, TLS, sent, waiting, download; a striped segment while running), loaded
for the rows on screen (`POST /api/exchanges/timings`, batched and cached) and shown as one plain
bar from the summary until then; a tooltip lists the phase durations. Sticky label column
(method, status, host, path) and tick header; zoom 1×–64× (buttons or Ctrl/⌘ + wheel, anchored on
the pointer); click selects like the list (Ctrl/⌘ toggles). Virtualised; it plots at most the
newest 5,000 and says so above the rows ("Showing the newest 5,000 of 23,456 exchanges. Narrow
the filter or select the exchanges to compare to see the rest.").

### 6.4 Detail pane — single exchange

**Header** (sticky, `--bg-subtle`, padding 10px 14px, bottom border):
* Line 1: MethodBadge · StatusBadge + reason text · URL (mono 12.5px, scheme+host `--text`,
  path `--text`, query `--text-muted`, selectable, wraps to max 3 lines with expand).
* Line 2 (meta chips, `--fs-xs`, muted, separated by ·): duration · response size (+ decoded
  size if compressed) · protocol (HTTP/2) · TLS version · source app icon+name · client
  IP:port · rule chip (accent soft, click → opens rule) · "Modified" chip (click toggles
  Original/Modified view) · colour tag dot · pinned.
* Actions (right, IconButtons): Replay (`Ctrl R`) · Edit in Compose (`Ctrl E`) · Copy as ▾
  (default cURL; menu of all targets; remembers last) · Create rule ▾ · Pin · ⋮ (note,
  highlight, download bodies, export HAR, delete).
* **Paused banner** (when paused): amber soft bg, `CirclePause` icon, "Paused before the
  request is sent · waiting 12s" (live), buttons: **Resume** (primary, `Ctrl Enter`),
  Respond… (opens response editor), Close connection, Reset; editors below become editable.
* **Error banner** (aborted): danger soft bg, icon, plain-language title ("Connection
  refused by api.example.com:443"), explanation & suggested fix, raw error code (mono, muted).

**Body** — two sections, **Request** and **Response**, side-by-side when the pane is wider
than 860px, otherwise stacked; the divider between them is a SplitPane. Each section has a
section header row: label (`REQUEST`/`RESPONSE`, `--fs-2xs` letter-spaced, muted) + tabs:

* Request tabs: **Headers** · **Query** (n) · **Body** (size) · **Cookies** (n) · **Auth** (when
  Authorization/JWT present) · **Raw** · **Code**.
* Response tabs: **Headers** · **Body** (size) · **Cookies** (n Set-Cookie) · **Raw** ·
  **Timing** · **Connection** · **Cache**; WebSocket/SSE exchanges add **Messages** (count)
  as the first response tab and select it by default.
* Recognised LLM API calls add **LLM** (✦) as the first response tab, selected by default (its
  own remembered-tab group), and the response section gets ~3/4 of the height by default.
* Tabs remember the last chosen tab per section across selections.

**Headers tab**: two-column grid (name 34% `--text-muted` semibold 12px, value mono 12px
wraps). Hover row shows actions: copy, filter by header, docs (popover with MDN-style
description of well-known headers, bundled JSON). Toggle "Wire order / Sorted". Pseudo
headers (`:authority`) shown dimmed at top for HTTP/2. Special renderers: `Cookie`/`Set-Cookie`
expand into attribute tables, `Authorization: Bearer <jwt>` shows a "JWT" pill that expands the
decoded header/payload with expiry status, `Content-Type` shows kind icon.

**Body tab**: toolbar (32px): view mode SegmentedControl (contextual: Auto · Pretty · Raw ·
Tree (json) · Preview (html/markdown/image/svg/pdf/audio/video) · Hex · Decoded (protobuf/
grpc/msgpack/cbor) · Parts (multipart) · Events (SSE)) · wrap toggle · search (opens CM search)
· copy · download · info text right ("gzip · 3.1 KB → 12.8 KB", "truncated at 32 MB").
Content area: CodeEditor read-only with folding for text kinds; ImageViewer (checkerboard,
dimensions pill, zoom fit/100%); HexViewer for binary; JsonTree; sandboxed `<iframe sandbox>`
HTML preview; `<audio>/<video>` players via raw URL; protobuf decoded tree rendered as JSON;
form-urlencoded as key/value table; multipart as parts list each with its own sub-viewer.
Protobuf / gRPC bodies decoded with an imported schema get a schema bar above the tree
(`schema: shop.v1.Item` accent pill, source hint — gRPC method, Content-Type or mapping — and a
Typed / Schemaless SegmentedControl remembered across bodies); mismatches show a warning row
and fall back to the schemaless tree, unknown fields an info row; without a schema a neutral
row offers "Add schema" (→ Settings › Protobuf schemas). gRPC exchanges show
`Service / Method` and a coloured `grpc-status` chip in the detail header meta line.
Empty body: subtle "No body". Decode failure: warning row + hex view of raw bytes.

**Query tab**: key/value table of decoded params (copy each), raw query string below.
**Cookies tab**: table (name, value, domain, path, expires, flags as pills HttpOnly/Secure/SameSite).
**Raw tab**: the full HTTP/1.1-style message text (request line/status line, headers in wire
order, blank line, body text or `<N bytes binary>`), CodeEditor with `http` highlighting, copy.
**Code tab**: target Select (grouped) + CodeEditor with the snippet + Copy button.

**LLM tab** (`GET /api/exchanges/:id/llm`, refetched — throttled — while the response streams):
* Hero card (`--bg-subtle`, `--r-lg`, faint accent radial wash): provider mark + name + API label,
  badges (Streamed / Streaming pulse, stop reason in plain words with tone, Error), actions (¶
  Markdown toggle, Copy ▾ Markdown / JSON); the model in mono `--fs-lg`; metric tiles (Input with
  % cached, Output with reasoning, Cache read/write, TTFT in accent, Speed tok/s, Total with chunk
  count); request parameters as mono key/value chips (structured values open a popover).
* Error banner (HTTP status — message, type/code), warnings banner.
* Collapsible System prompt (open when short, 2-line preview otherwise) and Tools (name chips;
  rows with description, JSON Schema on expand).
* "Conversation" divider, then bubbles: user right-aligned (accent-tinted, `--r-lg` with a squared
  top-right corner), assistant left (`--bg-subtle`), tool turns as bare cards, system turns dashed.
  Head row: role avatar, label, `#n`, hover copy. Parts: text (assistant Markdown with chat line
  breaks, others pre-wrap), thinking (collapsed disclosure, muted), tool-call card (orange tool
  icon, mono name, id, highlighted Prettier-style JSON), tool-result card (green icon, tool name
  resolved from the call id, JSON or mono text, red when `isError`), image thumbnails on a
  checkerboard (click to enlarge; remote URLs never loaded), file chips, refusal row. Long texts
  and old turns (> 14 messages: last 10 shown) fold.
* "Response" divider (accent) with tokens · duration · stop reason, then the reply bubble with an
  accent border/glow, "Response" pill and a blinking caret while streaming.

**Timing tab** — waterfall: horizontal bars on a shared ms scale for Blocked/Queued (if any),
DNS, Connect, TLS, Request sent, Waiting (TTFB), Download; each bar with label + ms; total
duration; absolute start time; "connection reused" note; paused time excluded and shown
separately.

**Connection tab** — Client (address, process name/path/pid, interceptor), Client TLS (SNI,
version, cipher, ALPN offered/negotiated, JA3, JA4 — copyable), Upstream (remote address, via
proxy, TLS version/cipher/ALPN, the fingerprint Akev presented — profile badge (accent when
impersonating, warning with an explanation when a requested browser profile fell back to
native), JA4 / JA3 / HTTP/2 (Akamai) sent, copyable —, certificate: subject, issuer, SANs,
validity with expiry status, SHA-256), HTTP version.

**Cache tab** — cacheability analysis (port of HTTP Toolkit `caching.ts`, fixed): a list of
findings each with a status icon, summary line and markdown explanation.

**Messages tab** (WebSocket/SSE) — virtualised list: direction arrow (↑ sent accent, ↓
received success), time (relative `+1.204s` / absolute toggle), size, type (text/binary/
ping/pong/close), preview (first line, mono). Click expands an inline viewer (auto-formatted
JSON / hex). Filter box + direction filter (All/Sent/Received). Follow mode toggle (auto-scroll).
Close frame row shows code & reason. Messages written by Akev carry a tag (*injected* — sent
from this window's composer — or *modified*). Below the list (WebSockets only):
**paused messages** — an amber panel (`--warning-soft`) with one card per message held at a
`ws-breakpoint` rule: direction, kind, live "waiting" timer, payload CodeEditor (binary as
Hex/Base64), Revert, Drop and Forward (`Ctrl Enter`; label "Forward edited" when changed);
Forward all / Drop all when several wait. **Composer** (open WebSockets) — collapsible
"Send a message" bar: To server / To client, Text / JSON / Hex / Base64, a CodeEditor with
Send (`Ctrl Enter`), inline validation, and a collapsible "Sent from Akev" history (click to
reuse).

### 6.5 Multi-selection pane
Title "N exchanges selected", stats (total size, time span, status breakdown bars by class,
top hosts), actions: Export HAR, Copy URLs, Copy as cURL, Pin, Highlight, Delete, **Compare**
(2 selected → side-by-side diff of request line, headers and bodies using a diff view).

### 6.6 Breakpoint editing
When paused at request: Request section editable — method Select + URL Input, KeyValueEditor
for headers, CodeEditor for body (with "Content-Length auto-updates" note), Response section
shows "Waiting for you to resume". When paused at response: Response editable (status Number +
message, headers, body). Resume applies edits; `Ctrl Enter` resumes. A global toast appears
when an exchange pauses while the user is on another page ("Request paused · POST /login",
[Jump to]); the nav Breakpoints badge counts paused items. Desktop app also sends an OS
notification if the window is unfocused.

---

## 7. Rules page (`#/rules`)

Master–detail. Left: rule list (min 360px, resizable). Right: editor for the selected rule.

* **Toolbar**: "New rule" primary split button (click = new rule from the last template;
  ▾ menu = templates with icons & one-line descriptions: Mock response · Map Remote (forward to
  another host) · Map Local (file or folder) · Rewrite request · Rewrite response · Breakpoint ·
  Delay / throttle · Block (close/reset/timeout) · Highlight · Script · Impersonate a browser
  (TLS fingerprint) · WebSocket echo/reject · Pause WebSocket messages) ·
  New group · search rules · Import (file: Akev JSON or HTTP Toolkit `.htkrules`; dialog asks
  "Add as new group / Replace all") · Export · ⋮ (enable all, disable all, reset stats).
* **Rule list**: tree with groups (chevron collapse, title editable inline on double-click,
  enable switch affecting children, rule count). Each rule row (40px): drag handle (on hover),
  enable Switch, auto-generated **sentence summary** with coloured tokens — e.g.
  `GET` `api.example.com/users*` → **Mock 200** `application/json` — title (if set) shown
  instead with summary below in muted text, live **hit counter** pill (`12×`, flashes accent on
  increment), last hit relative time on hover, ⋮ menu (duplicate, move to group ▸, test against
  selected exchange, delete). Disabled rules at 50% opacity. Drag & drop with dnd-kit
  (before/after/inside group drop indicators). Order = priority (top wins; modifiers stack, see
  akev-core rules docs) — explained by a subtle info line at the top of the list.
* **Editor panel**: Title input (placeholder = auto summary) · **When** section: list of matcher
  rows; each row: type label (fixed width), operator Select (equals/contains/starts with/ends
  with/glob/regex/exists, depending on type), value input(s), case-sensitivity toggle (Aa),
  negate toggle (≠), remove ×. "+ Add condition" dropdown with all matcher types (grouped:
  Request line, Headers & cookies, Body, Client, Logic). Empty matchers = "Matches every
  request" note. · **Then** section: action Select with icon cards; action-specific form:
  - Mock: status (Number + reason preview), headers KeyValueEditor, body (source: text/JSON/
    file path with picker in desktop) CodeEditor with language picker.
  - Forward: target URL, strip prefix, preserve host Switch, optional transforms.
  - Map Local: path input, strip prefix, index file, fallback Switch.
  - Rewrite request/response (`transform`): sub-sections for method/status, URL find/replace
    list, query set/remove, headers set/add/remove (KeyValueEditors), body transform (replace /
    find & replace list / JSON merge patch editor / JSON patch editor).
  - Breakpoint: checkboxes request/response. Delay: ms + jitter. Throttle: preset Select (3G,
    4G, DSL, Wi-Fi, Custom) + down/up kbps + latency. Highlight: colour picker + note.
  - Script: CodeEditor (JS) prefilled with documented `onRequest/onResponse` template + "Test
    with selected exchange" button showing result/console output.
  · **Options**: Match limit (Always / Once / N times) · Note.
* Edits apply live (debounced 400ms) when valid; invalid fields show inline errors and are
  not applied; a small "Saved ✓ / Unsaved: fix errors" indicator in the editor header. Full
  **undo/redo** (Ctrl Z / Ctrl Shift Z) across rule edits, deletes, moves.
* Creating a rule from the traffic context menu opens Rules with the new rule selected and the
  matcher prefilled from the exchange (method + host + path, query optional).

---

## 8. Compose (`#/compose`), Connect (`#/connect`), Settings (`#/settings`)

### 8.1 Compose
* Left sidebar (240px, collapsible): **History** (most recent first: method badge, path, host,
  status, relative time; search; click re-opens in a tab) and **Saved** (named requests,
  grouped in folders; drag to reorder). Saved data persisted via `PATCH /api/settings/ui`.
* Request tabs across the top (like browser tabs, closable, middle-click close, `Ctrl T` new,
  `Ctrl W` close, dirty dot). Each tab: request line row — Method Select (editable combobox,
  coloured) + URL input (mono, `{{var}}` highlighting) + **Send** primary button (`Ctrl Enter`,
  shows spinner + Cancel while in flight) + options ▾ (Apply rules · Follow redirects · Ignore
  TLS errors · HTTP/1.1 only · Timeout). Pasting a cURL command into the URL field converts it
  (`/api/import/curl`) with a toast.
* **Repeat…** (⟳ button next to Send, palette; also *Repeat…* in the traffic row menu and the
  palette for a selected exchange, which resends it as captured): a dialog with Times (≤ 10,000),
  At once (concurrency ≤ 64) and Pause between each worker's requests, then live progress, status
  chips (`200 ×9`, `503 ×1`, failures by kind) and Min / Average / p50 / p95 / Max latency; Stop
  cancels (so does closing), *Open the slowest* jumps to that exchange. Runs on the daemon
  (`POST /api/send/repeat`, NDJSON progress), so concurrency isn't limited by the browser's
  connections per host.
* Request sub-tabs: **Params** (KeyValueEditor synced both ways with URL query) · **Headers**
  (KeyValueEditor with header-name autocomplete & enable checkboxes) · **Body** (mode
  SegmentedControl: None · JSON · Text · Form · Multipart · Binary (file) ; CodeEditor or
  KeyValueEditor accordingly; auto Content-Type) · **Auth** (None · Basic · Bearer → generates
  header) · **Code** (snippet).
* Response area (SplitPane below or right): status/time/size summary line + "Open in Traffic"
  link, then the same Headers/Body/Raw/Timing tabs as the Traffic detail pane (reuse components).
* Environment variables: a simple key/value environment selector in the toolbar
  (`{{baseUrl}}` substitution), managed in a dialog.

### 8.2 Connect
* Header card: "Connect a client" title + subtitle; **Proxy card** showing `127.0.0.1:8000`
  (copy), LAN addresses (when bind allows), "Allow other devices" Switch (sets bind 0.0.0.0),
  **CA card**: fingerprint, expiry, Download (PEM/DER), "Device setup page" QR code
  (`http://<lan-ip>:<port>/` served by the proxy) for phones.
* **Active** strip (when any interceptor active): chips with icon + name + Stop.
* Search (fuzzy) + category chips (All · Browsers · Terminal · Runtimes · Mobile · Containers ·
  System · Manual).
* Responsive grid of cards (min 240px): app icon (40px, brand colour), name, one-line
  description, status line (✓ Available / version · ⚠ Not installed · ● Active), primary
  button (Launch / Activate / Set up). Unavailable cards are dimmed with reason tooltip.
  Cards with `needsConfig` open a right **Sheet**: Android (device list, steps, push CA,
  status), Docker (container list with attach buttons), Existing terminal (shell tabs: bash ·
  zsh · fish · PowerShell; one-liner with copy; reset command), Manual setup (per-tool
  instructions: HTTP_PROXY env, Java, Node, Python, Go, Rust, iOS device steps, Android
  manual steps), JVM (running JVMs with PID · Java version · main class, Attach / Restore per
  row, *Other session* badge + *Attach here* for JVMs a previous Akev attached, “Include
  localhost requests” checkbox, agent limitations under the attached row, coverage notes).

### 8.3 Settings
Left sub-nav (200px) + content (max 760px, sections as cards):
**General** (Theme cards with mini previews: System/Dark/Light/Midnight/High contrast ·
Density: Compact/Default/Comfortable · UI scale · Monospace paths toggle · Start page) ·
**Proxy** (port + "currently listening on" status — with the reason and a *Use port N* / *Retry*
button when a busy port or a CLI override keeps the proxy elsewhere — bind address / Allow LAN, HTTP/2, SOCKS,
TLS passthrough hosts (string list editor), upstream TLS fingerprint (profile Select with the
expected JA4 / HTTP/2 fingerprint, ordered per-host overrides, a warning callout in builds
without the impersonation backend), upstream proxy (mode SegmentedControl + url +
no-proxy list), DNS overrides table, DoH URL, timeouts) · **Certificates** (CA info + download
+ regenerate (danger, confirm) + trust instructions per OS/browser, client certificates table,
trusted CA paths, ignore-upstream-cert-error hosts) · **Capture** (max body size, memory
budget, max exchanges, ignored hosts, record tunnels, record TLS failures, detect processes) ·
**Protobuf schemas** (drop zone for `.proto` files / folders / descriptor sets, file list with
expandable services → methods, messages, enums and compile errors with line:column, source
viewer dialog, remove with undo; URL mappings table: on/off, pattern with live match count,
request/response/both, message type combobox) ·
**Shortcuts** (every shortcut, searchable and grouped per action; click a key and press new
keys to rebind it — keys already used where both can be active are flagged with *Use anyway* /
*Cancel*; changed keys are tinted with a reset button; *Reset all* with undo; keys the lists handle
themselves are marked built in; stored in the UI prefs, so tooltips, menus and the palette show the
new keys) · **Data** (sessions & autosave: autosave switch,
restore on startup, interval, size limit, manage sessions; export/import settings & rules,
clear all data) · **License** (plan, licensee, updates-until, enter/remove key, start trial,
Pro feature list) · **About** (version, build, plan, licensee, links, open-source licenses).
Every change applies immediately (PUT /api/settings) with inline validation; fields that need
a proxy rebind show "Applied — proxy moved to :8001" toast.

---

## 9. Frontend architecture

The folder layout is in [`ui/README.md` § Folder layout](../ui/README.md#folder-layout).

* **TrafficStore**: holds `summaries: Map<number, ExchangeSummary>`, `order: number[]`
  (ascending id), `visible: number[]` (after filter + type chip, from the daemon's
  `filter-result` + incremental `filtered` deltas), `version`. Stream messages mutate in place
  and schedule one notification per animation frame. Components subscribe with selectors
  (`useTraffic(s => s.visible)`), rows read summaries by id.
* Filtering: the UI sends `{type:"set-filter", query}` over the stream (debounced 120ms);
  daemon replies `filter-result` and keeps the UI's visible set updated (see the HTTP API reference, § Live stream).
  Type chip is translated into an extra filter clause appended server-side
  (`type:json,xml,...`).
* Selected exchange detail: fetched on selection (`GET /api/exchanges/:id`), refetched when its
  summary version changes (state/updated), bodies fetched lazily per tab (cache by
  `id+side+original` with LRU of 50).
* Desktop integration: if `window.__TAURI__` is present use Tauri APIs for native file
  dialogs (save HAR / open file), notifications, and opening external links; otherwise use
  browser downloads/file inputs.
* Accessibility: all interactive elements reachable by keyboard; list is `role="grid"` with
  `aria-rowcount`/`aria-selected`; dialogs trap focus; colour is never the only signal
  (icons/labels accompany statuses).
* Performance budgets: initial JS < 400 KB gzip (CodeMirror languages lazy-loaded per kind);
  first paint < 300ms from daemon; 60fps scrolling at 100k rows; ingest 2,000 exchanges/s
  without jank.
