import clsx from 'clsx';
import { ChevronsUpDown } from 'lucide-react';
import { useRef, useState, type KeyboardEvent } from 'react';
import type { ActionType } from '../../../api/types';
import { ProBadge } from '../../../components/pro';
import { Popover, PopoverContent, PopoverTrigger } from '../../../components/ui';
import { actionFeature } from '../../../lib/license';
import { ACTION_CATEGORIES, ACTIONS, actionInfo } from '../../../lib/rules/actions';
import { iconFor } from '../icons';
import styles from './Editor.module.css';

/** Approximate height of the open card grid (4 sections). */
const PICKER_HEIGHT = 500;

/**
 * "Then" action selector: a large trigger showing the current action (icon, name, what it
 * does, whether it stops evaluation) that opens a grid of icon cards grouped by category.
 * Arrow keys move through the grid; Enter picks.
 */
export function ActionPicker({ value, onChange }: { value: ActionType; onChange: (type: ActionType) => void }) {
  const [open, setOpen] = useState(false);
  const info = actionInfo(value);
  const Icon = iconFor(info.icon);
  const feature = actionFeature(value);
  const gridRef = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);

  // The card grid is tall: if it can't fit below the trigger, scroll the editor so the
  // trigger sits near the top first (the popover then opens below it, fully visible).
  const makeRoom = () => {
    const el = triggerRef.current;
    const scroller = el?.closest<HTMLElement>('[data-editor-scroll]');
    if (!el || !scroller) return;
    // Temporary scroll-past-end space, so even short forms can scroll the trigger up.
    scroller.dataset.pickerOpen = '';
    const t = el.getBoundingClientRect();
    const box = scroller.getBoundingClientRect();
    const spaceBelow = box.bottom - t.bottom;
    if (spaceBelow >= PICKER_HEIGHT) return;
    scroller.scrollTop += Math.min(t.top - box.top - 12, PICKER_HEIGHT - spaceBelow + 12);
  };

  const onGridKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const buttons = Array.from(gridRef.current?.querySelectorAll<HTMLButtonElement>('button[data-action]') ?? []);
    const i = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (i < 0) return;
    let next = -1;
    if (e.key === 'ArrowRight') next = Math.min(buttons.length - 1, i + 1);
    else if (e.key === 'ArrowLeft') next = Math.max(0, i - 1);
    else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') next = verticalNeighbour(buttons, i, e.key === 'ArrowDown' ? 1 : -1);
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = buttons.length - 1;
    if (next >= 0) {
      e.preventDefault();
      buttons[next]?.focus();
    }
  };

  return (
    <Popover
      open={open}
      onOpenChange={(o) => {
        // Scroll before the content mounts, so it is positioned against the final layout.
        if (o) makeRoom();
        else delete triggerRef.current?.closest<HTMLElement>('[data-editor-scroll]')?.dataset.pickerOpen;
        setOpen(o);
      }}
    >
      <PopoverTrigger asChild>
        <button
          ref={triggerRef}
          type="button"
          className={styles.actionTrigger}
          aria-label={`Action: ${info.label}. Change action`}
        >
          <span className={styles.actionTriggerIcon} data-category={info.category} aria-hidden>
            <Icon size={18} strokeWidth={1.8} />
          </span>
          <span className={styles.actionTriggerText}>
            <span className={styles.actionTriggerTitle}>
              {info.label}
              <span className={clsx(styles.flowBadge, info.terminal ? styles.flowStop : styles.flowContinue)}>{info.terminal ? 'Stops evaluation' : 'Continues to next rules'}</span>
              {feature && <ProBadge feature={feature} />}
            </span>
            <span className={styles.actionTriggerDesc}>{info.description}</span>
          </span>
          <ChevronsUpDown size={15} className={styles.actionTriggerChevron} aria-hidden />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        side="bottom"
        flush
        className={styles.actionPopover}
        onOpenAutoFocus={(e) => {
          e.preventDefault();
          requestAnimationFrame(() => gridRef.current?.querySelector<HTMLButtonElement>('button[aria-pressed="true"]')?.focus());
        }}
      >
        <div ref={gridRef} onKeyDown={onGridKey} role="listbox" aria-label="Actions">
          {ACTION_CATEGORIES.map((cat) => (
            <div key={cat.id} className={styles.actionGroup}>
              <div className={styles.actionGroupHead}>
                <span className={styles.actionGroupLabel}>{cat.label}</span>
                <span className={styles.actionGroupHint}>{cat.hint}</span>
              </div>
              <div className={styles.actionGrid}>
                {ACTIONS.filter((a) => a.category === cat.id).map((a) => {
                  const AIcon = iconFor(a.icon);
                  const selected = a.type === value;
                  const pro = actionFeature(a.type);
                  return (
                    <button
                      key={a.type}
                      type="button"
                      role="option"
                      aria-selected={selected}
                      data-action={a.type}
                      className={clsx(styles.actionCard, selected && styles.actionCardOn)}
                      onClick={() => {
                        onChange(a.type);
                        setOpen(false);
                      }}
                    >
                      <span className={styles.actionCardIcon} data-category={a.category} aria-hidden>
                        <AIcon size={15} strokeWidth={1.8} />
                      </span>
                      <span className={styles.actionCardText}>
                        <span className={styles.actionCardTitle}>
                          {a.label}
                          {pro && <ProBadge feature={pro} className={styles.actionCardPro} />}
                        </span>
                        <span className={styles.actionCardDesc}>{a.description}</span>
                      </span>
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}

/**
 * The card in the next (or previous) visual row closest horizontally — the grid has a
 * responsive column count and rows of different lengths, so index arithmetic won't do.
 */
function verticalNeighbour(buttons: readonly HTMLElement[], i: number, dir: 1 | -1): number {
  const rects = buttons.map((b) => b.getBoundingClientRect());
  const cur = rects[i];
  if (!cur) return -1;
  const cx = cur.left + cur.width / 2;
  const candidates = rects.map((r, j) => ({ r, j })).filter(({ r }) => (dir > 0 ? r.top >= cur.bottom - 1 : r.bottom <= cur.top + 1));
  if (candidates.length === 0) return i;
  const rowTop = dir > 0 ? Math.min(...candidates.map((c) => c.r.top)) : Math.max(...candidates.map((c) => c.r.top));
  const row = candidates.filter((c) => Math.abs(c.r.top - rowTop) < 2);
  row.sort((a, b) => Math.abs(a.r.left + a.r.width / 2 - cx) - Math.abs(b.r.left + b.r.width / 2 - cx));
  return row[0]?.j ?? i;
}
