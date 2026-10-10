// Gap plan G4.6: Projects on the hub, a pick of this desktop only.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import HubProjectsPick from './HubProjectsPick.svelte';
import { hubStatus, STANDALONE } from './hub';
import { projects, type ProjectTreeRow } from './projects';
import { filterHubProjects, hubProjectPicks, hubProjectsLabel } from './hub_projects';

const URL = 'https://hub.example:7443';

function row(id: number, repo: string, system = false): ProjectTreeRow {
  return {
    project: { id, owner: 'acme', repo, base_path: `/p/${repo}`, last_session_at: null, adopted: false, system },
    worktrees: [],
  };
}

beforeEach(() => {
  hubProjectPicks.set({});
  hubStatus.set({ ...STANDALONE, remote: true, url: URL });
  projects.set([row(1, 'api'), row(2, 'web'), row(3, 'docs'), row(9, 'operator', true)]);
});

describe('Projects on the hub', () => {
  it('lists every project until some are left out, then only the picked ones', async () => {
    render(HubProjectsPick);
    expect(screen.getByTestId('hub-projects-toggle').textContent).toContain('All 3');
    await fireEvent.click(screen.getByTestId('hub-projects-toggle'));
    await fireEvent.click(screen.getByTestId('hub-projects-3'));
    expect(get(hubProjectPicks)).toEqual({ [URL]: [1, 2] });
    expect(screen.getByTestId('hub-projects-toggle').textContent).toContain('2 of 3');
    expect(get(projects).map((p) => p.project.id)).toEqual([1, 2]);
    // Back to all: the pick is forgotten.
    await fireEvent.click(screen.getByTestId('hub-projects-3'));
    expect(get(hubProjectPicks)).toEqual({});
    expect(get(projects)).toHaveLength(4);
  });

  it('keeps the last project, and filters nothing when not paired', async () => {
    hubProjectPicks.set({ [URL]: [2] });
    render(HubProjectsPick);
    await fireEvent.click(screen.getByTestId('hub-projects-toggle'));
    expect((screen.getByTestId('hub-projects-2') as HTMLInputElement).disabled).toBe(true);
    hubStatus.set({ ...STANDALONE });
    expect(get(projects)).toHaveLength(4);
  });

  it('shows every project when the pick names none of them', () => {
    const rows = [row(1, 'api'), row(2, 'web')];
    expect(filterHubProjects(rows, new Set([7]))).toEqual(rows);
    expect(filterHubProjects(rows, new Set([2]))).toEqual([rows[1]]);
    expect(filterHubProjects(rows, null)).toEqual(rows);
    expect(hubProjectsLabel(6, 9)).toBe('6 of 9');
  });
});
