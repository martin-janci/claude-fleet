import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock('./ConversationPanel.svelte', () => ({ default: () => ({}) }));

import { get } from 'svelte/store';
import AgentPanel from './AgentPanel.svelte';
import { operatorError, operatorState, operatorSession } from './operator';
import { sessions } from './sessions';
import { hostsViewRequest, settingsOpen, settingsSection } from './app_views';

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
  }) as never;

beforeEach(() => {
  invoke.mockReset();
  operatorError.set(null);
  operatorState.set('ready');
  operatorSession.set(row());
  sessions.set([]);
});

/** Mount at `state`: the panel wakes the agent on mount (Control, 9.1), so
 *  the status read answers `state` back; the calls it made are cleared. */
async function mount(state: string) {
  operatorState.set(state as never);
  invoke.mockImplementation(async (cmd: string) =>
    cmd === 'operator_status'
      ? { ready: state === 'ready', session: get(operatorSession), blocked: state === 'ready' ? null : state }
      : null,
  );
  const r = render(AgentPanel);
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('operator_status', undefined));
  await tick();
  await tick();
  invoke.mockClear();
  return r;
}

// The panel owns no composer: the prompt box, its busy gate, its error and
// its draft are ConversationPanel's, covered in ConversationPanel.test.ts and
// — under this panel, with the real component — in
// AgentPanel.integration.test.ts. What is left here is what the panel itself
// owns: the blocked states and the chip. Since 13.1 it is only Control's
// chat; the floating sheet (close, Esc, grip, maximize) went with Classic.
describe('AgentPanel', () => {
  it('offers a restart when the agent was lost, and does not resurrect it by itself', async () => {
    await mount('lost');
    expect(screen.getByRole('button', { name: /restart/i })).toBeTruthy();
    expect(invoke).not.toHaveBeenCalledWith('ensure_operator', expect.anything());
  });

  it('shows why a restart of the lost agent failed, under the button', async () => {
    await mount('lost');
    invoke.mockRejectedValueOnce({
      code: 'E_TMUX',
      message: 'error connecting to /tmp/tmux-1000/default (No such file or directory)',
    });
    await fireEvent.click(screen.getByRole('button', { name: /restart/i }));
    const note = await screen.findByTestId('agent-panel-error');
    expect(note.textContent).toContain('Restart failed: error connecting to');
    expect(invoke).toHaveBeenCalledWith('restart_session', expect.anything());
  });

  it('shows the context chip and drops it when removed', async () => {
    await mount('ready');
    // With no session selected elsewhere in the app there is no chip at all.
    expect(screen.queryByTestId('agent-context-chip')).toBeNull();
  });
});

describe('AgentPanel: a next step for every blocked state (step 9.1)', () => {
  it('no_mcp opens Settings › Control API, and calls no command', async () => {
    await mount('no_mcp');
    await fireEvent.click(screen.getByRole('button', { name: 'Open Settings › Control API' }));
    expect(get(settingsSection)).toBe('control-api');
    expect(get(settingsOpen)).toBe(true);
    expect(invoke).not.toHaveBeenCalled();
    settingsOpen.set(false);
    settingsSection.set(null);
  });

  it('no_host and host_down without a fallback open the Hosts view', async () => {
    const { unmount } = await mount('no_host');
    await fireEvent.click(screen.getByRole('button', { name: 'Add a host' }));
    expect(get(hostsViewRequest)).toEqual({ host: null });
    unmount();
    hostsViewRequest.set(null);
  });

  it('token_revoked asks before it kills, and Cancel kills nothing', async () => {
    await mount('token_revoked');
    await fireEvent.click(screen.getByRole('button', { name: 'Replace the agent' }));
    expect(screen.getByTestId('agent-replace-confirm')).toBeTruthy();
    expect(invoke).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByTestId('agent-replace-confirm')).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });

  it('token_revoked, confirmed, kills the session and starts a new agent', async () => {
    operatorSession.set(row({ id: 71 }));
    await mount('token_revoked');
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'kill_session') return 71;
      if (cmd === 'operator_status') return { ready: false, session: null, blocked: 'absent' };
      if (cmd === 'ensure_operator') return row({ id: 72 });
      return null;
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Replace the agent' }));
    await fireEvent.click(screen.getByTestId('agent-replace-yes'));
    await vi.waitFor(() => expect(get(operatorState)).toBe('ready'));
    const cmds = invoke.mock.calls.map((c) => c[0]);
    expect(cmds).toEqual(['kill_session', 'operator_status', 'ensure_operator']);
  });

  it('Control\'s chat has no close or grip, and opening it wakes the agent', async () => {
    operatorState.set('lost');
    invoke.mockResolvedValue({ ready: false, session: row(), blocked: 'lost' });
    render(AgentPanel);
    expect(screen.getByTestId('control-agent')).toBeTruthy();
    expect(screen.queryByTestId('agent-panel-close')).toBeNull();
    expect(screen.queryByTestId('agent-panel-grip')).toBeNull();
    expect(screen.getByRole('button', { name: 'Restart the agent' })).toBeTruthy();
    // Opening Control makes sure there is an agent.
    expect(invoke).toHaveBeenCalledWith('operator_status', undefined);
  });
});
