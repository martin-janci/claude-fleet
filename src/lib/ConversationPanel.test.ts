import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('./conversation', async () => {
  const actual = await vi.importActual<typeof import('./conversation')>('./conversation');
  return { ...actual, sessionConversation: vi.fn(), sessionActivity: vi.fn(), listConversations: vi.fn(), toolDetail: vi.fn() };
});
vi.mock('./clipboard', async () => {
  const actual = await vi.importActual<typeof import('./clipboard')>('./clipboard');
  return { ...actual, copyText: vi.fn() };
});
vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return { ...actual, sendPrompt: vi.fn() };
});
vi.mock('./selection', async () => {
  const actual = await vi.importActual<typeof import('./selection')>('./selection');
  return { ...actual, selectSession: vi.fn() };
});
import { sessionConversation, sessionActivity, listConversations, toolDetail, type ConversationSummary, PROMPT_CLAMP_LINES, CONVERSATION_POLL_MS, ACTIVITY_POLL_MS, QUIET_POLL_MS, PROBE_TTL_MS, CONV_MAX_TURNS, type Conversation, type ActivityProbe } from './conversation';
import ConversationPanel from './ConversationPanel.svelte';
import { sendPrompt, sessions, type SessionRow } from './sessions';
import { selectSession } from './selection';
import { tasks, type TaskRow } from './tasks';
import { composerPresets, resetComposerPresets } from './composer_presets';
import { composerDrafts } from './conversation';
import { openPathRequest } from './app_views';
import { dispatchTimelineEvents, dispatchConversationsChanged } from './live_events';
import type { SessionEvent } from './timeline';
import { copyText } from './clipboard';
import { hubStatus, STANDALONE, type HubStatus } from './hub';

const REMOTE: HubStatus = {
  remote: true,
  url: 'https://fleet.example.com',
  client_name: 'laptop',
  client_mode: null,
  configured_url: 'https://fleet.example.com',
  configured_client_name: 'laptop',
  allow_plaintext: false,
  warning: null,
  restart_required: false,
  unavailable: null,
};

const mockedConv = sessionConversation as unknown as ReturnType<typeof vi.fn>;
const mockedSend = sendPrompt as unknown as ReturnType<typeof vi.fn>;
const mockedAct = sessionActivity as unknown as ReturnType<typeof vi.fn>;
const mockedList = listConversations as unknown as ReturnType<typeof vi.fn>;
const mockedDetail = toolDetail as unknown as ReturnType<typeof vi.fn>;
const selectSessionSpy = selectSession as unknown as ReturnType<typeof vi.fn>;

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

/** A `tool` ConvItem with the fields not under test defaulted (a finished
 *  call, so it does not take over the activity indicator's label). */
function tool(summary: string, over: Partial<{ error: boolean; id: string | null; name: string; target: string | null; at: string | null; ended_at: string | null; done: boolean }> = {}) {
  return {
    kind: 'tool' as const,
    summary,
    error: false,
    id: null,
    name: '',
    target: null,
    at: null,
    ended_at: null,
    done: true,
    ...over,
  };
}

