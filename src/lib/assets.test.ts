import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  inventory, catalog, catalogConfig,
  loadAssets, scanHosts, importHost, configureCatalog, loadCatalog,
  mergeInventoryRow, clearInventoryFor, groupByKind, stateCounts,
  type AssetInventoryRow, type AssetListing,
} from './assets';

const row = (over: Partial<AssetInventoryRow> = {}): AssetInventoryRow => ({
  host_alias: 'local', harness: 'claude', kind: 'skill', name: 's', state: 'in_sync',
  catalog_hash: 'c', host_hash: 'h', scanned_at: 1, ...over,
});

const listing: AssetListing = {
  head: 'abc', loaded_at: 1, problems: [],
  unmanaged: [row({ name: 'extra', state: 'unmanaged' })],
  assets: [
    { kind: 'skill', name: 's', version: '1', description: 'd', tags: [], hosts: [
      { host_alias: 'local', harness: 'claude', state: 'in_sync' },
      { host_alias: 'mefistos', harness: 'claude', state: 'drifted' },
    ] },
    { kind: 'agent', name: 'pm', version: '1', description: 'd', tags: [], hosts: [] },
  ],
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  inventory.set([]); catalog.set(null); catalogConfig.set(null);
});

describe('assets store', () => {
  it('loadAssets populates catalog', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(listing);
    const r = await loadAssets();
    expect(r.ok).toBe(true);
    expect(get(catalog)?.assets).toHaveLength(2);
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_list_assets', undefined);
  });

  it('configureCatalog and loadCatalog pass args and patch config', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ repo_path: '/r', remote_url: null, head_commit: null, last_loaded_at: null });
    await configureCatalog('/r', '');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_configure', { args: { repo_path: '/r', remote_url: null } });
    expect(get(catalogConfig)?.repo_path).toBe('/r');
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ head: 'h', loaded_at: 2, asset_count: 1, problem_count: 0 });
    const r = await loadCatalog(true);
    expect(r.ok).toBe(true);
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_load', { args: { pull: true } });
    expect(get(catalogConfig)?.head_commit).toBe('h');
  });

  it('scanHosts and importHost pass optional args', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    await scanHosts();
    expect(mockedInvoke).toHaveBeenCalledWith('assets_scan_hosts', { args: { host_alias: null } });
    await scanHosts('mefistos');
    expect(mockedInvoke).toHaveBeenCalledWith('assets_scan_hosts', { args: { host_alias: 'mefistos' } });
    await importHost('local', true);
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_import_host', { args: { host_alias: 'local', dry_run: true } });
  });

  it('mergeInventoryRow upserts by identity and clearInventoryFor prunes one host+harness', () => {
    mergeInventoryRow(row());
    mergeInventoryRow(row({ state: 'drifted' }));
    mergeInventoryRow(row({ host_alias: 'mefistos' }));
    expect(get(inventory)).toHaveLength(2);
    expect(get(inventory).find((r) => r.host_alias === 'local')?.state).toBe('drifted');
    clearInventoryFor('local', 'claude');
    expect(get(inventory)).toHaveLength(1);
    expect(get(inventory)[0].host_alias).toBe('mefistos');
  });

  it('groupByKind keeps kind order and stateCounts tallies', () => {
    const groups = groupByKind(listing);
    expect(groups.map((g) => g.kind)).toEqual(['skill', 'agent']);
    expect(groups[0].assets[0].name).toBe('s');
    expect(stateCounts(listing.assets[0].hosts)).toEqual({ in_sync: 1, drifted: 1, missing: 0, unsupported: 0 });
  });
});
