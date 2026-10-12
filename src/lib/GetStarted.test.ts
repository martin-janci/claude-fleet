import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import GetStarted from './GetStarted.svelte';
import { buildingFirstFleet, getStartedFolded, getStartedItems, type GetStartedInputs } from './get_started';
import { creatingStart, startedIds } from './sessions';
import { NO_START_STEPS } from './start_steps';
import { resetStarting } from './session_starting';
import { hosts, type HostRow } from './hosts';
import { accounts, type AccountRow } from './accounts';
import { sessions, type SessionRow } from './sessions';
import { trackers, type TrackerRow } from './trackers';
import { devices } from './devices';
import { destination } from './destination';
import { hostsViewRequest, settingsOpen, settingsSection } from './app_views';
import { switcherRequest } from './switcher_request';
import { onboardingDismissed } from './onboarding';
import { routinesRequest } from './routines';
import { automationTab } from './automation';

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
  resetStarting();
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
    expect(await screen.findByRole('dialog')).toBeTruthy();
    // Step 10.12: Get started runs on its own form spec.
    expect(screen.getByTestId('wizard-get_started')).toBeTruthy();
    expect(get(switcherRequest)).toBeNull();
    await fireEvent.click(screen.getByTestId('get-started-github'));
    expect(get(settingsSection)).toBe('trackers');
    await fireEvent.click(screen.getByTestId('get-started-phone'));
    expect(get(settingsSection)).toBe('devices');
    expect(get(settingsOpen)).toBe(true);
    await fireEvent.click(screen.getByTestId('get-started-routine'));
    expect(get(destination)).toBe('automation');
    expect(get(automationTab)).toBe('routines');
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

// Step 10.10: the Galaxy while the first fleet is built.
describe('Galaxy while the first fleet is built', () => {
  it('only while a start is in flight and no other session is up', () => {
    const none = new Set<number>();
    expect(buildingFirstFleet({ workSessionIds: [], starting: none, creating: false })).toBe(false);
    expect(buildingFirstFleet({ workSessionIds: [], starting: none, creating: true })).toBe(true);
    expect(buildingFirstFleet({ workSessionIds: [4], starting: new Set([4]), creating: false })).toBe(true);
    expect(buildingFirstFleet({ workSessionIds: [3, 4], starting: new Set([4]), creating: false })).toBe(false);
    expect(buildingFirstFleet({ workSessionIds: [3], starting: none, creating: true })).toBe(false);
  });

  it('shows the Galaxy while the first session starts, and drops it once its agent is up', async () => {
    vi.useFakeTimers();
    try {
      creatingStart.set({ host_alias: 'mac', name: '', kind: 'work', token: 't1', steps: NO_START_STEPS });
      render(GetStarted);
      await vi.advanceTimersByTimeAsync(400);
      expect(screen.getByTestId('get-started-building').textContent).toContain('Building your fleet');
      expect(screen.getByTestId('get-started-galaxy').getAttribute('data-loader')).toBe('galaxy');
      creatingStart.set(null);
      sessions.set([{ id: 4, kind: 'work', claude_status: null } as SessionRow]);
      startedIds.set(new Set([4]));
      await vi.advanceTimersByTimeAsync(0);
      expect(screen.getByTestId('get-started-building')).toBeTruthy();
      sessions.set([{ id: 4, kind: 'work', claude_status: 'idle' } as SessionRow]);
      await vi.advanceTimersByTimeAsync(0);
      expect(screen.queryByTestId('get-started-building')).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('a fleet with a session up starts its next one without the Galaxy', async () => {
    sessions.set([{ id: 1, kind: 'work', claude_status: 'idle' } as SessionRow]);
    creatingStart.set({ host_alias: 'mac', name: '', kind: 'work', token: 't1', steps: NO_START_STEPS });
    render(GetStarted);
    expect(screen.queryByTestId('get-started-building')).toBeNull();
  });
});