function conv(over: Partial<Conversation> = {}): Conversation {
  return {
    truncated: false,
    context: null,
    events: [],
    turns: [
      {
        prompt: 'fix the bug',
        at: '2026-09-13T10:00:00.000Z',
        ended_at: null,
        items: [{ kind: 'text', text: 'looking into it' }, tool('Bash(command=ls -la)', { name: 'Bash', target: 'ls -la' })],
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
  mockedList.mockReset();
  mockedDetail.mockReset();
  selectSessionSpy.mockReset();
  mockedDetail.mockResolvedValue({
    ok: true,
    value: { id: 't1', name: 'Bash', input: '{}', edit: null, command: 'ls', result: 'out', is_error: false },
  });
  mockedList.mockResolvedValue({ ok: true, value: [] });
  mockedAct.mockResolvedValue({ ok: false, error: { code: 'E_INVALID_STATE', message: 'no pane' } });
  composerDrafts.clear();
  resetComposerPresets();
  setVisibility('visible');
  hubStatus.set({ ...STANDALONE });
  sessions.set([]);
  tasks.set([]);
});

afterEach(() => {
  vi.useRealTimers();
  setVisibility('visible');
  hubStatus.set({ ...STANDALONE });
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
    expect(screen.getByTestId('conv-tool').textContent).toContain('Run');
    expect(screen.getByTestId('conv-tool').textContent).toContain('ls -la');
    expect(screen.getByText(/Older turns not shown/)).toBeTruthy();
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
    expect(mockedConv).toHaveBeenLastCalledWith(2, undefined, 'sess-abc');
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
                tool('Read(file_path=a)'),
                tool('Bash(command=ls)'),
                tool('Read(file_path=b)'),
                { kind: 'text', text: 'between' },
                tool('Edit(file_path=c)', { name: 'Edit', target: '/r/src/lib/c.ts' }),
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
    expect(all[3].closest('[data-testid="conv-tools"]')).toBeNull();
    expect(all[3].textContent).toContain('Edit');
    expect(all[3].textContent).toContain('…/lib/c.ts');
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

describe('ConversationPanel composer auto-grow', () => {
  /** jsdom has no layout: scrollHeight is always 0, so the box's content
   *  height has to be stubbed for the grow to have anything to measure. */
  function stubScrollHeight(el: HTMLElement, px: number) {
    Object.defineProperty(el, 'scrollHeight', { configurable: true, get: () => px });
  }

  it('grows the box to fit the draft and shrinks back when it is sent', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;

    stubScrollHeight(box, 180);
    await fireEvent.input(box, { target: { value: 'a\nb\nc\nd\ne\nf' } });
    expect(box.style.height).toBe('180px');

    // Sending empties the draft: the box must come back down, not stay tall.
    stubScrollHeight(box, 42);
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    expect(box.value).toBe('');
    expect(box.style.height).toBe('42px');
  });

  it('leaves the CSS height alone when the content height cannot be measured', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    // scrollHeight is 0 here (no layout); an explicit 0px height would
    // collapse the composer, so nothing must be written.
    await fireEvent.input(box, { target: { value: 'hello' } });
    expect(box.style.height).toBe('');
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

  it('wires the box to the menu so assistive tech follows the highlighted command', async () => {
    const box = await mountWithDraft('/');
    const menu = screen.getByTestId('conv-slash-menu');
    // The listbox must be reachable from the box, and the highlighted option
    // must be the one aria-activedescendant names.
    expect(menu.id).toBeTruthy();
    expect(box.getAttribute('aria-controls')).toBe(menu.id);
    const options = within(menu).getAllByRole('option');
    expect(options[0].id).toBeTruthy();
    expect(box.getAttribute('aria-activedescendant')).toBe(options[0].id);
    expect(options[0].getAttribute('aria-selected')).toBe('true');

    await fireEvent.keyDown(box, { key: 'ArrowDown' });
    expect(box.getAttribute('aria-activedescendant')).toBe(options[1].id);
    expect(options[1].getAttribute('aria-selected')).toBe('true');
    expect(options[0].getAttribute('aria-selected')).toBe('false');

    // Closed again, the box points at nothing.
    await fireEvent.input(box, { target: { value: 'hello' } });
    expect(box.getAttribute('aria-activedescendant')).toBeNull();
    expect(box.getAttribute('aria-controls')).toBeNull();
  });

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
    // aria-selected lives on the option itself (the button), not the li.
    const opts = () => screen.getAllByTestId('conv-slash-item').map((li) => li.querySelector('[role="option"]')!);
    const items = opts();
    expect(items.length).toBeGreaterThan(1);
    expect(items[0].getAttribute('aria-selected')).toBe('true');
    await fireEvent.keyDown(box, { key: 'ArrowDown' });
    const after = opts();
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
    // the indicator goes live with one probe at once, then on the interval
    const ind = screen.getByTestId('conv-indicator');
    expect(ind.getAttribute('data-kind')).toBe('working');
    expect(mockedAct).toHaveBeenCalledTimes(1);
    expect(mockedAct).toHaveBeenCalledWith(1);
    expect(ind.textContent).toContain('Cooking… 3s');
    vi.advanceTimersByTime(ACTIVITY_POLL_MS);
    await settle();
    expect(mockedAct).toHaveBeenCalledTimes(2);
  });

  // #147: session_activity is local-only in remote mode (the hub's pane reads
  // answer a different shape) — a hub client must not poll it every 2s only to
  // drop an E_LOCAL_ONLY each time.
  it('a hub client never polls session_activity, even for a working row', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    hubStatus.set(REMOTE);
    mockedConv.mockReturnValue(ok(conv()));
    mockedAct.mockResolvedValue({ ok: true, value: probe({ claude_status: 'working' }) });
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    vi.advanceTimersByTime(ACTIVITY_POLL_MS * 3);
    await settle();
    expect(mockedAct).not.toHaveBeenCalled();
  });

  it('standalone is untouched: a working row still polls', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    mockedAct.mockResolvedValue({ ok: true, value: probe({ claude_status: 'working' }) });
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    expect(mockedAct).toHaveBeenCalledTimes(1);
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
    // the pane has not classified yet: the send shows as "sent"
    mockedAct.mockResolvedValue({ ok: true, value: probe({ claude_status: null }) });
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
            { prompt: 'p', at: null, ended_at: null, items: [tool('Bash(cargo test)', { error: true })] },
            {
              prompt: 'q',
              at: null,
              ended_at: null,
              items: [tool('Read(a)'), tool('Bash(b)', { error: true })],
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
    const tools = (n: string) => [tool(`${n}1()`), tool(`${n}2()`)];
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

describe('ConversationPanel load older', () => {
  it('asks for a bigger turn window, keeps using it for polls, and resets on a session switch', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv({ truncated: true })));
    const { rerender } = render(ConversationPanel, { session: session(), visible: true });
    await settle();
    expect(mockedConv).toHaveBeenLastCalledWith(1, undefined, 'sess-abc');

    await fireEvent.click(screen.getByTestId('conv-load-older'));
    await settle();
    expect(mockedConv).toHaveBeenLastCalledWith(1, 20, 'sess-abc');
    await fireEvent.click(screen.getByTestId('conv-load-older'));
    await settle();
    expect(mockedConv).toHaveBeenLastCalledWith(1, 30, 'sess-abc');

    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await settle();
    expect(mockedConv).toHaveBeenLastCalledWith(1, 30, 'sess-abc');

    await rerender({ session: session({ id: 2 }), visible: true });
    await settle();
    expect(mockedConv).toHaveBeenLastCalledWith(2, undefined, 'sess-abc');
  });

  it('offers Load older only when the read was truncated', async () => {
    mockedConv.mockReturnValue(ok(conv({ truncated: false })));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-load-older')).toBeNull();
  });
});

describe('ConversationPanel context meter', () => {
  it('shows the context meter and suggests Compact from the warn threshold', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    const { rerender } = render(ConversationPanel, { session: session({ context_pct: 42 }), visible: true });
    await settle();
    const meter = screen.getByTestId('conv-ctx');
    expect(meter.textContent).toContain('42%');
    expect(meter.getAttribute('data-level')).toBe('ok');
    const compact = screen.getAllByTestId('conv-chip').find((c) => c.textContent?.trim() === 'Compact')!;
    expect(compact.getAttribute('data-suggested')).toBeNull();

    await rerender({ session: session({ context_pct: 83 }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-ctx').getAttribute('data-level')).toBe('warn');
    const suggested = screen.getAllByTestId('conv-chip').find((c) => c.textContent?.trim() === 'Compact')!;
    expect(suggested.getAttribute('data-suggested')).toBe('true');
    expect(suggested.getAttribute('title')).toContain('83%');
    // other chips are never suggested
    const clear = screen.getAllByTestId('conv-chip').find((c) => c.textContent?.trim() === 'Clear')!;
    expect(clear.getAttribute('data-suggested')).toBeNull();
  });

  it('no meter when the context usage is unknown', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session({ context_pct: null }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-ctx')).toBeNull();
  });
});

describe('ConversationPanel review-round fixes', () => {
  it('a slash command sends but leaves no pending turn and no optimistic working state', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(ConversationPanel, { session: session({ claude_status: 'idle' }), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: '/status' } });
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'ctl', '/status');
    expect(box.value).toBe('');
    expect(screen.queryByTestId('conv-pending')).toBeNull();
    expect(screen.queryByTestId('conv-indicator')).toBeNull();
  });

  it('a send that resolves after a session switch touches nothing in the new session', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    let resolveSend: (v: { ok: true; value: undefined }) => void = () => {};
    mockedSend.mockReturnValue(new Promise((res) => (resolveSend = res)));
    const { rerender } = render(ConversationPanel, { session: session({ id: 1 }), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'for one' } });
    await fireEvent.keyDown(box, { key: 'Enter' });
    await rerender({ session: session({ id: 2 }), visible: true });
    await settle();
    await fireEvent.input(screen.getByTestId('conv-composer-input'), { target: { value: 'typing in two' } });
    resolveSend({ ok: true, value: undefined });
    await settle();
    expect((screen.getByTestId('conv-composer-input') as HTMLTextAreaElement).value).toBe('typing in two');
    expect(composerDrafts.get(2)).toBe('typing in two');
    expect(screen.queryByTestId('conv-pending')).toBeNull();
    expect(screen.queryByTestId('conv-indicator')).toBeNull();
  });

  it('Load older never inflates the new-item count', async () => {
    mockedConv.mockReturnValue(ok(conv({ truncated: true })));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const scroller = screen.getByTestId('conv-scroller');
    Object.defineProperty(scroller, 'scrollHeight', { value: 2000, configurable: true });
    Object.defineProperty(scroller, 'clientHeight', { value: 500, configurable: true });
    scroller.scrollTop = 0;
    await fireEvent.scroll(scroller);
    const older = conv({ truncated: true });
    older.turns.unshift({ prompt: 'old', at: '2026-09-13T09:00:00Z', ended_at: null, items: [{ kind: 'text', text: 'past' }] });
    mockedConv.mockReturnValue(ok(older));
    await fireEvent.click(screen.getByTestId('conv-load-older'));
    await settle();
    expect(screen.getByTestId('conv-latest').textContent).toContain('Latest');
  });

  it('never stacks probes: a slow probe blocks the next tick, and the indicator goes live with one probe at once', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    mockedAct.mockReturnValue(new Promise(() => {}));
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    expect(mockedAct).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(ACTIVITY_POLL_MS * 3);
    await settle();
    expect(mockedAct).toHaveBeenCalledTimes(1);
  });

  it('a row patch that leaves the status unchanged keeps the live probe', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    mockedAct.mockResolvedValue({ ok: true, value: { claude_status: 'working', current_activity: null, stuck_kind: null, waiting_for: null, spinner: 'Cooking… (3s)' } });
    const { rerender } = render(ConversationPanel, { session: session({ claude_status: 'working', turn_seq: 1 }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-indicator').textContent).toContain('Cooking… 3s');
    await rerender({ session: session({ claude_status: 'working', turn_seq: 1, context_pct: 55 }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-indicator').textContent).toContain('Cooking… 3s');
    await rerender({ session: session({ claude_status: 'idle', turn_seq: 1, context_pct: 55 }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-indicator')).toBeNull();
  });

  it('the WebKit composition Enter (keyCode 229) does not send', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: '日本' } });
    await fireEvent.keyDown(box, { key: 'Enter', keyCode: 229 });
    expect(mockedSend).not.toHaveBeenCalled();
  });
});

describe('ConversationPanel second review-round fixes', () => {
  it('Shift+click on a chip leaves the typed draft alone', async () => {
    composerPresets.set([{ label: 'Continue', text: 'Continue where you left off.' }]);
    mockedConv.mockReturnValue(ok(conv()));
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'my long prompt' } });
    await fireEvent.click(screen.getByTestId('conv-chip'), { shiftKey: true });
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'ctl', 'Continue where you left off.');
    expect(box.value).toBe('my long prompt');
  });

  it('a stale probe expires: the row wins again after the TTL and a new probe is taken', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'setTimeout', 'clearTimeout'] });
    mockedConv.mockReturnValue(ok(conv()));
    mockedAct.mockResolvedValue({ ok: true, value: { claude_status: 'idle', current_activity: null, stuck_kind: null, waiting_for: null, spinner: null } });
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    // the immediate probe said idle: indicator gone, loop stopped
    expect(screen.queryByTestId('conv-indicator')).toBeNull();
    const calls = mockedAct.mock.calls.length;
    vi.advanceTimersByTime(ACTIVITY_POLL_MS * 2);
    await settle();
    expect(mockedAct.mock.calls.length).toBe(calls);
    // after the TTL the row's "working" shows again and the loop resumes
    mockedAct.mockResolvedValue({ ok: true, value: { claude_status: 'working', current_activity: null, stuck_kind: null, waiting_for: null, spinner: 'Cooking… (9s)' } });
    vi.advanceTimersByTime(PROBE_TTL_MS);
    await settle();
    expect(screen.getByTestId('conv-indicator').textContent).toContain('Cooking… 9s');
    expect(mockedAct.mock.calls.length).toBeGreaterThan(calls);
  });

  it('a manually closed running group stays closed on refresh; a manually opened finished group stays open', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    const tools = (n: string, k: number) => Array.from({ length: k }, (_, i) => tool(`${n}${i}()`));
    const base = conv({
      turns: [
        { prompt: 'a', at: '2026-09-13T10:00:00Z', ended_at: null, items: tools('A', 2) },
        { prompt: 'b', at: '2026-09-13T10:05:00Z', ended_at: null, items: tools('B', 2) },
      ],
    });
    mockedConv.mockReturnValue(ok(base));
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    let groups = screen.getAllByTestId('conv-tools') as HTMLDetailsElement[];
    expect(groups[1].open).toBe(true);
    groups[0].open = true; // user opens the finished one
    groups[1].open = false; // user closes the running one
    const grown = conv({ turns: [base.turns[0], { ...base.turns[1], items: tools('B', 3) }] });
    mockedConv.mockReturnValue(ok(grown));
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await settle();
    groups = screen.getAllByTestId('conv-tools') as HTMLDetailsElement[];
    expect(groups[0].open).toBe(true);
    expect(groups[1].open).toBe(false);
  });

  it('Load older disappears once the window reached the backend cap', async () => {
    mockedConv.mockReturnValue(ok(conv({ truncated: true })));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    for (let i = 0; i < CONV_MAX_TURNS / 10 - 1; i++) {
      await fireEvent.click(screen.getByTestId('conv-load-older'));
      await settle();
    }
    expect(mockedConv).toHaveBeenLastCalledWith(1, CONV_MAX_TURNS, 'sess-abc');
    expect(screen.queryByTestId('conv-load-older')).toBeNull();
    expect(screen.getByText(/Older turns not shown/)).toBeTruthy();
  });

  it('a failed read on a quiet session is retried at the normal cadence', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'Date'] });
    mockedConv.mockReturnValueOnce(ok(conv()));
    render(ConversationPanel, { session: session({ claude_status: 'idle' }), visible: true });
    await settle();
    expect(mockedConv).toHaveBeenCalledTimes(1);
    // the quiet-cadence read at 15 s fails
    mockedConv.mockReturnValueOnce(err('E_SSH', 'hiccup'));
    vi.advanceTimersByTime(QUIET_POLL_MS);
    await settle();
    expect(mockedConv).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId('conv-error').textContent).toContain('hiccup');
    // the failure did not restart the quiet clock: the very next tick retries
    mockedConv.mockReturnValue(ok(conv()));
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await settle();
    expect(mockedConv).toHaveBeenCalledTimes(3);
    expect(screen.queryByTestId('conv-error')).toBeNull();
  });

  it('bg rows never probe the pane', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session({ kind: 'bg', tmux_name: 'bg:abc', claude_status: 'working' }), visible: true });
    await settle();
    vi.advanceTimersByTime(ACTIVITY_POLL_MS * 2);
    await settle();
    expect(mockedAct).not.toHaveBeenCalled();
    expect(screen.getByTestId('conv-indicator')).toBeTruthy();
  });
});

describe('ConversationPanel prompt recall', () => {
  async function mountWithHistory() {
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            { prompt: 'first', at: null, ended_at: null, items: [] },
            { prompt: 'second', at: null, ended_at: null, items: [] },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    return screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
  }

  it('ArrowUp in an empty box walks earlier prompts newest first; ArrowDown walks back and restores the box', async () => {
    const box = await mountWithHistory();
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('second');
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('first');
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('first');
    await fireEvent.keyDown(box, { key: 'ArrowDown' });
    expect(box.value).toBe('second');
    await fireEvent.keyDown(box, { key: 'ArrowDown' });
    expect(box.value).toBe('');
  });

  it('a typed draft is never replaced, and editing a recalled prompt ends the walk', async () => {
    const box = await mountWithHistory();
    await fireEvent.input(box, { target: { value: 'typing' } });
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('typing');
    await fireEvent.input(box, { target: { value: '' } });
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('second');
    await fireEvent.input(box, { target: { value: 'second edited' } });
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('second edited');
  });

  it('Enter sends the recalled prompt', async () => {
    mockedSend.mockResolvedValue({ ok: true, value: undefined });
    const box = await mountWithHistory();
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    await fireEvent.keyDown(box, { key: 'Enter' });
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('local', 'ctl', 'second');
    expect(box.value).toBe('');
  });
});

