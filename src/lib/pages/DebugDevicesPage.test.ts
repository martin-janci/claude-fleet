// Step 11.6: Debug devices on the desktop page claim a device, install an
// app on it, show its logs and take a screenshot, each through a command
// that routes to the hub's `debug_devices` (a fake device here).
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ResourcePage from './ResourcePage.svelte';
import { hubStatus, STANDALONE } from '../hub';
import { hosts } from '../hosts';
import { toasts } from '../toasts';
import { bundle } from './testing';
import type { Page } from './pages';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const page = bundle.pages.find((p) => p.id === 'debug_devices') as Page;
const resource = bundle.resources.find((r) => r.id === 'debug_device')!;

const pixel = {
  id: 3,
  title: 'Pixel 9',
  host: 'mac',
  platform: 'android',
  kind: 'physical',
  state: 'online',
  shared: false,
  last_seen_at: 2,
};
const LOGS = { exit_code: 0, output: '10-08 16:00 E/App: boom\n10-08 16:01 I/App: ok', truncated: true };
const SHOT = { caption: 'Pixel 9 on mac: 3 bytes', mime: 'image/png', data: 'AAEC' };

function route(extra: Record<string, unknown> = {}) {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd in extra) return extra[cmd];
    if (cmd === 'list_debug_devices') return [pixel];
    if (cmd === 'claim_debug_device') return { ...pixel, claimed_by: 'you (desktop)' };
    if (cmd === 'install_debug_device') return { exit_code: 0, output: 'Success', truncated: false };
    if (cmd === 'debug_device_logs') return LOGS;
    if (cmd === 'debug_device_screenshot') return SHOT;
    return null;
  });
}

const argsOf = (cmd: string) =>
  (invoke.mock.calls.filter((c) => c[0] === cmd).at(-1)![1] as { args: Record<string, unknown> }).args;

async function open() {
  render(ResourcePage, { props: { page, resource } });
  await fireEvent.click((await screen.findAllByTestId('resource-row'))[0]);
}

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  toasts.set([]);
  hosts.set([{ alias: 'mac' }, { alias: 'mercury' }] as never);
  route();
});

describe('Debug devices: claim, install, logs, screenshot (11.6)', () => {
  it('offers the four buttons on a physical device', async () => {
    await open();
    for (const id of ['claim', 'install', 'logs', 'screenshot'])
      expect(screen.getByTestId(`record-action-debug_device.${id}`)).toBeTruthy();
  });

  it('claims a device with a note', async () => {
    await open();
    await fireEvent.click(screen.getByTestId('record-action-debug_device.claim'));
    await fireEvent.input(screen.getByTestId('param-debug_device.claim-note'), { target: { value: 'login flow' } });
    await fireEvent.click(screen.getByTestId('run-debug_device.claim'));
    await waitFor(() => expect(argsOf('claim_debug_device')).toEqual({ id: 3, note: 'login flow' }));
  });

  it('installs an app from a path, on the device’s own host unless one is picked, and shows the output', async () => {
    await open();
    await fireEvent.click(screen.getByTestId('record-action-debug_device.install'));
    const host = screen.getByTestId('param-debug_device.install-host') as HTMLSelectElement;
    expect(Array.from(host.options).map((o) => o.value)).toEqual(['', 'mac', 'mercury']);
    await fireEvent.input(screen.getByTestId('param-debug_device.install-path'), { target: { value: '~/app.apk' } });
    await fireEvent.click(screen.getByTestId('run-debug_device.install'));
    // M15 G7.14: claimed while installing unless switched off.
    await waitFor(() =>
      expect(argsOf('install_debug_device')).toEqual({ id: 3, path: '~/app.apk', host: null, claim: true, note: null }),
    );
    expect((await screen.findByTestId('action-result-output')).textContent).toBe('Success');
  });

  it('installs without a claim when switched off, and sends the claim’s note otherwise', async () => {
    await open();
    await fireEvent.click(screen.getByTestId('record-action-debug_device.install'));
    await fireEvent.input(screen.getByTestId('param-debug_device.install-path'), { target: { value: '~/app.apk' } });
    await fireEvent.input(screen.getByTestId('param-debug_device.install-note'), { target: { value: 'login flow' } });
    await fireEvent.click(screen.getByTestId('run-debug_device.install'));
    await waitFor(() =>
      expect(argsOf('install_debug_device')).toEqual({ id: 3, path: '~/app.apk', host: null, claim: true, note: 'login flow' }),
    );
    await fireEvent.click(screen.getByTestId('record-action-debug_device.install'));
    await fireEvent.input(screen.getByTestId('param-debug_device.install-path'), { target: { value: '~/b.apk' } });
    await fireEvent.click(screen.getByTestId('param-debug_device.install-claim'));
    await fireEvent.click(screen.getByTestId('run-debug_device.install'));
    await waitFor(() =>
      expect(argsOf('install_debug_device')).toEqual({ id: 3, path: '~/b.apk', host: null, claim: false, note: null }),
    );
  });

  it('shows what is not built yet greyed, not as controls', async () => {
    await open();
    const later = screen.getAllByTestId('notice-later');
    expect(later.map((n) => n.textContent?.trim().slice(0, 30))).toEqual([
      'Not built yetLive screen and i',
      'Not built yetA device on anoth',
    ]);
  });

  it('shows the logs in a block that says when it was cut, until dismissed', async () => {
    await open();
    await fireEvent.click(screen.getByTestId('record-action-debug_device.logs'));
    const result = await screen.findByTestId('action-result');
    expect(argsOf('debug_device_logs')).toEqual({ id: 3 });
    expect(within(result).getByRole('heading').textContent).toBe('Logs · Pixel 9');
    expect(within(result).getByTestId('action-result-output').textContent).toBe(LOGS.output);
    expect(within(result).getByTestId('action-result-truncated')).toBeTruthy();
    expect(within(result).queryByTestId('action-result-exit')).toBeNull();
    await fireEvent.click(screen.getByTestId('action-result-close'));
    expect(screen.queryByTestId('action-result')).toBeNull();
  });

  it('shows a screenshot as an image', async () => {
    await open();
    await fireEvent.click(screen.getByTestId('record-action-debug_device.screenshot'));
    const img = (await screen.findByTestId('action-result-image')) as HTMLImageElement;
    expect(argsOf('debug_device_screenshot')).toEqual({ id: 3 });
    expect(img.getAttribute('src')).toBe('data:image/png;base64,AAEC');
    expect(img.getAttribute('alt')).toBe(SHOT.caption);
  });

  it('says a failed command’s exit code instead of hiding it', async () => {
    route({ debug_device_logs: { exit_code: 1, output: '', truncated: false } });
    await open();
    await fireEvent.click(screen.getByTestId('record-action-debug_device.logs'));
    expect((await screen.findByTestId('action-result-exit')).textContent).toContain('exit code 1');
    expect(screen.getByTestId('action-result-empty')).toBeTruthy();
  });
});
