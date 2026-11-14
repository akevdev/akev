import { ChevronRight, CircleAlert, Copy, Download, PlugZap, RefreshCw, Search, Square, Wifi, WifiOff, Wrench } from 'lucide-react';
import { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { api } from '../../api/client';
import { useCommands, type Command } from '../../app/commands';
import { navigate, useRouteParam } from '../../app/router';
import { useShortcut, useShortcutDocs } from '../../app/shortcuts';
import { AppIcon, Button, EmptyState, SearchInput, Skeleton, Toolbar, toast } from '../../components/ui';
import { copyText, downloadUrl } from '../../lib/platform';
import { useAppStore } from '../../state/app';
import { usePagePref } from '../../state/ui';
import { deactivateAll, explainError } from './actions';
import { ActiveStrip } from './ActiveStrip';
import { CATEGORY_FILTERS, CATEGORY_LABEL, countByCategory, filterCatalog, iconKey, isDormant, manualInterceptor, panelKindFor, sortCatalog, splitDormant, type CategoryFilter } from './catalog';
import { CategoryChips } from './CategoryChips';
import { InterceptorCard } from './InterceptorCard';
import { InterceptorSheet } from './InterceptorSheet';
import { CertificateCard, DeviceSetupCard, ProxyCard } from './OverviewCards';
import { useLanToggle, useProxyInfo } from './useProxyInfo';
import { smoothScroll } from '../../lib/motion';
import styles from './ConnectPage.module.css';

type LoadState = 'loading' | 'ready' | 'error';

/** Move focus between card actions with the arrow keys (grid-aware). */
function moveCardFocus(e: KeyboardEvent<HTMLDivElement>): void {
  if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) return;
  const target = e.target as HTMLElement;
  if (!target.matches('[data-card-action]')) return;
  const buttons = Array.from(e.currentTarget.querySelectorAll<HTMLElement>('[data-card-action]'));
  const index = buttons.indexOf(target);
  if (index < 0) return;
  const firstTop = buttons[0]?.closest('article')?.offsetTop ?? 0;
  const columns = Math.max(1, buttons.filter((b) => (b.closest('article')?.offsetTop ?? 0) === firstTop).length);
  let next = index;
  if (e.key === 'ArrowLeft') next = index - 1;
  else if (e.key === 'ArrowRight') next = index + 1;
  else if (e.key === 'ArrowUp') next = index - columns;
  else if (e.key === 'ArrowDown') next = index + columns;
  else if (e.key === 'Home') next = 0;
  else if (e.key === 'End') next = buttons.length - 1;
  next = Math.max(0, Math.min(buttons.length - 1, next));
  if (next === index) return;
  e.preventDefault();
  const el = buttons[next];
  el?.focus();
  el?.closest('article')?.scrollIntoView({ block: 'nearest' });
}

function SkeletonGrid() {
  return (
    <div className={styles.grid} role="status" aria-busy="true" aria-label="Loading interceptors">
      {Array.from({ length: 8 }, (_, i) => (
        <div key={i} className={styles.skeletonCard}>
          <Skeleton width={40} height={40} radius={10} />
          <Skeleton width="45%" height={14} />
          <Skeleton lines={2} height={10} />
          <div className={styles.skeletonFoot}>
            <Skeleton width={90} height={10} />
            <Skeleton width={72} height={24} radius={6} />
          </div>
        </div>
      ))}
    </div>
  );
}

