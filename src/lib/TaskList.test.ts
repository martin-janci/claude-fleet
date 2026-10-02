// The Work tab's List layout (design 2026-09-29): one `work_tree` read with
// the current filters (archived on), To do / Doing / Done, + New task, Start.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TaskList from './TaskList.svelte';
import { task } from './work_view_fixture';
import { workViewFilters, type WorkTreePage } from './work_view';

const NOW = Math.floor(Date.now() / 1000);
const page: WorkTreePage = {
  tasks: [
    task({
      task_id: 'item:1',
      item_id: 1,
      key: 'TASK-1',
      title: 'Write notes',
      kind: 'local',
      origin: 'manual',
      status_category: 'todo',
      counts: { active: 0, ended: 0, suggested: 0 },
      sessions: [],
      last_activity_at: NOW,
      project_id: 3,
      project_label: 'acme/api',
    }),
    task({
      task_id: 'item:110',
      item_id: 110,
      key: 'OM-110',
      title: 'Qomora',
      status_category: 'todo',
      status_name: 'Backlog',
      counts: { active: 1, ended: 2, suggested: 0 },
      open_proposals: 2,
      last_activity_at: NOW,
    }),
    task({
      task_id: 'item:42',
      item_id: 42,
      key: 'TASK-42',
      title: 'Neutral review',
      kind: 'local',
      origin: 'agent',
      parent_task_id: 'item:110',
      job_state: 'done',
      status_category: 'done',
      last_activity_at: NOW,
    }),
  ],
  groups: [],
  orgs: [],
  trackers: [],
  total: 3,
  archived_hidden: 0,
  next_cursor: null,
  generated_at: NOW,
};
const flush = async () => {
  for (let i = 0; i < 6; i++) {
    await Promise.resolve();
    await tick();
  }
};
const calls = (cmd: string) => (invoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === cmd);

beforeEach(() => {
  workViewFilters.set({ tracker: 1 });
  (invoke as ReturnType<typeof vi.fn>).mockReset();
  (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
    if (cmd === 'work_tree') return page;
    if (cmd === 'list_projects') return [];
    if (cmd === 'create_work_task') return { id: 9, key: 'TASK-9', title: 'New', source: 'local' };
    if (cmd === 'start_work') return { id: 50, host_alias: 'mac', tmux_name: 'x' };
    return null;
  });
});

