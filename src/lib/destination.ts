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
// Classic reads this store with no visible change; the New layout's rail
// (step 3.2) will write the same store.
import { derived, writable, type Readable } from 'svelte/store';

export type Destination = 'session' | 'files' | 'hosts' | 'assets' | 'board';

export const DESTINATIONS: readonly Destination[] = ['session', 'files', 'hosts', 'assets', 'board'];

export const destination = writable<Destination>('session');

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
