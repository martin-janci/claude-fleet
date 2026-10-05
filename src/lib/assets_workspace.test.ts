import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  keyOf, parseKey, scopeBadge, canWrite, summarizeRun, ago, assetHistory,
  loadChangesets, changesetSummaries, loadCatalogStatuses, catalogStatuses,
  loadLayers, layerListing, repoStatusOf, isOpenCard, blockedOnSecrets, type ChangesetSummary, type ChangesetItem,
} from './assets_workspace';
import type { SyncRunSummary } from './assets';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
beforeEach(() => { invoke.mockReset(); changesetSummaries.set(null); catalogStatuses.set(null); });

describe('selection keys', () => {
  it('round-trip every kind of row, names with colons included', () => {
    for (const s of [
      { type: 'asset' as const, catalog: 'papayapos', kind: 'skill', name: 'ppt-implement' },
      { type: 'identity' as const, kind: 'skill', name: 'superpowers:brainstorming' },
      { type: 'orphan' as const, kind: 'hook', name: 'stop' },
      { type: 'card' as const, id: 12 },
    ]) expect(parseKey(keyOf(s))).toEqual(s);
    expect(parseKey('nonsense')).toBeNull();
    expect(parseKey('card:x')).toBeNull();
  });
});

describe('scope and write rights', () => {
  it('names the scope in words, private by its dashed border', () => {
    expect(scopeBadge({ catalog: 'papayapos' })).toMatchObject({ label: 'papayapos', tone: 'accent', dashed: false });
    expect(scopeBadge({ catalog: 'personal', scope: 'shared' })).toMatchObject({ label: 'shared', dashed: false });
    expect(scopeBadge({})).toMatchObject({ label: 'private', dashed: true });
  });
  it('writes: never read-only; always standalone or personal; an org catalog only when granted', () => {
    const statuses = [{ id: 2, name: 'acme', org_id: 7, repo_path: '/a', remote_url: null, head_commit: null, last_loaded_at: null, state: 'loaded' as const, asset_count: 0, granted: ['desk'] }];
    expect(canWrite('personal', { readOnly: true, remote: true, clientName: 'desk', statuses })).toBe(false);
    expect(canWrite('acme', { readOnly: false, remote: false, clientName: null, statuses: null })).toBe(true);
    expect(canWrite('acme', { readOnly: false, remote: true, clientName: 'desk', statuses })).toBe(true);
    expect(canWrite('acme', { readOnly: false, remote: true, clientName: 'phone', statuses })).toBe(false);
    expect(canWrite(undefined, { readOnly: false, remote: true, clientName: null, statuses: null })).toBe(true);
  });
});

describe('words', () => {
  it('summarises a run and an age', () => {
    expect(summarizeRun({ plan_id: 'p', started_at: 1, finished_at: 2, hosts: [
      { host_alias: 'a', harness: 'claude', status: 'applied', detail: null, restart_required: false, actions: [] },
      { host_alias: 'b', harness: 'claude', status: 'applied', detail: null, restart_required: false, actions: [] },
    ] })).toContain('2 applied');
    expect(ago(100, 130)).toBe('just now');
    expect(ago(0, 180)).toBe('3 min ago');
    expect(ago(0, 7200)).toBe('2 h ago');
    expect(ago(0, 172800)).toBe('2 d ago');
  });
});

describe('reads', () => {
  it('loads the cards, and reads a refusal as "not available here"', async () => {
    invoke.mockResolvedValueOnce([{ id: 1, kind: 'new', summary: 's', state: 'proposed', created_at: 1 }]);
    await loadChangesets();
    expect(invoke).toHaveBeenCalledWith('catalog_list_changesets', undefined);
    expect(get(changesetSummaries)).toHaveLength(1);
    invoke.mockRejectedValueOnce({ code: 'E_FORBIDDEN', message: 'no' });
    await loadCatalogStatuses();
    expect(get(catalogStatuses)).toBeNull();
  });
  it('asks for History with the catalog only when it is not personal', async () => {
    invoke.mockResolvedValue([]);
    await assetHistory('skill', 'w', 'personal');
    expect(invoke).toHaveBeenLastCalledWith('catalog_asset_history', { args: { kind: 'skill', name: 'w', catalog: null } });
    await assetHistory('skill', 'w', 'acme');
    expect(invoke).toHaveBeenLastCalledWith('catalog_asset_history', { args: { kind: 'skill', name: 'w', catalog: 'acme' } });
  });
});

