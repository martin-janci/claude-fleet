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
import { openExternal } from './open_external';
import WorkTaskDetail from './WorkTaskDetail.svelte';
import { expectAccessible } from './a11y_check';
import { expectOnePrimary } from './action_hierarchy_check';
import { sessions } from './sessions';
import { selectedSession, clearSelection } from './selection';
import { session } from './hosts_fixture';
import { link, task } from './work_view_fixture';
import { noteWorkChanged, selectedTaskId, workTreeMeta, type OrgImpact, type TaskDetail } from './work_view';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';

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
    group: { id: 'repo:acme/api', label: 'acme/api', source: 'repo' },
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

/** The task page's action bar: one WorkButton (redesign 6.6). */
const bar = () => document.querySelector('.wb--bar') as HTMLElement;
async function fromMenu(testid: string) {
  await fireEvent.click(within(bar()).getByTestId('work-button-menu'));
  await flush();
  await fireEvent.click(within(bar()).getAllByTestId(testid)[0]);
  await flush();
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

  it('a local task offers Edit, which opens the edit dialog; a ticket does not', async () => {
    const { unmount } = render(WorkTaskDetail, { props: { taskId: 'ABC-12' } });
    await flush();
    expect(screen.queryByTestId('work-task-edit')).toBeNull();
    unmount();
    render(WorkTaskDetail, { props: { taskId: 'item:77' } });
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-edit'));
    await flush();
    expect(screen.getByTestId('edit-task-dialog')).toBeTruthy();
    expect((screen.getByTestId('edit-task-title') as HTMLInputElement).value).toBe('Clean up the CI cache');
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
    // Every session with its state and why, on the Sessions tab; evidence
    // lines under it.
    await fireEvent.click(screen.getByTestId('work-task-tab-sessions'));
    const links = screen.getAllByTestId('work-task-link');
    expect(links.map((l) => l.getAttribute('data-kind'))).toEqual(['primary', 'suggested', 'past', 'past']);
    expect(within(links[0]).getByTestId('work-task-link-state').textContent).toBe('active · primary');
    expect(within(links[0]).getByTestId('work-task-evidence').textContent).toContain('branch `abc-12-login`');
    expect(within(links[1]).getByTestId('work-task-link-why').textContent).toContain('R6');
    // Past, newest first.
    expect(links[2].textContent).toContain('old-2');
    // The last outcome is Activity's.
    await fireEvent.click(screen.getByTestId('work-task-tab-activity'));
    expect(screen.getByTestId('work-task-outcome').textContent).toContain('Fixed the token refresh.');
    // A tracker's task has no org to assign: its tracker's is its org.
    expect(screen.queryByTestId('work-task-assign-org')).toBeNull();
  });

  it('a whole description carries no cut notice', async () => {
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    expect(screen.queryByTestId('work-task-description-cut')).toBeNull();
  });

  it('the notice names shown and full lengths; its link opens the ticket', async () => {
    const excerpt = 'é'.repeat(600);
    handlers.work_task = () => ({ ...trackerTask, description: excerpt, description_chars: 6812, description_truncated: true });
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    const notice = screen.getByTestId('work-task-description-cut');
    expect(notice.textContent?.replace(/\s+/g, ' ').trim()).toBe('Shown 600 of 6812 characters — open the ticket');
    await fireEvent.click(within(notice).getByTestId('work-task-description-open'));
    expect(vi.mocked(openExternal)).toHaveBeenCalledWith(trackerTask.task.url);
  });

  it('a live session with a PR shows its Result; an ended one does not', async () => {
    const now = Math.floor(Date.now() / 1000);
    const head = '1490bc3a9275fba9c26757c531b7702f5a68df8c';
    sessions.set([
      session('mefistos', 'api', {
        id: 7,
        pr_url: 'https://github.com/o/r/pull/5',
        pr_checked_at: now - 60,
        pr_evidence: {
          head_oid: head, local_head: head, ahead: 0, dirty: false, draft: false, state: 'OPEN',
          checks: { total: 2, pending: 0, skipped: 0, failing_total: 1, failing: [{ name: 'rust' }] },
        },
      }),
      session('mefistos', 'web', { id: 9 }),
    ]);
    const withPrs: TaskDetail = {
      ...trackerTask,
      task: {
        ...trackerTask.task,
        sessions: (trackerTask.task.sessions ?? []).map((l) => ({ ...l, pr_url: 'https://github.com/o/r/pull/5' })),
      },
    };
    handlers.work_task = () => withPrs;
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    // The Delivery block names the live PR with its checks.
    expect(screen.getByTestId('work-task-delivery-pr').textContent).toContain('PR #5');
    expect(screen.getByTestId('work-task-delivery-pr').textContent).toContain('✕ 1 failing');
    await fireEvent.click(screen.getByTestId('work-task-tab-sessions'));
    const chips = screen.getAllByTestId('work-task-link-result');
    expect(chips).toHaveLength(1);
    expect(chips[0]).toHaveTextContent('Failed · cannot merge');
    expect(chips[0]).toHaveAttribute('data-verdict', 'blocked');
  });

  it('an inferred org says it is not a boundary', async () => {
    render(WorkTaskDetail, { taskId: 'item:77' });
    await flush();
    expect(screen.getByTestId('work-task-org').textContent).toContain('inferred from its sessions — not a boundary');
    expect(screen.getByTestId('work-task-group').textContent).toContain('most recent session (acme/api)');
  });

  it('the action bar opens the live session; ▾ continues the last conversation and starts new', async () => {
    handlers.resume_work = () => session('mefistos', 'resumed', { id: 11 });
    handlers.start_work = () => session('mefistos', 'fresh', { id: 12 });
    handlers.preview_start_work = () => ({
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
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fireEvent.click(within(bar()).getByTestId('work-button-primary'));
    expect(get(selectedSession)?.id).toBe(7);
    await fromMenu('work-button-continue');
    await flush();
    expect(calls('resume_work')[0]).toEqual({ key: 'ABC-12', mode: 'last', link_id: 41, host_alias: null, brief: null });
    expect(get(selectedSession)?.id).toBe(11);
    await fromMenu('work-button-start-new');
    await flush();
    expect(calls('preview_start_work')[0]).toEqual({ item_id: 12, with_brief: true });
    // Start new… always asks where: the popover, then Start.
    await fireEvent.click(screen.getByTestId('start-popover-go'));
    await flush();
    expect(calls('start_work')[0]).toEqual({ item_id: 12, with_brief: true, project_id: 3, host_alias: 'mefistos' });
    expect(get(selectedSession)?.id).toBe(12);
  });

  it('a start preview that answers after another task opened is dropped (review r07)', async () => {
    let release: (v: unknown) => void = () => {};
    handlers.preview_start_work = () => new Promise((r) => (release = r));
    handlers.start_work = () => session('mefistos', 'fresh', { id: 12 });
    const { rerender } = render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fireEvent.click(within(bar()).getByTestId('work-button-primary'));
    await rerender({ taskId: 'item:77' });
    await flush();
    release({
      key: 'ABC-12', title: 'Login', item_id: 12, missing: null,
      plan: { key: 'ABC-12', title: 'Login', item_id: 12, project_id: 3, host_alias: 'mefistos', branch: 'abc-12-login', name: 'ABC-12 Login' },
      projects: [{ id: 3, owner: 'acme', repo: 'api' }],
      hosts: [{ alias: 'mefistos', reachable: true }],
      conflicts: [{ kind: 'done', message: 'This task is done.' }],
      brief: null, checkout: { exists: false },
    });
    await flush();
    expect(screen.queryByTestId('start-popover')).toBeNull();
    expect(calls('start_work')).toHaveLength(0);
  });

  it('Start new opens the start popover when something must be chosen, and sends the choice', async () => {
    handlers.start_work = () => session('mefistos', 'fresh', { id: 12 });
    const preview = (over: Record<string, unknown> = {}) => ({
      key: 'ABC-12',
      title: 'Login',
      item_id: 12,
      plan: null,
      missing: 'project',
      projects: [
        { id: 3, owner: 'acme', repo: 'api' },
        { id: 4, owner: 'acme', repo: 'web' },
      ],
      hosts: [
        { alias: 'mefistos', reachable: true },
        { alias: 'oci', reachable: false },
      ],
      conflicts: [{ kind: 'done', message: 'This task is done.' }],
      brief: 'Steps: log in',
      checkout: null,
      ...over,
    });
    handlers.preview_start_work = (a) =>
      a.project_id === 4
        ? preview({
            plan: { key: 'ABC-12', title: 'Login', item_id: 12, project_id: 4, host_alias: 'mefistos', branch: 'abc-12-login', name: 'ABC-12 Login' },
            missing: null,
            checkout: { exists: false },
          })
        : preview();
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fromMenu('work-button-start-new');
    await flush();
    const pop = screen.getByTestId('start-popover');
    expect(pop.textContent).toContain('This task is done.');
    expect((screen.getByTestId('start-popover-go') as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId('start-popover-why').textContent).toBe('Pick a repository.');
    expect(calls('start_work')).toHaveLength(0);
    // The brief is shown as text, on request.
    await fireEvent.click(screen.getByTestId('start-popover-brief-toggle'));
    expect(screen.getByTestId('start-popover-brief-text').textContent).toBe('Steps: log in');
    // Picking a repository re-reads the preview with it.
    await fireEvent.change(screen.getByTestId('start-popover-project'), { target: { value: '4' } });
    await new Promise((r) => setTimeout(r, 300));
    await flush();
    expect(calls('preview_start_work').at(-1)).toEqual({ item_id: 12, with_brief: true, project_id: 4 });
    expect(screen.getByTestId('start-popover-checkout').textContent).toBe('new');
    await fireEvent.click(screen.getByTestId('start-popover-go'));
    await flush();
    expect(calls('start_work')[0]).toEqual({ item_id: 12, with_brief: true, project_id: 4, host_alias: 'mefistos' });
    expect(get(selectedSession)?.id).toBe(12);
    expect(screen.queryByTestId('start-popover')).toBeNull();
  });

  it('the start popover drafts the brief on the planned host and Start sends the draft (redesign 6.10)', async () => {
    fleetSettings.set({ ...SETTING_DEFAULTS, 'work.draft_briefs': 'true' });
    handlers.start_work = () => session('mefistos', 'fresh', { id: 12 });
    const plan = { key: 'ABC-12', title: 'Login', item_id: 12, project_id: 3, host_alias: 'mefistos', branch: 'abc-12-login', name: 'ABC-12 Login' };
    handlers.preview_start_work = (a) => ({
      key: 'ABC-12',
      title: 'Login',
      item_id: 12,
      plan,
      missing: null,
      projects: [{ id: 3, owner: 'acme', repo: 'api' }],
      hosts: [{ alias: 'mefistos', reachable: true }],
      // A conflict keeps the popover open instead of starting at once.
      conflicts: [{ kind: 'done', message: 'This task is done.' }],
      brief: a.draft_brief ? 'Goal: fix the login.' : 'Steps: log in',
      ...(a.draft_brief ? { brief_draft: { model: 'haiku', host_alias: 'mefistos', notes: 3 } } : {}),
      checkout: { exists: false },
    });
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fromMenu('work-button-start-new');
    await fireEvent.click(screen.getByTestId('start-popover-brief-draft-ask'));
    await flush();
    expect(calls('preview_start_work').at(-1)).toEqual({
      item_id: 12, with_brief: true, brief: undefined, draft_brief: true, project_id: 3, host_alias: 'mefistos',
    });
    const field = screen.getByTestId('start-popover-brief-draft-input') as HTMLTextAreaElement;
    expect(field.value).toBe('Goal: fix the login.');
    expect(screen.getByTestId('start-popover-brief-draft-meta').textContent).toContain(
      'by haiku on mefistos · from the ticket and 3 earlier notes',
    );
    expect(calls('start_work')).toHaveLength(0);
    await fireEvent.click(screen.getByTestId('start-popover-go'));
    await flush();
    expect(calls('start_work')[0]).toEqual({
      item_id: 12, with_brief: true, project_id: 3, host_alias: 'mefistos', brief: 'Goal: fix the login.',
    });
  });

  it("pre-selects Jev's proposed repository, resolves it, and still waits for Start", async () => {
    handlers.start_work = () => session('mefistos', 'fresh', { id: 12 });
    const base = {
      key: 'ABC-12',
      title: 'Login',
      item_id: 12,
      projects: [
        { id: 3, owner: 'acme', repo: 'api' },
        { id: 4, owner: 'acme', repo: 'web' },
      ],
      hosts: [{ alias: 'mefistos', reachable: true }],
      conflicts: [],
      brief: null,
      checkout: null,
    };
    handlers.preview_start_work = (a) =>
      a.project_id === 4
        ? {
            ...base,
            plan: { key: 'ABC-12', title: 'Login', item_id: 12, project_id: 4, host_alias: 'mefistos', branch: 'abc-12-login', name: 'ABC-12 Login' },
            missing: null,
            checkout: { exists: false },
          }
        : { ...base, plan: null, missing: 'project', suggested_project: { project_id: 4, confidence_pct: 88, run_id: 5 } };
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fromMenu('work-button-start-new');
    await flush();
    await flush();
    expect((screen.getByTestId('start-popover-project') as HTMLSelectElement).value).toBe('4');
    expect(screen.getByTestId('start-popover-suggested').textContent).toMatch(/Proposed by Jev\s+likely/);
    // The pre-selection is read back with its host and plan; nothing starts by itself.
    expect(calls('preview_start_work').at(-1)).toEqual({ item_id: 12, with_brief: true, project_id: 4 });
    expect(calls('start_work')).toHaveLength(0);
    await fireEvent.click(screen.getByTestId('start-popover-go'));
    await flush();
    expect(calls('start_work')[0]).toEqual({ item_id: 12, with_brief: true, project_id: 4, host_alias: 'mefistos' });
  });

  it('a hub without the preview starts as before', async () => {
    handlers.start_work = () => session('mefistos', 'fresh', { id: 12 });
    handlers.preview_start_work = () => {
      throw { code: 'E_INVALID', message: 'preview_start needs session_id' };
    };
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fromMenu('work-button-start-new');
    await flush();
    expect(calls('start_work')[0]).toEqual({ item_id: 12, with_brief: true });
    expect(get(selectedSession)?.id).toBe(12);
    expect(screen.queryByTestId('start-popover')).toBeNull();
  });

  it('a live session on the key makes Start a parallel start; Esc closes the popover', async () => {
    handlers.preview_start_work = () => ({
      key: 'ABC-12',
      title: 'Login',
      item_id: 12,
      plan: { key: 'ABC-12', title: 'Login', item_id: 12, project_id: 3, host_alias: 'mefistos', branch: 'abc-12-login-2', name: 'ABC-12 Login', parallel: true },
      projects: [{ id: 3, owner: 'acme', repo: 'api' }],
      hosts: [{ alias: 'mefistos', reachable: true }],
      conflicts: [{ kind: 'live_session', message: 'ABC-12 is already open in api on mefistos.', session_id: 7 }],
      brief: null,
      checkout: { exists: false },
    });
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fromMenu('work-button-start-new');
    await flush();
    expect(screen.getByTestId('start-popover-go').textContent).toContain('Start parallel');
    expect(screen.getByTestId('start-popover-open-live')).toBeTruthy();
    await fireEvent.keyDown(screen.getByTestId('start-popover'), { key: 'Escape' });
    await flush();
    expect(screen.queryByTestId('start-popover')).toBeNull();
    expect(calls('start_work')).toHaveLength(0);
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
    await flush();
    expect(screen.getByTestId('work-org-impact')).toBeTruthy();
    await fireEvent.change(screen.getByTestId('work-org-target'), { target: { value: '1' } });
    await flush();
    // The new target's impact is read at once; the old one is gone.
    expect(calls('work_org_impact').map((a) => a.org_id)).toEqual([2, 1]);
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
    render(WorkTaskDetail, { taskId: 'item:12', debounceMs: 5 });
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
    // The write's own bump re-reads the detail once (debounced), not twice.
    await new Promise((r) => setTimeout(r, 30));
    await flush();
    expect(calls('work_task')).toHaveLength(2);
  });

  it('a placement change re-reads the task, not the rules; a rule change both', async () => {
    handlers.work_task = () => trackerTask;
    handlers.work_rules = () => [];
    render(WorkTaskDetail, { taskId: 'item:12', debounceMs: 5 });
    await flush();
    const tasks = calls('work_task').length;
    const rules = calls('work_rules').length;
    noteWorkChanged([{ what: 'placement', task_id: 'item:12' }]);
    await new Promise((r) => setTimeout(r, 30));
    await flush();
    expect(calls('work_task')).toHaveLength(tasks + 1);
    expect(calls('work_rules')).toHaveLength(rules);
    noteWorkChanged([{ what: 'rule', rule_id: 3 }]);
    await new Promise((r) => setTimeout(r, 30));
    await flush();
    expect(calls('work_task')).toHaveLength(tasks + 2);
    expect(calls('work_rules')).toHaveLength(rules + 1);
    // A saved view's change is neither's.
    noteWorkChanged([{ what: 'view', view_id: 1 }]);
    await new Promise((r) => setTimeout(r, 30));
    await flush();
    expect(calls('work_task')).toHaveLength(tasks + 2);
    expect(calls('work_rules')).toHaveLength(rules + 1);
  });

  it('a tracker-controlled org is refused with the admin path', async () => {
    handlers.work_org_impact = () => impact('x', { allowed: false, reason: 'tracker_controlled', impact_token: null });
    render(WorkTaskDetail, { taskId: 'item:77' });
    await flush();
    await fireEvent.click(screen.getByTestId('work-task-assign-org'));
    await fireEvent.change(screen.getByTestId('work-org-target'), { target: { value: '2' } });
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
    await fireEvent.click(screen.getAllByTestId('work-place-label').find((b) => b.dataset.label === 'Payments')!);
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
    // The preview runs by itself once the draft is still (G2.2).
    await new Promise((r) => setTimeout(r, 450));
    await flush();
    expect(screen.getAllByTestId('rule-preview-row')[0].textContent).toContain('ABC → Payments');
    expect(screen.getByTestId('rule-preview-kept').textContent).toContain('2 placed by a person');
    // Changing the draft makes the preview stale again.
    await fireEvent.input(screen.getByTestId('rule-key-prefix'), { target: { value: 'ABC' } });
    await flush();
    expect(screen.getByTestId('rule-save').hasAttribute('disabled')).toBe(true);
    expect(screen.getByTestId('rule-preview-stale')).toBeTruthy();
    // The preview runs by itself once the draft is still (G2.2).
    await new Promise((r) => setTimeout(r, 450));
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
    await fromMenu('work-button-continue');
    await flush();
    expect(screen.getByTestId('work-button-error').textContent).toContain('is live in api');
    await fireEvent.click(screen.getByTestId('work-button-open-existing'));
    expect(get(selectedSession)?.id).toBe(7);
  });

  it('Continue: an E_EXISTS naming its session opens that one', async () => {
    handlers.resume_work = () => {
      throw { code: 'E_EXISTS', message: 'being resumed already', details: { session_id: 9 } };
    };
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    await fromMenu('work-button-continue');
    await flush();
    await fireEvent.click(screen.getByTestId('work-button-open-existing'));
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

  it('placement and rules sit behind a disclosure; the work sections are Overview’s, the steps Activity’s', async () => {
    handlers.work_task = () => ({
      ...trackerTask,
      subtasks: [{ task_id: 'item:41', item_id: 41, key: 'TASK-41', title: 'SELECT stats', origin: 'manual', status: 'todo', live_sessions: 0 }],
      steps: [{ label: 'ABC-12 login', claude_session_id: 'c1', steps: [{ text: 'Read ABC-12', state: 'completed', at: 1 }] }],
    });
    render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    const more = screen.getByTestId('work-task-more') as HTMLDetailsElement;
    expect(more.open).toBe(false);
    expect(within(more).getByTestId('work-task-place')).toBeTruthy();
    expect(within(more).getByTestId('work-task-org')).toBeTruthy();
    await fireEvent.click(screen.getByText('Placement & rules'));
    expect(screen.getByTestId('task-subtasks')).toBeTruthy();
    expect(screen.queryByTestId('task-steps')).toBeNull();
    await fireEvent.click(screen.getByTestId('work-task-tab-activity'));
    expect(screen.queryByTestId('task-subtasks')).toBeNull();
    expect(screen.getByTestId('task-step').textContent).toContain('Read ABC-12');
  });
  describe('G3.4: tabs, Delivery, inline status, the start rule', () => {
    it('Sessions counts its links; Activity says when nothing happened', async () => {
      handlers.work_task = () => ({ ...localTask, last_outcome: null });
      render(WorkTaskDetail, { taskId: 'item:77' });
      await flush();
      expect(screen.getByTestId('work-task-tab-overview').getAttribute('aria-selected')).toBe('true');
      expect(screen.getByTestId('work-task-tab-sessions').textContent).toBe('Sessions1');
      expect(screen.queryByTestId('work-task-tab-comments')).toBeNull();
      await fireEvent.click(screen.getByTestId('work-task-tab-activity'));
      expect(screen.getByTestId('work-task-no-activity')).toBeTruthy();
    });

    it('sets a native task’s status from the header, without the edit dialog', async () => {
      handlers.set_work_status = (a) => ({ id: a.item_id, status_category: a.status });
      render(WorkTaskDetail, { taskId: 'item:77' });
      await flush();
      const pick = screen.getByTestId('work-task-status-pick') as HTMLSelectElement;
      expect(pick.value).toBe('in_progress');
      pick.value = 'done';
      await fireEvent.change(pick);
      await flush();
      expect(calls('set_work_status')[0]).toEqual({ item_id: 77, status: 'done' });
    });

    it('a refused status says why on the page', async () => {
      handlers.set_work_status = () => {
        throw { code: 'E_FORBIDDEN', message: 'not yours' };
      };
      render(WorkTaskDetail, { taskId: 'item:77' });
      await flush();
      const pick = screen.getByTestId('work-task-status-pick') as HTMLSelectElement;
      pick.value = 'done';
      await fireEvent.change(pick);
      await flush();
      expect(screen.getByTestId('work-task-status-error').textContent).toContain('not yours');
    });

    it('a ticket has no inline status pick', async () => {
      render(WorkTaskDetail, { taskId: 'item:12' });
      await flush();
      expect(screen.queryByTestId('work-task-status-pick')).toBeNull();
    });

    it('Delivery names the column, the spend over its span and the owner; the rule names where a start lands', async () => {
      handlers.work_task = () => ({
        ...trackerTask,
        task: { ...trackerTask.task, key: 'PD-12', status_name: 'QA Review', cost_micros: 4_200_000, assignees: ['Ana'], mine: false },
        last_outcome: { ...trackerTask.last_outcome!, pr_url: 'https://github.com/o/r/pull/9' },
      });
      handlers.start_rules = () => [
        { id: 1, pattern: 'PD-*', project_id: 3, project: 'acme/pos', host_alias: null, state: 'active', created_at: 1, updated_at: 1 },
        { id: 2, pattern: 'PD-1*', project_id: 4, project: 'acme/web', host_alias: 'mac', state: 'active', created_at: 1, updated_at: 1 },
        { id: 3, pattern: 'PD-12', project_id: 5, project: 'acme/old', host_alias: null, state: 'dismissed', created_at: 1, updated_at: 1 },
      ];
      render(WorkTaskDetail, { taskId: 'item:12' });
      await flush();
      expect(screen.getByTestId('work-task-delivery-column').textContent).toBe('QA Review');
      expect(screen.getByTestId('work-task-delivery-spend').textContent?.trim()).toMatch(/^\$4\.20\sover /);
      expect(screen.getByTestId('work-task-delivery-owner').textContent).toBe('Ana');
      // No live PR: the last outcome's.
      expect(screen.getByTestId('work-task-delivery-pr').textContent).toContain('PR #9');
      // The most specific active rule wins; a dismissed one never.
      const rule = screen.getByTestId('work-task-start-rule').textContent?.replace(/\s+/g, ' ');
      expect(rule).toContain('Starts in acme/web on mac');
      expect(rule).toContain('rule PD-1*');
    });
  });

  describe('K5: Jev proposes a group (redesign 6.9)', () => {
    const proposed: TaskDetail = {
      ...localTask,
      task: { ...localTask.task, proposals: [{ feature: 'work_placement', value: 'Payments', source: 'jev', confidence_pct: 77 }] },
    };

    it('offers Place in the proposed group, and places on click', async () => {
      handlers.work_task = () => proposed;
      handlers.place_work = () => ({ ...proposed.task, group: { id: 'label:Payments', label: 'Payments', source: 'manual' }, placement_version: 1 });
      render(WorkTaskDetail, { props: { taskId: 'item:77' } });
      await flush();
      expect(screen.getByTestId('work-task-group-proposal').textContent).toContain('Jev proposes “Payments”');
      expect(screen.getByTestId('work-task-group-proposed-by').textContent).toContain('likely');
      await fireEvent.click(screen.getByTestId('work-task-group-proposal-place'));
      await flush();
      expect(calls('place_work')[0]).toEqual({ task_id: 'item:77', group: 'Payments', expected_version: 0 });
      expect(screen.queryByTestId('work-task-group-proposal')).toBeNull();
    });

    it('shows nothing once a person placed the task', async () => {
      handlers.work_task = () => ({ ...proposed, task: { ...proposed.task, group: { id: 'label:Infra', label: 'Infra', source: 'manual' } } });
      render(WorkTaskDetail, { props: { taskId: 'item:77' } });
      await flush();
      expect(screen.queryByTestId('work-task-group-proposal')).toBeNull();
    });
  });

  // Redesign 1.5: one primary per view. A task has nothing destructive to
  // put last: its edits (Place, Assign org, Make a rule) each preview first.
  it('has one primary: the Work button', async () => {
    render(WorkTaskDetail, { props: { taskId: 'ABC-12' } });
    await flush();
    const primary = expectOnePrimary(screen.getByTestId('work-task-detail'));
    expect(primary.closest('[data-testid="work-button"]')).not.toBeNull();
  });

  it('is accessible', async () => {
    handlers.work_task = () => ({
      ...trackerTask,
      subtasks: [{ task_id: 'item:41', item_id: 41, key: 'TASK-41', title: 'SELECT stats', origin: 'manual', status: 'todo', live_sessions: 0 }],
      steps: [{ label: 'ABC-12 login', claude_session_id: 'c1', steps: [{ text: 'Read ABC-12', state: 'completed', at: 1 }] }],
    });
    const { container } = render(WorkTaskDetail, { taskId: 'item:12' });
    await flush();
    (screen.getByTestId('work-task-more') as HTMLDetailsElement).open = true;
    await flush();
    await expectAccessible(container);
  });
});
