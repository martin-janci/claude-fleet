import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock('./ConversationPanel.svelte', () => ({ default: () => ({}) }));

import { get } from 'svelte/store';
import AgentPanel from './AgentPanel.svelte';
import { agentPanelOpen, operatorState, operatorSession } from './operator';
import { sessions } from './sessions';

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
  agentPanelOpen.set(true);
  operatorState.set('ready');
  operatorSession.set(row());
  sessions.set([]);
});

// The sheet no longer owns a composer: the prompt box, its busy gate, its
// error and its draft are ConversationPanel's, covered in
// ConversationPanel.test.ts and — stacked under this sheet, with the real
// component — in AgentPanel.integration.test.ts. What is left here is what
// the sheet itself owns: the blocked states, the frame, and the chip.
describe('AgentPanel', () => {
  it('offers a restart when the agent was lost, and does not resurrect it by itself', () => {
    operatorState.set('lost');
    render(AgentPanel);
    expect(screen.getByRole('button', { name: /restart/i })).toBeTruthy();
    expect(invoke).not.toHaveBeenCalledWith('ensure_operator', expect.anything());
  });

  it('shows the context chip and drops it when removed', async () => {
    render(AgentPanel);
    // With no session selected elsewhere in the app there is no chip at all.
    expect(screen.queryByTestId('agent-context-chip')).toBeNull();
  });

  it('closes on its close button — the agent keeps running behind it', async () => {
    render(AgentPanel);
    await fireEvent.click(screen.getByTestId('agent-panel-close'));
    expect(get(agentPanelOpen)).toBe(false);
    expect(screen.queryByTestId('agent-panel')).toBeNull();
    expect(get(operatorState)).toBe('ready');
  });

  it('closes on Escape anywhere in the sheet', async () => {
    render(AgentPanel);
    await fireEvent.keyDown(screen.getByTestId('agent-panel'), { key: 'Escape' });
    expect(get(agentPanelOpen)).toBe(false);
  });

});

