import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  projects,
  refreshProjects,
  addProject,
  listGithubRepos,
  confirmTokenOf,
  type AddProjectSource,
} from './projects';
import { get } from 'svelte/store';

const fake = [
  {
    project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null, adopted: false, system: false },
    worktrees: [
      { id: 11, project_id: 1, name: 'main', path: '/r/cf', branch: 'main' },
    ],
  },
  {
    project: { id: 2, owner: 'papayapos', repo: 'pos-frontend', base_path: '/r/pf', last_session_at: 1716120000, adopted: false, system: false },
    worktrees: [
      { id: 21, project_id: 2, name: 'main', path: '/r/pf', branch: 'main' },
      { id: 22, project_id: 2, name: 'feature-x', path: '/r/pf/.worktrees/feature-x', branch: 'feature-x' },
    ],
  },
];

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

const newRow = {
  project: { id: 3, owner: 'martin-janci', repo: 'new-repo', base_path: '/r/nr', last_session_at: null, adopted: false, system: false },
  worktrees: [{ id: 31, project_id: 3, name: 'main', path: '/r/nr', branch: 'main' }],
};

describe('addProject', () => {
  const sources: Array<{ label: string; source: AddProjectSource }> = [
    { label: 'clone', source: { kind: 'clone', url: 'martin-janci/claude-fleet' } },
    { label: 'folder', source: { kind: 'folder', path: '/Users/m/repo' } },
    {
      label: 'new',
      source: { kind: 'new', owner: 'martin-janci', repo: 'new-repo', create_remote: true },
    },
  ];

  for (const { label, source } of sources) {
    it(`sends the exact payload for kind "${label}" and merges the row on success`, async () => {
      (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(newRow);
      const r = await addProject('local', source);
      expect(r.ok).toBe(true);

      expect(mockedInvoke).toHaveBeenCalledWith('add_project', {
        args: { host_alias: 'local', source, call_id: expect.any(Number) },
      });

      const stored = get(projects).find((p) => p.project.id === 3);
      expect(stored).toEqual(newRow);
    });
  }

  it('does not touch the store when the call fails', async () => {
    const before = get(projects);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce({
      code: 'E_EXISTS',
      message: 'already exists',
    });
    const r = await addProject('local', { kind: 'clone', url: 'owner/repo' });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe('E_EXISTS');
    expect(get(projects)).toBe(before);
  });
});

describe('listGithubRepos', () => {
  it('sends the exact payload', async () => {
    const repos = [
      {
        name_with_owner: 'martin-janci/claude-fleet',
        description: 'fleet',
        is_private: false,
        updated_at: '2026-01-01T00:00:00Z',
      },
    ];
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(repos);
    const r = await listGithubRepos('mefistos');
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value).toEqual(repos);
    expect(mockedInvoke).toHaveBeenCalledWith('list_github_repos', {
      args: { host_alias: 'mefistos' },
    });
  });
});

describe('confirmTokenOf', () => {
  const validToken = 'a'.repeat(64);

  it('returns the token for a valid E_CONFIRM_REQUIRED error', () => {
    const err = {
      code: 'E_CONFIRM_REQUIRED',
      message: 'needs confirmation',
      details: { confirm: validToken },
    };
    expect(confirmTokenOf(err)).toBe(validToken);
  });

  it('returns null for a different error code', () => {
    const err = {
      code: 'E_INVALID',
      message: 'nope',
      details: { confirm: validToken },
    };
    expect(confirmTokenOf(err)).toBeNull();
  });

  it('returns null when details is missing', () => {
    const err = { code: 'E_CONFIRM_REQUIRED', message: 'needs confirmation' };
    expect(confirmTokenOf(err)).toBeNull();
  });

  it('returns null for a malformed token', () => {
    const err = {
      code: 'E_CONFIRM_REQUIRED',
      message: 'needs confirmation',
      details: { confirm: 'not-hex-and-too-short' },
    };
    expect(confirmTokenOf(err)).toBeNull();
  });
});
