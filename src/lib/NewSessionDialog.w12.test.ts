// Review wave 12 (the New session flow): the keys follow the form rule, a
// nested sheet's keys stay its own, the busy state says what can be
// cancelled, a closed dialog never closes the next one, failures read as a
// sentence, a ticket start offers the cross-org tick and "Start another",
// a background run starts in the project and opens its row, and a preset
// kind is not remembered.
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

describe('w12-1: a nested sheet keeps its own keys', () => {
  it('an Enter another handler already took, or one inside a nested <dialog>, starts nothing', async () => {
    answer({ new_session: row() });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const name = screen.getByTestId('friendly-name');
    // The login list asks once on open; a submit would ask again.
    const checks = calls('check_account_headroom').length;
    const taken = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true, ...submitChord });
    taken.preventDefault();
    name.dispatchEvent(taken);
    // A sheet that renders inside the dialog (ResumeDialog) is its own <dialog>.
    const nested = document.createElement('dialog');
    const field = document.createElement('input');
    nested.appendChild(field);
    document.querySelector('.dialog .fields')!.appendChild(nested);
    await fireEvent.keyDown(field, { key: 'Enter' });
    await fireEvent.keyDown(field, { key: 'Enter', ...submitChord });
    await tick();
    await tick();
    expect(calls('new_session')).toHaveLength(0);
    expect(calls('check_account_headroom')).toHaveLength(checks);
  });
});