describe('ConversationPanel file paths', () => {
  it('a path in reply text is a button that asks Files to open it for this session', async () => {
    openPathRequest.set(null);
    mockedConv.mockReturnValue(
      ok(conv({ turns: [{ prompt: 'q', at: null, ended_at: null, items: [{ kind: 'text', text: 'Edited `src/lib/foo.ts:42` and docs/x.md, see https://a.b/c.ts' }] }] })),
    );
    render(ConversationPanel, { session: session({ id: 9 }), visible: true });
    await settle();
    const paths = screen.getAllByTestId('md-path');
    expect(paths.map((p) => p.textContent)).toEqual(['src/lib/foo.ts:42', 'docs/x.md']);
    await fireEvent.click(paths[0]);
    expect(get(openPathRequest)).toEqual({ sessionId: 9, path: 'src/lib/foo.ts', line: 42 });
  });
});

describe('ConversationPanel final review fixes', () => {
  it('a chip after a recall ends the walk: arrows no longer replace the chip text', async () => {
    composerPresets.set([{ label: 'Tests', text: 'run the tests' }]);
    mockedConv.mockReturnValue(ok(conv({ turns: [{ prompt: 'earlier', at: null, ended_at: null, items: [] }] })));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('earlier');
    await fireEvent.click(screen.getByTestId('conv-chip'));
    expect(box.value).toBe('run the tests');
    await fireEvent.keyDown(box, { key: 'ArrowDown' });
    expect(box.value).toBe('run the tests');
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('run the tests');
  });

  it('inside a recalled multi-line prompt the arrows move the caret; only the edge lines walk', async () => {
    mockedConv.mockReturnValue(ok(conv({ turns: [{ prompt: 'one', at: null, ended_at: null, items: [] }, { prompt: 'line a\nline b\nline c', at: null, ended_at: null, items: [] }] })));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('line a\nline b\nline c');
    // caret on the middle line: neither arrow is consumed
    box.selectionStart = box.selectionEnd = 8;
    const down = new KeyboardEvent('keydown', { key: 'ArrowDown', cancelable: true, bubbles: true });
    box.dispatchEvent(down);
    expect(down.defaultPrevented).toBe(false);
    expect(box.value).toBe('line a\nline b\nline c');
    // caret on the first line: ArrowUp walks to the older prompt
    box.selectionStart = box.selectionEnd = 2;
    await fireEvent.keyDown(box, { key: 'ArrowUp' });
    expect(box.value).toBe('one');
  });

  it('a path-shaped link label stays a plain link, never a nested button', async () => {
    mockedConv.mockReturnValue(
      ok(conv({ turns: [{ prompt: 'q', at: null, ended_at: null, items: [{ kind: 'text', text: 'see [src/lib/foo.ts](https://example.com/x) and src/lib/bar.ts' }] }] })),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const buttons = screen.getAllByTestId('md-path');
    expect(buttons.map((b) => b.textContent)).toEqual(['src/lib/bar.ts']);
    expect(document.querySelector('a.md-link button')).toBeNull();
  });
});

function summary(over: Partial<ConversationSummary> = {}): ConversationSummary {
  return {
    id: 1, session_id: 1, claude_session_id: 'aaa', transcript_path: null, started_at: 1_789_000_000,
    ended_at: 1_789_000_500, start_source: 'fleet', end_reason: 'clear', model: null, first_prompt: 'earlier ask',
    turns: 3, compactions: 0, current: false, ...over,
  };
}
function listOk(value: ConversationSummary[]) {
  return Promise.resolve({ ok: true as const, value });
}
function event(over: Partial<SessionEvent> = {}): SessionEvent {
  return { id: 1, session_id: 1, at: 0, kind: 'notification', detail: null, claude_session_id: 'sess-abc', ...over };
}
const TURN1_AT = '2026-09-13T10:00:00.000Z';
const TURN1_SECS = Date.parse(TURN1_AT) / 1000;

describe('ConversationPanel conversations', () => {
  it('renders the header with the current conversation and the context meter', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(listOk([summary({ id: 2, claude_session_id: 'sess-abc', current: true, ended_at: null, start_source: 'startup', turns: 1 })]));
    render(ConversationPanel, {
      session: session({ context_pct: 21, context_tokens: 42_000, context_window: 200_000, context_stale: false } as Partial<SessionRow>),
      visible: true,
    });
    await settle();
    expect(mockedList).toHaveBeenCalledWith(1);
    const header = screen.getByTestId('conv-header');
    expect(header.textContent).toContain('Current');
    expect(header.contains(screen.getByTestId('conv-ctx'))).toBe(true);
    expect(screen.getByTestId('conv-ctx').textContent).toContain('42k / 200k · 21%');
    // the composer no longer carries its own meter
    expect(screen.getByTestId('conv-composer').querySelector('[data-testid="conv-ctx"]')).toBeNull();
  });

  it('switches to an earlier conversation read-only and back', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(
      listOk([summary({ id: 2, claude_session_id: 'sess-abc', current: true, ended_at: null, start_source: 'clear', turns: 1 }), summary({ id: 1, claude_session_id: 'aaa' })]),
    );
    const { rerender } = render(ConversationPanel, { session: session({ claude_status: 'idle' }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-viewing-banner')).toBeNull();

    await fireEvent.click(screen.getByTestId('conv-switcher'));
    const earlier = screen.getAllByTestId('conv-switcher-item').find((li) => li.getAttribute('data-current') === 'false')!;
    await fireEvent.click(earlier);
    await settle();
    expect(mockedConv).toHaveBeenLastCalledWith(1, undefined, 'aaa');
    expect(screen.getByTestId('conv-viewing-banner').textContent).toContain('Viewing an earlier conversation');
    expect((screen.getByTestId('conv-composer-input') as HTMLTextAreaElement).disabled).toBe(true);
    expect((screen.getByTestId('conv-composer-send') as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId('conv-composer-status').textContent).toContain('go back to current to send');

    const calls = mockedConv.mock.calls.length;
    vi.advanceTimersByTime(QUIET_POLL_MS * 2);
    await settle();
    expect(mockedConv.mock.calls.length).toBe(calls);
    // a Stop hook on the current conversation does not refetch the earlier one
    await rerender({ session: session({ claude_status: 'idle', turn_seq: 7 }), visible: true });
    await settle();
    expect(mockedConv.mock.calls.length).toBe(calls);

    await fireEvent.click(screen.getByTestId('conv-back-current'));
    await settle();
    expect(mockedConv.mock.calls.length).toBe(calls + 1);
    expect(mockedConv.mock.calls.at(-1)![2]).toBe('sess-abc');
    expect(screen.queryByTestId('conv-viewing-banner')).toBeNull();
    expect((screen.getByTestId('conv-composer-input') as HTMLTextAreaElement).disabled).toBe(false);
  });

  it('follows a /clear: new claude_session_id resets the thread and shows the notice', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(listOk([summary({ id: 1, claude_session_id: 'aaa', current: true, ended_at: null })]));
    const { rerender } = render(ConversationPanel, { session: session({ claude_session_id: 'aaa' }), visible: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: 'half-typed' } });
    expect(screen.queryByTestId('conv-switch-notice')).toBeNull();

    mockedConv.mockClear();
    mockedList.mockReturnValue(
      listOk([summary({ id: 2, claude_session_id: 'bbb', current: true, ended_at: null, start_source: 'clear', turns: 0 }), summary({ id: 1, claude_session_id: 'aaa' })]),
    );
    mockedConv.mockReturnValue(ok(conv({ turns: [{ prompt: 'fresh start', at: null, ended_at: null, items: [] }] })));
    await rerender({ session: session({ claude_session_id: 'bbb' }), visible: true });
    await settle();
    await settle();
    expect(mockedConv).toHaveBeenCalledTimes(1);
    expect(screen.getByText('fresh start')).toBeTruthy();
    expect(screen.queryByText('fix the bug')).toBeNull();
    expect(screen.getByTestId('conv-switch-notice').textContent).toContain('New conversation (/clear)');
    expect((screen.getByTestId('conv-composer-input') as HTMLTextAreaElement).value).toBe('half-typed');

    // View previous opens the earlier conversation read-only
    await fireEvent.click(screen.getByTestId('conv-view-previous'));
    await settle();
    expect(mockedConv).toHaveBeenLastCalledWith(1, undefined, 'aaa');
    expect(screen.getByTestId('conv-viewing-banner')).toBeTruthy();
  });

  it('the switch notice waits for a list that knows the new conversation', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(listOk([summary({ claude_session_id: 'aaa', current: true })]));
    const { rerender } = render(ConversationPanel, { session: session({ claude_session_id: 'aaa' }), visible: true });
    await settle();
    // the reloaded list does not carry bbb yet
    await rerender({ session: session({ claude_session_id: 'bbb' }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-switch-notice')).toBeNull();
    mockedList.mockReturnValue(listOk([summary({ id: 2, claude_session_id: 'bbb', current: true, start_source: 'clear' }), summary({ claude_session_id: 'aaa' })]));
    dispatchConversationsChanged([1]);
    await settle();
    expect(screen.getByTestId('conv-switch-notice').textContent).toContain('New conversation (/clear)');
  });

  it('a malformed conversation list is ignored and the panel still renders', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(Promise.resolve({ ok: true as const, value: null }));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    expect(screen.getByTestId('conv-header')).toBeTruthy();
    expect(screen.getByTestId('conv-prompt').textContent).toContain('fix the bug');
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    expect(screen.queryAllByTestId('conv-switcher-item')).toHaveLength(0);
  });

  it('the switch notice is dismissable', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(listOk([summary({ claude_session_id: 'bbb', current: true, start_source: 'resume' })]));
    const { rerender } = render(ConversationPanel, { session: session({ claude_session_id: 'aaa' }), visible: true });
    await settle();
    await rerender({ session: session({ claude_session_id: 'bbb' }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-switch-notice').textContent).toContain('New conversation (/resume)');
    await fireEvent.click(screen.getByTestId('conv-switch-dismiss'));
    expect(screen.queryByTestId('conv-switch-notice')).toBeNull();
  });

  it('does not show the notice on first mount or on a session switch', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(listOk([summary({ claude_session_id: 'aaa', current: true, start_source: 'clear' })]));
    const { rerender } = render(ConversationPanel, { session: session({ id: 1, claude_session_id: 'aaa' }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-switch-notice')).toBeNull();
    await rerender({ session: session({ id: 2, claude_session_id: 'zzz' }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-switch-notice')).toBeNull();
    // a first id appearing on a session that had none is not a switch either
    await rerender({ session: session({ id: 3, claude_session_id: null }), visible: true });
    await settle();
    await rerender({ session: session({ id: 3, claude_session_id: 'yyy' }), visible: true });
    await settle();
    expect(screen.queryByTestId('conv-switch-notice')).toBeNull();
    expect(mockedConv).toHaveBeenLastCalledWith(3, undefined, 'yyy');
  });

  it('while viewing an earlier conversation, a new id only lights the switcher dot', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(listOk([summary({ id: 2, claude_session_id: 'sess-abc', current: true }), summary({ id: 1, claude_session_id: 'aaa' })]));
    const { rerender } = render(ConversationPanel, { session: session(), visible: true });
    await settle();
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.click(screen.getAllByTestId('conv-switcher-item').find((li) => li.getAttribute('data-current') === 'false')!);
    await settle();
    expect(screen.queryByTestId('conv-switcher-dot')).toBeNull();
    const calls = mockedConv.mock.calls.length;

    await rerender({ session: session({ claude_session_id: 'new-one' }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-switcher-dot')).toBeTruthy();
    expect(screen.getByTestId('conv-viewing-banner')).toBeTruthy();
    expect(screen.queryByTestId('conv-switch-notice')).toBeNull();
    expect(mockedConv.mock.calls.length).toBe(calls);

    await fireEvent.click(screen.getByTestId('conv-back-current'));
    await settle();
    expect(screen.queryByTestId('conv-switcher-dot')).toBeNull();
  });

  it('renders compact, command and interrupt items', async () => {
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            {
              prompt: null,
              at: TURN1_AT,
              ended_at: null,
              items: [
                { kind: 'command', name: '/model', args: 'opus', output: 'Set model to opus' },
                { kind: 'compact', trigger: 'auto', pre_tokens: 180_000, summary: 'S' },
                { kind: 'interrupt', during_tool: true },
              ],
            },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    // a null-prompt turn opening with a command renders no prompt block
    expect(screen.queryByTestId('conv-prompt')).toBeNull();
    const cmd = screen.getByTestId('conv-command');
    expect(cmd.textContent).toContain('/model opus');
    expect(cmd.textContent).toContain('Set model to opus');
    const compact = screen.getByTestId('conv-compact') as HTMLDetailsElement;
    expect(compact.querySelector('summary')!.textContent).toContain('Compacted (auto) · was 180k tokens');
    expect(compact.open).toBe(false);
    expect(screen.getByTestId('conv-interrupt').textContent).toContain('Interrupted during a tool call');
  });

  it('renders a task notification as an event row, never as XML', async () => {
    // A conversation whose only turn carries one notification item.
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            {
              prompt: null,
              at: '2026-09-18T10:01:00Z',
              ended_at: null,
              items: [
                {
                  kind: 'notification',
                  task_id: 'a6',
                  tool_use_id: 'toolu_1',
                  status: 'failed',
                  summary: 'Agent "Posúdiť stratégiu testov" failed',
                  result: null,
                  output_file: '/private/tmp/x/tasks/a6.output',
                  event: null,
                  at: null,
                },
              ],
            },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const row = screen.getByTestId('conv-notification');
    expect(row.textContent).toContain('Agent "Posúdiť stratégiu testov" failed');
    expect(row.getAttribute('data-tone')).toBe('error');
    expect(document.body.textContent).not.toContain('<task-notification>');
  });

  it('clamps a long command output behind Show more; a compact without summary says so', async () => {
    const long = Array.from({ length: 12 }, (_, i) => `line ${i}`).join('\n');
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            {
              prompt: null,
              at: TURN1_AT,
              ended_at: null,
              items: [
                { kind: 'command', name: '/cost', args: null, output: long },
                { kind: 'compact', trigger: null, pre_tokens: null, summary: null },
                { kind: 'interrupt', during_tool: false },
              ],
            },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const cmd = screen.getByTestId('conv-command');
    expect(cmd.querySelector('code')!.textContent).toBe('/cost');
    expect(cmd.querySelector('pre')!.classList.contains('clamped')).toBe(true);
    await fireEvent.click(screen.getByTestId('conv-command-toggle'));
    expect(cmd.querySelector('pre')!.classList.contains('clamped')).toBe(false);
    const compact = screen.getByTestId('conv-compact');
    expect(compact.querySelector('summary')!.textContent!.trim()).toBe('Compacted (unknown)');
    expect(compact.textContent).toContain('Summary not in the loaded tail.');
    expect(screen.getByTestId('conv-interrupt').textContent!.trim()).toBe('Interrupted');
  });

  it('clamps prompts and command output to the line counts the module defines', async () => {
    const longPrompt = Array.from({ length: PROMPT_CLAMP_LINES + 3 }, (_, i) => `p ${i}`).join('\n');
    const longOut = Array.from({ length: 30 }, (_, i) => `line ${i}`).join('\n');
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            {
              prompt: longPrompt,
              at: TURN1_AT,
              ended_at: null,
              items: [{ kind: 'command', name: '/cost', args: null, output: longOut }],
            },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    // The CSS must read its clamp from the module, not carry its own copy:
    // a changed constant that the stylesheet did not follow shows "Show
    // more" over text nothing actually clipped.
    const text = screen.getByTestId('conv-prompt').querySelector('.prompt-text') as HTMLElement;
    expect(text.classList.contains('clamped')).toBe(true);
    expect(text.style.getPropertyValue('--clamp-lines')).toBe(String(PROMPT_CLAMP_LINES));

    const out = screen.getByTestId('conv-command').querySelector('pre') as HTMLElement;
    expect(out.classList.contains('clamped')).toBe(true);
    expect(Number(out.style.getPropertyValue('--clamp-lines'))).toBeGreaterThan(0);
  });

  it('interleaves timeline events and appends pushed ones', async () => {
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            { prompt: 'first ask', at: TURN1_AT, ended_at: null, items: [{ kind: 'text', text: 'a' }] },
            { prompt: 'second ask', at: new Date((TURN1_SECS + 600) * 1000).toISOString(), ended_at: null, items: [{ kind: 'text', text: 'b' }] },
          ],
          events: [event({ id: 5, at: TURN1_SECS + 60, kind: 'stop_failure', detail: 'rate_limit: slow down' })],
        }),
      ),
    );
    render(ConversationPanel, { session: session({ claude_status: 'blocked' }), visible: true });
    await settle();
    const prompts = screen.getAllByTestId('conv-prompt');
    const failed = screen.getByTestId('conv-event');
    expect(failed.textContent).toContain('Turn failed: rate limit');
    expect(failed.textContent).toContain('slow down');
    expect(failed.getAttribute('data-tone')).toBe('error');
    expect(prompts[0].compareDocumentPosition(failed) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(failed.compareDocumentPosition(prompts[1]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    const calls = mockedConv.mock.calls.length;
    dispatchTimelineEvents([event({ id: 9, at: TURN1_SECS + 700, kind: 'notification', detail: 'permission_prompt', claude_session_id: 'other' })]);
    await settle();
    expect(screen.getAllByTestId('conv-event')).toHaveLength(1);

    dispatchTimelineEvents([event({ id: 10, at: TURN1_SECS + 700, kind: 'notification', detail: 'permission_prompt' })]);
    await settle();
    const rows = screen.getAllByTestId('conv-event');
    expect(rows).toHaveLength(2);
    expect(rows[1].textContent).toContain('Waiting for permission');
    expect(rows[1].getAttribute('data-tone')).toBe('warn');
    expect(mockedConv.mock.calls.length).toBe(calls);
  });

  it('refreshes the conversation list on session:conversations', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    const { unmount } = render(ConversationPanel, { session: session(), visible: true });
    await settle();
    expect(mockedList).toHaveBeenCalledTimes(1);
    dispatchConversationsChanged([1]);
    await settle();
    expect(mockedList).toHaveBeenCalledTimes(2);
    dispatchConversationsChanged([2]);
    await settle();
    expect(mockedList).toHaveBeenCalledTimes(2);
    unmount();
    dispatchConversationsChanged([1]);
    await settle();
    expect(mockedList).toHaveBeenCalledTimes(2);
  });

  it('shows "Transcript no longer on host" for a vanished earlier transcript', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(listOk([summary({ id: 2, claude_session_id: 'sess-abc', current: true }), summary({ id: 1, claude_session_id: 'aaa' })]));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    mockedConv.mockReturnValue(err('E_NO_TRANSCRIPT'));
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.click(screen.getAllByTestId('conv-switcher-item').find((li) => li.getAttribute('data-current') === 'false')!);
    await settle();
    expect(screen.getByTestId('conv-empty').textContent).toBe('Transcript no longer on host');
    mockedConv.mockReturnValue(ok(conv()));
    await fireEvent.click(screen.getByTestId('conv-back-current'));
    await settle();
    expect(screen.getByTestId('conv-prompt').textContent).toContain('fix the bug');
  });
});

describe('ConversationPanel final phase-2 review fixes', () => {
  const turn = (text?: string) =>
    [{ prompt: 'first ask', at: TURN1_AT, ended_at: null, items: text ? [{ kind: 'text' as const, text }] : [] }];

  it('a pushed event carried by a later read follows the backend from then on', async () => {
    const ev10 = event({ id: 10, at: TURN1_SECS + 60, kind: 'notification', detail: 'permission_prompt' });
    mockedConv.mockReturnValue(ok(conv({ turns: turn() })));
    const { rerender } = render(ConversationPanel, { session: session({ turn_seq: 1 }), visible: true });
    await settle();
    dispatchTimelineEvents([ev10]);
    await settle();
    expect(screen.getAllByTestId('conv-event')).toHaveLength(1);
    // the next read carries it: the pushed copy is dropped
    mockedConv.mockReturnValue(ok(conv({ turns: turn('a'), events: [ev10] })));
    await rerender({ session: session({ turn_seq: 2 }), visible: true });
    await settle();
    expect(screen.getAllByTestId('conv-event')).toHaveLength(1);
    // a later read no longer carries it: nothing keeps it alive
    mockedConv.mockReturnValue(ok(conv({ turns: turn('b'), events: [] })));
    await rerender({ session: session({ turn_seq: 3 }), visible: true });
    await settle();
    expect(screen.queryAllByTestId('conv-event')).toHaveLength(0);
  });

  it('going back to the current conversation drops the old probe reading', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(listOk([summary({ id: 2, claude_session_id: 'sess-abc', current: true }), summary({ id: 1, claude_session_id: 'aaa' })]));
    mockedAct.mockResolvedValue({ ok: true, value: { claude_status: 'working', current_activity: null, stuck_kind: null, waiting_for: null, spinner: 'Cooking… (3s)' } });
    render(ConversationPanel, { session: session({ claude_status: 'working', turn_seq: 1 }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-indicator').textContent).toContain('Cooking… 3s');
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.click(screen.getAllByTestId('conv-switcher-item').find((li) => li.getAttribute('data-current') === 'false')!);
    await settle();
    // the next probe hangs: only the old reading could show a spinner
    mockedAct.mockReturnValue(new Promise(() => {}));
    await fireEvent.click(screen.getByTestId('conv-back-current'));
    await settle();
    expect(screen.getByTestId('conv-indicator').textContent).not.toContain('Cooking');
  });

  it('an inline event carries a machine-readable time', async () => {
    mockedConv.mockReturnValue(
      ok(conv({ turns: turn('a'), events: [event({ id: 5, at: TURN1_SECS + 60, kind: 'stop_failure', detail: 'rate_limit: x' })] })),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const time = screen.getByTestId('conv-event').querySelector('time')!;
    expect(time.getAttribute('datetime')).toBe(new Date((TURN1_SECS + 60) * 1000).toISOString());
  });

  it('a switch with an unknown source says just "New conversation"', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(listOk([summary({ id: 2, claude_session_id: 'bbb', current: true, start_source: 'unknown' }), summary({ claude_session_id: 'aaa' })]));
    const { rerender } = render(ConversationPanel, { session: session({ claude_session_id: 'aaa' }), visible: true });
    await settle();
    await rerender({ session: session({ claude_session_id: 'bbb' }), visible: true });
    await settle();
    const notice = screen.getByTestId('conv-switch-notice').textContent!;
    expect(notice).toContain('New conversation');
    expect(notice).not.toContain('(');
  });
});

describe('ConversationPanel detail UX', () => {
  it('tool groups render ToolLine rows', async () => {
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            {
              prompt: 'q',
              at: null,
              ended_at: null,
              items: [
                tool('Read(/r/a/b/c.rs)', { id: 't1', name: 'Read', target: '/r/a/b/c.rs', done: true }),
                tool('Bash(cargo test)', { id: 't2', name: 'Bash', target: 'cargo test', done: true, at: '2026-09-18T09:00:00Z', ended_at: '2026-09-18T09:00:12Z' }),
                { kind: 'text', text: 'between' },
                tool('Grep(foo)', { id: null, name: 'Grep', target: 'foo', done: true }),
              ],
            },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const group = screen.getByTestId('conv-tools');
    expect(group.querySelector('summary')?.textContent).toContain('2 tool calls · Read, Bash');
    const rows = screen.getAllByTestId('conv-tool');
    expect(rows).toHaveLength(3);
    expect(rows[0].tagName).toBe('BUTTON');
    expect(rows[0].textContent).toContain('Read');
    expect(rows[0].textContent).toContain('…/b/c.rs');
    expect(rows[1].textContent).toContain('Run');
    expect(rows[1].textContent).toContain('12s');
    expect(rows[2].closest('[data-testid="conv-tools"]')).toBeNull();
    expect(rows[2].tagName).toBe('DIV');
    expect(rows[2].textContent).toContain('Search');
  });

  it('a subagent renders as a block', async () => {
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            {
              prompt: 'q',
              at: null,
              ended_at: null,
              items: [
                {
                  kind: 'subagent',
                  id: 'toolu_9',
                  name: 'Task',
                  agent_type: 'Explore',
                  description: 'Map the store',
                  result: 'Found it.',
                  error: false,
                  at: '2026-09-18T09:00:00Z',
                  ended_at: '2026-09-18T09:02:00Z',
                  done: true,
                },
              ],
            },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const block = screen.getByTestId('conv-subagent');
    expect(block.textContent).toContain('Explore');
    expect(block.textContent).toContain('Map the store');
    expect(block.textContent).toContain('2m 00s');
    expect(block.textContent).toContain('Found it.');
  });

  it('the indicator shows what is running', async () => {
    const at = new Date(Date.now() - 3_000).toISOString();
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            {
              prompt: 'q',
              at,
              ended_at: null,
              items: [
                tool('Read(/a)', { id: 't1', name: 'Read', target: '/a', done: true, at, ended_at: at }),
                tool('Bash(cargo test)', { id: 't2', name: 'Bash', target: 'cargo test', done: false, at }),
              ],
            },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    const ind = screen.getByTestId('conv-indicator');
    expect(ind.textContent).toContain('Run cargo test');
    expect(ind.textContent).toMatch(/Run cargo test · \d+s/);
  });

  it('the running timer ticks every second while something runs', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'Date'] });
    vi.setSystemTime(Date.parse('2026-09-18T09:00:05Z'));
    const running = conv({
      turns: [
        {
          prompt: 'q',
          at: '2026-09-18T09:00:00Z',
          ended_at: null,
          items: [tool('Bash(cargo test)', { id: 't2', name: 'Bash', target: 'cargo test', done: false, at: '2026-09-18T09:00:00Z' })],
        },
      ],
    });
    mockedConv.mockReturnValue(ok(running));
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-indicator').textContent).toContain('Run cargo test · 5s');
    vi.advanceTimersByTime(1_000);
    await settle();
    expect(screen.getByTestId('conv-indicator').textContent).toContain('Run cargo test · 6s');
    expect(screen.getByTestId('conv-tool').textContent).toContain('running 6s');
  });

  it('the indicator keeps the spinner label when nothing is running', async () => {
    mockedAct.mockResolvedValue({ ok: true, value: { claude_status: 'working', current_activity: null, stuck_kind: null, waiting_for: null, spinner: 'Cooking… (3s)' } });
    mockedConv.mockReturnValue(
      ok(conv({ turns: [{ prompt: 'q', at: null, ended_at: null, items: [tool('Bash(ls)', { id: 't1', name: 'Bash', target: 'ls', done: true })] }] })),
    );
    render(ConversationPanel, { session: session({ claude_status: 'working', turn_seq: 1 }), visible: true });
    await settle();
    await settle();
    expect(screen.getByTestId('conv-indicator').textContent).toContain('Cooking… 3s');
  });
});

describe('ConversationPanel find, copy and turn index', () => {
  const mockedCopy = copyText as unknown as ReturnType<typeof vi.fn>;
  let scrolled: Element[];
  const realScroll = Element.prototype.scrollIntoView;

  beforeEach(() => {
    scrolled = [];
    mockedCopy.mockReset();
    mockedCopy.mockResolvedValue(true);
    Element.prototype.scrollIntoView = vi.fn(function (this: Element) {
      scrolled.push(this);
    });
  });
  afterEach(() => {
    Element.prototype.scrollIntoView = realScroll;
  });

  function threeTurns() {
    return conv({
      turns: [
        { prompt: 'Fix the parser', at: '2026-09-18T09:00:00Z', ended_at: null, items: [{ kind: 'text', text: 'on it' }] },
        { prompt: 'and the lexer', at: '2026-09-18T09:05:00Z', ended_at: null, items: [{ kind: 'text', text: 'the PARSER calls it' }] },
        { prompt: null, at: '2026-09-18T09:06:00Z', ended_at: null, items: [{ kind: 'command', name: '/model', args: 'opus', output: null }] },
      ],
    });
  }

  it('Ctrl+F opens find; typing marks matches; Enter moves to the next; Escape closes', async () => {
    mockedConv.mockReturnValue(ok(threeTurns()));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();

    // Outside the panel the shortcut is left alone.
    const outside = new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true, cancelable: true });
    document.body.dispatchEvent(outside);
    await settle();
    expect(outside.defaultPrevented).toBe(false);
    expect(screen.queryByTestId('conv-find')).toBeNull();

    const inside = new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true, cancelable: true });
    screen.getByTestId('conv-scroller').dispatchEvent(inside);
    await settle();
    expect(inside.defaultPrevented).toBe(true);
    expect(screen.getByTestId('conv-find')).toBeTruthy();
    const input = screen.getByTestId('conv-find-input') as HTMLInputElement;
    expect(document.activeElement).toBe(input);

    await fireEvent.input(input, { target: { value: 'parser' } });
    await settle();
    const marked = Array.from(document.querySelectorAll('[data-match]')).map((el) => el.getAttribute('data-row-key'));
    expect(marked).toEqual(['t0', 't1']);
    expect(screen.getByTestId('conv-find-count').textContent).toBe('1 / 2');
    const current = () => document.querySelector('[data-current-match]');
    expect(current()?.getAttribute('data-row-key')).toBe('t0');
    expect(scrolled.at(-1)).toBe(current());
    expect(Element.prototype.scrollIntoView).toHaveBeenLastCalledWith({ block: 'center' });

    await fireEvent.keyDown(input, { key: 'Enter' });
    await settle();
    expect(current()?.getAttribute('data-row-key')).toBe('t1');
    expect(screen.getByTestId('conv-find-count').textContent).toBe('2 / 2');
    expect(scrolled.at(-1)).toBe(current());

    await fireEvent.keyDown(input, { key: 'Enter', shiftKey: true });
    await settle();
    expect(current()?.getAttribute('data-row-key')).toBe('t0');

    await fireEvent.click(screen.getByTestId('conv-find-next'));
    await settle();
    expect(current()?.getAttribute('data-row-key')).toBe('t1');
    await fireEvent.click(screen.getByTestId('conv-find-prev'));
    await settle();
    expect(current()?.getAttribute('data-row-key')).toBe('t0');

    await fireEvent.keyDown(input, { key: 'Escape' });
    await settle();
    expect(screen.queryByTestId('conv-find')).toBeNull();
    expect(document.querySelector('[data-match]')).toBeNull();
    expect(current()).toBeNull();
    expect(screen.getByTestId('conv-turns-button')).toBeTruthy();
  });

  it('paints matches with the Custom Highlight API when it exists', async () => {
    const g = globalThis as unknown as { CSS?: unknown; Highlight?: unknown };
    const savedCss = g.CSS;
    const savedHl = g.Highlight;
    const registry = new Map<string, { ranges: Range[] }>();
    g.CSS = { highlights: registry };
    g.Highlight = class {
      ranges: Range[];
      constructor(...ranges: Range[]) {
        this.ranges = ranges;
      }
    };
    try {
      mockedConv.mockReturnValue(ok(threeTurns()));
      render(ConversationPanel, { session: session(), visible: true });
      await settle();
      await fireEvent.keyDown(screen.getByTestId('conv-scroller'), { key: 'f', ctrlKey: true });
      await settle();
      await fireEvent.input(screen.getByTestId('conv-find-input'), { target: { value: 'parser' } });
      await settle();
      const names = [...registry.keys()];
      const allName = names.find((n) => /^conv-find-\d+$/.test(n))!;
      const curName = names.find((n) => /^conv-find-current-\d+$/.test(n))!;
      const all = registry.get(allName)!.ranges.map((r) => r.toString().toLowerCase());
      expect(all).toEqual(['parser', 'parser']);
      expect(registry.get(curName)!.ranges).toHaveLength(1);
      // Control labels (the Copy buttons, times) are not painted.
      await fireEvent.input(screen.getByTestId('conv-find-input'), { target: { value: 'copy' } });
      await settle();
      expect(registry.get(allName)?.ranges ?? []).toHaveLength(0);
      await fireEvent.click(screen.getByTestId('conv-find-close'));
      await settle();
      expect(registry.size).toBe(0);
    } finally {
      g.CSS = savedCss;
      g.Highlight = savedHl;
    }
  });

  it('on macOS find is Cmd+F; Ctrl+F in the composer keeps its caret meaning', async () => {
    mockedConv.mockReturnValue(ok(threeTurns()));
    render(ConversationPanel, { session: session(), visible: true, isMac: true });
    await settle();
    const box = screen.getByTestId('conv-composer-input');
    const ctrl = new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true, cancelable: true });
    box.dispatchEvent(ctrl);
    await settle();
    expect(ctrl.defaultPrevented).toBe(false);
    expect(screen.queryByTestId('conv-find')).toBeNull();
    const cmd = new KeyboardEvent('keydown', { key: 'f', metaKey: true, bubbles: true, cancelable: true });
    box.dispatchEvent(cmd);
    await settle();
    expect(cmd.defaultPrevented).toBe(true);
    expect(screen.getByTestId('conv-find')).toBeTruthy();
  });

  it('elsewhere find is Ctrl+F, not Cmd+F', async () => {
    mockedConv.mockReturnValue(ok(threeTurns()));
    render(ConversationPanel, { session: session(), visible: true, isMac: false });
    await settle();
    const cmd = new KeyboardEvent('keydown', { key: 'f', metaKey: true, bubbles: true, cancelable: true });
    screen.getByTestId('conv-scroller').dispatchEvent(cmd);
    await settle();
    expect(cmd.defaultPrevented).toBe(false);
    expect(screen.queryByTestId('conv-find')).toBeNull();
  });

  it('the shortcut does nothing while there is no thread on screen', async () => {
    mockedConv.mockReturnValue(err('E_NO_TRANSCRIPT'));
    render(ConversationPanel, { session: session(), visible: true, isMac: false });
    await settle();
    expect(screen.getByTestId('conv-empty')).toBeTruthy();
    const ev = new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true, cancelable: true });
    screen.getByTestId('conv-composer-input').dispatchEvent(ev);
    await settle();
    expect(ev.defaultPrevented).toBe(false);
    expect(screen.queryByTestId('conv-find')).toBeNull();
  });

  it('while blocked on a prompt the last turn\'s pending tool call keeps running, not "no result"', async () => {
    const at = new Date(Date.now() - 3_000).toISOString();
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            { prompt: 'a', at, ended_at: null, items: [tool('Bash(sleep)', { id: 't1', name: 'Bash', target: 'sleep', done: false, at })] },
            { prompt: 'b', at, ended_at: null, items: [tool('Bash(rm x)', { id: 't2', name: 'Bash', target: 'rm x', done: false, at })] },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session({ claude_status: 'blocked' }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-blocked')).toBeTruthy();
    const rows = screen.getAllByTestId('conv-tool');
    expect(rows[0].textContent).toContain('no result');
    expect(rows[1].textContent).toMatch(/running \d+s/);
    expect(rows[1].textContent).not.toContain('no result');
  });

  it('the find bar closes with its close button', async () => {
    mockedConv.mockReturnValue(ok(threeTurns()));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    await fireEvent.keyDown(screen.getByTestId('conv-composer-input'), { key: 'f', ctrlKey: true });
    await settle();
    await fireEvent.input(screen.getByTestId('conv-find-input'), { target: { value: 'nothing like this' } });
    await settle();
    expect(screen.getByTestId('conv-find-count').textContent).toBe('0 / 0');
    await fireEvent.click(screen.getByTestId('conv-find-close'));
    await settle();
    expect(screen.queryByTestId('conv-find')).toBeNull();
  });

  it('copy on a prompt calls copyText with the prompt', async () => {
    mockedConv.mockReturnValue(ok(threeTurns()));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const prompt = screen.getAllByTestId('conv-prompt')[1];
    const btn = prompt.querySelector('[data-testid="conv-copy"]') as HTMLButtonElement;
    expect(btn.getAttribute('aria-label')).toBe('Copy prompt');
    await fireEvent.click(btn);
    await settle();
    expect(mockedCopy).toHaveBeenCalledWith('and the lexer');
    expect(btn.textContent).toContain('Copied');
  });

  it('copy on a text group copies its markdown source', async () => {
    mockedConv.mockReturnValue(ok(conv({ turns: [{ prompt: 'q', at: null, ended_at: null, items: [{ kind: 'text', text: 'see **this**' }] }] })));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const btn = screen.getByTestId('conv-text').querySelector('[data-testid="conv-copy"]') as HTMLButtonElement;
    expect(btn.getAttribute('aria-label')).toBe('Copy reply');
    await fireEvent.click(btn);
    await settle();
    expect(mockedCopy).toHaveBeenCalledWith('see **this**');
  });

  it('the turn index lists prompts and jumps', async () => {
    mockedConv.mockReturnValue(ok(threeTurns()));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const button = screen.getByTestId('conv-turns-button');
    expect(button.textContent).toContain('3 turns');
    await fireEvent.click(button);
    await settle();
    const items = screen.getAllByTestId('conv-turn-index-item');
    expect(items.map((i) => i.textContent)).toEqual([
      expect.stringContaining('Fix the parser'),
      expect.stringContaining('and the lexer'),
      expect.stringContaining('/model opus'),
    ]);
    scrolled = [];
    await fireEvent.click(items[1]);
    await settle();
    expect(scrolled).toHaveLength(1);
    expect(scrolled[0].getAttribute('data-row-key')).toBe('t1');
    expect(screen.queryByTestId('conv-turn-index')).toBeNull();

    // Escape and an outside pointerdown close it too.
    await fireEvent.click(button);
    await settle();
    await fireEvent.keyDown(screen.getByTestId('conv-turn-index'), { key: 'Escape' });
    await settle();
    expect(screen.queryByTestId('conv-turn-index')).toBeNull();
    await fireEvent.click(button);
    await settle();
    await fireEvent.pointerDown(document.body);
    await settle();
    expect(screen.queryByTestId('conv-turn-index')).toBeNull();
  });

  it('the empty state says what to do about it', async () => {
    mockedConv.mockReturnValue(err('E_NO_TRANSCRIPT'));
    const { unmount } = render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const state = screen.getByTestId('conv-empty-state');
    expect(within(state).getByTestId('conv-empty').textContent).toBe('No conversation yet');
    expect(state.textContent).toContain('Send a prompt below');
    unmount();

    // A session with no pane has no composer to point at.
    mockedConv.mockReturnValue(err('E_NO_TRANSCRIPT'));
    render(ConversationPanel, { session: session({ kind: 'bg' }), visible: true });
    await settle();
    const ro = screen.getByTestId('conv-empty-state');
    expect(ro.textContent).not.toContain('Send a prompt below');
    expect(ro.textContent).toContain('outside tmux');
  });

  it('the transcript is a named region a keyboard can reach and scroll', async () => {
    mockedConv.mockReturnValue(ok(threeTurns()));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    const scroller = screen.getByTestId('conv-scroller');
    // A scrollable region that is not in the tab order cannot be scrolled
    // without a pointer; and once it is reachable it needs a name.
    expect(scroller.getAttribute('tabindex')).toBe('0');
    expect(scroller.getAttribute('role')).toBe('region');
    expect(scroller.getAttribute('aria-label')).toBeTruthy();
  });

  it('Escape closes find from anywhere in the panel, but not over a menu that handled it', async () => {
    mockedConv.mockReturnValue(ok(threeTurns()));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    await fireEvent.click(screen.getByTestId('conv-find-button'));
    await settle();
    expect(screen.getByTestId('conv-find')).toBeTruthy();

    // Focus has moved into the thread (clicking a match); Escape must still
    // dismiss the bar rather than leaving it stranded.
    await fireEvent.keyDown(screen.getByTestId('conv-scroller'), { key: 'Escape' });
    await settle();
    expect(screen.queryByTestId('conv-find')).toBeNull();

    // The slash menu handles its own Escape: that must not also close find.
    await fireEvent.click(screen.getByTestId('conv-find-button'));
    await settle();
    const box = screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box, { target: { value: '/' } });
    expect(screen.getByTestId('conv-slash-menu')).toBeTruthy();
    await fireEvent.keyDown(box, { key: 'Escape' });
    await settle();
    expect(screen.queryByTestId('conv-slash-menu')).toBeNull();
    expect(screen.getByTestId('conv-find')).toBeTruthy();
  });

  it('the turn index walks with the arrow keys and Home/End', async () => {
    mockedConv.mockReturnValue(ok(threeTurns()));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    await fireEvent.click(screen.getByTestId('conv-turns-button'));
    await settle();
    const list = screen.getByTestId('conv-turn-index');
    const items = screen.getAllByTestId('conv-turn-index-item');
    expect(document.activeElement).toBe(items[0]);

    await fireEvent.keyDown(list, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items[1]);
    await fireEvent.keyDown(list, { key: 'End' });
    expect(document.activeElement).toBe(items[2]);
    // The ends hold rather than wrap.
    await fireEvent.keyDown(list, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items[2]);
    await fireEvent.keyDown(list, { key: 'Home' });
    expect(document.activeElement).toBe(items[0]);
    await fireEvent.keyDown(list, { key: 'ArrowUp' });
    expect(document.activeElement).toBe(items[0]);
  });

  it('an unfinished tool call in an earlier turn shows "no result"; the running turn counts up', async () => {
    const at = new Date(Date.now() - 3_000).toISOString();
    mockedConv.mockReturnValue(
      ok(
        conv({
          turns: [
            { prompt: 'a', at, ended_at: null, items: [tool('Bash(sleep)', { id: 't1', name: 'Bash', target: 'sleep', done: false, at })] },
            { prompt: 'b', at, ended_at: null, items: [tool('Bash(make)', { id: 't2', name: 'Bash', target: 'make', done: false, at })] },
          ],
        }),
      ),
    );
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    const rows = screen.getAllByTestId('conv-tool');
    expect(rows[0].textContent).toContain('no result');
    expect(rows[1].textContent).toMatch(/running \d+s/);
  });
});

