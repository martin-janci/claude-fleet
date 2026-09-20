import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import AgentFab from './AgentFab.svelte';
import { get } from 'svelte/store';
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

  it('TOGGLES: a second press closes the panel it opened', async () => {
    invoke.mockResolvedValue({ ready: true, session: null, blocked: null });
    render(AgentFab);
    const btn = screen.getByRole('button', { name: /agent/i });
    await fireEvent.click(btn);
    expect(get(agentPanelOpen)).toBe(true);
    expect(btn).toHaveAttribute('aria-expanded', 'true');

    // The sheet is fixed over the bottom-right corner of every view; a
    // button that only ever opens leaves no way back.
    const before = invoke.mock.calls.length;
    await fireEvent.click(btn);
    expect(get(agentPanelOpen)).toBe(false);
    expect(btn).toHaveAttribute('aria-expanded', 'false');
    expect(invoke.mock.calls.length).toBe(before);
  });

  it('says it will close while the panel is open', async () => {
    agentPanelOpen.set(true);
    render(AgentFab);
    expect(screen.getByRole('button', { name: /agent/i })).toHaveAttribute(
      'title',
      expect.stringContaining('Close'),
    );
  });

  it('says why it is unavailable when the control API is off', async () => {
    operatorState.set('no_mcp');
    render(AgentFab);
    const btn = screen.getByRole('button', { name: /agent/i });
    expect(btn).toHaveAttribute('title', expect.stringContaining('control API'));
  });

  it('registers the agent-fab hint anchor when the button is actionable', async () => {
    const { anchorEl } = await import('./hints');
    operatorState.set('ready');
    render(AgentFab);
    expect(anchorEl('agent-fab')).toBeDefined();
  });

  it('does not anchor the hint while the button only explains why the agent is unavailable', async () => {
    const { anchorEl } = await import('./hints');
    operatorState.set('no_mcp');
    render(AgentFab);
    expect(anchorEl('agent-fab')).toBeUndefined();
  });
});
