import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetsPanel from './AssetsPanel.svelte';
import { catalog, catalogConfig, inventory } from './assets';
import { hosts } from './hosts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

function byCmd(map: Record<string, unknown>) {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd in map) return map[cmd];
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
  catalog.set(null); catalogConfig.set(null); inventory.set([]);
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: false, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true },
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

  it('import completion reloads the catalog without pulling', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_import_host: { created: [['skill', 'new']], problems: [], flagged_secrets: [], dry_run: true },
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
  });

  it('scan button calls assets_scan_hosts and refreshes', async () => {
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 }, catalog_list_assets: listing, assets_inventory: [], assets_scan_hosts: [{ host: 'local', status: 'scanned', detail: null, rows: 3 }] });
    render(AssetsPanel);
    expect(await screen.findByTestId('assets-scan')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('assets-scan'));
    expect(await screen.findByTestId('assets-scan-result')).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith('assets_scan_hosts', { args: { host_alias: null } });
    expect(screen.getByTestId('assets-scan-result').textContent).toContain('local: scanned');
  });
});
