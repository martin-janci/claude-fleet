// The Share sheet (multi-user M1, F2). Modelled on `ForkSheet.test.ts`, the
// smallest sheet test in the repo: mock the `sessions.ts` wrappers, render the
// component, drive the controls.
//
// What it pins beyond the three buttons working: that the sheet NAMES no team
// (M1 has no membership table, so a team share has no referent), that it offers
// nothing that raises a level, and that it says out loud the two things a
// sharer is deciding without being told — the recipient gets the history from
// before the share, and watch/drive are Fleet's rule and not SSH's.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

// `vi.mock` is hoisted above the imports, so the factory cannot close over a
// top-level `const` (TDZ): the established idiom is `importActual` + override,
// then import the mocked function and cast it (ForkSheet.test.ts,
// ReplyActions.test.ts).
vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return {
    ...actual,
    shareSession: vi.fn(),
    unshareSession: vi.fn(),
    narrowShare: vi.fn(),
    fetchSessionAccess: vi.fn(),
  };
});

import ShareSheet from './ShareSheet.svelte';
import {
  fetchSessionAccess,
  narrowShare,
  sessions,
  shareSession,
  unshareSession,
  type SessionGrant,
} from './sessions';
import { shareSheetFor } from './share';
import { hubStatus, STANDALONE } from './hub';
import { resetAccessForTests, setMyGrants } from './access';
import { session } from './hosts_fixture';

const mockedShare = shareSession as unknown as ReturnType<typeof vi.fn>;
const mockedUnshare = unshareSession as unknown as ReturnType<typeof vi.fn>;
const mockedNarrow = narrowShare as unknown as ReturnType<typeof vi.fn>;
const mockedList = fetchSessionAccess as unknown as ReturnType<typeof vi.fn>;

const ROW = session('mefistos', 'dev-foo', { id: 42, visibility: 'private', owner_person_id: 1 });

const grant = (over: Partial<SessionGrant> = {}): SessionGrant => ({
  session_id: 42,
  person_id: 2,
  person_name: 'bea',
  person_display_name: 'Bea',
  level: 'watch',
  granted_at: 1,
  ...over,
});

async function settle() {
  await tick();
  await Promise.resolve();
  await tick();
  await Promise.resolve();
  await tick();
}

beforeEach(() => {
  // Standalone, which is `own` on every row (`access.ts` rule 1) — the owner's
  // view, which is the only one the sheet's controls are for.
  hubStatus.set({ ...STANDALONE });
  resetAccessForTests();
  sessions.set([ROW]);
  shareSheetFor.set(42);
  mockedShare.mockReset();
  mockedUnshare.mockReset();
  mockedNarrow.mockReset();
  mockedList.mockReset();
  mockedList.mockResolvedValue({ ok: true, value: [] });
});

