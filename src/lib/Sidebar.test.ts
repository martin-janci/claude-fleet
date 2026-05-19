import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

const fake = [
  {
    project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: Math.floor(Date.now() / 1000) - 60 },
    worktrees: [{ id: 11, project_id: 1, name: 'main', path: '/r/cf', branch: 'main' }],
  },
  {
    project: { id: 2, owner: 'papayapos', repo: 'pos-frontend', base_path: '/r/pf', last_session_at: Math.floor(Date.now() / 1000) - 60 * 60 * 24 * 14 },
    worktrees: [{ id: 21, project_id: 2, name: 'main', path: '/r/pf', branch: 'main' }],
  },
  {
    project: { id: 3, owner: 'martin-janci', repo: 'phone-manager', base_path: '/r/pm', last_session_at: null },
    worktrees: [{ id: 31, project_id: 3, name: 'main', path: '/r/pm', branch: 'main' }],
  },
];

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => (cmd === 'list_projects' ? fake : [])),
}));

import Sidebar from './Sidebar.svelte';
import { projects } from './projects';

beforeEach(() => {
  projects.set([]);
});

describe('Sidebar', () => {
  it('renders all projects by default', async () => {
    render(Sidebar);
    await tick(); await tick();
    const rows = await screen.findAllByTestId('proj-row');
    expect(rows).toHaveLength(3);
  });

  it('filters by 7d recency', async () => {
    render(Sidebar);
    await tick(); await tick();
    await screen.findAllByTestId('proj-row');
    await fireEvent.click(screen.getByText('7d'));
    const rows = await screen.findAllByTestId('proj-row');
    expect(rows).toHaveLength(1); // only claude-fleet (1 minute old) matches "7d"
  });

  it('filters by search query', async () => {
    render(Sidebar);
    await tick(); await tick();
    await screen.findAllByTestId('proj-row');
    const search = screen.getByTestId('sidebar-search') as HTMLInputElement;
    await fireEvent.input(search, { target: { value: 'phone' } });
    const rows = await screen.findAllByTestId('proj-row');
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent('phone-manager');
  });
});
