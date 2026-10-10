import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetsPanel from './AssetsPanel.svelte';
import { catalog, catalogConfig, inventory, lastSyncRun, repoStatusStore } from './assets';
import { hosts } from './hosts';
import { hubStatus, STANDALONE } from './hub';
import { assetsViewRequest, requestAssetsView } from './app_views';
import { toasts } from './toasts';
import { authorSessionOpened, clearAuthorSessionOpened } from './AuthorSessionDialog.svelte';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

function byCmd(map: Record<string, unknown>) {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd in map) {
      const v = map[cmd];
      if (v instanceof Error || (v && typeof v === 'object' && 'code' in v)) throw v;
      return v;
    }
    throw { code: 'E_TEST', message: `unexpected ${cmd}` };
  });
}

/** The Library rail entry: where the catalog's rows, New asset, Import and
 *  Lint all live since Assets M5 (Rulings R17). */
async function openLibrary() {
  await fireEvent.click(await screen.findByTestId('assets-rail-library'));
}
/** The Layers rail entry (Assets M6, R17). */
async function openLayers() {
  await fireEvent.click(await screen.findByTestId('assets-rail-layers'));
}
/** The personal catalog chip's popover: Pull, Commit pending, Push and the
 *  repo status line (R17, R24). */
async function openPersonalChip() {
  await fireEvent.click(await screen.findByTestId('catalog-chip-personal'));
}

const listing = {
  head: 'abcdef1234567890', loaded_at: 1, problems: [{ path: 'hooks/bad.yaml', message: 'name' }],
  unmanaged: [{ host_alias: 'local', harness: 'claude', kind: 'skill', name: 'extra', state: 'unmanaged', catalog_hash: null, host_hash: null, scanned_at: 1 }],
  assets: [
    { kind: 'skill', name: 'worktree', version: '1', description: 'Make one.', tags: [], hosts: [
      { host_alias: 'local', harness: 'claude', state: 'in_sync' },
      { host_alias: 'mefistos', harness: 'claude', state: 'missing' },
    ] },
    { kind: 'mcp_server', name: 'fleet', version: '1', description: 'd', tags: [], hosts: [] },
  ],
};

beforeEach(() => {
  invoke.mockReset();
  catalog.set(null); catalogConfig.set(null); inventory.set([]); lastSyncRun.set(null); repoStatusStore.set(null);
  clearAuthorSessionOpened();
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true, transport: 'ssh' },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: false, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true, transport: 'ssh' },
  ]);
});

