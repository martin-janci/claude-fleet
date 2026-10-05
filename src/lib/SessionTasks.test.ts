// The session's Tasks section (work graph M14): every link grouped active
// (primary first) / suggested / past / rejected, Make primary as a
// compare-and-set, Remove with the link's version, Add task… that never
// takes an existing primary, conflicts, and "Show in Work view".
import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import SessionTasks from './SessionTasks.svelte';
import { sessions } from './sessions';
import { session } from './hosts_fixture';
import { link } from './work_view_fixture';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { applyGrantChanges, resetAccessForTests, setMyGrants } from './access';
import { bumpWorkChanged, selectedTaskId, sidebarView, taskDetailOpen, type SessionTasks as Tasks } from './work_view';

const ref = (task_id: string, key: string, title: string) => ({
  task_id,
  key,
  title,
  kind: 'tracker',
  status_category: 'in_progress',
  status_name: 'In Progress',
  url: null,
  unavailable: false,
  org_id: 1,
  tracker_name: 'Jira (acme)',
});

const answer: Tasks = {
  session_id: 7,
  org_id: 1,
  primary_link_id: 42,
  links: [
    { ...link({ link_id: 43, primary: false, link_version: 1 }), task: ref('item:13', 'ABC-13', 'Logout') },
    { ...link({ link_id: 42, primary: true, link_version: 3 }), task: ref('item:12', 'ABC-12', 'Login fails') },
    { ...link({ link_id: 44, state: 'suggested', primary: false, why: 'mentioned ABC-14 · R6' }), task: ref('item:14', 'ABC-14', 'Refund') },
    { ...link({ link_id: 40, state: 'ended', primary: false, ended_at: 1789990000 }), task: ref('ref:ABC-9', 'ABC-9', '') },
    { ...link({ link_id: 39, state: 'rejected', primary: false }), task: ref('ref:ABC-8', 'ABC-8', '') },
  ],
};

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;
const row = session('mefistos', 'api', { id: 7, work: { link_id: 42, item_id: 12, key: 'ABC-12', title: 'Login fails', source: 'manual' } });

function calls(cmd: string) {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);
}

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

