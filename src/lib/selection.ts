import { derived, get, writable, type Readable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import { findSession, mergeSession, sessions, type SessionRow } from './sessions';

// The selected session drives the center pane.

/**
 * Identity of the selected session. Only the *reference* is stored here; the
 * row itself is derived from the `sessions` store below so that every
 * `session:updated` event (status, safe_kill_state, friendly_name, …) flows
 * through to whoever reads `$selectedSession` — a frozen snapshot taken at
 * click time went stale the moment the backend changed anything.
 *
 * `id` is the primary key; `host_alias` + `tmux_name` is the fallback for a
 * row that was re-discovered under a fresh id (the numeric id churns, the
 * pair is stable). A bare tmux_name is never enough: default names are
 * project-derived, so the same name on two hosts is the common case.
 */
export interface SessionRef {
  id: number;
  host_alias: string;
  tmux_name: string;
}

const selectedRef = writable<SessionRef | null>(null);

export const selectedSession: Readable<SessionRow | null> = derived(
  [sessions, selectedRef],
  ([$sessions, $ref]) => ($ref ? (findSession($sessions, $ref) ?? null) : null),
);

// When the selected row leaves the store (killed, dismissed, dropped by a
// full re-fetch) the selection is cleared — not merely hidden — so a later
// row that happens to reuse the identity isn't silently re-selected, and the
// terminal pane drops back to its empty state instead of trying to attach.
sessions.subscribe(($sessions) => {
  const ref = get(selectedRef);
  if (!ref) return;
  const match = findSession($sessions, ref);
  if (!match) {
    selectedRef.set(null);
  } else if (match.id !== ref.id || match.tmux_name !== ref.tmux_name) {
    // Matched via the host+name fallback (id churned) or the row was renamed
    // under the same id — re-sync the ref so the primary key stays accurate.
    selectedRef.set({ id: match.id, host_alias: match.host_alias, tmux_name: match.tmux_name });
  }
});

// Persist the last-selected session so it can be re-opened on the next launch.
// Keyed by the stable host_alias+tmux_name identity — the numeric `id` churns
// when a session is re-discovered, so it can't be used across restarts.
const LAST_SESSION_KEY = 'session.last';

interface SessionIdent {
  host_alias: string;
  tmux_name: string;
}

const isSessionIdentOrNull = (v: unknown): v is SessionIdent | null =>
  v === null ||
  (typeof v === 'object' &&
    v !== null &&
    typeof (v as SessionIdent).host_alias === 'string' &&
    typeof (v as SessionIdent).tmux_name === 'string');

// Listeners told each time a session is deliberately OPENED (a sidebar
// click, the quick switcher, a Hosts-view jump, a fresh create) — App leaves
// the Hosts view on it. Re-syncs that merely follow the same session (a
// rename, a recreate, restore-on-launch) pass `{ follow: true }` and stay
// silent, so they never yank the user out of a view.
const openedListeners = new Set<(s: SessionRow) => void>();

/** Subscribe to deliberate session opens; returns the unsubscribe. */
export function onSessionOpened(fn: (s: SessionRow) => void): () => void {
  openedListeners.add(fn);
  return () => openedListeners.delete(fn);
}

export function selectSession(s: SessionRow | null, opts: { follow?: boolean } = {}): void {
  if (s === null) {
    selectedRef.set(null);
    return;
  }
  // A row handed to us that the store doesn't know yet (a command result that
  // raced its own event) is merged first so the derived selection can resolve
  // it. mergeSession keeps its tombstone + monotonic guards, so a fresher row
  // already in the store wins and a just-killed one stays dead.
  if (!findSession(get(sessions), s)) mergeSession(s);
  selectedRef.set({ id: s.id, host_alias: s.host_alias, tmux_name: s.tmux_name });
  writePref<SessionIdent>(LAST_SESSION_KEY, {
    host_alias: s.host_alias,
    tmux_name: s.tmux_name,
  });
  if (!opts.follow) for (const fn of openedListeners) fn(s);
}

// Set by `selectSessionExplicitly` for the id it just selected, and cleared
// the first time Sidebar consumes it. A plain `selectSession` call (a
// rename/recreate resync, a post-move follow reselect, the sessions-store
// re-sync above) never touches this, so it never widens `hostFilter` — see
// `consumeRevealRequest`.
const revealRequested = writable<number | null>(null);

/**
 * Select a session as a deliberate user action: a sidebar row click, the
 * quick switcher, restore-on-launch, or a fresh New-session create. Marks
 * the pick as reveal-worthy so Sidebar widens `hostFilter` to `all` if it
 * hides the session's host — a non-explicit reselect (a rename/recreate
 * resync, a completed move's follow reselect) must go through the plain
 * `selectSession` instead and stays silent.
 */
export function selectSessionExplicitly(
  s: SessionRow,
  opts: { follow?: boolean } = {},
): void {
  selectSession(s, opts);
  revealRequested.set(s.id);
}

/**
 * Sidebar-only: true the first time it's called for `id` after an explicit
 * select, then clears itself so a later non-explicit id change never re-widens
 * the filter on this same id's behalf.
 */
export function consumeRevealRequest(id: number): boolean {
  if (get(revealRequested) !== id) return false;
  revealRequested.set(null);
  return true;
}

/**
 * Re-select the session the user last had open. Call once after the `sessions`
 * store is populated (post-bootstrap). If the remembered session no longer
 * exists or is now a ghost, forget the pref and select nothing.
 */
export function restoreLastSession(): void {
  const ident = readPref<SessionIdent | null>(LAST_SESSION_KEY, null, isSessionIdentOrNull);
  if (!ident) return;
  const match = get(sessions).find(
    (s) => s.host_alias === ident.host_alias && s.tmux_name === ident.tmux_name,
  );
  if (match && match.status !== 'ghost') {
    selectSessionExplicitly(match, { follow: true });
  } else {
    writePref<SessionIdent | null>(LAST_SESSION_KEY, null);
  }
}

export function clearSelection(): void {
  selectedRef.set(null);
}
