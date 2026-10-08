// Redesign step 6.6: in the New layout every place a session starts from a
// task uses one split button (WorkButton) with the same words, "Continue" and
// "Start new"; Classic keeps its own buttons.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkTaskDetail from './WorkTaskDetail.svelte';
import TaskWorkSections from './TaskWorkSections.svelte';
import ResumeButton from './ResumeButton.svelte';
import WorkButton from './WorkButton.svelte';
import { sessions } from './sessions';
import { selectedSession, clearSelection } from './selection';
import { session } from './hosts_fixture';
import { link, task } from './work_view_fixture';
import { uiLayout } from './prefs';
import type { TaskDetail } from './work_view';

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;

const calls = (cmd: string) =>
  vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

const cleanPreview = () => ({
  key: 'ABC-12',
  title: 'Login',
  item_id: 12,
  plan: { key: 'ABC-12', title: 'Login', item_id: 12, project_id: 3, host_alias: 'mefistos', branch: 'abc-12-login', name: 'ABC-12 Login' },
  projects: [{ id: 3, owner: 'acme', repo: 'api' }],
  hosts: [{ alias: 'mefistos', reachable: true }],
  conflicts: [],
  brief: null,
  checkout: { exists: false },
});

const pastOnly: TaskDetail = {
  task: task({
    sessions: [link({ link_id: 41, session_id: null, name: 'old', state: 'ended', primary: false, ended_at: 1789995000 })],
  }),
  rules: [],
};

const fresh: TaskDetail = { task: task({ sessions: [] }), rules: [] };

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  clearSelection();
  sessions.set([session('mefistos', 'api', { id: 7 })]);
  handlers = { work_rules: () => [], session_history: () => [] };
  vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
    const h = handlers[cmd];
    return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
  });
});

afterEach(() => uiLayout.set('classic'));

describe('the task page', () => {
  it('New: one split button, Continue when a past session can resume', async () => {
    uiLayout.set('new');
    handlers.work_task = () => pastOnly;
    handlers.resume_work = () => session('mefistos', 'resumed', { id: 11 });
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    expect(screen.queryByTestId('work-task-start')).toBeNull();
    const primary = screen.getByTestId('work-button-primary');
    expect(primary.textContent).toBe('Continue');
    await fireEvent.click(primary);
    await flush();
    expect(calls('resume_work')[0]).toMatchObject({ key: 'ABC-12', mode: 'last', link_id: 41 });
    expect(get(selectedSession)?.id).toBe(11);
  });

  it('New: Start new starts from a clean preview and shows the start progress', async () => {
    uiLayout.set('new');
    handlers.work_task = () => fresh;
    handlers.preview_start_work = cleanPreview;
    handlers.start_work = () => {
      sessions.update((list) => [...list, session('mefistos', 'fresh', { id: 12 })]);
      return session('mefistos', 'fresh', { id: 12 });
    };
    handlers.session_history = () => [
      { id: 1, session_id: 12, at: 1, kind: 'start_spawned', detail: JSON.stringify({ key: 'ABC-12', worktree_id: null, new_worktree: true, branch: 'abc-12-login', brief: true }) },
    ];
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    const primary = screen.getByTestId('work-button-primary');
    expect(primary.textContent).toBe('Start new');
    await fireEvent.click(primary);
    await flush();
    expect(calls('start_work')[0]).toEqual({ item_id: 12, with_brief: true, project_id: 3, host_alias: 'mefistos' });
    expect(get(selectedSession)?.id).toBe(12);
    expect(screen.getByTestId('work-button').querySelector('[data-testid="start-progress"]')).not.toBeNull();
  });

  it('Classic keeps Open, Continue and Start new as three buttons', async () => {
    handlers.work_task = () => fresh;
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    expect(screen.getByTestId('work-task-start').textContent).toBe('Start new');
    expect(screen.queryByTestId('work-button')).toBeNull();
  });
});

describe('a subtask', () => {
  const detail: TaskDetail = {
    task: task({ task_id: 'item:110', item_id: 110, key: 'OM-110' }),
    subtasks: [
      { task_id: 'item:41', item_id: 41, key: 'TASK-41', title: 'SELECT stats', origin: 'manual', status: 'todo', project_id: 3, live_sessions: 0, job_state: null },
    ],
  } as TaskDetail;

  it('New: starts through the same split button', async () => {
    uiLayout.set('new');
    render(TaskWorkSections, { detail, part: 'work' });
    await flush();
    expect(screen.queryByTestId('task-subtask-start')).toBeNull();
    expect(screen.getByTestId('work-button-primary').textContent).toBe('Start new');
  });

  it('Classic keeps its Start button', async () => {
    render(TaskWorkSections, { detail, part: 'work' });
    await flush();
    expect(screen.getByTestId('task-subtask-start').textContent).toBe('Start');
    expect(screen.queryByTestId('work-button')).toBeNull();
  });
});

describe('the words', () => {
  it('a past-work Resume says Continue in New and Resume in Classic', async () => {
    const { unmount } = render(ResumeButton, { workKey: 'ABC-12' });
    expect(screen.getByTestId('resume-quick').textContent).toBe('Resume');
    unmount();
    uiLayout.set('new');
    render(ResumeButton, { workKey: 'ABC-12' });
    expect(screen.getByTestId('resume-quick').textContent).toBe('Continue');
  });

  it('a task row says Start in Classic and Start new in New', async () => {
    const { unmount } = render(WorkButton, { task: task({ sessions: [] }) });
    expect(screen.getByTestId('work-button-primary').textContent).toBe('Start');
    unmount();
    uiLayout.set('new');
    render(WorkButton, { task: task({ sessions: [] }) });
    expect(screen.getByTestId('work-button-primary').textContent).toBe('Start new');
  });
});
