// Pure index builders for Sidebar.svelte.
//
// Sidebar renders one row per session; both builders must run ONCE per
// `$sessions` change (inside a `$derived`), never per row — otherwise the
// tree goes quadratic at a few hundred sessions. They live in their own
// module so Sidebar.test.ts can spy on them and assert the call count
// deterministically instead of measuring jsdom wall-clock.
import type { SessionRow } from './sessions';
import type { WorkKey } from './work_keys';

/** Optional per-row predicate layered on top of the host / bg filters
 *  (the "N stuck" and "needs attention" pills). `null` = no extra filter. */
export type SessionPredicate = ((s: SessionRow) => boolean) | null;

/** The org scope a view is narrowed to (work graph M5): the chosen scope
 *  id and how to read a session's. `null` = every scope. */
export type ScopeFilter = { id: string; of: (s: SessionRow) => string } | null;

/** One row of any list the filters apply to — a live session, a ticket, a
 *  past (ended) link — reduced to what a filter reads. */
export interface FilterRow {
  /** The session's host (a past link: its snapshot's); null for a ticket. */
  host: string | null;
  /** Its org scope (`org:<id>`, `owner:<name>`, `unassigned`), or `*`
   *  for a row no scope can claim (passes every scope filter). */
  scope: string;
  /** A session's kind (`bg` rows follow the bg toggle). */
  kind?: string;
  trackerId?: number | null;
  /** todo | in_progress | done. */
  statusCategory?: string | null;
  assignees?: readonly string[];
  /** A live session works on it. */
  live: boolean;
  /** Ended / past work. */
  archived: boolean;
  /** The session itself, for the needs-you predicate. */
  session?: SessionRow;
}

/** Every filter of the sidebar, ⌘K and the work views, composed. A field
 *  left undefined does not filter. */
export interface RowFilters {
  host?: string; // 'all' | alias
  scope?: string; // 'all' | scope id
  showBgAgents?: boolean;
  tracker?: number | 'all';
  status?: string; // 'all' | category
  assignee?: string; // 'all' | name
  hasSession?: 'any' | 'yes' | 'no';
  /** Include archived (past) rows. Default true. */
  archived?: boolean;
  /** The needs-you (or any other) predicate over the row's session. */
  predicate?: SessionPredicate;
}

/** THE one filter predicate (work graph M5.5): project mode, work mode and
 *  ⌘K all call it, so the filters can never diverge. Every clause must hold. */
export function rowMatches(r: FilterRow, f: RowFilters): boolean {
  if (f.host && f.host !== 'all' && r.host !== null && r.host !== f.host) return false;
  // `*`: a row that belongs to no scope (a ticket of an unassigned
  // tracker) is not narrowed away by one.
  if (f.scope && f.scope !== 'all' && r.scope !== '*' && r.scope !== f.scope) return false;
  if (f.showBgAgents === false && r.kind === 'bg') return false;
  if (f.tracker !== undefined && f.tracker !== 'all' && r.trackerId !== f.tracker) return false;
  if (f.status && f.status !== 'all' && r.statusCategory !== f.status) return false;
  if (f.assignee && f.assignee !== 'all' && !(r.assignees ?? []).includes(f.assignee)) return false;
  if (f.hasSession === 'yes' && !r.live) return false;
  if (f.hasSession === 'no' && r.live) return false;
  if (f.archived === false && r.archived) return false;
  if (f.predicate) {
    if (!r.session || !f.predicate(r.session)) return false;
  }
  return true;
}

/** A live session as a [`FilterRow`]. */
export function sessionFilterRow(s: SessionRow, scopeOf?: (s: SessionRow) => string): FilterRow {
  return {
    host: s.host_alias,
    scope: scopeOf ? scopeOf(s) : 'all',
    kind: s.kind,
    statusCategory: s.work?.status_category ?? null,
    live: true,
    archived: false,
    session: s,
  };
}

/** True when the row passes the host, bg-agent, scope and optional extra
 *  filters — `rowMatches` over the session. */
export function sessionVisible(
  s: SessionRow,
  hostFilter: string,
  showBgAgents: boolean,
  predicate: SessionPredicate = null,
  scope: ScopeFilter = null,
): boolean {
  return rowMatches(sessionFilterRow(s, scope?.of), {
    host: hostFilter,
    scope: scope?.id ?? 'all',
    showBgAgents,
    predicate,
  });
}

/** project_id → sessions visible under the current host / bg-agent filter
 *  (plus the optional predicate). */
