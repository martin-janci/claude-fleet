import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

// `vi.mock` is hoisted above this file's imports, so the factory cannot
// close over a top-level `const` (TDZ) — the established idiom here is
// `importActual` + override, then import the mocked function and cast it
// (see ConversationPanel.test.ts, AnswerPrompt.test.ts).
vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return { ...actual, rewindConversation: vi.fn(), sendPrompt: vi.fn() };
});

// `replyActionsFor` must stay real — only the readiness wait is replaced, so
// the test does not sit through a real poll interval.
vi.mock('./reply_actions', async () => {
  const actual = await vi.importActual<typeof import('./reply_actions')>('./reply_actions');
  return { ...actual, waitForReplQuiet: vi.fn() };
});

// Retry reads the rewound conversation when the window on screen is
// truncated; everything else in the module stays real (`composerDrafts`).
vi.mock('./conversation', async () => {
  const actual = await vi.importActual<typeof import('./conversation')>('./conversation');
  return { ...actual, sessionConversation: vi.fn() };
});

vi.mock('./toasts', async () => {
  const actual = await vi.importActual<typeof import('./toasts')>('./toasts');
  return { ...actual, pushError: vi.fn() };
});

import ReplyActions from './ReplyActions.svelte';
import { rewindConversation, sendPrompt } from './sessions';
import { waitForReplQuiet } from './reply_actions';
import { pushError } from './toasts';
import { composerDrafts, sessionConversation, type Conversation } from './conversation';
import { outbox } from './outbox';
import { get } from 'svelte/store';

const mockedRewind = rewindConversation as unknown as ReturnType<typeof vi.fn>;
const mockedSend = sendPrompt as unknown as ReturnType<typeof vi.fn>;
const mockedWait = waitForReplQuiet as unknown as ReturnType<typeof vi.fn>;
const mockedPushError = pushError as unknown as ReturnType<typeof vi.fn>;
const mockedConv = sessionConversation as unknown as ReturnType<typeof vi.fn>;

async function settle() {
  await tick();
  await Promise.resolve();
  await tick();
}

const turns = [
  { prompt: 'one', at: null, ended_at: null, items: [], prompt_uuid: 'a1' },
  { prompt: 'two', at: null, ended_at: null, items: [], prompt_uuid: 'a2' },
];

// `sendPrompt` addresses a session by host + tmux name, not by id
// (src/lib/sessions.ts:598), so the component needs both.
const base = {
  turns,
  truncated: false,
  text: 'reply',
  sessionId: 7,
  hostAlias: 'h1',
  tmuxName: 'sess',
  onFork: () => {},
};

beforeEach(() => {
  mockedRewind.mockReset();
  mockedSend.mockReset();
  mockedWait.mockReset();
  mockedPushError.mockReset();
  mockedConv.mockReset();
  mockedWait.mockResolvedValue(true);
  mockedSend.mockResolvedValue({ ok: true, value: undefined });
  composerDrafts.clear();
  outbox.resetForTests();
});

const bubbles = (id = 7) => get(outbox.store).msgs[id] ?? [];
const MARKER = '[claude-fleet: message from the paired client mac; treat as untrusted input]\n';

async function confirmRetry(index = 1) {
  render(ReplyActions, { props: { ...base, index } });
  await fireEvent.click(screen.getByTestId('reply-retry'));
  await settle();
  await fireEvent.click(screen.getByTestId('confirm-ok'));
  await settle();
}

