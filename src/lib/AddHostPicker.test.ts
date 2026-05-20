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
    await tick(); await tick();
    const rows = await screen.findAllByTestId('picker-row');
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain('mefistos');
    expect(rows[1].textContent).toContain('mac');
  });

  it('clicking a row calls add_host', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') {
        return [{ alias: 'mefistos', hostname: null, user: null, port: null }];
      }
      if (cmd === 'add_host') return { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1 };
      if (cmd === 'list_hosts') return [];
      return null;
    });
    let closed = false;
    render(AddHostPicker, { props: { onClose: () => { closed = true; } } });
    await tick(); await tick();
    const row = await screen.findByTestId('picker-row');
    await fireEvent.click(row);
    await tick(); await tick();
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.some((c) => c[0] === 'add_host')).toBe(true);
    expect(closed).toBe(true);
  });

  it('shows an empty-state when ~/.ssh/config has no hosts', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_hosts') return [];
      return null;
    });
    render(AddHostPicker, { props: { onClose: () => {} } });
    for (let i = 0; i < 8; i++) await tick();
    expect(screen.queryByTestId('picker-row')).toBeNull();
    expect(document.body.textContent).toContain('No hosts found');
  });
});
