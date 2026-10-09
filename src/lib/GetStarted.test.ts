import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import GetStarted from './GetStarted.svelte';
import { getStartedFolded, getStartedItems, type GetStartedInputs } from './get_started';
import { hosts, type HostRow } from './hosts';
import { accounts, type AccountRow } from './accounts';
import { sessions, type SessionRow } from './sessions';
import { trackers, type TrackerRow } from './trackers';
import { devices } from './devices';
import { destination } from './destination';
import { hostsViewRequest, settingsOpen, settingsSection } from './app_views';
import { switcherRequest } from './switcher_request';
import { onboardingDismissed } from './onboarding';
import { routinesDialogOpen, routinesRequest } from './routines';
import { sidebarView } from './work_view';

// Redesign step 10.5: Get started's six rows, what ticks them and where
// each one goes.

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const none: GetStartedInputs = {
  visibleHostCount: 0,
  accountCount: 0,
  workSessionCount: 0,
  githubConnected: false,
  otherDeviceCount: 0,
  enabledRoutineCount: null,
};

beforeEach(() => {
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    if (cmd === 'routines') return [{ enabled: true }];
    if (cmd === 'list_trackers' || cmd === 'list_devices') return [];
    return null;
  });
  hosts.set([{ alias: 'mac', hidden: false } as HostRow]);
  accounts.set([{ uuid: 'a1' } as AccountRow]);
  sessions.set([]);
  trackers.set([]);
  devices.set([]);
  getStartedFolded.set(false);
  onboardingDismissed.set(false);
  settingsOpen.set(false);
  settingsSection.set(null);
  hostsViewRequest.set(null);
  switcherRequest.set(null);
  destination.set('session');
});

describe('getStartedItems', () => {
  it('lists the six rows in the board order, the first open one next', () => {
    const items = getStartedItems({ ...none, visibleHostCount: 1, accountCount: 1 });
    expect(items.map((i) => i.label)).toEqual([
      'Add a host',
      'Sign in a Claude account',
      'Start your first session',
      'Connect GitHub for PRs',
      'Pair your phone',
      'Turn on a routine',
    ]);
    expect(items.map((i) => i.done)).toEqual([true, true, false, false, false, false]);
    expect(items.filter((i) => i.next).map((i) => i.id)).toEqual(['session']);
  });

  it('a routine count this build cannot read leaves the row open', () => {
    expect(getStartedItems(none)[5].done).toBe(false);
    expect(getStartedItems({ ...none, enabledRoutineCount: 2 })[5].done).toBe(true);
  });
});

describe('GetStarted', () => {
  it('ticks rows from the stores and counts them', async () => {
    sessions.set([{ id: 1, kind: 'work' } as SessionRow]);
    trackers.set([{ id: 1, provider: 'github', state: 'ok' } as TrackerRow]);
    devices.set([
      { name: 'this mac', this_device: true } as never,
      { name: 'Pixel', this_device: false } as never,
    ]);
    render(GetStarted);
    await waitFor(() => expect(screen.getByTestId('get-started-count').textContent).toBe('6 of 6'));
    expect(screen.getByTestId('get-started-all')).toBeTruthy();
  });

  it('a GitHub tracker that fails to sync is not connected, and only this device is not a phone', async () => {
    trackers.set([{ id: 1, provider: 'github', state: 'auth_failed' } as TrackerRow]);
    devices.set([{ name: 'this mac', this_device: true } as never]);
    render(GetStarted);
    await waitFor(() => expect(screen.getByTestId('get-started-routine').textContent).toContain('✓'));
    expect(screen.getByTestId('get-started-github').textContent).toContain('○');
    expect(screen.getByTestId('get-started-phone').textContent).toContain('○');
    expect(screen.getByTestId('get-started-session').textContent).toContain('1 min');
  });

  it('each row opens the place that does it', async () => {
    render(GetStarted);
    await fireEvent.click(screen.getByTestId('get-started-host'));
    expect(get(hostsViewRequest)).toEqual({ host: null });
    await fireEvent.click(screen.getByTestId('get-started-account'));
    expect(get(destination)).toBe('accounts');
    await fireEvent.click(screen.getByTestId('get-started-session'));
    expect(get(switcherRequest)).not.toBeNull();
    await fireEvent.click(screen.getByTestId('get-started-github'));
    expect(get(settingsSection)).toBe('trackers');
    await fireEvent.click(screen.getByTestId('get-started-phone'));
    expect(get(settingsSection)).toBe('devices');
    expect(get(settingsOpen)).toBe(true);
    await fireEvent.click(screen.getByTestId('get-started-routine'));
    expect(get(sidebarView)).toBe('inbox');
    expect(get(destination)).toBe('session');
    expect(get(routinesDialogOpen)).toBe(true);
    expect(get(routinesRequest)?.template).toBe('morning-pr-sweep');
  });

  it('folds to its title line and hides until Settings replays it', async () => {
    render(GetStarted);
    await fireEvent.click(screen.getByTestId('get-started-fold'));
    expect(screen.queryByTestId('get-started-host')).toBeNull();
    expect(screen.getByTestId('get-started-count')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('get-started-hide'));
    expect(get(onboardingDismissed)).toBe(true);
  });
});