describe('ConversationPanel detail UX fixes', () => {
  const AT = '2026-09-18T09:00:00Z';
  function withTools(tools: ReturnType<typeof tool>[]): Conversation {
    return conv({ turns: [{ prompt: 'q', at: AT, ended_at: null, items: [{ kind: 'text', text: 'on it' }, ...tools] }] });
  }

  it('the view names its conversation when read, and tool detail is read from that same one', async () => {
    // The backend's row can already be on a newer conversation this row has
    // not seen: the read names the one the row shows, and so does detail.
    mockedConv.mockReturnValue(ok(withTools([tool('Bash(ls)', { id: 't1', name: 'Bash', target: 'ls' })])));
    render(ConversationPanel, { session: session({ claude_session_id: 'conv-A' }), visible: true });
    await settle();
    expect(mockedConv).toHaveBeenLastCalledWith(1, undefined, 'conv-A');
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(mockedDetail).toHaveBeenCalledWith(1, 't1', 'conv-A');
  });

  it('after /clear the tool lines of the new conversation read from the new id', async () => {
    mockedConv.mockReturnValue(ok(withTools([tool('Bash(ls)', { id: 't1', name: 'Bash', target: 'ls' })])));
    const { rerender } = render(ConversationPanel, { session: session({ claude_session_id: 'conv-A' }), visible: true });
    await settle();
    mockedConv.mockReturnValue(ok(withTools([tool('Read(/b)', { id: 't9', name: 'Read', target: '/b' })])));
    await rerender({ session: session({ claude_session_id: 'conv-B' }), visible: true });
    await settle();
    expect(mockedConv).toHaveBeenLastCalledWith(1, undefined, 'conv-B');
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(mockedDetail).toHaveBeenLastCalledWith(1, 't9', 'conv-B');
  });

  it('tool detail of an earlier conversation is read from that conversation', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    mockedList.mockReturnValue(listOk([summary({ id: 2, claude_session_id: 'sess-abc', current: true }), summary({ id: 1, claude_session_id: 'aaa' })]));
    render(ConversationPanel, { session: session(), visible: true });
    await settle();
    mockedConv.mockReturnValue(ok(withTools([tool('Bash(ls)', { id: 'old1', name: 'Bash', target: 'ls' })])));
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.click(screen.getAllByTestId('conv-switcher-item').find((li) => li.getAttribute('data-current') === 'false')!);
    await settle();
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(mockedDetail).toHaveBeenCalledWith(1, 'old1', 'aaa');
  });

  it('while viewing an earlier conversation the doing-now label is not shown', async () => {
    const running = withTools([tool('Bash(cargo test)', { id: 't2', name: 'Bash', target: 'cargo test', done: false, at: AT })]);
    mockedConv.mockReturnValue(ok(running));
    mockedList.mockReturnValue(listOk([summary({ id: 2, claude_session_id: 'sess-abc', current: true }), summary({ id: 1, claude_session_id: 'aaa' })]));
    render(ConversationPanel, { session: session({ claude_status: 'working' }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-indicator').textContent).toContain('Run cargo test');
    // The earlier conversation also ends in an unfinished call.
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.click(screen.getAllByTestId('conv-switcher-item').find((li) => li.getAttribute('data-current') === 'false')!);
    await settle();
    expect(screen.queryByTestId('conv-indicator')).toBeNull();
    expect(document.body.textContent).not.toMatch(/Run cargo test ·/);
    expect(screen.getByTestId('conv-tool').textContent).toContain('no result');
  });

  it('an open tool line stays open when a second call joins its group', async () => {
    mockedConv.mockReturnValue(ok(withTools([tool('Bash(ls)', { id: 't1', name: 'Bash', target: 'ls' })])));
    const { rerender } = render(ConversationPanel, { session: session({ turn_seq: 1 }), visible: true });
    await settle();
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(screen.getByTestId('conv-tool-detail')).toBeTruthy();
    expect(mockedDetail).toHaveBeenCalledTimes(1);

    mockedConv.mockReturnValue(
      ok(withTools([tool('Bash(ls)', { id: 't1', name: 'Bash', target: 'ls' }), tool('Read(/a)', { id: 't2', name: 'Read', target: '/a' })])),
    );
    await rerender({ session: session({ turn_seq: 2 }), visible: true });
    await settle();
    await settle();
    const rows = screen.getAllByTestId('conv-tool');
    expect(rows).toHaveLength(2);
    expect(screen.getByTestId('conv-tools').querySelectorAll('[data-testid="conv-tool"]')).toHaveLength(2);
    expect(rows[0].getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByTestId('conv-tool-detail')).toBeTruthy();
    expect((screen.getByTestId('conv-tools') as HTMLDetailsElement).open).toBe(true);
    expect(mockedDetail).toHaveBeenCalledTimes(1);
  });

  it('a pending call keeps a 1 s clock while Claude is blocked on the terminal', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'Date'] });
    vi.setSystemTime(Date.parse('2026-09-18T09:00:05Z'));
    mockedConv.mockReturnValue(ok(withTools([tool('Bash(rm x)', { id: 't1', name: 'Bash', target: 'rm x', done: false, at: AT })])));
    render(ConversationPanel, { session: session({ claude_status: 'blocked' }), visible: true });
    await settle();
    expect(screen.getByTestId('conv-blocked')).toBeTruthy();
    expect(screen.getByTestId('conv-tool').textContent).toContain('running 5s');
    vi.advanceTimersByTime(1_000);
    await settle();
    expect(screen.getByTestId('conv-tool').textContent).toContain('running 6s');
  });

  it('switching conversation closes find and clears its marks', async () => {
    const g = globalThis as unknown as { CSS?: unknown; Highlight?: unknown };
    const savedCss = g.CSS;
    const savedHl = g.Highlight;
    const registry = new Map<string, unknown>();
    g.CSS = { highlights: registry };
    g.Highlight = class {};
    try {
      mockedConv.mockReturnValue(ok(conv()));
      mockedList.mockReturnValue(listOk([summary({ id: 2, claude_session_id: 'sess-abc', current: true }), summary({ id: 1, claude_session_id: 'aaa' })]));
      render(ConversationPanel, { session: session(), visible: true });
      await settle();
      await fireEvent.keyDown(screen.getByTestId('conv-scroller'), { key: 'f', ctrlKey: true });
      await settle();
      await fireEvent.input(screen.getByTestId('conv-find-input'), { target: { value: 'bug' } });
      await settle();
      expect(document.querySelector('[data-match]')).toBeTruthy();
      expect(registry.size).toBe(2);
      await fireEvent.click(screen.getByTestId('conv-switcher'));
      await fireEvent.click(screen.getAllByTestId('conv-switcher-item').find((li) => li.getAttribute('data-current') === 'false')!);
      await settle();
      expect(screen.queryByTestId('conv-find')).toBeNull();
      expect(document.querySelector('[data-match]')).toBeNull();
      expect(registry.size).toBe(0);
    } finally {
      g.CSS = savedCss;
      g.Highlight = savedHl;
    }
  });

  it('two panels keep their own find highlights; unmounting one clears only its own', async () => {
    const g = globalThis as unknown as { CSS?: unknown; Highlight?: unknown };
    const savedCss = g.CSS;
    const savedHl = g.Highlight;
    const registry = new Map<string, unknown>();
    g.CSS = { highlights: registry };
    g.Highlight = class {};
    try {
      mockedConv.mockReturnValue(ok(conv()));
      const a = render(ConversationPanel, { session: session({ id: 1 }), visible: true });
      const b = render(ConversationPanel, { session: session({ id: 2 }), visible: true });
      await settle();
      for (const r of [a, b]) {
        const w = within(r.container);
        await fireEvent.keyDown(w.getByTestId('conv-scroller'), { key: 'f', ctrlKey: true });
        await settle();
        await fireEvent.input(w.getByTestId('conv-find-input'), { target: { value: 'bug' } });
        await settle();
      }
      expect(registry.size).toBe(4);
      const before = new Set(registry.keys());
      a.unmount();
      await settle();
      expect(registry.size).toBe(2);
      for (const k of registry.keys()) expect(before.has(k)).toBe(true);
      // b's rule sheet survives a's unmount.
      const sheets = Array.from(document.head.querySelectorAll('style')).map((el) => el.textContent ?? '');
      for (const k of registry.keys()) expect(sheets.some((t) => t.includes(`::highlight(${k})`))).toBe(true);
    } finally {
      g.CSS = savedCss;
      g.Highlight = savedHl;
    }
  });
  it('the composer textarea and Send button carry accessible names and the Enter shortcut', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    const box = screen.getByTestId('conv-composer-input');
    // A placeholder is not an accessible name: it disappears as soon as the
    // user types, so the field must carry its own label.
    expect(box.getAttribute('aria-label')).toBe('Prompt');
    expect(screen.getByTestId('conv-composer-send').getAttribute('aria-keyshortcuts')).toBe('Enter');
  });

  it('renders its own composer by default (showComposer defaults to true)', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true });
    await tick();
    await Promise.resolve();
    await tick();
    expect(screen.getByTestId('conv-composer')).toBeTruthy();
    expect(screen.getByTestId('conv-composer-send')).toBeTruthy();
  });

  it('renders no composer, and no read-only fallback either, when showComposer is false', async () => {
    // AgentPanel's case: a promptable (tmux-backed) session, but a host
    // that brings its own composer over this same session and does not
    // want ConversationPanel's send path active alongside its own.
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, { session: session(), visible: true, showComposer: false });
    await tick();
    await Promise.resolve();
    await tick();
    expect(screen.queryByTestId('conv-composer')).toBeNull();
    expect(screen.queryByTestId('conv-composer-send')).toBeNull();
    expect(screen.queryByTestId('conv-readonly')).toBeNull();
  });
});

