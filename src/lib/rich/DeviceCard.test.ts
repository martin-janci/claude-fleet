// A `device` block in the chat (docs/debug-devices.md): the device it names,
// drawn live from the person's own list, with a screenshot on a click.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import UiBlockCard from './UiBlockCard.svelte';
import { checkUiBlock } from '../rich_blocks';
import { expectAccessible } from '../a11y_check';

const mock = invoke as ReturnType<typeof vi.fn>;

const dev = (o: Record<string, unknown>) => ({
  id: 3,
  title: 'bench pixel',
  label: 'bench pixel',
  host: 'mac-mini',
  platform: 'android',
  kind: 'physical',
  key: 'usb:R5CT',
  name: 'Pixel 9',
  os_version: '16',
  state: 'online',
  ready: true,
  shared: true,
  first_seen_at: 1,
  last_seen_at: 2,
  ...o,
});
const DEVICES = [
  dev({}),
  dev({ id: 4, host: 'box', label: undefined, title: 'Pixel 9', key: 'usb:X', claimed_by: 'host:box#7', claimed_until: 4e9, queue: [{ holder: 'host:mercury#1', queued_at: 1 }], waiting: 1 }),
  dev({ id: 5, host: 'mac-mini', label: undefined, title: 'iPhone 16', name: 'iPhone 16', platform: 'ios', kind: 'simulator', state: 'booted', key: 'sim:A' }),
  dev({ id: 6, host: 'box', label: undefined, title: 'iPhone 16', name: 'iPhone 16', platform: 'ios', kind: 'simulator', state: 'booted', key: 'sim:B' }),
];
const SHOT = { caption: 'bench pixel on mac-mini: 3 bytes', mime: 'image/png', data: 'AAEC' };

function block(o: Record<string, unknown>) {
  const raw = JSON.stringify({ spec: 'fleet.ui/1', kind: 'device', ...o });
  const c = checkUiBlock(raw);
  if (!c.ok) throw new Error(c.problems.join('; '));
  return { block: c.block, raw };
}

beforeEach(() => {
  mock.mockReset();
  mock.mockImplementation(async (cmd: string) => {
    if (cmd === 'list_debug_devices') return DEVICES;
    if (cmd === 'debug_device_screenshot') return SHOT;
    if (cmd === 'start_device_mirror') return { title: 'bench pixel', output: '', exit_code: 0, truncated: false };
    return null;
  });
});

describe('DeviceCard', () => {
  it('resolves the device and draws it with the block’s title and note', async () => {
    const { container } = render(UiBlockCard, { props: { ...block({ device: 'mac-mini/bench pixel', title: 'The login screen', note: 'Check **Sign in**.' }), sessionId: 1 } });
    const card = await screen.findByTestId('device-summary');
    expect(card.getAttribute('data-device')).toBe('3');
    expect(card.textContent).toContain('The login screen');
    expect(card.textContent).toContain('on mac-mini');
    expect(screen.getByTestId('device-state').textContent).toBe('Online');
    expect(screen.getByTestId('rich-device').querySelector('.note strong')?.textContent).toBe('Sign in');
    expect(screen.getByTestId('rich-device-mirror')).toBeTruthy();
    await expectAccessible(container);
  });

  it('shows who holds it and the line, and resolves `#id`', async () => {
    render(UiBlockCard, { props: { ...block({ device: '#4' }), sessionId: 1 } });
    await screen.findByTestId('device-summary');
    expect(screen.getByTestId('device-holder').textContent).toContain('session #7 on box');
    expect(screen.getByTestId('device-queue').textContent).toContain('1 waiting');
  });

  it('takes a screenshot on a click and shows it', async () => {
    render(UiBlockCard, { props: { ...block({ device: 'bench pixel' }), sessionId: 1 } });
    await screen.findByTestId('device-summary');
    await fireEvent.click(screen.getByTestId('rich-device-screenshot'));
    const shot = await screen.findByTestId('rich-device-shot');
    expect(shot.querySelector('img')?.getAttribute('src')).toBe('data:image/png;base64,AAEC');
    expect(mock).toHaveBeenCalledWith('debug_device_screenshot', { args: { id: 3 } });
    expect(screen.getByTestId('rich-device-screenshot').textContent).toContain('Refresh screenshot');
  });

  it('shows the backend’s message when a screenshot fails', async () => {
    mock.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_debug_devices') return DEVICES;
      throw { code: 'E_CONFLICT', message: 'held by session #7 on box' };
    });
    render(UiBlockCard, { props: { ...block({ device: 'bench pixel' }), sessionId: 1 } });
    await screen.findByTestId('device-summary');
    await fireEvent.click(screen.getByTestId('rich-device-screenshot'));
    expect((await screen.findByTestId('rich-device-error')).textContent).toContain('held by session #7 on box');
  });

  it('a device it cannot find, or cannot tell apart, is a quiet line', async () => {
    render(UiBlockCard, { props: { ...block({ device: 'Galaxy' }), sessionId: 1 } });
    expect((await screen.findByTestId('rich-device-none')).textContent).toContain('No debug device Galaxy on this fleet');
    render(UiBlockCard, { props: { ...block({ device: 'iPhone 16' }), sessionId: 1 } });
    await waitFor(() => expect(screen.getAllByTestId('rich-device-none')).toHaveLength(2));
    expect(screen.queryByTestId('rich-device-screenshot')).toBeNull();
  });

  it('Mirror waits for the live conversation; Screenshot does not', async () => {
    render(UiBlockCard, { props: { ...block({ device: 'bench pixel' }), sessionId: null } });
    await screen.findByTestId('device-summary');
    expect((screen.getByTestId('rich-device-mirror') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('rich-device-screenshot') as HTMLButtonElement).disabled).toBe(false);
  });

  it('Refresh reads the list again', async () => {
    render(UiBlockCard, { props: { ...block({ device: 'bench pixel' }), sessionId: 1 } });
    await screen.findByTestId('device-summary');
    await fireEvent.click(screen.getByTestId('rich-device-refresh'));
    await waitFor(() => expect(mock.mock.calls.filter((c) => c[0] === 'list_debug_devices')).toHaveLength(2));
  });
});
