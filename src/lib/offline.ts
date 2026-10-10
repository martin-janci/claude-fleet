// Open offline (redesign step 3.15): a desktop paired with a hub that cannot
// be reached opens on this computer's own sessions, and only those.
//
// The backend's mode is decided once at launch (backend/mod.rs), and a paired
// desktop never manages the hub's fleet from here, so this is not a switch to
// standalone: the hub's hosts and sessions stay the hub's, and this view
// reads nothing of them. It lists the tmux sessions on THIS machine
// (`offline_local_sessions`, which reads this machine's tmux server and
// writes nothing) under its one local host, with what this machine can do
// without the hub: open the session in VS Code, or copy the command that
// attaches it in a terminal. When the hub answers again the view steps aside
// and the app, loaded from the hub, is under it.
import { derived, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { hubConnection } from './hub_connection';

/** The one host Open offline lists: this computer. */
export const OFFLINE_HOST = 'local';

/** Mirrors `OfflineSession` in src-tauri/src/commands/hub.rs. */
export interface OfflineSession {
  name: string;
  created: number;
  last_activity: number;
  attached: boolean;
  /** `tmux attach -t '=name'`, quoted by the backend. */
  attach: string;
}

/** The person chose Open offline. */
export const offlineChosen = writable(false);

/** Offline is on screen: chosen, and the hub still not connected. */
export const offlineMode = derived([offlineChosen, hubConnection], ([$chosen, $conn]) => $chosen && $conn.state !== 'connected');

export const offlineSessions = writable<OfflineSession[]>([]);

export async function loadOfflineSessions(): Promise<Result<OfflineSession[]>> {
  const r = await invokeCmd<OfflineSession[]>('offline_local_sessions');
  if (r.ok) offlineSessions.set([...(r.value ?? [])].sort((a, b) => b.last_activity - a.last_activity));
  return r;
}

export function openOffline(): Promise<Result<OfflineSession[]>> {
  offlineChosen.set(true);
  return loadOfflineSessions();
}

export function leaveOffline(): void {
  offlineChosen.set(false);
}