export function buildSessionsByProject(
  sessions: readonly SessionRow[],
  hostFilter: string,
  showBgAgents: boolean,
  predicate: SessionPredicate = null,
  scope: ScopeFilter = null,
): Map<number, SessionRow[]> {
  const m = new Map<number, SessionRow[]>();
  for (const s of sessions) {
    if (s.kind === 'external') continue;
    if (s.project_id == null) continue;
    if (!sessionVisible(s, hostFilter, showBgAgents, predicate, scope)) continue;
    if (!m.has(s.project_id)) m.set(s.project_id, []);
    m.get(s.project_id)!.push(s);
  }
  return m;
}

/** Interactive Claude sessions running entirely outside fleet (Claude
 *  Desktop, a bare terminal), for the read-only "Outside fleet" group. The
 *  host filter applies (a hidden host's rows stay hidden); `showBgAgents`
 *  does not — that toggle only governs supervised `bg` agents. Sorted by
 *  `created_at` descending (set once, when fleet first saw the session —
 *  `last_activity_at` is rewritten on every reconcile pass), ties by id
 *  descending. */
export function buildOutsideFleet(
  sessions: readonly SessionRow[],
  hostFilter: string,
  scope: ScopeFilter = null,
): SessionRow[] {
  return sessions
    .filter(
      (s) =>
        s.kind === 'external' &&
        rowMatches(sessionFilterRow(s, scope?.of), { host: hostFilter, scope: scope?.id ?? 'all' }),
    )
    .slice()
    .sort((a, b) => b.created_at - a.created_at || b.id - a.id);
}

/** session.id → count of OTHER sessions sharing the same (project, worktree_key). */
export function buildRelatedCountById(sessions: readonly SessionRow[]): Map<number, number> {
  const grouped = new Map<string, SessionRow[]>();
  for (const s of sessions) {
    if (s.project_id == null || s.worktree_key == null) continue;
    const key = `${s.project_id}:${s.worktree_key}`;
    if (!grouped.has(key)) grouped.set(key, []);
    grouped.get(key)!.push(s);
  }
  const out = new Map<number, number>();
  for (const list of grouped.values()) {
    for (const s of list) out.set(s.id, list.length - 1);
  }
  return out;
}

/** Stable sort of project rows by the worst severity among their visible
 *  sessions (descending); ties keep the incoming order. `severityByProject`
 *  comes from `attention.worstSeverityByProject` over the VISIBLE sessions so
 *  a stuck session hidden by the host filter does not float its project. */
export function sortProjectsBySeverity<T extends { project: { id: number } }>(
  rows: readonly T[],
  severityByProject: ReadonlyMap<number, number>,
): T[] {
  return rows
    .map((row, i) => ({ row, i, sev: severityByProject.get(row.project.id) ?? -1 }))
    .sort((a, b) => b.sev - a.sev || a.i - b.i)
    .map((x) => x.row);
}

/** One work group of the sidebar's "group by work" mode. */
export interface WorkGroup {
  key: string;
  sessions: SessionRow[];
}

/** Sessions grouped by work key, for the sidebar's "group by work" mode.
 *
 *  Hybrid by design: only sessions that carry a key are grouped here.
 *  `keyed` names every such session — under ANY filter — so the caller can
 *  leave the rest under their project headers (no "Unclassified" bucket:
 *  ad-hoc work is not a defect). `groups` holds only the rows visible under
 *  the host / bg-agent / predicate filters, and drops a group that has none.
 *  Groups come out in first-seen order, i.e. the store's recency order;
 *  sorting by severity is the caller's (`sortWorkGroups`). */
export function buildSessionsByWork(
  sessions: readonly SessionRow[],
  hostFilter: string,
  showBgAgents: boolean,
  predicate: SessionPredicate,
  keyOf: (s: SessionRow) => WorkKey | null,
  scope: ScopeFilter = null,
): { groups: WorkGroup[]; keyed: Map<number, WorkKey> } {
  const keyed = new Map<number, WorkKey>();
  const byKey = new Map<string, SessionRow[]>();
  for (const s of sessions) {
    if (s.kind === 'external') continue;
    const k = keyOf(s);
    if (!k) continue;
    keyed.set(s.id, k);
    if (!sessionVisible(s, hostFilter, showBgAgents, predicate, scope)) continue;
    if (!byKey.has(k.key)) byKey.set(k.key, []);
    byKey.get(k.key)!.push(s);
  }
  return {
    groups: [...byKey.entries()].map(([key, rows]) => ({ key, sessions: rows })),
    keyed,
  };
}

/** Work groups by the worst severity among their sessions (descending);
 *  ties keep the incoming (recency) order — the same rule as projects. */
export function sortWorkGroups(
  groups: readonly WorkGroup[],
  severityOf: (s: SessionRow) => number,
): WorkGroup[] {
  return groups
    .map((g, i) => ({ g, i, sev: Math.max(-1, ...g.sessions.map(severityOf)) }))
    .sort((a, b) => b.sev - a.sev || a.i - b.i)
    .map((x) => x.g);
}
