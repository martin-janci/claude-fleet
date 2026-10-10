import { describe, it, expect, vi, afterEach } from 'vitest';
import { trayStateOf, startTraySync, type TrayState } from './tray_state';
import { hubConnection, SIGNAL_LOST_AFTER_MS } from './hub_connection';
import { sessions } from './sessions';
import { session } from './hosts_fixture';

afterEach(() => {
  vi.useRealTimers();
  hubConnection.set({ state: 'standalone' });
  sessions.set([]);
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

  // Step 3.14: Halo follows the Inbox count, on the dock as on the tray.
  it('hands the Inbox count along for the dock badge, and clears it at zero', () => {
    const sent: [TrayState, number][] = [];
    const stop = startTraySync((s, n) => sent.push([s, n]));
    expect(sent).toEqual([['idle', 0]]);
    sessions.set([session('mac', 'a', { claude_status: 'blocked' }), session('mac', 'b', { claude_status: 'blocked' })]);
    expect(sent.at(-1)).toEqual(['needs_you', 2]);
    sessions.set([session('mac', 'a', { claude_status: 'blocked' })]);
    expect(sent.at(-1)).toEqual(['needs_you', 1]);
    sessions.set([]);
    expect(sent.at(-1)).toEqual(['idle', 0]);
    stop();
  });

  it('sends the count to set_tray_state as needsYou', async () => {
    const core = await import('@tauri-apps/api/core');
    const spy = vi.spyOn(core, 'invoke').mockResolvedValue(null);
    const stop = startTraySync();
    expect(spy).toHaveBeenCalledWith('set_tray_state', { state: 'idle', needsYou: 0 });
    stop();
    spy.mockRestore();
  });
});
