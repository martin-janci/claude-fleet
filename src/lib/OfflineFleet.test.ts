// Open offline (redesign step 3.15): this computer's sessions only.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(() => Promise.resolve(null)) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { invoke } from '@tauri-apps/api/core';
import OfflineFleet from './OfflineFleet.svelte';
import { offlineChosen, offlineMode, offlineSessions } from './offline';
import { hubConnection } from './hub_connection';
import { get } from 'svelte/store';
import { expectAccessible } from './a11y_check';

const answer = (sessions: unknown, err?: { code: string; message: string }) =>
  vi.mocked(invoke).mockImplementation((cmd: string) =>
    cmd === 'offline_local_sessions'
      ? err
        ? Promise.reject(err)
        : Promise.resolve(sessions)
      : (Promise.resolve(null) as never),
  );

afterEach(() => {
  offlineChosen.set(false);
  offlineSessions.set([]);
  hubConnection.set({ state: 'standalone' });
  vi.mocked(invoke).mockReset();
});

describe('OfflineFleet', () => {
  it('copies the attach command the backend quoted', async () => {
    answer([{ name: 'api', created: 1, last_activity: 2, attached: false, attach: "tmux attach -t '=api'" }]);
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    const { container } = render(OfflineFleet, { props: { onhubsettings: vi.fn() } });
    await waitFor(() => expect(screen.getAllByTestId('offline-session')).toHaveLength(1));
    await fireEvent.click(screen.getByTestId('offline-copy'));
    expect(writeText).toHaveBeenCalledWith("tmux attach -t '=api'");
    await expectAccessible(container);
  });

  it('says so when this computer has no sessions', async () => {
    answer([]);
    render(OfflineFleet, { props: { onhubsettings: vi.fn() } });
    await waitFor(() => expect(screen.getByTestId('offline-empty')).toBeTruthy());
  });

  it('shows the error when tmux cannot be read, with Try again', async () => {
    answer(null, { code: 'E_TMUX', message: 'tmux binary not found on PATH' });
    render(OfflineFleet, { props: { onhubsettings: vi.fn() } });
    await waitFor(() => expect(screen.getByTestId('offline-error').textContent).toContain('tmux binary not found'));
  });

  it('is on screen only while chosen and the hub is not connected', () => {
    hubConnection.set({ state: 'offline', attempt: 1, retry_in_secs: 4, reason: 'refused' });
    expect(get(offlineMode)).toBe(false);
    offlineChosen.set(true);
    expect(get(offlineMode)).toBe(true);
    hubConnection.set({ state: 'connected' });
    expect(get(offlineMode)).toBe(false);
  });
});
