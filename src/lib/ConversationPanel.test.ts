import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('./conversation', async () => {
  const actual = await vi.importActual<typeof import('./conversation')>('./conversation');
  return { ...actual, sessionConversation: vi.fn() };
});
import { sessionConversation, CONVERSATION_POLL_MS, type Conversation } from './conversation';
import ConversationPanel from './ConversationPanel.svelte';
import type { SessionRow } from './sessions';

const mockedConv = sessionConversation as unknown as ReturnType<typeof vi.fn>;

function session(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: 1, tmux_name: 'ctl', host_alias: 'local', project_id: null, worktree_id: null,
    created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null,
    kind: 'work', reviews_session_id: null, worktree_key: null, lost_at: null,
    claude_session_id: 'sess-abc', claude_status: null, effort_level: null, pr_url: null,
    current_activity: null, context_pct: null, stuck_kind: null, friendly_name: null,
    safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null,
    idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null,
    last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [],
    ...over,
  } as SessionRow;
}

function conv(over: Partial<Conversation> = {}): Conversation {
  return {
    truncated: false,
    turns: [
      {
        prompt: 'fix the bug',
        at: '2026-09-13T10:00:00.000Z',
        items: [
          { kind: 'text', text: 'looking into it' },
          { kind: 'tool', summary: 'Bash(command=ls -la)' },
        ],
      },
    ],
    ...over,
  };
}

function ok(value: Conversation) {
  return Promise.resolve({ ok: true as const, value });
}
function err(code: string, message = code) {
  return Promise.resolve({ ok: false as const, error: { code, message } });
}

function setVisibility(state: 'visible' | 'hidden') {
  Object.defineProperty(document, 'visibilityState', { value: state, configurable: true });
}

beforeEach(() => {
  mockedConv.mockReset();
  setVisibility('visible');
});

afterEach(() => {
  vi.useRealTimers();
  setVisibility('visible');
});

