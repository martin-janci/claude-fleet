import { describe, it, expect } from 'vitest';
import { buildSessionsByProject, sessionVisible, sortProjectsBySeverity } from './sidebar_index';
import type { SessionRow } from './sessions';

let nextId = 1;
function row(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: nextId++,
    tmux_name: 'dev-x',
    host_alias: 'local',
    project_id: 1,
    worktree_id: null,
    created_at: 1,
    last_activity_at: 1,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: 'main',
    lost_at: null,
    claude_session_id: null,
    claude_status: null,
    effort_level: null,
    pr_url: null,
    current_activity: null,
    context_pct: null,
    stuck_kind: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null,
    idle_since: null,
    stuck_since: null,
    last_playbook_at: null,
    last_prompt: null,
    started_at: null,
    last_turn_at: null,
    ci_status: null,
    ...over,
  };
}

describe('sessionVisible / buildSessionsByProject', () => {
  it('applies host, bg and the optional predicate', () => {
    const stuck = row({ stuck_kind: 'oom' });
    const plain = row();
    const remote = row({ host_alias: 'mefistos' });
    const bg = row({ kind: 'bg' });
    expect(sessionVisible(plain, 'all', true)).toBe(true);
    expect(sessionVisible(remote, 'local', true)).toBe(false);
    expect(sessionVisible(bg, 'all', false)).toBe(false);
    expect(sessionVisible(plain, 'all', true, (s) => s.stuck_kind !== null)).toBe(false);
    expect(sessionVisible(stuck, 'all', true, (s) => s.stuck_kind !== null)).toBe(true);

    const m = buildSessionsByProject([stuck, plain, remote, bg], 'all', true, (s) => s.stuck_kind !== null);
    expect(m.get(1)?.map((s) => s.id)).toEqual([stuck.id]);
    const all = buildSessionsByProject([stuck, plain, remote, bg], 'all', true);
    expect(all.get(1)).toHaveLength(4);
  });
});

describe('sortProjectsBySeverity', () => {
  it('sorts worst-first and keeps the incoming order for ties/unknowns', () => {
    const rows = [
      { project: { id: 1 } },
      { project: { id: 2 } },
      { project: { id: 3 } },
      { project: { id: 4 } },
    ];
    const sev = new Map<number, number>([
      [2, 6],
      [3, 2],
      [4, 6],
    ]);
    expect(sortProjectsBySeverity(rows, sev).map((r) => r.project.id)).toEqual([2, 4, 3, 1]);
    // No severities at all ⇒ untouched.
    expect(sortProjectsBySeverity(rows, new Map()).map((r) => r.project.id)).toEqual([1, 2, 3, 4]);
  });
});
