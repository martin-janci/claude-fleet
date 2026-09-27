// The Work view's sidebar (work graph M14.2): the tree drawn from `tree`
// pages, occurrence states, selection and highlight, Load more, filters as
// requests, saved views applied, and the explicit loading / empty / error /
// older-hub states.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { sessions } from './sessions';
import WorkSidebar from './WorkSidebar.svelte';
import { createWorkTreeStore, openTaskId } from './work_tree';
import { selectedSession, clearSelection } from './selection';
import { session } from './hosts_fixture';
import type { TreeOpts, TreePage, WorkTask, WorkTaskLink } from './work_view';

const G_ABC = { id: 'tracker:1:ABC', label: 'ABC', source: 'tracker', editable: true };

function link(over: Partial<WorkTaskLink>): WorkTaskLink {
  return {
    link_id: 1,
    link_version: 1,
    state: 'active',
    primary: true,
    session_id: 41,
    name: 'api',
    host: 'mefistos',
    source: 'manual',
    why: 'branch abc-12',
    created_at: 1,
    needs_you: false,
    archived: false,
    resumable: true,
    cross_org: false,
    other_tasks: 0,
    ...over,
  };
}

function task(id: string, over: Partial<WorkTask> = {}): WorkTask {
  return {
    task_id: id,
    key: id.replace('item:', 'ABC-'),
    title: `<b>Task ${id}</b>`,
    kind: 'tracker',
    tracker_name: 'Jira',
    tracker_state: 'ok',
    status_name: 'In Review',
    unavailable: false,
    mine: false,
    org_id: 1,
    org_source: 'tracker',
    org_fenced: true,
    org_mixed: false,
    group: G_ABC,
    counts: { active: 2, ended: 1, suggested: 1 },
    needs_you: false,
    review: false,
    placement_version: 0,
    sessions: [],
    sessions_more: 0,
    ...over,
  };
}

const T1 = task('item:1', {
  review: true,
  needs_you: true,
  sessions: [
    link({ link_id: 1, session_id: 41, primary: true }),
    link({ link_id: 2, session_id: 42, primary: false, name: 'web' }),
    link({ link_id: 3, session_id: 43, state: 'suggested', primary: false, name: 'guess' }),
    link({ link_id: 4, session_id: undefined, state: 'ended', primary: false, name: 'old' }),
  ],
});
const T2 = task('item:2', { unavailable: true, tracker_state: 'auth_failed', sessions: [link({ link_id: 5, session_id: 41, primary: false })] });
const T3 = task('item:3', { sessions: [] });

function header(over: Partial<TreePage> = {}): TreePage {
  return {
    tasks: [],
    groups: [{ org_id: 1, org_name: 'Acme', group: G_ABC, count: 3 }],
    orgs: [{ id: 1, name: 'Acme' }],
    trackers: [{ id: 1, name: 'Jira', provider: 'jira', state: 'ok', org_id: 1 }],
    total: 3,
    generated_at: 1,
    ...over,
  };
}

let treeCalls: TreeOpts[] = [];
function hub(opts: { header?: TreePage; fail?: { code: string; message: string } } = {}) {
  treeCalls = [];
  vi.mocked(invoke).mockImplementation(async (cmd: string, a?: unknown) => {
    const args = (a as { args: Record<string, unknown> } | undefined)?.args ?? {};
    if (cmd === 'work_views') return [{ id: 9, name: 'My open work', filters: { mine: true, status: 'open' }, version: 1, updated_at: 0 }];
    if (cmd !== 'work_tree') return null;
    if (opts.fail) throw opts.fail;
    const o: TreeOpts = { filters: args.filters as never, cursor: args.cursor as never, limit: args.limit as never };
    treeCalls.push(o);
    if (o.limit === 1) return opts.header ?? header();
    if (!o.cursor) return { ...header(), tasks: [T1, T2], next_cursor: 'c2' };
    return { ...header(), tasks: [T3] };
  });
}

async function flush() {
  for (let i = 0; i < 12; i++) await tick();
}

async function mount() {
  const store = createWorkTreeStore({ debounceMs: 10 });
  sessions.set([
    session('mefistos', 'api', { id: 41 }),
    session('mefistos', 'web', { id: 42 }),
    session('mefistos', 'guess', { id: 43 }),
  ]);
  const r = render(WorkSidebar, { props: { store } });
  await flush();
  return { ...r, store };
}

