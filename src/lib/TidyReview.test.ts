// Work graph M7.3: the Tidy up and Reopened entries — counts and colours,
// the sheet's preselection, actions and keyboard, and the reopened list.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TidyReview from './TidyReview.svelte';
import { EMPTY_REPORT, reopenedLoads, reopenedWork, tidyReport, type TidyCandidate } from './tidy';
import { toasts } from './toasts';

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
});