/** Connect page (DESIGN.md §8.2): proxy & CA overview, active sources, interceptor catalogue. */
export function ConnectPage() {
  const interceptors = useAppStore((s) => s.interceptors);
  const connection = useAppStore((s) => s.connection);
  const proxy = useProxyInfo();
  const lan = useLanToggle();
  const routeId = useRouteParam('interceptor');
  const [query, setQuery] = useState('');
  const deferredQuery = useDeferredValue(query);
  const [storedCategory, setCategory] = usePagePref<CategoryFilter>('connect.category', 'all');
  const [loadState, setLoadState] = useState<LoadState>(interceptors.length > 0 ? 'ready' : 'loading');
  const searchRef = useRef<HTMLInputElement | null>(null);
  const gridRef = useRef<HTMLDivElement | null>(null);
  const openedByPush = useRef(false);

  const refresh = useCallback(() => {
    setLoadState((s) => (s === 'error' ? 'loading' : s));
    useAppStore
      .getState()
      .refreshInterceptors()
      .then(
        () => setLoadState('ready'),
        (e: unknown) => {
          if (useAppStore.getState().interceptors.length > 0) setLoadState('ready');
          else {
            setLoadState('error');
            console.warn('[akev] failed to load interceptors:', explainError(e));
          }
        },
      );
  }, []);

  useEffect(refresh, [refresh]);
  // The stream may deliver the list before our fetch resolves.
  useEffect(() => {
    if (interceptors.length > 0) setLoadState('ready');
  }, [interceptors.length]);

  const sorted = useMemo(() => sortCatalog(interceptors), [interceptors]);
  const counts = useMemo(() => countByCategory(sorted), [sorted]);
  const category: CategoryFilter = CATEGORY_FILTERS.some((c) => c.id === storedCategory) && (storedCategory === 'all' || counts[storedCategory] > 0) ? storedCategory : 'all';
  const visible = useMemo(() => filterCatalog(sorted, category, deferredQuery), [sorted, category, deferredQuery]);
  // "All" without a search keeps not-installed / unsupported entries behind a disclosure.
  const [showDormant, setShowDormant] = usePagePref<boolean>('connect.showUnavailable', false);
  const grouped = useMemo(() => (category === 'all' && !deferredQuery.trim() ? splitDormant(visible) : { main: visible, dormant: [] }), [visible, category, deferredQuery]);
  const matchCounts = useMemo(() => (deferredQuery.trim() ? countByCategory(filterCatalog(sorted, 'all', deferredQuery)) : null), [sorted, deferredQuery]);
  const active = useMemo(() => sorted.filter((i) => i.active), [sorted]);
  const manual = manualInterceptor(sorted);

  // ---- sheet routing (#/connect/<id>)
  const selected = routeId ? (interceptors.find((i) => i.id === routeId) ?? null) : null;
  const lastSelected = useRef(selected);
  if (selected) lastSelected.current = selected; // keeps content during the close animation
  const sheetOpen = selected != null && panelKindFor(selected) != null;

  const openInterceptor = useCallback((id: string) => {
    openedByPush.current = true;
    navigate(`/connect/${encodeURIComponent(id)}`);
  }, []);

  const closeSheet = useCallback(() => {
    if (openedByPush.current && window.history.length > 1) {
      openedByPush.current = false;
      window.history.back();
    } else {
      navigate('/connect', { replace: true });
    }
  }, []);

  // A route to a one-click interceptor (e.g. from elsewhere): reveal its card instead.
  useEffect(() => {
    if (!routeId || loadState !== 'ready') return;
    const target = interceptors.find((i) => i.id === routeId);
    if (target && panelKindFor(target) != null) return;
    navigate('/connect', { replace: true });
    if (!target) return;
    setQuery('');
    setCategory('all');
    if (isDormant(target)) setShowDormant(true);
    // Two frames: the category / disclosure change has to render first.
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const card = gridRef.current?.querySelector<HTMLElement>(`[data-interceptor="${CSS.escape(target.id)}"]`);
        card?.scrollIntoView({ block: 'center', behavior: smoothScroll() });
        card?.querySelector<HTMLElement>('[data-card-action]')?.focus({ preventScroll: true });
      }),
    );
  }, [routeId, loadState, interceptors, setCategory, setShowDormant]);

  const openGuide = useCallback(
    (kind: 'ios' | 'android' | 'wireguard') => {
      const target = sorted.find((i) => panelKindFor(i) === kind);
      if (target) openInterceptor(target.id);
      else if (manual) openInterceptor(manual.id);
    },
    [sorted, manual, openInterceptor],
  );

  // ---- keyboard
  const focusSearch = () => {
    searchRef.current?.focus();
    searchRef.current?.select();
  };
  useShortcut('mod+f', focusSearch, { description: 'Search interceptors', group: 'Connect' });
  useShortcut('/', focusSearch);
  useShortcutDocs([
    { combo: 'arrowleft or arrowright', description: 'Move between interceptor cards', group: 'Connect' },
    { combo: 'arrowup or arrowdown', description: 'Move up / down the card grid', group: 'Connect' },
    { combo: 'enter', description: 'Launch / set up the focused interceptor', group: 'Connect' },
  ]);

  // ---- palette
  useCommands((): Command[] => {
    const cmds: Command[] = [
      { id: 'connect.search', title: 'Search interceptors', group: 'Connect', icon: Search, shortcut: 'mod+f', run: () => void requestAnimationFrame(focusSearch) },
      { id: 'connect.ca.pem', title: 'Download CA certificate (PEM)', group: 'Connect', icon: Download, keywords: ['certificate', 'cert', 'trust'], run: () => void downloadUrl(api.ca.pemUrl(), 'akev-ca.pem') },
      { id: 'connect.ca.der', title: 'Download CA certificate (DER .crt)', group: 'Connect', icon: Download, keywords: ['certificate', 'cert', 'windows', 'android'], run: () => void downloadUrl(api.ca.derUrl(), 'akev-ca.crt') },
      {
        id: 'connect.lan',
        title: lan.checked ? 'Only allow this computer to connect' : 'Allow other devices to connect',
        subtitle: lan.checked ? 'Stop listening on the network' : 'Phones, tablets and other computers',
        group: 'Connect',
        icon: lan.checked ? WifiOff : Wifi,
        keywords: ['lan', 'network', 'bind', '0.0.0.0', 'phone'],
        disabled: lan.disabled,
        run: () => lan.toggle(!lan.checked),
      },
    ];
    if (proxy.primaryLan) {
      const addr = proxy.primaryLan;
      cmds.push({
        id: 'connect.copy-lan',
        title: 'Copy network proxy address',
        subtitle: addr,
        group: 'Connect',
        icon: Copy,
        run: async () => {
          if (await copyText(addr)) toast.success('Network address copied', { description: addr });
        },
      });
    }
    if (active.length > 0) {
      cmds.push({ id: 'connect.stop-all', title: active.length === 1 ? `Stop ${active[0]?.name}` : `Stop all ${active.length} interceptors`, group: 'Connect', icon: Square, run: () => void deactivateAll(sorted) });
    }
    return cmds;
  }, [lan.checked, lan.disabled, proxy.primaryLan, active, sorted]);

  const showChips = loadState === 'ready' && sorted.length > 0;
  const noMatches = loadState === 'ready' && sorted.length > 0 && visible.length === 0;
  const otherMatches = noMatches && category !== 'all' && matchCounts ? matchCounts.all : 0;
  const q = deferredQuery.trim();

  return (
    <div className={styles.page}>
      <Toolbar
        titleBar
        left={<h1 className={styles.toolbarTitle}>Connect</h1>}
        right={
          active.length > 0 ? (
            <span className={styles.toolbarActive}>
              <span className={styles.toolbarDot} aria-hidden />
              <span className="tnum">{active.length}</span> active
            </span>
          ) : undefined
        }
      />
      <div className={styles.scroll}>
        <div className={styles.content}>
          <header className={styles.header}>
            <h2 className={styles.title}>Connect a client</h2>
            <p className={styles.subtitle}>Launch a pre-configured browser or terminal, intercept phones and containers, or point any tool at the proxy by hand.</p>
          </header>

          <div className={styles.overview}>
            <ProxyCard />
            <CertificateCard />
            <DeviceSetupCard onOpenGuide={openGuide} />
          </div>

          <ActiveStrip active={active} all={sorted} onOpen={openInterceptor} proxyAddress={proxy.local} loading={loadState === 'loading' && sorted.length === 0} />

          <div className={styles.filters} role="search">
            <SearchInput
              ref={searchRef}
              value={query}
              onValueChange={setQuery}
              placeholder="Chrome, Android, Docker, curl…"
              shortcut="mod+f"
              className={styles.search}
              aria-label="Search interceptors"
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === 'ArrowDown') {
                  const first = gridRef.current?.querySelector<HTMLElement>('[data-card-action]:not(:disabled)');
                  if (first) {
                    e.preventDefault();
                    first.focus();
                  }
                }
              }}
            />
            {showChips && <CategoryChips value={category} onValueChange={setCategory} counts={counts} matchCounts={matchCounts} />}
          </div>

          {loadState === 'loading' && sorted.length === 0 ? (
            <SkeletonGrid />
          ) : loadState === 'error' && sorted.length === 0 ? (
            <EmptyState
              icon={CircleAlert}
              tone="danger"
              title="Couldn't load interceptors"
              description={
                connection === 'open'
                  ? "Akev didn't return its list of interceptors. Try again, or set up your client by hand with the proxy address and certificate above."
                  : "Akev's background service isn't reachable right now. The list loads as soon as it reconnects."
              }
              actions={
                <Button icon={RefreshCw} onClick={refresh}>
                  Try again
                </Button>
              }
              className={styles.state}
            />
          ) : sorted.length === 0 ? (
            <EmptyState icon={PlugZap} title="No interceptors on this platform" description="Point your client at the proxy address above and trust the certificate." className={styles.state} />
          ) : noMatches ? (
            <EmptyState
              icon={Search}
              title={category === 'all' ? `Nothing matches “${q}”` : `No ${CATEGORY_LABEL[category as Exclude<CategoryFilter, 'all'>].toLowerCase()} interceptor matches “${q}”`}
              description={
                manual
                  ? 'Manual setup works with any client that supports an HTTP proxy — env vars, Node, Python, Java, Go, Rust, curl and more.'
                  : 'Try a different name, or point your client at the proxy address above.'
              }
              actions={
                <>
                  {otherMatches > 0 && (
                    <Button variant="primary" onClick={() => setCategory('all')}>
                      Show {otherMatches} in all categories
                    </Button>
                  )}
                  {manual && otherMatches === 0 && (
                    <Button variant="primary" icon={Wrench} onClick={() => openInterceptor(manual.id)}>
                      Open manual setup
                    </Button>
                  )}
                  <Button onClick={() => setQuery('')}>Clear search</Button>
                </>
              }
              className={styles.state}
            />
          ) : (
            <div ref={gridRef} className={styles.catalog} onKeyDown={moveCardFocus} role="group" aria-label="Interceptors">
              <div className={styles.grid}>
                {grouped.main.map((i) => (
                  <InterceptorCard key={i.id} info={i} platform={proxy.platform} onOpen={openInterceptor} />
                ))}
                {q && manual && !visible.includes(manual) && (
                  <button type="button" className={styles.manualHint} onClick={() => openInterceptor(manual.id)}>
                    <AppIcon name={iconKey(manual.icon)} size={32} tile />
                    <span className={styles.manualHintText}>
                      <span className={styles.manualHintTitle}>Not listed?</span>
                      <span className={styles.manualHintDesc}>Manual setup works with anything that supports a proxy.</span>
                    </span>
                  </button>
                )}
              </div>
              {grouped.dormant.length > 0 && (
                <>
                  <button type="button" className={styles.dormantToggle} aria-expanded={showDormant} aria-controls="connect-dormant" onClick={() => setShowDormant(!showDormant)}>
                    <ChevronRight size={14} strokeWidth={2} className={styles.dormantChevron} aria-hidden />
                    <span className={styles.dormantTitle}>
                      <span className="tnum">{grouped.dormant.length}</span> not available on this computer
                    </span>
                    <span className={styles.dormantNames}>{grouped.dormant.map((i) => i.name).join(', ')}</span>
                  </button>
                  {showDormant && (
                    <div className={styles.grid} id="connect-dormant">
                      {grouped.dormant.map((i) => (
                        <InterceptorCard key={i.id} info={i} platform={proxy.platform} onOpen={openInterceptor} />
                      ))}
                    </div>
                  )}
                </>
              )}
            </div>
          )}
        </div>
      </div>

      <InterceptorSheet
        info={selected ?? lastSelected.current}
        open={sheetOpen}
        onOpenChange={(open) => {
          if (!open) closeSheet();
        }}
        onOpenOther={openInterceptor}
      />
    </div>
  );
}

export default ConnectPage;
