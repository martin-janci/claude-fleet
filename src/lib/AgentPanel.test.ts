import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock('./ConversationPanel.svelte', () => ({ default: () => ({}) }));

import { get } from 'svelte/store';
import AgentPanel from './AgentPanel.svelte';
import { agentPanelOpen, operatorError, operatorState, operatorSession } from './operator';
import { sessions } from './sessions';
import { hostsViewRequest, settingsOpen, settingsSection } from './app_views';
import { agentPanelSize, agentPanelMaximized, AGENT_PANEL_MIN_W } from './agent_panel_size';

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
  agentPanelOpen.set(true);
  operatorState.set('ready');
  operatorSession.set(row());
  sessions.set([]);
  agentPanelSize.set(null);
  agentPanelMaximized.set(false);
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

  it('shows why a restart of the lost agent failed, under the button', async () => {
    operatorState.set('lost');
    invoke.mockRejectedValueOnce({
      code: 'E_TMUX',
      message: 'error connecting to /tmp/tmux-1000/default (No such file or directory)',
    });
    render(AgentPanel);
    await fireEvent.click(screen.getByRole('button', { name: /restart/i }));
    const note = await screen.findByTestId('agent-panel-error');
    expect(note.textContent).toContain('Restart failed: error connecting to');
    expect(invoke).toHaveBeenCalledWith('restart_session', expect.anything());
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

  // It used to be a fixed 360px column under a 60vh cap, with no way to
  // make it bigger for a long answer.
  describe('resizing', () => {
    function stubRect(el: HTMLElement, r: { left: number; top: number; right: number; bottom: number }) {
      el.getBoundingClientRect = () =>
        ({ ...r, x: r.left, y: r.top, width: r.right - r.left, height: r.bottom - r.top }) as DOMRect;
    }

    it('grows up and left when its top-left grip is dragged', async () => {
      render(AgentPanel);
      const panel = screen.getByTestId('agent-panel');
      stubRect(panel, { left: 600, top: 300, right: 960, bottom: 700 });
      const grip = screen.getByTestId('agent-panel-grip');
      await fireEvent.pointerDown(grip, { button: 0, clientX: 600, clientY: 300, pointerId: 1 });
      await fireEvent.pointerMove(grip, { clientX: 500, clientY: 150, pointerId: 1 });
      await fireEvent.pointerUp(grip, { pointerId: 1 });
      expect(get(agentPanelSize)).toEqual({ w: 460, h: 550 });
      expect(panel.classList.contains('sized')).toBe(true);
      expect(panel.style.getPropertyValue('--agent-w')).toBe('460px');
    });

    it('never shrinks below the minimum or grows past the window edge', async () => {
      render(AgentPanel);
      const panel = screen.getByTestId('agent-panel');
      stubRect(panel, { left: 600, top: 300, right: 960, bottom: 700 });
      const grip = screen.getByTestId('agent-panel-grip');
      await fireEvent.pointerDown(grip, { button: 0, clientX: 600, clientY: 300, pointerId: 1 });
      await fireEvent.pointerMove(grip, { clientX: 950, clientY: 690, pointerId: 1 });
      expect(get(agentPanelSize)!.w).toBe(AGENT_PANEL_MIN_W);
      await fireEvent.pointerMove(grip, { clientX: -500, clientY: -500, pointerId: 1 });
      // right 960 - 20px margin, bottom 700 - 20px margin.
      expect(get(agentPanelSize)).toEqual({ w: 940, h: 680 });
    });

    it('resizes from the keyboard and resets with Home', async () => {
      render(AgentPanel);
      const panel = screen.getByTestId('agent-panel');
      stubRect(panel, { left: 600, top: 300, right: 960, bottom: 700 });
      const grip = screen.getByTestId('agent-panel-grip');
      await fireEvent.keyDown(grip, { key: 'ArrowLeft' });
      expect(get(agentPanelSize)).toEqual({ w: 380, h: 400 });
      await fireEvent.keyDown(grip, { key: 'Home' });
      expect(get(agentPanelSize)).toBeNull();
      expect(panel.classList.contains('sized')).toBe(false);
    });

    it('maximizes and restores from the header', async () => {
      render(AgentPanel);
      const max = screen.getByTestId('agent-panel-maximize');
      await fireEvent.click(max);
      expect(screen.getByTestId('agent-panel').classList.contains('maximized')).toBe(true);
      expect(max.getAttribute('aria-pressed')).toBe('true');
      await fireEvent.click(max);
      expect(screen.getByTestId('agent-panel').classList.contains('maximized')).toBe(false);
    });
  });
});

describe('AgentPanel: a next step for every blocked state (step 9.1)', () => {
  it('no_mcp opens Settings › Control API, and calls no command', async () => {
    operatorState.set('no_mcp');
    render(AgentPanel);
    await fireEvent.click(screen.getByRole('button', { name: 'Open Settings › Control API' }));
    expect(get(settingsSection)).toBe('control-api');
    expect(get(settingsOpen)).toBe(true);
    expect(invoke).not.toHaveBeenCalled();
    settingsOpen.set(false);
    settingsSection.set(null);
  });

  it('no_host and host_down without a fallback open the Hosts view', async () => {
    operatorState.set('no_host');
    const { unmount } = render(AgentPanel);
    await fireEvent.click(screen.getByRole('button', { name: 'Add a host' }));
    expect(get(hostsViewRequest)).toEqual({ host: null });
    unmount();
    hostsViewRequest.set(null);
  });

  it('token_revoked asks before it kills, and Cancel kills nothing', async () => {
    operatorState.set('token_revoked');
    render(AgentPanel);
    await fireEvent.click(screen.getByRole('button', { name: 'Replace the agent' }));
    expect(screen.getByTestId('agent-replace-confirm')).toBeTruthy();
    expect(invoke).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByTestId('agent-replace-confirm')).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });

  it('token_revoked, confirmed, kills the session and starts a new agent', async () => {
    operatorState.set('token_revoked');
    operatorSession.set(row({ id: 71 }));
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'kill_session') return 71;
      if (cmd === 'operator_status') return { ready: false, session: null, blocked: 'absent' };
      if (cmd === 'ensure_operator') return row({ id: 72 });
      return null;
    });
    render(AgentPanel);
    await fireEvent.click(screen.getByRole('button', { name: 'Replace the agent' }));
    await fireEvent.click(screen.getByTestId('agent-replace-yes'));
    await vi.waitFor(() => expect(get(operatorState)).toBe('ready'));
    const cmds = invoke.mock.calls.map((c) => c[0]);
    expect(cmds).toEqual(['kill_session', 'operator_status', 'ensure_operator']);
  });

  it('embedded (Control) shows without the sheet and has no close or grip', async () => {
    agentPanelOpen.set(false);
    operatorState.set('lost');
    invoke.mockResolvedValue({ ready: false, session: row(), blocked: 'lost' });
    render(AgentPanel, { embedded: true });
    expect(screen.getByTestId('control-agent')).toBeTruthy();
    expect(screen.queryByTestId('agent-panel-close')).toBeNull();
    expect(screen.queryByTestId('agent-panel-grip')).toBeNull();
    expect(screen.getByRole('button', { name: 'Restart the agent' })).toBeTruthy();
    // Opening Control makes sure there is an agent, without opening the sheet.
    expect(invoke).toHaveBeenCalledWith('operator_status', undefined);
    expect(get(agentPanelOpen)).toBe(false);
  });
});
