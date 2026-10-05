import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  keyOf, parseKey, scopeBadge, canWrite, summarizeRun, ago, assetHistory,
  loadChangesets, changesetSummaries, loadCatalogStatuses, catalogStatuses,
  loadLayers, layerListing, repoStatusOf, isOpenCard, blockedOnSecrets, blockedSecretKeys, type ChangesetSummary, type ItemView,
  applyChangeset, rejectItems, undoChangeset, dismissChangeset, getChangeset, proposeChangesets, proposeLayerChange,
  loadOpenCardViews, loadAllLayers, cardViews, layersByCatalog, driftDiff, admitCatalog, hostProvenance,
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
    const item: ItemView = { position: 0, grp: 'core', kind: 'skill', name: 's', action: 'import', params: {}, decider: 'rule', state: 'applied', outcome: { held: [{ kind: 'skill', name: 'w', why: 'edited' }] } };
    const run: SyncRunSummary = { plan_id: 'p', started_at: 1, finished_at: 2, hosts: [], auto: true };
    expect(item.outcome?.held?.[0].why).toBe('edited');
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
  it('keys blocked assets by catalog too', () => {
    const r = {
      plan_id: 'p', started_at: 1, finished_at: 2,
      hosts: [{ host_alias: 'oci', harness: 'claude', status: 'partial', detail: null, restart_required: false,
        actions: [{ kind: 'skill', name: 'w', op: 'blocked', outcome: 'blocked', detail: 'missing secrets: TOKEN', catalog: 'acme' }] }],
    } as never;
    expect(blockedSecretKeys(r)).toEqual(['acme:skill/w']);
    const blocked = blockedOnSecrets(r);
    expect(blocked({ kind: 'skill', name: 'w', catalog: 'acme' })).toBe(true);
    expect(blocked({ kind: 'skill', name: 'w', catalog: 'personal' })).toBe(false);
    expect(blocked({ kind: 'skill', name: 'w' })).toBe(false);
  });
  it('a result without a catalog (a hub before M6) keys as personal', () => {
    const b = blockedOnSecrets(run([act('skill', 'w', 'blocked', 'missing secrets: TOKEN')]));
    expect(b({ kind: 'skill', name: 'w', catalog: 'personal' })).toBe(true);
    expect(b({ kind: 'skill', name: 'w' })).toBe(true);
    expect(b({ kind: 'skill', name: 'w', catalog: 'acme' })).toBe(false);
  });
  it('is false for everything with no run', () => {
    expect(blockedOnSecrets(null)({ kind: 'mcp', name: 'fleet' })).toBe(false);
  });
});

