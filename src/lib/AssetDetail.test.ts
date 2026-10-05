import { render, screen, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetDetail from './AssetDetail.svelte';

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

beforeEach(() => {
  invoke.mockReset();
});

describe('AssetDetail install_as', () => {
  it('shows "installs as <name>" when install_as is set', async () => {
    byCmd({
      catalog_get_asset: {
        asset: {
          kind: 'skill', name: 'foo-bar', version: '1', description: 'd', tags: [],
          body: '# b', install_as: 'foo_bar',
        },
        previews: [], hosts: [],
      },
    });
    render(AssetDetail, { kind: 'skill', name: 'foo-bar', hosts: [] });

    await waitFor(() => expect(screen.getByTestId('asset-detail-title')).toBeTruthy());
    expect(await screen.findByTestId('asset-install-as')).toBeTruthy();
    expect(screen.getByTestId('asset-install-as').textContent).toContain('installs as foo_bar');
  });

  it('does not render the install_as line when unset', async () => {
    byCmd({
      catalog_get_asset: {
        asset: { kind: 'skill', name: 'worktree', version: '1', description: 'd', tags: [], body: '# b' },
        previews: [], hosts: [],
      },
    });
    render(AssetDetail, { kind: 'skill', name: 'worktree', hosts: [] });

    await waitFor(() => expect(screen.getByTestId('asset-detail-title')).toBeTruthy());
    expect(screen.queryByTestId('asset-install-as')).toBeNull();
  });
});

describe('AssetDetail sections (Assets M5)', () => {
  const detail = {
    asset: { kind: 'skill', name: 'w', version: '1', description: 'Make one.', tags: [], body: '# b' },
    previews: [{ harness: 'claude', plan: { files: [{ path: '~/.claude/skills/w/SKILL.md', bytes: 'x' }], merges: [], placeholders: [], warnings: [] }, unsupported: null }],
    hosts: [{ host_alias: 'local', harness: 'claude', state: 'in_sync' }],
  };

  it('shows only the section it is asked for', async () => {
    byCmd({ catalog_get_asset: detail });
    const { rerender } = render(AssetDetail, { kind: 'skill', name: 'w', hosts: [], section: 'overview' });
    expect(await screen.findByTestId('asset-detail-title')).toBeTruthy();
    expect(screen.queryByTestId('preview-file-path')).toBeNull();
    await rerender({ kind: 'skill', name: 'w', hosts: [], section: 'source' });
    expect(screen.queryByTestId('asset-detail-title')).toBeNull();
    expect(screen.getByTestId('preview-file-path').textContent).toContain('SKILL.md');
    expect(invoke.mock.calls.filter((c) => c[0] === 'catalog_get_asset')).toHaveLength(1);
  });

  it('Edit asks for the Source section', async () => {
    byCmd({ catalog_get_asset: detail });
    const onsection = vi.fn();
    render(AssetDetail, { kind: 'skill', name: 'w', hosts: [], section: 'overview', onsection });
    (await screen.findByTestId('asset-edit')).click();
    expect(onsection).toHaveBeenCalledWith('source');
  });

  it('hosts shows the matrix and nothing else', async () => {
    byCmd({ catalog_get_asset: detail });
    render(AssetDetail, { kind: 'skill', name: 'w', hosts: [{ alias: 'local', reachable: true } as never], section: 'hosts' });
    expect(await screen.findByTestId('matrix-cell-local-claude')).toBeTruthy();
    expect(screen.queryByTestId('asset-detail-title')).toBeNull();
    expect(screen.queryByTestId('preview-file-path')).toBeNull();
  });

  it('a drifted cell says which side moved (final review minor 2)', async () => {
    byCmd({
      catalog_get_asset: {
        ...detail,
        hosts: [
          { host_alias: 'local', harness: 'claude', state: 'drifted', drift_side: 'host' },
          { host_alias: 'oci', harness: 'claude', state: 'drifted', drift_side: 'catalog' },
          { host_alias: 'gpu', harness: 'claude', state: 'drifted' },
        ],
      },
    });
    const up = (alias: string) => ({ alias, reachable: true }) as never;
    render(AssetDetail, { kind: 'skill', name: 'w', hosts: [up('local'), up('oci'), up('gpu')], section: 'hosts' });
    const local = await screen.findByTestId('matrix-cell-local-claude');
    expect(local.textContent).toContain('drifted — edited on host');
    expect(local.className).toContain('state-drifted');
    expect(screen.getByTestId('matrix-cell-oci-claude').textContent).toContain('drifted — behind the catalog');
    expect(screen.getByTestId('matrix-cell-gpu-claude').textContent).not.toContain('—');
  });
});
