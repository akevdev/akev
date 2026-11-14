/**
 * `#/_kitchen-sink` — every primitive from components/ui in every state, for design reviews
 * and visual regression screenshots (see ui/README.md → "Screenshots").
 *
 * Keep it up to date when adding or changing a primitive: it is the living style guide.
 */

import clsx from 'clsx';
import {
  ArrowLeftRight,
  Bell,
  ChevronDown,
  Circle,
  Code,
  Copy,
  Download,
  FileJson,
  Filter,
  Inbox,
  LayoutGrid,
  List,
  MoreHorizontal,
  Pin,
  Play,
  Plus,
  RefreshCw,
  Search,
  Send,
  Settings,
  ShieldAlert,
  Star,
  Trash2,
  Upload,
  Wifi,
} from 'lucide-react';
import { useEffect, useMemo, useState, type ReactNode } from 'react';
import type { TagColor } from '../../api/types';
import { TAG_COLORS } from '../../api/types';
import {
  APP_ICON_NAMES,
  AppIcon,
  Badge,
  Button,
  Checkbox,
  CodeEditor,
  ColorDot,
  Combobox,
  confirm,
  ContextMenu,
  CopyButton,
  Dialog,
  EmptyState,
  Field,
  HexViewer,
  IconButton,
  Input,
  JsonTree,
  Kbd,
  KeyValueEditor,
  makeRow,
  Menu,
  MethodBadge,
  NumberInput,
  Popover,
  PopoverContent,
  PopoverTrigger,
  ProgressBar,
  QrCode,
  SearchInput,
  Section,
  SegmentedControl,
  Select,
  Sheet,
  Skeleton,
  Slider,
  Spinner,
  SplitPane,
  StatusBadge,
  Switch,
  TabList,
  Tabs,
  TabsContent,
  TextArea,
  toast,
  Toolbar,
  ToolbarSeparator,
  Tooltip,
  undoToast,
  type KeyValueRow,
  type MenuItems,
} from '../../components/ui';
import { ErrorBoundary } from '../../app/ErrorBoundary';
import { useRoute } from '../../app/router';
import { utf8Encode } from '../../lib/encoding';
import { KNOWN_HEADER_NAMES } from '../../lib/headerDocs';
import { HTTP_METHODS } from '../../lib/http';
import { THEMES, useUiStore, type Density, type ThemePref } from '../../state/ui';
import { DENSITIES } from '../../state/ui';
import { smoothScroll } from '../../lib/motion';
import styles from './KitchenSinkPage.module.css';

// =====================================================================================
// Layout helpers
// =====================================================================================

const SECTIONS = [
  ['tokens', 'Colour tokens'],
  ['type', 'Typography'],
  ['buttons', 'Buttons'],
  ['inputs', 'Inputs'],
  ['toggles', 'Toggles'],
  ['tabs', 'Tabs'],
  ['overlays', 'Overlays'],
  ['badges', 'Badges & labels'],
  ['icons', 'App icons'],
  ['data', 'Data editors'],
  ['viewers', 'Viewers'],
  ['feedback', 'Feedback'],
  ['layout', 'Layout'],
] as const;

type SectionId = (typeof SECTIONS)[number][0];

function Group({ id, title, description, children }: { id: SectionId; title: string; description?: string; children: ReactNode }) {
  return (
    <section id={`ks-${id}`} className={styles.group} aria-labelledby={`ks-${id}-title`}>
      <header className={styles.groupHeader}>
        <h2 id={`ks-${id}-title`} className={styles.groupTitle}>
          {title}
        </h2>
        {description && <p className={styles.groupDesc}>{description}</p>}
      </header>
      <div className={styles.grid}>{children}</div>
    </section>
  );
}

function Specimen({ title, children, wide, full, flush }: { title: string; children: ReactNode; wide?: boolean; full?: boolean; flush?: boolean }) {
  return (
    <div className={clsx(styles.specimen, wide && styles.wide, full && styles.full)}>
      <div className={styles.specimenTitle}>{title}</div>
      <div className={clsx(styles.specimenBody, flush && styles.flush)}>{children}</div>
    </div>
  );
}

function Row({ children, label }: { children: ReactNode; label?: string }) {
  return (
    <div className={styles.row}>
      {label && <span className={styles.rowLabel}>{label}</span>}
      <div className={styles.rowItems}>{children}</div>
    </div>
  );
}

// =====================================================================================
// Sample data
// =====================================================================================

const SAMPLE_JSON = {
  id: 1042,
  login: 'ada',
  name: 'Ada Lovelace',
  admin: false,
  score: 98.6,
  manager: null,
  tags: ['analyst', 'engine', 'poet'],
  'odd key': 'quoted path',
  profile: {
    company: 'Analytical Engines Ltd',
    location: 'London',
    repos: [
      { name: 'akev', stars: 1820, private: false },
      { name: 'notes', stars: 3, private: true },
    ],
  },
  createdAt: '1843-07-10T09:30:00Z',
};

const SAMPLE_HTTP = `POST /api/v2/orders?expand=items HTTP/1.1
Host: api.example.com
Content-Type: application/json
Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhZGEifQ.sig
:authority: api.example.com
Accept: */*

{"items":[{"sku":"A-1","qty":2}]}`;

const SAMPLE_SHELL = `curl -X POST 'https://api.example.com/v2/orders' \\
  -H 'Content-Type: application/json' \\
  -H "Authorization: Bearer $TOKEN" \\
  --data-raw '{"items":[{"sku":"A-1","qty":2}]}'  # create an order`;

