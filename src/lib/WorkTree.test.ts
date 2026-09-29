// The Work view's tree (work graph M14): section headers from `groups`, the
// per-section read and its paging, occurrences of one session lit
// everywhere, and the explicit loading / empty / error states.
import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkTree from './WorkTree.svelte';
import { sessions } from './sessions';
import { selectedSession, selectSession, clearSelection } from './selection';
import { session } from './hosts_fixture';
import { link, task } from './work_view_fixture';
import {
  activeWorkViewId,
  bumpWorkChanged,
  NEWER_HUB,
  revealTaskRequest,
  selectedTaskId,
  showTaskInWorkView,
  taskDetailOpen,
  workExpanded,
  workViewFilters,
  type WorkTreeFilters,
  type WorkTreePage,
} from './work_view';

const ABC = { id: 'tracker:1:ABC', label: 'ABC', source: 'tracker', rule_id: null, tracker_value: 'ABC' };
const PAY = { id: 'label:Payments', label: 'Payments', source: 'rule', rule_id: 3 };
const NONE = { id: 'none', label: 'No group', source: 'none' };

const firstPage: WorkTreePage = {
  tasks: [
    task({
      task_id: 'item:12',
      needs_you: true,
      sessions: [
        link({ link_id: 42, session_id: 7, primary: true }),
        link({ link_id: 45, session_id: 9, name: 'web', primary: false, state: 'suggested' }),
        link({ link_id: 46, session_id: null, name: 'old', state: 'ended', ended_at: 1790000000 }),
      ],
    }),
    task({
      task_id: 'item:13',
      key: 'ABC-13',
      title: 'Logout <b>broken</b>',
      review: true,
      unavailable: true,
      unavailable_reason: 'not_found_or_no_permission',
      tracker_state: 'unreachable',
      sessions: [link({ link_id: 43, session_id: 7, primary: false, name: 'ABC-12 login' })],
    }),
  ],
  groups: [
    { org_id: 1, org_name: 'Acme', group: ABC, count: 2 },
    { org_id: 1, org_name: 'Acme', group: PAY, count: 3 },
    { org_id: null, group: NONE, count: 1 },
  ],
  orgs: [{ id: 1, name: 'Acme', color: '#3a7' }],
  trackers: [{ id: 1, name: 'Jira (acme)', provider: 'jira', state: 'ok', org_id: 1 }],
  total: 6,
  next_cursor: 'p2',
  generated_at: 1790000300,
};

type Args = { args: { filters?: WorkTreeFilters; cursor?: string; limit?: number } };
let treeImpl: (a: Args['args']) => unknown;

function mockHub() {
  vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
    const a = (raw as Args | undefined)?.args ?? {};
    if (cmd === 'work_tree') return treeImpl(a);
    if (cmd === 'work_review') return { items: [], total: 4, next_cursor: null };
    if (cmd === 'work_views') return [];
    return null;
  });
}

function treeCalls() {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === 'work_tree')
    .map((c) => (c[1] as Args).args);
}

async function flush() {
  for (let i = 0; i < 8; i++) await tick();
}

