import { describe, it, expect } from 'vitest';
import {
  buildOutsideFleet,
  buildSessionsByProject,
  buildSessionsByWork,
  sessionVisible,
  sortProjectsBySeverity,
  sortWorkGroups,
} from './sidebar_index';
import { workKeyFor, type WorkKey } from './work_keys';
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
    ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
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

  it('excludes external rows even when they carry a project_id', () => {
    const external = row({ kind: 'external', project_id: 1 });
    const plain = row({ project_id: 1 });
    const all = buildSessionsByProject([external, plain], 'all', true);
    expect(all.get(1)?.map((s) => s.id)).toEqual([plain.id]);
  });
});

describe('buildOutsideFleet', () => {
  it('returns only external rows, respects the host filter, ignores the bg toggle, sorted by created_at desc', () => {
    // last_activity_at is rewritten to "now" on every reconcile pass, so it
    // must not drive the order; created_at is set once, on insert.
    const extOld = row({ kind: 'external', host_alias: 'local', created_at: 10, last_activity_at: 99 });
    const extNew = row({ kind: 'external', host_alias: 'local', created_at: 30, last_activity_at: 1 });
    const extRemote = row({ kind: 'external', host_alias: 'mefistos', created_at: 20, last_activity_at: 50 });
    const bg = row({ kind: 'bg', host_alias: 'local', created_at: 40 });
    const work = row({ kind: 'work', host_alias: 'local', created_at: 50 });

    const all = buildOutsideFleet([extOld, extNew, extRemote, bg, work], 'all');
    expect(all.map((s) => s.id)).toEqual([extNew.id, extRemote.id, extOld.id]);

    const local = buildOutsideFleet([extOld, extNew, extRemote, bg, work], 'local');
    expect(local.map((s) => s.id)).toEqual([extNew.id, extOld.id]);
  });

  it('breaks a created_at tie by id, newest first', () => {
    const a = row({ kind: 'external', created_at: 5 });
    const b = row({ kind: 'external', created_at: 5 });
    expect(buildOutsideFleet([a, b], 'all').map((s) => s.id)).toEqual([b.id, a.id]);
    expect(buildOutsideFleet([b, a], 'all').map((s) => s.id)).toEqual([b.id, a.id]);
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

describe('buildSessionsByWork / sortWorkGroups', () => {
  const keyOf = (s: SessionRow): WorkKey | null =>
    s.tags.length ? { key: s.tags[0], source: 'tag', from: s.tags[0] } : null;

  it('groups keyed sessions and leaves the rest to the project tree', () => {
    const a1 = row({ tags: ['ABC-1'] });
    const a2 = row({ tags: ['ABC-1'], host_alias: 'mefistos' });
    const b = row({ tags: ['DEF-2'] });
    const plain = row();
    const ext = row({ kind: 'external', tags: ['ABC-1'] });
    const { groups, keyed } = buildSessionsByWork([a1, plain, b, a2, ext], 'all', true, null, keyOf);
    expect(groups.map((g) => [g.key, g.sessions.map((s) => s.id)])).toEqual([
      ['ABC-1', [a1.id, a2.id]],
      ['DEF-2', [b.id]],
    ]);
    expect([...keyed.keys()].sort()).toEqual([a1.id, a2.id, b.id].sort());
  });

  it('a suggestion never regroups: only a confirmed link makes a work group (M4.4)', () => {
    const suggested = row({
      tags: ['ABC-1'],
      work_suggested: {
        link_id: 3, item_id: null, key: 'ABC-1', title: '', source: 'branch', state: 'suggested',
      },
    });
    const linked = row({
      work: { link_id: 4, item_id: null, key: 'ABC-1', title: '', source: 'branch', state: 'confirmed' },
    });
    const real = (s: SessionRow) => workKeyFor(s, new Map());
    const { groups, keyed } = buildSessionsByWork([suggested, linked], 'all', true, null, real);
    expect(groups.map((g) => [g.key, g.sessions.map((s) => s.id)])).toEqual([['ABC-1', [linked.id]]]);
    expect(keyed.has(suggested.id)).toBe(false);
  });

  it('filters rows but still reports every keyed session', () => {
    const a1 = row({ tags: ['ABC-1'] });
    const a2 = row({ tags: ['ABC-1'], host_alias: 'mefistos' });
    const { groups, keyed } = buildSessionsByWork([a1, a2], 'mefistos', true, null, keyOf);
    expect(groups).toHaveLength(1);
    expect(groups[0].sessions.map((s) => s.id)).toEqual([a2.id]);
    // a1 is hidden by the host filter, but it is still keyed: it must not
    // reappear under its project header.
    expect(keyed.has(a1.id)).toBe(true);
  });

  it('drops a group with no visible session', () => {
    const bg = row({ tags: ['ABC-1'], kind: 'bg' });
    const { groups, keyed } = buildSessionsByWork([bg], 'all', false, null, keyOf);
    expect(groups).toEqual([]);
    expect(keyed.has(bg.id)).toBe(true);
  });

  it('sorts groups by worst severity, keeping recency order on ties', () => {
    const g = (key: string, ...sev: number[]) => ({
      key,
      sessions: sev.map((n) => row({ context_pct: n })),
    });
    const sorted = sortWorkGroups([g('A', 1), g('B', 3, 0), g('C', 1)], (s) => s.context_pct ?? 0);
    expect(sorted.map((x) => x.key)).toEqual(['B', 'A', 'C']);
  });
});
