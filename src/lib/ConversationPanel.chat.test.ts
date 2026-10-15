import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('./conversation', async () => {
  const actual = await vi.importActual<typeof import('./conversation')>('./conversation');
  return { ...actual, sessionConversation: vi.fn(), sessionActivity: vi.fn(), listConversations: vi.fn(), toolDetail: vi.fn() };
});
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}));
vi.mock('./forms/forms', async () => ({
  ...(await vi.importActual<typeof import('./forms/forms')>('./forms/forms')),
  getForm: vi.fn(async () => ({ ok: false, error: { code: 'E_NOTFOUND', message: 'x' } })),
}));

import { sessionConversation, sessionActivity, listConversations, type Conversation } from './conversation';
import ConversationPanel from './ConversationPanel.svelte';
import { sessions, type SessionRow } from './sessions';
import { hosts } from './hosts';
import { session, host } from './hosts_fixture';
import { outbox } from './outbox';

// Thread room (2026-10-14): Control's chat is the same ConversationPanel in
// its chat look — bubbles, the agent's prose beside the mark, tool calls
// folded to one line, and `#`/`@` references as chips.

const mockedConv = sessionConversation as unknown as ReturnType<typeof vi.fn>;

function tool(summary: string) {
  return { kind: 'tool' as const, summary, error: false, id: null, name: 'Bash', target: 'ls', at: null, ended_at: null, done: true };
}

const operator: SessionRow = session('local', 'fleet-operator', { claude_session_id: 'c1' });
const flake = session('mercury', 'fix-flake');

function conv(prompt: string): Conversation {
  return {
    truncated: false,
    context: null,
    events: [],
    turns: [{ prompt, at: '2026-10-14T10:00:00.000Z', ended_at: null, items: [{ kind: 'text', text: 'On it.' }, tool('Bash(ls)'), tool('Bash(pwd)')] }],
  };
}

beforeEach(() => {
  mockedConv.mockReset();
  (sessionActivity as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({ ok: false, error: { code: 'E_INVALID_STATE', message: 'x' } });
  (listConversations as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({ ok: true, value: [] });
  outbox.resetForTests();
  sessions.set([operator, flake]);
  hosts.set([host('mercury')]);
});

describe('ConversationPanel, chat look', () => {
  it("draws the person's bubble, the agent's prose beside the mark and its tool calls as one folded line", async () => {
    mockedConv.mockResolvedValue({ ok: true, value: conv('fix the flake') });
    const { container } = render(ConversationPanel, { session: operator, visible: true, look: 'chat' });
    await screen.findByTestId('conv-prompt');
    expect(container.querySelector('.conversation-panel')!.classList.contains('chat-look')).toBe(true);
    expect(container.querySelector('.reply-mark .of-mark')).not.toBeNull();
    const tools = screen.getByTestId('conv-tools');
    expect(tools.querySelector('summary')!.textContent).toContain('Worked on 2 things');
    expect((tools as HTMLDetailsElement).open).toBe(false);
  });

  it('the transcript look keeps its own drawing', async () => {
    mockedConv.mockResolvedValue({ ok: true, value: conv('fix the flake') });
    const { container } = render(ConversationPanel, { session: operator, visible: true });
    await screen.findByTestId('conv-prompt');
    expect(container.querySelector('.chat-look')).toBeNull();
    expect(container.querySelector('.reply-mark')).toBeNull();
    expect(screen.getByTestId('conv-tools').querySelector('summary')!.textContent).not.toContain('Worked on');
  });

  it('a #task or @host / @session the person wrote is a chip that opens through the host', async () => {
    mockedConv.mockResolvedValue({ ok: true, value: conv('ask @fix-flake on @mercury about #PD-2592') });
    const onOpenRef = vi.fn();
    render(ConversationPanel, { session: operator, visible: true, look: 'chat', onOpenRef });
    await waitFor(() => expect(screen.getAllByTestId('conv-ref')).toHaveLength(3));
    const [s, h, t] = screen.getAllByTestId('conv-ref');
    expect(screen.getByTestId('conv-prompt').textContent).toContain('ask @fix-flake on @mercury about #PD-2592');
    await fireEvent.click(s);
    await fireEvent.click(h);
    await fireEvent.click(t);
    expect(onOpenRef.mock.calls.map((c) => c[0])).toEqual([
      { kind: 'session', id: flake.id, label: 'fix-flake' },
      { kind: 'host', alias: 'mercury' },
      { kind: 'task', key: 'PD-2592' },
    ]);
  });

  it("fleet's own prompt is a folded \"Fleet · N events\" line, not the person's bubble", async () => {
    const digest = [
      '[fleet events] What changed since the last note.',
      `- session fix-flake on mercury (#${flake.id}, you started it) finished a turn`,
      '- host mercury is low on memory (6% available)',
      '- … and 2 more.',
    ].join('\n');
    mockedConv.mockResolvedValue({ ok: true, value: conv(digest) });
    const onOpenRef = vi.fn();
    render(ConversationPanel, { session: operator, visible: true, look: 'chat', onOpenRef });
    const fold = (await screen.findByTestId('conv-fleet-events')) as HTMLDetailsElement;
    expect(screen.queryByTestId('conv-prompt')).toBeNull();
    expect(fold.open).toBe(false);
    expect(fold.querySelector('summary')!.textContent).toContain('Fleet · 4 events');
    expect(screen.getAllByTestId('conv-fleet-event').map((e) => e.textContent?.trim())).toEqual([
      `session fix-flake on mercury (#${flake.id}, you started it) finished a turn`,
      'host mercury is low on memory (6% available)',
    ]);
    await fireEvent.click(screen.getByTestId('conv-fleet-event-open'));
    expect(onOpenRef.mock.calls[0][0]).toMatchObject({ kind: 'session', id: flake.id });
    // Control's reaction still reads as its reply.
    expect(screen.getByTestId('conv-text').textContent).toContain('On it.');
  });

  it("the transcript look leaves fleet's prompt as it was typed", async () => {
    mockedConv.mockResolvedValue({ ok: true, value: conv('[fleet] Your session was restarted automatically.') });
    render(ConversationPanel, { session: operator, visible: true });
    expect((await screen.findByTestId('conv-prompt')).textContent).toContain('[fleet] Your session was restarted');
    expect(screen.queryByTestId('conv-fleet-events')).toBeNull();
  });
});