function sampleBytes(): Uint8Array {
  const magic = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52];
  const head = new Uint8Array([...magic, ...utf8Encode(' Akev hex viewer sample — ')]);
  const out = new Uint8Array(4096);
  out.set(head);
  let x = 0x2545f491;
  for (let i = head.length; i < out.length; i++) {
    x ^= x << 13;
    x ^= x >>> 17;
    x ^= x << 5;
    out[i] = i % 97 < 12 ? 0 : x & 0xff;
  }
  return out;
}

const MENU_ITEMS = (pinned: boolean, setPinned: (v: boolean) => void, density: string, setDensity: (v: string) => void): MenuItems => [
  { label: 'Copy URL', icon: Copy, shortcut: 'mod+c', onSelect: () => toast.success('URL copied') },
  { label: 'Copy as cURL', icon: Code, shortcut: 'mod+shift+c' },
  {
    type: 'submenu',
    label: 'Copy as…',
    icon: FileJson,
    items: [{ label: 'Python · requests' }, { label: 'JavaScript · fetch' }, { label: 'Go · net/http' }, { label: 'Rust · reqwest' }],
  },
  { type: 'separator' },
  { label: 'Replay', icon: Play, shortcut: 'mod+r' },
  { label: 'Edit in Compose', icon: Send, shortcut: 'mod+e' },
  { label: 'Export as HAR', icon: Download, disabled: true, hint: 'Pro? Never.' },
  { type: 'separator' },
  { type: 'checkbox', label: 'Pinned', icon: Pin, checked: pinned, onCheckedChange: setPinned, shortcut: 'mod+p' },
  { type: 'label', label: 'Density' },
  { type: 'radio', value: density, onValueChange: setDensity, options: DENSITIES.map((d) => ({ value: d.id, label: d.label })) },
  { type: 'separator' },
  { label: 'Delete', icon: Trash2, destructive: true, shortcut: 'delete' },
];

// =====================================================================================
// Sections
// =====================================================================================

const SWATCHES: { title: string; tokens: string[] }[] = [
  { title: 'Surfaces', tokens: ['bg-app', 'bg-panel', 'bg-subtle', 'bg-elevated', 'bg-input', 'bg-hover', 'bg-active', 'bg-selected', 'bg-selected-strong'] },
  { title: 'Text', tokens: ['text', 'text-muted', 'text-subtle', 'text-disabled', 'accent-text'] },
  { title: 'Accent & semantic', tokens: ['accent', 'accent-hover', 'accent-soft', 'success', 'warning', 'danger', 'info'] },
  { title: 'Methods', tokens: ['m-get', 'm-post', 'm-put', 'm-patch', 'm-delete', 'm-head', 'm-options', 'm-connect', 'm-other'] },
  { title: 'Status classes', tokens: ['s-1xx', 's-2xx', 's-3xx', 's-4xx', 's-5xx', 's-err'] },
  { title: 'Content kinds', tokens: ['k-json', 'k-html', 'k-js', 'k-css', 'k-image', 'k-media', 'k-font', 'k-xml', 'k-text', 'k-binary', 'k-ws', 'k-grpc', 'k-form'] },
  { title: 'Syntax', tokens: ['syn-key', 'syn-string', 'syn-number', 'syn-bool', 'syn-keyword', 'syn-tag', 'syn-fn', 'syn-type', 'syn-comment', 'syn-punct'] },
];

function TokensSection() {
  return (
    <Group id="tokens" title="Colour tokens" description="Identical names in every theme; components never use raw colours.">
      {SWATCHES.map((s) => (
        <Specimen key={s.title} title={s.title} wide={s.tokens.length > 8}>
          <div className={styles.swatches}>
            {s.tokens.map((t) => (
              <div key={t} className={styles.swatch}>
                <span className={styles.swatchChip} style={{ background: `var(--${t})` }} />
                <code className={styles.swatchName}>--{t}</code>
              </div>
            ))}
          </div>
        </Specimen>
      ))}
      <Specimen title="Text on surfaces" wide>
        <div className={styles.surfaces}>
          {['bg-app', 'bg-panel', 'bg-subtle', 'bg-elevated'].map((bg) => (
            <div key={bg} className={styles.surface} style={{ background: `var(--${bg})` }}>
              <code className={styles.surfaceName}>--{bg}</code>
              <span style={{ color: 'var(--text)' }}>Text — primary</span>
              <span style={{ color: 'var(--text-muted)' }}>Muted — secondary</span>
              <span style={{ color: 'var(--text-subtle)' }}>Subtle — tertiary</span>
              <span style={{ color: 'var(--accent-text)' }}>Accent text</span>
            </div>
          ))}
        </div>
      </Specimen>
    </Group>
  );
}

function TypeSection() {
  const sizes = ['2xl', 'xl', 'lg', 'md', 'base', 'sm', 'xs', '2xs'];
  return (
    <Group id="type" title="Typography" description="Inter Variable for UI, JetBrains Mono Variable for data. Tabular numerals via .tnum.">
      <Specimen title="Scale (--fs-*)" wide>
        <div className={styles.typeScale}>
          {sizes.map((s) => (
            <div key={s} className={styles.typeRow}>
              <code className={styles.typeToken}>--fs-{s}</code>
              <span style={{ fontSize: `var(--fs-${s})` }}>Intercept, inspect &amp; mock HTTP</span>
            </div>
          ))}
        </div>
      </Specimen>
      <Specimen title="Weights & families">
        <div className={styles.stack}>
          <span style={{ fontWeight: 'var(--fw-regular)' }}>Regular 450 — The quick brown fox</span>
          <span style={{ fontWeight: 'var(--fw-medium)' }}>Medium 530 — The quick brown fox</span>
          <span style={{ fontWeight: 'var(--fw-semibold)' }}>Semibold 620 — The quick brown fox</span>
          <span className="mono">Mono — GET /api/v1/users?page=2 → 200</span>
        </div>
      </Specimen>
      <Specimen title="Tabular numerals">
        <div className={styles.numbers}>
          <span>Proportional</span>
          <span className="tnum">Tabular</span>
          {['1,111.11 KB', '88.08 ms', '10,204'].map((n) => (
            <FragmentPair key={n} value={n} />
          ))}
        </div>
      </Specimen>
    </Group>
  );
}

