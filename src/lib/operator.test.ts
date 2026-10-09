import { describe, it, expect, vi, beforeEach } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { get } from 'svelte/store';
import {
  operatorState,
  operatorSession,
  operatorRow,
  restartOperator,
  replaceOperator,
  ensureAgent,
  refreshOperator,
  operatorError,
  operatorHost,
  operatorFallback,
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
  operatorError.set(null);
  operatorState.set('unknown');
  operatorSession.set(null);
  sessions.set([]);
});

describe('ensureAgent: waking the agent', () => {
  it('wakes the agent and ends ready', async () => {
    invoke.mockResolvedValueOnce({ ready: false, session: null, blocked: 'absent' });
    invoke.mockResolvedValueOnce({ id: 7, tmux_name: 'fleet-operator', host_alias: 'local' });
    await ensureAgent();
    expect(get(operatorState)).toBe('ready');
    expect(get(operatorSession)?.id).toBe(7);
  });

  it('a blocked agent does not get woken, and the reason survives', async () => {
    invoke.mockResolvedValueOnce({ ready: false, session: null, blocked: 'no_mcp' });
    await ensureAgent();
    expect(get(operatorState)).toBe('no_mcp');
    // Only the status call — ensure_operator must not run when the control
    // API is off: it would create a session that cannot reach any tool.
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it('an agent stranded on a host that went down is started on the fallback', async () => {
    invoke.mockResolvedValueOnce({
      ready: false,
      session: { id: 3, tmux_name: 'fleet-operator', host_alias: 'mefistos' },
      blocked: 'host_down',
      host: 'mefistos',
      fallback: 'oci',
    });
    invoke.mockResolvedValueOnce({ id: 9, tmux_name: 'fleet-operator', host_alias: 'oci' });
    await ensureAgent();
    expect(invoke).toHaveBeenNthCalledWith(2, 'ensure_operator', undefined);
    expect(get(operatorState)).toBe('ready');
    expect(get(operatorSession)?.host_alias).toBe('oci');
    expect(get(operatorFallback)).toBeNull();
  });

  it('a stranded agent with nowhere to go stays host_down, and nothing is started', async () => {
    invoke.mockResolvedValueOnce({
      ready: false,
      session: { id: 3, tmux_name: 'fleet-operator', host_alias: 'mefistos' },
      blocked: 'host_down',
      host: 'mefistos',
      fallback: null,
    });
    await ensureAgent();
    expect(get(operatorState)).toBe('host_down');
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it('a failed move keeps host_down, so its button can try again', async () => {
    invoke.mockResolvedValueOnce({
      ready: false,
      session: null,
      blocked: 'host_down',
      host: 'mefistos',
      fallback: 'oci',
    });
    invoke.mockRejectedValueOnce({ code: 'E_SSH', message: 'oci went away too' });
    await ensureAgent();
    expect(get(operatorState)).toBe('host_down');
  });

  it('a ready agent is not re-created', async () => {
    invoke.mockResolvedValueOnce({
      ready: true,
      session: { id: 3, tmux_name: 'fleet-operator', host_alias: 'local' },
      blocked: null,
    });
    await ensureAgent();
    expect(get(operatorState)).toBe('ready');
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});

describe('blockedCopy', () => {
  // Redesign step 9.1 (the AgentStates board): every blocked state has a
  // next step. None of them is a LocalOnly command: `no_mcp` opens Settings
  // rather than calling `mcp_configure`, and `token_revoked` replaces the
  // agent (kill, after a confirm, then a fresh start) rather than calling
  // `ensure_operator` on a live session, which would no-op.
  it('every blocked state offers a next step', () => {
    expect(blockedCopy('absent')).toMatchObject({ action: 'Wake the agent', next: 'wake' });
    expect(blockedCopy('lost')).toMatchObject({ action: 'Restart the agent', next: 'restart' });
    expect(blockedCopy('no_mcp')).toMatchObject({ action: 'Open Settings › Control API', next: 'control_api' });
    expect(blockedCopy('token_revoked')).toMatchObject({ action: 'Replace the agent', next: 'replace' });
    expect(blockedCopy('no_host')).toMatchObject({ action: 'Add a host', next: 'add_host' });
    expect(blockedCopy('host_down', 'mercury', 'mac').next).toBe('move');
    expect(blockedCopy('host_down', 'mercury').next).toBe('open_host');
    for (const b of ['no_mcp', 'lost', 'token_revoked', 'absent', 'no_host', 'host_down'] as const) {
      expect(blockedCopy(b).title.length).toBeGreaterThan(0);
      expect(blockedCopy(b).action.length).toBeGreaterThan(0);
    }
  });

  it('says where the agent would have to run, not merely that it is not running', () => {
    const t = blockedCopy('no_host').title;
    expect(t).toContain('local');
    expect(t).not.toContain('not running');
  });

  it('no_host on local names the fix; no_host elsewhere names the host', () => {
    // A hub without a local host: the way out is homing the operator on a
    // fleet host, and the copy says which flag does that.
    const onLocal = blockedCopy('no_host', 'local').title;
    expect(onLocal).toContain('--operator-host');
    // A configured home the fleet does not have: the copy names it, so the
    // person can tell a typo from a host they never added.
    const onGhost = blockedCopy('no_host', 'mefistos').title;
    expect(onGhost).toContain('mefistos');
    expect(onGhost).toContain('not in this fleet');
    expect(onGhost).not.toContain('hub.local_host');
    expect(blockedCopy('no_host', 'mefistos').action).toBe('Add a host');
  });
});

describe('host_down', () => {
  it('offers to start the agent on the fallback, and names both hosts', () => {
    const c = blockedCopy('host_down', 'mefistos', 'oci');
    expect(c.action).toContain('oci');
    expect(c.title).toContain('mefistos');
    expect(c.title).toContain('oci');
  });

  it('with no fallback, says what a host needs and opens that host', () => {
    const c = blockedCopy('host_down', 'mefistos');
    expect(c.action).toBe('Open mefistos');
    expect(c.title).toContain('mefistos');
    expect(c.title).toContain('no other host');
  });
});

describe('operatorHost', () => {
  it('is read from the status and defaults to local', async () => {
    invoke.mockResolvedValueOnce({
      ready: false,
      session: null,
      blocked: 'no_host',
      host: 'mefistos',
    });
    await ensureAgent();
    expect(get(operatorState)).toBe('no_host');
    expect(get(operatorHost)).toBe('mefistos');
    // An older hub answers without the field.
    invoke.mockResolvedValueOnce({ ready: false, session: null, blocked: 'no_host' });
    await refreshOperator();
    expect(get(operatorHost)).toBe('local');
  });
});

describe('restartOperator', () => {
  it('calls nothing when there is no operator session in the store, and says so', async () => {
    operatorSession.set(null);
    await restartOperator();
    expect(invoke).not.toHaveBeenCalled();
    expect(get(operatorError)).toMatch(/not known yet/);
  });

  // 2026-10-06: after mefistos rebooted the `lost` button answered
  // E_SELF_TARGET, and the panel looked exactly as it did before the click.
  it('a failed restart says why under the button and leaves the state lost', async () => {
    operatorState.set('lost');
    operatorSession.set(row({ host_alias: 'mefistos' }));
    invoke.mockRejectedValueOnce({
      code: 'E_SELF_TARGET',
      message: 'fleet-operator on mefistos is the registered fleet controller',
    });
    await restartOperator();
    const cmds = invoke.mock.calls.map((c) => c[0]);
    expect(cmds[0]).toBe('restart_session');
    // No status refresh after a failure: the state stays what the panel shows.
    expect(cmds).not.toContain('operator_status');
    expect(get(operatorState)).toBe('lost');
    expect(get(operatorError)).toBe(
      'Restart failed: fleet-operator on mefistos is the registered fleet controller',
    );
  });

  it('a successful restart clears the previous failure', async () => {
    operatorError.set('Restart failed: earlier');
    operatorSession.set(row());
    invoke.mockResolvedValueOnce(row()); // restart_session
    invoke.mockResolvedValueOnce({ ready: true, session: row(), blocked: null }); // operator_status
    await restartOperator();
    expect(get(operatorError)).toBeNull();
    expect(get(operatorState)).toBe('ready');
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

describe('replaceOperator (token_revoked, step 9.1)', () => {
  it('kills the agent\'s session, then starts a new one', async () => {
    operatorState.set('token_revoked');
    // An id of its own: a kill leaves a tombstone that later tests' row 7
    // events would hit.
    operatorSession.set(row({ id: 70 }));
    invoke.mockResolvedValueOnce(70); // kill_session
    invoke.mockResolvedValueOnce({ ready: false, session: null, blocked: 'absent' }); // operator_status
    invoke.mockResolvedValueOnce(row({ id: 8 })); // ensure_operator
    await replaceOperator();
    expect(invoke.mock.calls.map((c) => c[0])).toEqual(['kill_session', 'operator_status', 'ensure_operator']);
    expect(invoke.mock.calls[0][1]).toEqual({ args: { host_alias: 'local', name: 'fleet-operator' } });
    expect(get(operatorState)).toBe('ready');
    expect(get(operatorSession)?.id).toBe(8);
  });

  it('a failed kill says why and starts nothing', async () => {
    operatorState.set('token_revoked');
    operatorSession.set(row());
    invoke.mockRejectedValueOnce({ code: 'E_TMUX', message: 'no server running' });
    await replaceOperator();
    const cmds = invoke.mock.calls.map((c) => c[0]).filter((c) => c !== 'report_client_error');
    expect(cmds).toEqual(['kill_session']);
    expect(get(operatorError)).toBe('Kill failed: no server running');
    expect(get(operatorState)).toBe('token_revoked');
  });
});

describe('ensureAgent (Control, step 9.1)', () => {
  it('wakes the agent', async () => {
    invoke.mockResolvedValueOnce({ ready: false, session: null, blocked: 'absent' });
    invoke.mockResolvedValueOnce(row());
    await ensureAgent();
    expect(get(operatorState)).toBe('ready');
  });
});

describe('ensureAgent re-entrancy', () => {
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

    const first = ensureAgent();
    const second = ensureAgent();
    expect(second).toBe(first);
    releaseStatus({ ready: false, session: null, blocked: 'absent' });
    await Promise.all([first, second]);

    const ensures = invoke.mock.calls.filter((c) => c[0] === 'ensure_operator');
    expect(ensures).toHaveLength(1);
    expect(get(operatorState)).toBe('ready');
  });

  it('a later press is a fresh call, not the stale promise', async () => {
    invoke.mockResolvedValue({ ready: true, session: row(), blocked: null });
    await ensureAgent();
    await ensureAgent();
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
