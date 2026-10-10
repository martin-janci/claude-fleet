// Work graph M7.3: the Tidy up and Reopened entries — counts and colours,
// the sheet's preselection, actions and keyboard, and the reopened list.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TidyReview from './TidyReview.svelte';
import { EMPTY_REPORT, refreshTidy, reopenedLoads, reopenedWork, requestTidy, tidyReport, tidyRequest, type TidyCandidate } from './tidy';
import { toasts, runToastAction } from './toasts';
import { get } from 'svelte/store';
import { sessionFocus } from './session_focus';
import { sessions, type SessionRow } from './sessions';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { applyGrantChanges, resetAccessForTests, setMyGrants } from './access';
import { session as fixtureSession } from './hosts_fixture';
import { expectAccessible } from './a11y_check';

/** A desktop paired with a hub whose live link is down: every routed
 *  mutation (tidy_apply, dismiss_reopened) is blocked with a reason. */
const remote: HubStatus = {
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

const cand = (id: number, over: Partial<TidyCandidate> = {}): TidyCandidate => ({
  session_id: id,
  link_id: 100 + id,
  host_alias: 'h',
  tmux_name: `s${id}`,
  reason: 'done_idle',
  action: 'safe_kill',
  since: 0,
  idle_secs: 5 * 3600,
  key: `ABC-${id}`,
  item_status: 'Done',
  branch: `abc-${id}`,
  ...over,
});

let candidates: TidyCandidate[] = [];
let reopened: unknown[] = [];

beforeEach(() => {
  candidates = [];
  reopened = [];
  tidyReport.set(EMPTY_REPORT);
  reopenedWork.set([]);
  reopenedLoads.set(0);
  toasts.set([]);
  sessionFocus.set(null);
  sessions.set([]);
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  // Who this client is, forgotten between tests: since F2d an unresolvable
  // candidate is REFUSED on a paired desktop, so a leaked identity would change
  // what the next test's sheet offers.
  resetAccessForTests();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'work_tidy':
        return { ...EMPTY_REPORT, candidates };
      case 'work_reopened':
        return reopened;
      case 'tidy_apply':
        return { results: [{ session_id: 1, action: 'safe_kill', ok: true, outcome: 'killed' }] };
      case 'dismiss_reopened':
        return { dismissed: true };
      default:
        return null;
    }
  });
});

async function mount() {
  render(TidyReview);
  await waitFor(() =>
    expect(vi.mocked(invoke).mock.calls.some((c) => c[0] === 'work_reopened')).toBe(true),
  );
  await tick();
  await tick();
}

