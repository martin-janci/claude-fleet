import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetsPanel from './AssetsPanel.svelte';
import { catalog, catalogConfig, inventory, lastSyncRun, repoStatusStore } from './assets';
import { hosts } from './hosts';
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

  it('lists assets grouped by kind with state chips, unmanaged group and problems badge', async () => {
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'abcdef1234567890', last_loaded_at: 1 }, catalog_load: { head: 'abcdef1234567890', loaded_at: 1, asset_count: 2, problem_count: 1 }, catalog_list_assets: listing, assets_inventory: [] });
    render(AssetsPanel);
    expect(await screen.findByText('Skills')).toBeTruthy();
    expect(screen.getByText('MCP servers')).toBeTruthy();
    expect(screen.getByTestId('asset-row-skill-worktree').textContent).toContain('1 in sync');
    expect(screen.getByTestId('asset-row-skill-worktree').textContent).toContain('1 missing');
    expect(screen.getByText('On hosts, not in catalog')).toBeTruthy();
    expect(screen.getByTestId('unmanaged-row-local-claude-skill-extra')).toBeTruthy();
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
    expect(await screen.findByTestId('asset-row-skill-worktree')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('asset-row-skill-worktree'));
    expect(await screen.findByTestId('asset-detail-title')).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith('catalog_get_asset', { args: { kind: 'skill', name: 'worktree' } });
    expect(screen.getByTestId('asset-detail-title').textContent).toContain('worktree');
    expect(screen.getByTestId('matrix-cell-local-claude').textContent).toContain('in sync');
    expect(screen.getByTestId('matrix-cell-mefistos-claude').textContent).toContain('skipped');
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
    expect(await screen.findByText('Import from host')).toBeTruthy();
    await fireEvent.click(screen.getByText('Import from host'));
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

    const row = await screen.findByTestId('unmanaged-row-mefistos-claude-skill-ghost');
    expect(row.textContent).toContain('orphan');
    expect(screen.getByTestId('orphan-badge-mefistos-claude-skill-ghost')).toBeTruthy();
    expect(within(row).queryByText('Import')).toBeNull();
    // The plain unmanaged row from `listing` still gets its Import button.
    expect(screen.getByTestId('unmanaged-row-local-claude-skill-extra').textContent).toContain('Import');
  });

  it('Sync button calls catalog_plan_sync and opens the plan dialog', async () => {
    const plan = { id: 'plan-1', computed_at: 1, hosts: [], counts: {} };
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 }, catalog_list_assets: listing, assets_inventory: [], catalog_last_sync: null, catalog_plan_sync: plan });
    render(AssetsPanel);
    expect(await screen.findByTestId('assets-sync')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('assets-sync'));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_plan_sync', { args: { host_alias: null, kind: null, name: null } }));
    expect(await screen.findByTestId('sync-plan-dialog')).toBeTruthy();
  });

  it('Secrets button opens the secrets panel', async () => {
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 }, catalog_list_assets: listing, assets_inventory: [], catalog_last_sync: null, catalog_list_secrets: [] });
    render(AssetsPanel);
    expect(await screen.findByTestId('assets-secrets')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('assets-secrets'));

    expect(await screen.findByTestId('secrets-panel')).toBeTruthy();
  });

  it('applying a plan keeps the dialog mounted, showing outcomes/restart and disabling re-apply', async () => {
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
    expect(await screen.findByTestId('sync-plan-dialog')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('plan-apply'));

    expect(await screen.findByTestId('plan-restart-local')).toBeTruthy();
    expect(screen.getByTestId('plan-outcome-local-claude-skill-worktree').textContent).toContain('done');
    // The dialog stays mounted (this is the whole point) and re-applying the
    // now-consumed plan id is blocked.
    expect(screen.getByTestId('sync-plan-dialog')).toBeTruthy();
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
    expect(await screen.findByTestId('assets-new')).toBeTruthy();

    await fireEvent.click(screen.getByTestId('assets-new'));
    expect(await screen.findByTestId('new-asset-dialog')).toBeTruthy();
    await fireEvent.input(screen.getByTestId('new-asset-name'), { target: { value: 'my-new-skill' } });
    await fireEvent.click(screen.getByTestId('new-asset-create'));

    await waitFor(() => expect(screen.getByTestId('asset-detail-title').textContent).toContain('my-new-skill'));
    // A freshly created asset opens straight into edit mode.
    expect(await screen.findByTestId('editor-save')).toBeTruthy();
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
    expect(await screen.findByTestId('assets-push')).toBeDisabled();
  });

  it('Commit pending prompts for a message (defaulted) and calls catalog_commit_pending', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_repo_status: { head: 'h', dirty: 2, ahead: 0, behind: 0, has_upstream: true },
      catalog_commit_pending: 'sha-commit',
    });
    render(AssetsPanel, { visible: true });
    await fireEvent.click(await screen.findByTestId('assets-commit-pending'));

    expect(await screen.findByTestId('prompt-dialog')).toBeTruthy();
    expect((screen.getByTestId('prompt-input') as HTMLInputElement).value).toBe('catalog: commit pending changes');
    await fireEvent.click(screen.getByTestId('prompt-submit'));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_commit_pending', { args: { message: 'catalog: commit pending changes' } }));
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
    await fireEvent.click(await screen.findByTestId('assets-push'));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('catalog_push', undefined));
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
    await screen.findByTestId('assets-new');
    invoke.mockClear();

    await rerender({ visible: false });
    await rerender({ visible: true });

    expect(invoke).not.toHaveBeenCalledWith('catalog_load', expect.anything());
  });
});
