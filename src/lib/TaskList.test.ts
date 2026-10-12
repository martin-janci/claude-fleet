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
import { sessions } from './sessions';
import { session } from './hosts_fixture';

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
/** A preview with everything resolved and nothing in the way. */
const cleanPreview = {
  key: 'TASK-1',
  title: 'Write notes',
  item_id: 1,
  plan: { key: 'TASK-1', title: 'Write notes', item_id: 1, project_id: 3, host_alias: 'mac', branch: 'task-1-write-notes', name: 'TASK-1 Write notes' },
  projects: [{ id: 3, owner: 'acme', repo: 'api' }],
  hosts: [{ alias: 'mac', reachable: true }],
  conflicts: [],
  brief: 'Write notes',
  checkout: { exists: false },
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
    if (cmd === 'preview_start_work') return cleanPreview;
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
    // OM-110 is in its tracker's Backlog with a live session: it stays in
    // To do, the column the Board puts it in (one status rule, step 1.6).
    const todo = screen.getByTestId('task-section-todo');
    expect(todo.textContent).toContain('Qomora');
    expect(todo.textContent).toContain('Backlog');
    // Nothing is in Doing, so it is a word in one line, not an empty heading.
    expect(screen.queryByTestId('task-section-doing')).toBeNull();
    expect(screen.getByTestId('task-proposals-badge').textContent).toContain('2');
    expect(screen.getByTestId('task-child').textContent).toContain('Neutral review');
    expect(screen.getByTestId('task-child').textContent).toContain('agent');
  });

  it('Done is collapsed by default', async () => {
    const shipped = task({ task_id: 'item:7', item_id: 7, key: 'TASK-7', title: 'Shipped', kind: 'local', status_category: 'done', last_activity_at: NOW });
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) =>
      cmd === 'work_tree' ? { ...page, tasks: [...page.tasks, shipped], total: 4 } : null,
    );
    render(TaskList);
    await flush();
    expect(screen.getByTestId('task-section-done').querySelector('ul')).toBeNull();
    await fireEvent.click(screen.getByTestId('task-section-done').querySelector('button')!);
    expect(screen.getByTestId('task-section-done').textContent).toContain('Shipped');
  });

  it('empty sections fold into one line instead of a heading each (redesign 1.4)', async () => {
    render(TaskList);
    await flush();
    // The fixture has To do rows only: Doing and Done are one line between them.
    expect(screen.getByTestId('task-section-todo')).toBeTruthy();
    expect(screen.queryByTestId('task-section-doing')).toBeNull();
    expect(screen.queryByTestId('task-section-done')).toBeNull();
    expect(screen.getByTestId('task-sections-empty').textContent).toBe('Nothing in Doing or Done in the last 7 days.');
    expect(screen.getAllByRole('heading', { level: 3 })).toHaveLength(1);
  });

  it('quick add creates a task; the Work button previews, then starts where the preview said', async () => {
    render(TaskList);
    await flush();
    const input = screen.getByTestId('task-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'New' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    expect(calls('create_work_task')[0][1]).toEqual({ args: { title: 'New' } });
    const row = screen.getByTestId('task-section-todo');
    const primary = row.querySelector('[data-testid="work-button-primary"]') as HTMLButtonElement;
    expect(primary.textContent).toBe('Start new');
    await fireEvent.click(primary);
    await flush();
    expect(calls('preview_start_work')[0][1]).toEqual({ args: { item_id: 1, project_id: 3, with_brief: true } });
    expect(calls('start_work')[0][1]).toEqual({
      args: { item_id: 1, project_id: 3, host_alias: 'mac', with_brief: true },
    });
    expect(screen.queryByTestId('start-popover')).toBeNull();
  });

  it('a task with a live session opens it; s and ⇧S act on the selected row', async () => {
    render(TaskList);
    await flush();
    const om110 = screen.getAllByTestId('task-row').find((r) => r.textContent?.includes('Qomora'));
    // OM-110 has an active count but no live link in the fixture: Start.
    expect(om110?.querySelector('[data-testid="work-button-primary"]')?.textContent).toBe('Start new');
    const list = screen.getByTestId('task-list');
    await fireEvent.keyDown(list, { key: 'j' });
    await flush();
    await fireEvent.keyDown(list, { key: 'S', shiftKey: true });
    await flush();
    expect(calls('preview_start_work')).toHaveLength(1);
    expect(screen.getByTestId('start-popover')).toBeTruthy();
    expect(calls('start_work')).toHaveLength(0);
  });

  it('▾ Attach running session… switches an idle session onto the task, with Undo', async () => {
    sessions.set([
      session('mac', 'api--other', { id: 60, project_id: 3, claude_status: 'idle', work: { link_id: 70, item_id: null, key: 'OPS-1', title: '', source: 'manual' } }),
    ]);
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, raw?: { args: { link_id?: number } }) => {
      if (cmd === 'work_tree') return page;
      if (cmd === 'switch_session_work') {
        const back = raw?.args.link_id === 71;
        return session('mac', 'api--other', {
          id: 60,
          work: { link_id: back ? 72 : 71, item_id: back ? null : 1, key: back ? 'OPS-1' : 'TASK-1', title: '', source: 'manual' },
        });
      }
      return null;
    });
    render(TaskList);
    await flush();
    const todo = screen.getByTestId('task-section-todo');
    await fireEvent.click(todo.querySelector('[data-testid="work-button-menu"]') as HTMLButtonElement);
    await fireEvent.click(screen.getByTestId('work-button-attach'));
    await flush();
    await fireEvent.click(screen.getByTestId('attach-picker-row'));
    await flush();
    expect((screen.getByTestId('attach-picker-switch') as HTMLInputElement).checked).toBe(true);
    await fireEvent.click(screen.getByTestId('attach-picker-go'));
    await flush();
    expect(calls('switch_session_work')[0][1]).toEqual({
      args: { session_id: 60, link_id: 70, item_id: 1, expected_primary: 70, ack_live: false },
    });
    expect(screen.queryByTestId('attach-picker')).toBeNull();
    expect(screen.getByTestId('work-button-undo-notice').textContent).toContain('Switched api--other from OPS-1');
    await fireEvent.click(screen.getByTestId('work-button-undo'));
    await flush();
    expect(calls('switch_session_work')[1][1]).toEqual({
      args: { session_id: 60, link_id: 71, key: 'OPS-1', expected_primary: 71, ack_live: true, force_cross_org: true },
    });
    sessions.set([]);
  });

  it('▾ opens New task with the typed title, a project and notes (G2.1)', async () => {
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
    const input = screen.getByTestId('task-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'New' } });
    await fireEvent.click(screen.getByTestId('task-add-more'));
    await flush();
    expect((screen.getByTestId('new-task-title') as HTMLInputElement).value).toBe('New');
    const select = screen.getByTestId('new-task-project') as HTMLSelectElement;
    select.value = select.options[1].value;
    await fireEvent.change(select);
    await fireEvent.input(screen.getByTestId('new-task-notes'), { target: { value: 'Rebase first' } });
    await fireEvent.click(screen.getByTestId('new-task-submit'));
    await flush();
    expect(calls('create_work_task')[0][1]).toEqual({ args: { title: 'New', project_id: 3, notes: 'Rebase first' } });
    expect(screen.queryByTestId('new-task-dialog')).toBeNull();
    expect(input.value).toBe('');
  });

  it('"Start a session for it now" opens the new task\'s start menu once its row mounts (G2.1)', async () => {
    let created = false;
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_tree') return created ? page : { ...page, tasks: page.tasks.slice(1) };
      if (cmd === 'list_projects') return [];
      if (cmd === 'create_work_task') {
        created = true;
        return { id: 1, key: 'TASK-1', title: 'Write notes', source: 'local' };
      }
      if (cmd === 'preview_start_work') return cleanPreview;
      return null;
    });
    render(TaskList, { debounceMs: 0, maxWaitMs: 0 });
    await flush();
    await fireEvent.click(screen.getByTestId('task-add-more'));
    await flush();
    await fireEvent.input(screen.getByTestId('new-task-title'), { target: { value: 'Write notes' } });
    await fireEvent.click(screen.getByTestId('new-task-start-now'));
    await fireEvent.click(screen.getByTestId('new-task-submit'));
    await flush();
    await new Promise((r) => setTimeout(r, 10));
    await flush();
    // A clean preview still asks: the person picks in the start menu.
    expect(calls('preview_start_work')).toHaveLength(1);
    expect(screen.getByTestId('start-popover')).toBeTruthy();
    expect(calls('start_work')).toHaveLength(0);
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