// ─── Background switcher (task 6) ────────────────────────────────────────

const bgSubagentItem = {
  kind: 'subagent' as const,
  id: 'toolu_1',
  name: 'Agent',
  agent_type: 'general-purpose',
  description: 'Posúdiť stratégiu testov',
  result: null,
  error: false,
  at: '2026-09-13T10:00:00.000Z',
  ended_at: null,
  done: false,
};

const bgNotificationItem = {
  kind: 'notification' as const,
  task_id: 'a6',
  tool_use_id: 'toolu_1',
  status: 'completed',
  summary: 'Posúdiť stratégiu testov finished',
  result: 'All good.',
  output_file: null,
  event: null,
  at: '2026-09-13T10:05:00.000Z',
};

const convWithBackgroundAgent: Conversation = conv({
  turns: [
    { prompt: 'go check', at: '2026-09-13T10:00:00.000Z', ended_at: null, items: [bgSubagentItem] },
    { prompt: null, at: '2026-09-13T10:05:00.000Z', ended_at: '2026-09-13T10:05:01.000Z', items: [bgNotificationItem] },
  ],
});

const convWithNoBackground: Conversation = conv();

async function renderWithConversation(c: Conversation, opts: { sessions?: SessionRow[]; tasks?: TaskRow[] } = {}) {
  sessions.set(opts.sessions ?? []);
  tasks.set(opts.tasks ?? []);
  mockedConv.mockReturnValue(ok(c));
  const result = render(ConversationPanel, { session: session(), visible: true });
  await settle();
  return result;
}

