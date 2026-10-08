// The tray and menu-bar icon's state (Orbit Fleet redesign step 3.17, motion
// map "Tray and menu bar"): Breathe idle, Chase working, Halo needs you,
// Signal lost. Worked out here from what the window already knows and handed
// to `set_tray_state` (src-tauri/src/commands/tray.rs) when it changes.
import { derived, type Readable } from 'svelte/store';
import { invokeCmd } from './result';
import { sessions } from './sessions';
import { inboxCount } from './inbox';
import { hubConnection, isLost, lostSince, SIGNAL_LOST_AFTER_MS } from './hub_connection';

/** The names `TrayState` in tray.rs deserialises. */
export type TrayState = 'idle' | 'working' | 'needs_you' | 'lost';

export interface TrayFacts {
  /** The hub has been gone for longer than SIGNAL_LOST_AFTER_MS. */
  lost: boolean;
  needsYou: number;
  working: number;
}

/** Signal lost first, then what needs you, then work running. */
export function trayStateOf(f: TrayFacts): TrayState {
  if (f.lost) return 'lost';
  if (f.needsYou > 0) return 'needs_you';
  if (f.working > 0) return 'working';
  return 'idle';
}

// A clock that ticks only while the hub is lost, so the switch to Signal
// lost lands SIGNAL_LOST_AFTER_MS after the loss without a timer otherwise.
const lostLongEnough: Readable<boolean> = derived(
  [hubConnection, lostSince],
  ([$c, $since], set) => {
    if (!isLost($c) || $since === null) {
      set(false);
      return;
    }
    const left = $since + SIGNAL_LOST_AFTER_MS - Date.now();
    set(left <= 0);
    if (left <= 0) return;
    const t = setTimeout(() => set(true), left);
    return () => clearTimeout(t);
  },
  false,
);

const workingCount = derived(sessions, ($s) => $s.filter((r) => r.claude_status === 'working').length);

export const trayState: Readable<TrayState> = derived(
  [lostLongEnough, inboxCount, workingCount],
  ([$lost, $needs, $working]) => trayStateOf({ lost: $lost, needsYou: $needs, working: $working }),
);

/** Keeps the tray icon on `trayState`; returns the stop. A window with no
 *  tray (or no backend, in a browser) ignores the answer. */
export function startTraySync(set: (s: TrayState) => unknown = (state) => invokeCmd('set_tray_state', { state })): () => void {
  let last: TrayState | null = null;
  return trayState.subscribe((s) => {
    if (s === last) return;
    last = s;
    void set(s);
  });
}
