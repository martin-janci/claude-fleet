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

  it('a neutral "Tidy up · n" and an accent "Reopened · n"', async () => {
    candidates = [cand(1), cand(2)];
    reopened = [{ item_id: 7, key: 'PAY-7', title: 'Retry', reopened_at: 5, past_sessions: 2 }];
    await mount();
    const pill = await screen.findByTestId('tidy-pill');
    expect(pill).toHaveTextContent('Tidy up · 2');
    expect(pill.classList.contains('tidy-pill')).toBe(true);
    expect(pill.classList.contains('reopened-pill')).toBe(false);
    const r = screen.getByTestId('reopened-pill');
    expect(r).toHaveTextContent('Reopened · 1');
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
});
