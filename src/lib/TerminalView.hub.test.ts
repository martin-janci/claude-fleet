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
import { applyGrantChanges, resetAccessForTests, setMyGrants } from './access';

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
    context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
    // Multi-user M1: every row here belongs to THIS person, and `beforeEach`
    // seeds the client's identity to match. Without that pairing a paired
    // desktop derives no access at all and attaches nothing — which is the
    // correct behaviour and would make every `pty_open` assertion below fail
    // for a reason that has nothing to do with what it is testing. Seeding the
    // identity once per suite is both the smaller diff and the honest shape:
    // the client's identity is ONE value, not a per-row one.
    owner_person_id: ME,
    visibility: 'private',
    ...over,
  };
}

/** This client's person id on the hub. */
const ME = 7;
/** Somebody else's. */
const OTHER = 9;

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
  setMyGrants(ME, []);
});

afterEach(() => {
  clearSelection();
  hubStatus.set({ ...STANDALONE });
  resetAccessForTests();
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

  // `transport: 'agent'` says the HUB cannot dial the host. It says nothing
  // about THIS machine, which may well have a route — an ssh config entry
  // with a ProxyCommand through a box that can reach it. The pane used to
  // refuse outright and assert that no route existed anywhere, which is
  // false for exactly that setup. So it attaches like any other host and
  // explains only once the attempt has actually failed.
  it('attaches an agent host rather than refusing on its transport alone', async () => {
    hubStatus.set(remote);
    hosts.set([makeHost({ alias: 'trn', transport: 'agent' })]);
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(true);
    expect(screen.queryByTestId('terminal-no-attach')).toBeNull();
  });

  it('explains the agent transport when the attach actually fails', async () => {
    hubStatus.set(remote);
    hosts.set([makeHost({ alias: 'trn', transport: 'agent' })]);
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') return { data: '', bytes: 0 };
      if (cmd === 'pty_open') throw { code: 'E_SSH', message: 'ssh: connect timed out' };
      return null;
    });
    render(TerminalView);
    selectSession(session);
    await settle();
    const why = screen.getByTestId('terminal-agent-transport');
    expect(why.textContent).toContain('fleet-agent');
    // It must not claim nothing anywhere can reach the host — the whole bug.
    expect(why.textContent).not.toContain('Nothing can dial');
  });

  it('keeps a non-agent host on its ordinary error, not the agent sentence', async () => {
    hosts.set([makeHost({ alias: 'trn', transport: 'ssh' })]);
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'pty_drain') return { data: '', bytes: 0 };
      if (cmd === 'pty_open') throw { code: 'E_SSH', message: 'ssh: connect timed out' };
      return null;
    });
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(screen.queryByTestId('terminal-agent-transport')).toBeNull();
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

