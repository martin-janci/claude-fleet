import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { projects, refreshProjects, mergeWorktree } from './projects';
import { get } from 'svelte/store';

const fake = [
  {
    project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null },
    worktrees: [
      { id: 11, project_id: 1, name: 'main', path: '/r/cf', branch: 'main', missing_since: null },
    ],
  },
  {
    project: { id: 2, owner: 'papayapos', repo: 'pos-frontend', base_path: '/r/pf', last_session_at: 1716120000 },
    worktrees: [
      { id: 21, project_id: 2, name: 'main', path: '/r/pf', branch: 'main', missing_since: null },
      { id: 22, project_id: 2, name: 'feature-x', path: '/r/pf/.worktrees/feature-x', branch: 'feature-x', missing_since: null },
    ],
  },
];

it('mergeWorktree carries missing_since onto the worktree', () => {
  projects.set([
    { project: { id: 1, owner: 'o', repo: 'r', base_path: '/r', last_session_at: null },
      worktrees: [{ id: 9, project_id: 1, name: 'feat', path: '/r/.worktrees/feat', branch: 'feat', missing_since: null }] },
  ]);
  mergeWorktree({ id: 9, project_id: 1, name: 'feat', path: '/r/.worktrees/feat', branch: 'feat', missing_since: 1234 });
  const wt = get(projects)[0].worktrees.find((w) => w.id === 9)!;
  expect(wt.missing_since).toBe(1234);
});

describe('projects store', () => {
  it('refreshProjects populates the store on Ok', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(fake);
    const r = await refreshProjects();
    expect(r.ok).toBe(true);
    expect(get(projects)).toHaveLength(2);
    expect(get(projects)[1].worktrees).toHaveLength(2);
  });

  it('refreshProjects sets the error and does not touch the store on Err', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(fake);
    await refreshProjects();
    const before = get(projects).length;

    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce({
      code: 'E_IO',
      message: 'permission denied',
    });
    const r = await refreshProjects();
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe('E_IO');
    expect(get(projects)).toHaveLength(before);
  });
});
