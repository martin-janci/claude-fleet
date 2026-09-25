// AgentPanel.test.ts mocks ConversationPanel down to a stub, which proves
// AgentPanel's own frame but nothing about what happens when the two
// components are actually stacked. This file mounts the REAL
// ConversationPanel underneath AgentPanel — the arrangement App.svelte uses
// — to prove that exactly one composer reaches the page and that it is
// ConversationPanel's own, so everything the sheet sends goes through the
// send path that owns the panel's live state.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

vi.mock('./conversation', async () => {
  const actual = await vi.importActual<typeof import('./conversation')>('./conversation');
  return {
    ...actual,
    sessionConversation: vi.fn(),
    sessionActivity: vi.fn(),
    listConversations: vi.fn(),
    toolDetail: vi.fn(),
  };
});
vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return { ...actual, sendPrompt: vi.fn() };
});

import AgentPanel from './AgentPanel.svelte';
import { get } from 'svelte/store';
import { agentPanelOpen, operatorState, operatorSession } from './operator';
import { sessionConversation, sessionActivity, listConversations, toolDetail } from './conversation';
import { sendPrompt, sessions, applySessionEvents, type SessionRow } from './sessions';

const mockedConv = sessionConversation as unknown as ReturnType<typeof vi.fn>;
const mockedAct = sessionActivity as unknown as ReturnType<typeof vi.fn>;
const mockedList = listConversations as unknown as ReturnType<typeof vi.fn>;
const mockedDetail = toolDetail as unknown as ReturnType<typeof vi.fn>;
const mockedSend = sendPrompt as unknown as ReturnType<typeof vi.fn>;

function row(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: 7,
    tmux_name: 'fleet-operator',
    host_alias: 'local',
    project_id: null,
    worktree_id: null,
    created_at: 1,
    last_activity_at: 1,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: null,
    lost_at: null,
    claude_session_id: 'sess-op',
    claude_status: 'idle',
    effort_level: null,
    pr_url: null,
    current_activity: null,
    context_pct: null,
    stuck_kind: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null,
    idle_since: null,
    stuck_since: null,
    last_playbook_at: null,
    last_prompt: null,
    started_at: null,
    last_turn_at: null,
    ci_status: null,
    turn_seq: 0,
    last_stop_at: null,
    parent_session_id: null,
    tags: [],
    ...over,
  } as SessionRow;
}

beforeEach(() => {
  invoke.mockReset();
  mockedConv.mockReset();
  mockedConv.mockReturnValue(
    Promise.resolve({ ok: true, value: { truncated: false, context: null, events: [], turns: [] } }),
  );
  mockedAct.mockReset();
  mockedAct.mockResolvedValue({ ok: false, error: { code: 'E_INVALID_STATE', message: 'no pane' } });
  mockedList.mockReset();
  mockedList.mockResolvedValue({ ok: true, value: [] });
  mockedDetail.mockReset();
  mockedSend.mockReset();
  agentPanelOpen.set(true);
  operatorState.set('ready');
  operatorSession.set(row());
  sessions.set([row()]);
});

describe('AgentPanel with the real ConversationPanel underneath', () => {
  it('renders exactly one composer — ConversationPanel\'s, and the sheet adds none', async () => {
    render(AgentPanel);
    await tick();
    await Promise.resolve();
    await tick();

    expect(screen.getAllByTestId('conv-composer')).toHaveLength(1);
    expect(screen.getAllByTestId('conv-composer-send')).toHaveLength(1);
    // The sheet's own textarea is gone for good: it is what sent around
    // ConversationPanel's live state in the first place.
    expect(screen.queryByPlaceholderText(/ask the agent/i)).toBeNull();
    expect(screen.queryByTestId('conv-readonly')).toBeNull();
  });
});

