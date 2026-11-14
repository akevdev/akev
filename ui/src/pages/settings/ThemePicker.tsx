import clsx from 'clsx';
import { Check } from 'lucide-react';
import { useRef, type KeyboardEvent, type ReactNode } from 'react';
import { THEMES, type ResolvedTheme, type ThemePref } from '../../state/ui';
import styles from './ThemePicker.module.css';

const ROWS = ['get', 'post', 'get', 'delete', 'put'] as const;

/** A miniature Akev window rendered with the given theme's tokens. */
function Mini() {
  return (
    <div className={styles.mini} aria-hidden>
      <div className={styles.nav}>
        <span className={styles.logo} />
        <span className={styles.navItem} data-active="" />
        <span className={styles.navItem} />
        <span className={styles.navItem} />
      </div>
      <div className={styles.main}>
        <div className={styles.toolbar}>
          <span className={styles.search} />
          <span className={styles.chip} />
        </div>
        <div className={styles.body}>
          <div className={styles.list}>
            {ROWS.map((method, i) => (
              <span key={i} className={styles.row} data-selected={i === 1 || undefined}>
                <span className={styles.method} data-method={method} />
                <span className={styles.line} />
              </span>
            ))}
          </div>
          <div className={styles.detail}>
            <span className={styles.line} />
            <span className={styles.lineMuted} />
            <span className={styles.lineMuted} />
            <span className={styles.accentBar} />
          </div>
        </div>
      </div>
    </div>
  );
}

/** Scope tokens to a theme. Midnight/contrast only override parts of dark, so nest them in dark. */
function Themed({ theme, children }: { theme: ResolvedTheme; children: ReactNode }) {
  if (theme === 'midnight' || theme === 'contrast') {
    return (
      <div data-theme="dark" className={styles.scope}>
        <div data-theme={theme} className={styles.scope}>
          {children}
        </div>
      </div>
    );
  }
  return (
    <div data-theme={theme} className={styles.scope}>
      {children}
    </div>
  );
}

function Preview({ theme }: { theme: ThemePref }) {
  if (theme === 'system') {
    return (
      <div className={styles.split}>
        <div className={styles.half}>
          <Themed theme="dark">
            <Mini />
          </Themed>
        </div>
        <div className={clsx(styles.half, styles.right)}>
          <Themed theme="light">
            <Mini />
          </Themed>
        </div>
      </div>
    );
  }
  return (
    <Themed theme={theme}>
      <Mini />
    </Themed>
  );
}

export interface ThemePickerProps {
  value: ThemePref;
  onChange: (theme: ThemePref) => void;
  /** Theme currently applied (for the "System → Dark" hint). */
  resolved: ResolvedTheme;
}

/** Theme cards with live mini previews (radio group, arrow keys move). */
export function ThemePicker({ value, onChange, resolved }: ThemePickerProps) {
  const ref = useRef<HTMLDivElement | null>(null);
  const onKeyDown = (e: KeyboardEvent) => {
    const keys = ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'];
    if (!keys.includes(e.key)) return;
    e.preventDefault();
    const i = THEMES.findIndex((t) => t.id === value);
    const dir = e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 1;
    const next = THEMES[(i + dir + THEMES.length) % THEMES.length];
    if (!next) return;
    onChange(next.id);
    ref.current?.querySelector<HTMLButtonElement>(`[data-theme-id="${next.id}"]`)?.focus();
  };
  return (
    <div className={styles.grid} role="radiogroup" aria-label="Theme" ref={ref} onKeyDown={onKeyDown}>
      {THEMES.map((t) => {
        const selected = t.id === value;
        return (
          <button
            key={t.id}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            data-theme-id={t.id}
            className={clsx(styles.card, selected && styles.selected)}
            onClick={() => onChange(t.id)}
          >
            <span className={styles.previewFrame}>
              <Preview theme={t.id} />
            </span>
            <span className={styles.label}>
              <span className={styles.radio} aria-hidden>
                {selected && <Check size={10} strokeWidth={3} />}
              </span>
              <span className={styles.name}>{t.label}</span>
              {t.id === 'system' && <span className={styles.hint}>{resolved === 'light' ? 'Light' : 'Dark'}</span>}
            </span>
          </button>
        );
      })}
    </div>
  );
}
