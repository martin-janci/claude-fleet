// Task editing: the dialog reads the task, writes only what changed (the
// text through `edit_work_item`, the status through `set_work_status`), and
// offers nothing for a tracker's ticket.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import EditTaskDialog from './EditTaskDialog.svelte';
import { dueLabel, ownerDueChip, parseAssignees } from './work';
import type { TaskDetail, WorkTask } from './work_view';

const task = (over: Partial<WorkTask> = {}): WorkTask => ({
  task_id: 'item:9',
  item_id: 9,
  key: 'TASK-9',
  title: 'Fix login',
  kind: 'local',
  status_category: 'todo',
  assignees: ['Ana'],
  origin: 'manual',
  group: { source: 'none', label: '' } as WorkTask['group'],
  ...over,
});

function answer(detail: TaskDetail) {
  vi.mocked(invoke).mockImplementation(async (cmd: string, payload?: unknown) => {
    if (cmd === 'work_task') return detail;
    const args = (payload as { args: Record<string, unknown> }).args;
    if (cmd === 'edit_work_item') {
      return { id: 9, source: 'local', title: args.title ?? 'Fix login', notes: args.notes ?? 'old', assignees: args.assignees ?? ['Ana'] };
    }
    if (cmd === 'set_work_status') return { id: 9, source: 'local', title: 'Fix login', status_category: args.status };
    throw new Error(`unexpected ${cmd}`);
  });
}

async function type(testid: string, value: string) {
  await fireEvent.input(screen.getByTestId(testid), { target: { value } });
  await tick();
}

const calls = (cmd: string) => vi.mocked(invoke).mock.calls.filter(([c]) => c === cmd);

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

describe('parseAssignees', () => {
  it('splits on commas, trims, and drops empty and repeated names', () => {
    expect(parseAssignees(' Ana , bo,, ana ,Bo ')).toEqual(['Ana', 'bo']);
    expect(parseAssignees('  ')).toEqual([]);
  });
});

describe('dueLabel / ownerDueChip', () => {
  // Thursday 2026-10-15, mid-afternoon local time.
  const now = new Date(2026, 9, 15, 15, 30);
  it('says today, tomorrow, a weekday this week, else the date; a past date is overdue', () => {
    expect(dueLabel('2026-10-15', now)).toEqual({ text: 'Today', overdue: false });
    expect(dueLabel('2026-10-16', now)).toEqual({ text: 'Tomorrow', overdue: false });
    expect(dueLabel('2026-10-20', now)).toEqual({ text: 'Tue', overdue: false });
    expect(dueLabel('2026-10-22', now)).toEqual({ text: 'Oct 22', overdue: false });
    expect(dueLabel('2026-10-02', now)).toEqual({ text: 'Oct 2', overdue: true });
    expect(dueLabel(null, now)).toBeNull();
    expect(dueLabel('2026-02-30', now)).toBeNull();
    expect(dueLabel('soon', now)).toBeNull();
  });
  it('names the owner ("You" for my ticket) and the due date', () => {
    expect(ownerDueChip({ assignees: ['Dana Dev'], mine: true, due_at: '2026-10-17' }, now)).toEqual({
      text: 'You · Sat',
      overdue: false,
    });
    expect(ownerDueChip({ assignees: ['Ana', 'Bo'], due_at: '2026-10-01' }, now)).toEqual({
      text: 'Ana +1 · Oct 1',
      overdue: true,
    });
    expect(ownerDueChip({ assignees: ['Ana'] }, now)).toEqual({ text: 'Ana', overdue: false });
    expect(ownerDueChip({ due_at: '2026-10-16' }, now)).toEqual({ text: 'Tomorrow', overdue: false });
    expect(ownerDueChip({ assignees: [] }, now)).toBeNull();
  });
});

