import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SecretsPanel from './SecretsPanel.svelte';
import { hosts } from './hosts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

function byCmd(map: Record<string, unknown>) {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd in map) return map[cmd];
    throw { code: 'E_TEST', message: `unexpected ${cmd}` };
  });
}

beforeEach(() => {
  invoke.mockReset();
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true },
  ]);
});

describe('SecretsPanel', () => {
  it('shows the union of stored names and names passed in, masked input, and set/not-set status', async () => {
    byCmd({ catalog_list_secrets: [{ name: 'GH_TOKEN', host_alias: null, updated_at: 1 }] });
    render(SecretsPanel, { names: ['NPM_TOKEN'], onclose: () => {} });

    await screen.findByTestId('secret-row-GH_TOKEN');
    expect(screen.getByTestId('secret-row-NPM_TOKEN')).toBeTruthy();
    expect(screen.getByTestId('secret-row-GH_TOKEN').textContent).toContain('global: set');
    expect(screen.getByTestId('secret-row-NPM_TOKEN').textContent).toContain('global: not set');

    const input = screen.getByTestId('secret-value-GH_TOKEN') as HTMLInputElement;
    expect(input.type).toBe('password');
  });

  it('never displays a value, even after typing one in', async () => {
    byCmd({ catalog_list_secrets: [] });
    render(SecretsPanel, { names: ['GH_TOKEN'], onclose: () => {} });
    await screen.findByTestId('secret-row-GH_TOKEN');
    const input = screen.getByTestId('secret-value-GH_TOKEN') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'super-secret' } });
    expect(input.type).toBe('password');
    expect(screen.queryByText('super-secret')).toBeNull();
  });

  it('Set calls catalog_set_secret with { name, host_alias: null, value } for the default (global) scope', async () => {
    byCmd({ catalog_list_secrets: [], catalog_set_secret: null });
    render(SecretsPanel, { names: ['GH_TOKEN'], onclose: () => {} });
    await screen.findByTestId('secret-row-GH_TOKEN');

    await fireEvent.input(screen.getByTestId('secret-value-GH_TOKEN'), { target: { value: 'shh' } });
    await fireEvent.click(screen.getByTestId('secret-set-GH_TOKEN'));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_set_secret', { args: { name: 'GH_TOKEN', host_alias: null, value: 'shh' } }));
  });

  it('Set with a host selected calls catalog_set_secret with that host_alias', async () => {
    byCmd({ catalog_list_secrets: [], catalog_set_secret: null });
    render(SecretsPanel, { names: ['GH_TOKEN'], onclose: () => {} });
    await screen.findByTestId('secret-row-GH_TOKEN');

    await fireEvent.change(screen.getByTestId('secret-host-GH_TOKEN'), { target: { value: 'mefistos' } });
    await fireEvent.input(screen.getByTestId('secret-value-GH_TOKEN'), { target: { value: 'shh' } });
    await fireEvent.click(screen.getByTestId('secret-set-GH_TOKEN'));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_set_secret', { args: { name: 'GH_TOKEN', host_alias: 'mefistos', value: 'shh' } }));
  });

  it('Delete calls catalog_delete_secret for the selected scope and reloads', async () => {
    byCmd({ catalog_list_secrets: [{ name: 'GH_TOKEN', host_alias: null, updated_at: 1 }], catalog_delete_secret: true });
    render(SecretsPanel, { names: [], onclose: () => {} });
    await screen.findByTestId('secret-row-GH_TOKEN');

    await fireEvent.click(screen.getByTestId('secret-delete-GH_TOKEN'));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_delete_secret', { args: { name: 'GH_TOKEN', host_alias: null } }));
  });

  it('a free-text add-name input adds a row for a name not yet known', async () => {
    byCmd({ catalog_list_secrets: [] });
    render(SecretsPanel, { names: [], onclose: () => {} });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_list_secrets', undefined));

    await fireEvent.input(screen.getByTestId('secrets-add-name'), { target: { value: 'anthropic_key' } });
    await fireEvent.click(screen.getByTestId('secrets-add-name-submit'));

    expect(screen.getByTestId('secret-row-ANTHROPIC_KEY')).toBeTruthy();
  });
});
