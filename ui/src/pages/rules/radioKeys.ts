import type { KeyboardEvent } from 'react';

/**
 * Arrow-key handling for a `role="radiogroup"` made of `role="radio"` buttons (WAI-ARIA
 * radio pattern): ←/↑ and →/↓ move to the previous/next option *and select it*, Home/End
 * jump to the ends. Attach to the radiogroup element's `onKeyDown`.
 */
export function onRadioGroupKeyDown(e: KeyboardEvent<HTMLElement>): void {
  const radios = Array.from(e.currentTarget.querySelectorAll<HTMLElement>('[role="radio"]:not([disabled])'));
  const i = radios.indexOf(document.activeElement as HTMLElement);
  if (i < 0) return;
  let next = -1;
  if (e.key === 'ArrowRight' || e.key === 'ArrowDown') next = (i + 1) % radios.length;
  else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') next = (i - 1 + radios.length) % radios.length;
  else if (e.key === 'Home') next = 0;
  else if (e.key === 'End') next = radios.length - 1;
  if (next < 0) return;
  e.preventDefault();
  e.stopPropagation();
  const el = radios[next];
  el?.focus();
  el?.click();
}

/** Roving tabindex for radio buttons: only the checked one (or the first) is tabbable. */
export function radioTabIndex(checked: boolean, isFirst: boolean, anyChecked: boolean): 0 | -1 {
  return checked || (!anyChecked && isFirst) ? 0 : -1;
}