describe('more reads and keys', () => {
  it('reads the layers, and the repo status of a named catalog', async () => {
    invoke.mockResolvedValueOnce({ layers: [], hosts: [] });
    await loadLayers();
    expect(invoke).toHaveBeenLastCalledWith('catalog_list_layers', undefined);
    expect(get(layerListing)).toEqual({ layers: [], hosts: [] });
    invoke.mockResolvedValueOnce({ head: 'a', dirty: 0, ahead: 0, behind: 0, has_upstream: true });
    await repoStatusOf('acme');
    expect(invoke).toHaveBeenLastCalledWith('catalog_repo_status_in', { args: { name: 'acme' } });
  });
  it('a refused layers read clears the listing', async () => {
    layerListing.set({ layers: [], hosts: [] });
    invoke.mockRejectedValueOnce({ code: 'E_FORBIDDEN', message: 'no' });
    await loadLayers();
    expect(get(layerListing)).toBeNull();
  });
  it('only proposed and failed cards are open', () => {
    const c = (state: ChangesetSummary['state']): ChangesetSummary => ({ id: 1, kind: 'new', summary: '', state, created_at: 1 });
    expect(['proposed', 'failed', 'applied', 'undone', 'dismissed'].map((s) => isOpenCard(c(s as ChangesetSummary['state'])))).toEqual([true, true, false, false, false]);
  });
  it('keys an asset without a catalog as personal, and rejects a malformed key', () => {
    expect(parseKey('asset:personal:skill/x')).toEqual({ type: 'asset', catalog: 'personal', kind: 'skill', name: 'x' });
    expect(keyOf({ type: 'asset', catalog: 'acme', kind: 'skill', name: 'a/b' })).toBe('asset:acme:skill/a/b');
    expect(parseKey('asset:skill/x')).toBeNull();
    expect(parseKey('asset:acme:/x')).toBeNull();
    expect(parseKey('card:')).toBeNull();
    expect(parseKey('card:1.5')).toBeNull();
    expect(parseKey('other:skill/x')).toBeNull();
  });
  it('words a run for the footer, and says no hosts for none', () => {
    expect(summarizeRun({ plan_id: 'p', started_at: 1, finished_at: 2, hosts: [] })).toContain('no hosts');
  });
  it('types carry the M5 fields', () => {
    const item: ChangesetItem = { changeset_id: 1, position: 0, grp: 'core', kind: 'skill', name: 's', action: 'import', decider: 'rule', state: 'applied', decided_at: 1790000000000 };
    const run: SyncRunSummary = { plan_id: 'p', started_at: 1, finished_at: 2, hosts: [], auto: true };
    expect(item.decided_at).toBe(1790000000000);
    expect(run.auto).toBe(true);
  });
});

describe('blockedOnSecrets', () => {
  const act = (kind: string, name: string, op: string, detail: string | null) => ({ kind, name, op: op as 'blocked', outcome: op, detail });
  const run = (actions: ReturnType<typeof act>[]): SyncRunSummary => ({
    plan_id: 'p', started_at: 1, finished_at: 2,
    hosts: [{ host_alias: 'oci', harness: 'claude', status: 'partial', detail: null, restart_required: false, actions }],
  });
  it('names the assets the last run could not apply for want of a secret', () => {
    const b = blockedOnSecrets(run([
      act('mcp', 'fleet', 'blocked', 'missing secrets: FLEET_MCP_TOKEN'),
      act('skill', 'x', 'blocked', 'unsupported kind'),
      act('skill', 'y', 'update', null),
    ]));
    expect(b({ kind: 'mcp', name: 'fleet' })).toBe(true);
    expect(b({ kind: 'skill', name: 'x' })).toBe(false);
    expect(b({ kind: 'skill', name: 'y' })).toBe(false);
  });
  it('is false for everything with no run', () => {
    expect(blockedOnSecrets(null)({ kind: 'mcp', name: 'fleet' })).toBe(false);
  });
});
