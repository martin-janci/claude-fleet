import { derived, get, writable, type Readable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import type { ProjectTreeRow } from './projects';
import { findSession, mergeSession, sessions, type SessionRow } from './sessions';

// Two mutually-exclusive selection slots drive the center pane:
//   - selectedProject: the user clicked a project row in the sidebar.
//   - selectedSession: the user clicked a session row in the sidebar.
// Setting one clears the other so the center pane always has a single
// unambiguous focus.

export const selectedProject = writable<ProjectTreeRow | null>(null);

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

export function selectProject(p: ProjectTreeRow | null): void {
  selectedProject.set(p);
  if (p !== null) selectedRef.set(null);
}

export function selectSession(s: SessionRow | null): void {
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
  selectedProject.set(null);
  writePref<SessionIdent>(LAST_SESSION_KEY, {
    host_alias: s.host_alias,
    tmux_name: s.tmux_name,
  });
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
    selectSession(match);
  } else {
    writePref<SessionIdent | null>(LAST_SESSION_KEY, null);
  }
}

export function clearSelection(): void {
  selectedProject.set(null);
  selectedRef.set(null);
}
