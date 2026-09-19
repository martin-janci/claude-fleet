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
