// The task page's shared-work sections (design 2026-09-29 §4): notes,
// subtasks, proposals, jobs and agent steps — text as text.
import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TaskWorkSections from './TaskWorkSections.svelte';
import { task } from './work_view_fixture';
import type { TaskDetail } from './work_view';

const detail: TaskDetail = {
  task: task({ task_id: 'item:110', item_id: 110, key: 'OM-110' }),
  notes: 'Rebase <b>now</b>',
  subtasks: [
    { task_id: 'item:41', item_id: 41, key: 'TASK-41', title: 'SELECT stats', origin: 'manual', status: 'todo', project_id: 3, live_sessions: 0, job_state: null },
  ],
  proposals: [{ item_id: 44, key: 'TASK-44', title: 'Decide the P0 owner', why: 'Both reviews block on it.', notes: null, proposed_by: 'OM-110 · trn', at: 1 }],
  rejected_proposals: [{ item_id: 40, key: 'TASK-40', title: 'Create om-catalog', why: null, notes: null, proposed_by: 'x', at: 1 }],
  jobs: [{ item_id: 42, key: 'TASK-42', title: 'Neutral review', state: 'done', result: 'Qomora has offers, not products.', worker: 'review-neutral', at: 1 }],
  steps: [
    {
      label: 'OM-110 Qomora',
      claude_session_id: 'c1',
      steps: [
        { text: 'Read OM-110', state: 'completed', at: 1 },
        { text: 'Prepare SELECTs', state: 'in_progress', at: 2 },
      ],
    },
  ],
};
const flush = async () => {
  for (let i = 0; i < 5; i++) {
    await Promise.resolve();
    await tick();
  }
};
const calls = (cmd: string) => (invoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === cmd);

beforeEach(() => {
  (invoke as ReturnType<typeof vi.fn>).mockReset();
  (invoke as ReturnType<typeof vi.fn>).mockResolvedValue({ id: 1, title: 'x', source: 'local' });
});

