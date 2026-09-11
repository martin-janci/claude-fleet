import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import TasksPanel from './TasksPanel.svelte';
import { tasks, type TaskRow } from './tasks';
import { sessions, type SessionRow } from './sessions';

function session(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: 1, tmux_name: 'ctl', host_alias: 'local', project_id: null, worktree_id: null,
    created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null,
    kind: 'work', reviews_session_id: null, worktree_key: null, lost_at: null,
    claude_session_id: null, claude_status: null, effort_level: null, pr_url: null,
    current_activity: null, context_pct: null, stuck_kind: null, friendly_name: null,
    safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null,
    idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null,
    last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [],
    ...over,
  };
}

function task(over: Partial<TaskRow> = {}): TaskRow {
  return {
    id: 1, requester_session_id: 1, worker_session_id: 2,
    prompt: 'fix the flaky test\nsecond line', state: 'running', result: null, error: null,
    created_at: 100, started_at: 101, finished_at: null, ...over,
  };
}

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  sessions.set([
    session({ id: 1, tmux_name: 'ctl', friendly_name: 'controller' }),
    session({ id: 2, tmux_name: 'worker-a' }),
    session({ id: 3, tmux_name: 'unrelated' }),
  ]);
  // Newest-first, as the store keeps them (created_at desc).
  tasks.set([
    task({ id: 1, state: 'running', created_at: 300 }),
    task({ id: 2, state: 'done', result: 'All green.', created_at: 200, finished_at: 260, requester_session_id: 3, worker_session_id: 2 }),
    task({ id: 3, state: 'failed', error: 'send failed', created_at: 100, requester_session_id: 3, worker_session_id: 3 }),
  ]);
});

describe('TasksPanel', () => {
  it('fleet view lists every task with state pill, parties, prompt first line and result/error', () => {
    render(TasksPanel, {});
    const rows = screen.getAllByTestId('task-row');
    expect(rows.length).toBe(3);
    const states = screen.getAllByTestId('task-state').map((e) => e.textContent?.trim());
    expect(states).toEqual(['running', 'done', 'failed']);
    // requester → worker uses friendly name when set, tmux_name otherwise.
    expect(screen.getAllByTestId('task-parties')[0].textContent).toContain('controller');
    expect(screen.getAllByTestId('task-parties')[0].textContent).toContain('worker-a');
    expect(screen.getAllByTestId('task-prompt')[0].textContent).toBe('fix the flaky test');
    expect(screen.getByTestId('task-result').textContent).toBe('All green.');
    expect(screen.getByTestId('task-error').textContent).toBe('send failed');
    // Only the open task offers Cancel.
    expect(screen.getAllByTestId('task-cancel').length).toBe(1);
  });

  it('session view narrows to tasks where the session is requester or worker', () => {
    render(TasksPanel, { sessionId: 2 });
    expect(screen.getAllByTestId('task-row').length).toBe(2);
    expect(screen.getByTestId('tasks-panel').dataset.scope).toBe('session');
  });

  it('shows an empty state when nothing matches', () => {
    render(TasksPanel, { sessionId: 99 });
    expect(screen.getByTestId('tasks-empty')).toBeTruthy();
  });

  it('cancel goes through the confirm dialog and calls cancel_task', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue(
      task({ id: 1, state: 'cancelled', error: 'cancelled from the desktop', created_at: 300, finished_at: 370 }),
    );
    render(TasksPanel, {});
    await fireEvent.click(screen.getByTestId('task-cancel'));
    expect(screen.getByTestId('confirm-dialog')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('confirm-cancel-task'));
    await tick();
    await Promise.resolve();
    await tick();
    expect(mockedInvoke).toHaveBeenCalledWith('cancel_task', { taskId: 1 });
    const states = screen.getAllByTestId('task-state').map((e) => e.textContent?.trim());
    expect(states[0]).toBe('cancelled');
    expect(screen.queryByTestId('task-cancel')).toBeNull();
  });

  it('a task:updated merge re-renders the row', async () => {
    render(TasksPanel, {});
    tasks.update((arr) => arr.map((t) => (t.id === 1 ? { ...t, state: 'done', result: 'shipped', finished_at: 200 } : t)));
    await tick();
    const states = screen.getAllByTestId('task-state').map((e) => e.textContent?.trim());
    expect(states[0]).toBe('done');
    expect(screen.getAllByTestId('task-result')[0].textContent).toBe('shipped');
  });
});