describe('SessionTasks', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    sessions.set([row]);
    handlers = { work_session_tasks: () => answer };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
  });

  it('lists every link grouped, the primary first, each with its why', async () => {
    render(SessionTasks, { session: row });
    await flush();
    expect(calls('work_session_tasks')[0]).toEqual({ session_id: 7 });
    const rows = screen.getAllByTestId('session-task');
    expect(rows.map((r) => [r.getAttribute('data-kind'), r.getAttribute('data-link-id')])).toEqual([
      ['active', '42'],
      ['active', '43'],
      ['suggested', '44'],
      ['past', '40'],
      ['rejected', '39'],
    ]);
    expect(rows[0].textContent).toContain('primary');
    expect(within(rows[2]).getByTestId('session-task-why').textContent).toBe('mentioned ABC-14 · R6');
    // Make primary only on a secondary active link.
    expect(within(rows[0]).queryByTestId('session-task-make-primary')).toBeNull();
    expect(within(rows[1]).getByTestId('session-task-make-primary')).toBeTruthy();
  });

  it('Make primary is a compare-and-set on the current primary', async () => {
    handlers.set_primary_work = () => row;
    render(SessionTasks, { session: row });
    await flush();
    await fireEvent.click(within(screen.getAllByTestId('session-task')[1]).getByTestId('session-task-make-primary'));
    await flush();
    expect(calls('set_primary_work')[0]).toEqual({ session_id: 7, link_id: 43, expected_primary: 42 });
    expect(calls('work_session_tasks').length).toBe(2);
  });

  it('a conflict says so, names the current primary by its task, and reloads', async () => {
    handlers.set_primary_work = () => {
      throw { code: 'E_CONFLICT', message: 'primary changed', details: { session_id: 7, primary_link_id: 44 } };
    };
    render(SessionTasks, { session: row });
    await flush();
    await fireEvent.click(within(screen.getAllByTestId('session-task')[1]).getByTestId('session-task-make-primary'));
    await flush();
    expect(screen.getByTestId('session-tasks-notice').textContent).toContain('changed elsewhere');
    expect(screen.getByTestId('work-conflict-current').textContent).toBe('Now: primary is ABC-14 Refund');
    expect(calls('work_session_tasks').length).toBe(2);
    await fireEvent.click(screen.getByTestId('work-conflict-reload'));
    await flush();
    expect(calls('work_session_tasks').length).toBe(3);
  });

  it('Remove sends the link’s version', async () => {
    handlers.unlink_session_work = () => row;
    render(SessionTasks, { session: row });
    await flush();
    await fireEvent.click(within(screen.getAllByTestId('session-task')[1]).getByTestId('session-task-remove'));
    await flush();
    expect(calls('unlink_session_work')[0]).toEqual({ session_id: 7, link_id: 43, expected_version: 1 });
  });

  it('Add task… searches tickets and links one without taking the primary', async () => {
    handlers.work_tickets = () => [{ id: 15, key: 'ABC-15', title: 'Audit log', source: 'tracker', status_category: 'todo', created_at: 1, updated_at: 1 }];
    handlers.link_session_work = () => row;
    render(SessionTasks, { session: row });
    await flush();
    await fireEvent.click(screen.getByTestId('session-tasks-add'));
    await fireEvent.input(screen.getByTestId('session-tasks-query'), { target: { value: 'audit' } });
    await new Promise((r) => setTimeout(r, 300));
    await flush();
    expect(calls('work_tickets')[0]).toEqual({ query: 'audit', limit: 10 });
    await fireEvent.click(screen.getByTestId('session-tasks-result'));
    await flush();
    expect(calls('link_session_work')[0]).toEqual({ session_id: 7, item_id: 15, primary: false });
    expect(screen.getByTestId('session-tasks-notice').textContent).toBe('Added ABC-15');
  });

  it('a typed key is linked as the primary when the session has none; cross-org asks first', async () => {
    handlers.work_session_tasks = () => ({ ...answer, primary_link_id: null, links: [] });
    let refused = false;
    handlers.link_session_work = () => {
      if (!refused) {
        refused = true;
        throw { code: 'E_FORBIDDEN', message: 'cross-org', details: { cross_org: true, work_org_id: 2, session_org_id: 1 } };
      }
      return row;
    };
    render(SessionTasks, { session: row });
    await flush();
    expect(screen.getByTestId('session-tasks-empty')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('session-tasks-add'));
    await fireEvent.input(screen.getByTestId('session-tasks-query'), { target: { value: 'OPS-1' } });
    await fireEvent.click(screen.getByTestId('session-tasks-add-key'));
    await flush();
    expect(screen.getByTestId('session-tasks-cross-org').textContent).toContain('OPS-1 belongs to organisation 2');
    await fireEvent.click(screen.getByTestId('session-tasks-force'));
    await flush();
    expect(calls('link_session_work')).toEqual([
      { session_id: 7, key: 'OPS-1', primary: true },
      { session_id: 7, key: 'OPS-1', force_cross_org: true, primary: true },
    ]);
  });

  it('"Show in Work view" switches the sidebar and opens the task', async () => {
    sidebarView.set('sessions');
    render(SessionTasks, { session: row });
    await flush();
    await fireEvent.click(within(screen.getAllByTestId('session-task')[1]).getByTestId('session-task-show'));
    expect(get(sidebarView)).toBe('work');
    expect(get(selectedTaskId)).toBe('item:13');
    expect(get(taskDetailOpen)).toBe(true);
  });

  it('re-reads when a secondary link changed (work_rev), not on a status tick', async () => {
    const { rerender } = render(SessionTasks, { session: row, debounceMs: 5 });
    await flush();
    expect(calls('work_session_tasks').length).toBe(1);
    // The primary is the same; only the claude status moved: nothing to read.
    await rerender({ session: { ...row, claude_status: 'working' } });
    await flush();
    expect(calls('work_session_tasks').length).toBe(1);
    // A secondary link was added / removed elsewhere: the primary did not
    // move, `work_rev` did.
    await rerender({ session: { ...row, work_rev: 17 } });
    await new Promise((r) => setTimeout(r, 30));
    await flush();
    expect(calls('work_session_tasks').length).toBe(2);
  });

  it('a work_rev move and the tick it brings are one read; another session loads at once', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] });
    try {
      const { rerender } = render(SessionTasks, { session: row, debounceMs: 500 });
      await flush();
      expect(calls('work_session_tasks')).toHaveLength(1);
      // A `session:updated` moves `work_rev`: the row changes, and App bumps
      // the tick for it, within one debounce window.
      await rerender({ session: { ...row, work_rev: 18 } });
      bumpWorkChanged('session');
      vi.advanceTimersByTime(200);
      await flush();
      bumpWorkChanged();
      vi.advanceTimersByTime(600);
      await flush();
      expect(calls('work_session_tasks')).toHaveLength(2);
      // Another session: at once, without waiting for the debounce.
      await rerender({ session: { ...row, id: 8, work_rev: 18 } });
      await flush();
      expect(calls('work_session_tasks')).toHaveLength(3);
      expect(calls('work_session_tasks').at(-1)).toEqual({ session_id: 8 });
      vi.advanceTimersByTime(600);
      await flush();
      expect(calls('work_session_tasks')).toHaveLength(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it('stays out of the way on an older hub', async () => {
    handlers.work_session_tasks = () => {
      throw { code: 'E_INVALID', message: 'unknown work action: session_tasks' };
    };
    render(SessionTasks, { session: row });
    await flush();
    expect(screen.queryByTestId('session-tasks')).toBeNull();
    expect(screen.queryByTestId('session-tasks-error')).toBeNull();
  });
});

