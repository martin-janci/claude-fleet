import { get, writable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import type { ProjectTreeRow } from './projects';
import { sessions, type SessionRow } from './sessions';

// Two mutually-exclusive selection slots drive the center pane:
//   - selectedProject: the user clicked a project row in the sidebar.
//   - selectedSession: the user clicked a session row in the sidebar.
// Setting one clears the other so the center pane always has a single
// unambiguous focus.

export const selectedProject = writable<ProjectTreeRow | null>(null);
export const selectedSession = writable<SessionRow | null>(null);

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
  if (p !== null) selectedSession.set(null);
}

export function selectSession(s: SessionRow | null): void {
  selectedSession.set(s);
  if (s !== null) {
    selectedProject.set(null);
    writePref<SessionIdent>(LAST_SESSION_KEY, {
      host_alias: s.host_alias,
      tmux_name: s.tmux_name,
    });
  }
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
  selectedSession.set(null);
}
