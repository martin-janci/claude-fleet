import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

// `vi.mock` is hoisted above this file's imports, so the factory cannot
// close over a top-level `const` (TDZ) — the established idiom here is
// `importActual` + override, then import the mocked function and cast it
// (see ReplyActions.test.ts, ConversationPanel.test.ts).
vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return { ...actual, rewindConversation: vi.fn() };
});

import ForkSheet from './ForkSheet.svelte';
import { rewindConversation } from './sessions';

const mockedRewind = rewindConversation as unknown as ReturnType<typeof vi.fn>;

async function settle() {
  await tick();
  await Promise.resolve();
  await tick();
}

beforeEach(() => {
  mockedRewind.mockReset();
});

describe('ForkSheet', () => {
  it('defaults to a new worktree, because two sessions in one checkout lose work', () => {
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'fork-of-canopus', onclose: () => {} } });
    const newWt = screen.getByTestId('fork-new-worktree') as HTMLInputElement;
    expect(newWt.checked).toBe(true);
  });

  it('warns in the same-worktree option rather than only in a tooltip', () => {
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'f', onclose: () => {} } });
    expect(screen.getByTestId('fork-same-warning').textContent).toMatch(/same files/i);
  });

  it('passes the chosen worktree name through to the backend', async () => {
    // The backend does not implement a new-worktree fork yet
    // (`rewind_conversation` refuses `mode: fork` + `new_worktree: Some(_)`
    // with E_UNSUPPORTED — crates/fleet-core/src/service/rewind.rs), so
    // the default (new worktree) selection must NOT be forkable: Fork stays
    // disabled until the user explicitly picks the working option, and only
    // then does it call through to the backend.
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 1 } });
    const onclose = vi.fn();
    render(ForkSheet, {
      props: { sessionId: 1, anchor: 'anchor-1', suggestedName: 'fork-of-canopus', onclose },
    });

    const confirm = screen.getByTestId('fork-confirm') as HTMLButtonElement;
    expect(confirm.disabled).toBe(true);
    expect(mockedRewind).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByTestId('fork-same-worktree'));
    await settle();
    expect(confirm.disabled).toBe(false);

    await fireEvent.click(confirm);
    await settle();

    expect(mockedRewind).toHaveBeenCalledWith(1, 'fork', 'anchor-1', null);
    expect(onclose).toHaveBeenCalled();
  });

  it('a refused fork does not close the sheet', async () => {
    mockedRewind.mockResolvedValue({
      ok: false,
      error: { code: 'E_NOTFOUND', message: 'session not found' },
    });
    const onclose = vi.fn();
    render(ForkSheet, {
      props: { sessionId: 1, anchor: null, suggestedName: 'f', onclose },
    });
    await fireEvent.click(screen.getByTestId('fork-same-worktree'));
    await settle();
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    await settle();
    expect(onclose).not.toHaveBeenCalled();
    expect(screen.getByText('session not found')).toBeTruthy();
  });
});