function FragmentPair({ value }: { value: string }) {
  return (
    <>
      <span>{value}</span>
      <span className="tnum">{value}</span>
    </>
  );
}

function ButtonsSection() {
  const [loading, setLoading] = useState(false);
  const [active, setActive] = useState(true);
  return (
    <Group id="buttons" title="Buttons" description="Sizes sm 24 / md 28 / lg 34. Hover, focus-visible, active and disabled states on every variant.">
      <Specimen title="Variants" wide>
        <Row label="md">
          <Button variant="primary" icon={Plus} kbd="mod+t">
            New request
          </Button>
          <Button icon={Upload}>Import</Button>
          <Button variant="ghost" icon={RefreshCw}>
            Refresh
          </Button>
          <Button variant="danger" icon={Trash2}>
            Delete
          </Button>
          <Button variant="subtle-danger">Reset</Button>
        </Row>
        <Row label="disabled">
          <Button variant="primary" disabled>
            Primary
          </Button>
          <Button disabled>Secondary</Button>
          <Button variant="ghost" disabled>
            Ghost
          </Button>
          <Button variant="danger" disabled>
            Danger
          </Button>
          <Button variant="subtle-danger" disabled>
            Subtle danger
          </Button>
        </Row>
      </Specimen>
      <Specimen title="Sizes">
        <Row label="sm">
          <Button size="sm" variant="primary">
            Send
          </Button>
          <Button size="sm" icon={Filter}>
            Filter
          </Button>
          <Button size="sm" iconRight={ChevronDown}>
            Export
          </Button>
        </Row>
        <Row label="md">
          <Button variant="primary">Send</Button>
          <Button icon={Filter}>Filter</Button>
          <Button iconRight={ChevronDown}>Export</Button>
        </Row>
        <Row label="lg">
          <Button size="lg" variant="primary" icon={Send}>
            Send
          </Button>
          <Button size="lg" icon={Filter}>
            Filter
          </Button>
          <Button size="lg" iconRight={ChevronDown}>
            Export
          </Button>
        </Row>
      </Specimen>
      <Specimen title="Loading & block">
        <Row>
          <Button
            variant="primary"
            loading={loading}
            kbd="mod+enter"
            onClick={() => {
              setLoading(true);
              setTimeout(() => setLoading(false), 1500);
            }}
          >
            Send request
          </Button>
          <Button loading>Saving</Button>
        </Row>
        <Button block icon={Download}>
          Download CA certificate
        </Button>
      </Specimen>
      <Specimen title="IconButton">
        <Row label="ghost">
          <IconButton label="Search" icon={Search} shortcut="mod+f" />
          <IconButton label="Settings" icon={Settings} />
          <IconButton label="Pin" icon={Pin} active={active} onClick={() => setActive((a) => !a)} />
          <IconButton label="More" icon={MoreHorizontal} />
          <IconButton label="Disabled" icon={Star} disabled />
          <IconButton label="Loading" icon={Star} loading />
        </Row>
        <Row label="subtle">
          <IconButton label="Search" icon={Search} variant="subtle" size="sm" />
          <IconButton label="Search" icon={Search} variant="subtle" />
          <IconButton label="Search" icon={Search} variant="subtle" size="lg" />
        </Row>
        <Row label="danger">
          <IconButton label="Delete" icon={Trash2} variant="danger" />
          <CopyButton value="https://api.example.com/v1/users" label="Copy URL" size="md" toastMessage="URL copied" />
        </Row>
      </Specimen>
    </Group>
  );
}

const SELECT_OPTIONS = [
  {
    label: 'Shell',
    options: [
      { value: 'curl', label: 'cURL', icon: Code },
      { value: 'httpie', label: 'HTTPie', icon: Code },
      { value: 'powershell', label: 'PowerShell', icon: Code, disabled: true },
    ],
  },
  {
    label: 'JavaScript',
    options: [
      { value: 'js-fetch', label: 'fetch', description: 'Browser & Node 18+' },
      { value: 'node-axios', label: 'axios' },
    ],
  },
];

