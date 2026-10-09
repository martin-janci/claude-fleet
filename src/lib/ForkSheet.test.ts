import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
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
import { rewindConversation, sessions } from './sessions';
import { session } from './hosts_fixture';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { resetAccessForTests, setMyGrants } from './access';

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

  // Step 5.13: forking is merging work, so the new layout shows the Liquid
  // orbit while it runs, and nothing once it is done.
  it('new layout: the Liquid orbit runs while the fork does', async () => {
    let resolve!: (v: unknown) => void;
    mockedRewind.mockReturnValue(new Promise((r) => (resolve = r)));
    render(ForkSheet, { props: { sessionId: 1, anchor: null, suggestedName: 'f', onclose: () => {} } });
    expect(screen.queryByTestId('fork-merging')).toBeNull();
    await fireEvent.click(screen.getByTestId('fork-confirm'));
    await settle();
    expect(screen.getByTestId('fork-merging').textContent).toContain('Copying the conversation');
    resolve({ ok: true, value: { id: 2 } });
    await settle();
    expect(screen.queryByTestId('fork-merging')).toBeNull();
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
    expect(screen.getByTestId('fork-error').textContent?.trim()).toBe(NEW_WORKTREE_OLD_HUB);
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

// ── Multi-user M1 (F2a): this sheet IS the confirmation ─────────────────────
//
// `ReplyActions`' Fork button composes both halves, but there is no second
// "are you sure?" on top of this sheet (spec §5.2), so a reason arriving while
// it is open — a revoke, a narrowed grant — has to reach Fork itself.
// `rewind_conversation` is `own` in `share.ts::SESSION_TIER`: a fork leaves a
// permanent verbatim copy of the owner's transcript behind and creates a branch
// and a worktree on the owner's host. The lookup is on the `sessionId` prop,
// which is all this component is given.
describe('ForkSheet access gate (multi-user M1)', () => {
  const paired: HubStatus = {
    ...STANDALONE,
    remote: true,
    url: 'https://fleet.example.com',
    configured_url: 'https://fleet.example.com',
  };
  const row = (owner: number) =>
    session('mefistos', 'dev-api', { id: 1, visibility: 'private', owner_person_id: owner });
  const confirm = () => screen.getByTestId('fork-confirm') as HTMLButtonElement;
  const props = { sessionId: 1, anchor: 'a1', suggestedName: 'f', onclose: () => {} };

  beforeEach(() => {
    mockedRewind.mockResolvedValue({ ok: true, value: { id: 2 } });
    hubStatus.set(paired);
    hubConnection.set({ state: 'connected' });
    resetAccessForTests();
  });

  afterEach(() => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    sessions.set([]);
    resetAccessForTests();
  });

  it('the owner forks on a paired desktop (the positive control)', async () => {
    sessions.set([row(7)]);
    setMyGrants(7, []);
    render(ForkSheet, { props });
    await settle();
    expect(confirm().disabled).toBe(false);
    await fireEvent.click(confirm());
    await settle();
    expect(mockedRewind).toHaveBeenCalledTimes(1);
  });

  for (const level of ['watch', 'drive'] as const) {
    it(`a ${level} grantee cannot fork — the copy outlives the grant`, async () => {
      sessions.set([row(42)]);
      setMyGrants(7, [{ session_id: 1, level }]);
      render(ForkSheet, { props });
      await settle();
      expect(confirm().disabled).toBe(true);
      expect(screen.getByText(/only the session’s owner/i)).toBeTruthy();
      await fireEvent.click(confirm());
      await settle();
      expect(mockedRewind).not.toHaveBeenCalled();
    });
  }

  it('a revoke arriving while the sheet is open reaches Fork', async () => {
    sessions.set([row(7)]);
    setMyGrants(7, []);
    render(ForkSheet, { props });
    await settle();
    expect(confirm().disabled).toBe(false);
    // The owner's row is reassigned (a move drops grants and re-owns the row):
    // no grant frame, just the row's own column.
    sessions.set([row(42)]);
    await settle();
    expect(confirm().disabled).toBe(true);
    await fireEvent.click(confirm());
    await settle();
    expect(mockedRewind).not.toHaveBeenCalled();
  });

  it('standalone is untouched, even with no row for the id at all', async () => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    sessions.set([]);
    render(ForkSheet, { props });
    await settle();
    expect(confirm().disabled).toBe(false);
  });
});