describe('card verbs', () => {
  beforeEach(() => { invoke.mockReset(); });
  it('applies with positions only when given', async () => {
    invoke.mockResolvedValue({ id: 3 });
    await applyChangeset(3);
    expect(invoke).toHaveBeenLastCalledWith('catalog_apply_changeset', { args: { id: 3 } });
    await applyChangeset(3, [1]);
    expect(invoke).toHaveBeenLastCalledWith('catalog_apply_changeset', { args: { id: 3, positions: [1] } });
  });
  it('rejects, undoes, dismisses, gets, proposes', async () => {
    invoke.mockResolvedValue({});
    await rejectItems(3, [0, 1]);
    expect(invoke).toHaveBeenLastCalledWith('catalog_reject_changeset_items', { args: { id: 3, positions: [0, 1] } });
    await undoChangeset(3);
    expect(invoke).toHaveBeenLastCalledWith('catalog_undo_changeset', { args: { id: 3 } });
    await dismissChangeset(3);
    expect(invoke).toHaveBeenLastCalledWith('catalog_dismiss_changeset', { args: { id: 3 } });
    await getChangeset(3);
    expect(invoke).toHaveBeenLastCalledWith('catalog_get_changeset', { args: { id: 3 } });
    await proposeChangesets();
    expect(invoke).toHaveBeenLastCalledWith('catalog_propose_changesets', undefined);
    await proposeLayerChange({ op: 'rename', layer: 'core', to: 'base' });
    expect(invoke).toHaveBeenLastCalledWith('catalog_propose_layer_change', { args: { change: { op: 'rename', layer: 'core', to: 'base' } } });
  });
  it('admits, reads provenance, and sends the drift diff with nulls', async () => {
    invoke.mockResolvedValue({});
    await admitCatalog('mef', 'acme');
    expect(invoke).toHaveBeenLastCalledWith('catalog_admit_catalog', { args: { host_alias: 'mef', catalog: 'acme' } });
    await hostProvenance('oci');
    expect(invoke).toHaveBeenLastCalledWith('catalog_host_provenance', { args: { host_alias: 'oci' } });
    await driftDiff({ host_alias: 'oci', kind: 'skill', name: 'w', catalog: 'personal' });
    expect(invoke).toHaveBeenLastCalledWith('catalog_drift_diff', { args: { host_alias: 'oci', kind: 'skill', name: 'w', harness: null, catalog: null } });
    await driftDiff({ host_alias: 'oci', kind: 'skill', name: 'w', harness: 'claude', catalog: 'acme' });
    expect(invoke).toHaveBeenLastCalledWith('catalog_drift_diff', { args: { host_alias: 'oci', kind: 'skill', name: 'w', harness: 'claude', catalog: 'acme' } });
  });
  it('loads the open cards in full', async () => {
    invoke.mockImplementation(async (_c: string, a: { args: { id: number } }) => ({ id: a.args.id, items: [] }));
    await loadOpenCardViews([
      { id: 1, kind: 'new', summary: '', state: 'proposed', created_at: 1 },
      { id: 2, kind: 'new', summary: '', state: 'applied', created_at: 1 },
      { id: 3, kind: 'rollout', summary: '', state: 'applied', created_at: 1, held_hosts: ['oci'] },
    ]);
    expect(Object.keys(get(cardViews))).toEqual(['1', '3']);
  });
  it('an older overlapping load that finishes last does not overwrite the newer one', async () => {
    cardViews.set({});
    const resolvers: ((v: unknown) => void)[] = [];
    invoke.mockImplementation(() => new Promise((res) => { resolvers.push(res); }));
    const card = { id: 1, kind: 'new' as const, summary: '', state: 'proposed' as const, created_at: 1 };
    const first = loadOpenCardViews([card]);
    const second = loadOpenCardViews([card]);
    resolvers[1]({ id: 1, summary: 'newer', items: [] });
    await second;
    resolvers[0]({ id: 1, summary: 'stale', items: [] });
    await first;
    expect(get(cardViews)[1].summary).toBe('newer');
  });
  it('a failed fetch keeps the card\'s previous view', async () => {
    cardViews.set({});
    const card = { id: 1, kind: 'new' as const, summary: '', state: 'proposed' as const, created_at: 1 };
    invoke.mockResolvedValueOnce({ id: 1, summary: 'kept', items: [] });
    await loadOpenCardViews([card]);
    invoke.mockRejectedValueOnce({ code: 'E_HUB_TIMEOUT', message: 'slow' });
    await loadOpenCardViews([card]);
    expect(get(cardViews)[1].summary).toBe('kept');
  });
  it('loads layers for every loaded catalog by name', async () => {
    invoke.mockResolvedValue({ layers: [], hosts: [] });
    await loadAllLayers([
      { id: 1, name: 'personal', org_id: null, repo_path: '', remote_url: null, head_commit: null, last_loaded_at: null, state: 'loaded', asset_count: 0 },
      { id: 2, name: 'acme', org_id: 7, repo_path: '', remote_url: null, head_commit: null, last_loaded_at: null, state: 'problem', asset_count: 0 },
    ]);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('catalog_list_layers_in', { args: { name: 'personal' } });
    expect(Object.keys(get(layersByCatalog)!)).toEqual(['personal']);
  });
  it('round-trips the new selection keys', () => {
    for (const s of [{ type: 'layer', catalog: 'acme', name: 'core' }, { type: 'host', alias: 'oci' }] as const) {
      expect(parseKey(keyOf(s))).toEqual(s);
    }
  });
});