function InputsSection() {
  const [text, setText] = useState('api.github.com');
  const [search, setSearch] = useState('');
  const [num, setNum] = useState<number | null>(1500);
  const [area, setArea] = useState('{\n  "name": "Akev"\n}');
  const [target, setTarget] = useState<string>('curl');
  const [method, setMethod] = useState<string>('POST');
  const [header, setHeader] = useState('Content-');
  return (
    <Group id="inputs" title="Inputs" description="Inputs share one 28px frame with a soft focus ring; mono for URLs and values.">
      <Specimen title="Input">
        <div className={styles.stack}>
          <Input value={text} onChange={(e) => setText(e.target.value)} placeholder="Hostname" aria-label="Hostname" />
          <Input placeholder="Placeholder text" aria-label="Placeholder" />
          <Input mono value="https://api.example.com/v1/users?page=2" readOnly aria-label="URL" />
          <Input value="Invalid value" invalid onChange={() => undefined} aria-label="Invalid" />
          <Input value="Disabled" disabled aria-label="Disabled" />
          <Input size="sm" placeholder="Small (24px)" aria-label="Small" />
          <Input size="lg" placeholder="Large (34px)" aria-label="Large" />
        </div>
      </Specimen>
      <Specimen title="SearchInput · NumberInput · TextArea">
        <div className={styles.stack}>
          <SearchInput value={search} onValueChange={setSearch} placeholder="Filter: method:post status:>=400" shortcut="mod+f" mono aria-label="Filter" />
          <SearchInput value="example.com" onValueChange={() => undefined} size="sm" aria-label="Search" />
          <Row>
            <NumberInput value={num} onValueChange={setNum} unit="ms" min={0} max={60000} step={100} stepper aria-label="Delay" className={styles.w160} />
            <NumberInput value={8000} onValueChange={() => undefined} aria-label="Port" className={styles.w120} />
            <NumberInput value={null} onValueChange={() => undefined} allowEmpty unit="KB" placeholder="∞" aria-label="Limit" className={styles.w120} />
          </Row>
          <TextArea mono autoResize minRows={3} value={area} onChange={(e) => setArea(e.target.value)} aria-label="Body" />
        </div>
      </Specimen>
      <Specimen title="Select">
        <div className={styles.stack}>
          <Select value={target} onValueChange={setTarget} options={SELECT_OPTIONS} aria-label="Snippet target" />
          <Select
            value={method}
            onValueChange={setMethod}
            mono
            options={HTTP_METHODS.map((m) => ({ value: m, label: <MethodBadge method={m} />, textValue: m }))}
            aria-label="Method"
          />
          <Select value={undefined} onValueChange={() => undefined} options={[{ value: 'a', label: 'A' }]} placeholder="Placeholder…" aria-label="Empty" />
          <Row>
            <Select value="x" onValueChange={() => undefined} options={[{ value: 'x', label: 'Invalid' }]} invalid aria-label="Invalid" />
            <Select value="x" onValueChange={() => undefined} options={[{ value: 'x', label: 'Disabled' }]} disabled aria-label="Disabled" />
            <Select value="x" onValueChange={() => undefined} options={[{ value: 'x', label: 'Ghost' }]} variant="ghost" size="sm" aria-label="Ghost" />
          </Row>
        </div>
      </Specimen>
      <Specimen title="Combobox (header names)">
        <div className={styles.stack}>
          <Combobox value={header} onValueChange={setHeader} options={KNOWN_HEADER_NAMES.map((n) => ({ value: n }))} mono placeholder="Header name" aria-label="Header name" />
          <p className={styles.note}>↑/↓ to move, Enter/Tab to accept, Esc to close. Free text is always allowed.</p>
        </div>
      </Specimen>
    </Group>
  );
}

function TogglesSection() {
  const [sw, setSw] = useState(true);
  const [sw2, setSw2] = useState(false);
  const [cb, setCb] = useState(true);
  const [view, setView] = useState('auto');
  const [layout, setLayout] = useState('list');
  const [chip, setChip] = useState('all');
  const [slider, setSlider] = useState(40);
  return (
    <Group id="toggles" title="Toggles" description="Switch, Checkbox (with indeterminate), SegmentedControl with a sliding thumb, Slider.">
      <Specimen title="Switch">
        <div className={styles.stack}>
          <Row>
            <Switch checked={sw} onCheckedChange={setSw} aria-label="On" />
            <Switch checked={sw2} onCheckedChange={setSw2} aria-label="Off" />
            <Switch checked size="sm" onCheckedChange={() => undefined} aria-label="Small" />
            <Switch checked disabled onCheckedChange={() => undefined} aria-label="Disabled on" />
            <Switch checked={false} disabled onCheckedChange={() => undefined} aria-label="Disabled off" />
          </Row>
          <Switch checked={sw} onCheckedChange={setSw} label="Allow other devices" description="Bind the proxy to 0.0.0.0 so phones on your network can connect." />
        </div>
      </Specimen>
      <Specimen title="Checkbox">
        <div className={styles.stack}>
          <Row>
            <Checkbox checked={cb} onCheckedChange={setCb} aria-label="Checked" />
            <Checkbox checked={false} onCheckedChange={() => undefined} aria-label="Unchecked" />
            <Checkbox checked="indeterminate" onCheckedChange={() => undefined} aria-label="Indeterminate" />
            <Checkbox checked disabled onCheckedChange={() => undefined} aria-label="Disabled" />
          </Row>
          <Checkbox checked={cb} onCheckedChange={setCb} label="Record TLS failures" description="Show rows for clients that reject the Akev certificate." />
        </div>
      </Specimen>
      <Specimen title="SegmentedControl" wide>
        <div className={styles.stack}>
          <SegmentedControl
            value={chip}
            onValueChange={setChip}
            aria-label="Type"
            items={[
              { value: 'all', label: 'All', count: 1234 },
              { value: 'fetch', label: 'Fetch/XHR', count: 412 },
              { value: 'doc', label: 'Doc' },
              { value: 'js', label: 'JS' },
              { value: 'css', label: 'CSS' },
              { value: 'img', label: 'Img' },
              { value: 'ws', label: 'WS', count: 3 },
              { value: 'other', label: 'Other', disabled: true },
            ]}
          />
          <Row>
            <SegmentedControl
              size="sm"
              value={view}
              onValueChange={setView}
              aria-label="View mode"
              items={[
                { value: 'auto', label: 'Auto' },
                { value: 'pretty', label: 'Pretty' },
                { value: 'raw', label: 'Raw' },
                { value: 'tree', label: 'Tree' },
                { value: 'hex', label: 'Hex' },
              ]}
            />
            <SegmentedControl
              value={layout}
              onValueChange={setLayout}
              aria-label="Layout"
              items={[
                { value: 'list', icon: List, tooltip: 'List' },
                { value: 'grid', icon: LayoutGrid, tooltip: 'Grid' },
              ]}
            />
          </Row>
        </div>
      </Specimen>
      <Specimen title="Slider">
        <div className={styles.stack}>
          <Slider value={slider} onValueChange={setSlider} max={100} formatValue={(v) => `${v}%`} aria-label="Throttle" />
          <Slider value={70} onValueChange={() => undefined} disabled formatValue={(v) => `${v} ms`} aria-label="Disabled" />
        </div>
      </Specimen>
    </Group>
  );
}

