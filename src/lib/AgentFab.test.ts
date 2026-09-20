import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import AgentFab from './AgentFab.svelte';
import { agentPanelOpen, operatorState } from './operator';

beforeEach(() => {
  invoke.mockReset();
  agentPanelOpen.set(false);
  operatorState.set('unknown');
});

describe('AgentFab', () => {
  it('is a labelled button that opens the agent', async () => {
    invoke.mockResolvedValue({ ready: true, session: null, blocked: null });
    render(AgentFab);
    const btn = screen.getByRole('button', { name: /agent/i });
    await fireEvent.click(btn);
    expect(invoke).toHaveBeenCalledWith('operator_status', undefined);
  });

  it('says why it is unavailable when the control API is off', async () => {
    operatorState.set('no_mcp');
    render(AgentFab);
    const btn = screen.getByRole('button', { name: /agent/i });
    expect(btn).toHaveAttribute('title', expect.stringContaining('control API'));
  });
});
