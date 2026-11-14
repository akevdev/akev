import { lazy, Suspense, useEffect, useState, type ComponentType, type LazyExoticComponent, type ReactNode } from 'react';
import { UpgradeDialog } from '../components/pro';
import { ConfirmHost, Spinner, Toaster, TooltipProvider } from '../components/ui';
import { TrafficPage } from '../pages/traffic/TrafficPage';
import { useUiStore } from '../state/ui';
import { usePaletteState } from './commands';
import { ConnectionOverlay } from './ConnectionOverlay';
import { useDesktopIntegration } from './desktop/desktopIntegration';
import { ErrorBoundary } from './ErrorBoundary';
import { useGlobalCommands } from './globalCommands';
import { NavRail } from './NavRail';
import { setStartPage, usePage, type PageId } from './router';
import { useSessionsDialog } from './sessions/sessions';
import { useShortcutsOverlayOpen } from './shortcutsOverlay';
import { StatusBar } from './StatusBar';
import { WindowControls } from './WindowControls';
import { useApplyTheme } from './theme';
import styles from './App.module.css';

const RulesPage = lazy(() => import('../pages/rules/RulesPage'));
const ComposePage = lazy(() => import('../pages/compose/ComposePage'));
const ConnectPage = lazy(() => import('../pages/connect/ConnectPage'));
const SettingsPage = lazy(() => import('../pages/settings/SettingsPage'));
const NotFoundPage = lazy(() => import('../pages/NotFoundPage'));
/** The primitives gallery (`#/_kitchen-sink`) exists only in dev builds; production builds
 *  drop the import and show Not Found for the route. */
const KitchenSinkPage = import.meta.env.DEV ? lazy(() => import('../pages/kitchen-sink/KitchenSinkPage')) : NotFoundPage;
const SessionsDialog = lazy(() => import('./sessions/SessionsDialog'));
const CommandPalette = lazy(() => import('./CommandPalette'));
const ShortcutsOverlay = lazy(() => import('./ShortcutsOverlay'));

/** Route → page component. Traffic is eager (default page); others are code-split. */
const PAGES: Record<PageId, ComponentType | LazyExoticComponent<ComponentType>> = {
  traffic: TrafficPage,
  rules: RulesPage,
  compose: ComposePage,
  connect: ConnectPage,
  settings: SettingsPage,
  'kitchen-sink': KitchenSinkPage,
  'not-found': NotFoundPage,
};

const TITLES: Record<PageId, string> = {
  traffic: 'Traffic',
  rules: 'Rules',
  compose: 'Compose',
  connect: 'Connect',
  settings: 'Settings',
  'kitchen-sink': import.meta.env.DEV ? 'Components' : 'Not found',
  'not-found': 'Not found',
};

function PageFallback() {
  return (
    <div className={styles.fallback}>
      <Spinner size={18} tone="muted" label="Loading page" />
    </div>
  );
}

function PageOutlet() {
  const page = usePage();
  const Page = PAGES[page];
  useEffect(() => {
    document.title = `${TITLES[page]} · Akev`;
  }, [page]);
  return (
    <ErrorBoundary scope="page" resetKey={page}>
      <Suspense fallback={<PageFallback />}>
        <Page />
      </Suspense>
    </ErrorBoundary>
  );
}

/**
 * A lazily loaded overlay, mounted from the first time it opens and kept mounted afterwards
 * (so it can animate closed and reopen instantly).
 */
function LazyOverlay({ open, children }: { open: boolean; children: ReactNode }) {
  const [used, setUsed] = useState(open);
  useEffect(() => {
    if (open) setUsed(true);
  }, [open]);
  if (!open && !used) return null;
  return <Suspense fallback={null}>{children}</Suspense>;
}

function CommandPaletteHost() {
  const { open } = usePaletteState();
  return (
    <LazyOverlay open={open}>
      <CommandPalette />
    </LazyOverlay>
  );
}

function ShortcutsOverlayHost() {
  const open = useShortcutsOverlayOpen();
  return (
    <LazyOverlay open={open}>
      <ShortcutsOverlay />
    </LazyOverlay>
  );
}

/** The Sessions dialog, loaded the first time it opens. */
function SessionsDialogHost() {
  const { open } = useSessionsDialog();
  if (!open) return null;
  return (
    <Suspense fallback={null}>
      <SessionsDialog />
    </Suspense>
  );
}

function Shell() {
  useApplyTheme();
  useGlobalCommands();
  useDesktopIntegration();
  const startPage = useUiStore((s) => s.startPage);
  useEffect(() => setStartPage(startPage), [startPage]);

  return (
    <div className={styles.app}>
      <NavRail />
      <main className={styles.main} id="main">
        <PageOutlet />
        <ConnectionOverlay />
      </main>
      <StatusBar />
      <WindowControls />
      <CommandPaletteHost />
      <ShortcutsOverlayHost />
      <SessionsDialogHost />
      <UpgradeDialog />
      <ConfirmHost />
      <Toaster />
    </div>
  );
}

/** Root component: providers + shell, wrapped in the app-level crash screen. */
export function App() {
  return (
    <ErrorBoundary scope="app">
      <TooltipProvider>
        <Shell />
      </TooltipProvider>
    </ErrorBoundary>
  );
}