// Multi-user M1: `link_session_work` is `drive` in `share.ts::SESSION_TIER`,
// and `SessionRowItem` has composed both halves — the hub's refusal and this
// client's access to the row — for the same action since F2. This panel asked
// the hub's half only, so a watcher could still unlink, confirm or reject the
// owner's work from here: the same control, two surfaces, two answers.
describe('SessionTasks access gate (multi-user M1)', () => {
  const REMOTE = {
    ...STANDALONE,
    remote: true,
    url: 'https://fleet.example.com',
    configured_url: 'https://fleet.example.com',
  };

  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    handlers = { work_session_tasks: () => answer };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
    resetAccessForTests();
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connected' });
  });

  afterEach(() => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    resetAccessForTests();
  });

  const theirs = { ...row, owner_person_id: 9 };

  it('a watch grantee cannot unlink, confirm, reject or add a task', async () => {
    sessions.set([theirs]);
    setMyGrants(7, [{ session_id: 7, level: 'watch' }]);
    render(SessionTasks, { session: theirs });
    await flush();
    for (const id of ['session-task-remove', 'session-task-confirm', 'session-task-reject']) {
      const btns = screen.queryAllByTestId(id) as HTMLButtonElement[];
      expect(btns.length, id).toBeGreaterThan(0);
      for (const b of btns) expect(b.disabled, id).toBe(true);
    }
    const add = screen.getByTestId('session-tasks-add') as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    expect(add.title).toMatch(/watch is read-only/i);
    await fireEvent.click(screen.queryAllByTestId('session-task-remove')[0]);
    await flush();
    expect(calls('work_link')).toEqual([]);
  });

  it('a drive grantee keeps them — a work link is a write, not a disposal', async () => {
    sessions.set([theirs]);
    setMyGrants(7, [{ session_id: 7, level: 'drive' }]);
    render(SessionTasks, { session: theirs });
    await flush();
    expect((screen.getByTestId('session-tasks-add') as HTMLButtonElement).disabled).toBe(false);
    for (const b of screen.queryAllByTestId('session-task-remove') as HTMLButtonElement[]) {
      expect(b.disabled).toBe(false);
    }
  });

  // F2a: `primaryBlocked` was the one line of this file still asking the hub's
  // half alone — three lines under a `linkBlocked` that composed both and
  // carried the comment saying why. Make primary is `set_primary_work`, `drive`
  // in `share.ts::SESSION_TIER`.
  it('Make primary: the owner keeps it, a watcher does not, a driver does', async () => {
    const mine = { ...row, owner_person_id: 7 };
    // The positive control first, so a gate that disabled it for everyone fails.
    sessions.set([mine]);
    setMyGrants(7, []);
    const own = render(SessionTasks, { session: mine });
    await flush();
    const ownBtns = screen.queryAllByTestId('session-task-make-primary') as HTMLButtonElement[];
    expect(ownBtns.length).toBeGreaterThan(0);
    for (const b of ownBtns) expect(b.disabled).toBe(false);
    own.unmount();

    sessions.set([theirs]);
    setMyGrants(7, [{ session_id: 7, level: 'watch' }]);
    const watch = render(SessionTasks, { session: theirs });
    await flush();
    const watchBtns = screen.queryAllByTestId('session-task-make-primary') as HTMLButtonElement[];
    expect(watchBtns.length).toBeGreaterThan(0);
    for (const b of watchBtns) {
      expect(b.disabled).toBe(true);
      expect(b.title).toMatch(/watch is read-only/i);
    }
    await fireEvent.click(watchBtns[0]);
    await flush();
    expect(calls('work_link')).toEqual([]);
    watch.unmount();

    setMyGrants(7, [{ session_id: 7, level: 'drive' }]);
    render(SessionTasks, { session: theirs });
    await flush();
    for (const b of screen.queryAllByTestId('session-task-make-primary') as HTMLButtonElement[]) {
      expect(b.disabled).toBe(false);
    }
  });

  // F2b: the panel's controls were disabled, but `act` and `add` trusted them.
  // The panel stays open across a narrowing, so this checks the narrowing
  // reaches the open panel (the controls re-render disabled, with the reason,
  // and nothing is sent). The handler's own re-ask — `if (busy || linkBlocked
  // !== null) return;` inside `act` and `add` — is what a disabled button
  // cannot prove, because the browser never calls the handler of one; that half
  // is held by `share_sweep.test.ts`, which fails for a write with no access
  // answer in reach of it.
  it('a grant narrowed while the panel is open reaches it, and nothing is sent', async () => {
    sessions.set([theirs]);
    setMyGrants(7, [{ session_id: 7, level: 'drive' }]);
    render(SessionTasks, { session: theirs });
    await flush();
    const remove = screen.queryAllByTestId('session-task-remove')[0] as HTMLButtonElement;
    expect(remove.disabled).toBe(false);
    applyGrantChanges([{ session_id: 7, person_id: 7, level: 'watch' }]);
    await flush();
    const after = screen.queryAllByTestId('session-task-remove')[0] as HTMLButtonElement;
    expect(after.disabled).toBe(true);
    expect(after.title).toMatch(/watch is read-only/i);
    await fireEvent.click(after);
    await flush();
    expect(calls('work_link')).toEqual([]);
    // The Add task… entrance goes with it.
    expect((screen.getByTestId('session-tasks-add') as HTMLButtonElement).disabled).toBe(true);
  });

  it('a standalone desktop is untouched', async () => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    sessions.set([theirs]);
    render(SessionTasks, { session: theirs });
    await flush();
    expect((screen.getByTestId('session-tasks-add') as HTMLButtonElement).disabled).toBe(false);
  });
});
