import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SecretsPanel from './SecretsPanel.svelte';
import { hosts } from './hosts';
import { expectAccessible } from './a11y_check';

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
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true, transport: 'ssh' },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true, transport: 'ssh' },
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

describe('SecretsPanel accessibility (7.2)', () => {
  it('passes the axe and audit checks', async () => {
    byCmd({ catalog_list_secrets: [{ name: 'GH_TOKEN', host_alias: null, updated_at: 1 }] });
    const { container } = render(SecretsPanel, { names: ['NPM_TOKEN'], onclose: () => {} });
    await screen.findByTestId('secret-row-GH_TOKEN');
    await expectAccessible(container);
  });
});

// G2.6: one secret written to several hosts in one form; the value is
// cleared once written and never shown.
describe('SecretsPanel: Add a secret', () => {
  it('writes the value everywhere chosen, then forgets it', async () => {
    const calls: unknown[] = [];
    invoke.mockImplementation(async (cmd: string, a: { args?: unknown }) => {
      if (cmd === 'catalog_list_secrets') return [];
      if (cmd === 'catalog_set_secret') {
        calls.push(a.args);
        return null;
      }
      throw { code: 'E_TEST', message: `unexpected ${cmd}` };
    });
    render(SecretsPanel, { names: [], onclose: () => {} });
    await fireEvent.input(screen.getByTestId('secrets-add-secret-name'), { target: { value: 'fleet_token' } });
    const value = screen.getByTestId('secrets-add-secret-value') as HTMLInputElement;
    expect(value.type).toBe('password');
    await fireEvent.input(value, { target: { value: 's3cret' } });
    await fireEvent.click(screen.getByTestId('secrets-add-global'));
    await fireEvent.change(screen.getByTestId('secrets-add-host-mefistos'), { target: { value: 'write' } });
    await fireEvent.change(screen.getByTestId('secrets-add-host-local'), { target: { value: 'write' } });
    await fireEvent.click(screen.getByTestId('secrets-add-secret'));
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls).toEqual([
      { name: 'FLEET_TOKEN', host_alias: 'local', value: 's3cret' },
      { name: 'FLEET_TOKEN', host_alias: 'mefistos', value: 's3cret' },
    ]);
    await waitFor(() => expect(value.value).toBe(''));
    expect(document.body.textContent).not.toContain('s3cret');
  });

  it('needs somewhere to write it', async () => {
    byCmd({ catalog_list_secrets: [] });
    render(SecretsPanel, { names: [], onclose: () => {} });
    await fireEvent.input(screen.getByTestId('secrets-add-secret-name'), { target: { value: 'X' } });
    await fireEvent.input(screen.getByTestId('secrets-add-secret-value'), { target: { value: 'v' } });
    expect((screen.getByTestId('secrets-add-secret') as HTMLButtonElement).disabled).toBe(false);
    await fireEvent.click(screen.getByTestId('secrets-add-global'));
    expect((screen.getByTestId('secrets-add-secret') as HTMLButtonElement).disabled).toBe(true);
  });
});
