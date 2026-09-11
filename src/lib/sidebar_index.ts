// Pure index builders for Sidebar.svelte.
//
// Sidebar renders one row per session; both builders must run ONCE per
// `$sessions` change (inside a `$derived`), never per row — otherwise the
// tree goes quadratic at a few hundred sessions. They live in their own
// module so Sidebar.test.ts can spy on them and assert the call count
// deterministically instead of measuring jsdom wall-clock.
import type { SessionRow } from './sessions';

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
    if (s.project_id == null) continue;
    if (!sessionVisible(s, hostFilter, showBgAgents, predicate)) continue;
    if (!m.has(s.project_id)) m.set(s.project_id, []);
    m.get(s.project_id)!.push(s);
  }
  return m;
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