describe('AssetsPanel', () => {
  it('shows the setup card when no catalog is configured and configures on submit', async () => {
    byCmd({ catalog_config: null, catalog_configure: { repo_path: '/r', remote_url: null, head_commit: null, last_loaded_at: null }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 0, problem_count: 0 }, catalog_list_assets: { ...listing, assets: [], unmanaged: [], problems: [] }, assets_inventory: [] });
    render(AssetsPanel);
    await tick(); await tick();
    expect(screen.getByTestId('assets-setup')).toBeTruthy();
    await fireEvent.input(screen.getByTestId('assets-setup-path'), { target: { value: '/r' } });
    await fireEvent.click(screen.getByTestId('assets-setup-submit'));
    // Setup chains configureCatalog -> loadCatalog -> refresh, each an async
    // IPC round trip; wait for the setup card to actually disappear instead
    // of a fixed tick count (Svelte's async-mode scheduler settles a nested
    // async chain over several real event-loop turns, not one microtask).
    await waitFor(() => expect(screen.queryByTestId('assets-setup')).toBeNull());
    expect(invoke).toHaveBeenCalledWith('catalog_configure', { args: { repo_path: '/r', remote_url: null } });
    expect(invoke).toHaveBeenCalledWith('catalog_load', { args: { pull: false } });
    expect(screen.queryByTestId('assets-setup')).toBeNull();
  });

  // `catalog` is only ever set on success, so a failed load used to render
  // the error line AND a permanent "Loading…" at the same time, with no way
  // to try again but the panel header's ↻.
  it('offers a retry instead of a permanent Loading when the catalog fails to load', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { code: 'E_GIT', message: 'not a checkout' },
    });
    render(AssetsPanel);
    const failed = await screen.findByTestId('assets-load-failed');
    expect(failed).toBeTruthy();
    expect(screen.queryByText('Loading…')).toBeNull();
    // The retry re-runs the load rather than leaving the header's ↻ as the
    // only way out.
    invoke.mockClear();
    await fireEvent.click(screen.getByTestId('assets-retry'));
    await waitFor(() => expect(invoke.mock.calls.some((c) => c[0] === 'catalog_load')).toBe(true));
  });

  // The recovery UI above must not hang off a variable that unrelated toolbar
  // handlers reset. Sync (and Push, and Scan hosts) clear `error` on entry and
  // leave it clear on success — which used to turn the Retry block back into a
  // permanent "Loading…" while the catalog was still unloaded.
  it('keeps the failed-load Retry after an unrelated toolbar action succeeds', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { code: 'E_GIT', message: 'not a checkout' },
      catalog_repo_status: { head: 'h', dirty: 0, ahead: 0, behind: 0, has_upstream: false },
      catalog_last_sync: null,
      catalog_plan_sync: { id: 'plan-1', computed_at: 1, hosts: [], counts: {} },
    });
    render(AssetsPanel);
    expect(await screen.findByTestId('assets-load-failed')).toBeTruthy();

    // A successful Sync — nothing to do with the catalog checkout.
    await fireEvent.click(screen.getByTestId('assets-sync'));
    await waitFor(() => expect(invoke.mock.calls.some((c) => c[0] === 'catalog_plan_sync')).toBe(true));
    await tick(); await tick();

    // The plan takes the main column; closing it brings the Retry block back.
    expect(screen.queryByText('Loading…')).toBeNull();
    await fireEvent.click(await screen.findByTestId('plan-back'));
    expect(screen.queryByText('Loading…')).toBeNull();
    expect(screen.getByTestId('assets-load-failed')).toBeTruthy();
    expect(screen.getByTestId('assets-retry')).toBeTruthy();
  });

  it('a refresh reads the layers of every loaded catalog (R17)', async () => {
    const st = (name: string, state: string) => ({ id: 1, name, org_id: null, repo_path: '/r', remote_url: null, head_commit: null, last_loaded_at: 1, state, asset_count: 0 });
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [], catalog_last_sync: null,
      catalog_list_catalogs: [st('personal', 'loaded'), st('acme', 'loaded'), st('idle', 'not_loaded')],
      catalog_list_layers_in: { layers: [{ name: 'core', axis: 'context' }], hosts: [] },
    });
    render(AssetsPanel, { visible: true });
    await openLayers();
    expect(await screen.findByTestId('layers-catalog-acme')).toBeTruthy();
    expect(screen.getByTestId('layers-catalog-personal')).toBeTruthy();
    expect(screen.queryByTestId('layers-catalog-idle')).toBeNull();
    const asked = invoke.mock.calls.filter((c) => c[0] === 'catalog_list_layers_in').map((c) => c[1].args.name).sort();
    expect(asked).toEqual(['acme', 'personal']);
  });

  it('lists assets grouped by kind with state chips, unmanaged group and problems badge', async () => {
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'abcdef1234567890', last_loaded_at: 1 }, catalog_load: { head: 'abcdef1234567890', loaded_at: 1, asset_count: 2, problem_count: 1 }, catalog_list_assets: listing, assets_inventory: [] });
    render(AssetsPanel);
    await openLibrary();
    expect(await screen.findByText('Skills')).toBeTruthy();
    expect(screen.getByText('MCP servers')).toBeTruthy();
    expect(screen.getByTestId('asset-row-skill-worktree').textContent).toContain('1 in sync');
    expect(screen.getByTestId('asset-row-skill-worktree').textContent).toContain('1 missing');
    expect(screen.getByText('On hosts, not in catalog')).toBeTruthy();
    expect(screen.getByTestId('identity-row-skill-extra')).toBeTruthy();
    expect(screen.getByTestId('assets-problems').textContent).toContain('1');
    expect(screen.getByTestId('assets-head').textContent).toContain('abcdef1');
  });

  it('selecting an asset loads the detail with host matrix and preview switcher', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_get_asset: {
        asset: { kind: 'skill', name: 'worktree', version: '1', description: 'Make one.', tags: ['core'], body: '# b' },
        previews: [
          { harness: 'claude', plan: { files: [{ path: '~/.claude/skills/worktree/SKILL.md', bytes: '---\nname: worktree\n---\n# b' }], merges: [], placeholders: [], warnings: [] }, unsupported: null },
          { harness: 'codex', plan: null, unsupported: 'codex cannot render skill assets' },
        ],
        hosts: [{ host_alias: 'local', harness: 'claude', state: 'in_sync' }],
      },
    });
    render(AssetsPanel);
    await openLibrary();
    expect(await screen.findByTestId('asset-row-skill-worktree')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('asset-row-skill-worktree'));
    expect(await screen.findByTestId('asset-detail-title')).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith('catalog_get_asset', { args: { kind: 'skill', name: 'worktree' } });
    expect(screen.getByTestId('asset-detail-title').textContent).toContain('worktree');
    await fireEvent.click(screen.getByTestId('inspector-tab-hosts'));
    expect(screen.getByTestId('matrix-cell-local-claude').textContent).toContain('in sync');
    expect(screen.getByTestId('matrix-cell-mefistos-claude').textContent).toContain('skipped');
    await fireEvent.click(screen.getByTestId('inspector-tab-source'));
    expect(screen.getByTestId('preview-file-path').textContent).toContain('SKILL.md');
    await fireEvent.click(screen.getByTestId('preview-tab-codex'));
    expect(await screen.findByText(/codex cannot render/)).toBeTruthy();
  });

  it('renders the detail title when catalog_get_asset omits the tags key entirely', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_get_asset: {
        // No `tags` key at all — the regression this guards against.
        asset: { kind: 'skill', name: 'worktree', version: '1', description: 'Make one.', body: '# b' },
        previews: [
          { harness: 'claude', plan: { files: [], merges: [], placeholders: [], warnings: [] }, unsupported: null },
        ],
        hosts: [],
      },
    });
    render(AssetsPanel);
    await openLibrary();
    expect(await screen.findByTestId('asset-row-skill-worktree')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('asset-row-skill-worktree'));
    expect(await screen.findByTestId('asset-detail-title')).toBeTruthy();
    expect(screen.getByTestId('asset-detail-title').textContent).toContain('worktree');
  });

  it('import completion reloads the catalog without pulling and refreshes repo status', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_import_host: { created: [['skill', 'new']], problems: [], flagged_secrets: [], dry_run: true },
      catalog_repo_status: { head: 'h', dirty: 1, ahead: 0, behind: 0, has_upstream: true },
    });
    render(AssetsPanel);
    await openLibrary();
    expect(screen.getByTestId('assets-import').textContent).toBe('Import from host');
    await fireEvent.click(screen.getByTestId('assets-import'));
    await fireEvent.click(screen.getByTestId('import-dry-run'));
    await waitFor(() => expect(screen.getByTestId('import-confirm')).not.toBeDisabled());

    invoke.mockClear();
    await fireEvent.click(screen.getByTestId('import-confirm'));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_load', { args: { pull: false } }));
    expect(invoke).not.toHaveBeenCalledWith('catalog_load', { args: { pull: true } });
    // Import leaves a dirty tree — the status strip (and the "Commit
    // pending" button it gates) must refresh so it shows up without a
    // remount.
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_repo_status', undefined));
  });

  it('Import next to an unmanaged identity presets the dialog to its host and asset', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_import_host: { created: [['skill', 'extra']], problems: [], flagged_secrets: [], dry_run: true },
    });
    render(AssetsPanel);
    await openLibrary();
    const row = await screen.findByTestId('identity-row-skill-extra');

    // The row is a picker since M5; its Import sits beside it, on its line.
    await fireEvent.click(within(row.parentElement!).getByText('Import'));

    expect(await screen.findByTestId('import-dialog')).toBeTruthy();
    expect(screen.getByTestId('import-only').textContent).toContain('skill:extra');
    expect((screen.getByTestId('import-host') as HTMLSelectElement).value).toBe('local');

    await fireEvent.click(screen.getByTestId('import-dry-run'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('catalog_import_host', {
        args: { host_alias: 'local', dry_run: true, only: ['skill:extra'] },
      }),
    );

    // Closing clears the preset: the toolbar's own Import from host reopens
    // it with the plain defaults, not the last identity's.
    await fireEvent.click(screen.getByText('Close'));
    expect(screen.queryByTestId('import-dialog')).toBeNull();
    await fireEvent.click(screen.getByTestId('assets-import'));
    expect(screen.queryByTestId('import-only')).toBeNull();
    expect((screen.getByTestId('import-host') as HTMLSelectElement).value).toBe('local');
  });

  it('scan button calls assets_scan_hosts and refreshes', async () => {
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 }, catalog_list_assets: listing, assets_inventory: [], assets_scan_hosts: [{ host: 'local', status: 'scanned', detail: null, rows: 3 }], catalog_last_sync: null });
    render(AssetsPanel);
    expect(await screen.findByTestId('assets-scan')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('assets-scan'));
    expect(await screen.findByTestId('assets-scan-result')).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith('assets_scan_hosts', { args: { host_alias: null } });
    expect(screen.getByTestId('assets-scan-result').textContent).toContain('local: scanned');
  });

  it('an orphan row on hosts shows an "orphan" badge and no Import button', async () => {
    const withOrphan = {
      ...listing,
      unmanaged: [
        ...listing.unmanaged,
        { host_alias: 'mefistos', harness: 'claude', kind: 'skill', name: 'ghost', state: 'orphan', catalog_hash: null, host_hash: null, scanned_at: 1, managed: true },
      ],
    };
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 }, catalog_list_assets: withOrphan, assets_inventory: [], catalog_last_sync: null });
    render(AssetsPanel);
    await openLibrary();

    const row = await screen.findByTestId('unmanaged-row-mefistos-claude-skill-ghost');
    expect(row.textContent).toContain('orphan');
    expect(screen.getByTestId('orphan-badge-mefistos-claude-skill-ghost')).toBeTruthy();
    expect(within(row).queryByText('Import')).toBeNull();
    // The plain unmanaged row from `listing` still gets its Import button.
    expect(screen.getByTestId('identity-row-skill-extra').parentElement!.textContent).toContain('Import');
  });

  it('the filter matches an orphan row case-insensitively, like it does identity rows', async () => {
    const withOrphan = {
      ...listing,
      unmanaged: [
        ...listing.unmanaged,
        { host_alias: 'mefistos', harness: 'claude', kind: 'skill', name: 'ghost', state: 'orphan', catalog_hash: null, host_hash: null, scanned_at: 1, managed: true },
      ],
    };
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 }, catalog_list_assets: withOrphan, assets_inventory: [], catalog_last_sync: null });
    render(AssetsPanel);
    await openLibrary();
    await screen.findByTestId('unmanaged-row-mefistos-claude-skill-ghost');

    await fireEvent.input(screen.getByTestId('assets-query'), { target: { value: 'GHOST' } });

    expect(screen.getByTestId('unmanaged-row-mefistos-claude-skill-ghost')).toBeTruthy();
    expect(screen.queryByTestId('asset-row-skill-worktree')).toBeNull();
  });

  it('hides fleet internals behind a toggle', async () => {
    const withInternal = {
      ...listing,
      identities: [
        { kind: 'skill', name: 'extra', hosts: [{ host_alias: 'local', harness: 'claude', host_hash: null }], signature: 'local', variants: 0, class: 'normal' as const, reason: null },
        { kind: 'hook', name: 'stop', hosts: [{ host_alias: 'local', harness: 'claude', host_hash: null }], signature: 'local', variants: 0, class: 'fleet_internal' as const, reason: null },
      ],
    };
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 }, catalog_list_assets: withInternal, assets_inventory: [], catalog_last_sync: null });
    render(AssetsPanel);
    await openLibrary();
    expect(await screen.findByTestId('identity-row-skill-extra')).toBeTruthy();
    expect(screen.queryByTestId('identity-row-hook-stop')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: /Show 1 fleet internal/ }));
    expect(screen.getByTestId('identity-row-hook-stop')).toBeTruthy();
  });

  it('Sync button calls catalog_plan_sync and opens the plan view in the workspace', async () => {
    const plan = { id: 'plan-1', computed_at: 1, hosts: [], counts: {} };
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 }, catalog_list_assets: listing, assets_inventory: [], catalog_last_sync: null, catalog_plan_sync: plan });
    render(AssetsPanel);
    expect(await screen.findByTestId('assets-sync')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('assets-sync'));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_plan_sync', { args: { host_alias: null, kind: null, name: null, allow_unlayered: false } }));
    expect(await screen.findByTestId('sync-plan-view')).toBeTruthy();
    // The plan is a view inside the workspace, not a modal; the list is hidden while it is open.
    const view = screen.getByTestId('sync-plan-view');
    expect(screen.getByTestId('assets-workspace').contains(view)).toBe(true);
    expect(view.closest('dialog,[role="dialog"]')).toBeNull();
    expect(screen.queryByTestId('assets-inbox')).toBeNull();
    // Back closes it and the list returns.
    await fireEvent.click(screen.getByTestId('plan-back'));
    expect(screen.queryByTestId('sync-plan-view')).toBeNull();
    expect(screen.getByTestId('assets-inbox')).toBeTruthy();
  });

  it('Secrets button opens the secrets panel', async () => {
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 }, catalog_list_assets: listing, assets_inventory: [], catalog_last_sync: null, catalog_list_secrets: [] });
    render(AssetsPanel);
    expect(await screen.findByTestId('assets-secrets')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('assets-secrets'));

    expect(await screen.findByTestId('secrets-panel')).toBeTruthy();
  });

  it('applying a plan keeps the plan view open, showing outcomes/restart and disabling re-apply', async () => {
    const plan = {
      id: 'plan-1',
      computed_at: 1,
      hosts: [
        {
          host_alias: 'local', harness: 'claude', status: 'ready', detail: null,
          actions: [{ kind: 'skill', name: 'worktree', op: 'update', reason: null, files: [], merges: [], backup: false, secrets: [], missing_secrets: [] }],
        },
      ],
      counts: { update: 1 },
    };
    const summary = {
      plan_id: 'plan-1', started_at: 1, finished_at: 2,
      hosts: [
        {
          host_alias: 'local', harness: 'claude', status: 'applied', detail: null, restart_required: true,
          actions: [{ kind: 'skill', name: 'worktree', op: 'update', outcome: 'done', detail: null }],
        },
      ],
    };
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [], catalog_last_sync: null,
      catalog_plan_sync: plan, catalog_apply_sync: summary,
    });
    render(AssetsPanel);
    expect(await screen.findByTestId('assets-sync')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('assets-sync'));
    expect(await screen.findByTestId('sync-plan-view')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('plan-apply'));

    expect(await screen.findByTestId('plan-restart-local')).toBeTruthy();
    expect(screen.getByTestId('plan-outcome-local-claude-skill-worktree').textContent).toContain('done');
    // The view stays open (this is the whole point) and re-applying the
    // now-consumed plan id is blocked.
    expect(screen.getByTestId('sync-plan-view')).toBeTruthy();
    expect(screen.getByTestId('plan-apply')).toBeDisabled();
  });

  it('shows a last-sync strip from lastSync() on mount', async () => {
    const summary = { plan_id: 'plan-1', started_at: 1, finished_at: 2, hosts: [{ host_alias: 'local', harness: 'claude', status: 'applied', detail: null, restart_required: false, actions: [] }] };
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 }, catalog_list_assets: listing, assets_inventory: [], catalog_last_sync: summary });
    render(AssetsPanel);
    expect(await screen.findByTestId('assets-last-sync')).toBeTruthy();
    expect(screen.getByTestId('assets-last-sync').textContent).toContain('applied');
  });
});

