import { describe, it, expect, vi, afterEach } from 'vitest';
import { trayStateOf, startTraySync, type TrayState } from './tray_state';
import { hubConnection, SIGNAL_LOST_AFTER_MS } from './hub_connection';

afterEach(() => {
  vi.useRealTimers();
  hubConnection.set({ state: 'standalone' });
});

describe('tray state (step 3.17)', () => {
  it('reads Signal lost, then needs you, then working, else idle', () => {
    expect(trayStateOf({ lost: false, needsYou: 0, working: 0 })).toBe('idle');
    expect(trayStateOf({ lost: false, needsYou: 0, working: 2 })).toBe('working');
    expect(trayStateOf({ lost: false, needsYou: 1, working: 2 })).toBe('needs_you');
    expect(trayStateOf({ lost: true, needsYou: 1, working: 2 })).toBe('lost');
  });

  it('switches the tray to Signal lost only after the hub is gone for a while, and back', () => {
    vi.useFakeTimers();
    const sent: TrayState[] = [];
    const stop = startTraySync((s) => sent.push(s));
    expect(sent).toEqual(['idle']);
    hubConnection.set({ state: 'reconnecting', attempt: 1, retry_in_secs: 2, reason: 'eof' });
    expect(sent).toEqual(['idle']);
    vi.advanceTimersByTime(SIGNAL_LOST_AFTER_MS);
    expect(sent).toEqual(['idle', 'lost']);
    hubConnection.set({ state: 'connected' });
    expect(sent).toEqual(['idle', 'lost', 'idle']);
    stop();
  });
});
