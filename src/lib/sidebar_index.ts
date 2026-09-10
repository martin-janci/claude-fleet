// Pure index builders for Sidebar.svelte.
//
// Sidebar renders one row per session; both builders must run ONCE per
// `$sessions` change (inside a `$derived`), never per row — otherwise the
// tree goes quadratic at a few hundred sessions. They live in their own
// module so Sidebar.test.ts can spy on them and assert the call count
// deterministically instead of measuring jsdom wall-clock.
import type { SessionRow } from './sessions';

/** project_id → sessions visible under the current host / bg-agent filter. */
export function buildSessionsByProject(
  sessions: readonly SessionRow[],
  hostFilter: string,
  showBgAgents: boolean,
): Map<number, SessionRow[]> {
  const m = new Map<number, SessionRow[]>();
  for (const s of sessions) {
    if (s.project_id == null) continue;
    if (hostFilter !== 'all' && s.host_alias !== hostFilter) continue;
    if (!showBgAgents && s.kind === 'bg') continue;
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
