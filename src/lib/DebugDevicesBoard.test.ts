// Settings › Organisations › Debug devices: the hand-written device board
// above the generated page. Cards by host, the filters, and each action's
// command with its args (routed to the hub on a paired desktop by the
// command layer, so a fake `invoke` stands in for both).
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import DebugDevicesBoard from './DebugDevicesBoard.svelte';
import { SETTINGS_TREE } from './settings_tree';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const base = {
  key: 'k',
  kind: 'physical',
  shared: false,
  first_seen_at: 1,
  last_seen_at: 1,
};
const pixel = { ...base, id: 3, title: 'Pixel 9', name: 'Pixel 9', host: 'mac', platform: 'android', state: 'online', ready: true };
const held = {
  ...base,
  id: 4,
  title: 'Bench pixel',
  name: 'Pixel 8',
  host: 'mac',
  platform: 'android',
  state: 'online',
  ready: true,
  shared: true,
  claimed_by: 'host:mercury#42',
  claim_note: 'login flow',
  queue: [{ holder: 'host:build#7', note: 'smoke test', queued_at: 1 }],
  waiting: 1,
};
const sim = {
  ...base,
  id: 7,
  title: 'iPhone 16',
  name: 'iPhone 16',
  host: 'mini',
  platform: 'ios',
  kind: 'simulator',
  state: 'shutdown',
  ready: false,
};
const LOGS = { exit_code: 0, output: 'E/App: boom', truncated: true };
const SHOT = { caption: 'Pixel 9 on mac: 3 bytes', mime: 'image/png', data: 'AAEC' };

function route(extra: Record<string, unknown> = {}, list: unknown[] = [pixel, held, sim]) {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, a?: { args?: { id?: number } }) => {
    if (cmd in extra) {
      const v = extra[cmd];
      if (v instanceof Error) throw { code: 'E_X', message: v.message };
      return v;
    }
    if (cmd === 'list_debug_devices') return list;
    if (cmd === 'claim_debug_device') {
      const id = a?.args?.id;
      return id === 4 ? { ...held, queue_place: 2, waiting: 2 } : { ...pixel, claimed_by: 'you (desktop)' };
    }
    if (cmd === 'release_debug_device') return { ...held, claimed_by: 'host:build#7', claim_note: 'smoke test', queue: [], waiting: 0 };
    if (cmd === 'debug_device_logs') return LOGS;
    if (cmd === 'debug_device_screenshot') return SHOT;
    if (cmd === 'scan_debug_devices') return { hosts: [] };
    return null;
  });
}

const argsOf = (cmd: string) =>
  (invoke.mock.calls.filter((c) => c[0] === cmd).at(-1)![1] as { args: Record<string, unknown> }).args;

beforeEach(() => route());

describe('the Settings leaf', () => {
  it('shows the board first, then the generated page', () => {
    const leaf = SETTINGS_TREE.flatMap((g) => g.items).find((l) => l.id === 'debug-devices');
    expect(leaf).toEqual({ id: 'debug-devices', label: 'Debug devices', panel: 'debug-devices', page: 'debug_devices' });
  });
});

