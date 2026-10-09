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
    // An LLM draft carries the Drafted pill (design-system/ai.md).
    expect(screen.getByTestId('watch-summary-drafted').textContent).toBe('Drafted');
  });

  // Review r16 (r07 follow-up): a row event re-renders the block with the
  // same session as a new object (selectedSession re-notified on every
  // flush before #721); the summary must stay, only another session clears it.
  it('keeps the summary when the same session comes back as a new object', async () => {
    vi.mocked(invoke).mockResolvedValue(ok);
    const r = render(WatchSummary, { session });
    await fireEvent.click(screen.getByTestId('watch-summary-run'));
    await flush();
    expect(screen.getByTestId('watch-summary-text').textContent).toBe(ok.text);
    await r.rerender({ session: { ...session } });
    await flush();
    expect(screen.getByTestId('watch-summary-text').textContent).toBe(ok.text);
    await r.rerender({ session: { ...session, id: 8 } });
    await flush();
    expect(screen.queryByTestId('watch-summary-text')).toBeNull();
  });

  it('says the summary covers only the last turns when the window reaches past them', async () => {
    vi.mocked(invoke).mockResolvedValue({ ...ok, turns: 40, turns_capped: true });
    render(WatchSummary, { session });
    await fireEvent.click(screen.getByTestId('watch-summary-run'));
    await flush();
    expect(screen.getByTestId('watch-summary-meta').textContent).toBe(
      'by haiku on mercury · from the last 40 turns · checked against the transcript by Jev',
    );
  });

  it('marks a summary nobody checked as not checked, beside the Drafted pill', async () => {
    // decide.jev.summary_check is off by default: the text shows, labelled.
    vi.mocked(invoke).mockResolvedValue({ ...ok, check: 'off' });
    render(WatchSummary, { session });
    await fireEvent.click(screen.getByTestId('watch-summary-run'));
    await flush();
    expect(screen.getByTestId('watch-summary-text').textContent).toBe(ok.text);
    expect(screen.getByTestId('watch-summary-drafted').textContent).toBe('Drafted');
    expect(screen.getByTestId('watch-summary-unchecked').textContent).toBe('Not checked');
  });

  it('has no Not checked mark once Jev passed it', async () => {
    vi.mocked(invoke).mockResolvedValue(ok);
    render(WatchSummary, { session });
    await fireEvent.click(screen.getByTestId('watch-summary-run'));
    await flush();
    expect(screen.queryByTestId('watch-summary-unchecked')).toBeNull();
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
