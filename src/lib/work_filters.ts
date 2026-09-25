// The work filters' chrome (work graph M10.4, the M5.5 "not done"): tracker,
// status category, assignee ("mine"), has-session and archived, persisted
// like the host and scope filters and applied through the one `rowMatches`
// (sidebar_index.ts) — this module only builds the rows and the filter
// object; it decides nothing `rowMatches` does not.
//
// Where the fields come from, all client-side (no backend change):
// - tracker: the work key's owning tracker (`trackerForKey`); a key two
//   trackers claim, or no key, has none.
// - status category: `SessionRow.work.status_category`; a past link has none,
//   so a status filter hides past work.
// - assignee "mine": the hub's own `mine` view (`work { tickets, view: mine }`
//   over its cache, up to 200 items): assigned to you and not done.
// - has-session: live sessions have one, past links do not. Only in work
//   mode — in project mode every row is a live session.
// - archived: a live session archived from the UI (M7), and every past link.

import { writable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import type { SessionRow } from './sessions';
import type { FilterRow, RowFilters, SessionPredicate } from './sidebar_index';
import { rowMatches, sessionFilterRow } from './sidebar_index';
import { trackerForKey, workTickets, type TrackerRow } from './trackers';

export type StatusFilter = 'all' | 'todo' | 'in_progress' | 'done';
export type HasSessionFilter = 'any' | 'yes' | 'no';

export interface WorkFilters {
  tracker: number | 'all';
  status: StatusFilter;
  assignee: 'all' | 'mine';
  hasSession: HasSessionFilter;
  /** Show archived rows (archived live sessions and past work). */
  archived: boolean;
}

export const DEFAULT_WORK_FILTERS: WorkFilters = {
  tracker: 'all',
  status: 'all',
  assignee: 'all',
  hasSession: 'any',
  archived: true,
};

export const STATUS_FILTERS: readonly StatusFilter[] = ['all', 'todo', 'in_progress', 'done'];
export const STATUS_FILTER_LABELS: Record<StatusFilter, string> = {
  all: 'any status',
  todo: 'to do',
  in_progress: 'in progress',
  done: 'done',
};
export const HAS_SESSION_FILTERS: readonly HasSessionFilter[] = ['any', 'yes', 'no'];
export const HAS_SESSION_LABELS: Record<HasSessionFilter, string> = {
  any: 'any',
  yes: 'with session',
  no: 'past only',
};

/** The assignee `rowMatches` reads for "mine". Not a name a tracker can
 *  return: `@` never starts a Jira / GitHub / Asana / Linear display name
 *  fleet stores. */
export const ASSIGNEE_MINE = '@me';

export function isWorkFilters(v: unknown): v is WorkFilters {
  if (typeof v !== 'object' || v === null) return false;
  const f = v as Record<string, unknown>;
  return (
    (f.tracker === 'all' || (typeof f.tracker === 'number' && Number.isInteger(f.tracker))) &&
    STATUS_FILTERS.includes(f.status as StatusFilter) &&
    (f.assignee === 'all' || f.assignee === 'mine') &&
    HAS_SESSION_FILTERS.includes(f.hasSession as HasSessionFilter) &&
    typeof f.archived === 'boolean'
  );
}

const PREF_KEY = 'sidebar.work-filters';

/** Persisted across restarts, like `hostFilter` and `scopeFilter`. */
export const workFilters = writable<WorkFilters>(readPref(PREF_KEY, DEFAULT_WORK_FILTERS, isWorkFilters));
workFilters.subscribe((v) => writePref(PREF_KEY, v));

/** The filters as they apply: a tracker that no longer exists is "all"
 *  (a removed tracker must not leave the sidebar empty), and has-session
 *  only in work mode. */
export function effectiveWorkFilters(
  f: WorkFilters,
  trackers: readonly Pick<TrackerRow, 'id'>[],
  workMode: boolean,
): WorkFilters {
  return {
    ...f,
    tracker: f.tracker !== 'all' && !trackers.some((t) => t.id === f.tracker) ? 'all' : f.tracker,
    hasSession: workMode ? f.hasSession : 'any',
  };
}

/** How many filters narrow the view (the chrome's count). */
export function activeWorkFilterCount(f: WorkFilters): number {
  return (
    (f.tracker !== 'all' ? 1 : 0) +
    (f.status !== 'all' ? 1 : 0) +
    (f.assignee !== 'all' ? 1 : 0) +
    (f.hasSession !== 'any' ? 1 : 0) +
    (f.archived ? 0 : 1)
  );
}

/** The `rowMatches` fields of these filters. */
export function toRowFilters(f: WorkFilters): RowFilters {
  return {
    tracker: f.tracker,
    status: f.status,
    assignee: f.assignee === 'mine' ? ASSIGNEE_MINE : 'all',
    hasSession: f.hasSession,
    archived: f.archived,
  };
}

/** What the rows are read against: the trackers and the items that are
 *  "mine". */
export interface WorkFilterContext {
  trackers: readonly TrackerRow[];
  mine: ReadonlySet<number>;
}

function trackerIdOf(key: string | null | undefined, trackers: readonly TrackerRow[]): number | null {
  return key ? (trackerForKey(key, trackers)?.id ?? null) : null;
}

/** A live session as a filter row with its work fields. */
export function sessionWorkRow(
  s: SessionRow,
  ctx: WorkFilterContext,
  scopeOf?: (s: SessionRow) => string,
): FilterRow {
  const itemId = s.work?.item_id ?? null;
  return {
    ...sessionFilterRow(s, scopeOf),
    trackerId: trackerIdOf(s.work?.key, ctx.trackers),
    assignees: itemId != null && ctx.mine.has(itemId) ? [ASSIGNEE_MINE] : [],
    archived: s.work?.archived_at != null,
  };
}

/** A past (ended) link's work fields, merged into its host / scope row. */
export function pastWorkFields(
  key: string,
  itemId: number | null | undefined,
  ctx: WorkFilterContext,
): Pick<FilterRow, 'trackerId' | 'assignees' | 'live' | 'archived'> {
  return {
    trackerId: trackerIdOf(key, ctx.trackers),
    assignees: itemId != null && ctx.mine.has(itemId) ? [ASSIGNEE_MINE] : [],
    live: false,
    archived: true,
  };
}

/** The session predicate of the work filters, `null` when none narrows. */
export function workFilterPredicate(f: WorkFilters, ctx: WorkFilterContext): SessionPredicate {
  if (activeWorkFilterCount(f) === 0) return null;
  const rf = toRowFilters(f);
  return (s) => rowMatches(sessionWorkRow(s, ctx), rf);
}

/** Two predicates, both of which must hold. */
export function bothPredicates(a: SessionPredicate, b: SessionPredicate): SessionPredicate {
  if (!a) return b;
  if (!b) return a;
  return (s) => a(s) && b(s);
}

/** The item ids of the hub's `mine` view. */
export const mineItemIds = writable<ReadonlySet<number>>(new Set());

/** Read the `mine` view (a read of the hub's cache; routed on a paired
 *  desktop). A failure keeps the last set. */
export async function loadMine(): Promise<void> {
  const r = await workTickets({ view: 'mine', limit: 200 });
  if (r.ok && Array.isArray(r.value)) mineItemIds.set(new Set(r.value.map((t) => t.id)));
}