function TabsSection() {
  const [tab, setTab] = useState('headers');
  const [tab2, setTab2] = useState('messages');
  return (
    <Group id="tabs" title="Tabs" description="Underline tabs with counts and badges; overflow scrolls with fading edges.">
      <Specimen title="Request tabs" wide flush>
        <Tabs value={tab} onValueChange={setTab}>
          <TabList
            aria-label="Request"
            items={[
              { value: 'headers', label: 'Headers', count: 14 },
              { value: 'query', label: 'Query', count: 0 },
              { value: 'body', label: 'Body', badge: '2.4 KB' },
              { value: 'cookies', label: 'Cookies', count: 3 },
              { value: 'auth', label: 'Auth' },
              { value: 'raw', label: 'Raw' },
              { value: 'code', label: 'Code', disabled: true },
            ]}
            trailing={<IconButton label="Copy" icon={Copy} size="sm" />}
          />
          <TabsContent value="headers" className={styles.tabPanel}>
            Headers panel
          </TabsContent>
          <TabsContent value="query" className={styles.tabPanel}>
            Query panel
          </TabsContent>
          <TabsContent value="body" className={styles.tabPanel}>
            Body panel
          </TabsContent>
          <TabsContent value="cookies" className={styles.tabPanel}>
            Cookies panel
          </TabsContent>
          <TabsContent value="auth" className={styles.tabPanel}>
            Auth panel
          </TabsContent>
          <TabsContent value="raw" className={styles.tabPanel}>
            Raw panel
          </TabsContent>
        </Tabs>
      </Specimen>
      <Specimen title="Small, overflowing" flush>
        <div className={styles.narrow}>
          <Tabs value={tab2} onValueChange={setTab2}>
            <TabList
              size="sm"
              aria-label="Response"
              items={['messages', 'headers', 'body', 'cookies', 'raw', 'timing', 'connection', 'cache'].map((v) => ({
                value: v,
                label: v[0]?.toUpperCase() + v.slice(1),
                count: v === 'messages' ? 128 : undefined,
              }))}
            />
          </Tabs>
        </div>
      </Specimen>
    </Group>
  );
}

function OverlaysSection() {
  const [dialog, setDialog] = useState(false);
  const [sheet, setSheet] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [density, setDensity] = useState('default');
  const items = MENU_ITEMS(pinned, setPinned, density, setDensity);
  return (
    <Group id="overlays" title="Overlays" description="Radix-based: focus-trapped, Escape-dismissable, collision-aware.">
      <Specimen title="Tooltip · Popover">
        <Row>
          <Tooltip content="Toggle recording" shortcut="mod+shift+r">
            <Button icon={Circle}>Hover me</Button>
          </Tooltip>
          <Tooltip content="A longer tooltip that wraps onto multiple lines when the multiline option is set." multiline side="bottom">
            <Button variant="ghost">Multiline</Button>
          </Tooltip>
          <Popover>
            <PopoverTrigger asChild>
              <Button iconRight={ChevronDown}>Popover</Button>
            </PopoverTrigger>
            <PopoverContent width={280} align="start">
              <div className={styles.stack}>
                <strong className={styles.popTitle}>Filter help</strong>
                <p className={styles.note}>
                  Combine fields like <code>method:post</code> and <code>status:&gt;=400</code>. Negate with <code>-</code>.
                </p>
                <Button size="sm" variant="primary">
                  Got it
                </Button>
              </div>
            </PopoverContent>
          </Popover>
        </Row>
      </Specimen>
      <Specimen title="DropdownMenu · ContextMenu">
        <Row>
          <Menu trigger={<Button iconRight={ChevronDown}>Actions</Button>} items={items} />
          <Menu trigger={<IconButton label="More" icon={MoreHorizontal} tooltip={false} />} items={items} align="end" />
        </Row>
        <ContextMenu items={() => items}>
          {/* biome-ignore lint/a11y/noNoninteractiveTabindex: demo: a focusable area for the context menu */}
          <div className={styles.contextArea} tabIndex={0}>
            Right-click (or press the context-menu key) here
          </div>
        </ContextMenu>
      </Specimen>
      <Specimen title="Dialog · Sheet · confirm()">
        <Row>
          <Button onClick={() => setDialog(true)}>Open dialog</Button>
          <Button onClick={() => setSheet(true)}>Open sheet</Button>
          <Button
            variant="subtle-danger"
            onClick={async () => {
              const ok = await confirm({ title: 'Clear all traffic?', message: '3 pinned exchanges will be deleted too.', destructive: true, confirmLabel: 'Clear all' });
              toast(ok ? 'Cleared' : 'Cancelled');
            }}
          >
            Confirm…
          </Button>
        </Row>
        <Dialog
          open={dialog}
          onOpenChange={setDialog}
          title="Import rules"
          description="Akev JSON or an HTTP Toolkit export. Nothing is applied until you confirm."
          footer={
            <>
              <Button variant="ghost" onClick={() => setDialog(false)}>
                Cancel
              </Button>
              <Button variant="primary" onClick={() => setDialog(false)}>
                Add as new group
              </Button>
            </>
          }
        >
          <div className={styles.stack}>
            <Field label="Group name" hint="Shown in the rules list.">
              <Input defaultValue="Imported from HTTP Toolkit" />
            </Field>
            <Switch checked onCheckedChange={() => undefined} label="Enable imported rules" />
          </div>
        </Dialog>
        <Sheet
          open={sheet}
          onOpenChange={setSheet}
          title="Android device"
          description="Intercept an Android device or emulator over ADB."
          footer={
            <>
              <Button variant="ghost" onClick={() => setSheet(false)}>
                Close
              </Button>
              <Button variant="primary" icon={Play}>
                Intercept
              </Button>
            </>
          }
        >
          <EmptyState icon={Wifi} title="No devices found" description="Connect a device with USB debugging enabled, or start an emulator." size="sm" />
        </Sheet>
      </Specimen>
      <Specimen title="Toasts">
        <Row>
          <Button size="sm" onClick={() => toast.success('Proxy address copied', { description: '127.0.0.1:8000' })}>
            Success
          </Button>
          <Button size="sm" onClick={() => toast.error("Couldn't save rules", { description: 'Rule 3: invalid regex', action: { label: 'Retry', onClick: () => undefined } })}>
            Error
          </Button>
          <Button size="sm" onClick={() => toast.warning('CA expires in 12 days')}>
            Warning
          </Button>
          <Button size="sm" onClick={() => toast.info('Recording paused', { description: 'Traffic is still proxied.' })}>
            Info
          </Button>
          <Button size="sm" onClick={() => undoToast('Deleted 3 exchanges', () => toast('Restored'))}>
            Undo
          </Button>
          <Button size="sm" onClick={() => toast('Request paused', { description: 'POST api.example.com/login', action: { label: 'Jump to', onClick: () => undefined } })}>
            Action
          </Button>
        </Row>
      </Specimen>
    </Group>
  );
}

