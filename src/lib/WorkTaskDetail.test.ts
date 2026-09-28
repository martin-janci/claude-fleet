// A task in Details (work graph M14): where its org and group come from,
// every session with its why, Continue / Start new, and the edits — the
// placement (with its conflict), the org move behind its impact preview, and
// a rule saved only after a preview.
import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('./open_external', () => ({ openExternal: vi.fn(async () => true) }));
import { invoke } from '@tauri-apps/api/core';
import WorkTaskDetail from './WorkTaskDetail.svelte';
import { sessions } from './sessions';
import { selectedSession, clearSelection } from './selection';
import { session } from './hosts_fixture';
import { link, task } from './work_view_fixture';
import { selectedTaskId, workTreeMeta, type OrgImpact, type TaskDetail } from './work_view';

const trackerTask: TaskDetail = {
  task: task({
    sessions: [
      link({ link_id: 42, session_id: 7, primary: true, evidence: [{ signal: 'branch', rule: 'R3', text: 'abc-12-login', at: 1790000000 }] }),
      link({ link_id: 45, session_id: 9, name: 'web', state: 'suggested', primary: false, why: 'mentioned ABC-12 in a prompt · R6' }),
      link({ link_id: 40, session_id: null, name: 'old-1', state: 'ended', primary: false, ended_at: 1789990000 }),
      link({ link_id: 41, session_id: null, name: 'old-2', state: 'ended', primary: false, ended_at: 1789995000 }),
    ],
  }),
  aliases: [],
  description: 'Steps: <script>alert(1)</script> log in',
  last_outcome: { at: 1789995000, name: 'old-2', host: 'mefistos', branch: 'abc-12-login', pr_url: null, summary: 'Fixed the token refresh.' },
  placement: null,
  rules: [],
};

const localTask: TaskDetail = {
  task: task({
    task_id: 'item:77',
    item_id: 77,
    key: null,
    title: 'Clean up the CI cache',
    url: null,
    kind: 'local',
    tracker_id: null,
    tracker_name: null,
    provider: null,
    tracker_state: null,
    org_id: null,
    org_source: 'sessions',
    org_fenced: false,
    group: { id: 'repo:acme/api', label: 'acme/api', source: 'repo', editable: true },
    placement_version: 0,
    sessions: [link({ link_id: 5, session_id: 9, name: 'api', host: 'h-a' })],
  }),
  rules: [],
};

const impact = (token: string, over: Partial<OrgImpact> = {}): OrgImpact => ({
  task_id: 'item:77',
  from_org: null,
  to_org: 2,
  allowed: true,
  reason: null,
  links: [{ link_id: 5, session_id: 9, name: 'api', host: 'h-a', state: 'active', session_org: 1, becomes_cross_org: true }],
  hosts_losing: ['h-b'],
  hosts_gaining: ['h-c'],
  bound_clients_losing: 1,
  bound_clients_gaining: 0,
  journal_entries: 4,
  summaries: 1,
  impact_token: token,
  ...over,
});

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;

function calls(cmd: string) {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);
}

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