// The defect these cover: AgentPanel used to own its own composer and call
// `sendPrompt` itself, bypassing ConversationPanel's send path entirely. All
// of ConversationPanel's liveness state — the pending turn, the `optimistic`
// flag that keeps the 5 s transcript cadence off the 15 s quiet one, and the
// immediate refetch — is set in that path and nowhere else, so a prompt sent
// from the agent sheet showed nothing, said nothing, and appeared whenever
// the next quiet tick happened to land. One composer, fed the chip's prefix,
// is the fix.
describe('the agent sheet sends through ConversationPanel, not around it', () => {
  async function settle() {
    for (let i = 0; i < 4; i++) {
      await tick();
      await Promise.resolve();
    }
  }

  it('shows what was just sent as a pending turn, before any transcript read carries it', async () => {
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(AgentPanel);
    await settle();

    const box = screen.getByPlaceholderText(/send a prompt/i) as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'co sa deje' } });
    await fireEvent.click(screen.getByTestId('conv-composer-send'));
    await waitFor(() => expect(screen.getByTestId('conv-pending')).toBeTruthy());
    expect(screen.getByTestId('conv-pending').textContent).toContain('co sa deje');
  });

  it('carries the context chip prefix into the one composer that sends', async () => {
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(AgentPanel, {
      contextInput: { view: 'hosts', session: null, hostAlias: 'beta', branch: null, friendly: true },
    });
    await settle();

    const box = screen.getByPlaceholderText(/send a prompt/i) as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'hello' } });
    await fireEvent.click(screen.getByTestId('conv-composer-send'));
    await waitFor(() => expect(mockedSend).toHaveBeenCalled());
    const body = mockedSend.mock.calls[0][2] as string;
    expect(body).toContain('beta');
    expect(body).toContain('hello');
  });

  it('leaves a slash command exactly as typed — the REPL reads it, not Claude', async () => {
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(AgentPanel, {
      contextInput: { view: 'hosts', session: null, hostAlias: 'beta', branch: null, friendly: true },
    });
    await settle();

    const box = screen.getByPlaceholderText(/send a prompt/i) as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: '/clear' } });
    await fireEvent.click(screen.getByTestId('conv-composer-send'));
    await waitFor(() => expect(mockedSend).toHaveBeenCalled());
    expect(mockedSend.mock.calls[0][2]).toBe('/clear');
  });

  it('still renders exactly one composer — the sheet no longer brings a second', async () => {
    render(AgentPanel);
    await settle();
    expect(screen.getAllByTestId('conv-composer')).toHaveLength(1);
    expect(screen.queryByPlaceholderText(/ask the agent/i)).toBeNull();
  });
});