function BadgesSection() {
  return (
    <Group id="badges" title="Badges & labels" description="Colour is never the only signal: errors and pauses carry icons and labels.">
      <Specimen title="Badge" wide>
        <Row label="soft">
          <Badge>Neutral</Badge>
          <Badge variant="accent">Accent</Badge>
          <Badge variant="success" dot>
            Listening
          </Badge>
          <Badge variant="warning" icon={ShieldAlert}>
            Expiring
          </Badge>
          <Badge variant="danger" pulse>
            Live
          </Badge>
          <Badge variant="info" mono>
            HTTP/2
          </Badge>
        </Row>
        <Row label="outline">
          <Badge outline>Neutral</Badge>
          <Badge outline variant="accent">
            Mock 200
          </Badge>
          <Badge outline variant="success">
            Active
          </Badge>
          <Badge outline variant="warning">
            2 paused
          </Badge>
          <Badge outline variant="danger">
            Failed
          </Badge>
          <Badge outline variant="info" size="sm">
            sm
          </Badge>
        </Row>
      </Specimen>
      <Specimen title="MethodBadge">
        <Row label="text">
          {[...HTTP_METHODS, 'PROPFIND'].map((m) => (
            <MethodBadge key={m} method={m} />
          ))}
        </Row>
        <Row label="pill">
          {['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].map((m) => (
            <MethodBadge key={m} method={m} variant="pill" />
          ))}
        </Row>
        <Row label="fixed 56px">
          {['GET', 'DELETE', 'OPTIONS'].map((m) => (
            <MethodBadge key={m} method={m} fixedWidth className={styles.outlined} />
          ))}
        </Row>
      </Specimen>
      <Specimen title="StatusBadge">
        <Row label="codes">
          {[101, 200, 201, 204, 304, 404, 418, 500, 503].map((s) => (
            <StatusBadge key={s} status={s} />
          ))}
        </Row>
        <Row label="states">
          <StatusBadge state="waiting" />
          <StatusBadge paused="request" />
          <StatusBadge kind="web-socket" status={101} state="streaming" />
          <StatusBadge kind="tls-failure" error="client-tls-rejected" />
          <StatusBadge kind="tunnel" state="complete" />
          <StatusBadge state="complete" />
        </Row>
        <Row label="errors">
          <StatusBadge error="connection-refused" />
          <StatusBadge error="timeout" />
          <StatusBadge error="dns-failed" />
          <StatusBadge error="upstream-cert-invalid" />
          <StatusBadge error="connection-reset" />
          <StatusBadge error="client-aborted" />
        </Row>
        <Row label="with text">
          <StatusBadge status={200} showText />
          <StatusBadge status={404} showText />
          <StatusBadge status={502} showText />
        </Row>
      </Specimen>
      <Specimen title="Kbd · ColorDot">
        <Row label="kbd">
          <Kbd combo="mod+k" />
          <Kbd combo="mod+shift+r" />
          <Kbd combo="shift+?" />
          <Kbd combo="escape" />
          <Kbd combo="mod+enter" size="sm" />
          <Kbd>J</Kbd>
          <Kbd>K</Kbd>
        </Row>
        <Row label="tags">
          {TAG_COLORS.map((c: TagColor) => (
            <ColorDot key={c} color={c} size={10} label={c} />
          ))}
        </Row>
        <Row label="picker">
          {TAG_COLORS.map((c: TagColor, i) => (
            <ColorDot key={c} color={c} size={14} selected={i === 4} label={c} />
          ))}
        </Row>
      </Specimen>
    </Group>
  );
}