// Multi-user M1: "sharing never confers a terminal" (spec §4.3 invariant 4).
//
// The attach is this machine's own `ssh … tmux attach`: the hub is not in the
// path, cannot refuse it and — the part that decides the design — cannot
// revoke it once it is up. So a session reached through a GRANT must not
// attach at all, and the gate has to be on the client. These tests pin the
// three halves of it that live in this component (App.svelte owns the fourth:
// it does not mount the component for such a row at all).
describe('sharing never confers a terminal', () => {
  const theirs = makeSession({ id: 5, owner_person_id: OTHER });

  beforeEach(() => {
    hubStatus.set(remote);
    hosts.set([makeHost({ alias: 'trn', transport: 'ssh' })]);
    sessions.set([theirs]);
  });

  it('opens no PTY and runs no workspace probe for a watch-granted session', async () => {
    setMyGrants(ME, [{ session_id: theirs.id, level: 'watch' }]);
    render(TerminalView);
    selectSession(theirs);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(false);
    // `repair_session` respawns tmux and re-adds worktrees. It must not run
    // for a session you may only watch — hence the early return in `openTerm`
    // sits BEFORE the probe, not after it.
    expect(inv().mock.calls.some((c) => c[0] === 'repair_session')).toBe(false);
    expect(screen.getByTestId('terminal-no-attach')).toBeInTheDocument();
    expect(screen.queryByTestId('terminal-host')).toBeNull();
  });

  it('opens no PTY for a DRIVE-granted session either', async () => {
    // `drive` is "make this machine do work" (through fleet, which the hub can
    // stop at any moment), not "take an SSH session into the owner's pane".
    setMyGrants(ME, [{ session_id: theirs.id, level: 'drive' }]);
    render(TerminalView);
    selectSession(theirs);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(false);
    expect(screen.getByTestId('terminal-no-attach')).toBeInTheDocument();
  });

  it('opens no PTY for a stranger’s row with no grant at all', async () => {
    render(TerminalView);
    selectSession(theirs);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(false);
  });

  // The fail-closed arm, and the one thing it must NOT say. "The hub has not
  // told us who we are" and "this session is not yours" are different
  // problems, and a person acts differently on them.
  it('blames the hub, not the session, when it does not know who this device is', async () => {
    resetAccessForTests();
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(false);
    const why = screen.getByTestId('terminal-no-attach');
    expect(why.textContent).toContain('who this device is');
    expect(why.textContent).not.toContain('Shared with you');
  });

  // Clause (c) of the gate: a LIVE pty is closed the moment the derived answer
  // stops being `own`. Without it a share (or a re-identification) would leave
  // a read/write channel into a pane this client may no longer touch, which
  // nothing on the hub can reach and which would clear only on the next
  // 30 s-throttled focus re-list.
  it('closes a live PTY when the row stops being this person’s', async () => {
    const mine = makeSession({ id: 6, owner_person_id: ME });
    sessions.set([mine]);
    setMyGrants(ME, []);
    render(TerminalView);
    selectSession(mine);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(true);
    const closesBefore = inv().mock.calls.filter((c) => c[0] === 'pty_close').length;

    // The row is replaced WHOLESALE, as `createRowStore` does on every merge.
    sessions.set([{ ...mine, owner_person_id: OTHER }]);
    setMyGrants(ME, [{ session_id: mine.id, level: 'watch' }]);
    await settle();

    expect(inv().mock.calls.filter((c) => c[0] === 'pty_close').length).toBeGreaterThan(
      closesBefore,
    );
    expect(screen.getByTestId('terminal-no-attach')).toBeInTheDocument();
    // And it must not immediately re-attach: the two open-effects re-run when
    // closeTerm nulls the attach identity, and `openTerm`'s early return is
    // what stops them turning the close into a reconnect loop.
    const opensAfter = inv().mock.calls.filter((c) => c[0] === 'pty_open').length;
    await settle();
    expect(inv().mock.calls.filter((c) => c[0] === 'pty_open').length).toBe(opensAfter);
  });

  // THE assertion a per-caller field on the row could not have satisfied. The
  // gate's effect has THREE inputs — the row, this client's person id and its
  // grant set — and the two that are not the row move with NO row event behind
  // them at all: a grant mutates no `sessions` column, and a re-identification
  // (a resync whose `my_grants` names a different person, a re-paired device)
  // touches no row either. An effect that read the row alone would never fire.
  it('closes a live PTY on an identity change alone, with no session event', async () => {
    const mine = makeSession({ id: 6, owner_person_id: ME });
    sessions.set([mine]);
    setMyGrants(ME, []);
    render(TerminalView);
    selectSession(mine);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(true);
    const closesBefore = inv().mock.calls.filter((c) => c[0] === 'pty_close').length;
    const rowBefore = get(sessions)[0];

    // Only the client's own identity moves. The store is not touched.
    setMyGrants(OTHER, []);
    await settle();

    expect(get(sessions)[0]).toBe(rowBefore);
    expect(inv().mock.calls.filter((c) => c[0] === 'pty_close').length).toBeGreaterThan(
      closesBefore,
    );
    expect(screen.getByTestId('terminal-no-attach')).toBeInTheDocument();
  });

  // The ordinary revoke. Under this gate a granted session never attached in
  // the first place — which is the stronger guarantee — so what a revoke has
  // to do is take the pane view away without a re-list, and that is the grant
  // map (patched by the `grant:changed` frame) and nothing else.
  it('a revoked grant drops back to nothing on the frame alone, with no re-list', async () => {
    setMyGrants(ME, [{ session_id: theirs.id, level: 'watch' }]);
    render(TerminalView);
    selectSession(theirs);
    await settle();
    const listsBefore = inv().mock.calls.filter((c) => c[0] === 'list_sessions').length;

    applyGrantChanges([{ session_id: theirs.id, person_id: ME, level: null }]);
    await settle();

    const why = screen.getByTestId('terminal-no-attach');
    // No longer "shared with you to watch": the grant is gone.
    expect(why.textContent).not.toContain('Shared with you');
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(false);
    expect(inv().mock.calls.filter((c) => c[0] === 'list_sessions').length).toBe(listsBefore);
  });

  it('still attaches every OWNED session on a paired desktop', async () => {
    setMyGrants(ME, []);
    sessions.set([session]);
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(true);
    expect(screen.queryByTestId('terminal-no-attach')).toBeNull();
  });

  // A standalone desktop IS the fleet: its `list_sessions` serves the personal
  // owner's rows and the unclaimed ones and nothing else, so every row it
  // holds is its own — and it keeps its terminal even though `my_grants` has
  // never answered. This is what makes the gate safe to ship before the
  // backend half exists.
  it('a standalone desktop attaches even with no identity and a foreign owner on the row', async () => {
    hubStatus.set({ ...STANDALONE });
    resetAccessForTests();
    sessions.set([theirs]);
    render(TerminalView);
    selectSession(theirs);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(true);
    expect(screen.queryByTestId('terminal-no-attach')).toBeNull();
  });

  // A configured hub this launch could not use: the backend owns nothing and
  // refuses every command. Fail closed — but, again, say which problem it is.
  it('a hub this launch cannot use attaches nothing and does not call it a sharing refusal', async () => {
    hubStatus.set({ ...remote, remote: false, unavailable: 'no stored token' });
    sessions.set([session]);
    render(TerminalView);
    selectSession(session);
    await settle();
    expect(inv().mock.calls.some((c) => c[0] === 'pty_open')).toBe(false);
    const why = screen.getByTestId('terminal-no-attach');
    expect(why.textContent).toContain('no stored token');
    expect(why.textContent).not.toContain('Shared with you');
  });
});
