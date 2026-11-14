import clsx from 'clsx';
import { Archive, Bot, Download, FileCode2, Globe, HardDrive, Info, Keyboard, KeyRound, RefreshCcw, RotateCcw, Search, ShieldCheck, SlidersHorizontal, Upload, type LucideIcon } from 'lucide-react';
import { useCallback, useEffect, useRef, useState, useSyncExternalStore, type KeyboardEvent } from 'react';
import type { Settings } from '../../api/types';
import { useCommands, type Command } from '../../app/commands';
import { navigate, useRouteParam } from '../../app/router';
import { useShortcut, useShortcutDocs } from '../../app/shortcuts';
import { SearchInput, Skeleton, Toolbar, toast } from '../../components/ui';
import { useAppStore } from '../../state/app';
import { explainError } from '../connect/actions';
import { SettingsSyncContext } from './context';
import { exportConfig, importConfig, resetUiPrefs, useDataBusy } from './dataActions';
import { SaveIndicator } from './SaveIndicator';
import { AboutSection } from './sections/AboutSection';
import { CaptureSection } from './sections/CaptureSection';
import { CertificatesSection } from './sections/CertificatesSection';
import { DataSection } from './sections/DataSection';
import { GeneralSection } from './sections/GeneralSection';
import { LicenseSection } from './sections/LicenseSection';
import { McpSection } from './sections/McpSection';
import { ProtobufSection } from './sections/ProtobufSection';
import { ProxySection } from './sections/ProxySection';
import { ShortcutsSection } from './sections/ShortcutsSection';
import { isSectionId, SECTIONS, type SectionId } from './sections';
import { searchSettings, type SettingEntry } from './settingsIndex';
import { SettingsSync } from './settingsSync';
import { smoothScroll } from '../../lib/motion';
import styles from './SettingsPage.module.css';

const ICONS: Record<SectionId, LucideIcon> = {
  general: SlidersHorizontal,
  proxy: Globe,
  certificates: ShieldCheck,
  capture: HardDrive,
  protobuf: FileCode2,
  mcp: Bot,
  shortcuts: Keyboard,
  data: Archive,
  license: KeyRound,
  about: Info,
};

const CONTENT: Record<SectionId, () => React.ReactNode> = {
  general: GeneralSection,
  proxy: ProxySection,
  certificates: CertificatesSection,
  capture: CaptureSection,
  protobuf: ProtobufSection,
  mcp: McpSection,
  shortcuts: ShortcutsSection,
  data: DataSection,
  license: LicenseSection,
  about: AboutSection,
};

/** Sections whose content needs the daemon settings. */
const NEEDS_SETTINGS: ReadonlySet<SectionId> = new Set(['proxy', 'capture']);

function LoadingCards() {
  return (
    <div className={styles.loading} role="status" aria-busy="true" aria-label="Loading settings">
      {[0, 1].map((i) => (
        <div key={i} className={styles.loadingCard}>
          <Skeleton width="30%" height={14} />
          <Skeleton lines={3} height={11} />
        </div>
      ))}
    </div>
  );
}

/** Scroll a setting row into view and flash it. */
function revealSetting(id: string): void {
  let tries = 0;
  const attempt = () => {
    const el = document.getElementById(`setting-${id}`);
    if (!el) {
      if (tries++ < 20) requestAnimationFrame(attempt);
      return;
    }
    el.scrollIntoView({ block: 'center', behavior: smoothScroll() });
    el.removeAttribute('data-flash');
    void el.offsetWidth; // restart the animation
    el.setAttribute('data-flash', '');
    window.setTimeout(() => el.removeAttribute('data-flash'), 1700);
    el.querySelector<HTMLElement>('input, button, [role="switch"], [role="radio"][aria-checked="true"], [tabindex="0"]')?.focus({ preventScroll: true });
  };
  requestAnimationFrame(attempt);
}

