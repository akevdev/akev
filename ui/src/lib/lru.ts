/**
 * Minimal LRU cache on top of `Map` insertion order (O(1) get/set). Used for exchange details
 * and body views (DESIGN.md §9: LRU of 50).
 *
 * Optionally bounded by weight as well as count: with `maxWeight` and `weigh` (e.g. bytes of a
 * body), the oldest entries are evicted until both limits hold, and a single value heavier
 * than the whole budget is not cached at all.
 */
export interface LruOptions<V> {
  /** Upper bound for the summed `weigh(value)` of all entries. */
  maxWeight?: number;
  /** Weight of one value (e.g. its approximate size in bytes). */
  weigh?: (value: V) => number;
}

export class LruCache<K, V> {
  private readonly map = new Map<K, V>();
  private readonly weights = new Map<K, number>();
  private total = 0;
  private readonly maxWeight: number;
  private readonly weigh: ((value: V) => number) | null;

  constructor(
    private readonly capacity: number,
    options: LruOptions<V> = {},
  ) {
    if (capacity < 1) throw new RangeError('LruCache capacity must be >= 1');
    this.maxWeight = options.maxWeight ?? Infinity;
    this.weigh = options.weigh ?? null;
  }

  get size(): number {
    return this.map.size;
  }

  /** Summed weight of the cached values (0 without a `weigh` function). */
  get totalWeight(): number {
    return this.total;
  }

  has(key: K): boolean {
    return this.map.has(key);
  }

  /** Get a value and mark it most-recently used. */
  get(key: K): V | undefined {
    const value = this.map.get(key);
    if (value === undefined) return undefined;
    this.map.delete(key);
    this.map.set(key, value);
    return value;
  }

  /** Read without affecting recency. */
  peek(key: K): V | undefined {
    return this.map.get(key);
  }

  /**
   * Insert or replace a value (most-recently used), evicting the oldest entries beyond the
   * count or weight limit. A value heavier than `maxWeight` on its own is not stored (and
   * replaces nothing: any previous value for `key` is dropped).
   */
  set(key: K, value: V): this {
    this.delete(key);
    const w = this.weigh ? Math.max(0, this.weigh(value) || 0) : 0;
    if (w > this.maxWeight) return this;
    this.map.set(key, value);
    if (w) this.weights.set(key, w);
    this.total += w;
    while (this.map.size > this.capacity || this.total > this.maxWeight) {
      const oldest = this.map.keys().next();
      if (oldest.done) break;
      this.delete(oldest.value);
    }
    return this;
  }

  delete(key: K): boolean {
    if (!this.map.delete(key)) return false;
    const w = this.weights.get(key);
    if (w !== undefined) {
      this.total -= w;
      this.weights.delete(key);
    }
    return true;
  }

  /** Delete every entry whose key matches. */
  deleteWhere(predicate: (key: K) => boolean): void {
    for (const key of [...this.map.keys()]) if (predicate(key)) this.delete(key);
  }

  clear(): void {
    this.map.clear();
    this.weights.clear();
    this.total = 0;
  }
}
