import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

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
import { clearToasts, toasts } from './toasts';
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
  // The PTY spawns `ssh <host>` then `tmux attach` FROM THIS MACHINE, using
  // this machine's own ssh config — the hub is never in that path, and
  // `pty_open` reads nothing out of the local store. So a paired desktop
  // attaches exactly as a standalone one does. The one host it cannot attach
  // is an agent host: nothing anywhere has an SSH route to one.
  it('attaches the PTY for an ssh-transport host, exactly as standalone', async () => {
    hubStatus.set(remote);
    hosts.set([makeHost({ alias: 'trn', transport: 'ssh' })]);
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(true);
    expect(screen.queryByTestId('terminal-no-attach')).toBeNull();
  });

  it('attaches for a host whose row has not loaded, rather than refusing on a guess', async () => {
    hubStatus.set(remote);
    hosts.set([]);
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(true);
  });

  it('never attaches an agent host, and explains instead', async () => {
    hubStatus.set(remote);
    hosts.set([makeHost({ alias: 'trn', transport: 'agent' })]);
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(false);
    const hint = screen.getByTestId('terminal-no-attach');
    expect(hint.textContent).toContain('fleet-agent');
    expect(hint.textContent).toContain('dev-martin-janci-claude-fleet');
    // Not even the ssh line, which would be a lie for this host.
    expect(screen.queryByTestId('terminal-attach-line')).toBeNull();
  });

  it('offers Transfer from the attached pane', async () => {
    const movable = makeSession({ kind: 'work', worktree_id: 10, claude_session_id: 'c-1' });
    sessions.set([movable]);
    hubStatus.set(remote);
    hosts.set([makeHost({ alias: 'trn', transport: 'ssh' })]);
    render(TerminalView);
    selectSession(movable);
    await settle();
    expect(screen.getByTestId('transfer-chip')).toBeTruthy();
  });

  it('offers Transfer from the agent-host note too: the move is hub-routed', async () => {
    const movable = makeSession({ kind: 'work', worktree_id: 10, claude_session_id: 'c-1' });
    sessions.set([movable]);
    hubStatus.set(remote);
    hosts.set([makeHost({ alias: 'trn', transport: 'agent' })]);
    render(TerminalView);
    selectSession(movable);
    await settle();
    expect(screen.getByTestId('transfer-chip')).toBeTruthy();
  });

  it('with no session selected it is the ordinary empty state, not a hub note', async () => {
    hubStatus.set(remote);
    render(TerminalView);
    await settle();
    expect(screen.getByTestId('terminal-empty')).toBeInTheDocument();
    expect(screen.queryByTestId('terminal-no-attach')).toBeNull();
  });

  it('standalone is untouched: the PTY still attaches', async () => {
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(true);
    expect(screen.queryByTestId('terminal-no-attach')).toBeNull();
  });

  // The automatic pre-attach check is `repair_session { explicit: false }`,
  // which a paired desktop REFUSES by design (`verdicts.rs`: routing it would
  // quietly become the hub's always-explicit repair). Calling it anyway meant
  // an E_LOCAL_ONLY toast on every attach of a project-backed session. There
  // is no safe variant to route, so the check simply does not exist here —
  // the same way an offline host is left to the attach error. Repair
  // workspace still works: it passes `explicit: true` and routes.
  it('skips the automatic workspace check instead of refusing it', async () => {
    hubStatus.set(remote);
    hosts.set([makeHost({ alias: 'trn', transport: 'ssh' })]);
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'repair_session')).toBe(false);
    expect(get(toasts)).toEqual([]);
    // The attach itself is unaffected.
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(true);
  });

  it('standalone still runs the automatic workspace check', async () => {
    hosts.set([makeHost({ alias: 'trn', transport: 'ssh' })]);
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'repair_session')).toBe(true);
  });
});
