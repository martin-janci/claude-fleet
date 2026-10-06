import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { loadProjectPicks, pickKey, previousPick, projectPicks, setProjectPick } from './project_picks';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const row = (over = {}) => ({ owner: 'o', repo: 'r', pinned: false, vis: null, grp: null, ...over });

beforeEach(() => {
  inv.mockReset();
  projectPicks.set(new Map());
});

describe('project_picks', () => {
  it('loads into a map keyed owner/repo', async () => {
    inv.mockResolvedValue([row({ pinned: true })]);
    await loadProjectPicks();
    expect(inv).toHaveBeenCalledWith('project_picks', undefined);
    expect(get(projectPicks).get(pickKey('o', 'r'))?.pinned).toBe(true);
  });

  it('an older hub (an error or null) leaves the map as it was', async () => {
    inv.mockResolvedValue(null);
    await loadProjectPicks();
    expect(get(projectPicks).size).toBe(0);
    inv.mockRejectedValue({ code: 'E_HUB', message: 'unknown tool' });
    expect((await loadProjectPicks()).ok).toBe(false);
    expect(get(projectPicks).size).toBe(0);
  });

  it('set is a full replace, optimistic, and remembers the previous state', async () => {
    projectPicks.set(new Map([[pickKey('o', 'r'), row({ grp: 'tools' })]]));
    let resolve!: (v: unknown) => void;
    inv.mockReturnValue(new Promise((r) => (resolve = r)));
    const p = setProjectPick('o', 'r', { pinned: true });
    expect(get(projectPicks).get('o/r')?.pinned).toBe(true); // before the answer
    expect(inv).toHaveBeenCalledWith('set_project_pick', {
      args: { owner: 'o', repo: 'r', pinned: true, vis: null, grp: 'tools' },
    });
    resolve(row({ pinned: true, grp: 'tools' }));
    await p;
    expect(previousPick('o', 'r')?.pinned).toBe(false);
  });

  it('a failed set rolls back', async () => {
    projectPicks.set(new Map([[pickKey('o', 'r'), row()]]));
    inv.mockRejectedValue({ code: 'E_HUB', message: 'down' });
    const r = await setProjectPick('o', 'r', { vis: 'hide' });
    expect(r.ok).toBe(false);
    expect(get(projectPicks).get('o/r')?.vis).toBe(null);
  });
});
