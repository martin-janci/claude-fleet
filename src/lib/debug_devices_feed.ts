// The person's debug device list, read once for every session strip on
// screen (`SessionDevices.svelte`). Each open Conversation pane used to poll
// `list_debug_devices` on its own timer, so N panes made N hub calls every
// 30 s; this keeps one timer while any strip is mounted, and a strip asks
// for a fresh read when it opens or after it acted on a device.
import { writable, type Readable } from 'svelte/store';
import { listDebugDevices, type DebugDevice } from './debug_devices';
import { pollWhileShown, type Poller } from './window_hidden';

/** How often the list is read again while a strip is mounted. */
export const DEVICES_EVERY_MS = 90_000;
/** Refusals no retry can change: stop asking until the last strip goes. */
const GIVE_UP = ['E_LOCAL_ONLY', 'E_HUB_CONTRACT', 'E_UNKNOWN_COMMAND'];

export interface DeviceFeed {
  devices: DebugDevice[];
  /** When the list was last read, in seconds. */
  at: number;
}

const feed = writable<DeviceFeed>({ devices: [], at: Date.now() / 1000 });
export const debugDeviceFeed: Readable<DeviceFeed> = { subscribe: feed.subscribe };

let poller: Poller | null = null;
let watchers = 0;

async function read(): Promise<boolean> {
  const r = await listDebugDevices();
  const at = Date.now() / 1000;
  if (r.ok) feed.set({ devices: Array.isArray(r.value) ? r.value : [], at });
  else if (GIVE_UP.includes(r.error.code)) {
    feed.set({ devices: [], at });
    return false;
  }
  // Any other failure keeps the last list: the next tick tries again.
  return true;
}

/** Keep the list fresh while the caller is mounted; returns the release.
 *  The first watcher reads at once; later ones share what it read. */
export function watchDebugDevices(): () => void {
  watchers += 1;
  poller ??= pollWhileShown(read, { every: DEVICES_EVERY_MS });
  let released = false;
  return () => {
    if (released) return;
    released = true;
    watchers -= 1;
    if (watchers === 0) {
      poller?.stop();
      poller = null;
      // Nobody shows it: the next strip starts from a fresh read, not an old list.
      feed.set({ devices: [], at: Date.now() / 1000 });
    }
  };
}

/** Read the list again now (a strip opened, a device was released). */
export function refreshDebugDevices(): void {
  poller?.poke();
}
