// Redesign 1.2: one quiet attention line ("2 links to review · 1 to tidy ·
// 1 reopened") replaces the link bar and the Tidy chips, and no control in
// either sheet is a native, unstyled button (`.pill` was never defined there).
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import LinkReview from './LinkReview.svelte';
import TidyReview from './TidyReview.svelte';
import { EMPTY_REPORT, reopenedLoads, reopenedWork, tidyReport, type TidyCandidate } from './tidy';
import { sessions, type SessionWork } from './sessions';
import { session } from './hosts_fixture';
import { hubStatus, STANDALONE } from './hub';
import { hubConnection } from './hub_connection';
import { resetAccessForTests } from './access';

const sg = (link_id: number, key: string): SessionWork => ({
  link_id,
  item_id: null,
  key,
  title: '',
  source: 'branch',
  state: 'suggested',
  rule: 'R3b',
});

const cand: TidyCandidate = {
  session_id: 9,
  link_id: 109,
  host_alias: 'h',
  tmux_name: 's9',
  reason: 'done_idle',
  action: 'safe_kill',
  since: 0,
  idle_secs: 5 * 3600,
  key: 'ABC-9',
  item_status: 'Done',
  branch: 'abc-9',
};

beforeEach(() => {
  tidyReport.set(EMPTY_REPORT);
  reopenedWork.set([]);
  reopenedLoads.set(0);
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  resetAccessForTests();
  sessions.set([
    session('h', 'a', { id: 1, status: 'running', work_suggested: sg(11, 'ABC-1') }),
    session('h', 'b', { id: 2, status: 'running', work_suggested: sg(12, 'ABC-2') }),
  ]);
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === 'work_tidy') return { ...EMPTY_REPORT, candidates: [cand] };
    if (cmd === 'work_reopened')
      return [{ item_id: 5, key: 'ABC-5', title: 'Came back', past_sessions: 1, status_name: 'In progress' }];
    return null;
  });
});

describe('the attention line (redesign 1.2)', () => {
  it('reads as one line of segments, in the order links, tidy, reopened', async () => {
    // Mounted side by side, as SidebarFilters' .attention-line holds them.
    const host = document.createElement('div');
    host.className = 'attention-line';
    document.body.appendChild(host);
    render(LinkReview, { target: host });
    render(TidyReview, { target: host });
    await waitFor(() => expect(screen.getByTestId('reopened-pill')).toBeInTheDocument());
    await tick();
    const segs = Array.from(host.querySelectorAll('.al-seg')).map((b) => b.textContent);
    expect(segs).toEqual(['2 links to review', '1 to tidy', '1 reopened']);
  });

  it('no control in the link or tidy sheet is a bare native button', async () => {
    render(LinkReview);
    render(TidyReview);
    await waitFor(() => expect(screen.getByTestId('tidy-pill')).toBeInTheDocument());
    await fireEvent.click(screen.getByTestId('link-review-pill'));
    await fireEvent.click(screen.getByTestId('tidy-pill'));
    await tick();
    const buttons = [
      ...Array.from(screen.getByTestId('link-review-sheet').querySelectorAll('button')),
      // The Reopened group's Resume is its own split control (ResumeButton).
      ...Array.from(screen.getByTestId('tidy-sheet').querySelectorAll('button')).filter((b) => !b.closest('.resume')),
    ];
    expect(buttons.length).toBeGreaterThan(3);
    for (const b of buttons) {
      expect(b.classList.contains('pill')).toBe(false);
      expect(b.classList.contains('btn')).toBe(true);
    }
  });
});

