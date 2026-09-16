import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('./conversation', async () => {
  const actual = await vi.importActual<typeof import('./conversation')>('./conversation');
  return { ...actual, sessionConversation: vi.fn(), sessionActivity: vi.fn() };
});
vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return { ...actual, sendPrompt: vi.fn() };
});
import { sessionConversation, sessionActivity, CONVERSATION_POLL_MS, ACTIVITY_POLL_MS, QUIET_POLL_MS, type Conversation, type ActivityProbe } from './conversation';
import ConversationPanel from './ConversationPanel.svelte';
import { sendPrompt, type SessionRow } from './sessions';
import { composerPresets, resetComposerPresets } from './composer_presets';
import { composerDrafts } from './conversation';

const mockedConv = sessionConversation as unknown as ReturnType<typeof vi.fn>;
const mockedSend = sendPrompt as unknown as ReturnType<typeof vi.fn>;
const mockedAct = sessionActivity as unknown as ReturnType<typeof vi.fn>;

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
        ended_at: null,
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
  mockedSend.mockReset();
  mockedAct.mockReset();
  mockedAct.mockResolvedValue({ ok: false, error: { code: 'E_INVALID_STATE', message: 'no pane' } });
  composerDrafts.clear();
  resetComposerPresets();
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

    mockedConv.mockReturnValueOnce(ok(conv({ turns: [{ prompt: 'second session prompt', at: null, ended_at: null, items: [{ kind: 'text', text: 'hi' }] }] })));
    await rerender({ session: session({ id: 2 }), visible: true });
    await tick();
    await Promise.resolve();
    await tick();

    // Now let the stale first-session promise resolve; its content must never appear.
    resolveFirst(ok(conv({ turns: [{ prompt: 'STALE first session prompt', at: null, ended_at: null, items: [] }] })) as unknown as { ok: true; value: Conversation });
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
          ended_at: null,
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

    mockedConv.mockReturnValueOnce(ok(conv({ turns: [{ prompt: 'after retry', at: null, ended_at: null, items: [] }] })));
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
            { prompt: null, at: '2026-09-13T10:00:00.000Z', ended_at: null, items: [{ kind: 'text', text: 'resumed reply' }] },
            { prompt: 'next ask', at: null, ended_at: null, items: [{ kind: 'text', text: 'ok' }] },
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
  it('renders reply text as markdown', async () => {
    mockedConv.mockReturnValue(
      ok(conv({ turns: [{ prompt: 'q', at: null, ended_at: null, items: [{ kind: 'text', text: '## Done\n\n- **one**\n- two' }] }] })),
    );
    const { container } = render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    const reply = screen.getByTestId('conv-text');
    expect(reply.querySelector('.md-h2')?.textContent).toBe('Done');
    expect(reply.querySelectorAll('li')).toHaveLength(2);
    expect(reply.querySelector('strong')?.textContent).toBe('one');
    expect(container.textContent).not.toContain('**one**');
  });

  it('folds consecutive tool calls into one expandable group; a single call stays a line', async () => {
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            {
              prompt: 'q',
              at: null,
              ended_at: null,
              items: [
                { kind: 'tool', summary: 'Read(file_path=a)' },
                { kind: 'tool', summary: 'Bash(command=ls)' },
                { kind: 'tool', summary: 'Read(file_path=b)' },
                { kind: 'text', text: 'between' },
                { kind: 'tool', summary: 'Edit(file_path=c)' },
              ],
            },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    const group = screen.getByTestId('conv-tools') as HTMLDetailsElement;
    expect(group.open).toBe(false);
    expect(group.querySelector('summary')?.textContent).toContain('3 tool calls · Read, Bash');
    expect(group.querySelectorAll('[data-testid="conv-tool"]')).toHaveLength(3);
    const all = screen.getAllByTestId('conv-tool');
    expect(all).toHaveLength(4);
    expect(all[3].closest('details')).toBeNull();
    expect(all[3].textContent).toContain('Edit(file_path=c)');
  });

  it('clamps a long prompt with Show more / Show less', async () => {
    const long = Array.from({ length: 12 }, (_, k) => `line ${k}`).join('\n');
    mockedConv.mockReturnValue(ok(conv({ turns: [{ prompt: long, at: null, ended_at: null, items: [{ kind: 'text', text: 'ok' }] }] })));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    const body = screen.getByTestId('conv-prompt').querySelector('.prompt-text')!;
    expect(body.classList.contains('clamped')).toBe(true);
    await fireEvent.click(screen.getByTestId('conv-prompt-toggle'));
    expect(body.classList.contains('clamped')).toBe(false);
    expect(screen.getByTestId('conv-prompt-toggle').textContent).toBe('Show less');
  });

  it('a short prompt has no toggle', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    expect(screen.queryByTestId('conv-prompt-toggle')).toBeNull();
  });

  it('shows a Latest button when scrolled up, which jumps back to the bottom', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    expect(screen.queryByTestId('conv-latest')).toBeNull();
    const scroller = screen.getByTestId('conv-scroller');
    Object.defineProperty(scroller, 'scrollHeight', { value: 2000, configurable: true });
    Object.defineProperty(scroller, 'clientHeight', { value: 500, configurable: true });
    scroller.scrollTop = 100;
    await fireEvent.scroll(scroller);
    const latest = screen.getByTestId('conv-latest');
    await fireEvent.click(latest);
    expect(scroller.scrollTop).toBe(2000);
    await fireEvent.scroll(scroller);
    expect(screen.queryByTestId('conv-latest')).toBeNull();
  });
});