describe('WorkTree', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    clearSelection();
    workExpanded.set({});
    activeWorkViewId.set(null);
    workViewFilters.set({});
    selectedTaskId.set(null);
    taskDetailOpen.set(false);
    revealTaskRequest.set(null);
    sessions.set([session('mefistos', 'api', { id: 7 }), session('mefistos', 'web', { id: 9 })]);
    treeImpl = (a) => {
      if (a.filters?.group === 'label:Payments') {
        if (a.cursor === 'pay2') {
          return { ...firstPage, tasks: [task({ task_id: 'item:32', key: 'PAY-3', group: PAY, sessions: [] })], next_cursor: null };
        }
        return {
          ...firstPage,
          tasks: [
            task({ task_id: 'item:30', key: 'PAY-1', title: 'Refunds', group: PAY, sessions: [] }),
            task({ task_id: 'item:31', key: 'PAY-2', title: 'Receipts', group: PAY, sessions: [link({ link_id: 60, session_id: 7, primary: false })] }),
          ],
          next_cursor: 'pay2',
        };
      }
      return firstPage;
    };
    mockHub();
  });

  it('draws every section header with its count and the first page’s tasks', async () => {
    render(WorkTree);
    await flush();
    const first = treeCalls()[0];
    expect(first).toEqual({ filters: { archived: false }, limit: 50 });
    const orgs = screen.getAllByTestId('work-org');
    expect(orgs.map((o) => within(o).getByTestId('work-org-head').textContent?.replace(/\s+/g, ' ').trim())).toEqual([
      '▸ Acme 5',
      '▸ Unassigned 1',
    ]);
    const counts = screen.getAllByTestId('work-group-count').map((c) => c.textContent);
    expect(counts).toEqual(['2', '3', '1']);
    // The first section is filled and open; the others wait to be opened.
    const tasks = screen.getAllByTestId('work-task');
    expect(tasks.map((t) => t.getAttribute('data-task-id'))).toEqual(['item:12', 'item:13']);
    // Badges: needs you, review, unavailable struck through, tracker down.
    expect(within(tasks[0]).getByTestId('work-task-needs-you')).toBeTruthy();
    expect(within(tasks[1]).getByTestId('work-task-review').textContent).toBe('?');
    expect(tasks[1].querySelector('.tlabel.unavailable')).toBeTruthy();
    expect(within(tasks[1]).getByTestId('work-task-tracker-down')).toBeTruthy();
    expect(within(tasks[0]).getByTestId('work-task-counts').textContent).toBe('1 active · 2 past');
    // Tracker text is text, never markup.
    expect(screen.getByText('Logout <b>broken</b>')).toBeTruthy();
    // Occurrence kinds: primary ★, suggested (dashed, "?"), past (ended).
    const occ = within(tasks[0]).getAllByTestId('work-occurrence');
    expect(occ.map((o) => o.getAttribute('data-kind'))).toEqual(['primary', 'suggested', 'past']);
    expect(occ[2].textContent).toContain('ended');
    // The Review tab carries its count.
    expect(screen.getByTestId('work-tab-review').textContent).toContain('4');
  });

  it('opening a section loads it by itself; Load more pages with its own cursor', async () => {
    render(WorkTree);
    await flush();
    const heads = screen.getAllByTestId('work-group-head');
    await fireEvent.click(heads[1]);
    await flush();
    const sec = treeCalls().at(-1)!;
    expect(sec).toEqual({ filters: { org: 1, group: 'label:Payments', archived: false }, limit: 50 });
    const group = screen.getAllByTestId('work-group')[1];
    expect(within(group).getAllByTestId('work-task').map((t) => t.getAttribute('data-task-id'))).toEqual(['item:30', 'item:31']);
    await fireEvent.click(within(group).getByTestId('work-load-more'));
    await flush();
    expect(treeCalls().at(-1)).toEqual({ filters: { org: 1, group: 'label:Payments', archived: false }, cursor: 'pay2', limit: 50 });
    expect(within(group).getAllByTestId('work-task').map((t) => t.getAttribute('data-task-id'))).toEqual([
      'item:30',
      'item:31',
      'item:32',
    ]);
    expect(within(group).queryByTestId('work-load-more')).toBeNull();
    // The expansion is kept for the view.
    expect(get(workExpanded).custom?.['1|label:Payments']).toBe(true);
  });

  it('lights every occurrence of the selected session, in every task', async () => {
    selectSession(get(sessions)[0]);
    render(WorkTree);
    await flush();
    const lit = screen.getAllByTestId('work-occurrence').filter((o) => o.classList.contains('current'));
    expect(lit.map((o) => o.getAttribute('data-session-id'))).toEqual(['7', '7']);
    expect(screen.getAllByTestId('work-task').filter((t) => t.classList.contains('lit'))).toHaveLength(2);
  });

  it('an occurrence opens the same session; a past one opens the task', async () => {
    render(WorkTree);
    await flush();
    const occ = screen.getAllByTestId('work-occurrence');
    await fireEvent.click(occ[3]); // ABC-13's secondary link of session 7
    expect(get(selectedSession)?.id).toBe(7);
    expect(get(selectedTaskId)).toBe('item:13');
    expect(get(taskDetailOpen)).toBe(false);
    await fireEvent.click(occ[2]); // ended
    expect(get(selectedTaskId)).toBe('item:12');
    expect(get(taskDetailOpen)).toBe(true);
  });

  it('a task row selects it and opens its detail', async () => {
    render(WorkTree);
    await flush();
    await fireEvent.click(screen.getAllByTestId('work-task-row')[1]);
    expect(get(selectedTaskId)).toBe('item:13');
    expect(get(taskDetailOpen)).toBe(true);
    await flush();
    expect(screen.getAllByTestId('work-task')[1].classList.contains('selected')).toBe(true);
  });

  it('empty, error with Retry, and an older hub', async () => {
    treeImpl = () => ({ ...firstPage, tasks: [], groups: [], total: 0 });
    const { unmount } = render(WorkTree);
    expect(screen.getByTestId('work-tree-loading')).toBeTruthy();
    await flush();
    expect(screen.getByTestId('work-tree-empty')).toBeTruthy();
    unmount();

    treeImpl = () => {
      throw { code: 'E_HUB', message: 'hub unreachable' };
    };
    const second = render(WorkTree);
    await flush();
    expect(screen.getByTestId('work-tree-error').textContent).toContain('hub unreachable');
    treeImpl = () => firstPage;
    await fireEvent.click(screen.getByTestId('work-tree-retry'));
    await flush();
    expect(screen.getAllByTestId('work-task')).toHaveLength(2);
    second.unmount();

    treeImpl = () => {
      throw { code: 'E_INVALID', message: 'unknown work action: tree' };
    };
    render(WorkTree);
    await flush();
    expect(screen.getByTestId('work-tree-error').textContent).toContain(NEWER_HUB);
  });

  it('archived tasks: hidden by default with a count, one click shows them, and back', async () => {
    treeImpl = (a) => ({ ...firstPage, archived_hidden: a.filters?.archived ? 0 : 5 });
    render(WorkTree);
    await flush();
    expect(treeCalls().at(-1)?.filters).toEqual({ archived: false });
    expect(screen.getByTestId('work-archived-row').textContent).toContain('5 archived tasks hidden');
    await fireEvent.click(screen.getByTestId('work-archived-toggle'));
    await flush();
    expect(get(workViewFilters)).toEqual({ archived: true });
    expect(treeCalls().at(-1)?.filters).toEqual({ archived: true });
    expect(screen.getByTestId('work-archived-row').textContent).toContain('Showing archived tasks');
    await fireEvent.click(screen.getByTestId('work-archived-toggle'));
    await flush();
    expect(get(workViewFilters)).toEqual({});
  });

  it('when every match is archived, the empty state offers them', async () => {
    treeImpl = (a) => (a.filters?.archived ? firstPage : { ...firstPage, tasks: [], groups: [], total: 0, archived_hidden: 2 });
    render(WorkTree);
    await flush();
    expect(screen.getByTestId('work-tree-empty').textContent).not.toContain('No work yet');
    await fireEvent.click(screen.getByTestId('work-archived-toggle'));
    await flush();
    expect(screen.getAllByTestId('work-task')).toHaveLength(2);
  });

  it('filters reload the view; work:changed re-reads it once, debounced', async () => {
    render(WorkTree, { debounceMs: 5 });
    await flush();
    const n = treeCalls().length;
    workViewFilters.set({ status: 'open' });
    await flush();
    expect(treeCalls().at(-1)).toEqual({ filters: { status: 'open', archived: false }, limit: 50 });
    const m = treeCalls().length;
    expect(m).toBe(n + 1);
    bumpWorkChanged();
    bumpWorkChanged();
    bumpWorkChanged();
    await new Promise((r) => setTimeout(r, 30));
    await flush();
    expect(treeCalls().length).toBe(m + 1);
  });

  it('"Show in Work view" opens the task’s section, loading it when needed', async () => {
    render(WorkTree);
    await flush();
    expect(screen.queryByText('Receipts')).toBeNull();
    // Not in the first page: the task's own read says where it lives.
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const a = (raw as Args | undefined)?.args ?? {};
      if (cmd === 'work_task') return { task: task({ task_id: 'item:31', group: PAY, org_id: 1 }), rules: [] };
      if (cmd === 'work_tree') return treeImpl(a);
      if (cmd === 'work_review') return { items: [], total: 0, next_cursor: null };
      return [];
    });
    showTaskInWorkView('item:31');
    await flush();
    await flush();
    expect(treeCalls().at(-1)).toEqual({ filters: { org: 1, group: 'label:Payments', archived: false }, limit: 50 });
    expect(screen.getByText('Receipts')).toBeTruthy();
    expect(get(workExpanded).custom?.['1|label:Payments']).toBe(true);
    expect(get(selectedTaskId)).toBe('item:31');
  });

  it('"Show in Work view" made before the tree mounts is revealed once it has loaded', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const a = (raw as Args | undefined)?.args ?? {};
      if (cmd === 'work_task') return { task: task({ task_id: 'item:31', group: PAY, org_id: 1 }), rules: [] };
      if (cmd === 'work_tree') return treeImpl(a);
      if (cmd === 'work_review') return { items: [], total: 0, next_cursor: null };
      return [];
    });
    // The Sessions view is showing: the request switches the sidebar, which
    // mounts the tree only afterwards.
    showTaskInWorkView('item:31');
    render(WorkTree);
    await flush();
    await flush();
    expect(treeCalls().at(-1)).toEqual({ filters: { org: 1, group: 'label:Payments', archived: false }, limit: 50 });
    expect(screen.getByText('Receipts')).toBeTruthy();
    expect(get(workExpanded).custom?.['1|label:Payments']).toBe(true);
    // Consumed: a later mount does not reveal it again.
    expect(get(revealTaskRequest)).toBeNull();
  });

  it('a refresh while a section is loading re-issues it; the section never stays loading', async () => {
    const base = treeImpl;
    let release: (() => void) | null = null;
    let held = 0;
    treeImpl = (a) => {
      if (a.filters?.group === 'label:Payments' && held++ === 0) {
        return new Promise((res) => {
          release = () => res(base(a));
        });
      }
      return base(a);
    };
    render(WorkTree, { debounceMs: 5 });
    await flush();
    await fireEvent.click(screen.getAllByTestId('work-group-head')[1]);
    await flush();
    expect(screen.getByTestId('work-section-loading')).toBeTruthy();
    // A refresh while the section's read is in flight.
    bumpWorkChanged();
    await new Promise((r) => setTimeout(r, 30));
    await flush();
    const sectionReads = () => treeCalls().filter((a) => a.filters?.group === 'label:Payments');
    expect(sectionReads()).toHaveLength(2);
    const group = () => screen.getAllByTestId('work-group')[1];
    const ids = () => within(group()).getAllByTestId('work-task').map((t) => t.getAttribute('data-task-id'));
    expect(screen.queryByTestId('work-section-loading')).toBeNull();
    expect(ids()).toEqual(['item:30', 'item:31']);
    // The cancelled read answers late: dropped, the section stays loaded.
    release!();
    await flush();
    expect(screen.queryByTestId('work-section-loading')).toBeNull();
    expect(ids()).toEqual(['item:30', 'item:31']);
    expect(within(group()).getByTestId('work-load-more')).toBeTruthy();
  });
  it('a failed refresh keeps the tree shown, with a line to retry; new filters that fail show the error', async () => {
    render(WorkTree, { debounceMs: 5 });
    await flush();
    const ids = () => screen.getAllByTestId('work-task').map((t) => t.getAttribute('data-task-id'));
    expect(ids()).toEqual(['item:12', 'item:13']);
    treeImpl = () => {
      throw { code: 'E_HUB_DOWN', message: 'hub unreachable' };
    };
    bumpWorkChanged();
    await new Promise((r) => setTimeout(r, 30));
    await flush();
    expect(ids()).toEqual(['item:12', 'item:13']);
    expect(screen.queryByTestId('work-tree-error')).toBeNull();
    expect(screen.getByTestId('work-tree-refresh-error').textContent).toContain('hub unreachable');
    // Other filters: what is shown is not theirs, so the error replaces it.
    workViewFilters.set({ mine: true });
    await flush();
    expect(screen.getByTestId('work-tree-error').textContent).toContain('hub unreachable');
  });

  it('a refresh re-reads a section in pages up to what was loaded, past one page', async () => {
    const PAY_BIG = { ...firstPage.groups[1], count: 300 };
    const payTasks = Array.from({ length: 300 }, (_, i) =>
      task({ task_id: `item:${1000 + i}`, key: `PAY-${i}`, title: `t${i}`, group: PAY, sessions: [] }),
    );
    treeImpl = (a) => {
      const page = { ...firstPage, groups: [firstPage.groups[0], PAY_BIG, firstPage.groups[2]] };
      if (a.filters?.group !== 'label:Payments') return page;
      const from = a.cursor ? Number(a.cursor) : 0;
      const to = Math.min(payTasks.length, from + (a.limit ?? 50));
      return { ...page, tasks: payTasks.slice(from, to), next_cursor: to < payTasks.length ? String(to) : null };
    };
    render(WorkTree, { debounceMs: 5, pageSize: 200 });
    await flush();
    await fireEvent.click(screen.getAllByTestId('work-group-head')[1]);
    await flush();
    await fireEvent.click(screen.getByTestId('work-load-more'));
    await flush();
    const payGroup = () => screen.getAllByTestId('work-group')[1];
    expect(within(payGroup()).getAllByTestId('work-task')).toHaveLength(300);
    const before = treeCalls().length;
    bumpWorkChanged();
    await new Promise((r) => setTimeout(r, 30));
    await flush();
    const reads = treeCalls().slice(before).filter((a) => a.filters?.group === 'label:Payments');
    expect(reads.map((a) => [a.cursor ?? null, a.limit])).toEqual([
      [null, 200],
      ['200', 100],
    ]);
    expect(within(payGroup()).getAllByTestId('work-task')).toHaveLength(300);
  });

  it('a steady stream of changes still refreshes within the max wait', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] });
    try {
      render(WorkTree, { debounceMs: 500, maxWaitMs: 3000 });
      await flush();
      const reads = () => treeCalls().filter((a) => !a.filters?.group).length;
      expect(reads()).toBe(1);
      // A change every 200 ms: a trailing-only debounce would never fire.
      for (let i = 0; i < 14; i++) {
        bumpWorkChanged();
        vi.advanceTimersByTime(200);
        await flush();
      }
      expect(reads()).toBe(1);
      bumpWorkChanged();
      vi.advanceTimersByTime(200);
      await flush();
      expect(reads()).toBe(2);
      // And once quiet, one trailing read.
      bumpWorkChanged();
      vi.advanceTimersByTime(500);
      await flush();
      expect(reads()).toBe(3);
    } finally {
      vi.useRealTimers();
    }
  });
});