describe('ShareSheet', () => {
  it('reads the live grant list on open and says when there is none', async () => {
    render(ShareSheet);
    await settle();
    expect(mockedList).toHaveBeenCalledWith(42);
    expect(screen.getByTestId('share-list-empty').textContent).toMatch(/not shared with anyone/i);
  });

  it('shares with one PERSON at a level, and offers no team anywhere', async () => {
    mockedShare.mockResolvedValue({ ok: true, value: null });
    render(ShareSheet);
    await settle();
    await fireEvent.input(screen.getByTestId('share-person'), { target: { value: ' bea ' } });
    await fireEvent.change(screen.getByTestId('share-level'), { target: { value: 'drive' } });
    await fireEvent.click(screen.getByTestId('share-confirm'));
    await settle();
    // Trimmed, and the level is the one that was picked.
    expect(mockedShare).toHaveBeenCalledWith(42, 'bea', 'drive');
    // The list is re-read rather than patched: the hub is the authority on what
    // the grant set now is.
    expect(mockedList).toHaveBeenCalledTimes(2);
    // M1 names people only. Nothing in the sheet offers a team, an org, or a
    // "share with my team" control — team sharing needs memberships and is M2.
    expect(screen.getByTestId('share-no-team').textContent).toMatch(/people only/i);
    const text = screen.getByTestId('share-sheet').textContent ?? '';
    expect(text).not.toMatch(/\borg\b|organisation|organization/i);
    const levels = Array.from(
      screen.getByTestId('share-level').querySelectorAll('option'),
      (o) => (o as HTMLOptionElement).value,
    );
    expect(levels).toEqual(['watch', 'drive']);
  });

  it('will not submit an empty recipient', async () => {
    render(ShareSheet);
    await settle();
    expect((screen.getByTestId('share-confirm') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.input(screen.getByTestId('share-person'), { target: { value: '   ' } });
    expect((screen.getByTestId('share-confirm') as HTMLButtonElement).disabled).toBe(true);
  });

  it('lists each grant, and offers narrow only on a drive one — never a widen', async () => {
    mockedList.mockResolvedValue({
      ok: true,
      value: [grant(), grant({ person_id: 3, person_name: 'cy', person_display_name: 'Cy', level: 'drive' })],
    });
    render(ShareSheet);
    await settle();
    const rows = screen.getAllByTestId('share-grant');
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain('Bea');
    expect(rows[0].textContent).toContain('watch');
    // A grant only ever moves downward (spec §4.3 invariant 3): the watch row
    // has no control at all that raises it, and no row anywhere offers one.
    expect(rows[0].querySelector('[data-testid="share-narrow"]')).toBeNull();
    expect(rows[1].querySelector('[data-testid="share-narrow"]')).not.toBeNull();
    expect(screen.getByTestId('share-sheet').textContent).not.toMatch(/widen|raise|upgrade|promote/i);
  });

  it('narrows a drive grant to watch', async () => {
    mockedList.mockResolvedValue({ ok: true, value: [grant({ level: 'drive' })] });
    mockedNarrow.mockResolvedValue({ ok: true, value: null });
    render(ShareSheet);
    await settle();
    await fireEvent.click(screen.getByTestId('share-narrow'));
    await settle();
    expect(mockedNarrow).toHaveBeenCalledWith(42, 'bea');
  });

  it('revokes behind a confirmation', async () => {
    mockedList.mockResolvedValue({ ok: true, value: [grant()] });
    mockedUnshare.mockResolvedValue({ ok: true, value: null });
    render(ShareSheet);
    await settle();
    await fireEvent.click(screen.getByTestId('share-revoke'));
    await tick();
    expect(mockedUnshare).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('share-revoke-yes'));
    await settle();
    expect(mockedUnshare).toHaveBeenCalledWith(42, 'bea');
  });

  it('keeps the sheet open on a refusal, with the reason in place', async () => {
    mockedShare.mockResolvedValue({
      ok: false,
      error: { code: 'E_FORBIDDEN', message: 'only the owner may share this session' },
    });
    render(ShareSheet);
    await settle();
    await fireEvent.input(screen.getByTestId('share-person'), { target: { value: 'bea' } });
    await fireEvent.click(screen.getByTestId('share-confirm'));
    await settle();
    expect(screen.getByTestId('share-sheet')).toBeInTheDocument();
    expect(screen.getByTestId('share-error').textContent).toContain('only the owner may share');
  });

  it('says the recipient gets the history from before the share, and that SSH is not Fleet’s to revoke', async () => {
    render(ShareSheet);
    await settle();
    expect(screen.getByTestId('share-history-note').textContent).toMatch(
      /history from before the share/i,
    );
    const enforcement = screen.getByTestId('share-enforcement-note').textContent ?? '';
    expect(enforcement).toMatch(/enforced by fleet, not by ssh/i);
    // "Sharing never confers a terminal" said where the decision is taken.
    expect(enforcement).toMatch(/never gives a terminal/i);
    expect(enforcement).toContain('mefistos');
  });

  it('closes itself when the session leaves the store', async () => {
    render(ShareSheet);
    await settle();
    expect(screen.queryByTestId('share-sheet')).not.toBeNull();
    sessions.set([]);
    await settle();
    expect(screen.queryByTestId('share-sheet')).toBeNull();
  });

  it('shows the owner-only refusal rather than the controls when a grant is all this client has', async () => {
    // A paired desktop whose grant on the row is `drive`: the sheet is the
    // owner's tool, and this is what a revoke ARRIVING while it is open looks
    // like.
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://fleet.example.com' });
    setMyGrants(9, [{ session_id: 42, level: 'drive' }]);
    render(ShareSheet);
    await settle();
    expect(screen.getByTestId('share-not-owner').textContent).toMatch(/only the session’s owner/i);
    expect(screen.queryByTestId('share-person')).toBeNull();
    expect(screen.queryByTestId('share-confirm')).toBeNull();
  });

  // F2b: `run` is where all three writes happen, and it trusted the markup
  // above it — which swaps the body for the refusal notice, but only on the next
  // render. This pins the user-visible half (the owner acts, a grantee gets the
  // notice and no controls); `run`'s own `if (busy || !owned) return;` is the
  // mid-gesture half, which no click can reach while the markup holds, and is
  // held by `share_sweep.test.ts` instead.
  it('the sheet’s own writer refuses once this client is no longer the owner', async () => {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://fleet.example.com' });
    mockedList.mockResolvedValue({ ok: true, value: [grant()] });
    mockedUnshare.mockResolvedValue({ ok: true, value: null });
    // The owner first: revoking a grant reaches the store.
    setMyGrants(1, []);
    sessions.set([session('mefistos', 'dev-foo', { id: 42, owner_person_id: 1 })]);
    const own = render(ShareSheet);
    await settle();
    await fireEvent.click(screen.getByTestId('share-revoke'));
    await fireEvent.click(screen.getByTestId('share-revoke-yes'));
    await settle();
    expect(mockedUnshare).toHaveBeenCalled();
    mockedUnshare.mockClear();
    own.unmount();

    // …and a client that only holds a grant gets no controls and no call.
    setMyGrants(9, [{ session_id: 42, level: 'drive' }]);
    render(ShareSheet);
    await settle();
    expect(screen.queryByTestId('share-revoke')).toBeNull();
    expect(screen.getByTestId('share-not-owner')).toBeTruthy();
    expect(mockedUnshare).not.toHaveBeenCalled();
    expect(mockedNarrow).not.toHaveBeenCalled();
    expect(mockedShare).not.toHaveBeenCalled();
  });

  it('will not act on a grant the hub did not name', async () => {
    // `person` is what the sharing tools take. A grant with no name is still
    // SHOWN — the owner has to be able to see it — but its controls are
    // disabled rather than guessing a name the hub would refuse.
    mockedList.mockResolvedValue({
      ok: true,
      value: [grant({ person_name: null, person_display_name: null, level: 'drive' })],
    });
    render(ShareSheet);
    await settle();
    expect(screen.getByTestId('share-grant-who').textContent).toContain('#2');
    expect((screen.getByTestId('share-revoke') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('share-narrow') as HTMLButtonElement).disabled).toBe(true);
  });

  it('shows a failed read as an error, not as "shared with nobody"', async () => {
    mockedList.mockResolvedValue({
      ok: false,
      error: { code: 'E_HUB_UNREACHABLE', message: 'the hub is not answering' },
    });
    render(ShareSheet);
    await settle();
    expect(screen.getByTestId('share-list-error').textContent).toContain('not answering');
    expect(screen.queryByTestId('share-list-empty')).toBeNull();
  });

  it('renders nothing at all while no session is open on it', async () => {
    shareSheetFor.set(null);
    render(ShareSheet);
    await settle();
    expect(screen.queryByTestId('share-sheet')).toBeNull();
    expect(mockedList).not.toHaveBeenCalled();
  });
});
