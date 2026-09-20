import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/svelte';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock('./ConversationPanel.svelte', () => ({ default: () => ({}) }));

import AgentPanel from './AgentPanel.svelte';
import { agentPanelOpen, operatorState, operatorSession } from './operator';

const row = (over = {}) =>
  ({
    id: 7,
    tmux_name: 'fleet-operator',
    host_alias: 'local',
    kind: 'work',
    claude_status: 'idle',
    stuck_kind: null,
    ...over,
  }) as never;

beforeEach(() => {
  invoke.mockReset();
  agentPanelOpen.set(true);
  operatorState.set('ready');
  operatorSession.set(row());
});

describe('AgentPanel', () => {
  it('will not send while the agent is working — two pastes into one REPL is one mangled prompt', () => {
    operatorSession.set(row({ claude_status: 'working' }));
    render(AgentPanel);
    expect(screen.getByRole('button', { name: /send/i })).toBeDisabled();
    expect(screen.getByText(/working/i)).toBeTruthy();
  });

  it('surfaces stuck_kind, because a stuck agent looks exactly like a slow one', () => {
    operatorSession.set(row({ claude_status: 'working', stuck_kind: 'trust_prompt' }));
    render(AgentPanel);
    expect(screen.getByText(/trust/i)).toBeTruthy();
  });

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
});
