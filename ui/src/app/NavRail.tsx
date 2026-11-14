import clsx from 'clsx';
import { CirclePause, Moon, Settings, Sun, SunMoon } from 'lucide-react';
import type { ReactNode } from 'react';
import { Tooltip } from '../components/ui';
import { useAppStore } from '../state/app';
import { THEMES, useUiStore } from '../state/ui';
import { useWsToolsStore } from '../state/wsTools';
import { Logo } from './Logo';
import { NAV_ITEMS } from './navItems';
import { href, navigate, usePage } from './router';
import { displayCombo, shortcutLabel } from './shortcuts';
import styles from './NavRail.module.css';

function RailLink({ path, label, shortcut, active, children }: { path: string; label: string; shortcut?: string; active: boolean; children: ReactNode }) {
  return (
    <Tooltip content={label} shortcut={shortcut} side="right">
      <a href={href(path)} className={clsx(styles.item, active && styles.active)} aria-label={label} aria-current={active ? 'page' : undefined}>
        {children}
      </a>
    </Tooltip>
  );
}

/** Left navigation rail: logo, pages, breakpoints badge, theme toggle, settings. */
export function NavRail() {
  const page = usePage();
  const paused = useAppStore((s) => s.paused);
  // WebSocket messages held at ws-breakpoints count too.
  const pausedMessages = useWsToolsStore((s) => s.paused);
  const pausedTotal = paused.length + pausedMessages.length;
  const firstPausedId = paused[0]?.id ?? pausedMessages[0]?.id;
  const status = useAppStore((s) => s.status);
  const theme = useUiStore((s) => s.theme);
  const cycleTheme = useUiStore((s) => s.cycleTheme);
  const ThemeIcon = theme === 'system' ? SunMoon : theme === 'light' ? Sun : Moon;
  const themeLabel = THEMES.find((t) => t.id === theme)?.label ?? theme;

  const logoTip = status
    ? `Akev ${status.version} · proxy :${status.proxy.port} · API ${status.apiUrl.replace(/^https?:\/\//, '')}`
    : 'Akev';

  return (
    <nav className={styles.rail} aria-label="Main" data-window-drag>
      <Tooltip content={logoTip} side="right">
        <a href={href('/traffic')} className={styles.logo} aria-label="Akev">
          <Logo size={28} />
        </a>
      </Tooltip>
      <div className={styles.items}>
        {NAV_ITEMS.map((item) => {
          const Icon = item.icon;
          return (
            <RailLink key={item.page} path={item.path} label={item.label} shortcut={item.shortcut} active={page === item.page}>
              <Icon size={18} strokeWidth={1.75} aria-hidden />
            </RailLink>
          );
        })}
      </div>
      <div className={styles.spacer} />
      <div className={styles.items}>
        {pausedTotal > 0 && (
          <Tooltip content={`${pausedTotal} paused at breakpoints — jump to the first`} side="right">
            <button type="button" className={clsx(styles.item, styles.breakpoints)} onClick={() => firstPausedId != null && navigate(`/traffic/${firstPausedId}`)} aria-label={`${pausedTotal} paused at breakpoints`}>
              <CirclePause size={18} strokeWidth={1.75} aria-hidden />
              <span className={clsx(styles.badge, 'tnum')}>{pausedTotal > 99 ? '99+' : pausedTotal}</span>
            </button>
          </Tooltip>
        )}
        <Tooltip content={`Theme: ${themeLabel} (click to cycle)`} side="right">
          <button type="button" className={styles.item} onClick={cycleTheme} aria-label={`Theme: ${themeLabel}`}>
            <ThemeIcon size={18} strokeWidth={1.75} aria-hidden />
          </button>
        </Tooltip>
        <RailLink path="/settings" label="Settings" shortcut="mod+," active={page === 'settings'}>
          <Settings size={18} strokeWidth={1.75} aria-hidden />
        </RailLink>
      </div>
      <span className="sr-only">Press {shortcutLabel(displayCombo('mod+k'))} for the command palette</span>
    </nav>
  );
}
