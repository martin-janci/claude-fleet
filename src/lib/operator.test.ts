import { describe, it, expect, vi, beforeEach } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { get } from 'svelte/store';
import {
  agentPanelOpen,
  operatorState,
  operatorSession,
  openAgent,
  restartOperator,
  blockedCopy,
} from './operator';

beforeEach(() => {
  invoke.mockReset();
  agentPanelOpen.set(false);
  operatorState.set('unknown');
  operatorSession.set(null);
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
  it('absent and lost offer a button; no_mcp and token_revoked do not', () => {
    expect(blockedCopy('absent').action).toBe('Wake the agent');
    expect(blockedCopy('lost').action).toBe('Restart the agent');
    expect(blockedCopy('no_mcp').action).toBeNull();
    expect(blockedCopy('token_revoked').action).toBeNull();
    for (const b of ['no_mcp', 'lost', 'token_revoked', 'absent'] as const) {
      expect(blockedCopy(b).title.length).toBeGreaterThan(0);
    }
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
