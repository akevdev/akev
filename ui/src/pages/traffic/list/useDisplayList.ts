import { useEffect, useMemo, useRef, useState } from 'react';
import type { ExchangeId, RuleItem, RuleSet } from '../../../api/types';
import { useAppStore } from '../../../state/app';
import { trafficStore, useTraffic } from '../../../state/traffic';
import { useUiStore } from '../../../state/ui';
import { filterHidden, useHidden } from '../actions/hiddenRows';
import { isNaturalOrder } from './columns';
import { IncrementalSort, sortDependsOnUpdates } from './sortIds';

/** Above this many rows, live re-sorting is throttled (each pass is an O(n) identity scan). */
const THROTTLE_ROWS = 20_000;
const THROTTLE_MS = 250;

const titleCache = new WeakMap<RuleSet, Map<string, string>>();

/** Rule id → display title (explicit title, else the action type). */
export function ruleTitles(rules: RuleSet | null): Map<string, string> {
  if (!rules) return new Map();
  let map = titleCache.get(rules);
  if (map) return map;
  map = new Map();
  const walk = (items: readonly RuleItem[]) => {
    for (const item of items) {
      if (item.type === 'group') walk(item.items);
      else map!.set(item.id, item.title?.trim() || ruleActionLabel(item.action.type));
    }
  };
  walk(rules.items);
  titleCache.set(rules, map);
  return map;
}

function ruleActionLabel(type: string): string {
  switch (type) {
    case 'mock':
      return 'Mock';
    case 'map-local':
      return 'Map Local';
    case 'breakpoint':
      return 'Breakpoint';
    case 'transform':
      return 'Rewrite';
    case 'forward':
      return 'Forward';
    case 'close':
      return 'Close';
    case 'reset':
      return 'Reset';
    case 'timeout':
      return 'Timeout';
    case 'delay':
      return 'Delay';
    case 'throttle':
      return 'Throttle';
    case 'highlight':
      return 'Highlight';
    case 'script':
      return 'Script';
    default:
      return type.charAt(0).toUpperCase() + type.slice(1);
  }
}

export interface DisplayList {
  /** Ids as displayed, top to bottom. */
  list: readonly ExchangeId[];
  /** `list` is the store's own `visible` array order (ascending id, nothing hidden). */
  natural: boolean;
}

/**
 * The traffic list's rows: the store's visible ids, sorted by the chosen column (client side)
 * and without rows hidden for undo. Sorted views are maintained incrementally (new/changed
 * rows are binary-inserted); summary updates are only watched when the sort key can change
 * (status, size, time…), throttled for very large lists.
 */
export function useDisplayList(): DisplayList {
  const visible = useTraffic((s) => s.visible);
  const sort = useUiStore((s) => s.sort);
  const hidden = useHidden();
  const natural = isNaturalOrder(sort);
  const live = !natural && sort != null && sortDependsOnUpdates(sort.column);
  const version = useTraffic((s) => (live ? s.version : 0));
  const rules = useAppStore((s) => (sort?.column === 'rule' ? s.rules : null));

  // Throttle the sort input for huge lists so a burst of updates doesn't re-sort every frame.
  const throttle = !natural && visible.length > THROTTLE_ROWS;
  const raw = useMemo(() => ({ visible, version }), [visible, version]);
  const input = useThrottled(raw, THROTTLE_MS, throttle);
  // Natural order depends only on `visible`; sorted views on the (throttled) input, whose
  // `version` invalidates live sorts.
  const source: unknown = natural ? visible : input;

  const sorter = useRef<IncrementalSort | null>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: recomputed only when the source, sort or rules change
  const sorted = useMemo(() => {
    if (natural || !sort) {
      sorter.current?.reset();
      return visible;
    }
    sorter.current ??= new IncrementalSort();
    const titles = ruleTitles(rules);
    return sorter.current.update(input.visible, trafficStore.getSnapshot().summaries, sort, (id) => titles.get(id), [rules]);
  }, [source, sort, rules]);

  const list = useMemo(() => filterHidden(sorted, hidden.entries, (id) => trafficStore.get(id)), [sorted, hidden]);
  return { list, natural: natural && list === visible };
}

/** Leading + trailing throttle of a value (pass-through when disabled). */
function useThrottled<T>(value: T, ms: number, enabled: boolean): T {
  const [state, setState] = useState(value);
  const last = useRef(0);
  const latest = useRef(value);
  latest.current = value;
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!enabled) return;
    const now = Date.now();
    const wait = ms - (now - last.current);
    if (wait <= 0) {
      last.current = now;
      setState(value);
    } else if (timer.current == null) {
      timer.current = setTimeout(() => {
        timer.current = null;
        last.current = Date.now();
        setState(latest.current);
      }, wait);
    }
  }, [value, ms, enabled]);

  useEffect(
    () => () => {
      if (timer.current != null) clearTimeout(timer.current);
    },
    [],
  );

  return enabled ? state : value;
}
