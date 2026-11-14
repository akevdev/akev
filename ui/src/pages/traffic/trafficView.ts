/**
 * The Traffic page's view controller: a tiny module-level registry connecting the toolbar,
 * the list and page-level shortcuts without re-rendering the page on every traffic batch.
 *
 * - The list publishes its current display list (filtered, sorted, minus rows hidden for
 *   undo) and an imperative handle (scroll, focus).
 * - The filter bar publishes a focus handle.
 */

import type { ExchangeId } from '../../api/types';
import { trafficStore } from '../../state/traffic';

export interface ListHandle {
  /** Scroll a row into view (`center` for deep links). */
  scrollToId(id: ExchangeId, align?: 'auto' | 'center'): void;
  /**
   * The user moved the selection to `id`: scroll it into view and stop following live
   * traffic unless it is the newest row (so the row they look at doesn't drift away).
   */
  reveal(id: ExchangeId): void;
  focus(): void;
}

export interface FilterHandle {
  focus(selectAll?: boolean): void;
}

let list: readonly ExchangeId[] = [];
let natural = true;
let listHandle: ListHandle | null = null;
let filterHandle: FilterHandle | null = null;
let detailElement: HTMLElement | null = null;

export const trafficView = {
  /** Called by the list whenever its display list changes. */
  setList(next: readonly ExchangeId[], isNatural: boolean): void {
    list = next;
    natural = isNatural;
  },
  /** The rows as displayed (top to bottom). */
  getList(): readonly ExchangeId[] {
    return list;
  },
  /**
   * The `list` argument for the selection model: `undefined` when the display list is the
   * store's own `visible` order (binary search), else the displayed array.
   */
  listArg(): readonly ExchangeId[] | undefined {
    return natural ? undefined : list;
  },
  registerList(handle: ListHandle | null): void {
    listHandle = handle;
  },
  registerFilter(handle: FilterHandle | null): void {
    filterHandle = handle;
  },
  registerDetail(el: HTMLElement | null): void {
    detailElement = el;
  },
  scrollToId(id: ExchangeId, align: 'auto' | 'center' = 'auto'): void {
    listHandle?.scrollToId(id, align);
  },
  focusList(): void {
    listHandle?.focus();
  },
  focusFilter(selectAll = true): void {
    filterHandle?.focus(selectAll);
  },
  /** Move focus into the detail pane (first focusable control, else the pane itself). */
  focusDetail(): void {
    const el = detailElement;
    if (!el) return;
    const target = el.querySelector<HTMLElement>('[role="tab"][aria-selected="true"], button:not([disabled]), [tabindex="0"], a[href], input, textarea');
    (target ?? el).focus();
  },
  /** Keyboard navigation through the displayed rows; scrolls the new active row into view. */
  move(delta: number, extend = false): void {
    const id = trafficStore.move(delta, extend, trafficView.listArg());
    if (id != null) listHandle?.reveal(id);
  },
  /** See {@link ListHandle.reveal}. */
  reveal(id: ExchangeId): void {
    listHandle?.reveal(id);
  },
};
