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
  it('opens with Same worktree selected, because New worktree is not implemented yet', () => {
    // The backend does not implement a new-worktree fork yet
    // (`rewind_conversation` refuses `mode: fork` + `new_worktree: Some(_)`
    // with E_UNSUPPORTED — crates/fleet-core/src/service/rewind.rs). A
    // default that cannot be submitted is worse than a default that is
    // honest about its risk, so "Same worktree" (with its warning) is what
    // opens selected and actionable. "New worktree" stays visible — its
    // presence is what makes the deferral legible — but permanently
    // disabled, never selectable.
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'fork-of-canopus', onclose: () => {} } });
    const sameWt = screen.getByTestId('fork-same-worktree') as HTMLInputElement;
    const newWt = screen.getByTestId('fork-new-worktree') as HTMLInputElement;
    expect(sameWt.checked).toBe(true);
    expect(newWt.checked).toBe(false);
    expect(newWt.disabled).toBe(true);
  });

  it('warns in the same-worktree option rather than only in a tooltip', () => {
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'f', onclose: () => {} } });
    expect(screen.getByTestId('fork-same-warning').textContent).toMatch(/same files/i);
  });

  it('is forkable on open, in one click, with the default selection', async () => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 1 } });
    const onclose = vi.fn();
    render(ForkSheet, {
      props: { sessionId: 1, anchor: 'anchor-1', suggestedName: 'fork-of-canopus', onclose },
    });

    const confirm = screen.getByTestId('fork-confirm') as HTMLButtonElement;
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
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    await settle();
    expect(onclose).not.toHaveBeenCalled();
    expect(screen.getByText('session not found')).toBeTruthy();
  });
});