function IconsSection() {
  return (
    <Group id="icons" title="App icons" description="Inline SVG, no network. appIconFor() maps process names (e.g. “python3”, “Go-http-client”).">
      <Specimen title="16px" wide>
        <div className={styles.iconGrid}>
          {APP_ICON_NAMES.map((n) => (
            <div key={n} className={styles.iconCell}>
              <AppIcon name={n} size={16} />
              <span>{n}</span>
            </div>
          ))}
        </div>
      </Specimen>
      <Specimen title="Tiles (Connect cards)" wide>
        <div className={styles.tileRow}>
          {APP_ICON_NAMES.map((n) => (
            <Tooltip key={n} content={n}>
              {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip */}
              <span tabIndex={0} className={styles.tileWrap}>
                <AppIcon name={n} size={40} tile label={n} />
              </span>
            </Tooltip>
          ))}
        </div>
      </Specimen>
    </Group>
  );
}

function DataSection() {
  const [rows, setRows] = useState<KeyValueRow[]>(() => [
    makeRow('Content-Type', 'application/json'),
    makeRow('Authorization', 'Bearer eyJhbGciOiJIUzI1NiJ9…'),
    makeRow('X-Debug', '1', false),
  ]);
  const [json, setJson] = useState(() => JSON.stringify(SAMPLE_JSON, null, 2));
  return (
    <Group id="data" title="Data editors" description="KeyValueEditor (paste “Name: value” lines; bulk mode) and CodeMirror 6 with token-driven theme.">
      <Specimen title="KeyValueEditor">
        <KeyValueEditor rows={rows} onChange={setRows} nameSuggestions={KNOWN_HEADER_NAMES} aria-label="Headers" />
      </Specimen>
      <Specimen title="KeyValueEditor (read-only)">
        <KeyValueEditor rows={rows} onChange={() => undefined} readOnly aria-label="Headers (read-only)" />
      </Specimen>
      <Specimen title="CodeEditor · JSON (editable, folding)" wide flush>
        <CodeEditor value={json} onChange={setJson} language="json" maxHeight={260} aria-label="JSON" />
      </Specimen>
      <Specimen title="CodeEditor · HTTP (read-only)" flush>
        <CodeEditor value={SAMPLE_HTTP} language="http" readOnly wrap aria-label="Raw HTTP" />
      </Specimen>
      <Specimen title="CodeEditor · shell, no gutter" flush>
        <CodeEditor value={SAMPLE_SHELL} language="shell" readOnly lineNumbers={false} wrap aria-label="cURL" />
      </Specimen>
      <Specimen title="CodeEditor · placeholder" flush>
        <CodeEditor value="" language="javascript" placeholder="// onRequest(request) { … }" minHeight={72} aria-label="Script" />
      </Specimen>
    </Group>
  );
}

function ViewersSection() {
  const bytes = useMemo(sampleBytes, []);
  return (
    <Group id="viewers" title="Viewers" description="Virtualised: smooth with multi-megabyte bodies.">
      <Specimen title="HexViewer" wide flush>
        <HexViewer data={bytes} fill={false} maxHeight={240} />
      </Specimen>
      <Specimen title="JsonTree" flush>
        <JsonTree data={SAMPLE_JSON} fill={false} maxHeight={300} />
      </Specimen>
      <Specimen title="QrCode">
        <div className={styles.qrRow}>
          <QrCode value="http://192.168.1.23:8000/" size={128} />
          <div className={styles.stack}>
            <strong className={styles.popTitle}>Device setup</strong>
            <span className={styles.note}>Scan on a phone connected to the same network.</span>
            <code className="mono">http://192.168.1.23:8000/</code>
          </div>
        </div>
      </Specimen>
    </Group>
  );
}

function Crasher({ crash }: { crash: boolean }): ReactNode {
  if (crash) throw new Error('Kitchen-sink demo crash: Cannot read properties of undefined (reading "headers")');
  return null;
}

function CrashDemo() {
  const [crash, setCrash] = useState(() => new URLSearchParams(window.location.hash.split('?')[1] ?? '').get('crash') === '1');
  const [key, setKey] = useState(0);
  return (
    <div className={styles.crashDemo}>
      {!crash && (
        <Button variant="subtle-danger" icon={Bell} onClick={() => setCrash(true)}>
          Throw a render error
        </Button>
      )}
      <ErrorBoundary scope="page" resetKey={String(key)}>
        <Crasher crash={crash} />
      </ErrorBoundary>
      {crash && (
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            setCrash(false);
            setKey((k) => k + 1);
          }}
        >
          Reset demo
        </Button>
      )}
    </div>
  );
}

function FeedbackSection() {
  return (
    <Group id="feedback" title="Feedback" description="Empty states teach the next action; loading states keep layout stable.">
      <Specimen title="EmptyState" wide>
        <div className={styles.emptyRow}>
          <EmptyState icon={Inbox} tone="accent" title="Waiting for traffic" description="Point a client at the proxy or launch one from Connect." actions={<Button variant="primary">Connect a client</Button>} />
          <EmptyState icon={Filter} title="No exchanges match" description={<>Nothing matches <code>status:5xx</code>.</>} actions={<Button size="sm">Clear filter</Button>} size="sm" />
          <EmptyState icon={ShieldAlert} tone="warning" title="CA expires soon" description="Regenerate it in Settings → Certificates." size="sm" />
          <EmptyState icon={Bell} tone="danger" title="Akev unreachable" size="sm" />
        </div>
      </Specimen>
      <Specimen title="Spinner · Skeleton">
        <Row label="spinner">
          <Spinner size={12} />
          <Spinner />
          <Spinner size={18} tone="accent" />
          <Spinner size={24} tone="muted" label="Loading" />
        </Row>
        <div className={styles.skeletonDemo}>
          <Skeleton circle height={28} />
          <div className={styles.stackFlex}>
            <Skeleton width="60%" />
            <Skeleton lines={3} height={10} />
          </div>
        </div>
      </Specimen>
      <Specimen title="ErrorBoundary (page scope)" wide>
        <CrashDemo />
      </Specimen>
      <Specimen title="ProgressBar">
        <div className={styles.stack}>
          <ProgressBar value={0.35} aria-label="Accent" />
          <ProgressBar value={0.7} variant="success" aria-label="Success" />
          <ProgressBar value={0.9} variant="warning" size="sm" aria-label="Warning" />
          <ProgressBar value={1} variant="danger" size="sm" aria-label="Danger" />
          <ProgressBar aria-label="Indeterminate" />
        </div>
      </Specimen>
    </Group>
  );
}

