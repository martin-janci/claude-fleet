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
  | { state: 'reconnecting' | 'offline'; attempt: number; retry_in_secs: number; reason: string };

export const hubConnection = writable<HubConnection>({ state: 'standalone' });

/** Start following the link. Only a hub client calls this. */
export async function startHubConnection(): Promise<void> {
  // Listen first, then ask: an event landing between the two is then applied
  // on top of the answer rather than overwritten by it.
  await listen<HubConnection>('hub:connection', (e) => hubConnection.set(e.payload));
  const r = await invokeCmd<HubConnection>('hub_connection');
  // A failed query keeps what we had; inventing "connected" would hide the
  // one banner this exists to show.
  if (r.ok && r.value) hubConnection.set(r.value);
}

/** The banner's sentence, or null when there is nothing to say. */
export function connectionBanner(c: HubConnection, url: string | null): string | null {
  const hub = url ?? 'the hub';
  switch (c.state) {
    case 'reconnecting':
      return `Lost the live connection to ${hub}; what you see may be out of date. Reconnecting — attempt ${c.attempt}, next try in ${c.retry_in_secs} s (${c.reason}).`;
    case 'offline':
      return `Cannot reach ${hub}; what you see may be out of date. Retrying — attempt ${c.attempt}, next try in ${c.retry_in_secs} s (${c.reason}).`;
    default:
      return null;
  }
}
