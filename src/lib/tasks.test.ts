import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import {
  tasks,
  loadTasks,
  cancelTask,
  mergeTask,
  applyTaskEvents,
  isTerminal,
  promptFirstLine,
  taskElapsed,
  type TaskRow,
} from './tasks';

function task(over: Partial<TaskRow> = {}): TaskRow {
  return {
    id: 1,
    requester_session_id: 10,
    worker_session_id: 20,
    prompt: 'fix the flaky test\n\nWhen finished, print exactly …',
    state: 'running',
    result: null,
    error: null,
    created_at: 100,
    started_at: 101,
    finished_at: null,
    ...over,
  };
}

beforeEach(() => {
  tasks.set([]);
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
});

describe('tasks store', () => {
  it('loadTasks fills the store from list_tasks', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue([task({ id: 2 }), task({ id: 1 })]);
    const r = await loadTasks();
    expect(r.ok).toBe(true);
    expect(mockedInvoke).toHaveBeenCalledWith('list_tasks', {});
    expect(get(tasks).map((t) => t.id)).toEqual([2, 1]);
  });

  it('mergeTask replaces by id and keeps newest-first order', () => {
    applyTaskEvents([
      { type: 'updated', row: task({ id: 1, created_at: 100 }) },
      { type: 'updated', row: task({ id: 2, created_at: 200 }) },
    ]);
    expect(get(tasks).map((t) => t.id)).toEqual([2, 1]);
    mergeTask(task({ id: 1, created_at: 100, state: 'done', result: 'ok' }));
    const rows = get(tasks);
    expect(rows.length).toBe(2);
    expect(rows[1].state).toBe('done');
    expect(rows[1].result).toBe('ok');
    // Same created_at: higher id first.
    mergeTask(task({ id: 3, created_at: 200 }));
    expect(get(tasks).map((t) => t.id)).toEqual([3, 2, 1]);
  });

  it('applyTaskEvents with no events leaves the store untouched', () => {
    const before = get(tasks);
    applyTaskEvents([]);
    expect(get(tasks)).toBe(before);
  });

  it('cancelTask calls the command and merges the returned row', async () => {
    tasks.set([task({ id: 7 })]);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue(
      task({ id: 7, state: 'cancelled', error: 'cancelled from the desktop', finished_at: 150 }),
    );
    const r = await cancelTask(7);
    expect(r.ok).toBe(true);
    expect(mockedInvoke).toHaveBeenCalledWith('cancel_task', { taskId: 7 });
    expect(get(tasks)[0].state).toBe('cancelled');
  });

  it('cancelTask surfaces an IpcError without touching the store', async () => {
    tasks.set([task({ id: 7, state: 'done' })]);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValue({
      code: 'E_TASK_TERMINAL',
      message: 'task 7 is already done',
    });
    const r = await cancelTask(7);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe('E_TASK_TERMINAL');
    expect(get(tasks)[0].state).toBe('done');
  });
});

describe('task helpers', () => {
  it('isTerminal', () => {
    expect(isTerminal('queued')).toBe(false);
    expect(isTerminal('running')).toBe(false);
    expect(isTerminal('done')).toBe(true);
    expect(isTerminal('failed')).toBe(true);
    expect(isTerminal('cancelled')).toBe(true);
  });

  it('promptFirstLine takes the first non-blank line and caps it', () => {
    expect(promptFirstLine('\n\n  hello world  \nmore')).toBe('hello world');
    expect(promptFirstLine(null)).toBe('');
    expect(promptFirstLine('x'.repeat(200), 10)).toBe('xxxxxxxxx…');
  });

  it('taskElapsed uses started→finished for finished tasks and now otherwise', () => {
    expect(taskElapsed(task({ started_at: 100, finished_at: 130 }), 9999)).toBe('30s');
    expect(taskElapsed(task({ started_at: 100, finished_at: null }), 100 + 3600 * 2 + 60)).toBe(
      '2h 1m',
    );
    expect(taskElapsed(task({ started_at: null, created_at: 100, finished_at: null }), 160)).toBe(
      '1m',
    );
    expect(taskElapsed(task({ started_at: 100, finished_at: 100 + 86400 + 3600 }), 0)).toBe(
      '1d 1h',
    );
  });
});
