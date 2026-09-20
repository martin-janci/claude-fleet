import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock('./ConversationPanel.svelte', () => ({ default: () => ({}) }));

import { get } from 'svelte/store';
import AgentPanel from './AgentPanel.svelte';
import { agentPanelOpen, operatorState, operatorSession } from './operator';
import { sessions, applySessionEvents } from './sessions';

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

  it('keeps the draft and shows an error when the send fails, instead of losing what was typed', async () => {
    invoke.mockImplementation((cmd: unknown) => {
      if (cmd === 'send_prompt') return Promise.reject(new Error('ssh: connection refused'));
      return Promise.resolve({ ready: true, session: null, blocked: null });
    });
    render(AgentPanel);
    const box = screen.getByPlaceholderText(/ask the agent/i) as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'do the thing' } });
    await fireEvent.click(screen.getByRole('button', { name: /^send$/i }));
    await waitFor(() => expect(screen.getByTestId('agent-composer-error')).toBeTruthy());
    expect(box.value).toBe('do the thing');
  });

  it('clears a stale error the moment the next send starts', async () => {
    let fail = true;
    invoke.mockImplementation((cmd: unknown) => {
      if (cmd === 'send_prompt') return fail ? Promise.reject(new Error('boom')) : Promise.resolve(undefined);
      return Promise.resolve({ ready: true, session: null, blocked: null });
    });
    render(AgentPanel);
    const box = screen.getByPlaceholderText(/ask the agent/i) as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'first try' } });
    await fireEvent.click(screen.getByRole('button', { name: /^send$/i }));
    await waitFor(() => expect(screen.getByTestId('agent-composer-error')).toBeTruthy());

    fail = false;
    await fireEvent.click(screen.getByRole('button', { name: /^send$/i }));
    await waitFor(() => expect(screen.queryByTestId('agent-composer-error')).toBeNull());
  });

  it('a dropped chip comes back when the context changes, and its prefix is sent again', async () => {
    const { rerender } = render(AgentPanel, {
      contextInput: { view: 'hosts', session: null, hostAlias: 'alpha', branch: null },
    });
    expect(screen.getByTestId('agent-context-chip').textContent).toContain('alpha');

    await fireEvent.click(screen.getByTestId('agent-context-chip'));
    expect(screen.queryByTestId('agent-context-chip')).toBeNull();

    // Same context re-rendered — the drop persists, this is not a one-shot toggle.
    await rerender({ contextInput: { view: 'hosts', session: null, hostAlias: 'alpha', branch: null } });
    expect(screen.queryByTestId('agent-context-chip')).toBeNull();

    // A different context — the chip returns on its own, no re-click needed.
    await rerender({ contextInput: { view: 'hosts', session: null, hostAlias: 'beta', branch: null } });
    expect(screen.getByTestId('agent-context-chip').textContent).toContain('beta');

    invoke.mockImplementation((cmd: unknown) =>
      cmd === 'send_prompt'
        ? Promise.resolve(undefined)
        : Promise.resolve({ ready: true, session: null, blocked: null }),
    );
    const box = screen.getByPlaceholderText(/ask the agent/i) as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'hello' } });
    await fireEvent.click(screen.getByRole('button', { name: /^send$/i }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        'send_prompt',
        expect.objectContaining({ args: expect.objectContaining({ prompt: expect.stringContaining('beta') }) }),
      ),
    );
  });

  it('closes on its close button — the agent keeps running behind it', async () => {
    render(AgentPanel);
    await fireEvent.click(screen.getByTestId('agent-panel-close'));
    expect(get(agentPanelOpen)).toBe(false);
    expect(screen.queryByTestId('agent-panel')).toBeNull();
    expect(get(operatorState)).toBe('ready');
  });

  it('closes on Escape from inside the composer, where a window-level handler would not', async () => {
    render(AgentPanel);
    const box = screen.getByPlaceholderText(/ask the agent/i);
    await fireEvent.keyDown(box, { key: 'Escape' });
    expect(get(agentPanelOpen)).toBe(false);
  });

  it('closes on Escape anywhere in the sheet', async () => {
    render(AgentPanel);
    await fireEvent.keyDown(screen.getByTestId('agent-panel'), { key: 'Escape' });
    expect(get(agentPanelOpen)).toBe(false);
  });

  it('the busy gate MOVES on a row event, not only when the panel was opened', async () => {
    // The spec's mitigation for "two clients at once". Read off the snapshot
    // `operator_status` returned, it could neither fire after you sent nor
    // ever clear; this drives it through the same bus the sidebar repaints
    // from.
    operatorSession.set(row({ claude_status: 'idle' }));
    sessions.set([row({ claude_status: 'idle' })]);
    render(AgentPanel);
    expect(screen.getByRole('button', { name: /^send$/i })).not.toBeDisabled();

    applySessionEvents([
      { type: 'updated', row: row({ claude_status: 'working', last_activity_at: 200 }) },
    ]);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^send$/i })).toBeDisabled(),
    );
    expect(screen.getByText(/working/i)).toBeTruthy();

    // And it clears again when the agent goes idle.
    applySessionEvents([
      { type: 'updated', row: row({ claude_status: 'idle', last_activity_at: 300 }) },
    ]);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^send$/i })).not.toBeDisabled(),
    );
  });

  it('surfaces a stuck_kind that arrives after the panel opened', async () => {
    operatorSession.set(row({ claude_status: 'working' }));
    sessions.set([row({ claude_status: 'working' })]);
    render(AgentPanel);
    applySessionEvents([
      {
        type: 'updated',
        row: row({ claude_status: 'working', stuck_kind: 'trust_prompt', last_activity_at: 200 }),
      },
    ]);
    await waitFor(() => expect(screen.getByText(/trust/i)).toBeTruthy());
  });
});