async function settle() {
  await tick();
  await Promise.resolve();
  await tick();
}

describe('ConversationPanel composer', () => {
  it('sends the typed prompt to the session on Enter, clears the box, shows it as a pending turn and refetches at once', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(ConversationPanel, { session: session({ host_alias: 'trn', tmux_name: 'dev-x' }), visible: true });
    await settle();
    expect(mockedConv).toHaveBeenCalledTimes(1);

    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'run the tests' } });
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();

    expect(mockedSend).toHaveBeenCalledWith('trn', 'dev-x', 'run the tests');
    expect(box.value).toBe('');
    expect(screen.getByTestId('conv-pending').textContent).toContain('run the tests');
    expect(mockedConv).toHaveBeenCalledTimes(2);
  });

  it('the Send button sends too, and a blank prompt never sends', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    const button = screen.getByTestId('conv-composer-send') as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    await fireEvent.input(box, { target: { value: '   ' } });
    await fireEvent.keyDown(box, { key: 'Enter' });
    expect(mockedSend).not.toHaveBeenCalled();
    await fireEvent.input(box, { target: { value: 'hello' } });
    expect(button.disabled).toBe(false);
    await fireEvent.click(button);
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'ctl', 'hello');
  });

  it('Shift+Enter does not send', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'line one' } });
    await fireEvent.keyDown(box, { key: 'Enter', shiftKey: true });
    expect(mockedSend).not.toHaveBeenCalled();
    expect(box.value).toBe('line one');
  });

  it('a failed send shows the error and keeps the text', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedSend.mockResolvedValue({ ok: false, error: { code: 'E_TMUX', message: "can't find session" } });
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'hello' } });
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    expect(screen.getByTestId('conv-composer-error').textContent).toContain("can't find session");
    expect(box.value).toBe('hello');
    expect(screen.queryByTestId('conv-pending')).toBeNull();
  });

  it('the pending turn disappears once the transcript carries the prompt', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'hello' } });
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    expect(screen.getByTestId('conv-pending')).toBeTruthy();

    // the transcript has not caught up yet: pending stays
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await settle();
    expect(screen.getByTestId('conv-pending')).toBeTruthy();

    const caughtUp = conv();
    caughtUp.turns.push({ prompt: 'hello', at: '2026-09-13T10:01:00.000Z', ended_at: null, items: [{ kind: 'text', text: 'hi' }] });
    mockedConv.mockReturnValue(ok(caughtUp));
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await settle();
    expect(screen.queryByTestId('conv-pending')).toBeNull();
    expect(screen.getAllByTestId('conv-prompt')).toHaveLength(2);
  });

  it('bg and external rows get a read-only note instead of a composer', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    const { unmount } = render(ConversationPanel, { session: session({ kind: 'bg', tmux_name: 'bg:abc' }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-composer-input')).toBeNull();
    expect(screen.getByTestId('conv-readonly').textContent).toMatch(/no terminal/i);
    unmount();
    render(ConversationPanel, { session: session({ kind: 'external' }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-composer-input')).toBeNull();
  });

  it('names the session state next to Send while Claude is working or stuck', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    const { unmount } = render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-composer-status').textContent).toMatch(/working/i);
    unmount();
    const second = render(ConversationPanel, { session: session({ claude_status: 'blocked', stuck_kind: 'auth_menu' }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-composer-status').textContent).toMatch(/stuck/i);
    second.unmount();
    render(ConversationPanel, { session: session({ claude_status: 'idle' }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-composer-status')).toBeNull();
  });

  it('the composer is there even before any transcript exists', async () => {
    mockedConv.mockReturnValue(err('E_NO_TRANSCRIPT'));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    expect(screen.getByTestId('conv-empty').textContent).toBe('No conversation yet');
    expect(screen.getByTestId('conv-composer-input')).toBeTruthy();
  });
});

describe('ConversationPanel slash commands', () => {
  async function mountWithDraft(text: string) {
    mockedConv.mockReturnValue(ok(conv()));
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: text } });
    return box;
  }

  it('typing a slash opens the command list, a prefix narrows it, plain text closes it', async () => {
    const box = await mountWithDraft('/');
    expect(screen.getByTestId('conv-slash-menu')).toBeTruthy();
    expect(screen.getAllByTestId('conv-slash-item').length).toBeGreaterThan(5);
    await fireEvent.input(box, { target: { value: '/cle' } });
    const items = screen.getAllByTestId('conv-slash-item');
    expect(items).toHaveLength(1);
    expect(items[0].textContent).toContain('/clear');
    await fireEvent.input(box, { target: { value: 'hello' } });
    expect(screen.queryByTestId('conv-slash-menu')).toBeNull();
  });

  it('Enter on a partial name completes it instead of sending; Enter again sends', async () => {
    const box = await mountWithDraft('/cle');
    await fireEvent.keyDown(box, { key: 'Enter' });
    expect(mockedSend).not.toHaveBeenCalled();
    expect(box.value).toBe('/clear');
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'ctl', '/clear');
  });

  it('Enter on an exact name sends it straight away', async () => {
    const box = await mountWithDraft('/clear');
    expect(screen.getByTestId('conv-slash-menu')).toBeTruthy();
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'ctl', '/clear');
  });

  it('arrows move the highlight and Tab accepts the highlighted command', async () => {
    const box = await mountWithDraft('/co');
    const items = screen.getAllByTestId('conv-slash-item');
    expect(items.length).toBeGreaterThan(1);
    expect(items[0].getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(box, { key: 'ArrowDown' });
    const after = screen.getAllByTestId('conv-slash-item');
    expect(after[0].getAttribute('aria-selected')).toBe('false');
    expect(after[1].getAttribute('aria-selected')).toBe('true');
    const wanted = after[1].textContent ?? '';
    await fireEvent.keyDown(box, { key: 'Tab' });
    expect(wanted).toContain(box.value.trim());
    expect(mockedSend).not.toHaveBeenCalled();
  });

  it('a command that takes arguments completes with a trailing space and the menu closes', async () => {
    const box = await mountWithDraft('/mod');
    await fireEvent.keyDown(box, { key: 'Tab' });
    expect(box.value).toBe('/model ');
    expect(screen.queryByTestId('conv-slash-menu')).toBeNull();
  });

  it('clicking an item accepts it', async () => {
    const box = await mountWithDraft('/cle');
    await fireEvent.click(screen.getAllByTestId('conv-slash-item')[0].querySelector('button')!);
    expect(box.value).toBe('/clear');
  });

  it('Escape hides the menu until the draft changes', async () => {
    const box = await mountWithDraft('/cle');
    await fireEvent.keyDown(box, { key: 'Escape' });
    expect(screen.queryByTestId('conv-slash-menu')).toBeNull();
    // Enter now sends the literal draft rather than completing
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'ctl', '/cle');
  });
});

describe('ConversationPanel quick actions', () => {
  async function mount(over: Partial<SessionRow> = {}) {
    mockedConv.mockReturnValue(ok(conv()));
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(ConversationPanel, { session: session(over), visible: true });
    await settle();
  }

  it('renders one chip per preset; a click fills the box without sending', async () => {
    composerPresets.set([
      { label: 'Clear', text: '/clear' },
      { label: 'Tests', text: 'run the tests' },
    ]);
    await mount();
    const chips = screen.getAllByTestId('conv-chip');
    expect(chips.map((c) => c.textContent?.trim())).toEqual(['Clear', 'Tests']);
    await fireEvent.click(chips[1]);
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    expect(box.value).toBe('run the tests');
    expect(mockedSend).not.toHaveBeenCalled();
    // a command preset does not pop the slash menu over a box it already filled
    await fireEvent.click(chips[0]);
    expect(box.value).toBe('/clear');
    expect(screen.queryByTestId('conv-slash-menu')).toBeNull();
  });

  it('Shift+click sends the preset at once', async () => {
    composerPresets.set([{ label: 'Status', text: '/status' }]);
    await mount({ host_alias: 'trn', tmux_name: 'dev-x' });
    await fireEvent.click(screen.getByTestId('conv-chip'), { shiftKey: true });
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('trn', 'dev-x', '/status');
    expect((screen.getByTestId('conv-composer-input') as HTMLTextAreaElement).value).toBe('');
  });

  it('a session stuck on press_enter gets a chip that sends a bare Enter', async () => {
    await mount({ claude_status: 'blocked', stuck_kind: 'press_enter' });
    await fireEvent.click(screen.getByTestId('conv-chip-enter'));
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'ctl', '');
    expect(screen.queryByTestId('conv-pending')).toBeNull();
  });

  it('no Enter chip otherwise, and no chips at all for a bg row', async () => {
    await mount();
    expect(screen.queryByTestId('conv-chip-enter')).toBeNull();
    expect(screen.getAllByTestId('conv-chip').length).toBeGreaterThan(0);
  });

  it('a bg row shows no chips', async () => {
    await mount({ kind: 'bg', tmux_name: 'bg:abc' });
    expect(screen.queryByTestId('conv-chip')).toBeNull();
  });
});

describe('ConversationPanel live indicator', () => {
  function probe(over: Partial<ActivityProbe> = {}): ActivityProbe {
    return { claude_status: null, current_activity: null, stuck_kind: null, waiting_for: null, spinner: null, ...over };
  }

  it('a working row shows the indicator and polls the pane for the spinner text', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    mockedAct.mockResolvedValue({ ok: true, value: probe({ claude_status: 'working', spinner: 'Cooking… (3s · esc to interrupt)' }) });
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    const ind = screen.getByTestId('conv-indicator');
    expect(ind.getAttribute('data-kind')).toBe('working');
    expect(ind.textContent).toContain('Working…');
    vi.advanceTimersByTime(ACTIVITY_POLL_MS);
    await settle();
    expect(mockedAct).toHaveBeenCalledWith(1);
    expect(screen.getByTestId('conv-indicator').textContent).toContain('Cooking… 3s');
  });

  it('an idle row shows no indicator and does not probe', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session({ claude_status: 'idle' }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-indicator')).toBeNull();
    vi.advanceTimersByTime(ACTIVITY_POLL_MS * 3);
    await settle();
    expect(mockedAct).not.toHaveBeenCalled();
  });

  it('a blocked row shows the banner with the pane detail and Open terminal', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    const onOpenTerminal = vi.fn();
    render(ConversationPanel, {
      session: session({ claude_status: 'blocked', current_activity: 'Do you want to proceed?' }),
      visible: true,
      onOpenTerminal,
    });
    await settle();
    const banner = screen.getByTestId('conv-blocked');
    expect(banner.textContent).toContain('Do you want to proceed?');
    await fireEvent.click(screen.getByTestId('conv-open-terminal'));
    expect(onOpenTerminal).toHaveBeenCalled();
  });

  it('a stuck row shows no indicator (the composer note covers it)', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session({ claude_status: 'blocked', stuck_kind: 'auth_menu' }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-indicator')).toBeNull();
    expect(screen.queryByTestId('conv-blocked')).toBeNull();
  });

  it('after our own send: sent → working (optimistic) → gone once a probe reports idle', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    mockedAct.mockResolvedValue({ ok: true, value: probe({ claude_status: 'working' }) });
    render(ConversationPanel, { session: session({ claude_status: 'idle' }), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'hello' } });
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    expect(screen.getByTestId('conv-indicator').getAttribute('data-kind')).toBe('sent');

    // transcript carries the prompt: pending clears, the session is still working
    const caughtUp = conv();
    caughtUp.turns.push({ prompt: 'hello', at: '2026-09-13T10:01:00.000Z', ended_at: null, items: [] });
    mockedConv.mockReturnValue(ok(caughtUp));
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await settle();
    expect(screen.queryByTestId('conv-pending')).toBeNull();
    expect(screen.getByTestId('conv-indicator').getAttribute('data-kind')).toBe('working');

    mockedAct.mockResolvedValue({ ok: true, value: probe({ claude_status: 'idle' }) });
    vi.advanceTimersByTime(ACTIVITY_POLL_MS);
    await settle();
    expect(screen.queryByTestId('conv-indicator')).toBeNull();
  });

  it('a quiet session re-reads the transcript only after the quiet cadence, or at once when its turn counter moves', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'Date'] });
    mockedConv.mockReturnValue(ok(conv()));
    const { rerender } = render(ConversationPanel, { session: session({ claude_status: 'idle', turn_seq: 3 }), visible: true });
    await settle();
    expect(mockedConv).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await settle();
    expect(mockedConv).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(QUIET_POLL_MS);
    await settle();
    expect(mockedConv).toHaveBeenCalledTimes(2);

    await rerender({ session: session({ claude_status: 'idle', turn_seq: 4 }), visible: true });
    await settle();
    expect(mockedConv).toHaveBeenCalledTimes(3);
  });
});

