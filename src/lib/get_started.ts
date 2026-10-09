// Get started (Orbit Fleet redesign step 10.5, board Tour): the six things
// that make a working fleet, as a floating checklist in the bottom-right
// corner. It replaced the sidebar's onboarding card;
// each row is done from the stores the app already keeps, and a click
// opens the place that does it.
import { readPref, writePref } from './prefs';
import { writable } from 'svelte/store';
import { listRoutines } from './routines';

export type GetStartedId = 'host' | 'account' | 'session' | 'github' | 'phone' | 'routine';

export interface GetStartedItem {
  id: GetStartedId;
  label: string;
  done: boolean;
  /** The first row not done: highlighted, with its time. */
  next: boolean;
  /** What it takes, shown on the next row ("2 min"). */
  minutes: number;
}

export interface GetStartedInputs {
  visibleHostCount: number;
  accountCount: number;
  workSessionCount: number;
  /** GitHub trackers whose last sync answered. */
  githubConnected: boolean;
  /** Paired devices other than the one reading the list. */
  otherDeviceCount: number;
  /** Routines switched on; `null` while this build cannot list them. */
  enabledRoutineCount: number | null;
}

const ROWS: ReadonlyArray<{ id: GetStartedId; label: string; minutes: number }> = [
  { id: 'host', label: 'Add a host', minutes: 2 },
  { id: 'account', label: 'Sign in a Claude account', minutes: 1 },
  { id: 'session', label: 'Start your first session', minutes: 1 },
  { id: 'github', label: 'Connect GitHub for PRs', minutes: 2 },
  { id: 'phone', label: 'Pair your phone', minutes: 1 },
  { id: 'routine', label: 'Turn on a routine', minutes: 2 },
];

function isDone(id: GetStartedId, i: GetStartedInputs): boolean {
  switch (id) {
    case 'host':
      return i.visibleHostCount > 0;
    case 'account':
      return i.accountCount > 0;
    case 'session':
      return i.workSessionCount > 0;
    case 'github':
      return i.githubConnected;
    case 'phone':
      return i.otherDeviceCount > 0;
    case 'routine':
      return (i.enabledRoutineCount ?? 0) > 0;
  }
}

export function getStartedItems(i: GetStartedInputs): GetStartedItem[] {
  let nextGiven = false;
  return ROWS.map((r) => {
    const done = isDone(r.id, i);
    const next = !done && !nextGiven;
    if (next) nextGiven = true;
    return { ...r, done, next };
  });
}

export function doneCount(items: readonly GetStartedItem[]): number {
  return items.filter((i) => i.done).length;
}

const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

/** The panel folded to its title line ("–"). Dismissing it for good is the
 *  onboarding card's own `onboardingDismissed`, so Settings' Replay brings
 *  either back. */
export const getStartedFolded = writable<boolean>(readPref('get-started-folded', false, isBool));
getStartedFolded.subscribe((v) => writePref('get-started-folded', v));

/** How many routines are switched on, or `null` when the hub cannot list
 *  them (an older hub): the row then stays open. */
export async function enabledRoutineCount(): Promise<number | null> {
  const r = await listRoutines();
  if (!r.ok || !Array.isArray(r.value)) return null;
  return r.value.filter((x) => x.enabled).length;
}

/**
 * Whether the first fleet is being built (step 10.10): a session start is
 * in flight (its create command, or a row whose agent is not up yet) and no
 * work session is up besides the ones starting. Get started shows the Galaxy
 * for exactly as long; a fleet that already has a running session starts
 * its next one under the Pulse sequence alone.
 */
export function buildingFirstFleet(i: { workSessionIds: readonly number[]; starting: ReadonlySet<number>; creating: boolean }): boolean {
  if (!i.creating && i.starting.size === 0) return false;
  return i.workSessionIds.every((id) => i.starting.has(id));
}
