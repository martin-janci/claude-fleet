import { get, writable } from 'svelte/store';
import { selectSessionExplicitly } from './selection';
import { sessions } from './sessions';

// A one-session view of the sidebar, set by clicking a suggestion (a link
// suggestion, a tidy-up candidate) so it can be looked at in detail. While
// set, the session tree shows only that session — past the host, bg-agent,
// scope, recency, search and Needs-you filters, which could otherwise hide
// the very row the user asked for. Session-scoped, never persisted: the
// review sheets clear it when they close, and the sidebar's chip clears it
// by hand.

export interface SessionFocus {
  id: number;
  /** What the chip names: the row's display name. */
  label: string;
}

export const sessionFocus = writable<SessionFocus | null>(null);

/** Narrow the sidebar to one session and open it in the center pane. */
export function focusSession(id: number, label: string): void {
  sessionFocus.set({ id, label });
  const row = get(sessions).find((s) => s.id === id);
  if (row) selectSessionExplicitly(row);
}

export function clearSessionFocus(): void {
  sessionFocus.set(null);
}

// A focused session that leaves the store (killed by the tidy-up it was
// focused for, dismissed) would leave an empty sidebar behind: drop the
// focus with it.
sessions.subscribe(($sessions) => {
  const f = get(sessionFocus);
  if (f && !$sessions.some((s) => s.id === f.id)) sessionFocus.set(null);
});