describe('ConversationPanel tool outcomes', () => {
  it('marks a failed tool line and counts failures in a folded group', async () => {
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            { prompt: 'p', at: null, ended_at: null, items: [{ kind: 'tool', summary: 'Bash(cargo test)', error: true }] },
            {
              prompt: 'q',
              at: null,
              ended_at: null,
              items: [
                { kind: 'tool', summary: 'Read(a)' },
                { kind: 'tool', summary: 'Bash(b)', error: true },
              ],
            },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const tools = screen.getAllByTestId('conv-tool');
    expect(tools[0].getAttribute('data-error')).toBe('true');
    expect(tools[0].getAttribute('title')).toContain('Failed');
    expect(tools[1].getAttribute('data-error')).toBeNull();
    expect(tools[2].getAttribute('data-error')).toBe('true');
    expect(screen.getByTestId('conv-tools').querySelector('summary')?.textContent).toContain('1 failed');
  });
});

describe('ConversationPanel turn duration and open tool group', () => {
  it('shows how long a finished turn took, not for the turn still running', async () => {
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            { prompt: 'a', at: '2026-09-13T10:00:00Z', ended_at: '2026-09-13T10:02:14Z', items: [{ kind: 'text', text: 'done' }] },
            { prompt: 'b', at: '2026-09-13T10:05:00Z', ended_at: '2026-09-13T10:05:30Z', items: [{ kind: 'text', text: 'still going' }] },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    const durations = screen.getAllByTestId('conv-duration');
    expect(durations).toHaveLength(1);
    expect(durations[0].textContent).toContain('2m 14s');
  });

  it('shows the duration on the last turn once the session is quiet', async () => {
    mockedConv.mockReturnValue(
      ok(conv({ turns: [{ prompt: 'a', at: '2026-09-13T10:00:00Z', ended_at: '2026-09-13T10:00:35Z', items: [{ kind: 'text', text: 'x' }] }] })),
    );
    render(ConversationPanel, { session: session({ claude_status: 'idle' }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-duration').textContent).toContain('35s');
  });

  it("keeps the running turn's last tool group open, earlier groups folded", async () => {
    const tools = (n: string) => [
      { kind: 'tool' as const, summary: `${n}1()` },
      { kind: 'tool' as const, summary: `${n}2()` },
    ];
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            { prompt: 'a', at: null, ended_at: null, items: tools('A') },
            { prompt: 'b', at: null, ended_at: null, items: [...tools('B'), { kind: 'text', text: 't' }, ...tools('C')] },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    const groups = screen.getAllByTestId('conv-tools') as HTMLDetailsElement[];
    expect(groups).toHaveLength(3);
    expect(groups[0].open).toBe(false);
    expect(groups[1].open).toBe(false);
    expect(groups[2].open).toBe(true);
  });
});

describe('ConversationPanel drafts and focus', () => {
  it('keeps an unsent draft per session across unmount and session switches', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    const first = render(ConversationPanel, { session: session({ id: 1 }), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'half typed' } });
    first.unmount();

    const second = render(ConversationPanel, { session: session({ id: 2 }), visible: true });
    await settle();
    expect((screen.getByTestId('conv-composer-input') as HTMLTextAreaElement).value).toBe('');
    // a switch must never copy the old text into the new session's slot
    await fireEvent.input(screen.getByTestId('conv-composer-input'), { target: { value: 'for two' } });
    await second.rerender({ session: session({ id: 3 }), visible: true });
    await settle();
    expect(composerDrafts.get(1)).toBe('half typed');
    expect(composerDrafts.get(2)).toBe('for two');
    expect(composerDrafts.has(3)).toBe(false);
    expect((screen.getByTestId('conv-composer-input') as HTMLTextAreaElement).value).toBe('');
    await second.rerender({ session: session({ id: 1 }), visible: true });
    await settle();
    expect((screen.getByTestId('conv-composer-input') as HTMLTextAreaElement).value).toBe('half typed');
  });

  it('sending forgets the stored draft', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(ConversationPanel, { session: session({ id: 7 }), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'go' } });
    expect(composerDrafts.get(7)).toBe('go');
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    expect(composerDrafts.has(7)).toBe(false);
  });

  it('focuses the composer when shown for a promptable session, not when hidden', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    const hidden = render(ConversationPanel, { session: session(), visible: false });
    await settle();
    expect(document.activeElement).not.toBe(screen.getByTestId('conv-composer-input'));
    hidden.unmount();
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    expect(document.activeElement).toBe(screen.getByTestId('conv-composer-input'));
  });
});

describe('ConversationPanel new-item count', () => {
  it('counts items that land while scrolled up and clears on Latest', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const scroller = screen.getByTestId('conv-scroller');
    Object.defineProperty(scroller, 'scrollHeight', { value: 2000, configurable: true });
    Object.defineProperty(scroller, 'clientHeight', { value: 500, configurable: true });
    scroller.scrollTop = 100;
    await fireEvent.scroll(scroller);
    expect(screen.getByTestId('conv-latest').textContent).toContain('Latest');

    const grown = conv();
    grown.turns.push({ prompt: 'more', at: null, ended_at: null, items: [{ kind: 'text', text: 'x' }] });
    mockedConv.mockReturnValue(ok(grown));
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await settle();
    expect(screen.getByTestId('conv-latest').textContent).toContain('2 new');

    await fireEvent.click(screen.getByTestId('conv-latest'));
    expect(screen.queryByTestId('conv-latest')).toBeNull();
    // back at the bottom, the next growth is seen live: no count accrues
    scroller.scrollTop = 100;
    await fireEvent.scroll(scroller);
    expect(screen.getByTestId('conv-latest').textContent).toContain('Latest');
  });
});
