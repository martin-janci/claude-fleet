import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { hosts, loadHosts, addHost, probeHost, removeHost, hideHost } from './hosts';

const sampleLocal = {
  alias: 'local',
  ssh_alias: null,
  reachable: true,
  claude_version: '2.1.145',
  tmux_version: '3.5a',
  hidden: false,
  last_pinged_at: 1,
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  hosts.set([]);
  localStorage.clear();
});

describe('hosts store', () => {
  it('loadHosts populates the store on success', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sampleLocal]);
    const r = await loadHosts();
    expect(r.ok).toBe(true);
    expect(get(hosts)).toHaveLength(1);
    expect(get(hosts)[0].alias).toBe('local');
  });

  it('addHost passes alias + ssh_alias and reloads', async () => {
    const added = { ...sampleLocal, alias: 'mefistos', ssh_alias: 'mefistos' };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(added);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sampleLocal, added]);
    const r = await addHost('mefistos', 'mefistos');
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'add_host',
      { args: { alias: 'mefistos', ssh_alias: 'mefistos' } },
    ]);
    expect(get(hosts)).toHaveLength(2);
  });

  it('probeHost re-fetches the list', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sampleLocal);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sampleLocal]);
    const r = await probeHost('local');
    expect(r.ok).toBe(true);
  });

  it('removeHost calls remove_host and reloads', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(null);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sampleLocal]);
    const r = await removeHost('mefistos');
    expect(r.ok).toBe(true);
  });

  it('hideHost passes the hidden flag', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(null);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([sampleLocal]);
    const r = await hideHost('mefistos', true);
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'hide_host',
      { args: { alias: 'mefistos', hidden: true } },
    ]);
  });
});
