import { useEffect, useRef, useState, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import styles from './TrafficList.module.css';

interface Tip {
  text: string;
  x: number;
  y: number;
  below: boolean;
}

const DELAY_MS = 450;
/** After a tooltip was visible, moving to another cell shows the next one immediately. */
const SKIP_MS = 300;

/**
 * One delegated tooltip for the whole grid: cells declare `data-tip="…"` (and
 * `data-tip-overflow` to show it only when the text is truncated). No per-row tooltip
 * components, so rows stay cheap to render while scrolling.
 */
export function GridTooltip({ containerRef }: { containerRef: RefObject<HTMLElement | null> }) {
  const [tip, setTip] = useState<Tip | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const current = useRef<HTMLElement | null>(null);
  const lastHidden = useRef(0);
  const visible = useRef(false);

  useEffect(() => {
    const root = containerRef.current;
    if (!root) return;

    const clearTimer = () => {
      if (timer.current != null) clearTimeout(timer.current);
      timer.current = null;
    };
    const hide = () => {
      clearTimer();
      current.current = null;
      if (visible.current) lastHidden.current = Date.now();
      visible.current = false;
      setTip(null);
    };
    const show = (el: HTMLElement) => {
      if (!el.isConnected || current.current !== el) return;
      const text = el.dataset.tip;
      if (!text) return;
      if (el.hasAttribute('data-tip-overflow') && el.scrollWidth <= el.clientWidth + 1) return;
      const r = el.getBoundingClientRect();
      const below = r.top < 64;
      visible.current = true;
      setTip({ text, x: Math.max(8, Math.min(r.left - 8, window.innerWidth - 328)), y: below ? r.bottom + 6 : r.top - 6, below });
    };

    const onOver = (e: PointerEvent) => {
      if (e.pointerType === 'touch' || e.buttons !== 0) return;
      const el = (e.target as Element | null)?.closest<HTMLElement>('[data-tip]') ?? null;
      if (el === current.current) return;
      const wasVisible = visible.current;
      hide();
      if (!el || !root.contains(el)) return;
      current.current = el;
      const quick = wasVisible || Date.now() - lastHidden.current < SKIP_MS;
      timer.current = setTimeout(() => show(el), quick ? 0 : DELAY_MS);
    };

    root.addEventListener('pointerover', onOver);
    root.addEventListener('pointerleave', hide);
    root.addEventListener('pointerdown', hide, true);
    root.addEventListener('wheel', hide, { passive: true });
    root.addEventListener('scroll', hide, { passive: true });
    window.addEventListener('keydown', hide, true);
    return () => {
      hide();
      root.removeEventListener('pointerover', onOver);
      root.removeEventListener('pointerleave', hide);
      root.removeEventListener('pointerdown', hide, true);
      root.removeEventListener('wheel', hide);
      root.removeEventListener('scroll', hide);
      window.removeEventListener('keydown', hide, true);
    };
  }, [containerRef]);

  if (!tip) return null;
  return createPortal(
    <div role="tooltip" className={styles.gridTip} data-below={tip.below || undefined} style={{ left: tip.x, top: tip.y }}>
      {tip.text}
    </div>,
    document.body,
  );
}
