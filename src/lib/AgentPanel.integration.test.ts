// AgentPanel.test.ts mocks ConversationPanel down to a stub, which proves
// AgentPanel's own frame but nothing about what happens when the two
// components are actually stacked. This file mounts the REAL
// ConversationPanel underneath AgentPanel — the arrangement App.svelte will
// use — to prove the fix for the duplicate-composer defect: exactly one
// Send button reaches the page, because ConversationPanel is told
// `showComposer={false}` and only AgentPanel's own composer sends.
import { render, screen } from '@testing-library/svelte';
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
import { agentPanelOpen, operatorState, operatorSession } from './operator';
import { sessionConversation, sessionActivity, listConversations, toolDetail } from './conversation';
import { sendPrompt, type SessionRow } from './sessions';

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
});

describe('AgentPanel with the real ConversationPanel underneath', () => {
  it('renders exactly one composer — ConversationPanel is told not to bring its own', async () => {
    render(AgentPanel);
    await tick();
    await Promise.resolve();
    await tick();

    // AgentPanel's own composer.
    expect(screen.getAllByRole('button', { name: /^send$/i })).toHaveLength(1);
    // ConversationPanel's composer never mounted at all.
    expect(screen.queryByTestId('conv-composer')).toBeNull();
    expect(screen.queryByTestId('conv-composer-send')).toBeNull();
    expect(screen.queryByTestId('conv-readonly')).toBeNull();
  });
});
