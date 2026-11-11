// Plan first (docs/plan-first.md): the checkbox on New session. A plain start
// sends the instructions as the first prompt; a ticket start with its brief on
// carries them under the brief. Nothing is remembered about the session: the
// "Plan ready" card goes by what the agent says.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import NewSessionDialog from './NewSessionDialog.svelte';
import * as sessionsModule from './sessions';
import { PRIMARY, emptyRows, _resetFleetsForTest, _setFleetRowsForTest, _setFleetsForTest } from './fleets';
import { fleetInfo } from './fleets_fixture';
import { hosts, type HostRow } from './hosts';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { projects } from './projects';
import { clearToasts, toasts } from './toasts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const calls = (cmd: string) => invoke.mock.calls.filter((c) => c[0] === cmd);
const argsOf = (cmd: string, i = 0) => (calls(cmd)[i][1] as { args: Record<string, unknown> }).args;
const warnings = () => get(toasts).filter((t) => t.kind === 'warning').map((t) => t.message);

const project = {
  project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null, adopted: false, system: false },
  worktrees: [{ id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/r/cf', branch: 'main' }],
};
const ticket = {
  id: 42,
  tracker_id: 1,
  source: 'jira',
  key: 'ABC-7',
  title: 'Fix login',
  status_category: 'todo',
  status_name: 'To Do',
  url: 'https://acme.atlassian.net/browse/ABC-7',
  created_at: 1,
  updated_at: 1,
  description: 'd',
};
function row(over: Record<string, unknown> = {}) {
  return {
    id: 77, tmux_name: 'dev-x', host_alias: 'local', project_id: 1, worktree_id: null, status: 'running', kind: 'work',
    last_activity_at: 1, created_at: 1, friendly_name: null, claude_status: null, ...over,
  };
}

/** `answers[cmd]` answers that command (a function may throw); the rest are null. */
function answer(answers: Record<string, unknown>) {
  invoke.mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd in answers) {
      const a = answers[cmd];
      return typeof a === 'function' ? (a as (x: unknown) => unknown)(args) : a;
    }
    if (cmd === 'list_sessions') return [];
    return null;
  });
}


