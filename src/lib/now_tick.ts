// A shared wall clock in unix seconds that ticks every 30 s while anything
// reads it: a view that derives from `now` (a usage reading's reset, its
// age) reads this store so it re-derives on time, not only when an event
// happens to arrive. One interval for every subscriber.
import { readable } from 'svelte/store';

export const NOW_TICK_MS = 30_000;

export const nowTick = readable(Math.floor(Date.now() / 1000), (set) => {
  set(Math.floor(Date.now() / 1000));
  const t = setInterval(() => set(Math.floor(Date.now() / 1000)), NOW_TICK_MS);
  return () => clearInterval(t);
});