describe('AssetsPanel authoring', () => {
  it('New asset creates via the dialog, then selects and opens the editor', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [], catalog_repo_status: { head: 'h', dirty: 0, ahead: null, behind: null, has_upstream: false },
      catalog_create_asset: { commit: 'sha-new', lint: { errors: [], warnings: [] } },
      catalog_lint_asset: { errors: [], warnings: [] },
      catalog_get_asset: {
        asset: { kind: 'skill', name: 'my-new-skill', version: '1', description: 'Describe when to use this skill.', tags: [], body: '# my-new-skill\n', allowed_tools: [], user_invocable: true, triggers: [] },
        previews: [], hosts: [],
      },
    });
    render(AssetsPanel, { visible: true });
    await openLibrary();
    expect(await screen.findByTestId('assets-new')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('assets-new'));
    expect(await screen.findByTestId('new-asset-dialog')).toBeTruthy();
    await fireEvent.input(screen.getByTestId('new-asset-name'), { target: { value: 'my-new-skill' } });
    await fireEvent.click(screen.getByTestId('new-asset-create'));

    // Opens in Source, where AssetDetail's title row (an Overview part) is
    // not shown: the Inspector's own title names it.
    await waitFor(() => expect(screen.getByTestId('inspector').textContent).toContain('my-new-skill'));
    // A freshly created asset opens straight into edit mode.
    expect(await screen.findByTestId('editor-save')).toBeTruthy();
  });

  it('a created asset opens in the editor once; selecting it again later opens its Overview', async () => {
    const withNew = {
      ...listing,
      assets: [...listing.assets, { kind: 'skill', name: 'my-new-skill', version: '1', description: 'd', tags: [], hosts: [] }],
    };
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 3, problem_count: 0 },
      catalog_list_assets: withNew, assets_inventory: [], catalog_repo_status: { head: 'h', dirty: 0, ahead: null, behind: null, has_upstream: false },
      catalog_create_asset: { commit: 'sha-new', lint: { errors: [], warnings: [] } },
      catalog_lint_asset: { errors: [], warnings: [] },
      catalog_get_asset: {
        asset: { kind: 'skill', name: 'my-new-skill', version: '1', description: 'd', tags: [], body: '# b\n', allowed_tools: [], user_invocable: true, triggers: [] },
        previews: [], hosts: [],
      },
    });
    render(AssetsPanel, { visible: true });
    await openLibrary();
    await fireEvent.click(await screen.findByTestId('assets-new'));
    await fireEvent.input(await screen.findByTestId('new-asset-name'), { target: { value: 'my-new-skill' } });
    await fireEvent.click(screen.getByTestId('new-asset-create'));
    expect(await screen.findByTestId('editor-save')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('asset-row-skill-worktree'));
    await fireEvent.click(screen.getByTestId('asset-row-skill-my-new-skill'));
    await screen.findByTestId('asset-detail-title');
    expect(screen.getByTestId('inspector-tab-overview').getAttribute('aria-selected')).toBe('true');
    expect(screen.queryByTestId('editor-save')).toBeNull();
  });

  it('Pull on the personal chip pulls and reloads the catalog', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_repo_status: { head: 'h', dirty: 0, ahead: 0, behind: 1, has_upstream: true },
    });
    render(AssetsPanel, { visible: true });
    await waitFor(() => expect(screen.getByTestId('assets-head').textContent).toContain('personal @h'));
    await openPersonalChip();
    await fireEvent.click(await screen.findByTestId('assets-pull'));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_load', { args: { pull: true } }));
  });

  it('a pull failure renders the git stderr, keeping the listing', async () => {
    let pulls = 0;
    invoke.mockImplementation(async (cmd: string, a?: { args?: { pull?: boolean } }) => {
      switch (cmd) {
        case 'catalog_config': return { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 };
        case 'catalog_load':
          if (a?.args?.pull) {
            pulls += 1;
            throw { code: 'E_CATALOG_GIT', message: 'git pull: failed', details: { stderr: 'fatal: Not possible to fast-forward, aborting.' } };
          }
          return { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 };
        case 'catalog_list_assets': return listing;
        case 'assets_inventory': return [];
        case 'catalog_repo_status': return { head: 'h', dirty: 0, ahead: 0, behind: 1, has_upstream: true };
        default: throw { code: 'E_TEST', message: `unexpected ${cmd}` };
      }
    });
    render(AssetsPanel, { visible: true });
    await waitFor(() => expect(screen.getByTestId('assets-head').textContent).toContain('personal @h'));
    await openPersonalChip();
    await fireEvent.click(await screen.findByTestId('assets-pull'));
    const err = await screen.findByText(/git pull: failed/);
    expect(err.textContent).toContain('Not possible to fast-forward');
    expect(pulls).toBe(1);
    expect(screen.getByTestId('assets-inbox')).toBeTruthy();
  });

  it('the Inbox’s cards are re-read whenever the panel refreshes (R14)', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [], catalog_last_sync: null,
      catalog_list_changesets: [{ id: 7, kind: 'new', summary: 'Import 1 new skill', state: 'proposed', created_at: 1 }],
      assets_scan_hosts: [{ host: 'local', status: 'scanned', detail: null, rows: 3 }],
    });
    render(AssetsPanel, { visible: true });
    expect(await screen.findByTestId('card-7')).toBeTruthy();
    const reads = () => invoke.mock.calls.filter((c) => c[0] === 'catalog_list_changesets').length;
    const before = reads();
    await fireEvent.click(screen.getByTestId('assets-scan'));
    await waitFor(() => expect(reads()).toBe(before + 1));
  });

  it('Delete asks for confirmation, then clears the selection on success', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [], catalog_repo_status: { head: 'h', dirty: 0, ahead: null, behind: null, has_upstream: false },
      catalog_get_asset: {
        asset: { kind: 'skill', name: 'worktree', version: '1', description: 'Make one.', tags: [], body: '# b' },
        previews: [], hosts: [],
      },
      catalog_delete_asset: 'sha-del',
    });
    render(AssetsPanel, { visible: true });
    await openLibrary();
    await fireEvent.click(await screen.findByTestId('asset-row-skill-worktree'));
    await screen.findByTestId('asset-detail-title');

    await fireEvent.click(screen.getByTestId('asset-delete'));
    expect(await screen.findByTestId('confirm-dialog')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('asset-delete-confirm'));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_delete_asset', { args: { kind: 'skill', name: 'worktree' } }));
    await waitFor(() => expect(screen.queryByTestId('asset-detail-title')).toBeNull());
    expect(screen.getByText('Select an asset.')).toBeTruthy();
  });

  it('Lint shows the inline report for the selected asset', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [], catalog_repo_status: { head: 'h', dirty: 0, ahead: null, behind: null, has_upstream: false },
      catalog_get_asset: {
        asset: { kind: 'skill', name: 'worktree', version: '1', description: 'Make one.', tags: [], body: '# b' },
        previews: [], hosts: [],
      },
      catalog_lint_asset: { errors: [{ field: 'description', message: 'must not be empty' }], warnings: [] },
    });
    render(AssetsPanel, { visible: true });
    await openLibrary();
    await fireEvent.click(await screen.findByTestId('asset-row-skill-worktree'));
    await screen.findByTestId('asset-detail-title');

    await fireEvent.click(screen.getByTestId('asset-lint'));
    expect(await screen.findByTestId('asset-lint-report')).toBeTruthy();
    expect(screen.getByTestId('asset-lint-report').textContent).toContain('must not be empty');
  });

  it('shows the repo status strip and gates Commit pending / Push on dirty/ahead/has_upstream', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_repo_status: { head: 'abcdef1234567890', dirty: 3, ahead: 2, behind: 0, has_upstream: true },
    });
    render(AssetsPanel, { visible: true });
    await waitFor(() => expect(screen.getByTestId('assets-head').textContent).toContain('abcdef1'));
    await openPersonalChip();

    expect(await screen.findByTestId('assets-repo-status')).toBeTruthy();
    expect(screen.getByTestId('assets-repo-status').textContent).toContain('3 dirty');
    expect(await screen.findByTestId('assets-commit-pending')).toBeTruthy();
    expect(screen.getByTestId('assets-push').textContent).toContain('↑2');
    expect(screen.getByTestId('assets-push')).not.toBeDisabled();
  });

  it('Push is disabled without an upstream', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_repo_status: { head: 'h', dirty: 0, ahead: null, behind: null, has_upstream: false },
    });
    render(AssetsPanel, { visible: true });
    await screen.findByTestId('assets-inbox');
    await openPersonalChip();
    expect(await screen.findByTestId('assets-push')).toBeDisabled();
  });

  it('Commit lists the changes, starts the message from them and calls catalog_commit_pending (G2.6)', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_repo_status: {
        head: 'h', dirty: 2, ahead: 0, behind: 0, has_upstream: false,
        changes: [{ path: 'skills/release-notes', status: 'M' }, { path: 'commands/ship', status: 'A' }],
      },
      catalog_commit_pending: 'sha-commit',
    });
    render(AssetsPanel, { visible: true });
    await waitFor(() => expect(screen.getByTestId('assets-head').textContent).toContain('±2'));
    await openPersonalChip();
    await fireEvent.click(await screen.findByTestId('assets-commit-pending'));

    const dialog = await screen.findByTestId('commit-assets-dialog');
    expect(dialog.textContent).toContain('Commit 2 asset changes');
    expect(screen.getAllByTestId('commit-assets-change').map((li) => li.textContent?.replace(/\s+/g, ' ').trim())).toEqual([
      'skills/release-notes M',
      'commands/ship A',
    ]);
    const msg = screen.getByTestId('commit-assets-message') as HTMLTextAreaElement;
    await waitFor(() => expect(msg.value).toBe('catalog: update skills/release-notes, commands/ship'));
    // No remote: nothing to push to.
    expect(screen.queryByTestId('commit-assets-push')).toBeNull();
    await fireEvent.click(screen.getByTestId('commit-assets-commit'));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('catalog_commit_pending', { args: { message: 'catalog: update skills/release-notes, commands/ship' } }),
    );
    expect(invoke.mock.calls.some((c: unknown[]) => c[0] === 'catalog_push')).toBe(false);
  });

  it('Commit and push commits, then pushes', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: 'git@github.com:o/r.git', head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_repo_status: { head: 'h', dirty: 1, ahead: 0, behind: 0, has_upstream: true, changes: [{ path: 'hooks/stop', status: 'D' }] },
      catalog_commit_pending: 'sha-commit',
      catalog_push: { head: 'h2', dirty: 0, ahead: 0, behind: 0, has_upstream: true },
    });
    render(AssetsPanel, { visible: true });
    await waitFor(() => expect(screen.getByTestId('assets-head').textContent).toContain('±1'));
    await openPersonalChip();
    await fireEvent.click(await screen.findByTestId('assets-commit-pending'));
    const msg = (await screen.findByTestId('commit-assets-message')) as HTMLTextAreaElement;
    await waitFor(() => expect(msg.value).toBe('catalog: remove hooks/stop'));
    await fireEvent.input(msg, { target: { value: 'catalog: drop the stop hook' } });
    await fireEvent.click(screen.getByTestId('commit-assets-push'));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_push', undefined));
    expect(invoke).toHaveBeenCalledWith('catalog_commit_pending', { args: { message: 'catalog: drop the stop hook' } });
  });

  it('Push calls catalog_push and refreshes', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_repo_status: { head: 'h', dirty: 0, ahead: 3, behind: 0, has_upstream: true },
      catalog_push: { head: 'h2', dirty: 0, ahead: 0, behind: 0, has_upstream: true },
    });
    render(AssetsPanel, { visible: true });
    await waitFor(() => expect(screen.getByTestId('assets-head').textContent).toContain('↑3'));
    await openPersonalChip();
    await fireEvent.click(await screen.findByTestId('assets-push'));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_push', undefined));
    // Acting closes the popover; open it again to read the fresh status.
    await waitFor(() => expect(screen.getByTestId('assets-head').textContent).not.toContain('↑3'));
    await openPersonalChip();
    await waitFor(() => expect(screen.getByTestId('assets-repo-status').textContent).not.toContain('↑3'));
  });

  it('a push failure renders the git stderr from error.details alongside the message', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_repo_status: { head: 'h', dirty: 0, ahead: 3, behind: 0, has_upstream: true },
      catalog_push: {
        code: 'E_CATALOG_GIT',
        message: 'git push: failed',
        details: { stderr: 'fatal: could not read Username for \'https://github.com\': terminal prompts disabled' },
      },
    });
    render(AssetsPanel, { visible: true });
    await waitFor(() => expect(screen.getByTestId('assets-head').textContent).toContain('↑3'));
    await openPersonalChip();
    await fireEvent.click(await screen.findByTestId('assets-push'));

    const err = await screen.findByText(/git push: failed/);
    expect(err.textContent).toContain('terminal prompts disabled');
  });

  it('Lint all opens the dialog and selecting a finding selects the asset', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_lint_all: {
        errors: 1, warnings: 0, problems: [],
        assets: [{ kind: 'skill', name: 'worktree', report: { errors: [{ field: 'body', message: 'empty' }], warnings: [] } }],
      },
      catalog_get_asset: {
        asset: { kind: 'skill', name: 'worktree', version: '1', description: 'Make one.', tags: [], body: '# b' },
        previews: [], hosts: [],
      },
    });
    render(AssetsPanel, { visible: true });
    await openLibrary();
    await fireEvent.click(await screen.findByTestId('assets-lint-all'));
    await fireEvent.click(await screen.findByTestId('lint-all-select-skill-worktree'));

    expect(await screen.findByTestId('asset-detail-title')).toBeTruthy();
    expect(screen.getByTestId('asset-detail-title').textContent).toContain('worktree');
  });

  it('reloads the catalog when the panel regains visibility after an author session was opened', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [], catalog_repo_status: { head: 'h', dirty: 0, ahead: null, behind: null, has_upstream: false },
      catalog_get_asset: {
        asset: { kind: 'skill', name: 'worktree', version: '1', description: 'Make one.', tags: [], body: '# b' },
        previews: [], hosts: [],
      },
      catalog_spawn_author_session: { id: 1, tmux_name: 'catalog-skill-worktree' },
    });
    const { rerender } = render(AssetsPanel, { visible: true });
    await openLibrary();
    await fireEvent.click(await screen.findByTestId('asset-row-skill-worktree'));
    await screen.findByTestId('asset-detail-title');

    // Delegating to a session (through the real UI, not a test-only setter —
    // the module flag is exported read-only) is what sets the flag.
    await fireEvent.click(screen.getByTestId('asset-open-session'));
    await fireEvent.click(await screen.findByTestId('author-open'));
    await waitFor(() => expect(authorSessionOpened).toBe(true));

    invoke.mockClear();
    await rerender({ visible: false });
    await rerender({ visible: true });

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_load', { args: { pull: false } }));
    expect(authorSessionOpened).toBe(false);
  });

  it('does not reload on a visibility flip when no author session was opened', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [], catalog_repo_status: { head: 'h', dirty: 0, ahead: null, behind: null, has_upstream: false },
    });
    const { rerender } = render(AssetsPanel, { visible: true });
    await screen.findByTestId('assets-sync');
    invoke.mockClear();

    await rerender({ visible: false });
    await rerender({ visible: true });

    expect(invoke).not.toHaveBeenCalledWith('catalog_load', expect.anything());
  });
});

