// Whether this window's live link to the hub is up — the "disconnected"
// banner the design asks for ("a dropped event stream reconnects with backoff
// and one refetch, showing a banner while disconnected").
//
// Fed by the backend's event bridge (`src-tauri/src/backend/connection.rs`):
// the `hub_connection` command for the state at mount time, then every
// `hub:connection` event. Its own small store on purpose — this is a fact
// about this process's socket, not a row, and no existing store changes.
// The reason text arrives already redacted, one line, and capped.
import { writable } from 'svelte/store';
import { listen } from '@tauri-apps/api/event';
import { invokeCmd } from './result';

export type HubConnection =
  | { state: 'standalone' | 'connecting' | 'connected' }
  | { state: 'reconnecting' | 'offline'; attempt: number; retry_in_secs: number; reason: string }
  // The hub's `ready` frame named a wire-contract revision this app does not
  // accept (`src-tauri/src/backend/contract.rs`). While in either state the
  // backend applies no row event and no re-list from that connection.
  | { state: 'hub_too_old'; hub_contract: number; min_contract: number }
  | { state: 'hub_too_new'; hub_contract: number; max_contract: number };

export const hubConnection = writable<HubConnection>({ state: 'standalone' });

export const isLost = (c: HubConnection): c is Extract<HubConnection, { state: 'reconnecting' | 'offline' }> =>
  c.state === 'reconnecting' || c.state === 'offline';

/** Redesign step 3.14: when this window lost its link to the hub (ms since
 *  the epoch), or null while it holds one. Stamped by the first
 *  `reconnecting` or `offline` (a hub that never answered included), kept
 *  across the retries after it and across a skew verdict, cleared by
 *  `connected`. The banner says "Lost … at 14:52" from it and turns its
 *  Gravity well into Signal lost after SIGNAL_LOST_AFTER_MS. */
export const lostSince = writable<number | null>(null);

/** How long a lost hub reads as reconnecting before it reads as lost. */
export const SIGNAL_LOST_AFTER_MS = 6000;

// Follows the store whoever sets it (the bridge's events, the query at
// mount, a test), so the two can never disagree.
hubConnection.subscribe((c) => {
  if (isLost(c)) lostSince.update((t) => t ?? Date.now());
  else if (c.state === 'connected' || c.state === 'standalone') lostSince.set(null);
});

/** "14:52", on this machine's clock. */
function clockTime(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** The lost banner's line, as the design writes it: "Lost the hub at 14:52 ·
 *  try 3 · your sessions keep running on their hosts". The banner puts the
 *  reason, the live countdown and Retry now beside it. */
export function lostLine(c: HubConnection, lostAt: number | null, url: string | null): string | null {
  if (!isLost(c)) return null;
  const hub = url ?? 'the hub';
  const at = lostAt === null ? null : clockTime(lostAt);
  const head =
    c.state === 'reconnecting'
      ? `Lost ${hub}${at ? ` at ${at}` : ''}`
      : `Cannot reach ${hub}${at ? ` since ${at}` : ''}`;
  return `${head} · try ${c.attempt} · your sessions keep running on their hosts`;
}

/** Runs after the backend's event bridge re-listed sessions, hosts, tasks
 * and accounts following a gap it could not replay (`hub:resynced`). The
 * app installs the loaders for the stores whose list tools answer a
 * different shape than their events: projects (with worktrees) and
 * trackers (with work items). */
export type GapHandler = () => void;
let onGap: GapHandler | null = null;
export function setGapHandler(fn: GapHandler | null): void {
  onGap = fn;
}

/** Start following the link. Only a hub client calls this. */
export async function startHubConnection(): Promise<void> {
  // Listen first, then ask: an event landing between the two is then applied
  // on top of the answer rather than overwritten by it.
  await listen<HubConnection>('hub:connection', (e) => hubConnection.set(e.payload));
  await listen('hub:resynced', () => onGap?.());
  const r = await invokeCmd<HubConnection>('hub_connection');
  // A failed query keeps what we had; inventing "connected" would hide the
  // one banner this exists to show.
  if (r.ok && r.value) hubConnection.set(r.value);
}

/** The banner's Retry now: try the hub again at once instead of after the
 *  backoff. Its answer is the next `hub:connection` event. */
export async function retryHubNow(): Promise<void> {
  await invokeCmd<null>('hub_retry_now');
}

/** The banner's sentence, or null when there is nothing to say. `live`:
 *  the banner counts the wait down beside it, so the sentence leaves it out. */
export function connectionBanner(c: HubConnection, url: string | null, opts: { live?: boolean } = {}): string | null {
  const hub = url ?? 'the hub';
  const wait = (secs: number) => (opts.live ? '' : `, next try in ${secs} s`);
  switch (c.state) {
    case 'reconnecting':
      return `Lost the live connection to ${hub}; what you see may be out of date. Reconnecting — attempt ${c.attempt}${wait(c.retry_in_secs)} (${c.reason}).`;
    case 'offline':
      return `Cannot reach ${hub}; what you see may be out of date. Retrying — attempt ${c.attempt}${wait(c.retry_in_secs)} (${c.reason}).`;
    case 'hub_too_old':
      return `${hub}'s wire contract is revision ${c.hub_contract}, older than the ${c.min_contract} this app requires; what you see may be out of date. Update the hub.`;
    case 'hub_too_new':
      return `${hub}'s wire contract is revision ${c.hub_contract}, newer than the ${c.max_contract} this app understands; what you see may be out of date. Update this app.`;
    default:
      return null;
  }
}
