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
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import Sidebar from './Sidebar.svelte';
import { projects } from './projects';
import { sessions } from './sessions';
import { selectedProject, selectedSession, selectProject, selectSession } from './selection';

const defaultInvoke = async (cmd: string) => {
  if (cmd === 'list_projects') return fake;
  if (cmd === 'list_sessions') return [];
  return null;
};

beforeEach(() => {
  projects.set([]);
  sessions.set([]);
  selectProject(null);
  selectSession(null);
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(defaultInvoke);
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

  it('renders orphan sessions (project_id null) in a dedicated section', async () => {
    const orphan = {
      id: 1,
      tmux_name: 'dev-stray',
      host_alias: 'local',
      project_id: null,
      worktree_id: null,
      created_at: 1,
      last_activity_at: 1,
      status: 'running',
      notes: null,
    };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fake;
      if (cmd === 'list_sessions') return [orphan];
      return null;
    });
    render(Sidebar);
    await tick(); await tick();
    const section = await screen.findByTestId('orphan-sessions');
    expect(section).toHaveTextContent('Other sessions (1)');
    expect(section).toHaveTextContent('dev-stray');
  });

  it('shows orphan section alongside matched projects, not instead of', async () => {
    const orphan = {
      id: 1,
      tmux_name: 'dev-stray',
      host_alias: 'local',
      project_id: null,
      worktree_id: null,
      created_at: 1,
      last_activity_at: 1,
      status: 'running',
      notes: null,
    };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fake;
      if (cmd === 'list_sessions') return [orphan];
      return null;
    });
    render(Sidebar);
    await tick(); await tick();
    const rows = await screen.findAllByTestId('proj-row');
    expect(rows.length).toBeGreaterThan(0);
    const section = await screen.findByTestId('orphan-sessions');
    expect(section).toBeInTheDocument();
  });

  it('hides the owner when repo name is unique (just shows repo)', async () => {
    render(Sidebar);
    await tick(); await tick();
    const rows = await screen.findAllByTestId('proj-row');
    // No repo collides in `fake`, so all rows show repo without owner prefix.
    for (const row of rows) {
      expect(row).not.toHaveTextContent('martin-janci/');
      expect(row).not.toHaveTextContent('papayapos/');
    }
    // Repo names still visible.
    expect(rows.some((r) => r.textContent?.includes('claude-fleet'))).toBe(true);
    expect(rows.some((r) => r.textContent?.includes('phone-manager'))).toBe(true);
    expect(rows.some((r) => r.textContent?.includes('pos-frontend'))).toBe(true);
  });

  it('shows the owner prefix when two repos share a name', async () => {
    const colliding = [
      ...fake,
      {
        project: { id: 4, owner: 'otherperson', repo: 'claude-fleet', base_path: '/x/cf', last_session_at: null },
        worktrees: [{ id: 41, project_id: 4, name: 'main', path: '/x/cf', branch: 'main' }],
      },
    ];
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return colliding;
      if (cmd === 'list_sessions') return [];
      return null;
    });
    render(Sidebar);
    await tick(); await tick();
    const rows = await screen.findAllByTestId('proj-row');
    // Both claude-fleet rows now display the owner prefix.
    const claudeFleetRows = rows.filter((r) => r.textContent?.includes('claude-fleet'));
    expect(claudeFleetRows).toHaveLength(2);
    expect(claudeFleetRows.some((r) => r.textContent?.includes('martin-janci/'))).toBe(true);
    expect(claudeFleetRows.some((r) => r.textContent?.includes('otherperson/'))).toBe(true);
    // A non-colliding repo (phone-manager) does NOT get the owner prefix.
    const phoneRow = rows.find((r) => r.textContent?.includes('phone-manager'));
    expect(phoneRow?.textContent).not.toContain('martin-janci/phone-manager');
  });

  it('clicking a project row selects it; clicking again deselects', async () => {
    render(Sidebar);
    await tick(); await tick();
    const rows = await screen.findAllByTestId('proj-row');
    expect(get(selectedProject)).toBeNull();
    await fireEvent.click(rows[0]);
    expect(get(selectedProject)?.project.id).toBe(1);
    expect(rows[0].className).toContain('selected');
    await fireEvent.click(rows[0]);
    expect(get(selectedProject)).toBeNull();
  });

  it('hides the worktrees list when the project has only main', async () => {
    render(Sidebar);
    await tick(); await tick();
    await screen.findAllByTestId('proj-row');
    // All fixture projects have only `main`, so no wt-row should render.
    const wtRows = screen.queryAllByTestId('wt-row');
    expect(wtRows).toHaveLength(0);
  });

  it('shows the worktrees list when there are non-main worktrees', async () => {
    const withWorktrees = [
      {
        project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null },
        worktrees: [
          { id: 11, project_id: 1, name: 'main', path: '/r/cf', branch: 'main' },
          { id: 12, project_id: 1, name: 'feature-x', path: '/r/cf/.worktrees/feature-x', branch: 'feature-x' },
        ],
      },
    ];
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return withWorktrees;
      if (cmd === 'list_sessions') return [];
      return null;
    });
    render(Sidebar);
    await tick(); await tick();
    const wtRows = await screen.findAllByTestId('wt-row');
    expect(wtRows.length).toBe(2);
  });

  it('clicking the + button selects nothing (does not also select the row)', async () => {
    render(Sidebar);
    await tick(); await tick();
    await screen.findAllByTestId('proj-row');
    const addButtons = screen.getAllByLabelText('New session');
    expect(get(selectedProject)).toBeNull();
    await fireEvent.click(addButtons[0]);
    // The dialog opens but the project should NOT be selected via the row click.
    expect(get(selectedProject)).toBeNull();
  });

  it('clicking a session row selects the session in the store', async () => {
    const sess = {
      id: 99,
      tmux_name: 'dev-foo',
      host_alias: 'local',
      project_id: 1,
      worktree_id: null,
      created_at: 1,
      last_activity_at: 1,
      status: 'running',
      notes: null,
    };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fake;
      if (cmd === 'list_sessions') return [sess];
      return null;
    });
    render(Sidebar);
    await tick(); await tick();
    const sessRows = await screen.findAllByTestId('sess-row');
    expect(get(selectedSession)).toBeNull();
    await fireEvent.click(sessRows[0]);
    expect(get(selectedSession)?.id).toBe(99);
    expect(sessRows[0].className).toContain('selected');
  });

  it('clicking the same session again deselects it', async () => {
    const sess = {
      id: 99,
      tmux_name: 'dev-foo',
      host_alias: 'local',
      project_id: 1,
      worktree_id: null,
      created_at: 1,
      last_activity_at: 1,
      status: 'running',
      notes: null,
    };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fake;
      if (cmd === 'list_sessions') return [sess];
      return null;
    });
    render(Sidebar);
    await tick(); await tick();
    const sessRows = await screen.findAllByTestId('sess-row');
    await fireEvent.click(sessRows[0]);
    await fireEvent.click(sessRows[0]);
    expect(get(selectedSession)).toBeNull();
  });

  it('selecting a session clears any project selection', async () => {
    const sess = {
      id: 99,
      tmux_name: 'dev-foo',
      host_alias: 'local',
      project_id: 1,
      worktree_id: null,
      created_at: 1,
      last_activity_at: 1,
      status: 'running',
      notes: null,
    };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fake;
      if (cmd === 'list_sessions') return [sess];
      return null;
    });
    render(Sidebar);
    await tick(); await tick();
    const projRows = await screen.findAllByTestId('proj-row');
    await fireEvent.click(projRows[0]);
    expect(get(selectedProject)?.project.id).toBe(1);

    const sessRows = await screen.findAllByTestId('sess-row');
    await fireEvent.click(sessRows[0]);
    expect(get(selectedSession)?.id).toBe(99);
    expect(get(selectedProject)).toBeNull();
  });

  it('clicking × on a session does not also select the session', async () => {
    const sess = {
      id: 99,
      tmux_name: 'dev-foo',
      host_alias: 'local',
      project_id: 1,
      worktree_id: null,
      created_at: 1,
      last_activity_at: 1,
      status: 'running',
      notes: null,
    };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fake;
      if (cmd === 'list_sessions') return [sess];
      if (cmd === 'kill_session') return null;
      return null;
    });
    // Stub confirm() to true so kill proceeds without UI.
    const origConfirm = window.confirm;
    window.confirm = () => true;
    render(Sidebar);
    await tick(); await tick();
    const sessRows = await screen.findAllByTestId('sess-row');
    const killBtn = sessRows[0].querySelector('.kill') as HTMLButtonElement;
    expect(get(selectedSession)).toBeNull();
    await fireEvent.click(killBtn);
    // Kill triggers but session was NOT selected via the row.
    expect(get(selectedSession)).toBeNull();
    window.confirm = origConfirm;
  });
});
