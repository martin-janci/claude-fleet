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
  { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false },
  { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false },
];

const mcpStatusObj = {
  enabled: false,
  running: false,
  port: 4180,
  token: 'test-token',
  url: 'http://127.0.0.1:4180/mcp',
  bind_error: null,
  confirm_destructive: false,
};

const hostTokens = [
  { host_alias: 'mefistos', mode: 'full', created_at: 1 },
];

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
      case 'list_host_tokens':
        return hostTokens;
      case 'set_host_token_mode':
        return { host_alias: 'mefistos', mode: 'readonly', created_at: 1 };
      case 'rotate_host_token':
        return { host_alias: 'mefistos', mode: 'full', created_at: 2 };
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
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: 'u1', provisioned: false },
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

  it('toggling destructive-call confirmation calls mcp_configure with confirm_destructive', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const toggle = (await screen.findByTestId('mcp-confirm-destructive')) as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    await fireEvent.click(toggle);
    await tick();
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find(
      (c) => c[0] === 'mcp_configure',
    );
    expect(call).toBeDefined();
    expect((call![1] as { args: { confirm_destructive: boolean } }).args.confirm_destructive).toBe(true);
  });

  it('Token column shows the mode for provisioned hosts and "none" otherwise', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    const cells = await screen.findAllByTestId('token-cell');
    const byRow = (alias: string) =>
      Array.from(document.querySelectorAll('.hosts-table tbody tr'))
        .find((r) => r.textContent?.includes(alias))!
        .querySelector('[data-testid="token-cell"]')!;
    expect(cells.length).toBe(2);
    expect(byRow('local').textContent).toContain('none');
    const mefSelect = byRow('mefistos').querySelector('select') as HTMLSelectElement;
    expect(mefSelect.value).toBe('full');
    // Changing the mode invokes set_host_token_mode with the new value.
    await fireEvent.change(mefSelect, { target: { value: 'readonly' } });
    await tick();
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find(
      (c) => c[0] === 'set_host_token_mode',
    );
    expect(call).toBeDefined();
    expect((call![1] as { hostAlias: string; mode: string }).mode).toBe('readonly');
    // Rotate invokes rotate_host_token for that host only.
    const rotate = byRow('mefistos').querySelector('button[aria-label="Rotate token"]') as HTMLButtonElement;
    await fireEvent.click(rotate);
    await tick();
    const rot = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find(
      (c) => c[0] === 'rotate_host_token',
    );
    expect((rot![1] as { hostAlias: string }).hostAlias).toBe('mefistos');
  });

  it('Account column shows — when host has no account', async () => {
    hosts.set([
      { alias: 'noaccount', ssh_alias: 'noaccount', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false },
    ]);
    accountsStore.set([]);
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const cells = await screen.findAllByTestId('account-cell');
    const noRow = cells[0];
    expect(noRow.textContent?.trim()).toBe('—');
  });
});

describe('SettingsDialog automation + notifications (W2 Track D)', () => {
  it('renders the playbook and GC controls off by default', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    expect(screen.getByTestId('automation-section')).toBeInTheDocument();
    expect(screen.getByTestId('playbook-press-enter')).not.toBeChecked();
    expect(screen.getByTestId('playbook-oom-recreate')).not.toBeChecked();
    expect(screen.getByTestId('gc-enabled')).not.toBeChecked();
    expect((screen.getByTestId('gc-bg-hours') as HTMLInputElement).value).toBe('24');
    expect((screen.getByTestId('gc-shell-hours') as HTMLInputElement).value).toBe('168');
    expect((screen.getByTestId('gc-work-hours') as HTMLInputElement).value).toBe('0');
    expect(mockedInvoke).toHaveBeenCalledWith('get_fleet_settings', undefined);
  });

  it('toggling a playbook writes the setting through set_fleet_setting', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string, args?: { key?: string; value?: string }) => {
      if (cmd === 'set_fleet_setting') return { [args!.key!]: args!.value! };
      if (cmd === 'mcp_status') return mcpStatusObj;
      return null;
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('playbook-press-enter'));
    await tick();
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'playbooks.press_enter', value: 'true' });
    expect(screen.getByTestId('playbook-press-enter')).toBeChecked();
  });

  it('GC TTL inputs convert hours to seconds on the wire', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string, args?: { key?: string; value?: string }) => {
      if (cmd === 'set_fleet_setting') return { [args!.key!]: args!.value! };
      if (cmd === 'mcp_status') return mcpStatusObj;
      return null;
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    const input = screen.getByTestId('gc-bg-hours') as HTMLInputElement;
    input.value = '1.5';
    await fireEvent.change(input);
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'gc.bg_idle_secs', value: '5400' });
  });

  it('renders the notifications section with the toast toggle on and OS off', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    expect(screen.getByTestId('notifications-section')).toBeInTheDocument();
    expect(screen.getByTestId('notify-toast')).toBeChecked();
    expect(screen.getByTestId('notify-os')).not.toBeChecked();
    // jsdom has no Notification API ⇒ the OS toggle is disabled + labelled.
    expect(screen.getByTestId('notify-permission')).toHaveTextContent('unsupported');
    expect(screen.getByTestId('notify-os')).toBeDisabled();
  });
});
