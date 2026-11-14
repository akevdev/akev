/**
 * App-level commands and shortcuts that are available on every page. Pages add their own
 * with `useCommands` / `useShortcut` while mounted.
 */

import {
  Archive,
  ArrowRight,
  Circle,
  CirclePause,
  CirclePlay,
  Copy,
  Download,
  FolderOpen,
  Hourglass,
  Keyboard,
  KeyRound,
  LayoutGrid,
  Moon,
  Play,
  RefreshCw,
  Rocket,
  Rows3,
  Save,
  Settings,
  Sparkles,
  Sun,
  SunMoon,
  Type,
  Upload,
} from 'lucide-react';
import { useLayoutEffect } from 'react';
import { api, errorMessage } from '../api/client';
import { startProTrial } from '../components/pro';
import { AppIcon, MethodBadge, toast } from '../components/ui';
import { PRO_TERMS, TRIAL_DAYS } from '../lib/license';
import { copyText } from '../lib/platform';
import { summaryUrl } from '../lib/http';
import { proxyAddress, useAppStore } from '../state/app';
import { openLicenseKeyEntry, openUpgrade, useLicense } from '../state/license';
import { SECTIONS } from '../pages/settings/sections';
import { exportHar, importFile, toggleRecording } from '../pages/traffic/actions/exchangeActions';
import { openFirstRun } from '../pages/traffic/first-run/firstRun';
import { trafficStore } from '../state/traffic';
import { DENSITIES, THEMES, UI_SCALES, useUiStore } from '../state/ui';
import { openPalette, togglePalette, useCommandProvider, useCommands, type Command } from './commands';
import { NAV_ITEMS } from './navItems';
import { navigate } from './router';
import { importForeign, openCaptureFiles } from './sessions/actions';
import { openSessionsDialog } from './sessions/sessions';
import { setShortcutOverrides, useShortcut, useShortcutDocs } from './shortcuts';
import { setShortcutsOverlayOpen } from './shortcutsOverlay';

/** Interceptors that are set up through a Connect panel (instructions, device pickers…)
 *  rather than launched directly, even though they need no activation options. */
const SETUP_PANEL_IDS: ReadonlySet<string> = new Set(['manual', 'ios', 'existing-terminal', 'android-adb', 'docker', 'electron']);

