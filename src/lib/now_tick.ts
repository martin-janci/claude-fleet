// A shared wall clock in unix seconds that ticks every 30 s while anything
// reads it: a view that derives from `now` (a usage reading's reset, its
// age) reads this store so it re-derives on time, not only when an event
// happens to arrive. One interval for every subscriber.
//
// Paired with a hub, the timestamps a view compares against are the hub's,
// so the clock is too: this machine's plus the difference the hub's `ready`
// frame showed (`hubClockSkew`, review w15-3). A desktop six minutes ahead
// otherwise read every usage figure as stale for good.
import { derived, readable } from 'svelte/store';
import { hubClockSkew } from './hub_connection';

export const NOW_TICK_MS = 30_000;

const localTick = readable(Math.floor(Date.now() / 1000), (set) => {
  set(Math.floor(Date.now() / 1000));
  const t = setInterval(() => set(Math.floor(Date.now() / 1000)), NOW_TICK_MS);
  return () => clearInterval(t);
});

export const nowTick = derived([localTick, hubClockSkew], ([$now, $skew]) => $now + $skew);
