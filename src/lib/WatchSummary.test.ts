// Orbit Fleet 11.11: "Since 13:20" runs only when asked, shows what checked
// it, says when the check hid it, and offers nothing without consent.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WatchSummary from './WatchSummary.svelte';
import { checkLabel, clock, defaultSince, type WatchSummary as Summary } from './watch_summary';

const viewed = new Date(2026, 9, 8, 13, 20).getTime() / 1000;
const session = { id: 7, last_viewed_at: viewed };

const ok: Summary = {
  text: 'Fixed the flaky test; the PR waits on review.',
  check: 'passed',
  since: viewed,
  turns: 3,
  model: 'haiku',
  host_alias: 'mercury',
  at: viewed + 600,
};

async function flush() {
  for (let i = 0; i < 5; i++) await tick();
}

function calls() {
  return vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'session_summary_since');
}

describe('WatchSummary', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(new Date(2026, 9, 8, 14, 0));
  });

  it('runs nothing on open and summarises since the last look when asked', async () => {
    vi.mocked(invoke).mockResolvedValue(ok);
    render(WatchSummary, { session });
    await flush();
    expect(calls()).toHaveLength(0);
    expect(screen.getByText('Since 13:20')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('watch-summary-run'));
    await flush();
    expect(calls()[0][1]).toEqual({ args: { session_id: 7, since: viewed } });
    expect(screen.getByTestId('watch-summary-text').textContent).toBe(ok.text);
    expect(screen.getByTestId('watch-summary-meta').textContent).toBe(
      'by haiku on mercury · from 3 turns · checked against the transcript by Jev',
    );
  });

  it('says when the check hid the summary', async () => {
    vi.mocked(invoke).mockResolvedValue({ ...ok, text: null, check: 'failed' });
    render(WatchSummary, { session });
    await fireEvent.click(screen.getByTestId('watch-summary-run'));
    await flush();
    expect(screen.queryByTestId('watch-summary-text')).toBeNull();
    expect(screen.getByTestId('watch-summary-hidden').textContent).toContain('does not match the transcript');
  });

  it('offers nothing without the org consent', async () => {
    vi.mocked(invoke).mockRejectedValue({
      code: 'E_FORBIDDEN',
      message: "this session's organisation has not consented to summaries",
    });
    render(WatchSummary, { session });
    await fireEvent.click(screen.getByTestId('watch-summary-run'));
    await flush();
    expect(screen.getByTestId('watch-summary-refused').textContent).toContain('not consented');
    expect(screen.queryByTestId('watch-summary-run')).toBeNull();
  });

  it('says when nothing happened', async () => {
    vi.mocked(invoke).mockResolvedValue({ ...ok, text: null, check: 'off', turns: 0 });
    render(WatchSummary, { session });
    await fireEvent.click(screen.getByTestId('watch-summary-run'));
    await flush();
    expect(screen.getByTestId('watch-summary-empty').textContent).toBe('Nothing happened since 13:20.');
  });
});

describe('watch_summary helpers', () => {
  it('defaults to the last look within a day, else an hour ago', () => {
    expect(defaultSince(900, 1_000)).toBe(900);
    expect(defaultSince(null, 100_000)).toBe(96_400);
    expect(defaultSince(1, 100_000)).toBe(96_400);
  });
  it('formats the clock and the check', () => {
    expect(clock(new Date(2026, 0, 1, 7, 5).getTime() / 1000)).toBe('07:05');
    expect(checkLabel('shadow')).toBe('not checked against the transcript');
  });
});