describe('DebugDevicesBoard', () => {
  it('draws a card per device, grouped by host, with the header counts', async () => {
    render(DebugDevicesBoard);
    await screen.findByTestId('device-card-3');
    expect(screen.getByTestId('device-board-count').textContent).toBe('3 devices · 1 in use · 1 waiting');
    const mac = screen.getByTestId('device-host-mac');
    expect(within(mac).getByRole('heading').textContent).toContain('2 devices');
    expect(within(mac).getByTestId('device-card-4')).toBeTruthy();
    expect(within(screen.getByTestId('device-host-mini')).getByTestId('device-card-7')).toBeTruthy();
    expect(screen.getByTestId('device-claim-3').textContent).toBe('Free');
    expect(screen.getByTestId('device-claim-4').textContent).toBe('In use by session #42 on mercury · login flow');
    expect(screen.getByTestId('device-shared-4')).toBeTruthy();
    expect(screen.getByTestId('device-state-7').textContent).toBe('Shut down');
    expect(screen.getByTestId('device-hint-7').textContent).toBe('Start it first.');
    // Only the actions that apply.
    expect(screen.getByTestId('device-action-3-claim')).toBeTruthy();
    expect(screen.queryByTestId('device-action-4-claim')).toBeNull();
    expect(screen.getByTestId('device-action-7-boot')).toBeTruthy();
    expect(screen.queryByTestId('device-action-7-screenshot')).toBeNull();
  });

  it('lists who waits on demand', async () => {
    render(DebugDevicesBoard);
    await fireEvent.click(await screen.findByTestId('device-queue-toggle-4'));
    expect(screen.getByTestId('device-waiters-4').textContent).toContain('session #7 on build · smoke test');
  });

  it('filters by state, platform and search', async () => {
    render(DebugDevicesBoard);
    await screen.findByTestId('device-card-3');
    await fireEvent.click(screen.getByTestId('device-filter-in-use'));
    expect(screen.queryByTestId('device-card-3')).toBeNull();
    expect(screen.getByTestId('device-card-4')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('device-filter-all'));
    await fireEvent.click(screen.getByTestId('device-platform-ios'));
    expect(screen.queryByTestId('device-card-4')).toBeNull();
    expect(screen.getByTestId('device-card-7')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('device-platform-ios'));
    await fireEvent.input(screen.getByTestId('device-search'), { target: { value: 'bench' } });
    expect(screen.queryByTestId('device-card-3')).toBeNull();
    expect(screen.getByTestId('device-card-4')).toBeTruthy();
    await fireEvent.input(screen.getByTestId('device-search'), { target: { value: 'nothing like it' } });
    expect(screen.getByTestId('device-board-nomatch')).toBeTruthy();
  });

  it('claims a free device with its note', async () => {
    render(DebugDevicesBoard);
    await fireEvent.input(await screen.findByTestId('device-note-3'), { target: { value: 'login flow' } });
    await fireEvent.click(screen.getByTestId('device-action-3-claim'));
    await waitFor(() => expect(argsOf('claim_debug_device')).toEqual({ id: 3, note: 'login flow', queue: false }));
    await waitFor(() => expect(screen.getByTestId('device-claim-3').textContent).toBe('In use by you'));
  });

  it('waits in line for a held device, and says the place', async () => {
    render(DebugDevicesBoard);
    await fireEvent.click(await screen.findByTestId('device-action-4-queue'));
    await waitFor(() => expect(argsOf('claim_debug_device')).toEqual({ id: 4, note: null, queue: true }));
    expect((await screen.findByTestId('device-place-4')).textContent).toBe("You're 2nd in line");
    // From inside the line: Leave line, not Release.
    expect(screen.getByTestId('device-action-4-leave')).toBeTruthy();
    expect(screen.queryByTestId('device-action-4-release')).toBeNull();
  });

  it('asks before releasing someone else’s claim', async () => {
    render(DebugDevicesBoard);
    await fireEvent.click(await screen.findByTestId('device-action-4-release'));
    const ask = screen.getByTestId('device-confirm-4');
    expect(ask.textContent).toContain('session #42 on mercury loses the claim; session #7 on build, next in line, gets it');
    expect(invoke.mock.calls.some((c) => c[0] === 'release_debug_device')).toBe(false);
    await fireEvent.click(screen.getByTestId('device-confirm-4-no'));
    expect(screen.queryByTestId('device-confirm-4')).toBeNull();
    await fireEvent.click(screen.getByTestId('device-action-4-release'));
    await fireEvent.click(screen.getByTestId('device-confirm-4-yes'));
    await waitFor(() => expect(argsOf('release_debug_device')).toEqual({ id: 4 }));
    await waitFor(() => expect(screen.getByTestId('device-claim-4').textContent).toBe('In use by session #7 on build · smoke test'));
  });

  it('takes a screenshot and shows it, larger on click', async () => {
    render(DebugDevicesBoard);
    await fireEvent.click(await screen.findByTestId('device-action-3-screenshot'));
    const img = (await screen.findByTestId('device-shot-img-3')) as HTMLImageElement;
    expect(argsOf('debug_device_screenshot')).toEqual({ id: 3 });
    expect(img.getAttribute('src')).toBe('data:image/png;base64,AAEC');
    expect(img.getAttribute('alt')).toBe(SHOT.caption);
    expect(screen.getByTestId('device-action-3-screenshot').textContent).toBe('Refresh');
    await fireEvent.click(img.closest('button')!);
    expect(within(screen.getByTestId('device-shot-overlay')).getByRole('img')).toBeTruthy();
  });

  it('opens the logs in a monospace block that says when they were cut', async () => {
    render(DebugDevicesBoard);
    await fireEvent.click(await screen.findByTestId('device-action-3-logs'));
    const box = await screen.findByTestId('device-logs');
    expect(argsOf('debug_device_logs')).toEqual({ id: 3, contains: null });
    expect(within(box).getByTestId('device-logs-output').textContent).toBe('E/App: boom');
    expect(within(box).getByTestId('device-logs-truncated')).toBeTruthy();
  });

  it('shows a failed action on its card', async () => {
    route({ start_device_mirror: new Error('scrcpy not found on this machine') });
    render(DebugDevicesBoard);
    await fireEvent.click(await screen.findByTestId('device-action-3-mirror'));
    expect((await screen.findByTestId('device-error-3')).textContent).toBe('scrcpy not found on this machine');
  });

  it('asks before stopping a simulator', async () => {
    route({}, [{ ...sim, state: 'booted', ready: true }]);
    render(DebugDevicesBoard);
    await fireEvent.click(await screen.findByTestId('device-action-7-shutdown'));
    expect(invoke.mock.calls.some((c) => c[0] === 'shutdown_debug_device')).toBe(false);
    await fireEvent.click(screen.getByTestId('device-confirm-7-yes'));
    await waitFor(() => expect(argsOf('shutdown_debug_device')).toEqual({ id: 7 }));
  });

  it('rescans every host, then lists again', async () => {
    render(DebugDevicesBoard);
    await screen.findByTestId('device-card-3');
    await fireEvent.click(screen.getByTestId('device-rescan'));
    await waitFor(() => expect(argsOf('scan_debug_devices')).toEqual({}));
    await waitFor(() => expect(invoke.mock.calls.filter((c) => c[0] === 'list_debug_devices').length).toBe(2));
  });

  it('says there are none with the page’s own words', async () => {
    route({}, []);
    render(DebugDevicesBoard);
    expect((await screen.findByTestId('device-board-empty')).textContent).toMatch(/^No debug devices yet\./);
  });

  it('shows a failed list with a Retry', async () => {
    route({ list_debug_devices: new Error('hub unreachable') });
    render(DebugDevicesBoard);
    expect((await screen.findByTestId('device-board-error')).textContent).toContain('hub unreachable');
    route();
    await fireEvent.click(screen.getByTestId('device-board-retry'));
    expect(await screen.findByTestId('device-card-3')).toBeTruthy();
    expect(screen.queryByTestId('device-board-error')).toBeNull();
  });
});
