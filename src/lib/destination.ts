// Where the right column is (Orbit Fleet redesign step 3.1). One value
// replaces the overlay flags App.svelte used to keep side by side
// (`filesMode`, `hostsMode`, `assetsMode` and the `workBoardOpen` store),
// so two overlays can never be open at once and every way out of one is the
// same assignment.
//
// `session` is the Session tab (Conversation or Terminal, by the stored
// `sessionView` pref). Every other value is an opaque overlay over it: the
// TerminalView underneath stays mounted whatever the destination, so a PTY
// survives any round trip.
//
// The rail (step 3.2, `AppRail.svelte`) writes this store. `accounts` (step
// 4.1) is reachable from the rail. `details` (step 3.5) is the Details tab:
// the session's details in the right column, in place of the inspector
// beside it. `control` (step 9.1) is Control: the fleet agent and Today
// (`control.ts`). `automation` (step 8.4) is Automation: routines, runs and
// the built-in agents (`automation.ts`).
import { derived, writable, type Readable } from 'svelte/store';
import { readPref, writePref } from './prefs';

export type Destination =
  | 'session'
  | 'files'
  | 'hosts'
  | 'assets'
  | 'board'
  | 'accounts'
  | 'details'
  | 'control'
  | 'automation';

export const DESTINATIONS: readonly Destination[] = [
  'session',
  'files',
  'hosts',
  'assets',
  'board',
  'accounts',
  'details',
  'control',
  'automation',
];

export const destination = writable<Destination>('session');

// Step 3.15's warm start shows the last screen at once, so the destination is
// kept (localStorage through `prefs`, which swallows a blocked store). What
// the LAST run left is read once, here, before anything in this run writes:
// App.svelte sets `session` as it mounts, and only a warm start
// (`startup.ts`'s `trackActivity`) puts the stored one back.
const LAST_KEY = 'nav.destination';
const isDestination = (v: unknown): v is Destination => DESTINATIONS.includes(v as Destination);
let lastRun: Destination = readPref<Destination>(LAST_KEY, 'session', isDestination);
destination.subscribe((d) => writePref(LAST_KEY, d));

/** Where the last run was when it was last in use. */
export function lastDestination(): Destination {
  return lastRun;
}

/** A warm start: back to the screen the last run left. */
export function restoreLastDestination(): void {
  destination.set(lastRun);
}

/** For tests: read the stored destination again, as a new run would. */
export function rereadLastDestination(): void {
  lastRun = readPref<Destination>(LAST_KEY, 'session', isDestination);
}

/** Go to `d`. Going to the Session tab is how any overlay closes. */
export function goTo(d: Destination): void {
  destination.set(d);
}

/** Leave `d` for the Session tab, only if `d` is where we are. */
export function leave(d: Destination): void {
  destination.update((cur) => (cur === d ? 'session' : cur));
}

/** A boolean view of one destination that can also be written: `true` goes
 *  there, `false` leaves it (and leaves any other destination alone). Lets a
 *  store that used to be a flag of its own keep its shape for its callers. */
export interface DestinationFlag extends Readable<boolean> {
  set(open: boolean): void;
  update(fn: (open: boolean) => boolean): void;
}

export function destinationFlag(d: Destination): DestinationFlag {
  const open = derived(destination, (cur) => cur === d);
  const set = (v: boolean) => (v ? goTo(d) : leave(d));
  return {
    subscribe: open.subscribe,
    set,
    update(fn) {
      let cur = false;
      open.subscribe((v) => (cur = v))();
      set(fn(cur));
    },
  };
}