describe('EditTaskDialog', () => {
  it('prefills the task and writes only the fields that changed', async () => {
    answer({ task: task(), notes: 'old' });
    const onclose = vi.fn();
    const ondone = vi.fn();
    render(EditTaskDialog, { props: { taskId: 'item:9', onclose, ondone } });
    await waitFor(() => expect(screen.getByTestId('edit-task-title')).toBeTruthy());
    expect((screen.getByTestId('edit-task-title') as HTMLInputElement).value).toBe('Fix login');
    expect((screen.getByTestId('edit-task-notes') as HTMLTextAreaElement).value).toBe('old');
    expect((screen.getByTestId('edit-task-assignees') as HTMLInputElement).value).toBe('Ana');
    const submit = screen.getByTestId('edit-task-submit') as HTMLButtonElement;
    // Nothing changed yet: nothing to save.
    expect(submit.disabled).toBe(true);

    await type('edit-task-notes', 'new notes');
    await type('edit-task-assignees', 'Ana, Bo');
    await fireEvent.change(screen.getByTestId('edit-task-status'), { target: { value: 'in_progress' } });
    await tick();
    await fireEvent.click(submit);
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(ondone).toHaveBeenCalled();
    expect(calls('edit_work_item')).toEqual([
      ['edit_work_item', { args: { item_id: 9, notes: 'new notes', assignees: ['Ana', 'Bo'] } }],
    ]);
    expect(calls('set_work_status')).toEqual([['set_work_status', { args: { item_id: 9, status: 'in_progress' } }]]);
  });

  it('sets, then clears, a due date', async () => {
    answer({ task: task({ due_at: '2026-10-16' }), notes: null });
    const onclose = vi.fn();
    render(EditTaskDialog, { props: { taskId: 'item:9', onclose } });
    await waitFor(() => expect(screen.getByTestId('edit-task-due')).toBeTruthy());
    const due = screen.getByTestId('edit-task-due') as HTMLInputElement;
    expect(due.value).toBe('2026-10-16');
    await type('edit-task-due', '2026-10-23');
    await fireEvent.click(screen.getByTestId('edit-task-submit'));
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls('edit_work_item')).toEqual([['edit_work_item', { args: { item_id: 9, due_at: '2026-10-23' } }]]);
  });

  it('clears a due date with an empty value', async () => {
    answer({ task: task({ due_at: '2026-10-16' }), notes: null });
    const onclose = vi.fn();
    render(EditTaskDialog, { props: { taskId: 'item:9', onclose } });
    await waitFor(() => expect(screen.getByTestId('edit-task-due')).toBeTruthy());
    await type('edit-task-due', '');
    await fireEvent.click(screen.getByTestId('edit-task-submit'));
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls('edit_work_item')).toEqual([['edit_work_item', { args: { item_id: 9, due_at: '' } }]]);
  });

  it('sends a status change alone, and refuses an empty title', async () => {
    answer({ task: task(), notes: null });
    const onclose = vi.fn();
    render(EditTaskDialog, { props: { taskId: 'item:9', onclose } });
    await waitFor(() => expect(screen.getByTestId('edit-task-title')).toBeTruthy());
    await type('edit-task-title', '  ');
    expect(screen.getByTestId('edit-task-title-error')).toBeTruthy();
    expect((screen.getByTestId('edit-task-submit') as HTMLButtonElement).disabled).toBe(true);
    await type('edit-task-title', 'Fix login');
    await fireEvent.change(screen.getByTestId('edit-task-status'), { target: { value: 'done' } });
    await tick();
    await fireEvent.click(screen.getByTestId('edit-task-submit'));
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    expect(calls('edit_work_item')).toEqual([]);
    expect(calls('set_work_status')).toEqual([['set_work_status', { args: { item_id: 9, status: 'done' } }]]);
  });

  it('keeps the dialog open with the error when a write fails', async () => {
    answer({ task: task(), notes: 'old' });
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_task') return { task: task(), notes: 'old' };
      throw { code: 'E_INVALID', message: 'an assignee is longer than 80 characters' };
    });
    const onclose = vi.fn();
    render(EditTaskDialog, { props: { taskId: 'item:9', onclose } });
    await waitFor(() => expect(screen.getByTestId('edit-task-title')).toBeTruthy());
    await type('edit-task-assignees', 'x'.repeat(81));
    await fireEvent.click(screen.getByTestId('edit-task-submit'));
    await waitFor(() => expect(screen.getByTestId('edit-task-error').textContent).toMatch(/80 characters/));
    expect(onclose).not.toHaveBeenCalled();
  });

  it("locks a delegated job's description: it is the prompt", async () => {
    answer({ task: task({ origin: 'agent' }), notes: 'the prompt' });
    render(EditTaskDialog, { props: { taskId: 'item:9', onclose: vi.fn() } });
    await waitFor(() => expect(screen.getByTestId('edit-task-notes-locked')).toBeTruthy());
    expect((screen.getByTestId('edit-task-notes') as HTMLTextAreaElement).disabled).toBe(true);
  });

  it("offers nothing to edit on a tracker's ticket", async () => {
    answer({ task: task({ kind: 'tracker', item_id: 3, key: 'OPS-1', tracker_name: 'Jira' }) });
    render(EditTaskDialog, { props: { taskId: 'item:3', onclose: vi.fn() } });
    await waitFor(() => expect(screen.getByTestId('edit-task-tracker').textContent).toMatch(/OPS-1 belongs to Jira/));
    expect(screen.queryByTestId('edit-task-submit')).toBeNull();
  });
});