describe('WorkSidebar', () => {
  beforeEach(() => {
    localStorage.clear();
    clearSelection();
    openTaskId.set(null);
    sessions.set([]);
    vi.mocked(invoke).mockReset();
  });

  it('draws org → group → task → occurrences from the pages', async () => {
    hub();
    await mount();
    expect(screen.getByTestId('work-tree')).toBeTruthy();
    expect(screen.getAllByTestId('work-org')[0].textContent).toContain('Acme');
    const tasks = screen.getAllByTestId('work-task');
    expect(tasks.map((t) => t.getAttribute('data-task'))).toEqual(['item:1', 'item:2']);
    // Tracker text as text, never markup.
    expect(tasks[0].textContent).toContain('<b>Task item:1</b>');
    expect(tasks[0].querySelector('b')).toBeNull();
    expect(tasks[0].textContent).toContain('ABC-1');
    expect(tasks[0].textContent).toContain('Jira');
    expect(tasks[0].textContent).toContain('In Review');
    expect(screen.getAllByTestId('work-task-counts')[0].textContent).toBe('2 active / 1 past');
    expect(screen.getAllByTestId('work-task-review')).toHaveLength(1);
    expect(screen.getAllByTestId('work-task-needs')).toHaveLength(1);
    // Unavailable is struck through; a failing tracker says so.
    expect(tasks[1].querySelector('.title.unavailable')).toBeTruthy();
    expect(screen.getAllByTestId('work-task-down')).toHaveLength(1);

    const occ = tasks[0].querySelectorAll('[data-testid="work-occ"]');
    expect(Array.from(occ, (o) => o.getAttribute('data-kind'))).toEqual(['primary', 'secondary', 'suggested', 'past']);
    expect(occ[0].textContent).toContain('★');
    expect(occ[2].classList.contains('suggested')).toBe(true);
    // Past is dimmed and never reads as active.
    expect(occ[3].textContent).toContain('ended');
    expect(occ[3].textContent).not.toContain('active');
  });

  it('opening an occurrence selects that session and highlights all of its occurrences', async () => {
    hub();
    await mount();
    const occ = screen.getAllByTestId('work-occ');
    await fireEvent.click(occ[0]);
    await flush();
    expect(get(selectedSession)?.id).toBe(41);
    const hl = screen.getAllByTestId('work-occ').filter((o) => o.classList.contains('hl'));
    expect(hl.map((o) => o.getAttribute('data-session'))).toEqual(['41', '41']);
  });

  it('clicking a task opens it in the center pane (openTaskId)', async () => {
    hub();
    await mount();
    await fireEvent.click(screen.getAllByTestId('work-task')[1].querySelector('.task-row')!);
    expect(get(openTaskId)).toBe('item:2');
    expect(screen.getAllByTestId('work-task')[1].classList.contains('selected')).toBe(true);
  });

  it('pages a section with Load more over the cursor', async () => {
    hub();
    await mount();
    const more = screen.getByTestId('work-load-more');
    expect(more.textContent).toContain('2 of 3');
    await fireEvent.click(more);
    await flush();
    expect(treeCalls.at(-1)).toMatchObject({ cursor: 'c2', filters: { org: 1, group: 'tracker:1:ABC' } });
    expect(screen.getAllByTestId('work-task').map((t) => t.getAttribute('data-task'))).toEqual(['item:1', 'item:2', 'item:3']);
    expect(screen.queryByTestId('work-load-more')).toBeNull();
  });

  it('a filter becomes the request of every read', async () => {
    hub();
    await mount();
    const status = screen.getByTestId('work-filter-status') as HTMLSelectElement;
    status.value = 'open';
    await fireEvent.change(status);
    await fireEvent.click(screen.getByTestId('work-filter-mine'));
    await flush();
    const head = treeCalls.filter((c) => c.limit === 1).at(-1)!;
    expect(head.filters).toEqual({ status: 'open', mine: true });
    const sec = treeCalls.filter((c) => c.limit !== 1).at(-1)!;
    expect(sec.filters).toEqual({ status: 'open', mine: true, org: 1, group: 'tracker:1:ABC' });
  });

  it('applies a saved view from the menu', async () => {
    hub();
    const { store } = await mount();
    await fireEvent.click(screen.getByTestId('work-views-btn'));
    await flush();
    await fireEvent.click(screen.getByTestId('work-view-item'));
    await flush();
    expect(get(store).view).toBe(9);
    expect(treeCalls.filter((c) => c.limit === 1).at(-1)!.filters).toEqual({ status: 'open', mine: true });
    expect(screen.getByTestId('work-views-btn').textContent).toContain('My open work');
  });

  it('says "needs a newer hub" on a hub without the Work view', async () => {
    hub({ fail: { code: 'E_INVALID', message: 'unknown work action "tree"' } });
    await mount();
    expect(screen.getByTestId('work-needs-hub').textContent).toContain('newer hub');
    expect(screen.queryByTestId('work-error')).toBeNull();
  });

  it('shows an explicit error and an explicit empty state', async () => {
    hub({ fail: { code: 'E_DB', message: 'locked' } });
    await mount();
    expect(screen.getByTestId('work-error').textContent).toContain('E_DB: locked');
  });

  it('empty', async () => {
    hub({ header: header({ groups: [], total: 0 }) });
    await mount();
    expect(screen.getByTestId('work-empty')).toBeTruthy();
  });
});
