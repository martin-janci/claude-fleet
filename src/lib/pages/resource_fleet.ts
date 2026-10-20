// Which fleet a resource page acts on (multi-hub), for the components deep
// inside it that call a command of their own rather than through the page's
// `run` (an org rule's live preview, a member's Trust device or pairing
// code). `ResourcePage` provides it; anything rendered outside one reads
// the primary.
import { getContext, setContext } from 'svelte';

export interface ResourceFleet {
  /** An extra fleet's slot id, or null for the primary. */
  fleet: string | null;
  /** That fleet's hub address, for what a record says about "this hub";
   *  null for the primary (whose address `hubStatus` holds). */
  hubUrl: string | null;
}

const KEY = Symbol('resource-fleet');
const PRIMARY_FLEET: ResourceFleet = { fleet: null, hubUrl: null };

/** Provide the page's fleet to everything rendered under it. A getter, so a
 *  prop that changes is read fresh on every call. */
export function provideResourceFleet(get: () => ResourceFleet): void {
  setContext(KEY, get);
}

/** The fleet the enclosing resource page acts on (the primary outside one).
 *  Call during component initialisation; call the getter when acting. */
export function useResourceFleet(): () => ResourceFleet {
  return getContext<(() => ResourceFleet) | undefined>(KEY) ?? (() => PRIMARY_FLEET);
}
