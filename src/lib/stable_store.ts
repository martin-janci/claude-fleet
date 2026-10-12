import { readable, type Readable } from 'svelte/store';

// A store that passes `src` on only when `equal` says the value changed.
// A svelte/store `derived` treats every object as changed, so one that
// rebuilds an equal Set or object on each row event re-renders every reader
// on every flush (review r16). The gate resets with the subscription, so a
// reader that comes back always gets the current value.
export function stable<T>(src: Readable<T>, equal: (a: T, b: T) => boolean): Readable<T> {
  return readable<T>(undefined as T, (set) => {
    let first = true;
    let last: T;
    return src.subscribe((v) => {
      if (first || !equal(last, v)) {
        first = false;
        last = v;
        set(v);
      }
    });
  });
}

/** Same members, for {@link stable}. */
export function sameSet<T>(a: ReadonlySet<T>, b: ReadonlySet<T>): boolean {
  if (a === b) return true;
  if (a.size !== b.size) return false;
  for (const v of a) if (!b.has(v)) return false;
  return true;
}