describe('ConversationPanel background switcher', () => {
  it('offers a background switcher listing what the conversation launched', async () => {
    const { getByTestId, getAllByTestId } = await renderWithConversation(convWithBackgroundAgent);
    await fireEvent.click(getByTestId('conv-background-button'));
    const rows = getAllByTestId('conv-background-item');
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain('Posúdiť stratégiu testov');
  });

  it('heads each group so you can tell a transcript agent from a fleet child', async () => {
    const { getByTestId, getAllByTestId } = await renderWithConversation(convWithBackgroundAgent, {
      sessions: [session({ id: 2, parent_session_id: 1, kind: 'bg', friendly_name: 'Load layers' })],
    });
    await fireEvent.click(getByTestId('conv-background-button'));
    const heads = getAllByTestId('conv-background-group').map((h) => h.textContent?.trim());
    expect(heads).toEqual(['In this conversation', 'Fleet children']);
  });

  it('heads the one group that has entries', async () => {
    const { getByTestId, getAllByTestId } = await renderWithConversation(convWithBackgroundAgent);
    await fireEvent.click(getByTestId('conv-background-button'));
    const heads = getAllByTestId('conv-background-group').map((h) => h.textContent?.trim());
    expect(heads).toEqual(['In this conversation']);
  });

  it('hides the switcher when nothing ran in the background', async () => {
    const { queryByTestId } = await renderWithConversation(convWithNoBackground);
    expect(queryByTestId('conv-background-button')).toBeNull();
  });

  it('replaces the thread with the picked entry, and comes back', async () => {
    const { getByTestId, getAllByTestId, queryByTestId } = await renderWithConversation(convWithBackgroundAgent);
    await fireEvent.click(getByTestId('conv-background-button'));
    await fireEvent.click(getAllByTestId('conv-background-item')[0]);
    expect(getByTestId('bg-detail')).toBeTruthy();
    expect(queryByTestId('conv-scroller')).toBeNull();
    expect(queryByTestId('conv-composer')).toBeNull();
    await fireEvent.click(getByTestId('bg-detail-back'));
    expect(getByTestId('conv-scroller')).toBeTruthy();
  });

  it('opens the entry a notification row names', async () => {
    const { getByTestId } = await renderWithConversation(convWithBackgroundAgent);
    await fireEvent.click(getByTestId('conv-notification'));
    expect(getByTestId('bg-detail-label').textContent).toContain('Posúdiť stratégiu testov');
  });

  it('lists a launch and the resume that reports the same task id as two rows', async () => {
    // `each_key_duplicate` is a hard throw in dev AND prod: the tab used to
    // break the moment the dropdown opened on this very ordinary shape.
    const sendItem = {
      kind: 'tool' as const,
      summary: 'SendMessage(agent=a6)',
      error: false,
      id: 'toolu_S',
      name: 'SendMessage',
      target: null,
      at: '2026-09-13T10:10:00.000Z',
      ended_at: null,
      done: true,
    };
    const c: Conversation = conv({
      turns: [
        { prompt: 'go check', at: '2026-09-13T10:00:00.000Z', ended_at: null, items: [bgSubagentItem] },
        { prompt: null, at: '2026-09-13T10:05:00.000Z', ended_at: null, items: [bgNotificationItem] },
        { prompt: 'now resume it', at: '2026-09-13T10:10:00.000Z', ended_at: null, items: [sendItem] },
        {
          prompt: null,
          at: '2026-09-13T10:15:00.000Z',
          ended_at: null,
          items: [{ ...bgNotificationItem, tool_use_id: 'toolu_S', at: '2026-09-13T10:15:00.000Z' }],
        },
      ],
    });
    const { getByTestId, getAllByTestId } = await renderWithConversation(c);
    await fireEvent.click(getByTestId('conv-background-button'));
    expect(getAllByTestId('conv-background-item')).toHaveLength(2);
  });

  it('keeps an open detail open when the agent reports in', async () => {
    // The entry's key must not change under the open detail: it used to
    // mutate from `tool:<id>` to `task:<id>` on the first notification, and
    // the detail snapped shut at exactly the moment the report landed.
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    const running: Conversation = conv({
      turns: [{ prompt: 'go check', at: '2026-09-13T10:00:00.000Z', ended_at: null, items: [bgSubagentItem] }],
    });
    const { getByTestId, getAllByTestId } = await renderWithConversation(running);
    await fireEvent.click(getByTestId('conv-background-button'));
    await fireEvent.click(getAllByTestId('conv-background-item')[0]);
    expect(getByTestId('bg-detail')).toBeTruthy();
    // The next poll carries the notification.
    mockedConv.mockReturnValue(ok(convWithBackgroundAgent));
    vi.advanceTimersByTime(CONVERSATION_POLL_MS);
    await settle();
    expect(getByTestId('bg-detail')).toBeTruthy();
    expect(getByTestId('bg-detail-result').textContent).toContain('All good.');
  });

  it('opens the right entry from a notification row whose sibling carried no task id', async () => {
    // First notification: no task-id. Second: task-id a6. Keying the entry
    // by the task id left the FIRST row looking up a key nothing had, so it
    // rendered as a dead, non-clickable div.
    const c: Conversation = conv({
      turns: [
        { prompt: 'go check', at: '2026-09-13T10:00:00.000Z', ended_at: null, items: [bgSubagentItem] },
        {
          prompt: null,
          at: '2026-09-13T10:03:00.000Z',
          ended_at: null,
          items: [
            { ...bgNotificationItem, task_id: null, status: null, summary: 'still going', result: null },
            bgNotificationItem,
          ],
        },
      ],
    });
    const { getByTestId, getAllByTestId } = await renderWithConversation(c);
    const rows = getAllByTestId('conv-notification');
    expect(rows).toHaveLength(2);
    expect(rows[0].tagName).toBe('BUTTON');
    await fireEvent.click(rows[0]);
    expect(getByTestId('bg-detail-label').textContent).toContain('Posúdiť stratégiu testov');
  });

  it('puts the time on a notification row', async () => {
    const { getAllByTestId } = await renderWithConversation(convWithBackgroundAgent);
    const row = getAllByTestId('conv-notification')[0];
    expect(row.querySelector('time')?.getAttribute('datetime')).toBe('2026-09-13T10:05:00.000Z');
  });

  it('gives a backgrounded subagent block a way into its detail', async () => {
    const { getByTestId } = await renderWithConversation(convWithBackgroundAgent);
    await fireEvent.click(getByTestId('conv-subagent-open'));
    expect(getByTestId('bg-detail-label').textContent).toContain('Posúdiť stratégiu testov');
  });

  it('leaves a foreground subagent block without one', async () => {
    const c: Conversation = conv({
      turns: [
        {
          prompt: 'go check',
          at: '2026-09-13T10:00:00.000Z',
          ended_at: null,
          items: [{ ...bgSubagentItem, done: true, result: 'inline report' }],
        },
      ],
    });
    const { queryByTestId } = await renderWithConversation(c);
    expect(queryByTestId('conv-subagent')).toBeTruthy();
    expect(queryByTestId('conv-subagent-open')).toBeNull();
  });

  it("switches the app to a fleet task's worker session from the detail", async () => {
    const { getByTestId, getAllByTestId } = await renderWithConversation(convWithNoBackground, {
      sessions: [session({ id: 4, friendly_name: 'Worker' })],
      tasks: [
        {
          id: 11,
          requester_session_id: 1,
          worker_session_id: 4,
          prompt: 'Implement task 2',
          state: 'running',
          result: null,
          error: null,
          created_at: 1,
          started_at: 2,
          finished_at: null,
        },
      ],
    });
    await fireEvent.click(getByTestId('conv-background-button'));
    await fireEvent.click(getAllByTestId('conv-background-item')[0]);
    await fireEvent.click(getByTestId('bg-detail-open-session'));
    expect(selectSessionSpy).toHaveBeenCalledWith(expect.objectContaining({ id: 4 }));
  });

  it('switches the app to a fleet child session rather than showing a detail', async () => {
    const { getByTestId, getAllByTestId } = await renderWithConversation(convWithNoBackground, {
      sessions: [session({ id: 2, parent_session_id: 1, friendly_name: 'Load layers' })],
    });
    await fireEvent.click(getByTestId('conv-background-button'));
    await fireEvent.click(getAllByTestId('conv-background-item')[0]);
    expect(selectSessionSpy).toHaveBeenCalledWith(expect.objectContaining({ id: 2 }));
  });
});
