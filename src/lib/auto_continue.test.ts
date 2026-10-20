// Control's chat goes on with the recommended option: the fleet runs the
// clock (service::control_continue); the bar shows it and settles it.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import AutoContinueBar from './AutoContinueBar.svelte';
import { countdown, liveCountdown, type AutoContinueView, type PendingPick } from './auto_continue';

const NOW = Date.parse('2026-10-11T12:00:00Z');
const NOW_S = NOW / 1000;

const pick = (over: Partial<PendingPick> = {}): PendingPick => ({
  session_id: 7,
  turn_seq: 4,
  label: 'Open a PR',
  prompt: 'Open a draft PR',
  ended_at: NOW_S - 60,
  state: 'waiting',
  ...over,
});

const view = (over: Partial<AutoContinueView> = {}, p: PendingPick | null = pick()): AutoContinueView => ({
  enabled: true,
  minutes: 2,
  pending: p,
  due_at: p ? p.ended_at + 120 : null,
  ...over,
});

describe('liveCountdown', () => {
  it('draws only a waiting card of this chat while it may run', () => {
    expect(liveCountdown(view(), 7)?.dueAt).toBe(NOW_S + 60);
    expect(liveCountdown(view(), 8)).toBeNull();
    expect(liveCountdown(view({ enabled: false }), 7)).toBeNull();
    expect(liveCountdown(view({}, pick({ state: 'sent' })), 7)).toBeNull();
    expect(liveCountdown(view({}, null), 7)).toBeNull();
    expect(liveCountdown(null, 7)).toBeNull();
  });
});

describe('countdown', () => {
  it('is m:ss, never negative', () => {
    expect(countdown(9 * 60_000 + 41_000)).toBe('9:41');
    expect(countdown(500)).toBe('0:01');
    expect(countdown(-5)).toBe('0:00');
  });
});

