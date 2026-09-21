import { describe, it, expect, vi, beforeEach } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { get } from 'svelte/store';
import {
  agentPanelOpen,
  operatorState,
  operatorSession,
  operatorRow,
  openAgent,
  closeAgent,
  toggleAgent,
  restartOperator,
  blockedCopy,
} from './operator';
import { sessions, applySessionEvents, type SessionRow } from './sessions';

const row = (over = {}) =>
  ({
    id: 7,
    tmux_name: 'fleet-operator',
    host_alias: 'local',
    kind: 'work',
    claude_status: 'idle',
    stuck_kind: null,
    last_activity_at: 100,
    ...over,
  }) as unknown as SessionRow;

beforeEach(() => {
  invoke.mockReset();
  agentPanelOpen.set(false);
  operatorState.set('unknown');
  operatorSession.set(null);
  sessions.set([]);
});

describe('openAgent', () => {
  it('opens the panel, wakes the agent, and ends ready', async () => {
    invoke.mockResolvedValueOnce({ ready: false, session: null, blocked: 'absent' });
    invoke.mockResolvedValueOnce({ id: 7, tmux_name: 'fleet-operator', host_alias: 'local' });
    await openAgent();
    expect(get(agentPanelOpen)).toBe(true);
    expect(get(operatorState)).toBe('ready');
    expect(get(operatorSession)?.id).toBe(7);
  });

  it('a blocked agent does not get woken, and the reason survives', async () => {
    invoke.mockResolvedValueOnce({ ready: false, session: null, blocked: 'no_mcp' });
    await openAgent();
    expect(get(agentPanelOpen)).toBe(true);
    expect(get(operatorState)).toBe('no_mcp');
    // Only the status call — ensure_operator must not run when the control
    // API is off: it would create a session that cannot reach any tool.
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it('a ready agent is not re-created', async () => {
    invoke.mockResolvedValueOnce({
      ready: true,
      session: { id: 3, tmux_name: 'fleet-operator', host_alias: 'local' },
      blocked: null,
    });
    await openAgent();
    expect(get(operatorState)).toBe('ready');
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});

describe('blockedCopy', () => {
  // RULING (overrides the original brief): only `absent` and `lost` are
  // recoverable from this panel without either calling a LocalOnly command
  // (`no_mcp` → `mcp_configure`) or doing nothing (`token_revoked` → the
  // session is alive, so `ensure_operator` would no-op). Those two states
  // get a title-only explanation and no button.
  it('absent and lost offer a button; no_mcp, token_revoked and no_host do not', () => {
    expect(blockedCopy('absent').action).toBe('Wake the agent');
    expect(blockedCopy('lost').action).toBe('Restart the agent');
    expect(blockedCopy('no_mcp').action).toBeNull();
    expect(blockedCopy('token_revoked').action).toBeNull();
    // The operator runs on the `local` host. A hub with `hub.local_host=false`
    // has none, so there is nowhere to start it and no press that could help
    // — reporting `absent` there offered a button that silently did nothing.
    expect(blockedCopy('no_host').action).toBeNull();
    for (const b of ['no_mcp', 'lost', 'token_revoked', 'absent', 'no_host'] as const) {
      expect(blockedCopy(b).title.length).toBeGreaterThan(0);
    }
  });

  it('says where the agent would have to run, not merely that it is not running', () => {
    const t = blockedCopy('no_host').title;
    expect(t).toContain('local');
    expect(t).not.toContain('not running');
  });
});

describe('restartOperator', () => {
  it('does nothing when there is no operator session in the store', async () => {
    operatorSession.set(null);
    await restartOperator();
    expect(invoke).not.toHaveBeenCalled();
  });

  it('restarts the operator session and refreshes status', async () => {
    operatorSession.set({
      id: 5,
      tmux_name: 'fleet-operator',
      host_alias: 'local',
    } as never);
    invoke.mockResolvedValueOnce({ id: 5, tmux_name: 'fleet-operator', host_alias: 'local' }); // restart_session
    invoke.mockResolvedValueOnce({
      ready: true,
      session: { id: 5, tmux_name: 'fleet-operator', host_alias: 'local' },
      blocked: null,
    }); // operator_status
    await restartOperator();
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke.mock.calls[0][0]).toBe('restart_session');
    expect(invoke.mock.calls[1][0]).toBe('operator_status');
    expect(get(operatorState)).toBe('ready');
  });
});

describe('closing the panel', () => {
  // The panel is `position: fixed` over the bottom-right corner of every
  // view. Before this, `agentPanelOpen` was written `true` in one place and
  // `false` nowhere in production code — the first press pinned the sheet
  // for the life of the process.
  it('toggleAgent closes an open panel without touching the backend', async () => {
    agentPanelOpen.set(true);
    await toggleAgent();
    expect(get(agentPanelOpen)).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
  });

  it('toggleAgent opens a closed panel', async () => {
    invoke.mockResolvedValueOnce({ ready: true, session: row(), blocked: null });
    await toggleAgent();
    expect(get(agentPanelOpen)).toBe(true);
    expect(invoke).toHaveBeenCalledWith('operator_status', undefined);
  });

  it('closeAgent leaves the agent itself alone — the panel is a window, not the session', () => {
    agentPanelOpen.set(true);
    operatorState.set('ready');
    closeAgent();
    expect(get(agentPanelOpen)).toBe(false);
    expect(get(operatorState)).toBe('ready');
    expect(invoke).not.toHaveBeenCalled();
  });
});

describe('openAgent re-entrancy', () => {
  // Two overlapping presses each minted a token and revoked the other's, and
  // because the DB write and the `.mcp.json` write are separately ordered the
  // host could end up on a token the database had revoked — every call 401s
  // while `operator_status` still says `ready`.
  it('two overlapping presses issue ONE ensure_operator', async () => {
    let releaseStatus: (v: unknown) => void = () => {};
    invoke.mockImplementationOnce(
      () => new Promise((res) => { releaseStatus = res; }),
    );
    invoke.mockResolvedValueOnce({ ready: false, session: null, blocked: 'absent' });
    invoke.mockResolvedValueOnce(row());

    const first = openAgent();
    const second = openAgent();
    expect(second).toBe(first);
    releaseStatus({ ready: false, session: null, blocked: 'absent' });
    await Promise.all([first, second]);

    const ensures = invoke.mock.calls.filter((c) => c[0] === 'ensure_operator');
    expect(ensures).toHaveLength(1);
    expect(get(operatorState)).toBe('ready');
  });

  it('a joiner opens the panel too — closing mid-birth must not wedge the button', async () => {
    // Opening is what EVERY caller wants, whether it starts the birth or
    // joins one already running. With `agentPanelOpen.set(true)` inside the
    // work function only, a press after closing mid-birth returned the
    // in-flight promise (already past that line) and the sheet stayed shut
    // until the birth resolved — the button doing nothing, visibly.
    let releaseEnsure: (v: unknown) => void = () => {};
    invoke.mockResolvedValueOnce({ ready: false, session: null, blocked: 'absent' });
    invoke.mockImplementationOnce(
      () => new Promise((res) => { releaseEnsure = res; }),
    );

    const birth = openAgent();
    await vi.waitFor(() => expect(get(operatorState)).toBe('waking'));

    closeAgent();
    expect(get(agentPanelOpen)).toBe(false);

    // Press again while the birth is STILL in flight.
    const joined = openAgent();
    expect(joined).toBe(birth);
    expect(get(agentPanelOpen)).toBe(true);

    releaseEnsure(row());
    await Promise.all([birth, joined]);
    expect(get(agentPanelOpen)).toBe(true);
    expect(invoke.mock.calls.filter((c) => c[0] === 'ensure_operator')).toHaveLength(1);
  });

  it('a later press is a fresh call, not the stale promise', async () => {
    invoke.mockResolvedValue({ ready: true, session: row(), blocked: null });
    await openAgent();
    agentPanelOpen.set(false);
    await openAgent();
    expect(invoke.mock.calls.filter((c) => c[0] === 'operator_status')).toHaveLength(2);
  });
});

describe('operatorRow', () => {
  // The spec's "refuse to send while working" gate and the stuck_kind line
  // both read this. As a snapshot taken when the panel opened, neither could
  // ever fire or clear.
  it('follows the row-event bus, not the snapshot the panel opened with', () => {
    operatorSession.set(row({ claude_status: 'idle' }));
    sessions.set([row({ claude_status: 'idle' })]);
    expect(get(operatorRow)?.claude_status).toBe('idle');

    applySessionEvents([
      { type: 'updated', row: row({ claude_status: 'working', last_activity_at: 200 }) },
    ]);
    expect(get(operatorRow)?.claude_status).toBe('working');
  });

  it('falls back to the anchor in the window before the row event arrives', () => {
    operatorSession.set(row({ claude_status: 'idle' }));
    sessions.set([]);
    expect(get(operatorRow)?.id).toBe(7);
  });

  it('matches on (host, tmux) — another host\'s same-named session is not the operator', () => {
    operatorSession.set(row({ claude_status: 'idle' }));
    sessions.set([row({ id: 99, host_alias: 'mefistos', claude_status: 'working' })]);
    expect(get(operatorRow)?.claude_status).toBe('idle');
  });

  it('is null with no operator at all', () => {
    operatorSession.set(null);
    sessions.set([row()]);
    expect(get(operatorRow)).toBeNull();
  });
});