describe('ConversationPanel', () => {
  it('renders prompt blocks, text items, tool items, and the truncated notice', async () => {
    mockedConv.mockReturnValue(ok(conv({ truncated: true })));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    expect(screen.getByTestId('conv-prompt').textContent).toContain('fix the bug');
    expect(screen.getByTestId('conv-text').textContent).toContain('looking into it');
    expect(screen.getByTestId('conv-tool').textContent).toContain('Bash(command=ls -la)');
    expect(screen.getByText('Older turns not shown')).toBeTruthy();
  });

  it('shows "No conversation yet" for E_NO_TRANSCRIPT', async () => {
    mockedConv.mockReturnValue(err('E_NO_TRANSCRIPT'));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    expect(screen.getByText('No conversation yet')).toBeTruthy();
  });

  it('shows "No Claude session id yet" and does not fetch when the session has no claude_session_id', async () => {
    render(ConversationPanel, { session: session({ claude_session_id: null }), visible: true });
    await tick();
    expect(screen.getByText('No Claude session id yet')).toBeTruthy();
    expect(mockedConv).not.toHaveBeenCalled();
  });

  it('other errors show the message plus a retry button that refetches, keeping earlier good turns visible', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValueOnce(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    expect(screen.getByTestId('conv-prompt').textContent).toContain('fix the bug');

    mockedConv.mockReturnValueOnce(err('E_SSH', 'connection refused'));
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await Promise.resolve();
    await tick();

    expect(screen.getByTestId('conv-error').textContent).toContain('connection refused');
    expect(screen.getByTestId('conv-prompt').textContent).toContain('fix the bug');

    mockedConv.mockReturnValueOnce(ok(conv()));
    await fireEvent.click(screen.getByTestId('conv-retry'));
    await Promise.resolve();
    await tick();

    expect(mockedConv).toHaveBeenCalledTimes(3);
    expect(screen.queryByTestId('conv-error')).toBeNull();
  });

  it('polls every 5s while visible; stops when not visible; skips a tick when the document is hidden', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    const { rerender } = render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(1);

    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await Promise.resolve();
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(2);

    setVisibility('hidden');
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await Promise.resolve();
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(2);

    setVisibility('visible');
    await rerender({ session: session(), visible: false });
    mockedConv.mockClear();
    vi.advanceTimersByTime(CONVERSATION_POLL_MS * 3);
    await Promise.resolve();
    await tick();
    expect(mockedConv).not.toHaveBeenCalled();
  });

  it('drops an in-flight response for the old session when the session changes', async () => {
    let resolveFirst!: (v: { ok: true; value: Conversation }) => void;
    const first = new Promise<{ ok: true; value: Conversation }>((res) => (resolveFirst = res));
    mockedConv.mockReturnValueOnce(first);
    const { rerender } = render(ConversationPanel, { session: session({ id: 1 }), visible: true });
    await tick();

    mockedConv.mockReturnValueOnce(ok(conv({ turns: [{ prompt: 'second session prompt', at: null, items: [{ kind: 'text', text: 'hi' }] }] })));
    await rerender({ session: session({ id: 2 }), visible: true });
    await tick();
    await Promise.resolve();
    await tick();

    // Now let the stale first-session promise resolve; its content must never appear.
    resolveFirst(ok(conv({ turns: [{ prompt: 'STALE first session prompt', at: null, items: [] }] })) as unknown as { ok: true; value: Conversation });
    await Promise.resolve();
    await tick();

    expect(screen.queryByText('STALE first session prompt')).toBeNull();
    expect(screen.getByText('second session prompt')).toBeTruthy();
  });

  it('does not replace DOM nodes when a poll result is identical', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    const node = screen.getByTestId('conv-prompt');

    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await Promise.resolve();
    await tick();

    expect(screen.getByTestId('conv-prompt')).toBe(node);
  });

  it('the relative-time label advances on an independent clock while the poll payload stays identical', async () => {
    const start = new Date('2026-09-13T12:00:00.000Z');
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'Date'] });
    vi.setSystemTime(start);
    const c = conv({
      turns: [
        {
          prompt: 'fix the bug',
          at: start.toISOString(),
          items: [{ kind: 'text', text: 'looking into it' }],
        },
      ],
    });
    mockedConv.mockReturnValue(ok(c));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    expect(screen.getByTestId('conv-prompt').textContent).toContain('just now');

    // Advance a full minute: the 5s poll fires several times with an
    // unchanged payload (never touching `conv`), but the label still ages
    // because its clock (`nowMs`) ticks independently every 30s.
    vi.advanceTimersByTime(60_000);
    await Promise.resolve();
    await tick();

    expect(screen.getByTestId('conv-prompt').textContent).toContain('1m ago');
  });
  it('never starts a poll fetch while one is in flight for the same session', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    let resolveFirst!: (v: { ok: true; value: Conversation }) => void;
    const first = new Promise<{ ok: true; value: Conversation }>((res) => (resolveFirst = res));
    mockedConv.mockReturnValueOnce(first);
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(1);

    // Two interval ticks while the first read is still pending: no new call.
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await Promise.resolve();
    await tick();
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await Promise.resolve();
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(1);

    // Once it settles, the next tick polls again.
    resolveFirst({ ok: true, value: conv() });
    await Promise.resolve();
    await tick();
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await Promise.resolve();
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(2);
  });

  it('a manual Retry still fetches while a poll is in flight', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValueOnce(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();

    mockedConv.mockReturnValueOnce(err('E_SSH', 'connection refused'));
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await Promise.resolve();
    await tick();
    expect(screen.getByTestId('conv-error')).toBeTruthy();

    // A poll starts and hangs.
    mockedConv.mockReturnValueOnce(new Promise(() => {}));
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await Promise.resolve();
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(3);

    mockedConv.mockReturnValueOnce(ok(conv({ turns: [{ prompt: 'after retry', at: null, items: [] }] })));
    await fireEvent.click(screen.getByTestId('conv-retry'));
    await Promise.resolve();
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(4);
    expect(screen.getByTestId('conv-prompt').textContent).toContain('after retry');
    expect(screen.queryByTestId('conv-error')).toBeNull();
  });

  it('a session switch fetches even while the old session\'s read is in flight', async () => {
    mockedConv.mockReturnValueOnce(new Promise(() => {}));
    const { rerender } = render(ConversationPanel, { session: session({ id: 1 }), visible: true });
    await tick();
    mockedConv.mockReturnValueOnce(ok(conv()));
    await rerender({ session: session({ id: 2 }), visible: true });
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(2);
    expect(mockedConv).toHaveBeenLastCalledWith(2);
  });

  it('does not render an empty quote block for a turn without a prompt', async () => {
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            { prompt: null, at: '2026-09-13T10:00:00.000Z', items: [{ kind: 'text', text: 'resumed reply' }] },
            { prompt: 'next ask', at: null, items: [{ kind: 'text', text: 'ok' }] },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    const quotes = screen.getAllByTestId('conv-prompt');
    expect(quotes).toHaveLength(1);
    expect(quotes[0].textContent).toContain('next ask');
    expect(screen.getByText('resumed reply')).toBeTruthy();
  });

  it('refetches immediately when it becomes visible again', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    const { rerender } = render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(1);

    await rerender({ session: session(), visible: false });
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(1);

    // No timer advance: the flip itself triggers the fetch.
    await rerender({ session: session(), visible: true });
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(2);
  });

  it('mounting visible fetches once, not twice', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(1);
  });
  it('a new row object for the same session (a store patch) neither resets nor refetches', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    const { rerender } = render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    const node = screen.getByTestId('conv-prompt');
    await rerender({ session: session({ claude_status: 'working' }), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    expect(mockedConv).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('conv-prompt')).toBe(node);
  });
});
