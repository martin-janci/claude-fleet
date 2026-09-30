import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  hosts,
  loadHosts,
  addHost,
  probeHost,
  deleteHost,
  hideHost,
  applyHostEvents,
  resetTombstonesForTests,
  isPickableHost,
  defaultHost,
  setHostHarnesses,
  codexModeOf,
  harnessesFor,
} from './hosts';

const sampleLocal = {
  alias: 'local',
  ssh_alias: null,
  reachable: true,
  claude_version: '2.1.145',
  tmux_version: '3.5a',
  hidden: false,
  last_pinged_at: 1,
  account_uuid: null,
  provisioned: false,
  transport: 'ssh' as const,
};

beforeEach(() => {
  resetTombstonesForTests();
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  hosts.set([]);
  localStorage.clear();
});

describe('hosts store', () => {
  // A probe that found the host exactly as it was sends three fields instead
  // of the whole row: reconcile probes every host every pass, so the full row
  // could never be diffed away and every client paid for it.
  it('a pinged event patches the row it already holds', () => {
    hosts.set([sampleLocal]);

    applyHostEvents([{ type: 'pinged', alias: 'local', last_pinged_at: 99, reachable: true }]);

    const row = get(hosts)[0];
    expect(row.last_pinged_at).toBe(99);
    expect(row.claude_version).toBe('2.1.145');
    expect(row.transport).toBe('ssh');
  });

  it('a pinged event for a host we do not hold invents nothing', () => {
    hosts.set([sampleLocal]);

    applyHostEvents([{ type: 'pinged', alias: 'ghosty', last_pinged_at: 99, reachable: false }]);

    expect(get(hosts).map((h) => h.alias)).toEqual(['local']);
  });

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

describe('defaultHost', () => {
  const row = (alias: string, over: Partial<typeof sampleLocal> = {}) => ({
    ...sampleLocal,
    alias,
    ssh_alias: alias === 'local' ? null : alias,
    ...over,
  });

  it('is local while local is pickable', () => {
    expect(defaultHost([row('devbox'), row('local')])).toBe('local');
    // `local` counts as pickable even unreachable: its state is this machine's.
    expect(defaultHost([row('devbox'), row('local', { reachable: false })])).toBe('local');
  });

  it('skips the hidden local row of a hub with hub.local_host=false', () => {
    const list = [
      row('local', { hidden: true, reachable: false }),
      row('offline', { reachable: false }),
      row('hidden', { hidden: true }),
      row('devbox'),
    ];
    expect(isPickableHost(list, 'local')).toBe(false);
    expect(defaultHost(list)).toBe('devbox');
  });

  it('falls back to local when nothing is pickable', () => {
    expect(defaultHost([])).toBe('local');
    expect(defaultHost([row('local', { hidden: true })])).toBe('local');
  });
});

describe('host harnesses (F3a)', () => {
  it('reads Codex as auto / on / off from the harnesses field', () => {
    expect(codexModeOf({})).toBe('auto');
    expect(codexModeOf({ harnesses: null })).toBe('auto');
    expect(codexModeOf({ harnesses: ['claude', 'codex'] })).toBe('on');
    expect(codexModeOf({ harnesses: ['claude'] })).toBe('off');
  });

  it('a mode stores null for auto and always keeps claude in a list', () => {
    expect(harnessesFor('auto')).toBeNull();
    expect(harnessesFor('on')).toEqual(['claude', 'codex']);
    expect(harnessesFor('off')).toEqual(['claude']);
  });

  it('setHostHarnesses sends the list and merges the answered row', async () => {
    hosts.set([sampleLocal]);
    const answered = { ...sampleLocal, harnesses: ['claude', 'codex'] };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(answered);
    const r = await setHostHarnesses('local', ['claude', 'codex']);
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'catalog_set_host_harnesses',
      { args: { host_alias: 'local', harnesses: ['claude', 'codex'] } },
    ]);
    expect(get(hosts)[0].harnesses).toEqual(['claude', 'codex']);
  });
});
