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
import { apiTokens, createTokenArgs, expiryLabel } from './api_tokens';
import { hosts } from './hosts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const NOW = 1_000_000;

let mercury = { host_alias: 'mercury', mode: 'full', created_at: NOW - 12 * 86_400, last_used_at: NOW - 120 as number | null, rotated_at: null as number | null };
let named: { id: number; name: string; scope: string; hosts: string[] | null; expires_at: number | null; created_at: number; last_used_at: number | null; revoked_at: number | null }[] = [];
let phone = { name: 'Grafana dashboard', mode: 'readonly', trusted: false, created_at: NOW - 40 * 86_400, last_seen_at: NOW - 3600, catalogs: [] };

beforeEach(() => {
  mercury = { host_alias: 'mercury', mode: 'full', created_at: NOW - 12 * 86_400, last_used_at: NOW - 120, rotated_at: null };
  phone = { name: 'Grafana dashboard', mode: 'readonly', trusted: false, created_at: NOW - 40 * 86_400, last_seen_at: NOW - 3600, catalogs: [] };
  hostTokens.set(new Map());
  devices.set([]);
  apiTokens.set([]);
  named = [{ id: 4, name: 'Grafana', scope: 'read', hosts: ['mercury'], expires_at: NOW + 30 * 86_400, created_at: NOW - 86_400, last_used_at: null, revoked_at: null }];
  hosts.set([{ alias: 'mercury', hidden: false } as never, { alias: 'venus', hidden: false } as never]);
  toasts.set([]);
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === 'list_host_tokens') return [mercury];
    if (cmd === 'list_devices') return [phone, { ...phone, name: 'this-mac', mode: 'full', this_device: true }];
    if (cmd === 'rotate_host_token') {
      mercury = { ...mercury, rotated_at: NOW - 5, last_used_at: null };
      return mercury;
    }
    if (cmd === 'revoke_device') return { name: phone.name };
    if (cmd === 'api_tokens') {
      const a = (args as { args: Record<string, unknown> }).args;
      if (a.action === 'list') return named;
      if (a.action === 'create') {
        const row = { id: 5, name: a.name, scope: a.scope, hosts: a.hosts, expires_at: null, created_at: NOW, last_used_at: null, revoked_at: null };
        named = [row as never, ...named];
        return { ...row, token: 'flt_live_0123456789abcdef0123456789abcdef', env_line: 'FLEET_MCP_TOKEN=flt_live_0123456789abcdef0123456789abcdef' };
      }
      if (a.action === 'revoke') {
        named = named.filter((t) => t.name !== a.name);
        return { name: a.name };
      }
    }
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

describe('named tokens (G2.8)', () => {
  it('lists a named token with its scope, hosts and expiry', async () => {
    render(ControlApiTokens, { props: { now: () => NOW } });
    await waitFor(() => expect(screen.getByTestId('token-row-named:Grafana')).toBeTruthy());
    expect(screen.getByTestId('token-row-named:Grafana').textContent).toContain('read');
    expect(screen.getByTestId('token-limits-named:Grafana').textContent).toBe('only mercury · expires in 30 d');
    expect(screen.getByTestId('token-used-named:Grafana').textContent).toBe('never');
  });

  it('creates a token through the form and shows it once', async () => {
    render(ControlApiTokens, { props: { now: () => NOW } });
    await fireEvent.click(await screen.findByTestId('token-new'));
    await fireEvent.input(screen.getByTestId('form-field-name'), { target: { value: 'CI' } });
    await fireEvent.click(screen.getByTestId('form-field-hosts-venus'));
    await fireEvent.click(screen.getByTestId('form-submit'));
    await waitFor(() => expect(screen.getByTestId('api-token-created')).toBeTruthy());
    expect(invoke).toHaveBeenCalledWith('api_tokens', {
      args: { action: 'create', name: 'CI', scope: 'act', expires_in_days: 90, hosts: ['venus'] },
    });
    // Head and tail on screen; Copy carries the whole.
    expect(screen.getByTestId('api-token-value').textContent).toBe('flt_live_0123456…def');
    await fireEvent.click(screen.getByTestId('api-token-done'));
    await waitFor(() => expect(screen.queryByTestId('api-token-created')).toBeNull());
    expect(screen.getByTestId('token-row-named:CI')).toBeTruthy();
    expect(document.body.textContent).not.toContain('flt_live_0123456789abcdef0123456789abcdef');
  });

  it('revokes a named token after asking', async () => {
    render(ControlApiTokens, { props: { now: () => NOW } });
    await fireEvent.click(await screen.findByTestId('token-revoke-named-Grafana'));
    await fireEvent.click(screen.getByTestId('token-confirm'));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('api_tokens', { args: { action: 'revoke', name: 'Grafana' } }));
    await waitFor(() => expect(screen.queryByTestId('token-row-named:Grafana')).toBeNull());
  });

  it('maps the answers: never, admin without hosts', () => {
    expect(createTokenArgs({ name: ' x ', scope: 'admin', expires: 'never', hosts: ['mercury'] })).toEqual({
      action: 'create',
      name: 'x',
      scope: 'admin',
      expires_in_days: null,
      hosts: null,
    });
    expect(createTokenArgs({ name: 'x', scope: 'read', expires: '30', hosts: [] }).hosts).toBeNull();
    expect(expiryLabel(null, NOW)).toBe('');
    expect(expiryLabel(NOW - 1, NOW)).toBe('expired');
    expect(expiryLabel(NOW + 7200, NOW)).toBe('expires in 2 h');
  });
});