describe('WorkTaskDetail', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    clearSelection();
    selectedTaskId.set(null);
    sessions.set([session('mefistos', 'api', { id: 7 }), session('mefistos', 'web', { id: 9 })]);
    workTreeMeta.set({
      orgs: [
        { id: 1, name: 'Acme', color: null },
        { id: 2, name: 'Globex', color: null },
      ],
      trackers: [{ id: 1, name: 'Jira (acme)', provider: 'jira', state: 'ok', org_id: 1 }],
      groups: [
        { org_id: 1, group: { id: 'label:Payments', label: 'Payments', source: 'rule', rule_id: 3 }, count: 3 },
        { org_id: 1, group: { id: 'label:Infra', label: 'Infra', source: 'manual' }, count: 1 },
      ],
    });
    handlers = {
      work_task: (a) => (a.task_id === 'item:77' ? localTask : trackerTask),
      work_rules: () => [],
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
  });

  it('shows the tracker’s data as text and where the org and group come from', async () => {
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    expect(calls('work_task')[0]).toEqual({ task_id: 'item:12' });
    expect(screen.getByTestId('work-task-status').textContent).toBe('In Review');
    expect(screen.getByTestId('work-task-assignees').textContent).toContain('Ana');
    // Third-party text is text.
    expect(screen.getByTestId('work-task-description').textContent).toBe('Steps: <script>alert(1)</script> log in');
    expect(screen.getByTestId('work-task-org').textContent?.replace(/\s+/g, ' ').trim()).toBe('Acme — from tracker Jira (acme)');
    expect(screen.getByTestId('work-task-group').textContent).toContain('from the tracker: Jira (acme) ABC');
    expect(screen.getByTestId('work-task-group-note').textContent).toBe(
      'Placing it elsewhere is local to fleet: it never changes Jira.',
    );
    expect(screen.getByTestId('work-task-repos').textContent).toBe('acme/api');
    // Every session with its state and why; evidence lines under it.
    const links = screen.getAllByTestId('work-task-link');
    expect(links.map((l) => l.getAttribute('data-kind'))).toEqual(['primary', 'suggested', 'past', 'past']);
    expect(within(links[0]).getByTestId('work-task-link-state').textContent).toBe('active · primary');
    expect(within(links[0]).getByTestId('work-task-evidence').textContent).toContain('branch `abc-12-login`');
    expect(within(links[1]).getByTestId('work-task-link-why').textContent).toContain('R6');
    // Past, newest first.
    expect(links[2].textContent).toContain('old-2');
    expect(screen.getByTestId('work-task-outcome').textContent).toContain('Fixed the token refresh.');
    // A tracker's task has no org to assign: its tracker's is its org.
    expect(screen.queryByTestId('work-task-assign-org')).toBeNull();
  });

  it('an inferred org says it is not a boundary', async () => {
    render(WorkTaskDetail, { taskId: 'item:77' });
    await flush();
    expect(screen.getByTestId('work-task-org').textContent).toContain('inferred from its sessions — not a boundary');
    expect(screen.getByTestId('work-task-group').textContent).toContain('most recent session (acme/api)');
  });

  it('Open, Continue (resume last) and Start new', async () => {
    handlers.resume_work = () => session('mefistos', 'resumed', { id: 11 });
    handlers.start_work = () => session('mefistos', 'fresh', { id: 12 });
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-open'));
    expect(get(selectedSession)?.id).toBe(7);
    await fireEvent.click(screen.getByTestId('work-task-continue'));
    await flush();
    expect(calls('resume_work')[0]).toEqual({ key: 'ABC-12', mode: 'last', link_id: 41, host_alias: null, brief: null });
    expect(get(selectedSession)?.id).toBe(11);
    await fireEvent.click(screen.getByTestId('work-task-start'));
    await flush();
    expect(calls('start_work')[0]).toEqual({ item_id: 12 });
    expect(get(selectedSession)?.id).toBe(12);
  });

  it('Assign org shows the impact exactly, then moves with its token', async () => {
    let n = 0;
    handlers.work_org_impact = () => impact(`tok-${++n}`);
    handlers.assign_work_org = () => ({ ...localTask.task, org_id: 2, org_source: 'item' });
    render(WorkTaskDetail, { taskId: 'item:77' });
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-assign-org'));
    await flush();
    expect(screen.getByTestId('work-org-confirm').hasAttribute('disabled')).toBe(true);
    await fireEvent.change(screen.getByTestId('work-org-target'), { target: { value: '2' } });
    await fireEvent.click(screen.getByTestId('work-org-review'));
    await flush();
    expect(calls('work_org_impact')[0]).toEqual({ task_id: 'item:77', org_id: 2 });
    expect(screen.getByTestId('work-org-impact-link').textContent).toContain('api');
    expect(screen.getByTestId('work-org-cross')).toBeTruthy();
    expect(screen.getByTestId('work-org-hosts-losing').textContent).toContain('h-b');
    expect(screen.getByTestId('work-org-hosts-gaining').textContent).toContain('h-c');
    expect(screen.getByTestId('work-org-clients').textContent).toContain('1 stop, 0 start');
    expect(screen.getByTestId('work-org-journal').textContent).toContain('4 journal entries and 1 summary');
    await fireEvent.click(screen.getByTestId('work-org-confirm'));
    await flush();
    expect(calls('assign_work_org')[0]).toEqual({ task_id: 'item:77', org_id: 2, impact_token: 'tok-1' });
    expect(screen.queryByTestId('work-org-dialog')).toBeNull();
  });

  it('Assign org: a changed impact is shown again and never sent with the old token', async () => {
    let n = 0;
    handlers.work_org_impact = () => impact(`tok-${++n}`, n > 1 ? { hosts_losing: ['h-b', 'h-d'] } : {});
    let first = true;
    handlers.assign_work_org = () => {
      if (first) {
        first = false;
        throw { code: 'E_CONFLICT', message: 'impact changed', details: {} };
      }
      return localTask.task;
    };
    render(WorkTaskDetail, { taskId: 'item:77' });
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-assign-org'));
    await fireEvent.change(screen.getByTestId('work-org-target'), { target: { value: '2' } });
    await fireEvent.click(screen.getByTestId('work-org-review'));
    await flush();
    await fireEvent.click(screen.getByTestId('work-org-confirm'));
    await flush();
    expect(screen.getByTestId('work-org-changed').textContent).toContain('The impact changed since you reviewed it');
    expect(screen.getByTestId('work-org-hosts-losing').textContent).toContain('h-d');
    await fireEvent.click(screen.getByTestId('work-org-confirm'));
    await flush();
    expect(calls('assign_work_org').map((a) => a.impact_token)).toEqual(['tok-1', 'tok-2']);
  });

  it('Assign org: an inferred org is not "now"; a changed target drops the old impact', async () => {
    handlers.work_task = () => ({ ...localTask, task: { ...localTask.task, org_id: 1, org_source: 'sessions', org_fenced: false } });
    handlers.work_org_impact = (a) => impact(`tok-${a.org_id}`, { to_org: a.org_id as number });
    render(WorkTaskDetail, { taskId: 'item:77' });
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-assign-org'));
    await flush();
    const dialog = screen.getByTestId('work-org-dialog');
    expect(dialog.textContent).toContain('Now: no organisation');
    // Its sessions' org is a target like any other; "No organisation" is where it is.
    const options = Array.from((screen.getByTestId('work-org-target') as HTMLSelectElement).options, (o) => o.value);
    expect(options).toEqual(['', '1', '2']);
    await fireEvent.change(screen.getByTestId('work-org-target'), { target: { value: '2' } });
    await fireEvent.click(screen.getByTestId('work-org-review'));
    await flush();
    expect(screen.getByTestId('work-org-impact')).toBeTruthy();
    await fireEvent.change(screen.getByTestId('work-org-target'), { target: { value: '1' } });
    await flush();
    expect(screen.queryByTestId('work-org-impact')).toBeNull();
    expect(screen.getByTestId('work-org-confirm').hasAttribute('disabled')).toBe(true);
    await fireEvent.click(screen.getByTestId('work-org-review'));
    await flush();
    handlers.assign_work_org = () => localTask.task;
    await fireEvent.click(screen.getByTestId('work-org-confirm'));
    await flush();
    expect(calls('assign_work_org')).toEqual([{ task_id: 'item:77', org_id: 1, impact_token: 'tok-1' }]);
  });

  it('Place in group keeps the placement note unless edited, and shows the new placement at once', async () => {
    const placed = { group: 'Infra', note: 'owned by ops', version: 2, updated_at: null, updated_by: 'mj' };
    handlers.work_task = () => ({ ...trackerTask, task: { ...trackerTask.task, placement_version: 2 }, placement: placed });
    handlers.place_work = () => ({
      ...trackerTask.task,
      group: { id: 'label:Payments', label: 'Payments', source: 'manual' },
      placement_version: 3,
    });
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    expect(screen.getByTestId('work-task-placement').textContent).toContain('owned by ops');
    await fireEvent.click(screen.getByTestId('work-task-place'));
    await flush();
    expect((screen.getByTestId('work-place-note-input') as HTMLInputElement).value).toBe('owned by ops');
    await fireEvent.input(screen.getByTestId('work-place-group'), { target: { value: 'Payments' } });
    // The detail's next read answers the new placement.
    handlers.work_task = () => ({
      ...trackerTask,
      task: { ...trackerTask.task, placement_version: 3 },
      placement: { ...placed, group: 'Payments', version: 3 },
    });
    await fireEvent.click(screen.getByTestId('work-place-submit'));
    await flush();
    expect(calls('place_work')[0]).toEqual({ task_id: 'item:12', group: 'Payments', note: 'owned by ops', expected_version: 2 });
    expect(calls('work_task').length).toBeGreaterThanOrEqual(2);
  });

  it('a tracker-controlled org is refused with the admin path', async () => {
    handlers.work_org_impact = () => impact('x', { allowed: false, reason: 'tracker_controlled', impact_token: null });
    render(WorkTaskDetail, { taskId: 'item:77' });
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-assign-org'));
    await fireEvent.change(screen.getByTestId('work-org-target'), { target: { value: '2' } });
    await fireEvent.click(screen.getByTestId('work-org-review'));
    await flush();
    expect(screen.getByTestId('work-org-refused').textContent).toContain('assign_tracker');
    expect(screen.getByTestId('work-org-confirm').hasAttribute('disabled')).toBe(true);
  });

  it('Place in group: sends the placement version; a conflict reloads and says so', async () => {
    handlers.place_work = () => {
      throw { code: 'E_CONFLICT', message: 'placement changed', details: { task_id: 'item:12', version: 2, group: 'Infra' } };
    };
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-place'));
    await flush();
    expect(screen.getByTestId('work-place-note').textContent).toContain('never changes Jira');
    await fireEvent.click(screen.getAllByTestId('work-place-label').find((b) => b.textContent === 'Payments')!);
    await fireEvent.click(screen.getByTestId('work-place-submit'));
    await flush();
    expect(calls('place_work')[0]).toEqual({ task_id: 'item:12', group: 'Payments', expected_version: 0 });
    expect(screen.getByTestId('work-place-error').textContent).toContain('placed elsewhere');
    // The current value, with Reload.
    expect(screen.getByTestId('work-conflict-current').textContent).toBe('Now: placed in “Infra” · version 2');
    expect(calls('work_task').length).toBe(2);
    await fireEvent.click(screen.getByTestId('work-conflict-reload'));
    await flush();
    expect(calls('work_task').length).toBe(3);
  });

  it('Place in group, then a rule for similar tasks — saved only after its preview', async () => {
    handlers.place_work = () => ({ ...trackerTask.task, group: { id: 'label:Payments', label: 'Payments', source: 'manual' }, placement_version: 1 });
    handlers.work_rule_preview = () => ({
      affected: [
        {
          task_id: 'item:14',
          key: 'ABC-14',
          title: 'Refund',
          from: { id: 'tracker:1:ABC', label: 'ABC', source: 'tracker' },
          to: { id: 'label:Payments', label: 'Payments', source: 'rule' },
        },
      ],
      total: 1,
      kept_manual: 2,
    });
    handlers.save_work_rule = (a) => ({ id: 9, version: 1, ...(a.rule as object) });
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-place'));
    await fireEvent.input(screen.getByTestId('work-place-group'), { target: { value: 'Payments' } });
    await fireEvent.click(screen.getByTestId('work-place-submit'));
    await flush();
    expect(screen.getByTestId('work-place-done').textContent).toContain('Payments');
    await fireEvent.click(screen.getByTestId('work-place-make-rule'));
    await flush();
    // Prefilled from where the task came from: its tracker and project.
    expect((screen.getByTestId('rule-group') as HTMLInputElement).value).toBe('Payments');
    expect((screen.getByTestId('rule-container') as HTMLInputElement).value).toBe('ABC');
    expect((screen.getByTestId('rule-tracker') as HTMLSelectElement).value).toBe('1');
    expect(screen.getByTestId('rule-save').hasAttribute('disabled')).toBe(true);
    await fireEvent.click(screen.getByTestId('rule-preview-btn'));
    await flush();
    expect(screen.getAllByTestId('rule-preview-row')[0].textContent).toContain('ABC → Payments');
    expect(screen.getByTestId('rule-preview-kept').textContent).toContain('2 placed by a person');
    // Changing the draft makes the preview stale again.
    await fireEvent.input(screen.getByTestId('rule-key-prefix'), { target: { value: 'ABC' } });
    await flush();
    expect(screen.getByTestId('rule-save').hasAttribute('disabled')).toBe(true);
    expect(screen.getByTestId('rule-preview-stale')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('rule-preview-btn'));
    await flush();
    await fireEvent.click(screen.getByTestId('rule-save'));
    await flush();
    expect(calls('save_work_rule')[0]).toEqual({
      rule: {
        name: 'Payments',
        enabled: true,
        conditions: { tracker_id: 1, container: 'ABC', key_prefix: 'ABC', title_contains: null, repo: null },
        group: 'Payments',
        expected_version: 0,
      },
    });
    expect(calls('work_rule_preview')).toHaveLength(2);
  });

  it('Continue refused because the work is live already offers to open that session', async () => {
    handlers.resume_work = () => {
      throw { code: 'E_EXISTS', message: 'ABC-12 is live in api on mefistos — jump to it' };
    };
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-continue'));
    await flush();
    expect(screen.getByTestId('work-task-action-error').textContent).toContain('is live in api');
    await fireEvent.click(screen.getByTestId('work-task-open-existing'));
    expect(get(selectedSession)?.id).toBe(7);
  });

  it('Continue: an E_EXISTS naming its session opens that one', async () => {
    handlers.resume_work = () => {
      throw { code: 'E_EXISTS', message: 'being resumed already', details: { session_id: 9 } };
    };
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-continue'));
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-open-existing'));
    expect(get(selectedSession)?.id).toBe(9);
  });

  it('a failed refresh keeps the task shown, with a line to retry', async () => {
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    expect(screen.getByTestId('work-task-status').textContent).toBe('In Review');
    handlers.work_task = () => {
      throw { code: 'E_HUB_DOWN', message: 'hub unreachable' };
    };
    await fireEvent.click(screen.getByTestId('work-task-refresh'));
    await flush();
    expect(screen.getByTestId('work-task-status').textContent).toBe('In Review');
    expect(screen.queryByTestId('work-task-error')).toBeNull();
    expect(screen.getByTestId('work-task-refresh-error').textContent).toContain('hub unreachable');
    handlers.work_task = () => trackerTask;
    await fireEvent.click(screen.getByTestId('work-task-refresh-retry'));
    await flush();
    expect(screen.queryByTestId('work-task-refresh-error')).toBeNull();
  });

  it('a first load that fails shows the error', async () => {
    handlers.work_task = () => {
      throw { code: 'E_HUB_DOWN', message: 'hub unreachable' };
    };
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    expect(screen.getByTestId('work-task-error').textContent).toContain('hub unreachable');
  });

  it('a task that is gone (or not visible) says so', async () => {
    handlers.work_task = () => {
      throw { code: 'E_NOTFOUND', message: 'no such task' };
    };
    render(WorkTaskDetail, { taskId: 'item:404' });
    await flush();
    expect(screen.getByTestId('work-task-error').textContent).toContain('no longer exists');
  });

  it('follows a bare key that a sync bound to a ticket', async () => {
    handlers.work_task = () => ({ ...trackerTask, aliases: ['ref:ABC-12'] });
    selectedTaskId.set('ref:ABC-12');
    render(WorkTaskDetail, { taskId: 'ref:ABC-12' });
    await flush();
    expect(get(selectedTaskId)).toBe('item:12');
  });
});
