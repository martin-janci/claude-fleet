import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  inventory, catalog, catalogConfig,
  loadAssets, scanHosts, importHost, configureCatalog, loadCatalog,
  mergeInventoryRow, clearInventoryFor, groupByKind, stateCounts,
  planSync, applySync, lastSync, listSecrets, setSecret, deleteSecret, isDestructive, lastSyncRun,
  createAsset, updateAsset, deleteAsset, addResource, removeResource, lintAsset, lintAll,
  commitPending, pushCatalog, repoStatus, assetTemplate, spawnAuthorSession, resourceSize,
  repoStatusStore, KIND_FIELDS, TOOLS, TIERS, EVENTS,
  type AssetInventoryRow, type AssetListing, type SyncPlan, type HostPlan, type SyncAction,
  type EditableAsset,
} from './assets';

const row = (over: Partial<AssetInventoryRow> = {}): AssetInventoryRow => ({
  host_alias: 'local', harness: 'claude', kind: 'skill', name: 's', state: 'in_sync',
  catalog_hash: 'c', host_hash: 'h', scanned_at: 1, managed: true, ...over,
});

const syncAction = (over: Partial<SyncAction> = {}): SyncAction => ({
  kind: 'skill', name: 's', op: 'create', reason: null, files: [], merges: [],
  backup: false, secrets: [], missing_secrets: [], ...over,
});

const hostPlan = (over: Partial<HostPlan> = {}): HostPlan => ({
  host_alias: 'local', harness: 'claude', status: 'planned', detail: null, actions: [], ...over,
});

const syncPlan = (hosts: HostPlan[]): SyncPlan => ({
  id: 'plan-1', computed_at: 1, hosts, counts: {},
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
  inventory.set([]); catalog.set(null); catalogConfig.set(null); lastSyncRun.set(null); repoStatusStore.set(null);
});

const editableAsset = (over: Partial<EditableAsset> = {}): EditableAsset => ({
  kind: 'skill', name: 'worktree', version: '1', description: 'd', tags: [], body: '# b',
  resources: [], allowed_tools: [], user_invocable: true, triggers: [], ...over,
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

describe('sync engine wrappers', () => {
  it('planSync sends null for every absent filter field', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(syncPlan([]));
    await planSync({});
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_plan_sync', { args: { host_alias: null, kind: null, name: null } });
  });

  it('planSync passes through the fields given', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(syncPlan([]));
    await planSync({ hostAlias: 'mefistos', kind: 'skill', name: 'worktree' });
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_plan_sync', {
      args: { host_alias: 'mefistos', kind: 'skill', name: 'worktree' },
    });
  });

  it('applySync goes through invokeCmdAbortable with plan_id and force_partial, injecting a call_id', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ plan_id: 'plan-1', started_at: 1, finished_at: 2, hosts: [] });
    const r = await applySync('plan-1', true);
    expect(r.ok).toBe(true);
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'catalog_apply_sync');
    expect(call).toBeDefined();
    const args = (call![1] as { args: { plan_id: string; force_partial: boolean; call_id: number } }).args;
    expect(args.plan_id).toBe('plan-1');
    expect(args.force_partial).toBe(true);
    expect(args.call_id).toEqual(expect.any(Number));
  });

  it('lastSync populates lastSyncRun', async () => {
    const summary = { plan_id: 'plan-1', started_at: 1, finished_at: 2, hosts: [] };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(summary);
    const r = await lastSync();
    expect(r.ok).toBe(true);
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_last_sync', undefined);
    expect(get(lastSyncRun)).toEqual(summary);
  });

  it('lastSync tolerates a null result (no run yet)', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(null);
    const r = await lastSync();
    expect(r.ok).toBe(true);
    expect(get(lastSyncRun)).toBeNull();
  });

  it('listSecrets calls catalog_list_secrets with no args', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]);
    await listSecrets();
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_list_secrets', undefined);
  });

  it('setSecret defaults host_alias to null', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(null);
    await setSecret('GH_TOKEN', 'shh');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_set_secret', { args: { name: 'GH_TOKEN', host_alias: null, value: 'shh' } });
  });

  it('setSecret passes a host_alias override through', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(null);
    await setSecret('GH_TOKEN', 'shh', 'mefistos');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_set_secret', { args: { name: 'GH_TOKEN', host_alias: 'mefistos', value: 'shh' } });
  });

  it('deleteSecret defaults host_alias to null and passes an override through', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(true);
    await deleteSecret('GH_TOKEN');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_delete_secret', { args: { name: 'GH_TOKEN', host_alias: null } });
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(true);
    await deleteSecret('GH_TOKEN', 'mefistos');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_delete_secret', { args: { name: 'GH_TOKEN', host_alias: 'mefistos' } });
  });

  it('isDestructive is false when every action is create/update/noop/blocked', () => {
    const plan = syncPlan([hostPlan({ actions: [syncAction({ op: 'create' }), syncAction({ op: 'update' }), syncAction({ op: 'noop' }), syncAction({ op: 'blocked' })] })]);
    expect(isDestructive(plan)).toBe(false);
  });

  it('isDestructive is true when any action overwrites', () => {
    const plan = syncPlan([hostPlan({ actions: [syncAction({ op: 'create' }), syncAction({ op: 'overwrite' })] })]);
    expect(isDestructive(plan)).toBe(true);
  });

  it('isDestructive is true when any action removes', () => {
    const plan = syncPlan([hostPlan({ actions: [syncAction({ op: 'remove' })] }), hostPlan({ host_alias: 'mefistos', actions: [] })]);
    expect(isDestructive(plan)).toBe(true);
  });
});

