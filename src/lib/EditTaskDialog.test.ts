// Task editing: the dialog reads the task, writes only what changed (the
// text through `edit_work_item`, the status through `set_work_status`), and
// offers nothing for a tracker's ticket.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(async () => undefined) }));
import { openUrl } from '@tauri-apps/plugin-opener';
import { invoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import { clearToasts, runToastAction, toasts } from './toasts';
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
  clearToasts();
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

  it('Undo puts the old due date back', async () => {
    answer({ task: task({ due_at: '2026-10-16' }), notes: null });
    const onclose = vi.fn();
    render(EditTaskDialog, { props: { taskId: 'item:9', onclose } });
    await waitFor(() => expect(screen.getByTestId('edit-task-due')).toBeTruthy());
    await type('edit-task-due', '2026-10-23');
    await fireEvent.click(screen.getByTestId('edit-task-submit'));
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    const [t] = get(toasts);
    vi.mocked(invoke).mockClear();
    runToastAction(t.id);
    await waitFor(() => expect(calls('edit_work_item')).toHaveLength(1));
    expect(calls('edit_work_item')).toEqual([['edit_work_item', { args: { item_id: 9, due_at: '2026-10-16' } }]]);
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
    // Checked on blur, never on each key; the off Save says why under it.
    expect(screen.queryByTestId('edit-task-title-error')).toBeNull();
    expect(screen.getByTestId('sheet-why').textContent).toBe('A title is required.');
    await fireEvent.blur(screen.getByTestId('edit-task-title'));
    expect(screen.getByTestId('edit-task-title-error').textContent).toBe('A title is required.');
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

  it('an untouched form says why Save is off', async () => {
    answer({ task: task(), notes: 'old' });
    render(EditTaskDialog, { props: { taskId: 'item:9', onclose: vi.fn() } });
    await waitFor(() => expect(screen.getByTestId('edit-task-title')).toBeTruthy());
    expect(screen.getByTestId('sheet-why').textContent).toBe('Nothing changed yet.');
  });

  it('a saved edit closes with an Undo toast that writes the old values back', async () => {
    answer({ task: task(), notes: 'old' });
    const onclose = vi.fn();
    const ondone = vi.fn();
    render(EditTaskDialog, { props: { taskId: 'item:9', onclose, ondone } });
    await waitFor(() => expect(screen.getByTestId('edit-task-title')).toBeTruthy());
    await type('edit-task-title', 'Fix the login');
    await fireEvent.change(screen.getByTestId('edit-task-status'), { target: { value: 'done' } });
    await tick();
    // ⌘↵ / Ctrl+Enter submits from any field (jsdom is not a Mac).
    await fireEvent.keyDown(screen.getByTestId('edit-task-notes'), { key: 'Enter', ctrlKey: true });
    await waitFor(() => expect(onclose).toHaveBeenCalled());
    const [t] = get(toasts);
    expect(t.message).toBe('Task saved.');
    expect(t.action?.label).toBe('Undo');

    vi.mocked(invoke).mockClear();
    runToastAction(t.id);
    await waitFor(() => expect(calls('set_work_status')).toHaveLength(1));
    expect(calls('edit_work_item')).toEqual([['edit_work_item', { args: { item_id: 9, title: 'Fix login' } }]]);
    expect(calls('set_work_status')).toEqual([['set_work_status', { args: { item_id: 9, status: 'todo' } }]]);
    await waitFor(() => expect(ondone).toHaveBeenCalledTimes(2));
  });

  it("a hub refusal is a banner on top, and the person's edit stays", async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_task') return { task: task(), notes: 'old' };
      throw { code: 'E_FORBIDDEN', message: 'you are a Viewer in 32bit' };
    });
    const onclose = vi.fn();
    render(EditTaskDialog, { props: { taskId: 'item:9', onclose } });
    await waitFor(() => expect(screen.getByTestId('edit-task-title')).toBeTruthy());
    await type('edit-task-title', 'Mine');
    await fireEvent.click(screen.getByTestId('edit-task-submit'));
    const banner = await screen.findByTestId('edit-task-error');
    expect(banner.textContent).toMatch(/The hub refused this: you are a Viewer in 32bit\./);
    expect(banner.textContent).toMatch(/Ask an admin/);
    expect((screen.getByTestId('edit-task-title') as HTMLInputElement).value).toBe('Mine');
    expect(onclose).not.toHaveBeenCalled();
    expect(get(toasts)).toEqual([]);
  });

  it('closing a changed task asks "Discard changes?" before it closes', async () => {
    answer({ task: task(), notes: 'old' });
    const onclose = vi.fn();
    render(EditTaskDialog, { props: { taskId: 'item:9', onclose } });
    await waitFor(() => expect(screen.getByTestId('edit-task-title')).toBeTruthy());
    await type('edit-task-title', 'Changed');
    await fireEvent.click(screen.getByTestId('sheet-cancel'));
    expect(onclose).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('form-discard'));
    expect(onclose).toHaveBeenCalledOnce();
    expect(calls('edit_work_item')).toEqual([]);
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
    // Nothing to fill in, and Save stays off.
    expect(screen.queryByTestId('edit-task-title')).toBeNull();
    expect((screen.getByTestId('edit-task-submit') as HTMLButtonElement).disabled).toBe(true);
    // No URL, no link.
    expect(screen.queryByTestId('edit-task-open-tracker')).toBeNull();
  });

  it("a tracker's ticket says nothing is written back and opens the ticket in its tracker (G2.1)", async () => {
    answer({
      task: task({
        kind: 'tracker',
        item_id: 3,
        key: 'PD-2988',
        provider: 'jira',
        tracker_name: 'Jira PD',
        url: 'https://acme.atlassian.net/browse/PD-2988',
      }),
    });
    render(EditTaskDialog, { props: { taskId: 'item:3', onclose: vi.fn() } });
    await waitFor(() => expect(screen.getByTestId('edit-task-tracker').textContent).toMatch(/PD-2988 belongs to Jira/));
    expect(screen.getByTestId('edit-task-tracker').textContent).toMatch(/writes nothing back/);
    const open = screen.getByTestId('edit-task-open-tracker');
    expect(open.textContent).toBe('Open in Jira Cloud ↗');
    await fireEvent.click(open);
    await waitFor(() => expect(vi.mocked(openUrl)).toHaveBeenCalledWith('https://acme.atlassian.net/browse/PD-2988'));
    // Nothing was written.
    expect(calls('edit_work_item')).toHaveLength(0);
    expect(calls('set_work_status')).toHaveLength(0);
  });
});
