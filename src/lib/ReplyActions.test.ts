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

vi.mock('./toasts', async () => {
  const actual = await vi.importActual<typeof import('./toasts')>('./toasts');
  return { ...actual, pushError: vi.fn() };
});

import ReplyActions from './ReplyActions.svelte';
import { rewindConversation, sendPrompt } from './sessions';
import { waitForReplQuiet } from './reply_actions';
import { pushError } from './toasts';
import { composerDrafts } from './conversation';

const mockedRewind = rewindConversation as unknown as ReturnType<typeof vi.fn>;
const mockedSend = sendPrompt as unknown as ReturnType<typeof vi.fn>;
const mockedWait = waitForReplQuiet as unknown as ReturnType<typeof vi.fn>;
const mockedPushError = pushError as unknown as ReturnType<typeof vi.fn>;

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
  mockedWait.mockResolvedValue(true);
  mockedSend.mockResolvedValue({ ok: true, value: undefined });
  composerDrafts.clear();
});

async function confirmRetry(index = 1) {
  render(ReplyActions, { props: { ...base, index } });
  await fireEvent.click(screen.getByTestId('reply-retry'));
  await settle();
  await fireEvent.click(screen.getByTestId('confirm-ok'));
  await settle();
}

describe('ReplyActions', () => {
  it('retry rewinds and then sends the same prompt', async () => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7 } });
    await confirmRetry();
    expect(mockedRewind).toHaveBeenCalledWith(7, 'rewind', 'a2');
    expect(mockedSend).toHaveBeenCalledWith('h1', 'sess', 'two');
    expect(mockedPushError).not.toHaveBeenCalled();
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
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7 } });
    const error = { code: 'E_PTY_BUSY', message: 'busy' };
    mockedSend.mockResolvedValue({ ok: false, error });
    await confirmRetry();
    expect(composerDrafts.get(7)).toBe('two');
    expect(mockedPushError).toHaveBeenCalledWith(error, expect.stringContaining('not resent'));
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
    expect(screen.queryByTestId('reply-retry')).toBeNull();
    expect(screen.getByTestId('reply-rewind')).toBeTruthy();
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