describe('AutoContinueBar', () => {
  const mocked = vi.mocked(invoke);
  let state: AutoContinueView;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
    state = view();
    mocked.mockReset();
    mocked.mockImplementation(async (_cmd, args) => {
      const a = args as { action: string; turnSeq: number | null };
      if (a.action === 'status') return state;
      const was = state.pending;
      const settled = was && was.state === 'waiting' && was.turn_seq === a.turnSeq;
      if (settled) state = { ...state, pending: { ...was, state: a.action === 'take' ? 'taken' : 'cancelled' } };
      return a.action === 'take' ? { ...state, pending: settled ? was : null } : state;
    });
  });
  afterEach(() => vi.useRealTimers());

  it("shows the fleet's countdown and never sends by itself", async () => {
    const onSend = vi.fn();
    render(AutoContinueBar, { sessionId: 7, turnSeq: 4, edits: 0, onSend });
    await vi.advanceTimersByTimeAsync(0);
    expect(screen.getByTestId('auto-continue-clock').textContent).toBe('1:00');
    await vi.advanceTimersByTimeAsync(120_000);
    expect(onSend).not.toHaveBeenCalled();
  });

  it('Send now takes the card and sends its prompt once', async () => {
    const onSend = vi.fn();
    render(AutoContinueBar, { sessionId: 7, turnSeq: 4, edits: 0, onSend });
    await vi.advanceTimersByTimeAsync(0);
    await fireEvent.click(screen.getByTestId('auto-continue-now'));
    await vi.advanceTimersByTimeAsync(0);
    expect(onSend).toHaveBeenCalledExactlyOnceWith('Open a draft PR');
    expect(mocked).toHaveBeenCalledWith('control_auto_continue', { action: 'take', turnSeq: 4 });
    expect(screen.queryByTestId('auto-continue')).toBeNull();
  });

  it('Send now after the fleet already sent it sends nothing', async () => {
    const onSend = vi.fn();
    render(AutoContinueBar, { sessionId: 7, turnSeq: 4, edits: 0, onSend });
    await vi.advanceTimersByTimeAsync(0);
    state = view({}, pick({ state: 'sent' }));
    await fireEvent.click(screen.getByTestId('auto-continue-now'));
    await vi.advanceTimersByTimeAsync(0);
    expect(onSend).not.toHaveBeenCalled();
  });

  it('Cancel and typing settle the card', async () => {
    const onSend = vi.fn();
    render(AutoContinueBar, { sessionId: 7, turnSeq: 4, edits: 0, onSend });
    await vi.advanceTimersByTimeAsync(0);
    await fireEvent.click(screen.getByTestId('auto-continue-cancel'));
    await vi.advanceTimersByTimeAsync(0);
    expect(screen.queryByTestId('auto-continue')).toBeNull();
    expect(state.pending?.state).toBe('cancelled');

    state = view();
    mocked.mockClear();
    const bar = render(AutoContinueBar, { sessionId: 7, turnSeq: 4, edits: 3, onSend });
    await vi.advanceTimersByTimeAsync(0);
    // Text already in the box (edits made before the card) cancels nothing (w13-8).
    expect(mocked).not.toHaveBeenCalledWith('control_auto_continue', { action: 'cancel', turnSeq: 4 });
    expect(screen.getByTestId('auto-continue')).toBeTruthy();
    await bar.rerender({ sessionId: 7, turnSeq: 4, edits: 4, onSend });
    await vi.advanceTimersByTimeAsync(0);
    expect(mocked).toHaveBeenCalledWith('control_auto_continue', { action: 'cancel', turnSeq: 4 });
    expect(onSend).not.toHaveBeenCalled();
  });

  it('an older status answer landing after Cancel does not bring the card back', async () => {
    render(AutoContinueBar, { sessionId: 7, turnSeq: 4, edits: 0, onSend: vi.fn() });
    await vi.advanceTimersByTimeAsync(0);
    // The next poll's status is held in flight while the person cancels.
    let release: (v: AutoContinueView) => void = () => {};
    const stale = state;
    mocked.mockImplementationOnce(() => new Promise((res) => (release = res as typeof release)));
    await vi.advanceTimersByTimeAsync(10_000);
    await fireEvent.click(screen.getByTestId('auto-continue-cancel'));
    await vi.advanceTimersByTimeAsync(0);
    expect(screen.queryByTestId('auto-continue')).toBeNull();
    release(stale);
    await vi.advanceTimersByTimeAsync(0);
    expect(screen.queryByTestId('auto-continue')).toBeNull();
  });

  it('reads again 3 s after the card is due, so the bar clears', async () => {
    render(AutoContinueBar, { sessionId: 7, turnSeq: 4, edits: 0, onSend: vi.fn() });
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(60_000); // due now; the 10 s polls have read it
    state = view({}, pick({ state: 'sent' }));
    const before = mocked.mock.calls.length;
    await vi.advanceTimersByTimeAsync(3_500);
    expect(mocked.mock.calls.length).toBeGreaterThan(before);
    expect(screen.queryByTestId('auto-continue')).toBeNull();
  });

  it('says the hub needs updating when it has no auto-continue tool', async () => {
    mocked.mockImplementation(async () => {
      throw { code: 'E_HUB_PROTOCOL', message: 'unknown tool control_auto_continue' };
    });
    render(AutoContinueBar, { sessionId: 7, turnSeq: 4, edits: 0, onSend: vi.fn() });
    await vi.advanceTimersByTimeAsync(0);
    expect(screen.getByTestId('auto-continue-unsupported')).toBeTruthy();
    const calls = mocked.mock.calls.length;
    await vi.advanceTimersByTimeAsync(30_000);
    expect(mocked.mock.calls.length).toBe(calls);
  });

  it('draws nothing when the fleet has it off (Ask first)', async () => {
    state = view({ enabled: false });
    render(AutoContinueBar, { sessionId: 7, turnSeq: 4, edits: 0, onSend: vi.fn() });
    await vi.advanceTimersByTimeAsync(0);
    expect(screen.queryByTestId('auto-continue')).toBeNull();
  });
});
