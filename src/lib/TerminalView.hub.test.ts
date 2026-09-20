import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  readText: vi.fn(),
  writeText: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import TerminalView from './TerminalView.svelte';
import { sessions, resetTombstonesForTests, type SessionRow } from './sessions';
import { hosts, type HostRow } from './hosts';
import { selectSession, clearSelection } from './selection';
import { clearToasts } from './toasts';
import { hubStatus, STANDALONE, type HubStatus } from './hub';

function makeSession(over: Partial<SessionRow>): SessionRow {
  return {
    id: 1,
    tmux_name: 'dev-martin-janci-claude-fleet',
    host_alias: 'trn',
    project_id: 1,
    worktree_id: null,
    created_at: 1,
    last_activity_at: 1,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: 'main',
    lost_at: null,
    claude_session_id: null,
    claude_status: null,
    effort_level: null,
    pr_url: null,
    current_activity: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null,
    stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null,
    last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null,
    parent_session_id: null, tags: [],
    model: null, context_tokens: null, context_window: null, context_source: null,
    context_at: null, context_stale: false, tmux_pane_id: null,
    ...over,
  };
}

const remote: HubStatus = {
  remote: true,
  url: 'https://fleet.example.com',
  client_name: 'laptop',
  client_mode: null,
  configured_url: 'https://fleet.example.com',
  configured_client_name: 'laptop',
  allow_plaintext: false,
  warning: null,
  restart_required: false,
  unavailable: null,
};

function makeHost(over: Partial<HostRow>): HostRow {
  return {
    alias: 'trn',
    ssh_alias: 'trn',
    reachable: true,
    claude_version: null,
    tmux_version: null,
    hidden: false,
    last_pinged_at: null,
    account_uuid: null,
    provisioned: true,
    transport: 'ssh',
    ...over,
  };
}

const session = makeSession({});
const inv = () => mockedInvoke as ReturnType<typeof vi.fn>;
const settle = async (n = 8) => {
  for (let i = 0; i < n; i++) await tick();
};

class FakeResizeObserver {
  constructor(_cb: () => void) {}
  observe() {}
  unobserve() {}
  disconnect() {}
}

beforeEach(() => {
  inv().mockReset();
  inv().mockImplementation(async (cmd: string) => {
    if (cmd === 'pty_drain') return { data: '', bytes: 0 };
    return null;
  });
  // @ts-expect-error: test stub
  globalThis.ResizeObserver = FakeResizeObserver;
  resetTombstonesForTests();
  sessions.set([session]);
  hosts.set([]);
  clearSelection();
  clearToasts();
  hubStatus.set({ ...STANDALONE });
});

afterEach(() => {
  clearSelection();
  hubStatus.set({ ...STANDALONE });
});

describe('the terminal tab against a hub', () => {
  // The spec's named non-goal: the PTY attaches to a local ssh/tmux process,
  // and the hub streams no pane. What must NOT happen is the attach being
  // attempted anyway — `pty_open` is guarded on the backend, so the user
  // would get an error toast where a terminal should be.
  it('never attempts to attach a PTY', async () => {
    hubStatus.set(remote);
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(false);
    // …and nothing else of the terminal's machinery starts either.
    expect(inv().mock.calls.some((c) => c[0] === 'pty_drain')).toBe(false);
    expect(inv().mock.calls.some((c) => c[0] === 'repair_session')).toBe(false);
  });

  it('offers the shell command for the selected session instead of a dead pane', async () => {
    hubStatus.set(remote);
    render(TerminalView);
    selectSession(session);
    await settle();
    const hint = screen.getByTestId('terminal-remote');
    // The two halves of actually getting there, for THIS session on THIS host.
    expect(hint.textContent).toContain('ssh trn');
    expect(hint.textContent).toContain('tmux attach');
    expect(hint.textContent).toContain('dev-martin-janci-claude-fleet');
    expect(hint.textContent).toContain('fleet.example.com');
  });

  it('still offers Transfer: the move is hub-routed even though the pane is not', async () => {
    // selectedSession resolves through the `sessions` store by identity, not
    // the object passed to selectSession — the store must carry the movable
    // fields, or the lookup resolves to the plain `session` seeded in
    // beforeEach (worktree_id/claude_session_id: null) and canMoveSession is
    // false.
    const movable = makeSession({ kind: 'work', worktree_id: 10, claude_session_id: 'c-1' });
    sessions.set([movable]);
    hubStatus.set(remote);
    render(TerminalView);
    selectSession(movable);
    await settle();
    expect(screen.getByTestId('transfer-chip')).toBeTruthy();
  });

  it('an agent-transport host gets no ssh command, just the explanation', async () => {
    hubStatus.set(remote);
    hosts.set([makeHost({ alias: 'trn', transport: 'agent' })]);
    render(TerminalView);
    selectSession(session);
    await settle();
    const hint = screen.getByTestId('terminal-remote');
    // No attach command for this session — not "ssh trn" (the command it
    // would otherwise print) anywhere in the hint.
    expect(hint.textContent).not.toContain('ssh trn');
    expect(screen.queryByTestId('terminal-attach-line')).toBeNull();
    expect(hint.textContent).toContain('fleet-agent');
    expect(hint.textContent).toContain('dev-martin-janci-claude-fleet');
  });

  it('an ssh-transport host (and an unknown host) still get the ssh command', async () => {
    hubStatus.set(remote);
    hosts.set([makeHost({ alias: 'trn', transport: 'ssh' })]);
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(screen.getByTestId('terminal-attach-line').textContent).toContain('ssh trn');
  });

  it('says so even with no session selected, rather than "select a session"', async () => {
    hubStatus.set(remote);
    render(TerminalView);
    await settle();
    expect(screen.getByTestId('terminal-remote')).toBeInTheDocument();
    expect(screen.queryByTestId('terminal-empty')).toBeNull();
  });

  it('standalone is untouched: the PTY still attaches', async () => {
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(true);
    expect(screen.queryByTestId('terminal-remote')).toBeNull();
  });
});
