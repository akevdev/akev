import { KeyRound, RefreshCw, Unplug } from 'lucide-react';
import { useEffect, useRef, useState, type RefObject } from 'react';
import { storedTokenWasStale } from '../api/auth';
import { api } from '../api/client';
import { Button, Spinner } from '../components/ui';
import { isDesktop } from '../lib/platform';
import { useAppStore } from '../state/app';
import { reconnectNow } from '../state/connection';
import { Logo } from './Logo';
import styles from './ConnectionOverlay.module.css';

function useCountdown(target: number | null): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (target == null) return;
    const t = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(t);
  }, [target]);
  return target == null ? null : Math.max(0, Math.ceil((target - now) / 1000));
}

const FOCUSABLE = 'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

/**
 * While `key` is set, keep keyboard focus inside the full-screen element (it covers the whole
 * app, so nothing behind it may take focus) and give focus back when it goes away.
 */
function useFocusTrap(ref: RefObject<HTMLElement | null>, key: string | null): void {
  useEffect(() => {
    const el = ref.current;
    if (!key || !el) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const first = () => el.querySelector<HTMLElement>(FOCUSABLE) ?? el;
    first().focus({ preventScroll: true });
    const onFocusIn = (e: FocusEvent) => {
      if (!el.contains(e.target as Node)) first().focus({ preventScroll: true });
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Tab') return;
      e.preventDefault();
      const items = [...el.querySelectorAll<HTMLElement>(FOCUSABLE)];
      if (items.length === 0) return;
      const i = items.indexOf(document.activeElement as HTMLElement);
      const next = e.shiftKey ? (i <= 0 ? items.length - 1 : i - 1) : i === items.length - 1 ? 0 : i + 1;
      items[next]?.focus();
    };
    document.addEventListener('focusin', onFocusIn);
    el.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('focusin', onFocusIn);
      el.removeEventListener('keydown', onKeyDown);
      if (previous?.isConnected) previous.focus({ preventScroll: true });
    };
  }, [ref, key]);
}

/**
 * Connection states:
 * - never connected → full-screen "Connecting…" splash (after 400 ms, with help after 4 s);
 * - lost connection → non-blocking banner over the page with a countdown + Retry;
 * - unauthorized → full-screen explanation (the token is invalid or expired).
 *
 * The full-screen states keep keyboard focus inside them while they show.
 */
export function ConnectionOverlay() {
  const connection = useAppStore((s) => s.connection);
  const hasConnected = useAppStore((s) => s.hasConnected);
  const failedAttempts = useAppStore((s) => s.failedAttempts);
  const retryAt = useAppStore((s) => s.retryAt);
  const seconds = useCountdown(connection === 'open' ? null : retryAt);
  const [showSplash, setShowSplash] = useState(false);
  const [showHelp, setShowHelp] = useState(false);
  const desktop = isDesktop();
  const screenRef = useRef<HTMLDivElement | null>(null);
  const fullScreen = connection === 'unauthorized' ? 'unauthorized' : !hasConnected && connection !== 'open' && showSplash ? 'splash' : null;
  useFocusTrap(screenRef, fullScreen);

  useEffect(() => {
    if (hasConnected || connection === 'open') return;
    const a = setTimeout(() => setShowSplash(true), 400);
    const b = setTimeout(() => setShowHelp(true), 4000);
    return () => {
      clearTimeout(a);
      clearTimeout(b);
    };
  }, [hasConnected, connection]);

  if (connection === 'unauthorized') {
    return (
      <div ref={screenRef} className={styles.screen} role="alertdialog" aria-modal="true" aria-labelledby="conn-title" aria-describedby="conn-text" tabIndex={-1}>
        <div className={styles.card}>
          <div className={styles.iconCircle}>
            <KeyRound size={22} strokeWidth={1.6} aria-hidden />
          </div>
          <h1 id="conn-title" className={styles.title}>
            This session isn't authorized
          </h1>
          {desktop ? (
            <p id="conn-text" className={styles.text}>
              Akev rejected this window's session token. Quit Akev and open it again to start a new session.
            </p>
          ) : (
            <p id="conn-text" className={styles.text}>
              {storedTokenWasStale()
                ? 'Akev has restarted since this browser last connected, so the session token it remembered no longer works.'
                : api.getConfig().token
                  ? 'Akev rejected the session token.'
                  : 'This browser has no session token for Akev yet.'}{' '}
              Open the UI again from the link printed by <code>akev start</code> or <code>akev status</code> (it contains a fresh <code>?token=</code>); this browser
              remembers it after that.
            </p>
          )}
          <Button variant="secondary" icon={RefreshCw} onClick={reconnectNow}>
            Try again
          </Button>
        </div>
      </div>
    );
  }

  if (!hasConnected && connection !== 'open') {
    if (!showSplash) return null;
    return (
      <div ref={screenRef} className={styles.screen} role="status" aria-live="polite" tabIndex={-1}>
        <div className={styles.card}>
          <div className={styles.logoWrap}>
            <Logo size={44} />
          </div>
          <div className={styles.row}>
            <Spinner size={14} tone="muted" />
            <span className={styles.title}>Connecting to Akev…</span>
          </div>
          {showHelp && (
            <div className={styles.help}>
              <p className={styles.text}>
                {failedAttempts > 0 ? `Still trying${seconds != null && seconds > 0 ? ` — next attempt in ${seconds}s` : ''}. ` : ''}
                {desktop ? (
                  "Akev's background service is starting. If this doesn't go away, quit Akev and open it again."
                ) : (
                  <>
                    Make sure Akev is running: start it with <code>akev start</code>, or open the Akev desktop app.
                  </>
                )}
              </p>
              <div className={styles.actions}>
                <Button variant="secondary" size="sm" icon={RefreshCw} onClick={reconnectNow}>
                  Retry now
                </Button>
              </div>
            </div>
          )}
        </div>
      </div>
    );
  }

  if (connection === 'reconnecting') {
    return (
      <div className={styles.banner} role="status" aria-live="polite">
        <Unplug size={14} strokeWidth={1.75} aria-hidden />
        <span>
          Lost connection to Akev.
          {seconds != null && seconds > 0 ? ` Reconnecting in ${seconds}s…` : ' Reconnecting…'}
        </span>
        <button type="button" className={styles.bannerAction} onClick={reconnectNow}>
          Retry now
        </button>
      </div>
    );
  }

  return null;
}