describe('w12-2: the form rule', () => {
  it('Enter on a checkbox does not start the work', async () => {
    answer({ start_work: row() });
    render(NewSessionDialog, { props: { project, ticket, initialName: 'ABC-7 Fix login', onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.keyDown(screen.getByTestId('ticket-brief-on'), { key: 'Enter' });
    await tick();
    expect(calls('start_work')).toHaveLength(0);
  });

  it('⌘↵ / Ctrl+Enter in the background prompt starts it', async () => {
    answer({ new_bg_session: { claude_session_id: null, session: null } });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('run-background'));
    const prompt = screen.getByTestId('bg-prompt');
    await fireEvent.input(prompt, { target: { value: 'fix the test' } });
    await fireEvent.keyDown(prompt, { key: 'Enter' });
    await tick();
    expect(calls('new_bg_session')).toHaveLength(0);
    await fireEvent.keyDown(prompt, { key: 'Enter', ...submitChord });
    await vi.waitFor(() => expect(calls('new_bg_session')).toHaveLength(1));
  });

  it('Enter refuses what the disabled button refuses, and says why', async () => {
    answer({ new_session: row(), check_account_headroom: { pause_at_pct: 90, chosen: null, over: false, suggestion: null, logins: [] } });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await vi.waitFor(() => expect(screen.getByTestId('launch-profile')).toBeTruthy());
    const profile = screen.getByTestId('launch-profile');
    await fireEvent.input(profile, { target: { value: 'my profile' } });
    expect((screen.getByTestId('create-btn') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.keyDown(profile, { key: 'Enter' });
    await tick();
    expect(calls('new_session')).toHaveLength(0);
    expect(screen.getByTestId('new-session-error').textContent).toContain('login profile');
  });
});

describe('w12-3 / w12-4: a ticket start in flight', () => {
  it('offers no dead "Cancel creation", and shows the start Pulse', async () => {
    let finish!: (v: unknown) => void;
    answer({ start_work: () => new Promise((r) => (finish = r)) });
    render(NewSessionDialog, { props: { project, ticket, initialName: 'ABC-7 Fix login', onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(calls('start_work')).toHaveLength(1));
    expect(screen.queryByTestId('cancel-create')).toBeNull();
    expect(screen.getByTestId('create-busy').textContent).toContain('Starting');
    expect(screen.getByTestId('new-session-pulse')).toBeTruthy();
    finish(row());
    await vi.waitFor(() => expect(screen.queryByTestId('create-busy')).toBeNull());
    expect(get(sessionsModule.creatingStart)).toBeNull();
  });
});

describe('w12-5: a create that lands after the dialog closed', () => {
  it('does not call onCreate (which would close the next dialog); a toast offers Open', async () => {
    let finish!: (v: unknown) => void;
    vi.spyOn(sessionsModule, 'newSessionAbortable').mockImplementation(
      () => new Promise((r) => (finish = r as (v: unknown) => void)) as never,
    );
    answer({ check_account_headroom: null });
    const onCreate = vi.fn();
    const onCancel = vi.fn();
    const { unmount } = render(NewSessionDialog, { props: { project, onCreate, onCancel } });
    await tick();
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(sessionsModule.newSessionAbortable).toHaveBeenCalled());
    unmount();
    finish({ ok: true, value: row({ id: 91, friendly_name: 'Red comet' }) });
    await tick();
    await tick();
    expect(onCreate).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
    const shown = get(toasts);
    expect(shown).toHaveLength(1);
    expect(shown[0].message).toContain('Red comet');
    shown[0].action!.run();
    expect(get(selectedSession)?.id).toBe(91);
    vi.restoreAllMocks();
  });
});

describe('w12-6: a failure reads as a sentence', () => {
  it('pins the sentence beside the buttons with the backend words under Details', async () => {
    vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: false,
      error: { code: 'E_SSH', message: 'E_SSH: ssh: connect to host mercury port 22: Connection timed out' },
    });
    answer({ check_account_headroom: null });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('create-btn'));
    const err = await screen.findByTestId('new-session-error');
    expect(err.getAttribute('role')).toBe('alert');
    // Not inside the scrolling fields.
    expect(err.closest('.fields')).toBeNull();
    expect(screen.getByTestId('new-session-error-text').textContent).not.toContain('E_SSH');
    expect(screen.getByTestId('new-session-error-details').textContent).toContain('E_SSH');
    vi.restoreAllMocks();
  });
});

describe('w12-7: a ticket start across organisations', () => {
  it('asks for the tick the preview names, then sends force_cross_org', async () => {
    answer({
      preview_start_work: { key: 'ABC-7', title: 'Fix login', plan: null, projects: [], hosts: [], conflicts: [{ kind: 'cross_org', message: 'ABC-7 is acme’s; this repository is globex’s' }] },
      start_work: row(),
    });
    render(NewSessionDialog, { props: { project, ticket, initialName: 'ABC-7 Fix login', onCreate: () => {}, onCancel: () => {} } });
    const tickBox = (await screen.findByTestId('ticket-cross-org-on')) as HTMLInputElement;
    expect(screen.getByTestId('ticket-cross-org-why').textContent).toContain('globex');
    expect((screen.getByTestId('create-btn') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.click(tickBox);
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(calls('start_work')).toHaveLength(1));
    expect(argsOf('start_work').force_cross_org).toBe(true);
  });

  it('maps a cross-org refusal to the tick, not the raw "pass force_cross_org" text', async () => {
    answer({
      start_work: () => {
        throw { code: 'E_FORBIDDEN', message: 'crosses organisations; pass force_cross_org: true to start anyway', details: { cross_org: true } };
      },
    });
    render(NewSessionDialog, { props: { project, ticket, initialName: 'ABC-7 Fix login', onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('create-btn'));
    const err = await screen.findByTestId('new-session-error');
    expect(err.textContent).toContain('Start across organisations');
    expect(err.textContent).not.toContain('force_cross_org: true');
    expect(screen.getByTestId('ticket-cross-org-on')).toBeTruthy();
  });
});

describe('w12-8: a ticket with a live session', () => {
  it('"Start another" sends parallel', async () => {
    sessionsModule.sessions.set([row({ id: 5, friendly_name: 'already', host_alias: 'local' }) as never]);
    answer({
      preview_start_work: { key: 'ABC-7', title: 'Fix login', plan: null, projects: [], hosts: [], conflicts: [{ kind: 'live_session', message: 'ABC-7 already runs as already', session_id: 5 }] },
      start_work: row({ id: 78 }),
    });
    const onCreate = vi.fn();
    render(NewSessionDialog, { props: { project, ticket, initialName: 'ABC-7 Fix login', onCreate, onCancel: () => {} } });
    const another = await screen.findByTestId('start-another');
    await fireEvent.click(another);
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    expect(argsOf('start_work').parallel).toBe(true);
  });

  it('a refused duplicate says it opened the live session', async () => {
    sessionsModule.sessions.set([row({ id: 5, friendly_name: 'already' }) as never]);
    answer({
      start_work: () => {
        throw { code: 'E_EXISTS', message: 'ABC-7 already has a live session', details: { session_id: 5 } };
      },
    });
    const onCancel = vi.fn();
    render(NewSessionDialog, { props: { project, ticket, onCreate: () => {}, onCancel } });
    await tick();
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCancel).toHaveBeenCalled());
    expect(get(toasts)[0].message).toContain('already runs as already');
  });
});

describe('w12-9 / w12-10: a background run', () => {
  it('starts in the project, hides the worktree rows, warns and opens its row', async () => {
    answer({ new_bg_session: { claude_session_id: 'c1', session: row({ id: 93, kind: 'bg' }), warning: 'the account is over pause_at' } });
    const onCreate = vi.fn();
    const onCancel = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate, onCancel } });
    await tick();
    await fireEvent.click(screen.getByTestId('run-background'));
    expect(screen.queryByTestId('wt-picker')).toBeNull();
    expect(screen.queryByTestId('new-session-name')).toBeNull();
    expect(screen.queryByTestId('path-preview')).toBeNull();
    expect(screen.getByTestId('bg-note').textContent).toContain("claude-fleet's checkout");
    await fireEvent.input(screen.getByTestId('bg-prompt'), { target: { value: 'fix the failing test' } });
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(onCreate).toHaveBeenCalled());
    expect(argsOf('new_bg_session').project_id).toBe(1);
    expect((onCreate.mock.calls[0][0] as { id: number }).id).toBe(93);
    expect(onCancel).not.toHaveBeenCalled();
    expect(get(toasts).some((t) => t.kind === 'warning' && t.message.includes('pause_at'))).toBe(true);
  });

  it('an empty new-worktree name does not block a background start', async () => {
    answer({ new_bg_session: { claude_session_id: null, session: null } });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('new-worktree-chip'));
    await fireEvent.input(screen.getByTestId('new-worktree-name'), { target: { value: '' } });
    await fireEvent.click(screen.getByTestId('run-background'));
    await fireEvent.input(screen.getByTestId('bg-prompt'), { target: { value: 'go' } });
    expect((screen.getByTestId('create-btn') as HTMLButtonElement).disabled).toBe(false);
  });
});

describe('w12-12: a preset kind is for this open only', () => {
  it('"Open a shell" does not make shell the project\'s remembered kind', async () => {
    vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: row({ kind: 'shell' }) as never });
    render(NewSessionDialog, { props: { project, initialKind: 'shell', onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('create-btn'));
    await vi.waitFor(() => expect(sessionsModule.newSessionAbortable).toHaveBeenCalled());
    await tick();
    const keys = Array.from({ length: localStorage.length }, (_, i) => localStorage.key(i) ?? '');
    const mem = JSON.parse(localStorage.getItem(keys.find((k) => k.includes('newsession.project.1'))!)!);
    expect(mem.kind).toBe('work');
    vi.restoreAllMocks();
  });
});
