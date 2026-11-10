// Plan first (docs/plan-first.md): the checkbox on New session. A plain start
// sends the instructions as the first prompt; a ticket start with its brief on
// carries them under the brief; either way the session is remembered for the
// "Plan ready" card.
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
import { hosts } from './hosts';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { toasts, clearToasts } from './toasts';
import { selectedSession } from './selection';
import { detectMac } from './terminal_keys';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const calls = (cmd: string) => invoke.mock.calls.filter((c) => c[0] === cmd);
const argsOf = (cmd: string, i = 0) => (calls(cmd)[i][1] as { args: Record<string, unknown> }).args;

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

const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
const submitChord = isMac ? { metaKey: true } : { ctrlKey: true };

beforeEach(() => {
  invoke.mockReset();
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
  ]);
  sessionsModule.sessions.set([]);
  localStorage.clear();
  clearToasts();
});

import { isPlanFirst } from './plan_first';

describe('Plan first on New session', () => {
  it('a plain start sends the instructions as the first prompt and remembers the session', async () => {
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
    expect(isPlanFirst(91)).toBe(true);
    vi.restoreAllMocks();
  });

  it('without the tick nothing is sent or remembered', async () => {
    vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: row({ id: 92 }) } as never);
    answer({ check_account_headroom: null });
    const onCreate = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    expect(calls('queue_prompt')).toHaveLength(0);
    expect(isPlanFirst(92)).toBe(false);
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
    expect(isPlanFirst(93)).toBe(true);
  });
});