/** Register global shortcuts, commands and providers. Mount once in App. */
export function useGlobalCommands(): void {
  const interceptors = useAppStore((s) => s.interceptors);
  const pausedCount = useAppStore((s) => s.paused.length);
  const recording = useAppStore((s) => s.recording);
  const status = useAppStore((s) => s.status);
  const license = useLicense();

  // ---- the user's rebound shortcuts (Settings › Shortcuts), applied before pages register theirs
  const shortcutOverrides = useUiStore((s) => s.shortcuts);
  useLayoutEffect(() => setShortcutOverrides(shortcutOverrides), [shortcutOverrides]);

  // ---- shortcuts
  useShortcut('mod+k', () => togglePalette(), { allowInInputs: true, allowInOverlays: true, description: 'Command palette', group: 'General' });
  useShortcut('mod+shift+p', () => openPalette(), { allowInInputs: true, description: 'Command palette', group: 'General' });
  useShortcut('shift+?', () => setShortcutsOverlayOpen(true), { description: 'Keyboard shortcuts', group: 'General' });
  useShortcut('mod+shift+r', toggleRecording, { allowInInputs: true, description: 'Pause / resume recording', group: 'Traffic' });
  useShortcut('mod+1', () => navigate('/traffic'), { allowInInputs: true, description: 'Go to Traffic', group: 'Navigation' });
  useShortcut('mod+2', () => navigate('/rules'), { allowInInputs: true, description: 'Go to Rules', group: 'Navigation' });
  useShortcut('mod+3', () => navigate('/compose'), { allowInInputs: true, description: 'Go to Compose', group: 'Navigation' });
  useShortcut('mod+4', () => navigate('/connect'), { allowInInputs: true, description: 'Go to Connect', group: 'Navigation' });
  useShortcut('mod+,', () => navigate('/settings'), { allowInInputs: true, description: 'Open Settings', group: 'Navigation' });
  useShortcut('mod+o', () => void openCaptureFiles(), { description: 'Open a capture file', group: 'Traffic' });
  useShortcutDocs([{ combo: 'escape', description: 'Close overlay / clear focus / deselect', group: 'General' }]);

  // ---- static commands
  useCommands(() => {
    const cmds: Command[] = [];
    for (const item of NAV_ITEMS) {
      cmds.push({ id: `nav.${item.page}`, title: `Go to ${item.label}`, subtitle: item.description, group: 'Navigation', icon: item.icon, shortcut: item.shortcut, run: () => navigate(item.path) });
    }
    cmds.push({ id: 'nav.settings', title: 'Open Settings', group: 'Navigation', icon: Settings, shortcut: 'mod+,', run: () => navigate('/settings') });
    for (const { id, label } of SECTIONS) {
      cmds.push({ id: `nav.settings.${id}`, title: `Settings: ${label}`, group: 'Navigation', icon: Settings, keywords: ['preferences'], run: () => navigate(`/settings/${id}`) });
    }

    cmds.push(
      {
        id: 'traffic.recording',
        title: recording ? 'Pause recording' : 'Resume recording',
        subtitle: recording ? 'Keep proxying without storing traffic' : undefined,
        group: 'Traffic',
        icon: recording ? CirclePause : Circle,
        shortcut: 'mod+shift+r',
        run: toggleRecording,
      },
      {
        id: 'traffic.export-har',
        title: 'Export all traffic as HAR',
        group: 'Traffic',
        icon: Download,
        keywords: ['save', 'download'],
        run: () => exportHar('all'),
      },
      {
        id: 'traffic.open-capture',
        title: 'Open capture file…',
        subtitle: 'HAR, .akev, Charles, Fiddler or mitmproxy: the format is detected',
        group: 'Traffic',
        icon: FolderOpen,
        shortcut: 'mod+o',
        keywords: ['import', 'open', 'load', 'har', 'session', 'file'],
        run: () => void openCaptureFiles(),
      },
      { id: 'traffic.import-har', title: 'Import HAR file…', group: 'Traffic', icon: Upload, keywords: ['open', 'load'], run: () => void importFile('har') },
      { id: 'traffic.import-session', title: 'Import Akev session…', group: 'Traffic', icon: Upload, keywords: ['open', 'load', '.akev'], run: () => void importFile('session') },
      { id: 'traffic.import-charles', title: 'Import Charles session…', subtitle: 'JSON session file (.chlsj)', group: 'Traffic', icon: Upload, keywords: ['charles', 'chlsj', 'open', 'load'], run: () => void importForeign('charles') },
      { id: 'traffic.import-fiddler', title: 'Import Fiddler sessions…', subtitle: 'Session archive (.saz)', group: 'Traffic', icon: Upload, keywords: ['fiddler', 'saz', 'open', 'load'], run: () => void importForeign('fiddler') },
      { id: 'traffic.import-mitmproxy', title: 'Import mitmproxy flows…', subtitle: 'Flow file saved by mitmproxy / mitmdump -w', group: 'Traffic', icon: Upload, keywords: ['mitmproxy', 'mitmdump', 'flows', 'open', 'load'], run: () => void importForeign('mitmproxy') },
      { id: 'sessions.open', title: 'Sessions…', subtitle: 'Open, rename or delete saved sessions and autosaves', group: 'Traffic', icon: Archive, keywords: ['saved', 'open', 'restore', 'autosave', 'library'], run: () => openSessionsDialog() },
      { id: 'sessions.save', title: 'Save traffic as session…', group: 'Traffic', icon: Save, keywords: ['save', 'keep', 'session', 'snapshot'], run: () => openSessionsDialog({ save: true }) },
    );

    if (pausedCount > 0) {
      cmds.push(
        {
          id: 'breakpoints.jump',
          title: 'Jump to paused exchange',
          subtitle: `${pausedCount} paused`,
          group: 'Breakpoints',
          icon: ArrowRight,
          run: () => {
            const first = useAppStore.getState().paused[0];
            if (first) navigate(`/traffic/${first.id}`);
          },
        },
        {
          id: 'breakpoints.resume-all',
          title: 'Resume all paused exchanges',
          subtitle: 'Continue unchanged',
          group: 'Breakpoints',
          icon: CirclePlay,
          run: async () => {
            try {
              await useAppStore.getState().resumeAllBreakpoints();
              toast.success('Resumed all paused exchanges');
            } catch (e) {
              toast.error("Couldn't resume", { description: errorMessage(e) });
            }
          },
        },
      );
    }

    const address = proxyAddress(status);
    if (address) {
      cmds.push({
        id: 'connect.copy-proxy',
        title: 'Copy proxy address',
        subtitle: address,
        group: 'Connect',
        icon: Copy,
        run: async () => {
          if (await copyText(address)) toast.success('Proxy address copied', { description: address });
        },
      });
    }
    for (const i of interceptors) {
      if (!i.supported) continue;
      if (i.needsConfig || SETUP_PANEL_IDS.has(i.id)) {
        cmds.push({
          id: `connect.setup.${i.id}`,
          title: `Set up ${i.name}…`,
          subtitle: i.description,
          group: 'Connect',
          iconNode: <AppIcon name={i.icon} size={15} />,
          keywords: i.tags,
          run: () => navigate(`/connect/${i.id}`),
        });
      } else {
        cmds.push({
          id: `connect.launch.${i.id}`,
          title:
            i.id === 'system-proxy'
              ? i.active
                ? 'Disable system proxy'
                : 'Enable system proxy'
              : i.id === 'fresh-terminal'
                ? 'Open an intercepted terminal'
                : i.active
                  ? `${i.name} (active)`
                  : `Launch ${i.name}`,
          subtitle: i.available ? i.description : i.unavailableReason,
          group: 'Connect',
          iconNode: <AppIcon name={i.icon} size={15} />,
          keywords: i.tags,
          disabled: !i.available,
          run: async () => {
            try {
              if (i.id === 'system-proxy' && i.active) {
                await api.interceptors.deactivate(i.id);
                toast.success('System proxy restored');
                return;
              }
              const res = await api.interceptors.activate(i.id);
              if (res.success) toast.success(res.message ?? `${i.name} is now intercepted`);
              else toast.error(res.message ?? `Couldn't launch ${i.name}`);
            } catch (e) {
              toast.error(`Couldn't launch ${i.name}`, { description: errorMessage(e) });
            }
          },
        });
      }
    }

    for (const t of THEMES) {
      const icon = t.id === 'light' ? Sun : t.id === 'system' ? SunMoon : Moon;
      cmds.push({ id: `appearance.theme.${t.id}`, title: `Theme: ${t.label}`, group: 'Appearance', icon, keywords: ['color', 'dark mode', 'light mode'], run: () => useUiStore.getState().setTheme(t.id) });
    }
    for (const d of DENSITIES) {
      cmds.push({ id: `appearance.density.${d.id}`, title: `Density: ${d.label}`, subtitle: `${d.rowHeight}px rows`, group: 'Appearance', icon: d.id === 'compact' ? Rows3 : LayoutGrid, run: () => useUiStore.getState().setDensity(d.id) });
    }
    for (const s of UI_SCALES) {
      cmds.push({ id: `appearance.scale.${s}`, title: `Text size: ${Math.round(s * 100)}%`, group: 'Appearance', icon: Type, keywords: ['zoom', 'font', 'scale'], run: () => useUiStore.getState().setScale(s) });
    }

    // ---- Akev Pro
    if (license) {
      const keywords = ['pro', 'license', 'licence', 'buy', 'purchase', 'unlock', 'upgrade'];
      if (license.state !== 'active') {
        cmds.push({ id: 'pro.upgrade', title: 'Upgrade to Akev Pro…', subtitle: PRO_TERMS, group: 'General', icon: Sparkles, keywords, run: () => openUpgrade() });
        if (license.trialAvailable) cmds.push({ id: 'pro.trial', title: `Start ${TRIAL_DAYS}-day Pro trial`, subtitle: 'No account, no card', group: 'General', icon: Hourglass, keywords: [...keywords, 'trial', 'try'], run: () => void startProTrial() });
      }
      cmds.push({ id: 'pro.license-key', title: 'Enter license key…', subtitle: license.state === 'active' ? 'Replace the installed key' : 'Activate Akev Pro', group: 'General', icon: KeyRound, keywords: [...keywords, 'activate', 'key', 'serial'], run: openLicenseKeyEntry });
    }

    cmds.push(
      { id: 'help.shortcuts', title: 'Keyboard shortcuts', group: 'Help', icon: Keyboard, shortcut: 'shift+?', run: () => setShortcutsOverlayOpen(true) },
      {
        id: 'help.setup-guide',
        title: 'Run the setup guide',
        subtitle: 'Trust the certificate, launch a browser, see your first request',
        group: 'Help',
        icon: Rocket,
        keywords: ['first run', 'onboarding', 'welcome', 'getting started', 'tour'],
        run: () => {
          openFirstRun();
          navigate('/traffic');
        },
      },
      { id: 'help.reload', title: 'Reload interface', group: 'Help', icon: RefreshCw, run: () => window.location.reload() },
    );
    return cmds;
  }, [interceptors, pausedCount, recording, status, license]);

  // ---- dynamic: jump to an exchange by #id or URL fragment
  useCommandProvider({
    id: 'exchanges.jump',
    provide(search) {
      const out: Command[] = [];
      const idMatch = /^#?(\d+)$/.exec(search);
      if (idMatch && search.startsWith('#')) {
        const id = Number(idMatch[1]);
        const s = trafficStore.get(id);
        out.push({
          id: `jump.${id}`,
          title: s ? `#${id}  ${s.host}${s.path}` : `Go to exchange #${id}`,
          group: 'Exchanges',
          icon: Play,
          iconNode: s ? <MethodBadge method={s.method} size="sm" /> : undefined,
          run: () => navigate(`/traffic/${id}`),
        });
        return out;
      }
      if (search.length < 3 || search.startsWith('>')) return out;
      const q = search.toLowerCase();
      const order = trafficStore.getSnapshot().order;
      for (let i = order.length - 1, scanned = 0; i >= 0 && out.length < 6 && scanned < 20_000; i--, scanned++) {
        const s = trafficStore.get(order[i] as number);
        if (!s) continue;
        const url = summaryUrl(s);
        if (!url.toLowerCase().includes(q)) continue;
        out.push({
          id: `jump.${s.id}`,
          title: `${s.host}${s.path}`,
          subtitle: `#${s.id}${s.status ? ` · ${s.status}` : ''}`,
          group: 'Exchanges',
          iconNode: <MethodBadge method={s.method} size="sm" />,
          run: () => navigate(`/traffic/${s.id}`),
        });
      }
      return out;
    },
  });
}