describe('authoring wrappers', () => {
  it('createAsset defaults duplicate_from to null and passes it through when given', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ commit: 'abc', lint: { errors: [], warnings: [] } });
    await createAsset('skill', 'my-skill');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_create_asset', { args: { kind: 'skill', name: 'my-skill', duplicate_from: null } });
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ commit: 'def', lint: { errors: [], warnings: [] } });
    await createAsset('skill', 'copy', 'my-skill');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_create_asset', { args: { kind: 'skill', name: 'copy', duplicate_from: 'my-skill' } });
  });

  it('updateAsset sends the whole asset and defaults resources to []', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ commit: 'abc', lint: { errors: [], warnings: [] } });
    const asset = editableAsset({ resources: undefined });
    const r = await updateAsset(asset);
    expect(r.ok).toBe(true);
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_update_asset', { args: { asset: { ...asset, resources: [] } } });
  });

  it('updateAsset surfaces E_LINT with the report in details', async () => {
    const report = { errors: [{ field: 'description', message: 'must not be empty' }], warnings: [] };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce({ code: 'E_LINT', message: 'lint errors', details: report });
    const r = await updateAsset(editableAsset());
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.code).toBe('E_LINT');
      expect(r.error.details).toEqual(report);
    }
  });

  it('deleteAsset passes kind and name and returns the commit', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce('sha1');
    const r = await deleteAsset('skill', 'worktree');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_delete_asset', { args: { kind: 'skill', name: 'worktree' } });
    expect(r.ok && r.value).toBe('sha1');
  });

  it('addResource defaults rel_path to null and removeResource requires it', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ commit: 'a', lint: { errors: [], warnings: [] } });
    await addResource('skill', 'worktree', '/tmp/x.sh');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_add_resource', { args: { kind: 'skill', name: 'worktree', local_path: '/tmp/x.sh', rel_path: null } });
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ commit: 'b', lint: { errors: [], warnings: [] } });
    await removeResource('skill', 'worktree', 'resources/x.sh');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_remove_resource', { args: { kind: 'skill', name: 'worktree', rel_path: 'resources/x.sh' } });
  });

  it('lintAsset and lintAll pass through', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ errors: [], warnings: [] });
    await lintAsset('skill', 'worktree');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_lint_asset', { args: { kind: 'skill', name: 'worktree' } });
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ assets: [], problems: [], errors: 0, warnings: 0 });
    await lintAll();
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_lint_all', undefined);
  });

  it('commitPending defaults message to null and passes an override through', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce('sha1');
    await commitPending();
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_commit_pending', { args: { message: null } });
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce('sha2');
    await commitPending('catalog: commit pending changes');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_commit_pending', { args: { message: 'catalog: commit pending changes' } });
  });

  it('pushCatalog and repoStatus call with no args and populate repoStatusStore', async () => {
    const status = { head: 'h', dirty: 0, ahead: 2, behind: 0, has_upstream: true };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(status);
    await pushCatalog();
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_push', undefined);
    expect(get(repoStatusStore)).toEqual(status);

    repoStatusStore.set(null);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(status);
    await repoStatus();
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_repo_status', undefined);
    expect(get(repoStatusStore)).toEqual(status);
  });

  it('assetTemplate passes kind and name', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(editableAsset());
    await assetTemplate('skill', 'new-skill');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_template', { args: { kind: 'skill', name: 'new-skill' } });
  });

  it('spawnAuthorSession goes through invokeCmdAbortable, injecting a call_id and defaulting kind/name to null', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ id: 1, tmux_name: 'catalog-new-abc' });
    const r = await spawnAuthorSession({ instructions: 'Create a new skill that …' });
    expect(r.ok).toBe(true);
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'catalog_spawn_author_session');
    expect(call).toBeDefined();
    const args = (call![1] as { args: { kind: string | null; name: string | null; instructions: string; call_id: number } }).args;
    expect(args.kind).toBeNull();
    expect(args.name).toBeNull();
    expect(args.instructions).toBe('Create a new skill that …');
    expect(args.call_id).toEqual(expect.any(Number));
  });

  it('spawnAuthorSession passes kind/name through when given', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ id: 1, tmux_name: 'catalog-skill-worktree' });
    await spawnAuthorSession({ kind: 'skill', name: 'worktree', instructions: 'Improve this skill: …' });
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'catalog_spawn_author_session');
    const args = (call![1] as { args: { kind: string | null; name: string | null } }).args;
    expect(args.kind).toBe('skill');
    expect(args.name).toBe('worktree');
  });

  it('resourceSize decodes base64 length and tolerates invalid input', () => {
    expect(resourceSize(btoa('hello'))).toBe(5);
    expect(resourceSize('not-valid-base64!!')).toBe(0);
  });

  it('KIND_FIELDS, TOOLS, TIERS and EVENTS mirror the Rust vocabularies', () => {
    expect(KIND_FIELDS.skill).toEqual(['allowed_tools', 'user_invocable', 'triggers']);
    expect(KIND_FIELDS.agent).toEqual(['tools', 'model']);
    expect(KIND_FIELDS.hook).toEqual(['event', 'action']);
    expect(KIND_FIELDS.mcp_server).toEqual(['transport', 'url', 'command', 'args', 'env']);
    expect(KIND_FIELDS.plugin_ref).toEqual(['harness', 'marketplace', 'plugin']);
    expect(TOOLS).toContain('bash');
    expect(TIERS).toEqual(['fast', 'default', 'strong']);
    expect(EVENTS).toContain('session_start');
  });
});