describe('TidyReview', () => {
  describe('Tidy › Duplicates: Jev\'s same work (redesign 6.9)', () => {
    const pair = () => {
      candidates = [cand(1), cand(2, { reason: 'same_work', link_id: null, key: null, same_as: 9 })];
      sessions.set([
        { id: 1, tmux_name: 's1', host_alias: 'h' },
        { id: 2, tmux_name: 's2', host_alias: 'h' },
        { id: 9, tmux_name: 's9', host_alias: 'h', friendly_name: 'payments retry' },
      ] as SessionRow[]);
    };

    it('groups it, names the kept session, marks Jev, never ticks it', async () => {
      pair();
      await mount();
      await fireEvent.click(await screen.findByTestId('tidy-pill'));
      await tick();
      expect(screen.getAllByTestId('tidy-group').map((g) => g.textContent)).toEqual([
        'Done and idle · 1',
        'Same work as another session · 1',
      ]);
      expect(screen.getByTestId('tidy-same-work')).toHaveTextContent('same work as payments retry');
      expect(screen.getByTestId('tidy-same-work-by')).toHaveTextContent('Proposed by Jev');
      const checks = screen.getAllByTestId('tidy-check') as HTMLInputElement[];
      expect(checks.map((c) => c.checked)).toEqual([true, false]);
      expect(screen.getByTestId('tidy-apply')).toHaveTextContent('Tidy 1');
    });

  });

  it('clicking a row shows only that session; its checkbox does not; Cancel lifts it', async () => {
    candidates = [cand(1), cand(2)];
    // The focus refuses a session the store does not have (a stale row).
    sessions.set(
      candidates.map((c) => ({ id: c.session_id, tmux_name: c.tmux_name, host_alias: c.host_alias }) as SessionRow),
    );
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    const row = screen.getAllByTestId('tidy-row')[1];
    await fireEvent.click(row.querySelector('[data-testid="tidy-check"]')!);
    expect(get(sessionFocus)).toBeNull();
    await fireEvent.click(row.querySelector('.name')!);
    expect(get(sessionFocus)).toEqual({ id: 2, label: 's2' });
    await fireEvent.click(screen.getByTestId('tidy-cancel'));
    await tick();
    expect(get(sessionFocus)).toBeNull();
  });

  it('a refused focus (stale row) never claims ownership, so closing keeps another focus', async () => {
    candidates = [cand(1), cand(2)];
    // Only session 1 is in the store; row 2 is a stale candidate.
    sessions.set([{ id: 1, tmux_name: 's1', host_alias: 'h' } as SessionRow]);
    sessionFocus.set({ id: 1, label: 'elsewhere' });
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    const stale = screen.getAllByTestId('tidy-row')[1];
    await fireEvent.click(stale.querySelector('.name')!);
    expect(get(sessionFocus)).toEqual({ id: 1, label: 'elsewhere' });
    await fireEvent.click(screen.getByTestId('tidy-cancel'));
    await tick();
    expect(get(sessionFocus)).toEqual({ id: 1, label: 'elsewhere' });
    sessionFocus.set(null);
  });

  it('j and k still move the cursor from a focused control', async () => {
    candidates = [cand(1), cand(2)];
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    const checks = screen.getAllByTestId('tidy-check') as HTMLInputElement[];
    checks[0].focus();
    const j = new KeyboardEvent('keydown', { key: 'j', bubbles: true, cancelable: true });
    checks[0].dispatchEvent(j);
    await tick();
    expect(j.defaultPrevented).toBe(true);
    // Space on the sheet toggles the cursor row, now row 1.
    await fireEvent.keyDown(screen.getByTestId('tidy-sheet'), { key: ' ' });
    expect(checks[0].checked).toBe(true);
    expect(checks[1].checked).toBe(false);
  });

  it('shows nothing when there is nothing to tidy or reopened', async () => {
    await mount();
    expect(screen.queryByTestId('tidy-pill')).toBeNull();
    expect(screen.queryByTestId('reopened-pill')).toBeNull();
  });

  it('a neutral "n to tidy" and an accent "n reopened"', async () => {
    candidates = [cand(1), cand(2)];
    reopened = [{ item_id: 7, key: 'PAY-7', title: 'Retry', reopened_at: 5, past_sessions: 2 }];
    await mount();
    const pill = await screen.findByTestId('tidy-pill');
    expect(pill).toHaveTextContent('2 to tidy');
    expect(pill.classList.contains('tidy-pill')).toBe(true);
    expect(pill.classList.contains('reopened-pill')).toBe(false);
    const r = screen.getByTestId('reopened-pill');
    expect(r).toHaveTextContent('1 reopened');
    expect(r.classList.contains('reopened-pill')).toBe(true);
  });

  it('preselects every row, groups by reason and applies from the keyboard', async () => {
    candidates = [
      cand(1),
      cand(2, { reason: 'pr_merged_idle' }),
      cand(3, { reason: 'ghost_expiring', action: 'resume_or_expire', expires_at: 9e9 }),
    ];
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    const sheet = screen.getByTestId('tidy-sheet');
    expect(screen.getAllByTestId('tidy-group').map((g) => g.textContent)).toEqual([
      'Done and idle · 1',
      'PR merged, idle · 1',
      'Lost session about to expire · 1',
    ]);
    const checks = screen.getAllByTestId('tidy-check') as HTMLInputElement[];
    expect(checks.map((c) => c.checked)).toEqual([true, true, false]);
    expect(screen.getByTestId('tidy-apply')).toHaveTextContent('Tidy 2');
    // j moves to the second row, space unticks it, Enter applies the rest.
    await fireEvent.keyDown(sheet, { key: 'j' });
    await fireEvent.keyDown(sheet, { key: ' ' });
    await tick();
    expect(screen.getByTestId('tidy-apply')).toHaveTextContent('Tidy 1');
    await fireEvent.keyDown(sheet, { key: 'Enter' });
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('tidy_apply', {
        args: { items: [{ session_id: 1, action: 'safe_kill', link_id: 101 }] },
      }),
    );
  });

  it('says what the ticked safe kills free, and follows the ticks and choices (G1.9)', async () => {
    const GB = 1024 * 1024;
    candidates = [
      cand(1, { worktree_kb: 1.5 * GB }),
      cand(2, { reason: 'pr_merged_idle', worktree_kb: 0.6 * GB }),
      cand(3, { reason: 'pr_merged_idle' }),
    ];
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    expect(screen.getByTestId('tidy-frees')).toHaveTextContent('3 selected · frees about 2.1 GB');
    // Archive keeps the tree: it frees nothing.
    const selects = screen.getAllByTestId('tidy-choice') as HTMLSelectElement[];
    await fireEvent.change(selects[1], { target: { value: 'archive' } });
    await tick();
    expect(screen.getByTestId('tidy-frees')).toHaveTextContent('3 selected · frees about 1.5 GB');
    // Unticking the only measured safe kill left: nothing measured, nothing said.
    const sheet = screen.getByTestId('tidy-sheet');
    await fireEvent.keyDown(sheet, { key: ' ' });
    await tick();
    expect(screen.queryByTestId('tidy-frees')).toBeNull();
  });

  it('the done toast says what the safe kills that went through free (G4.8)', async () => {
    const GB = 1024 * 1024;
    candidates = [cand(1, { worktree_kb: 1.5 * GB }), cand(2, { worktree_kb: 0.6 * GB }), cand(3, { worktree_kb: 9 * GB })];
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_tidy') return { ...EMPTY_REPORT, candidates };
      if (cmd === 'work_reopened') return reopened;
      if (cmd === 'tidy_apply')
        return {
          results: [
            { session_id: 1, action: 'safe_kill', ok: true, outcome: 'killed' },
            { session_id: 2, action: 'safe_kill', ok: true, outcome: 'safe_kill_requested' },
            // Refused: its tree stays, so its size is not counted.
            { session_id: 3, action: 'safe_kill', ok: false, error: 'the tree is dirty' },
          ],
        };
      return null;
    });
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    await fireEvent.click(screen.getByTestId('tidy-apply'));
    await waitFor(() => expect(get(toasts)).toHaveLength(1));
    expect(get(toasts)[0].message).toBe('Tidied 2; 1 failed: the tree is dirty');
    expect(get(toasts)[0].sub).toBe('frees about 2.1 GB');
  });

  it('the done toast has no second line when nothing measured went', async () => {
    candidates = [cand(1)];
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    await fireEvent.click(screen.getByTestId('tidy-apply'));
    await waitFor(() => expect(get(toasts)).toHaveLength(1));
    expect(get(toasts)[0].message).toBe('Tidied 1 session');
    expect(get(toasts)[0].sub).toBeUndefined();
  });

  it('a per-row choice changes the action sent', async () => {
    candidates = [cand(1)];
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    const select = screen.getByTestId('tidy-choice') as HTMLSelectElement;
    expect(Array.from(select.options, (o) => o.textContent)).toEqual([
      'Safe kill',
      'Archive only',
      'Snooze 7 d',
      'Never for this work',
    ]);
    await fireEvent.change(select, { target: { value: 'archive' } });
    await fireEvent.click(screen.getByTestId('tidy-apply'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('tidy_apply', {
        args: { items: [{ session_id: 1, action: 'archive', link_id: 101 }] },
      }),
    );
  });

  it('Enter on a focused control (Cancel, checkbox, PR link) is not a sheet chord', async () => {
    candidates = [cand(1, { pr_url: 'https://example.com/pr/1' }), cand(2)];
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    const sheet = screen.getByTestId('tidy-sheet');
    const chord = (key: string) =>
      new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
    // Enter on Cancel must reach the button (bubbling keydown, like a real key
    // press), not apply the tidy with every ticked row.
    const cancel = screen.getByTestId('tidy-cancel');
    cancel.focus();
    const onCancel = chord('Enter');
    cancel.dispatchEvent(onCancel);
    await tick();
    expect(onCancel.defaultPrevented).toBe(false);
    expect(screen.getByTestId('tidy-sheet')).toBeTruthy();
    // Enter on the PR link opens the link, not the tidy.
    const link = sheet.querySelector('a')!;
    const onLink = chord('Enter');
    link.dispatchEvent(onLink);
    expect(onLink.defaultPrevented).toBe(false);
    // Space on the second row's checkbox is its own toggle (cursor is on row
    // 0, which stays as it was), and Enter there does not apply either.
    const checks = screen.getAllByTestId('tidy-check') as HTMLInputElement[];
    checks[1].focus();
    const onCheck = chord(' ');
    checks[1].dispatchEvent(onCheck);
    await tick();
    expect(onCheck.defaultPrevented).toBe(false);
    expect(checks[0].checked).toBe(true);
    expect(screen.getByTestId('tidy-apply')).toHaveTextContent('Tidy 2');
    const onCheckEnter = chord('Enter');
    checks[1].dispatchEvent(onCheckEnter);
    expect(onCheckEnter.defaultPrevented).toBe(false);
    expect(vi.mocked(invoke).mock.calls.some((c) => c[0] === 'tidy_apply')).toBe(false);
    // Escape from a focused control still closes the sheet (never destructive).
    checks[1].focus();
    const onEsc = chord('Escape');
    checks[1].dispatchEvent(onEsc);
    await tick();
    expect(onEsc.defaultPrevented).toBe(true);
    expect(screen.queryByTestId('tidy-sheet')).toBeNull();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    // From the sheet itself the chord still applies.
    await fireEvent.keyDown(screen.getByTestId('tidy-sheet'), { key: 'Enter' });
    await waitFor(() =>
      expect(vi.mocked(invoke).mock.calls.some((c) => c[0] === 'tidy_apply')).toBe(true),
    );
  });

  it('closing the sheet hands focus back to the pill (review r11)', async () => {
    candidates = [cand(1)];
    await mount();
    const pill = await screen.findByTestId('tidy-pill');
    pill.focus();
    await fireEvent.click(pill);
    await tick();
    expect(document.activeElement).toBe(screen.getByTestId('tidy-sheet'));
    await fireEvent.keyDown(screen.getByTestId('tidy-sheet'), { key: 'Escape' });
    await tick();
    expect(screen.queryByTestId('tidy-sheet')).toBeNull();
    expect(document.activeElement).toBe(screen.getByTestId('tidy-pill'));
  });

  it('Cancel closes the sheet without applying', async () => {
    candidates = [cand(1)];
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    await fireEvent.click(screen.getByTestId('tidy-cancel'));
    await tick();
    expect(screen.queryByTestId('tidy-sheet')).toBeNull();
    expect(vi.mocked(invoke).mock.calls.some((c) => c[0] === 'tidy_apply')).toBe(false);
  });

  it('lists reopened work with its past sessions, Resume and Dismiss', async () => {
    reopened = [{ item_id: 7, key: 'PAY-7', title: 'Retry', reopened_at: 5, past_sessions: 2 }];
    await mount();
    await fireEvent.click(await screen.findByTestId('reopened-pill'));
    await tick();
    const row = screen.getByTestId('reopened-row');
    expect(row).toHaveTextContent('PAY-7');
    expect(screen.getByTestId('reopened-badge')).toHaveTextContent('reopened · 2 past sessions');
    expect(row.querySelector('[data-testid="resume-button"]')).not.toBeNull();
    await fireEvent.click(screen.getByTestId('reopened-dismiss'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('dismiss_reopened', { args: { item_id: 7 } }),
    );
  });

  it('a newly reopened item toasts once, and Show opens the list', async () => {
    await mount();
    await waitFor(() => expect(get(reopenedLoads)).toBe(1));
    // The first read is the baseline (nothing reopened); the next brings one.
    reopened = [{ item_id: 7, key: 'PAY-7', title: 'Retry', reopened_at: 5, past_sessions: 2 }];
    await refreshTidy();
    await tick();
    const t = get(toasts);
    expect(t.map((x) => x.message)).toEqual(['PAY-7 reopened · 2 past sessions']);
    expect(t[0].action?.label).toBe('Show');
    runToastAction(t[0].id);
    await tick();
    expect(screen.getByTestId('reopened-list')).toBeTruthy();
    expect(screen.getByTestId('reopened-row')).toHaveTextContent('PAY-7');
    // The same item is not announced again on the next read.
    await refreshTidy();
    await tick();
    expect(get(toasts)).toEqual([]);
  });

  it('a tidy that partly failed says which, and the sheet closes', async () => {
    candidates = [cand(1), cand(2)];
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_tidy') return { ...EMPTY_REPORT, candidates };
      if (cmd === 'work_reopened') return reopened;
      if (cmd === 'tidy_apply')
        return {
          results: [
            { session_id: 1, action: 'safe_kill', ok: true, outcome: 'killed' },
            { session_id: 2, action: 'safe_kill', ok: false, error: 'the tree is dirty' },
          ],
        };
      return null;
    });
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    await fireEvent.click(screen.getByTestId('tidy-apply'));
    await waitFor(() => expect(screen.queryByTestId('tidy-sheet')).toBeNull());
    expect(get(toasts).map((x) => [x.kind, x.message])).toEqual([
      ['error', 'Tidied 1; 1 failed: the tree is dirty'],
    ]);
  });

  it('a tidy the hub refuses is a toast, and the sheet stays open with its ticks', async () => {
    candidates = [cand(1), cand(2)];
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_tidy') return { ...EMPTY_REPORT, candidates };
      if (cmd === 'work_reopened') return reopened;
      if (cmd === 'tidy_apply') throw { code: 'E_HUB', message: 'hub unreachable' };
      return null;
    });
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    await fireEvent.click(screen.getByTestId('tidy-apply'));
    await waitFor(() =>
      expect(get(toasts).map((x) => x.message)).toEqual([
        expect.stringMatching(/^Tidy up failed: hub unreachable/),
      ]),
    );
    expect(screen.getByTestId('tidy-sheet')).toBeTruthy();
    const checks = screen.getAllByTestId('tidy-check') as HTMLInputElement[];
    expect(checks.map((c) => c.checked)).toEqual([true, true]);
    expect((screen.getByTestId('tidy-apply') as HTMLButtonElement).disabled).toBe(false);
  });

  it('a request from the Today view opens the sheet with just those sessions ticked (M9)', async () => {
    candidates = [cand(1), cand(2), cand(3, { reason: 'pr_merged_idle' })];
    await mount();
    requestTidy([3]);
    await waitFor(() => expect(screen.getByTestId('tidy-sheet')).toBeTruthy());
    // M10.4: the sheet also shows only the requested session.
    const checks = screen.getAllByTestId('tidy-check') as HTMLInputElement[];
    expect(checks.map((c) => c.checked)).toEqual([true]);
    expect(get(tidyRequest)).toBeNull();
    expect(invoke).not.toHaveBeenCalledWith('tidy_apply', expect.anything());
  });

  it('a request from the Today view shows only those sessions until Show all (M10.4)', async () => {
    candidates = [cand(1), cand(2), cand(3, { reason: 'pr_merged_idle' })];
    await mount();
    requestTidy([2, 3]);
    await waitFor(() => expect(screen.getByTestId('tidy-sheet')).toBeTruthy());
    const rows = () => screen.getAllByTestId('tidy-row').map((r) => Number(r.dataset.sessionId));
    expect(rows()).toEqual([2, 3]);
    expect(screen.getByTestId('tidy-only')).toHaveTextContent("From Today's Stale · 2 of 3");
    await fireEvent.click(screen.getByTestId('tidy-show-all'));
    expect(rows()).toEqual([1, 2, 3]);
    expect(screen.queryByTestId('tidy-only')).toBeNull();
    // Showing all ticks nothing more: only the requested rows stay picked.
    const checks = screen.getAllByTestId('tidy-check') as HTMLInputElement[];
    expect(checks.map((c) => c.checked)).toEqual([false, true, true]);
    expect(invoke).not.toHaveBeenCalledWith('tidy_apply', expect.anything());
  });

  it('Enter on Show all never applies (M10.4)', async () => {
    candidates = [cand(1), cand(2)];
    await mount();
    requestTidy([2]);
    await waitFor(() => expect(screen.getByTestId('tidy-show-all')).toBeTruthy());
    await fireEvent.keyDown(screen.getByTestId('tidy-show-all'), { key: 'Enter' });
    await tick();
    expect(invoke).not.toHaveBeenCalledWith('tidy_apply', expect.anything());
  });

  it('the pill after a Today request opens the whole sheet again (M10.4)', async () => {
    candidates = [cand(1), cand(2)];
    await mount();
    requestTidy([2]);
    await waitFor(() => expect(screen.getAllByTestId('tidy-row')).toHaveLength(1));
    await fireEvent.keyDown(screen.getByTestId('tidy-sheet'), { key: 'Escape' });
    await fireEvent.click(screen.getByTestId('tidy-pill'));
    await waitFor(() => expect(screen.getAllByTestId('tidy-row')).toHaveLength(2));
  });

  it('a request none of whose sessions is still a candidate shows everything (M10.4)', async () => {
    candidates = [cand(1), cand(2)];
    await mount();
    requestTidy([99]);
    await waitFor(() => expect(screen.getByTestId('tidy-sheet')).toBeTruthy());
    expect(screen.getAllByTestId('tidy-row')).toHaveLength(2);
    expect(screen.queryByTestId('tidy-only')).toBeNull();
  });

  it('an old request is dropped instead of opening the sheet later', async () => {
    candidates = [cand(1)];
    requestTidy([1], Date.now() - 60_000);
    await mount();
    await tick();
    expect(screen.queryByTestId('tidy-sheet')).toBeNull();
    expect(get(tidyRequest)).toBeNull();
  });
  it('a hub that cannot be reached blocks the sheet: the note, a disabled Tidy, no chord, and no Dismiss', async () => {
    candidates = [cand(1), cand(2)];
    reopened = [{ item_id: 7, key: 'PAY-7', title: 'Retry', reopened_at: 5, past_sessions: 2 }];
    hubStatus.set(remote);
    hubConnection.set({ state: 'offline', attempt: 4, retry_in_secs: 30, reason: 'connect refused' });
    // This test is about the HUB half alone, so the access half is satisfied:
    // the rows are in the store and they are this person's. Without them F2d's
    // fail-closed rule would refuse both candidates for a second, different
    // reason and the assertion below would stop measuring what it names.
    sessions.set(
      candidates.map((c) =>
        fixtureSession(c.host_alias, c.tmux_name, { id: c.session_id, owner_person_id: 1 }),
      ),
    );
    setMyGrants(1, []);
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    const sheet = screen.getByTestId('tidy-sheet');
    const note = screen.getByRole('note');
    expect(note.textContent).toContain('https://fleet.example.com is unreachable right now');
    // The rows are still listed and ticked — only the sending is off.
    const checks = screen.getAllByTestId('tidy-check') as HTMLInputElement[];
    expect(checks.map((c) => c.checked)).toEqual([true, true]);
    expect((screen.getByTestId('tidy-apply') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.keyDown(sheet, { key: 'Enter' });
    await tick();
    expect(invoke).not.toHaveBeenCalledWith('tidy_apply', expect.anything());
    expect(screen.getByTestId('tidy-sheet')).toBeTruthy();
    // Cancel still works: closing is never a hub call.
    await fireEvent.click(screen.getByTestId('tidy-cancel'));
    await tick();
    expect(screen.queryByTestId('tidy-sheet')).toBeNull();
    // The reopened list's Dismiss is a routed mutation too.
    await fireEvent.click(screen.getByTestId('reopened-pill'));
    await tick();
    expect((screen.getByTestId('reopened-dismiss') as HTMLButtonElement).disabled).toBe(true);
    // Once the link is back, the same sheet sends again.
    hubConnection.set({ state: 'connected' });
    await tick();
    await fireEvent.click(screen.getByTestId('tidy-pill'));
    await tick();
    expect(screen.queryByRole('note')).toBeNull();
    expect((screen.getByTestId('tidy-apply') as HTMLButtonElement).disabled).toBe(false);
  });

  it('re-reads the candidates every minute while mounted, and not after unmount', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    try {
      candidates = [cand(1)];
      const { unmount } = render(TidyReview);
      const flush = async () => {
        for (let i = 0; i < 8; i++) await tick();
      };
      await flush();
      const reads = () => vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'work_tidy').length;
      expect(reads()).toBe(1);
      // Nothing before the interval elapses.
      vi.advanceTimersByTime(59_000);
      await flush();
      expect(reads()).toBe(1);
      vi.advanceTimersByTime(1_000);
      await flush();
      expect(reads()).toBe(2);
      // The re-read is what the pill shows: a candidate that went away drops it.
      candidates = [];
      vi.advanceTimersByTime(60_000);
      await flush();
      expect(reads()).toBe(3);
      expect(screen.queryByTestId('tidy-pill')).toBeNull();
      unmount();
      vi.advanceTimersByTime(180_000);
      await flush();
      expect(reads()).toBe(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it('the Reopened pill closes the tidy sheet and toggles its own list; the Tidy pill closes the list', async () => {
    candidates = [cand(1)];
    reopened = [{ item_id: 7, key: 'PAY-7', title: 'Retry', reopened_at: 5, past_sessions: 2 }];
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    expect(screen.getByTestId('tidy-sheet')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('reopened-pill'));
    await tick();
    expect(screen.queryByTestId('tidy-sheet')).toBeNull();
    expect(screen.getByTestId('reopened-list')).toBeTruthy();
    // The pill is a toggle; so is the list's own close.
    await fireEvent.click(screen.getByTestId('reopened-pill'));
    await tick();
    expect(screen.queryByTestId('reopened-list')).toBeNull();
    await fireEvent.click(screen.getByTestId('reopened-pill'));
    await tick();
    expect(screen.getByTestId('reopened-list')).toBeTruthy();
    await fireEvent.click(screen.getByText('Close'));
    await tick();
    expect(screen.queryByTestId('reopened-list')).toBeNull();
    // Opening the tidy sheet puts the list away, so only one sheet shows.
    await fireEvent.click(screen.getByTestId('reopened-pill'));
    await tick();
    await fireEvent.click(screen.getByTestId('tidy-pill'));
    await tick();
    expect(screen.getByTestId('tidy-sheet')).toBeTruthy();
    expect(screen.queryByTestId('reopened-list')).toBeNull();
    expect(invoke).not.toHaveBeenCalledWith('tidy_apply', expect.anything());
    expect(invoke).not.toHaveBeenCalledWith('dismiss_reopened', expect.anything());
  });

  describe('idle, no work linked (work graph M11.3)', () => {
    const lonely = () =>
      cand(7, {
        link_id: null,
        key: null,
        item_status: null,
        reason: 'idle_unlinked',
        action: 'safe_kill',
        since: Math.floor(Date.now() / 1000) - 9 * 86_400 - 60,
        idle_secs: 9 * 86_400,
      });

    it('shows the reason, its evidence, starts unticked, and offers Keep and Safe kill', async () => {
      candidates = [cand(1), lonely()];
      await mount();
      await fireEvent.click(await screen.findByTestId('tidy-pill'));
      await tick();
      expect(screen.getAllByTestId('tidy-group').map((g) => g.textContent)).toEqual([
        'Done and idle · 1',
        'Idle, no work linked · 1',
      ]);
      const row = screen.getAllByTestId('tidy-row')[1];
      expect(row.querySelector('[data-testid="tidy-evidence"]')).toHaveTextContent('idle 9 d · no work linked');
      expect(row).toHaveTextContent('only if clean & pushed');
      expect(row).not.toHaveTextContent('commits & pushes first');
      const checks = screen.getAllByTestId('tidy-check') as HTMLInputElement[];
      expect(checks.map((c) => c.checked)).toEqual([true, false]);
      const select = row.querySelector('[data-testid="tidy-choice"]') as HTMLSelectElement;
      expect(Array.from(select.options, (o) => o.textContent)).toEqual(['Safe kill', 'Keep 7 d']);
      expect(row.querySelector('[data-testid="tidy-keep"]')).toHaveTextContent('Keep 7 d');
      expect(row.querySelector('[data-testid="tidy-safe-kill"]')).toHaveTextContent('Safe kill');
    });

    it('Keep sends a per-session keep for 7 days', async () => {
      candidates = [lonely()];
      await mount();
      await fireEvent.click(await screen.findByTestId('tidy-pill'));
      await tick();
      await fireEvent.click(screen.getByTestId('tidy-keep'));
      await waitFor(() =>
        expect(invoke).toHaveBeenCalledWith('tidy_apply', {
          args: { items: [{ session_id: 7, action: 'keep', days: 7 }] },
        }),
      );
    });

    it('Safe kill arms on the first click and acts on the second', async () => {
      candidates = [lonely()];
      await mount();
      await fireEvent.click(await screen.findByTestId('tidy-pill'));
      await tick();
      const kill = screen.getByTestId('tidy-safe-kill');
      await fireEvent.click(kill);
      await tick();
      expect(kill).toHaveTextContent('Confirm safe kill');
      expect(invoke).not.toHaveBeenCalledWith('tidy_apply', expect.anything());
      await fireEvent.click(kill);
      await waitFor(() =>
        expect(invoke).toHaveBeenCalledWith('tidy_apply', {
          args: { items: [{ session_id: 7, action: 'safe_kill' }] },
        }),
      );
    });

    it('Enter with nothing ticked applies nothing', async () => {
      candidates = [lonely()];
      await mount();
      await fireEvent.click(await screen.findByTestId('tidy-pill'));
      await tick();
      await fireEvent.keyDown(screen.getByTestId('tidy-sheet'), { key: 'Enter' });
      await tick();
      expect(invoke).not.toHaveBeenCalledWith('tidy_apply', expect.anything());
    });
  });
});

// ── Multi-user M1 (F2a): Tidy up can safe-kill, so it is the owner's ────────
//
// `blocked` here was `hubActionBlocked('tidy_apply', …)` and that was the whole
// gate on `applyRow` and `apply` — a live write path, so this was one of the
// task's two blockers: Tidy up could SAFE-KILL a session this client does not
// own. `tidy_apply` is `own` in `share.ts::SESSION_TIER` because
// `safe_kill_session` is.
//
// A `TidyCandidate` carries a session id, a host and a tmux name and no
// `owner_person_id`, so the sheet resolves the row out of `$sessions` first —
// the lookup that made every indirect surface get skipped by F2.
describe('TidyReview access gate (multi-user M1)', () => {
  const paired = () => {
    hubStatus.set({ ...remote });
    hubConnection.set({ state: 'connected' });
  };
  /** A candidate plus the session row it names, owned by `owner`. */
  function withRow(id: number, owner: number | null, over: Partial<TidyCandidate> = {}) {
    const c = cand(id, over);
    const row = fixtureSession(c.host_alias, c.tmux_name, {
      id,
      status: 'running',
      visibility: owner === null ? 'unclaimed' : 'private',
      owner_person_id: owner,
    }) as SessionRow;
    return { c, row };
  }
  const dis = (el: Element) => (el as HTMLButtonElement | HTMLInputElement).disabled;

  beforeEach(() => {
    resetAccessForTests();
  });

  it('the owner keeps the whole sheet: ticks, Tidy n and the row buttons', async () => {
    // The positive control. Without it, a gate that disabled everything for
    // everybody would pass every assertion below.
    const mine = withRow(7, 1, { reason: 'idle_unlinked', action: 'safe_kill' });
    candidates = [mine.c];
    sessions.set([mine.row]);
    paired();
    setMyGrants(1, []);
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    expect(dis(screen.getByTestId('tidy-check'))).toBe(false);
    expect(dis(screen.getByTestId('tidy-keep'))).toBe(false);
    expect(dis(screen.getByTestId('tidy-safe-kill'))).toBe(false);
    expect(screen.queryByTestId('tidy-not-mine')).toBeNull();
    await fireEvent.click(screen.getByTestId('tidy-check'));
    await tick();
    expect(dis(screen.getByTestId('tidy-apply'))).toBe(false);
    expect(screen.getByTestId('tidy-apply')).toHaveTextContent('Tidy 1');
  });

  it('a drive grantee cannot safe-kill the owner’s session from the sheet', async () => {
    // `drive` and not only `watch`: `tidy_apply` is the `own` tier, so a
    // driver is barred too. The reason a gate on `watch` alone would be wrong.
    const theirs = withRow(8, 42, { reason: 'idle_unlinked', action: 'safe_kill' });
    candidates = [theirs.c];
    sessions.set([theirs.row]);
    paired();
    setMyGrants(9, [{ session_id: 8, level: 'drive' }]);
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    expect(dis(screen.getByTestId('tidy-check'))).toBe(true);
    expect(dis(screen.getByTestId('tidy-keep'))).toBe(true);
    expect(dis(screen.getByTestId('tidy-safe-kill'))).toBe(true);
    expect(screen.getByTestId('tidy-safe-kill').title).toMatch(/only the session’s owner/i);
    expect(screen.getByTestId('tidy-not-mine')).toBeInTheDocument();
    // And the two write paths refuse even when reached directly: Safe kill is
    // two clicks, and ↵ applies from anywhere in the sheet.
    await fireEvent.click(screen.getByTestId('tidy-safe-kill'));
    await fireEvent.click(screen.getByTestId('tidy-safe-kill'));
    await fireEvent.keyDown(screen.getByTestId('tidy-sheet'), { key: 'Enter' });
    await tick();
    expect(invoke).not.toHaveBeenCalledWith('tidy_apply', expect.anything());
  });

  it('a mixed sheet applies only the owner’s rows — narrowed per target', async () => {
    // The fan-out half of the rule: one answer for the whole sheet would
    // either kill somebody else's session or refuse the owner's own.
    const mine = withRow(10, 1);
    const theirs = withRow(11, 42);
    candidates = [mine.c, theirs.c];
    sessions.set([mine.row, theirs.row]);
    paired();
    setMyGrants(1, [{ session_id: 11, level: 'watch' }]);
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    // Both are preselected candidates; only the owner's is ticked.
    expect(screen.getByTestId('tidy-apply')).toHaveTextContent('Tidy 1');
    expect(screen.getByTestId('tidy-not-mine').textContent).toMatch(/1 session/);
    await fireEvent.click(screen.getByTestId('tidy-apply'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('tidy_apply', {
        args: { items: [{ session_id: 10, action: 'safe_kill', link_id: 110 }] },
      }),
    );
  });

  it('a revoke while the sheet is open disables the row with no row event at all', async () => {
    // A grant moves no column on any session, so nothing a `session:updated`
    // could carry has changed here — the derivation is what re-answers.
    const theirs = withRow(12, 42, { reason: 'idle_unlinked', action: 'safe_kill' });
    candidates = [theirs.c];
    sessions.set([theirs.row]);
    paired();
    setMyGrants(9, [{ session_id: 12, level: 'drive' }]);
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    expect(dis(screen.getByTestId('tidy-safe-kill'))).toBe(true);
    // (already blocked at drive; narrowing to watch keeps it blocked, and a
    // revoke changes the sentence rather than the answer)
    applyGrantChanges([{ session_id: 12, person_id: 9, level: null }]);
    await tick();
    expect(dis(screen.getByTestId('tidy-safe-kill'))).toBe(true);
    expect(screen.getByTestId('tidy-safe-kill').title).toMatch(/belongs to someone else/i);
  });

  it('standalone is untouched: every row stays the owner’s', async () => {
    const mine = withRow(13, 1, { reason: 'idle_unlinked', action: 'safe_kill' });
    candidates = [mine.c];
    sessions.set([mine.row]);
    // No hub, no `my_grants` answer at all — `access.ts`'s rule 1.
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    expect(dis(screen.getByTestId('tidy-safe-kill'))).toBe(false);
    expect(screen.queryByTestId('tidy-not-mine')).toBeNull();
  });
});

describe('TidyReview accessibility (7.2)', () => {
  it('passes the axe and audit checks with the review open', async () => {
    candidates = [cand(1), cand(2)];
    sessions.set(
      candidates.map((c) => ({ id: c.session_id, tmux_name: c.tmux_name, host_alias: c.host_alias }) as SessionRow),
    );
    await mount();
    await fireEvent.click(await screen.findByTestId('tidy-pill'));
    await tick();
    await expectAccessible(document.body);
  });
});
