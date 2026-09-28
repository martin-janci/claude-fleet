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

import ForkSheet, { NEW_WORKTREE_OLD_HUB } from './ForkSheet.svelte';
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
  it('opens with New worktree selected, prefilled with the suggested name (spec §5.2)', () => {
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'fork-of-canopus', onclose: () => {} } });
    const newWt = screen.getByTestId('fork-new-worktree') as HTMLInputElement;
    const sameWt = screen.getByTestId('fork-same-worktree') as HTMLInputElement;
    expect(newWt.checked).toBe(true);
    expect(newWt.disabled).toBe(false);
    expect(sameWt.checked).toBe(false);
    expect((screen.getByTestId('fork-worktree-name') as HTMLInputElement).value).toBe('fork-of-canopus');
    // Uncommitted changes are not carried, and the sheet says so.
    expect(screen.getByTestId('fork-new-note').textContent).toMatch(/uncommitted changes stay here/i);
  });

  it('warns in the same-worktree option rather than only in a tooltip', () => {
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'f', onclose: () => {} } });
    expect(screen.getByTestId('fork-same-warning').textContent).toMatch(/same files/i);
  });

  it('forks into a new worktree in one click, with the name slugified', async () => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 2 } });
    const onclose = vi.fn();
    render(ForkSheet, { props: { sessionId: 1, anchor: 'anchor-1', suggestedName: 'fork-of-canopus', onclose } });
    await fireEvent.input(screen.getByTestId('fork-worktree-name'), { target: { value: 'Try The Other Way ' } });
    const confirm = screen.getByTestId('fork-confirm') as HTMLButtonElement;
    expect(confirm.disabled).toBe(false);
    await fireEvent.click(confirm);
    await settle();
    expect(mockedRewind).toHaveBeenCalledWith(1, 'fork', 'anchor-1', 'try-the-other-way');
    expect(onclose).toHaveBeenCalled();
  });

  it('forks into the same worktree with no name', async () => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 2 } });
    render(ForkSheet, { props: { sessionId: 1, anchor: 'anchor-1', suggestedName: 'f', onclose: () => {} } });
    await fireEvent.click(screen.getByTestId('fork-same-worktree'));
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    await settle();
    expect(mockedRewind).toHaveBeenCalledWith(1, 'fork', 'anchor-1', null);
  });

  it('will not submit a name the backend would refuse', async () => {
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'main', onclose: () => {} } });
    expect(screen.getByTestId('fork-name-problem').textContent).toMatch(/main or master/);
    expect((screen.getByTestId('fork-confirm') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.input(screen.getByTestId('fork-worktree-name'), { target: { value: '' } });
    expect((screen.getByTestId('fork-confirm') as HTMLButtonElement).disabled).toBe(true);
    // Same worktree needs no name.
    await fireEvent.click(screen.getByTestId('fork-same-worktree'));
    expect((screen.getByTestId('fork-confirm') as HTMLButtonElement).disabled).toBe(false);
  });

  it("an older hub's E_UNSUPPORTED reads as 'update the hub', and the sheet stays open", async () => {
    mockedRewind.mockResolvedValue({
      ok: false,
      error: { code: 'E_UNSUPPORTED', message: "forking into a new worktree isn't implemented yet" },
    });
    const onclose = vi.fn();
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'fork-x', onclose } });
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    await settle();
    expect(onclose).not.toHaveBeenCalled();
    expect(screen.getByTestId('fork-error').textContent).toBe(NEW_WORKTREE_OLD_HUB);
  });

  it('a refused fork does not close the sheet', async () => {
    mockedRewind.mockResolvedValue({
      ok: false,
      error: { code: 'E_NOTFOUND', message: 'session not found' },
    });
    const onclose = vi.fn();
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'f', onclose } });
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    await settle();
    expect(onclose).not.toHaveBeenCalled();
    expect(screen.getByText('session not found')).toBeTruthy();
  });
});
