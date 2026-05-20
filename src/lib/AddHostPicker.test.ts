import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AddHostPicker from './AddHostPicker.svelte';
import { hosts } from './hosts';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  hosts.set([]);
});

describe('AddHostPicker', () => {
  it('lists discovered hosts', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') {
        return [
          { alias: 'mefistos', hostname: '192.168.1.50', user: 'mjanci', port: 22 },
          { alias: 'mac', hostname: null, user: null, port: null },
        ];
      }
      return null;
    });
    render(AddHostPicker, { props: { onClose: () => {} } });
    for (let i = 0; i < 8; i++) await tick();
    const rows = await screen.findAllByTestId('picker-row');
    expect(rows).toHaveLength(2);
  });

  it('clicking a row probes (without adding yet)', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') return [{ alias: 'mefistos', hostname: null, user: null, port: null }];
      if (cmd === 'probe_ssh_alias') return {
        reachable: true,
        claude_version: '2.1.144',
        tmux_version: '3.6a',
        account: { uuid: 'u1', email: 'm.janci@32bit.sk', display_name: 'M', organization_name: null, organization_uuid: null, seat_tier: 'max' },
      };
      return null;
    });
    render(AddHostPicker, { props: { onClose: () => {} } });
    for (let i = 0; i < 8; i++) await tick();
    const row = await screen.findByTestId('picker-row');
    await fireEvent.click(row);
    for (let i = 0; i < 8; i++) await tick();
    // Preview is shown, NOT yet added
    const preview = await screen.findByTestId('preview-result');
    expect(preview).toBeInTheDocument();
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls.some((c) => c[0] === 'probe_ssh_alias')).toBe(true);
    expect(calls.some((c) => c[0] === 'add_host')).toBe(false);
  });

  it('preview shows account email + seatTier', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') return [{ alias: 'mefistos', hostname: null, user: null, port: null }];
      if (cmd === 'probe_ssh_alias') return {
        reachable: true,
        claude_version: '2.1.144',
        tmux_version: '3.6a',
        account: { uuid: 'u1', email: 'm.janci@32bit.sk', display_name: 'M', organization_name: null, organization_uuid: null, seat_tier: 'max' },
      };
      return null;
    });
    render(AddHostPicker, { props: { onClose: () => {} } });
    for (let i = 0; i < 8; i++) await tick();
    await fireEvent.click(await screen.findByTestId('picker-row'));
    for (let i = 0; i < 8; i++) await tick();
    const accountCell = screen.getByTestId('preview-account');
    expect(accountCell.textContent).toContain('m.janci@32bit.sk');
    expect(accountCell.textContent).toContain('max');
  });

  it('preview shows — when account is missing', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') return [{ alias: 'noaccount', hostname: null, user: null, port: null }];
      if (cmd === 'probe_ssh_alias') return {
        reachable: true,
        claude_version: null,
        tmux_version: '3.6a',
        account: null,
      };
      return null;
    });
    render(AddHostPicker, { props: { onClose: () => {} } });
    for (let i = 0; i < 8; i++) await tick();
    await fireEvent.click(await screen.findByTestId('picker-row'));
    for (let i = 0; i < 8; i++) await tick();
    const accountCell = screen.getByTestId('preview-account');
    expect(accountCell.textContent).toContain('—');
  });

  it('confirm-Add invokes add_host then closes', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') return [{ alias: 'mefistos', hostname: null, user: null, port: null }];
      if (cmd === 'probe_ssh_alias') return { reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', account: null };
      if (cmd === 'add_host') return { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null };
      if (cmd === 'list_hosts') return [];
      return null;
    });
    let closed = false;
    render(AddHostPicker, { props: { onClose: () => { closed = true; } } });
    for (let i = 0; i < 8; i++) await tick();
    await fireEvent.click(await screen.findByTestId('picker-row'));
    for (let i = 0; i < 8; i++) await tick();
    await fireEvent.click(await screen.findByTestId('preview-confirm'));
    for (let i = 0; i < 8; i++) await tick();
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls.some((c) => c[0] === 'add_host')).toBe(true);
    expect(closed).toBe(true);
  });
});
