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

/** True when the row passes the host, bg-agent and optional extra filters. */
export function sessionVisible(
  s: SessionRow,
  hostFilter: string,
  showBgAgents: boolean,
  predicate: SessionPredicate = null,
): boolean {
  if (hostFilter !== 'all' && s.host_alias !== hostFilter) return false;
  if (!showBgAgents && s.kind === 'bg') return false;
  if (predicate && !predicate(s)) return false;
  return true;
}

/** project_id → sessions visible under the current host / bg-agent filter
 *  (plus the optional predicate). */
export function buildSessionsByProject(
  sessions: readonly SessionRow[],
  hostFilter: string,
  showBgAgents: boolean,
  predicate: SessionPredicate = null,
): Map<number, SessionRow[]> {
  const m = new Map<number, SessionRow[]>();
  for (const s of sessions) {
    if (s.kind === 'external') continue;
    if (s.project_id == null) continue;
    if (!sessionVisible(s, hostFilter, showBgAgents, predicate)) continue;
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
): SessionRow[] {
  return sessions
    .filter((s) => s.kind === 'external' && (hostFilter === 'all' || s.host_alias === hostFilter))
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
): { groups: WorkGroup[]; keyed: Map<number, WorkKey> } {
  const keyed = new Map<number, WorkKey>();
  const byKey = new Map<string, SessionRow[]>();
  for (const s of sessions) {
    if (s.kind === 'external') continue;
    const k = keyOf(s);
    if (!k) continue;
    keyed.set(s.id, k);
    if (!sessionVisible(s, hostFilter, showBgAgents, predicate)) continue;
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