// Moved here from AgentPanel.test.ts when the sheet lost its own composer:
// these are about the sheet's state reaching ConversationPanel's composer,
// so they need the real component, not a stub.
describe('the sheet feeds the one composer its live state', () => {
  async function settle() {
    for (let i = 0; i < 4; i++) {
      await tick();
      await Promise.resolve();
    }
  }

  // The note, not the gate — the gate has its own tests below, and conflating
  // the two is how F3 shipped.
  it('the busy note MOVES on a row event, not only when the panel was opened', async () => {
    render(AgentPanel);
    await settle();
    expect(screen.queryByTestId('conv-composer-status')).toBeNull();

    applySessionEvents([
      { type: 'updated', row: row({ claude_status: 'working', last_activity_at: 200 }) },
    ]);
    await waitFor(() =>
      expect(screen.getByTestId('conv-composer-status').textContent).toMatch(/working/i),
    );

    applySessionEvents([
      { type: 'updated', row: row({ claude_status: 'idle', last_activity_at: 300 }) },
    ]);
    await waitFor(() => expect(screen.queryByTestId('conv-composer-status')).toBeNull());
  });

  it('surfaces a stuck_kind that arrives after the panel opened', async () => {
    render(AgentPanel);
    await settle();
    applySessionEvents([
      {
        type: 'updated',
        row: row({ claude_status: 'working', stuck_kind: 'trust_prompt', last_activity_at: 200 }),
      },
    ]);
    await waitFor(() =>
      expect(screen.getByTestId('conv-composer-status').textContent).toMatch(/trust/i),
    );
  });

  it('a dropped chip comes back when the context changes, and its prefix is sent again', async () => {
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    const { rerender } = render(AgentPanel, {
      contextInput: { view: 'hosts', session: null, hostAlias: 'alpha', branch: null, friendly: true },
    });
    await settle();
    expect(screen.getByTestId('agent-context-chip').textContent).toContain('alpha');

    await fireEvent.click(screen.getByTestId('agent-context-chip'));
    expect(screen.queryByTestId('agent-context-chip')).toBeNull();

    // Same context re-rendered — the drop persists, this is not a one-shot toggle.
    await rerender({ contextInput: { view: 'hosts', session: null, hostAlias: 'alpha', branch: null, friendly: true } });
    expect(screen.queryByTestId('agent-context-chip')).toBeNull();

    // A different context — the chip returns on its own, no re-click needed.
    await rerender({ contextInput: { view: 'hosts', session: null, hostAlias: 'beta', branch: null, friendly: true } });
    expect(screen.getByTestId('agent-context-chip').textContent).toContain('beta');

    const box = screen.getByPlaceholderText(/send a prompt/i) as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'hello' } });
    await fireEvent.click(screen.getByTestId('conv-composer-send'));
    await waitFor(() => expect(mockedSend).toHaveBeenCalled());
    expect(mockedSend.mock.calls[0][2]).toContain('beta');
  });

  it('a dropped chip sends nothing in front of the prompt', async () => {
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(AgentPanel, {
      contextInput: { view: 'hosts', session: null, hostAlias: 'alpha', branch: null, friendly: true },
    });
    await settle();
    await fireEvent.click(screen.getByTestId('agent-context-chip'));

    const box = screen.getByPlaceholderText(/send a prompt/i) as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'bare' } });
    await fireEvent.click(screen.getByTestId('conv-composer-send'));
    await waitFor(() => expect(mockedSend).toHaveBeenCalled());
    expect(mockedSend.mock.calls[0][2]).toBe('bare');
  });

  // Round 20 F3. The sheet's own composer gated on agent status before the
  // refactor deleted it (`busy = statusNote !== null`; `disabled={busy || …}`),
  // and the composer it delegated to had no such term — so the sheet silently
  // started accepting a second prompt mid-turn. The deleted test named the
  // consequence: two pastes into one REPL is one mangled prompt. Its
  // replacement asserted only that a note renders, which a broken gate passes.
  //
  // This asserts the SEND IS REFUSED, which is the thing that was lost.
  it('refuses to send while the agent is working — the gate, not just the note', async () => {
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(AgentPanel);
    await settle();

    const box = screen.getByPlaceholderText(/send a prompt/i) as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'druhy prompt' } });
    applySessionEvents([
      { type: 'updated', row: row({ claude_status: 'working', last_activity_at: 200 }) },
    ]);
    await waitFor(() =>
      expect(screen.getByTestId('conv-composer-status').textContent).toMatch(/working/i),
    );

    expect((screen.getByTestId('conv-composer-send') as HTMLButtonElement).disabled).toBe(true);
    // Not only the button: Enter goes down the same path and must be refused too.
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    await fireEvent.click(screen.getByTestId('conv-composer-send'));
    await settle();
    expect(mockedSend).not.toHaveBeenCalled();

    // …and it is a gate, not a wall: the turn ends and the same draft goes.
    applySessionEvents([
      { type: 'updated', row: row({ claude_status: 'idle', last_activity_at: 300 }) },
    ]);
    await waitFor(() => expect(screen.queryByTestId('conv-composer-status')).toBeNull());
    await fireEvent.click(screen.getByTestId('conv-composer-send'));
    await waitFor(() => expect(mockedSend).toHaveBeenCalledTimes(1));
    expect(mockedSend.mock.calls[0][2]).toBe('druhy prompt');
  });

  it('refuses to send while the agent is stuck, where the prompt may never be read', async () => {
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(AgentPanel);
    await settle();
    applySessionEvents([
      {
        type: 'updated',
        row: row({ claude_status: 'blocked', stuck_kind: 'trust_prompt', last_activity_at: 200 }),
      },
    ]);
    await waitFor(() => expect(screen.getByTestId('conv-composer-status')).toBeTruthy());

    const box = screen.getByPlaceholderText(/send a prompt/i) as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'ahoj' } });
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    expect(mockedSend).not.toHaveBeenCalled();
  });

  // The one exemption, and the reason it has to exist: the gate reads the
  // stuck state, and the press_enter chip is the way OUT of it. Gating the
  // recovery would wedge the sheet exactly when it is already wedged.
  it('still sends the bare-Enter recovery chip while stuck — a key press is not a prompt', async () => {
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(AgentPanel, {
      contextInput: { view: 'hosts', session: null, hostAlias: 'beta', branch: null, friendly: true },
    });
    await settle();
    applySessionEvents([
      {
        type: 'updated',
        row: row({ claude_status: 'blocked', stuck_kind: 'press_enter', last_activity_at: 200 }),
      },
    ]);
    await waitFor(() => expect(screen.getByTestId('conv-chip-enter')).toBeTruthy());

    await fireEvent.click(screen.getByTestId('conv-chip-enter'));
    await waitFor(() => expect(mockedSend).toHaveBeenCalled());
    // Bare, with the context chip active: not the paragraph (F2), not refused.
    expect(mockedSend.mock.calls[0][2]).toBe('');
  });

  it('closes on Escape from inside the composer, where a window-level handler would not', async () => {
    render(AgentPanel);
    await settle();
    await fireEvent.keyDown(screen.getByPlaceholderText(/send a prompt/i), { key: 'Escape' });
    expect(get(agentPanelOpen)).toBe(false);
  });
});

// Work graph M9: operator commands fill the operator's composer, never send.
describe('operator commands', () => {
  it('"Tidy up done tickets" puts its request in the box and sends nothing', async () => {
    render(AgentPanel);
    for (let i = 0; i < 4; i++) {
      await tick();
      await Promise.resolve();
    }
    const cmd = screen
      .getAllByTestId('agent-command')
      .find((b) => b.textContent === 'Tidy up done tickets')!;
    await fireEvent.click(cmd);
    await tick();
    await tick();
    const box = screen.getByPlaceholderText(/send a prompt/i) as HTMLTextAreaElement;
    expect(box.value).toContain('`work` tool, action `tidy`');
    expect(box.value).toContain('`tidy_apply`');
    expect(mockedSend).not.toHaveBeenCalled();
  });
});
