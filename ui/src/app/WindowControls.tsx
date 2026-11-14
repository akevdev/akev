import { Copy, Minus, Square, X } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { desktopWindow, useTitleBarSync, useWindowButtons, useWindowDrag, type WindowButton } from '../lib/windowChrome';
import styles from './WindowControls.module.css';

const LABELS: Record<WindowButton, string> = { minimize: 'Minimize', maximize: 'Maximize', close: 'Close' };

/**
 * Minimise / maximise / close at the right end of the top toolbar when the desktop window has
 * no system title bar (Linux and Windows; see `lib/windowChrome.ts`). Publishes its width as
 * `--window-controls-right` so title-bar toolbars keep their content clear of it.
 */
export function WindowControls() {
  useTitleBarSync();
  const buttons = useWindowButtons();
  useWindowDrag(buttons != null);
  const ref = useRef<HTMLDivElement | null>(null);
  const [maximized, setMaximized] = useState(false);
  const shown = buttons != null && buttons.length > 0;

  useLayoutEffect(() => {
    const root = document.documentElement;
    const el = ref.current;
    if (!shown || !el) {
      root.style.removeProperty('--window-controls-right');
      return;
    }
    const publish = () => root.style.setProperty('--window-controls-right', `${el.offsetWidth}px`);
    publish();
    const ro = new ResizeObserver(publish);
    ro.observe(el);
    return () => {
      ro.disconnect();
      root.style.removeProperty('--window-controls-right');
    };
  }, [shown]);

  useEffect(() => {
    if (!shown || !buttons.includes('maximize')) return;
    const win = desktopWindow();
    if (!win) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const refresh = () => void win.isMaximized().then((m) => !disposed && setMaximized(m), () => {});
    refresh();
    void win.onResized(refresh).then(
      (u) => (disposed ? u() : (unlisten = u)),
      () => {},
    );
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [shown, buttons]);

  if (!shown) return null;

  const act = (b: WindowButton) => {
    const win = desktopWindow();
    if (!win) return;
    const run = b === 'minimize' ? win.minimize() : b === 'maximize' ? win.toggleMaximize() : win.close();
    void run.catch((e: unknown) => console.warn(`[akev] window ${b} failed`, e));
  };

  return (
    <div ref={ref} className={styles.controls} data-window-drag>
      {buttons.map((b) => {
        const label = b === 'maximize' && maximized ? 'Restore' : LABELS[b];
        const Icon = b === 'minimize' ? Minus : b === 'close' ? X : maximized ? Copy : Square;
        return (
          <button key={b} type="button" className={styles.button} data-button={b} aria-label={label} title={label} onClick={() => act(b)}>
            <Icon size={b === 'close' ? 15 : 13} strokeWidth={b === 'close' ? 2 : 2.2} aria-hidden className={b === 'maximize' && maximized ? styles.restore : undefined} />
          </button>
        );
      })}
    </div>
  );
}
