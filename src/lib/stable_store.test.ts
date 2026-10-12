import { describe, expect, it } from 'vitest';
import { derived, get, writable } from 'svelte/store';
import { sameSet, stable } from './stable_store';

describe('stable (review r16)', () => {
  it('passes a value on only when it changed, and the current one to a new reader', () => {
    const src = writable([1, 2]);
    const asSet = stable(
      derived(src, (xs) => new Set(xs) as ReadonlySet<number>),
      sameSet,
    );
    let calls = 0;
    const stop = asSet.subscribe(() => calls++);
    expect(calls).toBe(1);
    src.set([2, 1]);
    expect(calls).toBe(1);
    src.set([1, 2, 3]);
    expect(calls).toBe(2);
    stop();
    src.set([4]);
    expect([...get(asSet)]).toEqual([4]);
  });

  it('sameSet compares members', () => {
    expect(sameSet(new Set([1]), new Set([1]))).toBe(true);
    expect(sameSet(new Set([1]), new Set([2]))).toBe(false);
    expect(sameSet(new Set([1]), new Set([1, 2]))).toBe(false);
  });
});