function LayoutSection() {
  const [port, setPort] = useState<number | null>(8000);
  return (
    <Group id="layout" title="Layout" description="SplitPane ratios persist per id in UI prefs (double-click a divider to reset).">
      <Specimen title="Toolbar" full flush>
        <Toolbar
          left={
            <>
              <SearchInput value="" onValueChange={() => undefined} placeholder="Filter traffic" size="md" className={styles.w280} shortcut="mod+f" aria-label="Filter" />
              <SegmentedControl value="all" onValueChange={() => undefined} aria-label="Type" items={[{ value: 'all', label: 'All' }, { value: 'fetch', label: 'Fetch' }, { value: 'doc', label: 'Doc' }]} />
            </>
          }
          right={
            <>
              <IconButton label="Pause recording" icon={Circle} shortcut="mod+shift+r" />
              <IconButton label="Clear" icon={Trash2} shortcut="mod+shift+x" />
              <ToolbarSeparator />
              <IconButton label="Export" icon={Download} />
              <IconButton label="More" icon={MoreHorizontal} />
            </>
          }
        />
      </Specimen>
      <Specimen title="SplitPane (horizontal + vertical)" full flush>
        <div className={styles.splitDemo}>
          <SplitPane id="kitchen-sink.h" defaultRatio={0.4} minFirst={120} minSecond={160}>
            <div className={styles.pane}>
              <ArrowLeftRight size={16} aria-hidden />
              List
            </div>
            <SplitPane id="kitchen-sink.v" direction="vertical" minFirst={40} minSecond={40}>
              <div className={styles.pane}>Request</div>
              <div className={styles.pane}>Response</div>
            </SplitPane>
          </SplitPane>
        </div>
      </Specimen>
      <Specimen title="Section · Field" wide>
        <Section title="Proxy" description="Where clients connect. Changes apply immediately." actions={<Button size="sm">Reset</Button>}>
          <div className={styles.stack}>
            <Field label="Port" layout="inline" hint="Currently listening on 127.0.0.1:8000">
              <NumberInput value={port} onValueChange={setPort} min={1} max={65535} className={styles.w120} />
            </Field>
            <Field label="Upstream proxy" layout="inline" error="Must be an http(s):// or socks5:// URL">
              <Input mono defaultValue="ftp://proxy:3128" className={styles.w280} />
            </Field>
            <Field label="DNS-over-HTTPS URL" hint="Leave empty to use the system resolver." required>
              <Input mono placeholder="https://cloudflare-dns.com/dns-query" />
            </Field>
          </div>
        </Section>
      </Specimen>
    </Group>
  );
}

// =====================================================================================
// Page
// =====================================================================================

/** Living style guide of every primitive (internal route). */
export function KitchenSinkPage() {
  const theme = useUiStore((s) => s.theme);
  const setTheme = useUiStore((s) => s.setTheme);
  const density = useUiStore((s) => s.density);
  const setDensity = useUiStore((s) => s.setDensity);

  const jump = (id: SectionId, behavior: ScrollBehavior = smoothScroll()) => document.getElementById(`ks-${id}`)?.scrollIntoView({ behavior, block: 'start' });

  // `#/_kitchen-sink?section=data` deep-links to a section (handy for screenshots).
  const section = useRoute().query.get('section');
  useEffect(() => {
    const id = SECTIONS.find(([s]) => s === section)?.[0];
    if (!id) return;
    // Wait a frame so lazily loaded editors have laid out before scrolling.
    const t = setTimeout(() => jump(id, 'instant'), 50);
    return () => clearTimeout(t);
  }, [section]);

  return (
    <div className={styles.page}>
      <Toolbar
        left={
          <>
            <h1 className={styles.title}>Components</h1>
            <Badge variant="accent" size="sm">
              internal
            </Badge>
          </>
        }
        right={
          <>
            <SegmentedControl<ThemePref> size="sm" value={theme} onValueChange={setTheme} aria-label="Theme" items={THEMES.map((t) => ({ value: t.id, label: t.id === 'contrast' ? 'Contrast' : t.label }))} />
            <ToolbarSeparator />
            <SegmentedControl<Density> size="sm" value={density} onValueChange={setDensity} aria-label="Density" items={DENSITIES.map((d) => ({ value: d.id, label: d.label }))} />
          </>
        }
      />
      <div className={styles.body}>
        <nav className={styles.index} aria-label="Sections">
          {SECTIONS.map(([id, label]) => (
            <button key={id} type="button" className={styles.indexItem} onClick={() => jump(id)}>
              {label}
            </button>
          ))}
        </nav>
        <div className={styles.content}>
          <TokensSection />
          <TypeSection />
          <ButtonsSection />
          <InputsSection />
          <TogglesSection />
          <TabsSection />
          <OverlaysSection />
          <BadgesSection />
          <IconsSection />
          <DataSection />
          <ViewersSection />
          <FeedbackSection />
          <LayoutSection />
        </div>
      </div>
    </div>
  );
}

export default KitchenSinkPage;
