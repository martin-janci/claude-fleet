// The overview of every link: why one is stale, and Clean up stale
// disconnects only those.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import LocalWorkspacesOverview from './LocalWorkspacesOverview.svelte';
import { localWorkspaces, _resetLocalWorkspacesForTests, type LocalWorkspace } from './local_workspaces';
import { sessions } from './sessions';
import { session } from './hosts_fixture';

const ws = (over: Partial<LocalWorkspace>): LocalWorkspace => ({
  id: 1,
  host_alias: 'devbox',
  owner: 'acme',
  repo: 'app',
  project_id: 4,
  worktree_key: 'main',
  remote_path: '/r',
  local_path: '/l',
  paused: false,
  excludes: [],
  state: 'synced',
  pending_local: 0,
  pending_remote: 0,
  skipped: 0,
  conflicts: [],
  created_at: 1,
  ...over,
});

beforeEach(() => {
  invoke.mockReset();
  _resetLocalWorkspacesForTests();
});

describe('LocalWorkspacesOverview', () => {
  it('marks the stale link and cleans up only that one', async () => {
    localWorkspaces.set([ws({ id: 1 }), ws({ id: 2, worktree_key: 'old', local_path: '/old' })]);
    sessions.set([session('devbox', 'dev-app', { project_id: 4, worktree_key: null })]);
    invoke.mockResolvedValue(null);
    render(LocalWorkspacesOverview, { onclose: () => {} });
    expect(screen.getAllByTestId('lw-overview-row')).toHaveLength(2);
    expect(screen.getAllByTestId('lw-stale')).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('lw-cleanup'));
    await fireEvent.click(screen.getByTestId('lw-cleanup-confirm'));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('disconnect_local_workspace', { args: { id: 2 } }),
    );
    expect(invoke).not.toHaveBeenCalledWith('disconnect_local_workspace', { args: { id: 1 } });
  });
});
