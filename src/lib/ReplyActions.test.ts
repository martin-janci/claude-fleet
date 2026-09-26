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

import ReplyActions from './ReplyActions.svelte';
import { rewindConversation, sendPrompt } from './sessions';

const mockedRewind = rewindConversation as unknown as ReturnType<typeof vi.fn>;
const mockedSend = sendPrompt as unknown as ReturnType<typeof vi.fn>;

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
});

describe('ReplyActions', () => {
  it('retry rewinds and then sends the same prompt', async () => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 7 } });
    render(ReplyActions, {
      props: { ...base, index: 1 },
    });
    await fireEvent.click(screen.getByTestId('reply-retry'));
    await settle();
    await fireEvent.click(screen.getByTestId('confirm-ok'));
    await settle();
    expect(mockedRewind).toHaveBeenCalledWith(7, 'rewind', 'a2');
    expect(mockedSend).toHaveBeenCalledWith('h1', 'sess', 'two');
  });

  it('a refused rewind does NOT then send the prompt', async () => {
    mockedRewind.mockResolvedValue({
      ok: false,
      error: { code: 'E_INVALID', message: 'this session is mid-turn' },
    });
    render(ReplyActions, {
      props: { ...base, index: 1 },
    });
    await fireEvent.click(screen.getByTestId('reply-retry'));
    await settle();
    await fireEvent.click(screen.getByTestId('confirm-ok'));
    await settle();
    expect(mockedSend).not.toHaveBeenCalled();
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
