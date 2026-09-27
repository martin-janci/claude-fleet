// A task's detail in the center pane (work graph M14.2): the tracker data,
// where its org and group come from, every session with its why, the last
// outcome, and Open / Continue / Start new over the existing commands.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('./open_external', () => ({ openExternal: vi.fn(async () => true) }));
import { invoke } from '@tauri-apps/api/core';
import WorkTaskDetail from './WorkTaskDetail.svelte';
import { sessions } from './sessions';
import { projects, type ProjectTreeRow } from './projects';
import { session } from './hosts_fixture';
import { selectedSession, clearSelection } from './selection';
import { newSessionRequest, clearNewSessionRequest } from './new_session_request';
import { createWorkTreeStore } from './work_tree';
import type { TaskDetail } from './work_view';

const detail: TaskDetail = {
  task: {
    task_id: 'item:12',
    item_id: 12,
    key: 'ABC-12',
    title: 'Login <i>fails</i>',
    url: 'https://acme.atlassian.net/browse/ABC-12',
    kind: 'tracker',
    tracker_name: 'Jira (acme)',
    tracker_state: 'rate_limited',
    status_name: 'In Review',
    status_category: 'in_progress',
    unavailable: false,
    assignees: ['Ana'],
    mine: true,
    org_id: 1,
    org_source: 'tracker',
    org_fenced: true,
    org_mixed: false,
    group: { id: 'label:Payments', label: 'Payments', source: 'rule', rule_id: 3, editable: true },
    counts: { active: 1, ended: 1, suggested: 0 },
    needs_you: false,
    review: false,
    repos: ['acme/api'],
    placement_version: 0,
    sessions: [
      { link_id: 2, link_version: 1, state: 'ended', primary: false, name: 'old', host: 'h2', source: 'branch', why: 'branch abc-12', created_at: 1, ended_at: 100, needs_you: false, archived: false, resumable: true, cross_org: false, other_tasks: 0 },
      { link_id: 1, link_version: 1, state: 'active', primary: true, session_id: 41, name: 'api', host: 'mefistos', source: 'manual', why: 'linked by hand', created_at: 1, needs_you: false, archived: false, resumable: true, cross_org: false, other_tasks: 0 },
    ],
    sessions_more: 0,
  },
  description: 'When <script>x</script> the user logs in',
  last_outcome: { at: 100, name: 'old', host: 'h2', branch: 'abc-12', end_reason: 'killed' },
};

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

function mount() {
  return render(WorkTaskDetail, {
    props: { taskId: 'item:12', onclose: () => {}, store: createWorkTreeStore() },
  });
}

describe('WorkTaskDetail', () => {
  beforeEach(() => {
    clearSelection();
    clearNewSessionRequest();
    sessions.set([session('mefistos', 'api', { id: 41, project_id: 1 })]);
    projects.set([
      { project: { id: 1, owner: 'acme', repo: 'api', base_path: '/p', last_session_at: 1, adopted: false }, worktrees: [] } as unknown as ProjectTreeRow,
    ]);
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_task') return detail;
      if (cmd === 'work_resume_plan')
        return { key: 'ABC-12', modes: [{ mode: 'last', ok: true }], live: [], link_id: 2 };
      if (cmd === 'resume_work') return session('h2', 'old', { id: 77 });
      return null;
    });
  });

  it('shows the tracker data, provenance, sessions and last outcome as text', async () => {
    const { container } = mount();
    await flush();
    expect(screen.getByTestId('work-task-title').textContent).toBe('Login <i>fails</i>');
    expect(container.querySelector('i')).toBeNull();
    expect(container.querySelector('script')).toBeNull();
    expect(screen.getByTestId('work-task-desc').textContent).toContain('<script>x</script>');
    expect(screen.getByTestId('work-task-group').textContent).toContain('placement rule #3');
    expect(screen.getByTestId('work-task-org').textContent).toContain("the tracker's org");
    expect(screen.getByTestId('work-task-tracker-down')).toBeTruthy();
    const rows = screen.getAllByTestId('work-task-session');
    expect(rows.map((r) => r.getAttribute('data-kind'))).toEqual(['primary', 'past']);
    expect(rows[0].textContent).toContain('linked by hand');
    expect(screen.getByTestId('work-task-outcome').textContent).toContain('killed');
  });

  it('Open selects the live session', async () => {
    mount();
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-open'));
    expect(get(selectedSession)?.id).toBe(41);
  });

  it('Continue resumes the last conversation through resume_work', async () => {
    mount();
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-continue'));
    await flush();
    const call = vi.mocked(invoke).mock.calls.find((c) => c[0] === 'resume_work');
    expect((call![1] as { args: Record<string, unknown> }).args).toMatchObject({ key: 'ABC-12', mode: 'last', link_id: 2 });
  });

  it('Start new opens the new-session dialog for the ticket (its confirmation)', async () => {
    mount();
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-start'));
    const req = get(newSessionRequest);
    expect(req?.project.project.id).toBe(1);
    expect(req?.ticket).toMatchObject({ id: 12, key: 'ABC-12' });
    // Nothing started without the dialog.
    expect(vi.mocked(invoke).mock.calls.some((c) => c[0] === 'start_work')).toBe(false);
  });

  it('says so when the task is gone', async () => {
    vi.mocked(invoke).mockImplementation(async () => {
      throw { code: 'E_NOTFOUND', message: 'no such task' };
    });
    mount();
    await flush();
    expect(screen.getByTestId('work-task-error').textContent).toContain('gone');
  });
});