const LOCAL: HostRow = { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' };

beforeEach(() => {
  invoke.mockReset();
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  hosts.set([LOCAL]);
  _resetFleetsForTest();
  sessionsModule.sessions.set([]);
  projects.set([]);
  localStorage.clear();
  clearToasts();
});

describe('Plan first on New session', () => {
  it('a plain start sends the instructions as the first prompt', async () => {
    vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: row({ id: 91 }) } as never);
    answer({ check_account_headroom: null, queue_prompt: { delivered: true } });
    const onCreate = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('plan-first-on'));
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    await vi.waitFor(() => expect(calls('queue_prompt')).toHaveLength(1));
    expect(argsOf('queue_prompt').session_id).toBe(91);
    expect(String(argsOf('queue_prompt').prompt)).toContain('docs/plans/');
    expect(String(argsOf('queue_prompt').prompt)).toContain('Say "Plan ready"');
    vi.restoreAllMocks();
  });

  it('without the tick nothing is sent', async () => {
    vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: row({ id: 92 }) } as never);
    answer({ check_account_headroom: null });
    const onCreate = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    expect(calls('queue_prompt')).toHaveLength(0);
    vi.restoreAllMocks();
  });

  it('a ticket start carries the instructions under the brief, in one prompt', async () => {
    answer({ start_work: row({ id: 93 }) });
    const onCreate = vi.fn();
    render(NewSessionDialog, { props: { project, ticket, initialName: 'ABC-7 Fix login', onCreate, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('plan-first-on'));
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(calls('start_work')).toHaveLength(1));
    const a = argsOf('start_work');
    expect(a.with_brief).toBe(true);
    expect(String(a.brief)).toContain('ABC-7');
    expect(String(a.brief)).toContain('Plan first.');
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    expect(calls('queue_prompt')).toHaveLength(0);
  });

  it('a ticket start without the brief sends the instructions as their own prompt', async () => {
    answer({ start_work: row({ id: 94 }), queue_prompt: { delivered: true } });
    const onCreate = vi.fn();
    render(NewSessionDialog, { props: { project, ticket, initialName: 'ABC-7 Fix login', onCreate, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('ticket-brief-on'));
    await fireEvent.click(screen.getByTestId('plan-first-on'));
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    expect(argsOf('start_work').with_brief).toBe(false);
    expect(argsOf('start_work').brief).toBeUndefined();
    expect(calls('queue_prompt')).toHaveLength(1);
    expect(argsOf('queue_prompt').session_id).toBe(94);
    expect(String(argsOf('queue_prompt').prompt)).toContain('Plan first.');
  });

  /** The ticket also ran in a second repository, which the dialog offers and the test ticks. */
  async function inTwoRepositories(onCreate: () => void) {
    projects.set([project as never, { project: { ...project.project, id: 2, repo: 'web', base_path: '/r/web' }, worktrees: [] } as never]);
    render(NewSessionDialog, { props: { project, ticket, initialName: 'ABC-7 Fix login', onCreate, onCancel: () => {} } });
    await fireEvent.click(await screen.findByTestId('ticket-also-in-2'));
  }
  const ranInWeb = [{ id: 1, state: 'confirmed', source: 'manual', created_at: 1, ended_at: 5, snap_project_id: 2 }];

  it('a start in several repositories tells every session it started', async () => {
    answer({
      session_work_links: ranInWeb,
      start_work_multi: { key: 'ABC-7', started: [row({ id: 95 }), row({ id: 96, tmux_name: 'dev-web' })] },
      queue_prompt: { delivered: true },
    });
    const onCreate = vi.fn();
    await inTwoRepositories(onCreate);
    await fireEvent.click(screen.getByTestId('ticket-brief-on'));
    await fireEvent.click(screen.getByTestId('plan-first-on'));
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    expect(argsOf('start_work_multi').with_brief).toBe(false);
    expect(calls('queue_prompt').map((_, i) => argsOf('queue_prompt', i).session_id)).toEqual([95, 96]);
    expect(String(argsOf('queue_prompt', 1).prompt)).toContain('Plan first.');
  });

  it('a start in several repositories with the brief on carries the instructions in it', async () => {
    answer({ session_work_links: ranInWeb, start_work_multi: { key: 'ABC-7', started: [row({ id: 95 }), row({ id: 96 })] } });
    const onCreate = vi.fn();
    await inTwoRepositories(onCreate);
    await fireEvent.click(screen.getByTestId('plan-first-on'));
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    const a = argsOf('start_work_multi');
    expect(a.with_brief).toBe(true);
    expect(String(a.brief)).toContain('ABC-7');
    expect(String(a.brief)).toContain('Plan first.');
    expect(calls('queue_prompt')).toHaveLength(0);
  });

  it('a shell has no agent to tell: the tick is not offered, and one made before is not sent', async () => {
    vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: row({ id: 97, kind: 'shell' }) } as never);
    answer({ queue_prompt: { delivered: true } });
    const onCreate = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('plan-first-on'));
    await fireEvent.click(screen.getByTestId('kind-shell'));
    expect(screen.queryByTestId('plan-first-on')).toBeNull();
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    expect(calls('queue_prompt')).toHaveLength(0);
    vi.restoreAllMocks();
  });

  it('the tick shows only where the instructions can go: not for a background run, nor on another fleet', async () => {
    _setFleetsForTest([fleetInfo(PRIMARY), fleetInfo('work')]);
    _setFleetRowsForTest('work', {
      ...emptyRows(),
      hosts: new Map([['mac', { ...LOCAL, alias: 'mac', ssh_alias: 'mac' }]]),
      projects: [{ project: { ...project.project, id: 42 }, worktrees: [] }],
    });
    answer({});
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId(`new-session-fleet-${PRIMARY}`));
    expect(screen.getByTestId('plan-first-on')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('run-background'));
    expect(screen.queryByTestId('plan-first-on')).toBeNull();
    await fireEvent.click(screen.getByTestId('run-pane'));
    expect(screen.getByTestId('plan-first-on')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('new-session-fleet-work'));
    expect(screen.queryByTestId('plan-first-on')).toBeNull();
  });

  it('with the hub gone once the session is up, the instructions are not sent and a toast says so', async () => {
    vi.spyOn(sessionsModule, 'newSessionAbortable').mockImplementation(async () => {
      // The start went through; the connection dropped before the next call.
      hubStatus.set({ ...STANDALONE, remote: true, url: 'https://fleet.example.com' });
      hubConnection.set({ state: 'offline', attempt: 1, retry_in_secs: 5, reason: 'socket closed' });
      return { ok: true, value: row({ id: 98, friendly_name: 'Retry loop' }) } as never;
    });
    answer({ check_account_headroom: null, queue_prompt: { delivered: true } });
    const onCreate = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('plan-first-on'));
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    expect(warnings()).toEqual(["Plan first was not sent to Retry loop: The hub is unreachable; reconnecting. Try again once it's back."]);
    expect(calls('queue_prompt')).toHaveLength(0);
    vi.restoreAllMocks();
  });

  it('instructions the hub refuses are named in a toast, and the session still opens', async () => {
    vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: row({ id: 99 }) } as never);
    answer({
      check_account_headroom: null,
      queue_prompt: () => {
        throw { code: 'E_SESSION_GONE', message: 'The session is not running.' };
      },
    });
    const onCreate = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('plan-first-on'));
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    await vi.waitFor(() => expect(warnings()).toEqual(['Plan first was not sent to dev-x: The session is not running.']));
    vi.restoreAllMocks();
  });
});