describe('TaskWorkSections', () => {
  it('renders every section, text as text', () => {
    render(TaskWorkSections, { detail });
    expect(screen.getByTestId('task-notes').textContent).toBe('Rebase <b>now</b>');
    expect(screen.getByTestId('task-subtasks').textContent).toContain('SELECT stats');
    expect(screen.getByTestId('task-proposal').textContent).toContain('Both reviews block on it.');
    expect(screen.getByTestId('task-job-result').textContent).toContain('offers, not products');
    expect(screen.getAllByTestId('task-step')).toHaveLength(2);
    expect(screen.getByTestId('task-steps').textContent).toContain('per the agent');
  });

  it('accepts and rejects a proposal', async () => {
    render(TaskWorkSections, { detail });
    await fireEvent.click(screen.getByTestId('task-proposal-accept'));
    await flush();
    expect(calls('accept_work_proposal')[0][1]).toEqual({ args: { item_id: 44 } });
    await fireEvent.click(screen.getByTestId('task-proposal-reject'));
    await flush();
    expect(calls('reject_work_proposal')[0][1]).toEqual({ args: { item_id: 44 } });
  });

  it('starts a subtask through its split button and adds one under this task', async () => {
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      // An older hub with no start preview: the button starts at once.
      if (cmd === 'preview_start_work') throw { code: 'E_INVALID', message: 'unknown work_link action preview_start' };
      return { id: 1, title: 'x', source: 'local' };
    });
    render(TaskWorkSections, { detail });
    await fireEvent.click(within(screen.getByTestId('task-subtasks')).getByTestId('work-button-primary'));
    await flush();
    expect(calls('start_work')[0][1]).toMatchObject({ args: { item_id: 41, project_id: 3 } });
    await fireEvent.click(screen.getByTestId('task-add-subtask'));
    const input = screen.getByLabelText('Subtask title') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Follow-up' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    expect(calls('create_work_task')[0][1]).toEqual({ args: { title: 'Follow-up', parent: 'item:110' } });
  });

  it('keeps rejected proposals behind a toggle', async () => {
    render(TaskWorkSections, { detail });
    expect(screen.queryByText('Create om-catalog')).toBeNull();
    await fireEvent.click(screen.getByTestId('task-rejected-toggle'));
    expect(screen.getByText('Create om-catalog')).toBeTruthy();
  });

  it('a subtask cannot have subtasks of its own', () => {
    render(TaskWorkSections, { detail: { ...detail, task: task({ task_id: 'item:41', item_id: 41, parent_task_id: 'item:110' }) } });
    expect(screen.queryByTestId('task-add-subtask')).toBeNull();
  });

  it('renders one part at a time: the work above Sessions, the steps below', () => {
    const { unmount } = render(TaskWorkSections, { detail, part: 'work' });
    expect(screen.getByTestId('task-subtasks')).toBeTruthy();
    expect(screen.queryByTestId('task-steps')).toBeNull();
    unmount();
    render(TaskWorkSections, { detail, part: 'steps' });
    expect(screen.getByTestId('task-steps')).toBeTruthy();
    expect(screen.queryByTestId('task-subtasks')).toBeNull();
  });
  describe('Jev\'s "may duplicate" (redesign 6.9, K4)', () => {
    const dupDetail: TaskDetail = {
      ...detail,
      proposals: [
        {
          ...detail.proposals![0],
          duplicate: { item_id: 36, task_id: 'item:36', key: 'TASK-36', title: 'Pick the P0 owner', source: 'jev', confidence_pct: 82, run_id: 9 },
        },
      ],
    };

    it('shows May duplicate with Merge and Keep both in the New layout', async () => {
      render(TaskWorkSections, { detail: dupDetail });
      const dup = screen.getByTestId('task-proposal-duplicate');
      expect(dup.textContent).toContain('May duplicate TASK-36');
      expect(screen.getByTestId('task-proposal-duplicate-by').textContent).toContain('likely');
      expect(screen.queryByTestId('task-proposal-accept')).toBeNull();
      await fireEvent.click(screen.getByTestId('task-proposal-merge'));
      await flush();
      // Merge carries the task it duplicates, so the backend moves what hangs on it.
      expect(calls('reject_work_proposal')[0][1]).toEqual({ args: { item_id: 44, merge_into: 36 } });
      await fireEvent.click(screen.getByTestId('task-proposal-keep-both'));
      await flush();
      expect(calls('accept_work_proposal')[0][1]).toEqual({ args: { item_id: 44 } });
    });

    it('shows Accept and Reject when Jev flagged nothing', () => {
      render(TaskWorkSections, { detail });
      expect(screen.queryByTestId('task-proposal-duplicate')).toBeNull();
      expect(screen.getByTestId('task-proposal-accept')).toBeTruthy();
    });
  });
});

describe('TaskWorkSections in the New layout', () => {
  it('+ Add subtask adds one under this task', async () => {
    render(TaskWorkSections, { detail, part: 'work' });
    await flush();
    expect(screen.queryByTestId('task-subtask-start')).toBeNull(); // New starts through the split button
    await fireEvent.click(screen.getByTestId('task-add-subtask'));
    const input = screen.getByLabelText('Subtask title') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Follow-up' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    expect(calls('create_work_task')[0][1]).toEqual({ args: { title: 'Follow-up', parent: 'item:110' } });
  });

  it('with no subtasks yet, + Add subtask stays a button', async () => {
    render(TaskWorkSections, { detail: { ...detail, subtasks: [] }, part: 'work' });
    await flush();
    expect(screen.getByText('No subtasks yet.')).toBeTruthy();
    expect(screen.getByTestId('task-add-subtask').tagName).toBe('BUTTON');
    await fireEvent.click(screen.getByTestId('task-add-subtask'));
    expect(screen.getByLabelText('Subtask title')).toBeTruthy();
  });
});
