// Gap plan G3.10 (board MCTasks): Control's Tasks view — tasks by status,
// Mine and Claude, and bulk Start new, Assign and Done.
import { render, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

const calls: { cmd: string; args: Record<string, unknown> }[] = [];
let tasks: unknown[] = [];
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string, raw: { args: Record<string, unknown> }) => {
    calls.push({ cmd, args: raw?.args });
    if (cmd === 'work_tree') return { tasks, groups: [], orgs: [], trackers: [], total: tasks.length };
    if (cmd === 'start_work') return { id: 99, tmux_name: 'new', host_alias: 'mac' };
    if (cmd === 'set_work_status' || cmd === 'edit_work_item') return { id: raw.args.item_id, title: 't', status_category: 'done' };
    throw { code: 'E_TEST', message: `no ${cmd}` };
  }),
}));
import ControlTasksView from './ControlTasksView.svelte';
import { expectAccessible } from './a11y_check';

const now = Math.floor(Date.now() / 1000);
const task = (id: number, over: Record<string, unknown> = {}) => ({
  task_id: `item:${id}`,
  item_id: id,
  kind: 'local',
  title: `Task ${id}`,
  group: { id: 'none', label: 'none', source: 'none' },
  stage: 'backlog',
  ...over,
});

beforeEach(() => {
  calls.length = 0;
  tasks = [
    task(1, { needs_you: true, stage: 'in_progress', counts: { active: 1 } }),
    task(2, { stage: 'in_progress', counts: { active: 1 } }),
    task(3),
    task(4, { stage: 'done', last_activity_at: now - 3600 }),
    { ...task(5), task_id: 'ref:ACME-5', item_id: null, kind: 'ref', key: 'ACME-5', title: 'Tracker one' },
  ];
});

describe('ControlTasksView (G3.10)', () => {
  it('lists the tasks by status and reads them with archived ones, for Done this week', async () => {
    const { getByTestId, container } = render(ControlTasksView);
    await waitFor(() => expect(getByTestId('control-tasks-section-needs_you')).toBeTruthy());
    expect(getByTestId('control-tasks-section-in_progress').textContent).toContain('Task 2');
    expect(getByTestId('control-tasks-section-up_next').textContent).toContain('Task 3');
    expect(getByTestId('control-tasks-section-done_week').textContent).toContain('Task 4');
    expect(calls[0].args.filters).toMatchObject({ archived: true });
    await expectAccessible(container);
  });

  it('Mine asks the hub; Claude keeps only tasks an agent is on', async () => {
    const { getByTestId, queryByTestId, getAllByTestId } = render(ControlTasksView);
    await waitFor(() => expect(getAllByTestId('control-task-row')).toHaveLength(5));
    await fireEvent.click(getByTestId('control-tasks-mine'));
    await waitFor(() => expect(calls.filter((c) => c.cmd === 'work_tree')).toHaveLength(2));
    expect(calls[1].args.filters).toMatchObject({ mine: true });
    await fireEvent.click(getByTestId('control-tasks-claude'));
    await waitFor(() => expect(getAllByTestId('control-task-row')).toHaveLength(2));
    expect(queryByTestId('control-tasks-section-up_next')).toBeNull();
  });

  it('Start new starts a session on each ticked task without one', async () => {
    const { getAllByTestId, getByTestId } = render(ControlTasksView);
    await waitFor(() => expect(getAllByTestId('control-task-row')).toHaveLength(5));
    const boxes = getAllByTestId('control-task-row').map((r) => r.querySelector('input')!);
    await fireEvent.click(boxes[1]); // Task 2: already running
    await fireEvent.click(boxes[2]); // Task 3
    await fireEvent.click(boxes[3]); // ACME-5 (Up next, after Task 3)
    expect(getByTestId('control-tasks-start').textContent).toBe('Start new (2)');
    await fireEvent.click(getByTestId('control-tasks-start'));
    await waitFor(() => expect(getByTestId('control-tasks-note').textContent).toContain('Started 2 sessions'));
    const starts = calls.filter((c) => c.cmd === 'start_work').map((c) => c.args);
    expect(starts).toEqual([
      { item_id: 3, with_brief: true },
      { reference: 'ACME-5', with_brief: true },
    ]);
  });

  it('Done and Assign change native items and say the tracker ticket changes in its tracker', async () => {
    const { getAllByTestId, getByTestId } = render(ControlTasksView);
    await waitFor(() => expect(getAllByTestId('control-task-row')).toHaveLength(5));
    const boxes = () => getAllByTestId('control-task-row').map((r) => r.querySelector('input')!);
    await fireEvent.click(boxes()[2]);
    await fireEvent.click(boxes()[3]);
    await fireEvent.click(getByTestId('control-tasks-done'));
    await waitFor(() => expect(getByTestId('control-tasks-note').textContent).toContain('Moved 1 to Done.'));
    expect(getByTestId('control-tasks-note').textContent).toContain('1 tracker ticket change in its tracker');
    expect(calls.filter((c) => c.cmd === 'set_work_status').map((c) => c.args)).toEqual([{ item_id: 3, status: 'done' }]);

    await fireEvent.click(boxes()[2]);
    await fireEvent.click(getByTestId('control-tasks-assign'));
    await fireEvent.input(getByTestId('control-tasks-assignee'), { target: { value: 'Ana, Ben' } });
    await fireEvent.click(getByTestId('control-tasks-assign-go'));
    await waitFor(() => expect(getByTestId('control-tasks-note').textContent).toContain('Assigned 1 to Ana, Ben.'));
    expect(calls.filter((c) => c.cmd === 'edit_work_item').map((c) => c.args)).toEqual([{ item_id: 3, assignees: ['Ana', 'Ben'] }]);
  });
});
