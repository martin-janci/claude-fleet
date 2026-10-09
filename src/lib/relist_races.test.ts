// A full re-list against the frames that land while it is in flight
// (review r07): the list owns order and the untouched rows, a frame or a
// command row merged after the request keeps its state, and an older list
// never lands over a newer one.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { createRowStore } from './row_store';
import { sessions, loadSessions, applySessionEvents, resetTombstonesForTests, type SessionRow } from './sessions';
import { hosts, loadHosts, applyHostEvents, type HostRow } from './hosts';
import { loadMyGrants, applyGrantChanges, myGrants, setMyGrants } from './access';
import { projects, loadProjects, applyProjectEvents, type ProjectTreeRow } from './projects';
import { accountUsage, loadAccountUsage, applyAccountUsageEvents, type AccountUsageSnapshot } from './account_usage_store';

/** The next call of `cmd` waits until the returned release is called. */
function hold(cmd: string): (v: unknown) => void {
  let release!: (v: unknown) => void;
  vi.mocked(invoke).mockImplementation(async (c: string) =>
    c === cmd ? new Promise((r) => (release = r)) : null,
  );
  return (v) => release(v);
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  resetTombstonesForTests();
  sessions.set([]);
  hosts.set([]);
});

describe('row_store re-list', () => {
  type Row = { id: number; v: string };
  const make = () => createRowStore<Row, number>({ key: (r) => r.id, tombstoneMs: 5000 });

  it('takes order and untouched rows from the list, and keeps what a frame changed', () => {
    const rs = make();
    rs.store.set([
      { id: 1, v: 'a' },
      { id: 2, v: 'b' },
      { id: 3, v: 'c' },
    ]);
    const t = rs.beginList();
    rs.merge({ id: 2, v: 'B (frame)' });
    rs.merge({ id: 4, v: 'new (frame)' });
    rs.remove(3);
    expect(
      rs.applyList(
        [
          { id: 3, v: 'c' },
          { id: 2, v: 'b' },
          { id: 1, v: 'a (list)' },
        ],
        t,
      ),
    ).toBe(true);
    expect(get(rs.store)).toEqual([
      { id: 2, v: 'B (frame)' },
      { id: 1, v: 'a (list)' },
      { id: 4, v: 'new (frame)' },
    ]);
  });

  it('an older list answering after a newer one changes nothing', () => {
    const rs = make();
    const older = rs.beginList();
    const newer = rs.beginList();
    expect(rs.applyList([{ id: 1, v: 'new' }], newer)).toBe(true);
    expect(rs.applyList([{ id: 1, v: 'old' }], older)).toBe(false);
    expect(get(rs.store)).toEqual([{ id: 1, v: 'new' }]);
  });
});

describe('the stores that re-list', () => {
  const base = { id: 1, tmux_name: 'a', host_alias: 'local', row_version: 1 } as unknown as SessionRow;

  it('a session created while list_sessions is in flight survives the list', async () => {
    sessions.set([base]);
    const release = hold('list_sessions');
    const p = loadSessions();
    applySessionEvents([{ type: 'created', row: { ...base, id: 2, tmux_name: 'new', row_version: 1 } }]);
    release([base]);
    await p;
    expect(get(sessions).map((s) => s.id)).toEqual([1, 2]);
  });

  it('a host:probed applied while list_hosts is in flight survives the list', async () => {
    const h = { alias: 'm', reachable: false, claude_version: '1.0' } as unknown as HostRow;
    hosts.set([h]);
    const release = hold('list_hosts');
    const p = loadHosts();
    applyHostEvents([{ type: 'probed', row: { ...h, reachable: true, claude_version: '2.0' } }]);
    release([h]);
    await p;
    expect(get(hosts)[0].reachable).toBe(true);
  });

  it('a host removed while list_hosts is in flight stays removed', async () => {
    const h = { alias: 'm', reachable: true } as unknown as HostRow;
    hosts.set([h]);
    const release = hold('list_hosts');
    const p = loadHosts();
    applyHostEvents([{ type: 'removed', alias: 'm' }]);
    release([h]);
    await p;
    expect(get(hosts)).toEqual([]);
  });

  it('a revoke applied while my_grants is in flight stays revoked', async () => {
    setMyGrants(7, [{ session_id: 5, level: 'drive' }]);
    const release = hold('my_grants');
    const p = loadMyGrants();
    applyGrantChanges([{ session_id: 5, person_id: 7, level: null }]);
    release({ person_id: 7, grants: [{ session_id: 5, level: 'drive' }] });
    await p;
    expect(get(myGrants).has(5)).toBe(false);
  });

  it('a grant given while my_grants is in flight stays, and the rest comes from the answer', async () => {
    setMyGrants(7, [{ session_id: 5, level: 'drive' }]);
    const release = hold('my_grants');
    const p = loadMyGrants();
    applyGrantChanges([{ session_id: 6, person_id: 7, level: 'watch' }]);
    release({ person_id: 7, grants: [{ session_id: 8, level: 'drive' }] });
    await p;
    expect([...get(myGrants)].sort()).toEqual([
      [6, 'watch'],
      [8, 'drive'],
    ]);
  });

  it('a worktree removed while list_projects is in flight stays removed', async () => {
    const tree = (wts: number[]) =>
      ({
        project: { id: 3, owner: 'o', repo: 'r', base_path: '/p', last_session_at: null, adopted: false, system: false },
        worktrees: wts.map((id) => ({ id, project_id: 3, host_alias: 'local', name: `w${id}`, path: `/p/w${id}`, branch: null })),
      }) as ProjectTreeRow;
    projects.set([tree([1, 2])]);
    const release = hold('list_projects');
    const p = loadProjects();
    applyProjectEvents([{ type: 'worktree_removed', id: 2 }]);
    release([tree([1, 2])]);
    await p;
    expect(get(projects)[0].worktrees.map((w) => w.id)).toEqual([1]);
  });

  it('a usage snapshot that lands while list_account_usage is in flight survives the list', async () => {
    const snap = (at: number) => ({ account_uuid: 'u1', fetched_at: at, status: 'ok' }) as unknown as AccountUsageSnapshot;
    accountUsage.set({ u1: snap(1) });
    const release = hold('list_account_usage');
    const p = loadAccountUsage();
    applyAccountUsageEvents([snap(9)]);
    release([snap(1)]);
    await p;
    expect(get(accountUsage).u1.fetched_at).toBe(9);
  });
});
