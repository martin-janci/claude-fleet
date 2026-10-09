import { derived, get, writable, type Readable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import { findSession, mergeSession, sessions, type SessionRow } from './sessions';

// One selection drives every pane (Orbit Fleet redesign step 3.4): the
// session the center pane shows and the task Details shows live in one place,
// so picking a task can never leave another task's session on screen.
//
// The selection is a session (`selectedRef`) plus a task (`taskSel`): the task
// Details shows when `focus` is `task`, and the one the Work lists light. While
// a task has the focus, the session is either one of its live sessions or none:
// `pickTask` enforces it, and `back` remembers the session a pick put away so
// the task's Close can return to it.

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

// Not a `derived`: a svelte store treats every object as changed, so a
// derived row re-notified every reader on each row event anywhere in the
// fleet, and the panes keyed on it (WatchView's capture poll, TicketCard's
// history read, WatchSummary) re-ran on every flush (review r16). This one
// notifies only when the row object itself changes; untouched rows keep their
// identity through `sessions`.
const selectedRow = writable<SessionRow | null>(null);
function syncSelectedRow(): void {
  const ref = get(selectedRef);
  const next = ref ? (findSession(get(sessions), ref) ?? null) : null;
  if (next !== get(selectedRow)) selectedRow.set(next);
}
sessions.subscribe(syncSelectedRow);
selectedRef.subscribe(syncSelectedRow);

export const selectedSession: Readable<SessionRow | null> = { subscribe: selectedRow.subscribe };

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
    // A rename made elsewhere (the agent's `rename_session`, another client)
    // arrives only as this event: the remembered session follows it, or the
    // next launch looks for the old name, finds nothing and forgets the
    // pref (review r07). Only when the pref named the old identity, so a
    // pop-out window's selection never takes over the main window's.
    const last = readPref<SessionIdent | null>(LAST_SESSION_KEY, null, isSessionIdentOrNull);
    if (last && last.host_alias === ref.host_alias && last.tmux_name === ref.tmux_name) {
      writePref<SessionIdent>(LAST_SESSION_KEY, { host_alias: match.host_alias, tmux_name: match.tmux_name });
    }
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

/** A task's link to a session, as far as the selection needs it: the live
 *  session (`session_id`) unless the link has `ended`. Structurally a
 *  `WorkTaskLink` (work_view.ts), which imports this module. */
export interface TaskSessionLink {
  session_id?: number | null;
  state?: string | null;
}

export type SelectionFocus = 'session' | 'task';

interface TaskSelection {
  /** The selected task's id (`item:<id>` or `ref:<KEY>`). */
  task: string | null;
  /** Which of the two Details shows; the center pane always shows the session. */
  focus: SelectionFocus;
  /** The session a task pick put away, for the task's Close. */
  back: SessionRef | null;
}

const taskSel = writable<TaskSelection>({ task: null, focus: 'session', back: null });

export interface Selection extends TaskSelection {
  session: SessionRef | null;
}

/** The whole selection, for a reader that needs both halves at once. */
export const selection: Readable<Selection> = derived([selectedRef, taskSel], ([$ref, $t]) => ({ ...$t, session: $ref }));

const refOf = (s: SessionRow): SessionRef => ({ id: s.id, host_alias: s.host_alias, tmux_name: s.tmux_name });

/** The selected task's id. Setting it renames the selection (a bare key that
 *  became a ticket, a per-view restore) and leaves the session alone; picking
 *  a task is `pickTask`. */
export const selectedTaskId = {
  subscribe: derived(taskSel, (t) => t.task).subscribe,
  set(id: string | null): void {
    taskSel.update((t) => (t.task === id ? t : { ...t, task: id }));
  },
};

/** Whether Details shows the selected task rather than the session. `false`
 *  is the task's Close (`closeTask`); `true` gives the selected task the focus
 *  as it stands. */
export const taskFocused = {
  subscribe: derived(taskSel, (t) => t.focus === 'task').subscribe,
  set(open: boolean): void {
    if (open) taskSel.update((t) => (t.focus === 'task' ? t : { ...t, focus: 'task' }));
    else closeTask();
  },
};

/**
 * Pick a task: Details shows it, and the center pane shows one of its live
 * sessions or none. The session already open stays when it is one of them;
 * otherwise the first live one the store holds is opened (quietly: it follows
 * the pick, it is not a session open), else the pane empties.
 *
 * `links` are the task's session links when the caller has them (a tree row,
 * a board card). Without them the open session stays only when the task is
 * its primary work; Details passes the links once the task's detail loads
 * (`taskLinksLoaded`), which opens the live session then.
 */
export function pickTask(taskId: string, links?: readonly TaskSessionLink[]): void {
  const all = get(sessions);
  const ref = get(selectedRef);
  const cur = ref ? (findSession(all, ref) ?? null) : null;
  let next: SessionRow | null | undefined; // undefined: keep `cur`
  if (links) {
    const live = liveSessionIds(links);
    next = cur && live.includes(cur.id) ? undefined : (firstInStore(all, live) ?? null);
  } else {
    next = cur && isPrimaryTaskOf(cur, taskId) ? undefined : null;
  }
  const prev = get(taskSel);
  if (next === undefined) {
    taskSel.set({ task: taskId, focus: 'task', back: null });
    return;
  }
  // A chain of picks keeps the session the first one put away.
  const back = cur ? refOf(cur) : prev.back;
  taskSel.set({ task: taskId, focus: 'task', back });
  if (next) selectSession(next, { follow: true });
  else selectedRef.set(null);
}

/** The selected task's links arrived (its detail loaded). While it has the
 *  focus and no session is open, its first live session opens. */
export function taskLinksLoaded(taskId: string, links: readonly TaskSessionLink[]): void {
  const t = get(taskSel);
  if (t.task !== taskId || t.focus !== 'task' || get(selectedRef)) return;
  const row = firstInStore(get(sessions), liveSessionIds(links));
  if (row) selectSession(row, { follow: true });
}

/** The task's Close: Details goes back to the session, reopening the one the
 *  pick put away when the pane is empty and that session still exists. */
export function closeTask(): void {
  const t = get(taskSel);
  if (t.focus === 'session' && !t.back) return;
  taskSel.set({ ...t, focus: 'session', back: null });
  if (t.back && !get(selectedRef)) {
    const row = findSession(get(sessions), t.back);
    if (row) selectSession(row, { follow: true });
  }
}

/** The session `back` names, while the pane is empty (for "← Back to …"). */
export const backSession: Readable<SessionRow | null> = derived([sessions, selectedRef, taskSel], ([$sessions, $ref, $t]) =>
  !$ref && $t.back ? (findSession($sessions, $t.back) ?? null) : null,
);

function liveSessionIds(links: readonly TaskSessionLink[]): number[] {
  return links.filter((l) => l.session_id != null && l.state !== 'ended').map((l) => l.session_id as number);
}

function firstInStore(all: SessionRow[], ids: number[]): SessionRow | undefined {
  for (const id of ids) {
    const row = all.find((r) => r.id === id && r.status !== 'ghost');
    if (row) return row;
  }
  return undefined;
}

function isPrimaryTaskOf(s: SessionRow, taskId: string): boolean {
  const w = s.work;
  if (!w) return false;
  if (w.item_id != null) return taskId === `item:${w.item_id}`;
  return w.key != null && taskId === `ref:${w.key}`;
}

/**
 * Select a session. A deliberate open (anything but `follow`) gives the
 * session the focus; `task` names the task it was opened from (an occurrence
 * under a task in the Work view), which the Work lists then light.
 */
export function selectSession(
  s: SessionRow | null,
  opts: { follow?: boolean; task?: string; remember?: boolean } = {},
): void {
  if (s === null) {
    selectedRef.set(null);
    return;
  }
  // A row handed to us that the store doesn't know yet (a command result that
  // raced its own event) is merged first so the derived selection can resolve
  // it. mergeSession keeps its tombstone + monotonic guards, so a fresher row
  // already in the store wins and a just-killed one stays dead.
  if (!findSession(get(sessions), s)) mergeSession(s);
  selectedRef.set(refOf(s));
  if (!opts.follow) {
    taskSel.update((t) => ({ task: opts.task ?? t.task, focus: 'session', back: null }));
  }
  // A pop-out terminal window (step 5.4) shares this pref store with the
  // main window; it passes `remember: false` so the next launch still
  // reopens what the main window had.
  if (opts.remember !== false) {
    writePref<SessionIdent>(LAST_SESSION_KEY, {
      host_alias: s.host_alias,
      tmux_name: s.tmux_name,
    });
  }
  if (!opts.follow) for (const fn of openedListeners) fn(s);
}

// Monotonically increasing counter bumped by `selectSessionExplicitly`,
// AFTER the selection itself has been applied. Deliberately NOT id-keyed:
// - Re-selecting the SAME session explicitly (no id change) must still
//   reveal it; a counter bump is a distinct event even when the id repeats,
//   where an id-keyed flag compared against `$selectedSession.id` would see
//   no change and do nothing.
// - Nothing can "replay" a stale reveal against an unrelated later
//   selection: a non-explicit reselect never bumps this, so the sequence
//   number a reader reacts to only ever changes on an explicit pick, and a
//   reader reads the CURRENT `$selectedSession` at the moment it handles a
//   bump rather than remembering which id the bump was "for".
// A reader must compare against the value it saw when IT started watching
// (not a fixed sentinel like 0, and not "did the number change since the
// component's own last run"): the Sidebar is destroyed and recreated on
// collapse/expand, so a fresh mount does NOT replay a bump that happened
// before it existed — see `Sidebar.svelte`'s `appliedSeq`.
export const revealSeq = writable(0);

/**
 * Select a session as a deliberate user action: a sidebar row click, the
 * quick switcher, restore-on-launch, a fresh New-session create, or any
 * other explicit "open this session" link (a related/background/task
 * session, a review, a move's "open the new session"). Bumps `revealSeq` so
 * Sidebar widens `hostFilter` to `all` if it hides the session's host — a
 * non-explicit reselect (a rename/recreate resync, a completed move's
 * follow reselect) must go through the plain `selectSession` instead and
 * stays silent.
 */
export function selectSessionExplicitly(
  s: SessionRow,
  opts: { follow?: boolean; task?: string } = {},
): void {
  selectSession(s, opts);
  revealSeq.update((n) => n + 1);
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
