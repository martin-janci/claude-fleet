// Review w14-2 / w14-10: the paired Add host ends on a sheet that says what
// happened and what is next, for an SSH host and for an agent host.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import HubAddHostDialog from './HubAddHostDialog.svelte';
import { hubStatus, STANDALONE } from './hub';

const inv = mockedInvoke as unknown as ReturnType<typeof vi.fn>;

beforeEach(() => {
  hubStatus.set({ ...STANDALONE, remote: true, url: 'https://fleet.example.com' });
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, payload?: { args?: { alias?: string } }) => {
    if (cmd === 'discover_hosts') return [];
    if (cmd === 'add_host') return { alias: payload?.args?.alias };
    if (cmd === 'agent_installs') return [];
    return null;
  });
});

async function fill(alias: string) {
  await fireEvent.input(await screen.findByTestId('form-field-ssh_alias'), { target: { value: alias } });
}

describe('HubAddHostDialog', () => {
  it('an SSH host: the sheet offers New session here and Install fleet-agent', async () => {
    const onNewSession = vi.fn();
    const onClose = vi.fn();
    render(HubAddHostDialog, { props: { onClose, onNewSession } });
    await fill('mercury');
    await fireEvent.click(screen.getByTestId('form-submit'));
    await waitFor(() => expect(screen.getByTestId('hub-add-host-done')).toBeInTheDocument());
    expect(screen.getByTestId('hub-add-host-summary').textContent).toContain('mercury added on the hub');
    expect(screen.getByTestId('hub-add-host-agent-install')).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('hub-add-host-new-session'));
    expect(onNewSession).toHaveBeenCalledWith('mercury');
    expect(onClose).toHaveBeenCalled();
  });

  it('an agent host: added without a probe, the sheet gives the token and install commands', async () => {
    render(HubAddHostDialog, { props: { onClose: vi.fn(), onNewSession: vi.fn() } });
    await fill('laptop');
    await fireEvent.click(screen.getByTestId('form-field-via-agent'));
    await fireEvent.click(screen.getByTestId('form-submit'));
    await waitFor(() => expect(screen.getByTestId('hub-add-host-agent-steps')).toBeInTheDocument());
    expect(inv.mock.calls.find((c) => c[0] === 'add_host')?.[1]).toEqual({
      args: { alias: 'laptop', ssh_alias: 'laptop', transport: 'agent' },
    });
    expect(screen.getByTestId('hub-add-host-token-cmd').textContent).toBe('fleet-hub agent-token laptop');
    expect(screen.getByTestId('hub-add-host-install-cmd').textContent).toBe(
      'fleet-agent install --user --hub https://fleet.example.com --token-file -',
    );
    expect(screen.queryByTestId('hub-add-host-new-session')).toBeNull();
  });
});