describe('TaskList', () => {
  it('reads once with the current filters and archived on', async () => {
    render(TaskList);
    await flush();
    expect(calls('work_tree')).toHaveLength(1);
    const args = (calls('work_tree')[0][1] as { args: { filters: Record<string, unknown> } }).args.filters;
    expect(args.tracker).toBe(1);
    expect(args.archived).toBe(true);
  });

  it('re-reads when the filters change', async () => {
    render(TaskList);
    await flush();
    workViewFilters.set({ tracker: 2 });
    await flush();
    expect(calls('work_tree')).toHaveLength(2);
    expect((calls('work_tree')[1][1] as { args: { filters: Record<string, unknown> } }).args.filters.tracker).toBe(2);
  });

  it('shows sections, the tracker status, the proposal badge and nested agent jobs', async () => {
    render(TaskList);
    await flush();
    expect(screen.getByTestId('task-section-todo').textContent).toContain('Write notes');
    const doing = screen.getByTestId('task-section-doing');
    expect(doing.textContent).toContain('Qomora');
    expect(doing.textContent).toContain('Backlog');
    expect(screen.getByTestId('task-proposals-badge').textContent).toContain('2');
    expect(screen.getByTestId('task-child').textContent).toContain('Neutral review');
    expect(screen.getByTestId('task-child').textContent).toContain('agent');
  });

  it('Done is collapsed by default', async () => {
    render(TaskList);
    await flush();
    expect(screen.getByTestId('task-section-done').querySelector('ul')).toBeNull();
  });

  it('quick add creates a task; Start starts one by item id', async () => {
    render(TaskList);
    await flush();
    const input = screen.getByTestId('task-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'New' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    expect(calls('create_work_task')[0][1]).toEqual({ args: { title: 'New' } });
    await fireEvent.click(screen.getByTestId('task-start'));
    await flush();
    expect(calls('start_work')[0][1]).toEqual({ args: { item_id: 1, project_id: 3 } });
  });

  it('▾ adds a project and notes to the new task', async () => {
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_tree') return page;
      if (cmd === 'list_projects')
        return [
          {
            project: { id: 3, owner: 'acme', repo: 'api', base_path: '/x', last_session_at: null, adopted: false, system: false },
            worktrees: [],
          },
        ];
      if (cmd === 'create_work_task') return { id: 9, key: 'TASK-9', title: 'New', source: 'local' };
      return null;
    });
    render(TaskList);
    await flush();
    await fireEvent.click(screen.getByTestId('task-add-more'));
    await flush();
    const select = screen.getByTestId('task-add-project') as HTMLSelectElement;
    select.value = select.options[1].value;
    await fireEvent.change(select);
    await fireEvent.input(screen.getByTestId('task-add-notes'), { target: { value: 'Rebase first' } });
    const input = screen.getByTestId('task-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'New' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    expect(calls('create_work_task')[0][1]).toEqual({ args: { title: 'New', project_id: 3, notes: 'Rebase first' } });
  });

  it('says what it has of the total and pages on from the cursor', async () => {
    const first = {
      ...page,
      tasks: [task({ task_id: 'item:1', title: 'One', status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 } })],
      total: 2,
      next_cursor: 'c1',
    };
    const second = {
      ...page,
      tasks: [task({ task_id: 'item:2', title: 'Two', status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 } })],
      total: 2,
      next_cursor: null,
    };
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, a: unknown) => {
      if (cmd !== 'work_tree') return [];
      return (a as { args: { cursor?: string | null } }).args.cursor === 'c1' ? second : first;
    });
    render(TaskList);
    await flush();
    // A short page is all the hub had to give, so the first read stops there
    // and says so instead of presenting one page as the whole result.
    expect(calls('work_tree')).toHaveLength(1);
    expect(screen.getByTestId('task-list-more').textContent).toContain('1 of 2');
    await fireEvent.click(screen.getByTestId('task-list-load-more'));
    await flush();
    expect((calls('work_tree')[1][1] as { args: { cursor?: string | null } }).args.cursor).toBe('c1');
    const todo = screen.getByTestId('task-section-todo').textContent;
    expect(todo).toContain('One');
    expect(todo).toContain('Two');
    expect(screen.queryByTestId('task-list-more')).toBeNull();
  });

  it('keeps the rows when a refetch fails, and loses them only on a first read', async () => {
    render(TaskList);
    await flush();
    expect(screen.getByTestId('task-section-todo').textContent).toContain('Write notes');

    // A background refetch fails: the rows stay, with a line above them.
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_tree') throw { code: 'E_HUB_DOWN', message: 'the hub is not answering' };
      return [];
    });
    workViewFilters.set({ tracker: 3 });
    await flush();
    expect(screen.getByTestId('task-list-refresh-error').textContent).toContain('the hub is not answering');
    expect(screen.getByTestId('task-section-todo').textContent).toContain('Write notes');
    expect(screen.queryByTestId('task-list-error')).toBeNull();
  });

  it('shows the error page when the FIRST read fails', async () => {
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_tree') throw { code: 'E_HUB_DOWN', message: 'the hub is not answering' };
      return [];
    });
    render(TaskList);
    await flush();
    expect(screen.getByTestId('task-list-error')).toBeTruthy();
    expect(screen.queryByTestId('task-section-todo')).toBeNull();
  });

  it('renders tracker text as text', async () => {
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) =>
      cmd === 'work_tree'
        ? { ...page, tasks: [task({ task_id: 'item:5', title: '<b>x</b>', status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 } })] }
        : [],
    );
    render(TaskList);
    await flush();
    expect(screen.getByTestId('task-section-todo').querySelector('b')).toBeNull();
  });
});