/** Settings page (DESIGN.md §8.3): sub-nav + section cards; every change applies immediately. */
export function SettingsPage() {
  const param = useRouteParam('section');
  const section: SectionId = isSectionId(param) ? param : 'general';
  const settings = useAppStore((s) => s.settings);
  const [query, setQuery] = useState('');
  const [activeResult, setActiveResult] = useState(0);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const navRef = useRef<HTMLElement | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);

  const [sync] = useState(
    () =>
      new SettingsSync<Settings>({
        save: (s) => useAppStore.getState().saveSettings(s),
        debounceMs: 600,
        // `ui` is owned by the UI prefs store (PATCH /api/settings/ui): never overwrite it with a stale copy.
        prepare: (d) => ({ ...d, ui: useAppStore.getState().settings?.ui ?? d.ui }),
        onError: (e) => toast.error("Couldn't save settings", { description: explainError(e) }),
      }),
  );
  useEffect(() => {
    if (settings) sync.setServer(settings);
  }, [settings, sync]);
  useEffect(() => () => sync.dispose(), [sync]);
  const snap = useSyncExternalStore(sync.subscribe, sync.getSnapshot, sync.getSnapshot);

  // Normalise unknown / missing sections in the URL.
  useEffect(() => {
    if (param !== section) navigate(`/settings/${section}`, { replace: true });
  }, [param, section]);

  // Each section starts at the top.
  useEffect(() => {
    contentRef.current?.scrollTo({ top: 0 });
  }, [section]);

  const results = searchSettings(query);
  useEffect(() => setActiveResult(0), [query]);

  const goTo = useCallback((id: SectionId) => navigate(`/settings/${id}`), []);

  const openResult = (entry: SettingEntry) => {
    setQuery('');
    if (entry.section !== section) navigate(`/settings/${entry.section}`);
    revealSetting(entry.id);
  };

  const onNavKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return;
    if (!(e.target as HTMLElement).matches('[data-section]')) return;
    e.preventDefault();
    const i = SECTIONS.findIndex((s) => s.id === section);
    const next = e.key === 'Home' ? 0 : e.key === 'End' ? SECTIONS.length - 1 : Math.max(0, Math.min(SECTIONS.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)));
    const target = SECTIONS[next];
    if (!target) return;
    navigate(`/settings/${target.id}`, { replace: true });
    navRef.current?.querySelector<HTMLElement>(`[data-section="${target.id}"]`)?.focus();
  };

  const focusSearch = () => {
    searchRef.current?.focus();
    searchRef.current?.select();
  };
  useShortcut('mod+f', focusSearch, { description: 'Search settings', group: 'Settings' });
  useShortcut('/', focusSearch);
  useShortcut('alt+arrowdown', () => goTo(SECTIONS[(SECTIONS.findIndex((s) => s.id === section) + 1) % SECTIONS.length]?.id ?? 'general'), { description: 'Next settings section', group: 'Settings' });
  useShortcut('alt+arrowup', () => goTo(SECTIONS[(SECTIONS.findIndex((s) => s.id === section) - 1 + SECTIONS.length) % SECTIONS.length]?.id ?? 'general'), { description: 'Previous settings section', group: 'Settings' });
  useShortcutDocs([{ combo: 'arrowup or arrowdown', description: 'Move between sections (in the sidebar)', group: 'Settings' }]);

  // ---- palette (section navigation itself is global: "Settings: Proxy" …)
  const { importing } = useDataBusy();
  useCommands((): Command[] => [
    { id: 'settings.search', title: 'Search settings', group: 'Settings', icon: Search, shortcut: 'mod+f', run: () => void requestAnimationFrame(focusSearch) },
    { id: 'settings.export', title: 'Export configuration…', subtitle: 'Settings, rules and preferences as JSON', group: 'Settings', icon: Download, keywords: ['backup', 'save', 'json'], run: () => exportConfig(sync) },
    { id: 'settings.import', title: 'Import configuration…', subtitle: 'Akev or HTTP Toolkit exports', group: 'Settings', icon: Upload, keywords: ['restore', 'load', 'json', 'http toolkit'], disabled: importing, run: () => importConfig(sync) },
    { id: 'settings.reset-ui', title: 'Reset interface preferences', subtitle: 'Undoable', group: 'Settings', icon: RotateCcw, keywords: ['defaults', 'layout', 'columns'], run: resetUiPrefs },
    {
      id: 'settings.regenerate-ca',
      title: 'Regenerate CA certificate…',
      group: 'Settings',
      icon: RefreshCcw,
      keywords: ['certificate', 'rotate', 'new'],
      run: () => {
        if (section !== 'certificates') navigate('/settings/certificates');
        revealSetting('regenerate');
      },
    },
  ], [sync, importing, section]);

  const meta = SECTIONS.find((s) => s.id === section) ?? SECTIONS[0];
  const Content = CONTENT[section];
  const waiting = NEEDS_SETTINGS.has(section) && snap.draft == null;

  return (
    <SettingsSyncContext.Provider value={sync}>
      <div className={styles.page}>
        <Toolbar titleBar left={<h1 className={styles.toolbarTitle}>Settings</h1>} right={<SaveIndicator phase={snap.phase} error={snap.error} />} />
        <div className={styles.body}>
          <nav className={styles.nav} aria-label="Settings sections" ref={navRef} onKeyDown={onNavKeyDown}>
            <div className={styles.searchWrap}>
              <SearchInput
                ref={searchRef}
                size="sm"
                value={query}
                onValueChange={setQuery}
                placeholder="Search settings"
                shortcut="mod+f"
                aria-label="Search settings"
                aria-controls="settings-search-results"
                aria-activedescendant={query.trim() && results[activeResult] ? `settings-result-${activeResult}` : undefined}
                onKeyDown={(e) => {
                  if (!results.length) return;
                  if (e.key === 'ArrowDown') {
                    e.preventDefault();
                    setActiveResult((i) => Math.min(results.length - 1, i + 1));
                  } else if (e.key === 'ArrowUp') {
                    e.preventDefault();
                    setActiveResult((i) => Math.max(0, i - 1));
                  } else if (e.key === 'Enter') {
                    e.preventDefault();
                    const r = results[activeResult];
                    if (r) openResult(r);
                  }
                }}
              />
            </div>
            {query.trim() ? (
              // biome-ignore lint/a11y/noNoninteractiveElementToInteractiveRole: a listbox driven by the search field (aria-activedescendant); the list markup keeps its styles
              <ul className={styles.results} id="settings-search-results" role="listbox" aria-label="Matching settings">
                {results.length === 0 && <li className={styles.noResults}>No settings match.</li>}
                {results.map((r, i) => {
                  const Icon = ICONS[r.section];
                  return (
                    // biome-ignore lint/a11y/noNoninteractiveElementToInteractiveRole lint/a11y/useFocusableInteractive: options of the listbox above; focus stays in the search field
                    <li key={r.id} id={`settings-result-${i}`} role="option" aria-selected={i === activeResult}>
                      <button type="button" className={clsx(styles.result, i === activeResult && styles.resultActive)} onClick={() => openResult(r)} onMouseEnter={() => setActiveResult(i)} tabIndex={-1}>
                        <Icon size={13} strokeWidth={1.75} className={styles.resultIcon} aria-hidden />
                        <span className={styles.resultText}>
                          <span className={styles.resultTitle}>{r.title}</span>
                          <span className={styles.resultSection}>{SECTIONS.find((s) => s.id === r.section)?.label}</span>
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            ) : (
              <ul className={styles.sections}>
                {SECTIONS.map((s) => {
                  const Icon = ICONS[s.id];
                  const current = s.id === section;
                  return (
                    <li key={s.id}>
                      <a
                        href={`#/settings/${s.id}`}
                        className={clsx(styles.navItem, current && styles.navItemActive)}
                        aria-current={current ? 'page' : undefined}
                        data-section={s.id}
                        tabIndex={current ? 0 : -1}
                        onClick={(e) => {
                          e.preventDefault();
                          goTo(s.id);
                        }}
                      >
                        <Icon size={15} strokeWidth={1.75} aria-hidden />
                        {s.label}
                      </a>
                    </li>
                  );
                })}
              </ul>
            )}
          </nav>
          <div className={styles.content} ref={contentRef}>
            <div className={styles.inner} key={section}>
              <header className={styles.header}>
                <h2 className={styles.title}>{meta?.label}</h2>
                <p className={styles.description}>{meta?.description}</p>
              </header>
              {waiting ? <LoadingCards /> : <Content />}
            </div>
          </div>
        </div>
      </div>
    </SettingsSyncContext.Provider>
  );
}

export default SettingsPage;
