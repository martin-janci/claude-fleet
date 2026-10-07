import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import {
  linkFor,
  badgeFor,
  suggestedFolder,
  localWorkspaces,
  loadLocalWorkspaces,
  enableLocalWorkspace,
  disconnectLocalWorkspace,
  _resetLocalWorkspacesForTests,
  type LocalWorkspace,
} from './local_workspaces';

function ws(over: Partial<LocalWorkspace> = {}): LocalWorkspace {
  return {
    id: 1,
    host_alias: 'devbox',
    owner: 'acme',
    repo: 'app',
    project_id: 7,
    worktree_key: 'main',
    remote_path: '/home/u/projects/github.com/acme/app',
    local_path: '/Users/me/fleet/app',
    session_id: 3,
    paused: false,
    excludes: [],
    state: 'synced',
    last_sync_at: 100,
    last_error: null,
    pending_local: 0,
    pending_remote: 0,
    skipped: 0,
    conflicts: [],
    created_at: 1,
    ...over,
  };
}

beforeEach(() => {
  invoke.mockReset();
  _resetLocalWorkspacesForTests();
});

describe('linkFor', () => {
  it('matches a session on the same host, project and worktree', () => {
    const rows = [ws(), ws({ id: 2, worktree_key: 'feat-x' })];
    expect(linkFor(rows, { host_alias: 'devbox', project_id: 7, worktree_key: null })?.id).toBe(1);
    expect(linkFor(rows, { host_alias: 'devbox', project_id: 7, worktree_key: 'feat-x' })?.id).toBe(2);
    expect(linkFor(rows, { host_alias: 'other', project_id: 7, worktree_key: null })).toBeUndefined();
    expect(linkFor(rows, { host_alias: 'devbox', project_id: null, worktree_key: null })).toBeUndefined();
  });
});

describe('badgeFor', () => {
  it('names each state', () => {
    expect(badgeFor(undefined).tone).toBe('off');
    expect(badgeFor(ws()).tone).toBe('ok');
    expect(badgeFor(ws({ paused: true })).label).toBe('Paused');
    expect(badgeFor(ws({ state: 'local_changes', pending_local: 7 })).label).toBe('7 local changes');
    const c = badgeFor(
      ws({ state: 'conflict', conflicts: [{ path: 'a', kind: 'both_modified', detected_at: 1 }] }),
    );
    expect(c).toEqual({ tone: 'conflict', label: '1 conflict' });
    expect(badgeFor(ws({ state: 'offline' })).tone).toBe('idle');
    expect(badgeFor(ws({ state: 'error' })).tone).toBe('error');
  });
});

describe('commands', () => {
  it('loads, enables and disconnects through the backend', async () => {
    invoke.mockResolvedValueOnce([ws()]);
    await loadLocalWorkspaces();
    expect(invoke).toHaveBeenCalledWith('list_local_workspaces', undefined);
    expect(get(localWorkspaces)).toHaveLength(1);

    invoke.mockResolvedValueOnce(ws({ id: 2, worktree_key: 'feat' }));
    await enableLocalWorkspace(3, '/Users/me/fleet/app-feat');
    expect(invoke).toHaveBeenLastCalledWith('enable_local_workspace', {
      args: { session_id: 3, local_path: '/Users/me/fleet/app-feat', excludes: [] },
    });
    expect(get(localWorkspaces).map((w) => w.id)).toEqual([1, 2]);

    invoke.mockResolvedValueOnce(null);
    expect(await disconnectLocalWorkspace(1)).toBe(true);
    expect(get(localWorkspaces).map((w) => w.id)).toEqual([2]);
  });
});

it('suggests a folder per repo and worktree', () => {
  expect(suggestedFolder('app', null)).toBe('~/fleet/app');
  expect(suggestedFolder('app', 'main')).toBe('~/fleet/app');
  expect(suggestedFolder('app', 'feat-x')).toBe('~/fleet/app-feat-x');
});
