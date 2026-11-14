/** Small seeded PRNG (mulberry32) so mock data is stable across reloads. */
export class Rng {
  private state: number;

  constructor(seed: number) {
    this.state = seed >>> 0 || 0x9e3779b9;
  }

  /** Float in [0, 1). */
  next(): number {
    this.state = (this.state + 0x6d2b79f5) >>> 0;
    let t = this.state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  }

  /** Integer in [min, max] (inclusive). */
  int(min: number, max: number): number {
    return Math.floor(this.next() * (max - min + 1)) + min;
  }

  /** Float in [min, max). */
  range(min: number, max: number): number {
    return min + this.next() * (max - min);
  }

  chance(p: number): boolean {
    return this.next() < p;
  }

  pick<T>(items: readonly T[]): T {
    const v = items[Math.floor(this.next() * items.length)];
    if (v === undefined) throw new RangeError('pick() from an empty list');
    return v;
  }

  /** Weighted pick: `items[i]` chosen with probability `weight(items[i]) / Σ`. */
  weighted<T>(items: readonly T[], weight: (item: T) => number): T {
    const total = items.reduce((a, i) => a + weight(i), 0);
    let r = this.next() * total;
    for (const item of items) {
      r -= weight(item);
      if (r < 0) return item;
    }
    return this.pick(items);
  }

  /** Derive an independent generator (e.g. per exchange, for lazily generated bodies). */
  fork(salt: number): Rng {
    return new Rng((this.state ^ Math.imul(salt + 1, 0x85ebca6b)) >>> 0);
  }
}
