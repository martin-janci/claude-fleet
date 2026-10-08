// Orbit Fleet 11.4: Settings → Control API lists every token — each host's,
// with when it was last used and rotated, beside the paired devices' — and
// rotates a host's or revokes a device's from the row.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ControlApiTokens from './ControlApiTokens.svelte';
import { hostTokens } from './host_actions';
import { devices } from './devices';
import { toasts } from './toasts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const NOW = 1_000_000;

let mercury = { host_alias: 'mercury', mode: 'full', created_at: NOW - 12 * 86_400, last_used_at: NOW - 120 as number | null, rotated_at: null as number | null };
let phone = { name: 'Grafana dashboard', mode: 'readonly', trusted: false, created_at: NOW - 40 * 86_400, last_seen_at: NOW - 3600, catalogs: [] };

beforeEach(() => {
  mercury = { host_alias: 'mercury', mode: 'full', created_at: NOW - 12 * 86_400, last_used_at: NOW - 120, rotated_at: null };
  phone = { name: 'Grafana dashboard', mode: 'readonly', trusted: false, created_at: NOW - 40 * 86_400, last_seen_at: NOW - 3600, catalogs: [] };
  hostTokens.set(new Map());
  devices.set([]);
  toasts.set([]);
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'list_host_tokens') return [mercury];
    if (cmd === 'list_devices') return [phone, { ...phone, name: 'this-mac', mode: 'full', this_device: true }];
    if (cmd === 'rotate_host_token') {
      mercury = { ...mercury, rotated_at: NOW - 5, last_used_at: null };
      return mercury;
    }
    if (cmd === 'revoke_device') return { name: phone.name };
    return null;
  });
});

describe('the Control API tokens table', () => {
  it('lists host tokens with last used and created, beside the devices', async () => {
    render(ControlApiTokens, { props: { now: () => NOW } });
    await waitFor(() => expect(screen.getByTestId('token-row-host:mercury')).toBeTruthy());
    expect(screen.getByTestId('token-used-host:mercury').textContent).toBe('2 min ago');
    expect(screen.getByTestId('token-created-host:mercury').textContent).toContain('12 d ago');
    expect(screen.queryByTestId('token-rotated-host:mercury')).toBeNull();
    expect(screen.getByTestId('token-row-device:Grafana dashboard').textContent).toContain('read-only');
    expect(screen.getByTestId('token-used-device:Grafana dashboard').textContent).toBe('1 h ago');
    // The device in hand cannot revoke itself from here.
    expect(screen.queryByTestId('token-revoke-this-mac')).toBeNull();
  });

  it('rotates a host token after asking, then shows when it was rotated', async () => {
    render(ControlApiTokens, { props: { now: () => NOW } });
    await fireEvent.click(await screen.findByTestId('token-rotate-mercury'));
    await fireEvent.click(screen.getByTestId('token-confirm'));
    await waitFor(() => expect(screen.getByTestId('token-rotated-host:mercury').textContent).toBe('rotated just now'));
    expect(invoke).toHaveBeenCalledWith('rotate_host_token', { hostAlias: 'mercury' });
    expect(screen.getByTestId('token-used-host:mercury').textContent).toBe('never');
  });

  it('revokes a device after asking', async () => {
    render(ControlApiTokens, { props: { now: () => NOW } });
    await fireEvent.click(await screen.findByTestId('token-revoke-Grafana dashboard'));
    await fireEvent.click(screen.getByTestId('token-confirm'));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('revoke_device', { args: { device: 'Grafana dashboard' } }));
  });
});