// ── Requests from the QuickSwitcher (Assets M6, R19) ──────────────────────
describe('AssetsPanel switcher requests', () => {
  const config = { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 };
  const base = {
    catalog_config: config,
    catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
    catalog_list_assets: listing, assets_inventory: [], catalog_last_sync: null,
    catalog_repo_status: { head: 'h', dirty: 0, ahead: null, behind: null, has_upstream: false },
    catalog_list_changesets: [],
    catalog_get_asset: {
      asset: { kind: 'skill', name: 'worktree', version: '1', description: 'Make one.', tags: [], body: '# b' },
      previews: [], hosts: [],
    },
  };
  const calls = (cmd: string) => invoke.mock.calls.filter((c) => c[0] === cmd);
  const WORKTREE = 'asset:personal:skill/worktree';

  beforeEach(() => {
    hubStatus.set(STANDALONE);
    assetsViewRequest.set(null);
  });
  afterEach(() => {
    hubStatus.set(STANDALONE);
    assetsViewRequest.set(null);
  });

  it('a select request made before the panel mounts selects the row, in the Library', async () => {
    byCmd(base);
    requestAssetsView({ select: WORKTREE });
    render(AssetsPanel, { visible: true });
    expect((await screen.findByTestId('asset-detail-title')).textContent).toContain('worktree');
    // The row is in the Library, not the Inbox it started on.
    expect(screen.getByTestId('asset-row-skill-worktree')).toBeTruthy();
    await waitFor(() => expect(get(assetsViewRequest)).toBeNull());
  });

  it('a newKind request opens New asset on that kind (M15 G7.13)', async () => {
    byCmd(base);
    requestAssetsView({ newKind: 'hook' });
    render(AssetsPanel, { visible: true });
    const dialog = await screen.findByTestId('new-asset-dialog');
    expect((dialog.querySelector('select') as HTMLSelectElement).value).toBe('hook');
    await waitFor(() => expect(get(assetsViewRequest)).toBeNull());
  });

  it('a select request while the panel is open moves from another view to the row', async () => {
    byCmd(base);
    render(AssetsPanel, { visible: true });
    await openLayers();
    expect(screen.queryByTestId('asset-row-skill-worktree')).toBeNull();
    requestAssetsView({ select: WORKTREE });
    expect((await screen.findByTestId('asset-detail-title')).textContent).toContain('worktree');
    expect(screen.getByTestId('asset-row-skill-worktree')).toBeTruthy();
    expect(get(assetsViewRequest)).toBeNull();
  });

  it('the workspace keeps its keys after a select request: j moves the focus on from the selected row', async () => {
    byCmd(base);
    requestAssetsView({ select: WORKTREE });
    render(AssetsPanel, { visible: true });
    await screen.findByTestId('asset-detail-title');
    const row = screen.getByTestId('asset-row-skill-worktree');
    expect(row.getAttribute('aria-current')).toBe('true');
    // The list has the keyboard once the Library is shown, as for any open.
    await waitFor(() => expect(document.activeElement && screen.getByTestId('assets-workspace').contains(document.activeElement)).toBe(true));
    await fireEvent.keyDown(document.activeElement as HTMLElement, { key: 'j' });
    expect((document.activeElement as HTMLElement).getAttribute('data-row-key')).toBe('asset:personal:mcp_server/fleet');
    // Focus moved; the selection stays until Enter.
    expect(screen.getByTestId('asset-row-skill-worktree').getAttribute('aria-current')).toBe('true');
  });

  it('a rescan request runs the scan once and is cleared', async () => {
    byCmd({ ...base, assets_scan_hosts: [{ host: 'local', status: 'scanned', detail: null, rows: 3 }] });
    requestAssetsView({ command: 'rescan' });
    render(AssetsPanel, { visible: true });
    expect(await screen.findByTestId('assets-scan-result')).toBeTruthy();
    expect(calls('assets_scan_hosts')).toHaveLength(1);
    expect(get(assetsViewRequest)).toBeNull();
    await tick();
    expect(calls('assets_scan_hosts')).toHaveLength(1);
  });

  it('a sync request plans the whole fleet and opens the plan view', async () => {
    byCmd({ ...base, catalog_plan_sync: { id: 'plan-1', computed_at: 1, hosts: [], counts: {} } });
    requestAssetsView({ command: 'sync' });
    render(AssetsPanel, { visible: true });
    expect(await screen.findByTestId('plan-back')).toBeTruthy();
    expect(calls('catalog_plan_sync')).toHaveLength(1);
    expect(calls('catalog_plan_sync')[0][1]).toEqual({ args: { host_alias: null, kind: null, name: null, allow_unlayered: false } });
    expect(get(assetsViewRequest)).toBeNull();
  });

  it('a propose request proposes, reloads the cards and shows the Inbox', async () => {
    byCmd({ ...base, catalog_propose_changesets: [] });
    render(AssetsPanel, { visible: true });
    await openLibrary();
    expect(screen.queryByTestId('assets-inbox')).toBeNull();
    invoke.mockClear();
    requestAssetsView({ command: 'propose' });
    expect(await screen.findByTestId('assets-inbox')).toBeTruthy();
    expect(calls('catalog_propose_changesets')).toHaveLength(1);
    expect(calls('catalog_list_changesets').length).toBeGreaterThan(0);
    expect(get(assetsViewRequest)).toBeNull();
  });

  it('a refused propose names the reason and keeps the view', async () => {
    byCmd({ ...base, catalog_propose_changesets: { code: 'E_FORBIDDEN', message: 'no grant' } });
    render(AssetsPanel, { visible: true });
    await openLibrary();
    requestAssetsView({ command: 'propose' });
    await waitFor(() => expect(calls('catalog_propose_changesets')).toHaveLength(1));
    await waitFor(() => expect(get(toasts).some((t) => t.message.includes('no grant'))).toBe(true));
    expect(screen.queryByTestId('assets-inbox')).toBeNull();
  });

  it('a command does nothing while the panel is busy', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_plan_sync') return new Promise(() => {});
      if (cmd in base) return (base as Record<string, unknown>)[cmd];
      throw { code: 'E_TEST', message: `unexpected ${cmd}` };
    });
    render(AssetsPanel, { visible: true });
    await screen.findByTestId('assets-sync');
    requestAssetsView({ command: 'sync' });
    await waitFor(() => expect(calls('catalog_plan_sync')).toHaveLength(1));
    // The plan is still computing: a rescan must not start on top of it.
    requestAssetsView({ command: 'rescan' });
    await waitFor(() => expect(get(assetsViewRequest)).toBeNull());
    expect(calls('assets_scan_hosts')).toHaveLength(0);
  });

  it('a command does nothing while a card verb is running (a card apply in flight)', async () => {
    const card = { id: 7, kind: 'new', summary: 'New on oci: skill/fresh', state: 'proposed', created_at: 1, catalogs: ['personal'] };
    const cardView = { ...card, commits: {}, undoable: false, items: [
      { position: 0, grp: 'core', catalog: 'personal', kind: 'skill', name: 'fresh', action: 'import', params: {}, decider: 'rule', state: 'pending' },
    ] };
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_apply_changeset') return new Promise(() => {});
      if (cmd === 'catalog_list_changesets') return [card];
      if (cmd === 'catalog_get_changeset') return cardView;
      if (cmd in base) return (base as Record<string, unknown>)[cmd];
      throw { code: 'E_TEST', message: `unexpected ${cmd}` };
    });
    render(AssetsPanel, { visible: true });
    await fireEvent.click(await screen.findByTestId('card-primary-7'));
    await waitFor(() => expect(calls('catalog_apply_changeset')).toHaveLength(1));
    requestAssetsView({ command: 'sync' });
    await waitFor(() => expect(get(assetsViewRequest)).toBeNull());
    requestAssetsView({ command: 'propose' });
    await waitFor(() => expect(get(assetsViewRequest)).toBeNull());
    requestAssetsView({ command: 'rescan' });
    await waitFor(() => expect(get(assetsViewRequest)).toBeNull());
    expect(calls('catalog_plan_sync')).toHaveLength(0);
    expect(calls('catalog_propose_changesets')).toHaveLength(0);
    expect(calls('assets_scan_hosts')).toHaveLength(0);
  });

  it('two Propose requests in a row make one propose call while the first is in flight', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_propose_changesets') return new Promise(() => {});
      if (cmd in base) return (base as Record<string, unknown>)[cmd];
      throw { code: 'E_TEST', message: `unexpected ${cmd}` };
    });
    render(AssetsPanel, { visible: true });
    await screen.findByTestId('assets-sync');
    requestAssetsView({ command: 'propose' });
    await waitFor(() => expect(calls('catalog_propose_changesets')).toHaveLength(1));
    requestAssetsView({ command: 'propose' });
    await waitFor(() => expect(get(assetsViewRequest)).toBeNull());
    expect(calls('catalog_propose_changesets')).toHaveLength(1);
  });

  it('Propose marks the panel busy and frees it again', async () => {
    let release: (v: unknown) => void = () => {};
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_propose_changesets') return new Promise((r) => { release = r; });
      if (cmd in base) return (base as Record<string, unknown>)[cmd];
      throw { code: 'E_TEST', message: `unexpected ${cmd}` };
    });
    render(AssetsPanel, { visible: true });
    await screen.findByTestId('assets-sync');
    requestAssetsView({ command: 'propose' });
    await waitFor(() => expect(screen.getByTestId('assets-sync')).toBeDisabled());
    release([]);
    await waitFor(() => expect(screen.getByTestId('assets-sync')).not.toBeDisabled());
    // A second Propose runs now.
    requestAssetsView({ command: 'propose' });
    await waitFor(() => expect(calls('catalog_propose_changesets')).toHaveLength(2));
  });

  it('a hub client without the grant: rescan, sync and propose do nothing and are cleared', async () => {
    hubStatus.set({ ...STANDALONE, remote: true, client_name: 'desk', client_mode: 'full' });
    byCmd({
      catalog_config: { code: 'E_FORBIDDEN', message: 'not granted' },
      catalog_list_assets: listing, assets_inventory: [], catalog_list_changesets: [], catalog_list_catalogs: [],
    });
    render(AssetsPanel, { visible: true });
    for (const command of ['rescan', 'sync', 'propose'] as const) {
      requestAssetsView({ command });
      await waitFor(() => expect(get(assetsViewRequest)).toBeNull());
    }
    // The request waited for the grant probe, then was dropped.
    expect(calls('catalog_config')).toHaveLength(1);
    expect(await screen.findByTestId('assets-workspace')).toBeTruthy();
    expect(calls('assets_scan_hosts')).toHaveLength(0);
    expect(calls('catalog_plan_sync')).toHaveLength(0);
    expect(calls('catalog_propose_changesets')).toHaveLength(0);
  });

  it('a hub client without the grant can still select a row', async () => {
    hubStatus.set({ ...STANDALONE, remote: true, client_name: 'desk', client_mode: 'full' });
    byCmd({
      catalog_config: { code: 'E_FORBIDDEN', message: 'not granted' },
      catalog_list_assets: listing, assets_inventory: [], catalog_list_changesets: [], catalog_list_catalogs: [],
    });
    requestAssetsView({ select: WORKTREE });
    render(AssetsPanel, { visible: true });
    expect(await screen.findByTestId('asset-row-skill-worktree')).toBeTruthy();
    await waitFor(() => expect(get(assetsViewRequest)).toBeNull());
  });

  it('a request still waiting when the panel goes away is dropped, not run at the next open', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_config') return new Promise(() => {});
      throw { code: 'E_TEST', message: cmd };
    });
    requestAssetsView({ command: 'rescan' });
    const { unmount } = render(AssetsPanel, { visible: true });
    await tick(); await tick();
    expect(get(assetsViewRequest)).not.toBeNull();
    unmount();
    expect(get(assetsViewRequest)).toBeNull();
    expect(calls('assets_scan_hosts')).toHaveLength(0);
  });
});
