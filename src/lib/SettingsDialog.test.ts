import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SettingsDialog from './SettingsDialog.svelte';
import { hosts } from './hosts';
import { accounts as accountsStore } from './accounts';

const sample = [
  { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1, account_uuid: null },
  { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null },
];

const mcpStatusObj = {
  enabled: false,
  running: false,
  port: 4180,
  token: 'test-token',
  url: 'http://127.0.0.1:4180/mcp',
  bind_error: null,
};

// Route invoke() by command name. The dialog calls mcp_status on mount, so an
// ordered mockResolvedValueOnce chain would be consumed by the wrong call —
// a routed implementation keeps each command's response stable.
beforeEach(() => {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'mcp_status':
      case 'mcp_configure':
        return mcpStatusObj;
      case 'discover_hosts':
        return [];
      case 'probe_host':
        return sample[1];
      case 'list_hosts':
        return sample;
      default:
        return null;
    }
  });
  hosts.set(sample);
});

describe('SettingsDialog', () => {
  it('renders one row per host', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const table = await screen.findByTestId('hosts-table');
    expect(table.textContent).toContain('local');
    expect(table.textContent).toContain('mefistos');
  });

  it('local row hides the Remove + Hide buttons', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const rows = document.querySelectorAll('.hosts-table tbody tr');
    const localRow = Array.from(rows).find((r) => r.textContent?.includes('local'));
    expect(localRow?.querySelector('button[aria-label="Remove"]')).toBeNull();
  });

  it('clicking Re-probe invokes probe_host', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const rows = document.querySelectorAll('.hosts-table tbody tr');
    const mefRow = Array.from(rows).find((r) => r.textContent?.includes('mefistos'))!;
    const probeBtn = mefRow.querySelector('button[aria-label="Re-probe"]') as HTMLButtonElement;
    await fireEvent.click(probeBtn);
    await tick();
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.some((c) => c[0] === 'probe_host')).toBe(true);
  });

  it('clicking + Add host opens the AddHostPicker', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('settings-add-host'));
    await tick(); await tick();
    expect(screen.getByRole('dialog', { name: 'Add SSH host' })).toBeInTheDocument();
  });

  it('Account column shows email (seatTier) when account is known', async () => {
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: 'u1' },
    ]);
    accountsStore.set([
      { uuid: 'u1', email: 'm.janci@32bit.sk', display_name: 'Martin', organization_name: '32bit', organization_uuid: 'org-1', seat_tier: 'max', last_seen_at: 1 },
    ]);
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const cells = await screen.findAllByTestId('account-cell');
    const mefRow = cells[0];  // single row in this test
    expect(mefRow.textContent).toContain('m.janci@32bit.sk');
    expect(mefRow.textContent).toContain('max');
  });

  it('renders the Control API section and toggling enable calls mcp_configure', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const section = await screen.findByTestId('mcp-section');
    expect(section.textContent).toContain('Control API');
    const toggle = screen.getByTestId('mcp-enable') as HTMLInputElement;
    await fireEvent.click(toggle);
    await tick();
    expect(
      (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.some(
        (c) => c[0] === 'mcp_configure',
      ),
    ).toBe(true);
  });

  it('Account column shows — when host has no account', async () => {
    hosts.set([
      { alias: 'noaccount', ssh_alias: 'noaccount', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null },
    ]);
    accountsStore.set([]);
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const cells = await screen.findAllByTestId('account-cell');
    const noRow = cells[0];
    expect(noRow.textContent?.trim()).toBe('—');
  });
});
