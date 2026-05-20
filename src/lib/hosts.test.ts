import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { hosts, loadHosts, addHost, probeHost, deleteHost, hideHost } from './hosts';

const sampleLocal = {
  alias: 'local',
  ssh_alias: null,
  reachable: true,
  claude_version: '2.1.145',
  tmux_version: '3.5a',
  hidden: false,
  last_pinged_at: 1,
  account_uuid: null,
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

  it('addHost passes alias + ssh_alias and merges into store', async () => {
    const added = { ...sampleLocal, alias: 'mefistos', ssh_alias: 'mefistos' };
    hosts.set([sampleLocal]);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(added);
    const r = await addHost('mefistos', 'mefistos');
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'add_host',
      { args: { alias: 'mefistos', ssh_alias: 'mefistos' } },
    ]);
    expect(get(hosts)).toHaveLength(2);
  });

  it('probeHost merges result into store', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sampleLocal);
    const r = await probeHost('local');
    expect(r.ok).toBe(true);
    expect(get(hosts)).toHaveLength(1);
    expect(get(hosts)[0].alias).toBe('local');
  });

  it('deleteHost calls remove_host and removes from store', async () => {
    const mefistos = { ...sampleLocal, alias: 'mefistos', ssh_alias: 'mefistos' };
    hosts.set([sampleLocal, mefistos]);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(mefistos);
    const r = await deleteHost('mefistos');
    expect(r.ok).toBe(true);
    expect(get(hosts)).toHaveLength(1);
    expect(get(hosts)[0].alias).toBe('local');
  });

  it('hideHost passes the hidden flag and merges into store', async () => {
    const mefistos = { ...sampleLocal, alias: 'mefistos', ssh_alias: 'mefistos', hidden: false };
    const mefistosHidden = { ...mefistos, hidden: true };
    hosts.set([mefistos]);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(mefistosHidden);
    const r = await hideHost('mefistos', true);
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'hide_host',
      { args: { alias: 'mefistos', hidden: true } },
    ]);
    expect(get(hosts)[0].hidden).toBe(true);
  });
});
