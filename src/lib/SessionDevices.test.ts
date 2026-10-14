// The strip of debug devices a session holds or waits for, under the
// Conversation header (docs/debug-devices.md).
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import SessionDevices from './SessionDevices.svelte';

const mock = invoke as ReturnType<typeof vi.fn>;
const session = { id: 42, host_alias: 'mercury' };
const ME = 'host:mercury#42';
const now = Math.floor(Date.now() / 1000);

const dev = (o: Record<string, unknown>) => ({
  id: 3,
  title: 'bench pixel',
  host: 'mac',
  platform: 'android',
  kind: 'physical',
  key: 'usb:1',
  name: 'Pixel 9',
  state: 'online',
  ready: true,
  shared: true,
  first_seen_at: 1,
  last_seen_at: 2,
  ...o,
});
const held = dev({ claimed_by: ME, claimed_until: now + 12 * 60 + 10, claim_note: 'login flow' });
const waited = dev({
  id: 4,
  title: 'iPhone 16',
  platform: 'ios',
  kind: 'simulator',
  state: 'booted',
  claimed_by: 'host:box#7',
  queue: [
    { holder: 'host:box#8', queued_at: 1 },
    { holder: ME, queued_at: 2 },
  ],
  waiting: 2,
});
const others = dev({ id: 5, title: 'Galaxy', claimed_by: 'host:box#9' });

function route(list: unknown[] | (() => unknown)) {
  mock.mockReset();
  mock.mockImplementation(async (cmd: string) => {
    if (cmd === 'list_debug_devices') return typeof list === 'function' ? list() : list;
    if (cmd === 'debug_device_screenshot') return { caption: 'bench pixel on mac', mime: 'image/png', data: 'AAEC' };
    if (cmd === 'release_debug_device') return { ...held, claimed_by: undefined };
    return null;
  });
}

const listCalls = () => mock.mock.calls.filter((c) => c[0] === 'list_debug_devices').length;

beforeEach(() => route([held, waited, others]));
afterEach(() => vi.useRealTimers());

describe('SessionDevices', () => {
  it('shows a held device and one it waits for', async () => {
    render(SessionDevices, { props: { session } });
    const line = await screen.findByTestId('session-device-held');
    expect(line.textContent).toContain('bench pixel');
    expect(line.textContent).toContain('on mac');
    expect(line.textContent).toContain('Online');
    expect(line.textContent).toContain('held · 12 min left');
    expect(line.textContent).toContain('login flow');
    expect(screen.getByTestId('session-device-mirror')).toBeTruthy();
    const w = screen.getByTestId('session-device-waiting');
    expect(w.textContent).toContain('Waiting for iPhone 16');
    expect(w.textContent).toContain('2nd in line');
    expect(w.textContent).toContain('held by session #7 on box');
    expect(w.textContent).not.toContain('Leave');
    expect(screen.getAllByTestId(/session-device-(held|waiting)/)).toHaveLength(2);
  });

  it('renders nothing when the session holds and waits for nothing', async () => {
    route([others]);
    const { container } = render(SessionDevices, { props: { session } });
    await waitFor(() => expect(listCalls()).toBe(1));
    expect(container.querySelector('[data-testid="session-devices"]')).toBeNull();
  });

  it('renders nothing when the list cannot be read, and stops asking', async () => {
    vi.useFakeTimers();
    route(() => {
      throw { code: 'E_LOCAL_ONLY', message: 'not through the hub' };
    });
    const { container } = render(SessionDevices, { props: { session } });
    await vi.advanceTimersByTimeAsync(0);
    expect(container.textContent?.trim()).toBe('');
    await vi.advanceTimersByTimeAsync(31_000);
    expect(listCalls()).toBe(1);
  });

  it('polls every 30 s', async () => {
    vi.useFakeTimers();
    render(SessionDevices, { props: { session } });
    await vi.advanceTimersByTimeAsync(0);
    expect(listCalls()).toBe(1);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(listCalls()).toBe(2);
  });

  it('shows a screenshot inline', async () => {
    render(SessionDevices, { props: { session } });
    await fireEvent.click(await screen.findByTestId('session-device-screenshot'));
    const shot = await screen.findByTestId('session-device-shot');
    expect(shot.querySelector('img')?.getAttribute('src')).toBe('data:image/png;base64,AAEC');
  });

  it('releases only after a confirm, then reads the list again', async () => {
    render(SessionDevices, { props: { session } });
    await fireEvent.click(await screen.findByTestId('session-device-release'));
    expect(mock.mock.calls.some((c) => c[0] === 'release_debug_device')).toBe(false);
    route([waited, others]);
    await fireEvent.click(screen.getByTestId('session-device-release-confirm'));
    await waitFor(() => expect(mock).toHaveBeenCalledWith('release_debug_device', { args: { id: 3 } }));
    await waitFor(() => expect(screen.queryByTestId('session-device-held')).toBeNull());
  });

  it('a cancelled release does nothing', async () => {
    render(SessionDevices, { props: { session } });
    await fireEvent.click(await screen.findByTestId('session-device-release'));
    await fireEvent.click(screen.getByTestId('confirm-cancel'));
    expect(mock.mock.calls.some((c) => c[0] === 'release_debug_device')).toBe(false);
    expect(screen.getByTestId('session-device-held')).toBeTruthy();
  });

  it('folds to a count above two lines', async () => {
    route([held, dev({ id: 6, title: 'Pixel 8', claimed_by: ME }), waited]);
    render(SessionDevices, { props: { session } });
    const toggle = await screen.findByTestId('session-devices-toggle');
    expect(toggle.textContent).toContain('2 held · 1 waiting');
    expect(screen.queryByTestId('session-device-held')).toBeNull();
    await fireEvent.click(toggle);
    expect(screen.getAllByTestId('session-device-held')).toHaveLength(2);
  });
});