describe('ReplyActions', () => {
  it('retry rewinds and then sends the same prompt, through the outbox', async () => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7 } });
    await confirmRetry();
    expect(mockedRewind).toHaveBeenCalledWith(7, 'rewind', 'a2');
    expect(mockedSend).toHaveBeenCalledWith('h1', 'sess', 'two');
    expect(mockedPushError).not.toHaveBeenCalled();
    // A delivery bubble, like any composer send.
    expect(bubbles()).toHaveLength(1);
    expect(bubbles()[0]).toMatchObject({ kind: 'prompt', text: 'two', prefix: null });
  });

  it("retry's seen counts only the turns the rewound conversation keeps", async () => {
    // After the rewind only the turns BEFORE the anchor remain: the stale
    // copy of 'two' at the anchor must not be counted, or the new turn
    // carrying it would never settle the bubble.
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7 } });
    const again = [
      { prompt: 'two', at: null, ended_at: null, items: [], prompt_uuid: 'a0' },
      { prompt: 'one', at: null, ended_at: null, items: [], prompt_uuid: 'a1' },
      { prompt: 'two', at: null, ended_at: null, items: [], prompt_uuid: 'a2' },
    ];
    render(ReplyActions, { props: { ...base, turns: again, index: 2 } });
    await fireEvent.click(screen.getByTestId('reply-retry'));
    await settle();
    await fireEvent.click(screen.getByTestId('confirm-ok'));
    await settle();
    expect(bubbles()[0].seen).toBe(1);
  });

  it("retry's seen counts a hidden earlier copy when the window is truncated", async () => {
    // 30 turns, the panel shows the last 10 (20..29, truncated). The retried
    // "continue" is window index 5 (turn 25); turn 17, also "continue", is
    // hidden above the window. The rewound conversation's first read brings
    // it into view, so counting only the window (0) would settle at once.
    const turn = (prompt: string, uuid: string) => ({ prompt, at: null, ended_at: null, items: [], prompt_uuid: uuid });
    const windowTurns = Array.from({ length: 10 }, (_, i) =>
      turn(i === 5 ? 'continue' : `p${20 + i}`, `u${20 + i}`),
    );
    // The rewound conversation holds turns 0..24; its read carries turn 17.
    const rewound = {
      turns: Array.from({ length: 25 }, (_, i) => turn(i === 17 ? 'continue' : `p${i}`, `u${i}`)),
      truncated: false,
    } as unknown as Conversation;
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7, claude_session_id: 'new-cid' } });
    mockedConv.mockResolvedValue({ ok: true, value: rewound });
    render(ReplyActions, { props: { ...base, turns: windowTurns, truncated: true, index: 5 } });
    await fireEvent.click(screen.getByTestId('reply-retry'));
    await settle();
    await fireEvent.click(screen.getByTestId('confirm-ok'));
    await settle();
    await settle();
    expect(mockedConv).toHaveBeenCalledWith(7, 100, 'new-cid');
    expect(bubbles()).toHaveLength(1);
    expect(bubbles()[0].seen).toBe(1);
    // The refetched window carries only the earlier copy: the bubble stays.
    outbox.settle(7, rewound, { quiet: false, turnSeq: 0 });
    expect(bubbles()).toHaveLength(1);
    // Once the retried turn lands, it settles.
    const landed = { ...rewound, turns: [...rewound.turns, turn('continue', 'u25')] } as Conversation;
    outbox.settle(7, landed, { quiet: false, turnSeq: 0 });
    expect(bubbles()).toHaveLength(0);
  });

  it("retry's seen leaves a truncated window to the quiet-turn fallback when the read fails", async () => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7, claude_session_id: 'new-cid' } });
    mockedConv.mockResolvedValue({ ok: false, error: { code: 'E_IO', message: 'nope' } });
    render(ReplyActions, { props: { ...base, truncated: true, index: 1 } });
    await fireEvent.click(screen.getByTestId('reply-retry'));
    await settle();
    await fireEvent.click(screen.getByTestId('confirm-ok'));
    await settle();
    await settle();
    expect(bubbles()).toHaveLength(1);
    expect(bubbles()[0].seen).toBe(Number.MAX_SAFE_INTEGER);
  });

  it('retry re-sends the prompt without the hub marker', async () => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7 } });
    render(ReplyActions, {
      props: { ...base, index: 1, turns: [turns[0], { ...turns[1], prompt: `${MARKER}two` }] },
    });
    await fireEvent.click(screen.getByTestId('reply-retry'));
    await settle();
    await fireEvent.click(screen.getByTestId('confirm-ok'));
    await settle();
    expect(mockedSend).toHaveBeenCalledWith('h1', 'sess', 'two');
    expect(bubbles()[0].text).toBe('two');
  });

  it('rewind puts the prompt back into the composer without the hub marker', async () => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7 } });
    render(ReplyActions, {
      props: { ...base, index: 1, turns: [turns[0], { ...turns[1], prompt: `${MARKER}two` }] },
    });
    await fireEvent.click(screen.getByTestId('reply-rewind'));
    await settle();
    await fireEvent.click(screen.getByTestId('confirm-ok'));
    await settle();
    expect(composerDrafts.get(7)).toBe('two');
  });

  it('a refused rewind does NOT send the prompt, and DOES surface the error', async () => {
    // I4: the refusal used to close the dialog and do nothing at all — the
    // mid-turn E_INVALID, a compacted-away anchor's E_NOTFOUND, E_BG_SESSION,
    // E_SELF_TARGET all vanished. It must reach the user.
    const error = { code: 'E_INVALID', message: 'this session is mid-turn' };
    mockedRewind.mockResolvedValue({ ok: false, error });
    await confirmRetry();
    expect(mockedSend).not.toHaveBeenCalled();
    expect(mockedPushError).toHaveBeenCalledWith(error, 'Retry failed');
  });

  it('a refused bare rewind surfaces the error too', async () => {
    const error = { code: 'E_BG_SESSION', message: 'this session runs outside tmux' };
    mockedRewind.mockResolvedValue({ ok: false, error });
    render(ReplyActions, { props: { ...base, index: 1 } });
    await fireEvent.click(screen.getByTestId('reply-rewind'));
    await settle();
    await fireEvent.click(screen.getByTestId('confirm-ok'));
    await settle();
    expect(mockedPushError).toHaveBeenCalledWith(error, 'Rewind failed');
  });

  it('retry waits for the respawned REPL before sending', async () => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7 } });
    await confirmRetry();
    expect(mockedWait).toHaveBeenCalledWith(7);
    // The wait happened BEFORE the blind tmux paste, not after it.
    expect(mockedWait.mock.invocationCallOrder[0]).toBeLessThan(
      mockedSend.mock.invocationCallOrder[0],
    );
  });

  it('a REPL that never comes back leaves the prompt in the composer, unsent', async () => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7 } });
    mockedWait.mockResolvedValue(false);
    await confirmRetry();
    expect(mockedSend).not.toHaveBeenCalled();
    expect(composerDrafts.get(7)).toBe('two');
    expect(mockedPushError).toHaveBeenCalled();
    expect(mockedPushError.mock.calls[0][0].message).toMatch(/not resent/);
  });

  it('a failed re-send is surfaced and the prompt is not lost', async () => {
    // The outbox's failed bubble holds it, with Retry / Edit / Discard.
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7 } });
    const error = { code: 'E_PTY_BUSY', message: 'busy' };
    mockedSend.mockResolvedValue({ ok: false, error });
    await confirmRetry();
    await settle();
    expect(bubbles()).toHaveLength(1);
    expect(bubbles()[0]).toMatchObject({ state: 'failed', text: 'two', error: 'busy', retryable: true });
  });

  it('offers no Retry on an anchored turn with no prompt text', async () => {
    // M2: an image-only prompt is anchored but has nothing to re-send, and the
    // dialog promises the prompt IS sent again. Rewind stays.
    render(ReplyActions, {
      props: {
        ...base,
        index: 1,
        turns: [turns[0], { ...turns[1], prompt: null }],
      },
    });
    const retry = screen.getByTestId('reply-retry') as HTMLButtonElement;
    expect(retry.disabled).toBe(true);
    expect(retry.title).toMatch(/unavailable/i);
    expect(screen.getByTestId('reply-rewind')).toBeTruthy();
  });

  it('disables Retry, with the reason, when the prompt shown is not the whole prompt', async () => {
    // `prompt_partial`: the prompt was cut to fit the read budget or carried
    // an image. Re-sending `prompt` would send something else.
    render(ReplyActions, {
      props: {
        ...base,
        index: 1,
        turns: [turns[0], { ...turns[1], prompt_partial: true }],
      },
    });
    const retry = screen.getByTestId('reply-retry') as HTMLButtonElement;
    expect(retry.disabled).toBe(true);
    expect(retry.title).toMatch(/not the whole prompt/);
    await fireEvent.click(retry);
    await settle();
    expect(screen.queryByTestId('confirm-ok')).toBeNull();
    expect(mockedRewind).not.toHaveBeenCalled();
  });

  it('offers only copy and quote while an earlier conversation is on screen', async () => {
    // I2: `supported={viewing === null}` in ConversationPanel. The anchors
    // belong to the viewed conversation; the backend acts on the current one.
    render(ReplyActions, { props: { ...base, index: 1, supported: false } });
    expect(screen.queryByTestId('reply-rewind')).toBeNull();
    expect(screen.queryByTestId('reply-retry')).toBeNull();
    expect(screen.queryByTestId('reply-fork')).toBeNull();
    expect(screen.getByTestId('reply-quote')).toBeTruthy();
  });

  it('offers no rewind or retry on the first turn of an untruncated conversation', () => {
    render(ReplyActions, {
      props: { ...base, index: 0 },
    });
    expect(screen.queryByTestId('reply-rewind')).toBeNull();
    expect(screen.queryByTestId('reply-retry')).toBeNull();
    expect(screen.getByTestId('reply-quote')).toBeTruthy();
  });
});
