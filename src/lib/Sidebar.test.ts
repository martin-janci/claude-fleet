import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

// Three sample projects. Sessions are attached per-test so we can verify
// the new "hide projects without sessions" behavior.
const fakeProjects = [
  {
    project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: Math.floor(Date.now() / 1000) - 60 },
    worktrees: [{ id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/r/cf', branch: 'main' }],
  },
  {
    project: { id: 2, owner: 'papayapos', repo: 'pos-frontend', base_path: '/r/pf', last_session_at: Math.floor(Date.now() / 1000) - 60 * 60 * 24 * 14 },
    worktrees: [{ id: 21, project_id: 2, host_alias: 'local', name: 'main', path: '/r/pf', branch: 'main' }],
  },
  {
    project: { id: 3, owner: 'martin-janci', repo: 'phone-manager', base_path: '/r/pm', last_session_at: null },
    worktrees: [{ id: 31, project_id: 3, host_alias: 'local', name: 'main', path: '/r/pm', branch: 'main' }],
  },
];

let nextSessionId = 1000;
function sessionFor(projectId: number | null, name = `dev-${projectId ?? 'orphan'}`): SessionRow {
  return {
    id: nextSessionId++,
    tmux_name: name,
    host_alias: 'local',
    project_id: projectId,
    worktree_id: null,
    created_at: 1,
    last_activity_at: 1,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: 'main',
    lost_at: null,
    claude_session_id: null,
    claude_status: null,
    effort_level: null,
    pr_url: null,
    current_activity: null,
    friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [],
  };
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

// Wrap the memoised index builders in call-through spies so the scale test
// below can assert they run once per render, not once per row.
vi.mock('./sidebar_index', { spy: true });

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { buildSessionsByProject, buildRelatedCountById } from './sidebar_index';
import { get } from 'svelte/store';
import Sidebar from './Sidebar.svelte';
import { projects, bootstrapProjects } from './projects';
import { sessions, bootstrapSessions, showBgAgents, resetTombstonesForTests, type SessionRow } from './sessions';
import { selectedSession, selectSession } from './selection';
import { hosts, bootstrapHosts, hostFilter, resetTombstonesForTests as resetHostTombstones } from './hosts';
import { accounts, bootstrapAccounts } from './accounts';
import { onboardingDismissed } from './onboarding';
import { toasts, clearToasts } from './toasts';

function mockBackend(projs: typeof fakeProjects, sess: ReturnType<typeof sessionFor>[]) {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: { args?: { id?: number; new_name?: string; alias?: string } }) => {
    if (cmd === 'list_projects') return projs;
    if (cmd === 'list_sessions') return sess;
    // Existing tests don't care about hosts — return empty so $hosts is a
    // valid array (never null) when Sidebar.svelte does `$hosts.filter(...)`.
    if (cmd === 'list_hosts') return [];
    if (cmd === 'list_accounts') return [];
    // Iter 4a Task 13: mutation IPCs now return the affected row (or id for
    // kill). The wrapper then patches the store via mergeSession/removeSession;
    // mergeSession(null) would throw. Return a sentinel that satisfies the
    // patch even though these tests only assert that the IPC was invoked.
    const id = args?.args?.id ?? 0;
    if (cmd === 'kill_session') return id;
    if (cmd === 'set_session_friendly_name') {
      const a = (args?.args ?? {}) as { tmux_name?: string; friendly_name?: string };
      const found = sess.find((s) => s.tmux_name === a.tmux_name) ?? sess[0];
      const label = a.friendly_name?.trim() ? a.friendly_name.trim() : null;
      return found ? { ...found, friendly_name: label } : null;
    }
    if (cmd === 'new_session' || cmd === 'rename_session' || cmd === 'restart_session') {
      const found = sess.find((s) => s.id === id) ?? sess[0];
      return found ?? null;
    }
    if (cmd === 'add_host' || cmd === 'probe_ssh_alias' || cmd === 'remove_host' || cmd === 'hide_host') {
      const alias = args?.args?.alias ?? 'local';
      return { alias, ssh_alias: null, hidden: false, account_uuid: null, reachable: true, claude_version: null, tmux_version: null, probed_at: null };
    }
    return null;
  });
  // Sidebar no longer bootstraps the stores itself (App.svelte owns that),
  // so seed them directly — the mounted component is a pure consumer.
  projects.set(projs);
  sessions.set(sess);
}

beforeEach(() => {
  resetTombstonesForTests();
  resetHostTombstones();
  projects.set([]);
  sessions.set([]);
  hosts.set([]);
  accounts.set([]);
  hostFilter.set('all');
  showBgAgents.set(true);
  selectSession(null);
  // Suppress the OnboardingCard so tests don't need stubs for its IPC calls
  // (check_local_prereqs, tunnel_status, mcp_status).
  onboardingDismissed.set(true);
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  // Wipe persisted prefs so one test's recency choice doesn't leak into
  // the next test's mount-time hydration.
  localStorage.clear();
});

describe('Sidebar (sessions-grouped view)', () => {
  it('hides projects that have no active sessions', async () => {
    // No sessions at all — main tree should show nothing.
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    const rows = screen.queryAllByTestId('proj-row');
    expect(rows).toHaveLength(0);
  });

  it('shows a project once it has at least one session', async () => {
    mockBackend(fakeProjects, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    const rows = await screen.findAllByTestId('proj-row');
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent('claude-fleet');
  });

  it('groups multiple sessions under their project', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a'), sessionFor(1, 'dev-b'), sessionFor(2, 'dev-c')]);
    render(Sidebar);
    await tick(); await tick();
    const projRows = await screen.findAllByTestId('proj-row');
    expect(projRows).toHaveLength(2);
    const sessRows = await screen.findAllByTestId('sess-row');
    expect(sessRows).toHaveLength(3);
  });

  it('does not render any worktree rows', async () => {
    // Even if a project has multiple worktrees, the sidebar must not show them.
    const multi = [
      {
        project: { id: 1, owner: 'o', repo: 'r', base_path: '/x', last_session_at: 0 },
        worktrees: [
          { id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/x', branch: 'main' },
          { id: 12, project_id: 1, host_alias: 'local', name: 'feature-x', path: '/x/.worktrees/feature-x', branch: 'feature-x' },
          { id: 13, project_id: 1, host_alias: 'local', name: 'bugfix', path: '/x/.worktrees/bugfix', branch: 'bugfix' },
        ],
      },
    ];
    mockBackend(multi, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    await screen.findAllByTestId('proj-row');
    expect(screen.queryAllByTestId('wt-row')).toHaveLength(0);
  });

  it('shows a session count badge per project', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a'), sessionFor(1, 'dev-b')]);
    render(Sidebar);
    await tick(); await tick();
    const row = await screen.findByTestId('proj-row');
    // Count "2" should appear in the project row.
    expect(row.textContent).toContain('2');
  });

  it('renders orphan sessions in a separate section', async () => {
    mockBackend(fakeProjects, [sessionFor(null, 'dev-stray')]);
    render(Sidebar);
    await tick(); await tick();
    const section = await screen.findByTestId('orphan-sessions');
    expect(section).toHaveTextContent('Other sessions (1)');
    expect(section).toHaveTextContent('dev-stray');
  });

  it('clicking project row toggles collapse (sessions show/hide)', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a')]);
    render(Sidebar);
    await tick(); await tick();
    const projRow = await screen.findByTestId('proj-row');
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(1);
    await fireEvent.click(projRow);
    await tick();
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(0);
    await fireEvent.click(projRow);
    await tick();
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(1);
  });

  it('selecting a session in a collapsed project expands the project and scrolls the row into view', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-reveal')]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(await screen.findByTestId('proj-row'));
    await tick();
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(0);

    const scrolled: string[] = [];
    const orig = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push((this as HTMLElement).dataset.sessionId ?? '');
    };
    try {
      // Select from outside the sidebar, as the quick switcher does.
      const row = get(sessions).find((x) => x.tmux_name === 'dev-reveal')!;
      selectSession(row);
      await tick(); await tick(); await Promise.resolve();
      const rows = screen.queryAllByTestId('sess-row');
      expect(rows).toHaveLength(1);
      expect(rows[0].getAttribute('data-session-id')).toBe(String(row.id));
      expect(scrolled).toContain(String(row.id));
    } finally {
      Element.prototype.scrollIntoView = orig;
    }
  });

  it('clicking a session row selects it in the store', async () => {
    const sess = sessionFor(1, 'dev-foo');
    mockBackend(fakeProjects, [sess]);
    render(Sidebar);
    await tick(); await tick();
    const sessRows = await screen.findAllByTestId('sess-row');
    expect(get(selectedSession)).toBeNull();
    await fireEvent.click(sessRows[0]);
    expect(get(selectedSession)?.id).toBe(sess.id);
    expect(sessRows[0].className).toContain('selected');
  });

  it('clicking the same session again deselects it', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-foo')]);
    render(Sidebar);
    await tick(); await tick();
    const sessRows = await screen.findAllByTestId('sess-row');
    await fireEvent.click(sessRows[0]);
    await fireEvent.click(sessRows[0]);
    expect(get(selectedSession)).toBeNull();
  });

  // FE-1: default tmux names are project-derived, so the same name on two
  // hosts is the normal case. Every lookup must key on the full identity.
  describe('two sessions sharing a tmux_name on different hosts', () => {
    const twinName = 'dev-martin-janci-claude-fleet';
    function twins() {
      const onAlpha = { ...sessionFor(1, twinName), host_alias: 'alpha' };
      const onBeta = { ...sessionFor(1, twinName), host_alias: 'beta' };
      return { onAlpha, onBeta };
    }

    it('selecting the second twin selects that row (not the first by name)', async () => {
      const { onAlpha, onBeta } = twins();
      mockBackend(fakeProjects, [onAlpha, onBeta]);
      render(Sidebar);
      await tick(); await tick();
      const rows = await screen.findAllByTestId('sess-row');
      expect(rows).toHaveLength(2);
      await fireEvent.click(rows[0]);
      expect(get(selectedSession)?.id).toBe(onAlpha.id);
      await fireEvent.click(rows[1]);
      expect(get(selectedSession)?.id).toBe(onBeta.id);
      expect(get(selectedSession)?.host_alias).toBe('beta');
      expect(rows[1].className).toContain('selected');
      expect(rows[0].className).not.toContain('selected');
    });

    it("renaming the second twin sends the rename to that twin's host", async () => {
      const { onAlpha, onBeta } = twins();
      mockBackend(fakeProjects, [onAlpha, onBeta]);
      render(Sidebar);
      await tick(); await tick();
      const rows = await screen.findAllByTestId('sess-row');
      // Tmux rename is the row's ✎ action (double-click edits the label).
      await fireEvent.click(rows[1].querySelector('[data-testid="rename-tmux"]')!);
      const input = await screen.findByTestId('rename-input');
      // Only the second row is in rename mode.
      expect(rows[1].className).toContain('renaming');
      expect(rows[0].className).not.toContain('renaming');
      await fireEvent.input(input, { target: { value: 'dev-renamed' } });
      await fireEvent.keyDown(input, { key: 'Enter' });
      await tick(); await tick();
      const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'rename_session');
      expect(call).toBeDefined();
      expect((call![1] as { args: { host_alias: string; old_name: string; new_name: string } }).args).toEqual({
        host_alias: 'beta',
        old_name: twinName,
        new_name: 'dev-renamed',
      });
    });

    it('killing the second twin targets its host and keeps the first twin selected', async () => {
      const { onAlpha, onBeta } = twins();
      mockBackend(fakeProjects, [onAlpha, onBeta]);
      render(Sidebar);
      await tick(); await tick();
      const rows = await screen.findAllByTestId('sess-row');
      await fireEvent.click(rows[0]); // select alpha's twin
      expect(get(selectedSession)?.id).toBe(onAlpha.id);
      const killBtn = rows[1].querySelector('button[aria-label="Kill"]') as HTMLButtonElement;
      await fireEvent.click(killBtn);
      await fireEvent.click(await screen.findByTestId('confirm-kill'));
      await tick(); await tick();
      const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'kill_session');
      expect((call![1] as { args: { host_alias: string; name: string } }).args).toEqual({
        host_alias: 'beta',
        name: twinName,
      });
      // The selection pointed at alpha's twin; a kill on beta must not clear it.
      expect(get(selectedSession)?.id).toBe(onAlpha.id);
    });
  });

  it('kill button opens an in-app confirm dialog (no window.confirm)', async () => {
    const sess = sessionFor(1, 'dev-foo');
    mockBackend(fakeProjects, [sess]);
    render(Sidebar);
    await tick(); await tick();
    const sessRow = await screen.findByTestId('sess-row');
    const killBtn = sessRow.querySelector('button[aria-label="Kill"]') as HTMLButtonElement;
    await fireEvent.click(killBtn);
    // Confirm dialog appears.
    const confirmBtn = await screen.findByTestId('confirm-kill');
    expect(confirmBtn).toBeInTheDocument();
  });

  it('confirming a kill actually invokes kill_session', async () => {
    const sess = sessionFor(1, 'dev-foo');
    mockBackend(fakeProjects, [sess]);
    render(Sidebar);
    await tick(); await tick();
    const sessRow = await screen.findByTestId('sess-row');
    const killBtn = sessRow.querySelector('button[aria-label="Kill"]') as HTMLButtonElement;
    await fireEvent.click(killBtn);
    const confirmBtn = await screen.findByTestId('confirm-kill');
    await fireEvent.click(confirmBtn);
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls.some((c) => c[0] === 'kill_session')).toBe(true);
  });

  it("purging a project targets its sessions' host, not 'local', and toasts the report", async () => {
    clearToasts();
    const remote = { ...sessionFor(1, 'dev-remote'), host_alias: 'mefistos' };
    mockBackend(fakeProjects, [remote]);
    const backend = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (
      cmd: string,
      a?: unknown,
    ) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, a?: unknown) => {
      if (cmd === 'purge_project') {
        return [
          {
            host_alias: 'mefistos',
            logical_path: '/r/cf',
            physical_path: '/mnt/r/cf',
            purged: ['/mnt/r/cf'],
            not_found: ['/r/cf'],
          },
        ];
      }
      // The post-purge refresh: the project is gone.
      if (cmd === 'refresh_projects') return [];
      return backend(cmd, a);
    });
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(await screen.findByTestId('purge-project'));
    await fireEvent.click(await screen.findByTestId('confirm-purge'));
    await tick(); await tick();
    const purges = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'purge_project');
    expect(purges).toHaveLength(1);
    expect(purges[0][1]).toEqual({
      args: { host_aliases: ['mefistos'], project_path: '/r/cf', project_id: 1 },
    });
    const t = get(toasts).find((x) => x.message.startsWith('Project removed.'));
    expect(t?.message).toBe('Project removed. mefistos: purged /mnt/r/cf; no Claude state for /r/cf');
    expect(t?.kind).toBe('success');
  });

  it('double-click on a session edits its label, with the input focused', async () => {
    mockBackend(fakeProjects, [{ ...sessionFor(1, 'dev-foo'), friendly_name: 'Fix login' }]);
    render(Sidebar);
    await tick(); await tick();
    const sessRow = await screen.findByTestId('sess-row');
    await fireEvent.dblClick(sessRow);
    const input = (await screen.findByTestId('label-input')) as HTMLInputElement;
    expect(input.value).toBe('Fix login');
    // Pins the bind:this → $bindable → beginEdit chain: the row's input ref
    // must reach Sidebar so beginEdit can focus it after its tick().
    await tick();
    expect(input).toHaveFocus();
    expect(input.getAttribute('aria-label')).toContain('Label for dev-foo');
    expect(input.placeholder).toBe('dev-foo');
    expect(screen.queryByTestId('rename-input')).toBeNull();
  });

  it('Enter in label mode saves via set_session_friendly_name, never rename_session', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-foo')]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.dblClick(await screen.findByTestId('sess-row'));
    const input = await screen.findByTestId('label-input');
    await fireEvent.input(input, { target: { value: '  Fix login  ' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick(); await tick();
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls;
    const call = calls.filter((c) => c[0] === 'set_session_friendly_name');
    expect(call).toHaveLength(1);
    expect(call[0][1]).toEqual({
      args: { host_alias: 'local', tmux_name: 'dev-foo', friendly_name: 'Fix login' },
    });
    expect(calls.some((c) => c[0] === 'rename_session')).toBe(false);
    expect(screen.queryByTestId('label-input')).toBeNull();
  });

  it('an emptied label is sent as empty, which clears it', async () => {
    mockBackend(fakeProjects, [{ ...sessionFor(1, 'dev-foo'), friendly_name: 'Old label' }]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.dblClick(await screen.findByTestId('sess-row'));
    const input = await screen.findByTestId('label-input');
    await fireEvent.input(input, { target: { value: '   ' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick(); await tick();
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'set_session_friendly_name');
    expect((call![1] as { args: { friendly_name: string } }).args.friendly_name).toBe('');
  });

  it('pressing Escape in label mode cancels without calling backend', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-foo')]);
    render(Sidebar);
    await tick(); await tick();
    const sessRow = await screen.findByTestId('sess-row');
    await fireEvent.dblClick(sessRow);
    const input = await screen.findByTestId('label-input');
    await fireEvent.input(input, { target: { value: 'typed' } });
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByTestId('label-input')).toBeNull();
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls.some((c) => c[0] === 'rename_session' || c[0] === 'set_session_friendly_name')).toBe(false);
  });

  it('the rename-tmux action edits the tmux name, focused', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-foo')]);
    render(Sidebar);
    await tick(); await tick();
    const row = await screen.findByTestId('sess-row');
    const btn = row.querySelector('[data-testid="rename-tmux"]') as HTMLButtonElement;
    expect(btn.getAttribute('aria-label')).toBe('Rename tmux session');
    await fireEvent.click(btn);
    const input = (await screen.findByTestId('rename-input')) as HTMLInputElement;
    expect(input.value).toBe('dev-foo');
    expect(document.activeElement).toBe(input);
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByTestId('rename-input')).toBeNull();
  });

  it('restart button invokes restart_session', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-foo')]);
    render(Sidebar);
    await tick(); await tick();
    const sessRow = await screen.findByTestId('sess-row');
    const restartBtn = sessRow.querySelector('button[aria-label="Restart"]') as HTMLButtonElement;
    await fireEvent.click(restartBtn);
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls.some((c) => c[0] === 'restart_session')).toBe(true);
  });

  it('hides the owner when repo name is unique', async () => {
    mockBackend(fakeProjects, [sessionFor(1), sessionFor(2), sessionFor(3)]);
    render(Sidebar);
    await tick(); await tick();
    const rows = await screen.findAllByTestId('proj-row');
    for (const row of rows) {
      expect(row).not.toHaveTextContent('martin-janci/');
      expect(row).not.toHaveTextContent('papayapos/');
    }
  });

  it('shows the owner prefix when two repos share a name', async () => {
    const colliding = [
      ...fakeProjects,
      {
        project: { id: 4, owner: 'otherperson', repo: 'claude-fleet', base_path: '/x/cf', last_session_at: null },
        worktrees: [{ id: 41, project_id: 4, host_alias: 'local', name: 'main', path: '/x/cf', branch: 'main' }],
      },
    ];
    mockBackend(colliding, [sessionFor(1, 'dev-a'), sessionFor(4, 'dev-b')]);
    render(Sidebar);
    await tick(); await tick();
    const rows = await screen.findAllByTestId('proj-row');
    const cfRows = rows.filter((r) => r.textContent?.includes('claude-fleet'));
    expect(cfRows).toHaveLength(2);
    expect(cfRows.some((r) => r.textContent?.includes('martin-janci/'))).toBe(true);
    expect(cfRows.some((r) => r.textContent?.includes('otherperson/'))).toBe(true);
  });

  it('footer "+ New session" button opens project picker', async () => {
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    const newBtn = screen.getByTestId('new-session-footer');
    await fireEvent.click(newBtn);
    // Picker now lists all known projects, even those without sessions.
    await tick();
    expect(screen.getByRole('listbox')).toBeInTheDocument();
  });

  it('project picker shows ALL projects regardless of recency/search filter', async () => {
    // Filter to "1d" so only the freshest project (claude-fleet, 60s ago)
    // would be in the main tree. The picker must still list everything.
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a')]);
    render(Sidebar);
    await tick(); await tick();
    // Apply a restrictive filter.
    await fireEvent.click(screen.getByText('1d'));
    await fireEvent.input(screen.getByTestId('sidebar-search'), { target: { value: 'phone' } });
    await tick();
    // Open the picker.
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick();
    const listbox = screen.getByRole('listbox');
    // All three fixture projects should be in the picker, including
    // pos-frontend (14 days old, would be filtered by "1d") and
    // phone-manager (no last_session_at at all).
    expect(listbox.textContent).toContain('claude-fleet');
    expect(listbox.textContent).toContain('pos-frontend');
    expect(listbox.textContent).toContain('phone-manager');
  });

  it('exposes a "1d" recency pill (replaces older "today")', async () => {
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByText('today')).toBeNull();
    expect(screen.getByText('1d')).toBeInTheDocument();
  });

  it('persists the chosen recency to localStorage', async () => {
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByText('7d'));
    await tick();
    expect(localStorage.getItem('cf:pref:recency')).toBe('"7d"');
  });

  it('hydrates recency from localStorage on mount', async () => {
    localStorage.setItem('cf:pref:recency', '"30d"');
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    // Scope to recency pills — host filter has its own active "all" pill.
    const activePill = document.querySelector('.recency .pill.active');
    expect(activePill?.textContent?.trim()).toBe('30d');
  });

  it('shows collapse button when onCollapse prop is provided', async () => {
    mockBackend(fakeProjects, []);
    let collapsed = false;
    render(Sidebar, { props: { onCollapse: () => { collapsed = true; } } });
    await tick(); await tick();
    const btn = screen.getByTestId('sidebar-collapse');
    expect(btn).toBeInTheDocument();
    await fireEvent.click(btn);
    expect(collapsed).toBe(true);
  });

  it('omits collapse button when onCollapse is not passed', async () => {
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('sidebar-collapse')).toBeNull();
  });

  it('header (search + filter) and footer (theme + new) stay rendered even with no projects', async () => {
    mockBackend([], []);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('sidebar-chrome-top')).toBeInTheDocument();
    expect(screen.getByTestId('sidebar-chrome-bottom')).toBeInTheDocument();
    expect(screen.getByTestId('sidebar-search')).toBeInTheDocument();
    expect(screen.getByTestId('theme-toggle')).toBeInTheDocument();
    expect(screen.getByTestId('new-session-footer')).toBeInTheDocument();
  });

  it('renders a host pill for each non-hidden host plus "all"', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [];
      if (cmd === 'list_hosts') return [
        { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null },
        { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null },
        { alias: 'old', ssh_alias: 'old', reachable: false, claude_version: null, tmux_version: null, hidden: true, last_pinged_at: 1, account_uuid: null },
      ];
      return null;
    });
    await Promise.all([bootstrapProjects(), bootstrapSessions(), bootstrapHosts(), bootstrapAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    const hostsBar = document.querySelector('.hosts');
    expect(hostsBar?.textContent).toContain('all');
    expect(hostsBar?.textContent).toContain('local');
    expect(hostsBar?.textContent).toContain('mefistos');
    expect(hostsBar?.textContent).not.toContain('old');
  });

  it('host filter narrows displayed sessions', async () => {
    const local = sessionFor(1, 'dev-local');
    const remote = { ...sessionFor(1, 'dev-remote'), host_alias: 'mefistos' };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [local, remote];
      if (cmd === 'list_hosts') return [
        { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null },
        { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null },
      ];
      return null;
    });
    await Promise.all([bootstrapProjects(), bootstrapSessions(), bootstrapHosts(), bootstrapAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(2);
    const pills = document.querySelectorAll('.hosts .pill');
    // [all, local, mefistos] → click "mefistos"
    const mefistos = Array.from(pills).find((p) => p.textContent?.includes('mefistos'))!;
    await fireEvent.click(mefistos);
    await tick();
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(1);
  });

  it('shows host badge before each session name', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [sessionFor(1, 'dev-foo')];
      if (cmd === 'list_hosts') return [
        { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null },
      ];
      return null;
    });
    await Promise.all([bootstrapProjects(), bootstrapSessions(), bootstrapHosts(), bootstrapAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    const badges = screen.queryAllByTestId('host-badge');
    expect(badges).toHaveLength(1);
    expect(badges[0].textContent).toBe('[local]');
  });

  it('host pill tooltip includes account info when present', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [];
      if (cmd === 'list_hosts') return [
        {
          alias: 'mefistos',
          ssh_alias: 'mefistos',
          reachable: true,
          claude_version: '2.1.144',
          tmux_version: '3.6a',
          hidden: false,
          last_pinged_at: 1,
          account_uuid: 'u1',
        },
      ];
      if (cmd === 'list_accounts') return [
        {
          uuid: 'u1',
          email: 'm.janci@32bit.sk',
          display_name: 'Martin Janci',
          organization_name: '32bit',
          organization_uuid: 'org-1',
          seat_tier: 'max',
          last_seen_at: 1,
        },
      ];
      return null;
    });
    await Promise.all([bootstrapProjects(), bootstrapSessions(), bootstrapHosts(), bootstrapAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    const pills = document.querySelectorAll('.hosts .pill');
    const mef = Array.from(pills).find((p) => p.textContent?.includes('mefistos'));
    expect(mef).toBeDefined();
    expect(mef!.getAttribute('title')).toContain('m.janci@32bit.sk');
    expect(mef!.getAttribute('title')).toContain('max');
  });

  it('host pill tooltip omits account info when host has no account', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [];
      if (cmd === 'list_hosts') return [
        {
          alias: 'noaccount',
          ssh_alias: 'noaccount',
          reachable: true,
          claude_version: '2.1.144',
          tmux_version: '3.6a',
          hidden: false,
          last_pinged_at: 1,
          account_uuid: null,
        },
      ];
      if (cmd === 'list_accounts') return [];
      return null;
    });
    await Promise.all([bootstrapProjects(), bootstrapSessions(), bootstrapHosts(), bootstrapAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    const pills = document.querySelectorAll('.hosts .pill');
    const noaccount = Array.from(pills).find((p) => p.textContent?.includes('noaccount'));
    expect(noaccount).toBeDefined();
    const title = noaccount!.getAttribute('title') ?? '';
    expect(title).not.toContain('@');
    expect(title).not.toContain('(max)');
  });

  it('renders 🔗N badge for sessions with related siblings', async () => {
    const a = sessionFor(1, 'dev-a');
    a.worktree_key = 'main';
    const b = sessionFor(1, 'dev-b');
    b.worktree_key = 'main';
    mockBackend(fakeProjects, [a, b]);
    await Promise.all([bootstrapProjects(), bootstrapSessions(), bootstrapHosts(), bootstrapAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    const badges = screen.queryAllByTestId('related-badge');
    expect(badges).toHaveLength(2); // each session sees one sibling
    expect(badges[0].textContent).toContain('1');
  });

  it('omits 🔗 badge for solo sessions', async () => {
    const solo = sessionFor(1, 'dev-solo');
    solo.worktree_key = 'main';
    mockBackend(fakeProjects, [solo]);
    await Promise.all([bootstrapProjects(), bootstrapSessions(), bootstrapHosts(), bootstrapAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    expect(screen.queryAllByTestId('related-badge')).toHaveLength(0);
  });

  it('omits 🔗 badge for same-project sessions with different worktree_key', async () => {
    const a = sessionFor(1, 'dev-a');
    a.worktree_key = 'main';
    const b = sessionFor(1, 'dev-b');
    b.worktree_key = 'feature-x';
    mockBackend(fakeProjects, [a, b]);
    await Promise.all([bootstrapProjects(), bootstrapSessions(), bootstrapHosts(), bootstrapAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    expect(screen.queryAllByTestId('related-badge')).toHaveLength(0);
  });

  it('shows a 🔍 badge for review sessions', async () => {
    const rev = sessionFor(1, 'dev-foo--review-1');
    rev.kind = 'review';
    rev.reviews_session_id = 999;
    mockBackend(fakeProjects, [sessionFor(1, 'dev-foo'), rev]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByText('🔍')).toBeInTheDocument();
  });

  describe('background-session filter', () => {
    it('hides bg sessions when showBgAgents is false, shows them when true', async () => {
      const normal = sessionFor(1, 'dev-1');           // kind: 'work'
      const bg = { ...sessionFor(1, 'bg:abc'), kind: 'bg' };
      mockBackend(fakeProjects, [normal, bg]);
      showBgAgents.set(true);
      render(Sidebar);
      await tick(); await tick();
      expect(screen.queryByText('bg:abc')).not.toBeNull();
      expect(screen.queryByText('🤖')).not.toBeNull();

      showBgAgents.set(false);
      await tick(); await tick();
      expect(screen.queryByText('bg:abc')).toBeNull();
      expect(screen.queryByText('dev-1')).not.toBeNull();
    });
  });

  it('renders 500 sessions across 25 projects without quadratic blow-up', async () => {
    const sess: SessionRow[] = [];
    const projs: typeof fakeProjects = [];
    for (let p = 1; p <= 25; p++) {
      projs.push({
        project: { id: p, owner: 'o', repo: `r${p}`, base_path: `/r/${p}`, last_session_at: Date.now() / 1000 },
        worktrees: [{ id: p * 10, project_id: p, host_alias: 'local', name: 'main', path: `/r/${p}`, branch: 'main' }],
      });
      for (let i = 0; i < 20; i++) {
        sess.push({
          id: p * 100 + i,
          tmux_name: `proj-${p}-sess-${i}`,
          host_alias: 'local',
          project_id: p,
          worktree_id: p * 10,
          created_at: 1,
          last_activity_at: 1,
          status: 'running',
          notes: null,
          account_uuid: null,
          kind: 'work',
          reviews_session_id: null,
          worktree_key: 'main',
          lost_at: null,
          claude_session_id: null,
          claude_status: null,
          effort_level: null,
          pr_url: null,
          current_activity: null,
          friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [],
        });
      }
    }
    mockBackend(projs, sess);
    vi.mocked(buildSessionsByProject).mockClear();
    vi.mocked(buildRelatedCountById).mockClear();
    render(Sidebar);
    await tick(); await tick();
    // Durable signal: all 25 projects render their rows (correctness at scale —
    // the memoised indices feed every project row).
    const projRows = await screen.findAllByTestId('proj-row');
    expect(projRows).toHaveLength(25);
    // O(N^2) tripwire, made deterministic: the index builders must run once
    // per `$sessions` change (a `$derived` at component level), never once per
    // project/session row. A wall-clock bound was used here before and flaked
    // on loaded machines (jsdom timing is load-sensitive); counting builder
    // calls catches the same regression — someone moving the grouping back
    // into a per-row `{@const}` or a plain function — without depending on
    // machine speed. Two calls are tolerated in case Svelte re-evaluates the
    // derived once after mount; 25 or 500 would mean per-row rebuilds.
    expect(vi.mocked(buildSessionsByProject).mock.calls.length).toBeLessThanOrEqual(2);
    expect(vi.mocked(buildRelatedCountById).mock.calls.length).toBeLessThanOrEqual(2);
    expect(buildSessionsByProject).toHaveBeenCalled();
    expect(buildRelatedCountById).toHaveBeenCalled();
    // Rendering 500 rows in jsdom is load-sensitive (6 s+ on a busy box); the
    // regression signal is the call count above, so give the render room.
  }, 20_000);
});

describe('Sidebar triage (W2 Track D)', () => {
  it('renders a red stuck chip that replaces the claude_status chip', async () => {
    const stuck = { ...sessionFor(1, 'dev-stuck'), claude_status: 'working' as const, stuck_kind: 'press_enter' as const };
    const fine = { ...sessionFor(1, 'dev-fine'), claude_status: 'working' as const };
    mockBackend(fakeProjects, [stuck, fine]);
    render(Sidebar);
    await tick(); await tick();
    const chips = screen.getAllByTestId('stuck-chip');
    expect(chips).toHaveLength(1);
    expect(chips[0]).toHaveTextContent('stuck: press Enter');
    // The stuck row shows no claude chip; the healthy one does.
    const rows = screen.getAllByTestId('sess-row');
    const stuckRow = rows.find((r) => r.getAttribute('data-stuck') === 'press_enter')!;
    expect(stuckRow.querySelector('[data-testid="claude-chip"]')).toBeNull();
    expect(screen.getAllByTestId('claude-chip')).toHaveLength(1);
  });

  it('shows a context badge with amber at 70 and red at 90', async () => {
    const warn = { ...sessionFor(1, 'dev-warn'), context_pct: 72 };
    const crit = { ...sessionFor(1, 'dev-crit'), context_pct: 95 };
    const ok = { ...sessionFor(1, 'dev-ok'), context_pct: 10 };
    const none = sessionFor(1, 'dev-none');
    mockBackend(fakeProjects, [warn, crit, ok, none]);
    render(Sidebar);
    await tick(); await tick();
    const badges = screen.getAllByTestId('context-badge');
    expect(badges).toHaveLength(3);
    const levels = badges.map((b) => b.getAttribute('data-level')).sort();
    expect(levels).toEqual(['crit', 'ok', 'warn']);
    expect(badges.find((b) => b.getAttribute('data-level') === 'crit')).toHaveTextContent('95%');
  });

  it('shows a compact estimated-cost badge only for sessions with counted usage', async () => {
    const spent = {
      ...sessionFor(1, 'dev-spent'),
      usage_input_tokens: 10,
      usage_output_tokens: 20,
      usage_cache_write_tokens: 0,
      usage_cache_read_tokens: 1_000,
      usage_cost_micros: 3_450_000,
      usage_model: 'claude-sonnet-5',
      usage_updated_at: 1,
    };
    const free = sessionFor(1, 'dev-free');
    mockBackend(fakeProjects, [spent, free]);
    render(Sidebar);
    await tick(); await tick();
    const badges = screen.getAllByTestId('cost-badge');
    expect(badges).toHaveLength(1);
    expect(badges[0]).toHaveTextContent('$3.45');
    expect(badges[0].getAttribute('title')).toContain('Estimated cost $3.45');
    expect(badges[0].getAttribute('title')).toContain('claude-sonnet-5');
  });

  it('shows an "unpriced" badge when tokens were counted for a model with no price', async () => {
    const unpriced = {
      ...sessionFor(1, 'dev-local-llm'),
      usage_input_tokens: 900,
      usage_output_tokens: 100,
      usage_cache_write_tokens: 0,
      usage_cache_read_tokens: 0,
      usage_cost_micros: 0,
      usage_model: 'local-llm-7b',
      usage_updated_at: 1,
    };
    mockBackend(fakeProjects, [unpriced]);
    render(Sidebar);
    await tick(); await tick();
    const badge = screen.getByTestId('cost-badge');
    expect(badge).toHaveTextContent('unpriced');
    expect(badge).not.toHaveTextContent('$');
    expect(badge.getAttribute('data-priced')).toBe('false');
    expect(badge.getAttribute('title')).toContain('no price for local-llm-7b');
  });

  it('"N stuck" counter reports the count and toggles a stuck-only filter', async () => {
    const stuck = { ...sessionFor(1, 'dev-stuck'), stuck_kind: 'oom' as const };
    const fine = sessionFor(2, 'dev-fine');
    mockBackend(fakeProjects, [stuck, fine]);
    render(Sidebar);
    await tick(); await tick();
    const pill = screen.getByTestId('stuck-filter');
    expect(pill).toHaveTextContent('1 stuck');
    expect(screen.getAllByTestId('sess-row')).toHaveLength(2);
    await fireEvent.click(pill);
    await tick();
    const rows = screen.getAllByTestId('sess-row');
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent('dev-stuck');
    // Project without a stuck child disappears from the tree.
    expect(screen.getAllByTestId('proj-row')).toHaveLength(1);
    await fireEvent.click(pill);
    await tick();
    expect(screen.getAllByTestId('sess-row')).toHaveLength(2);
  });

  it('"needs attention" filter keeps stuck, safe-kill, ghost and failed rows', async () => {
    const stuck = { ...sessionFor(1, 'dev-stuck'), stuck_kind: 'auth_menu' as const };
    const sk = { ...sessionFor(1, 'dev-sk'), safe_kill_state: 'failed' };
    const ghost = { ...sessionFor(2, 'dev-ghost'), status: 'ghost', lost_at: 5 };
    const failed = { ...sessionFor(2, 'dev-failed'), claude_status: 'failed' as const };
    const fine = { ...sessionFor(2, 'dev-fine'), claude_status: 'working' as const };
    mockBackend(fakeProjects, [stuck, sk, ghost, failed, fine]);
    render(Sidebar);
    await tick(); await tick();
    const pill = screen.getByTestId('attention-filter');
    expect(pill).toHaveTextContent('needs attention (4)');
    await fireEvent.click(pill);
    await tick();
    const names = screen.getAllByTestId('sess-row').map((r) => r.textContent ?? '');
    expect(names.some((n) => n.includes('dev-fine'))).toBe(false);
    expect(screen.getAllByTestId('sess-row')).toHaveLength(4);
  });

  it('orders projects by their worst child status', async () => {
    // Project 1 (claude-fleet) is idle; project 2 (pos-frontend) has a stuck
    // session and must float to the top despite coming second.
    const idle = { ...sessionFor(1, 'dev-idle'), claude_status: 'idle' as const };
    const stuck = { ...sessionFor(2, 'dev-stuck'), stuck_kind: 'reconnect' as const };
    mockBackend(fakeProjects, [idle, stuck]);
    render(Sidebar);
    await tick(); await tick();
    const rows = screen.getAllByTestId('proj-row');
    expect(rows[0]).toHaveTextContent('pos-frontend');
    expect(rows[1]).toHaveTextContent('claude-fleet');
  });

  it('shift-click multi-selects rows and bulk kill confirms then kills each', async () => {
    const a = sessionFor(1, 'dev-a');
    const b = sessionFor(1, 'dev-b');
    mockBackend(fakeProjects, [a, b]);
    render(Sidebar);
    await tick(); await tick();
    const rows = screen.getAllByTestId('sess-row');
    await fireEvent.click(rows[0], { shiftKey: true });
    await fireEvent.click(rows[1], { metaKey: true });
    await tick();
    // Neither click opened the session.
    expect(get(selectedSession)).toBeNull();
    const bar = screen.getByTestId('bulk-bar');
    expect(bar).toHaveTextContent('2 selected');
    await fireEvent.click(screen.getByTestId('bulk-kill'));
    const confirm = await screen.findByTestId('confirm-bulk-kill');
    await fireEvent.click(confirm);
    await tick(); await tick();
    const kills = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'kill_session');
    expect(kills.map((c) => (c[1] as { args: { name: string } }).args.name).sort()).toEqual(['dev-a', 'dev-b']);
    expect(screen.queryByTestId('bulk-bar')).toBeNull();
  });

  it('select mode shows checkboxes and bulk send opens the prompt dialog', async () => {
    const a = sessionFor(1, 'dev-a');
    mockBackend(fakeProjects, [a]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryAllByTestId('select-box')).toHaveLength(0);
    await fireEvent.click(screen.getByTestId('select-mode'));
    await tick();
    const box = screen.getByTestId('select-box');
    await fireEvent.click(box);
    await tick();
    expect(screen.getByTestId('bulk-bar')).toHaveTextContent('1 selected');
    await fireEvent.click(screen.getByTestId('bulk-send'));
    const dialog = await screen.findByTestId('bulk-prompt-dialog');
    expect(dialog).toBeInTheDocument();
    expect(screen.getByTestId('bulk-target-' + a.id)).toHaveTextContent('dev-a');
  });

  it('shows the friendly name by default with tmux_name as secondary text', async () => {
    const named = { ...sessionFor(1, 'dev-martin-janci-claude-fleet--fix-login'), friendly_name: 'Fix login' };
    mockBackend(fakeProjects, [named]);
    render(Sidebar);
    await tick(); await tick();
    const row = screen.getByTestId('sess-row');
    expect(row.querySelector('.sess-name')).toHaveTextContent('Fix login');
    expect(screen.getByTestId('sess-tmux-name')).toHaveTextContent('dev-martin-janci-claude-fleet--fix-login');
  });

  it('shows elapsed time, last prompt and a CI badge as secondary row text', async () => {
    const now = Math.floor(Date.now() / 1000);
    const s = {
      ...sessionFor(1, 'dev-a'),
      started_at: now - 3 * 3600 - 5 * 60,
      last_prompt: 'Implement the triage filter\nsecond line',
      pr_url: 'https://github.com/o/r/pull/3',
      ci_status: 'failing' as const,
    };
    mockBackend(fakeProjects, [s]);
    render(Sidebar);
    await tick(); await tick();
    const meta = screen.getByTestId('sess-meta');
    expect(meta).toHaveTextContent('3h 5m');
    expect(meta).toHaveTextContent('Implement the triage filter');
    expect(meta).not.toHaveTextContent('second line');
    expect(screen.getByTestId('ci-badge')).toHaveTextContent('CI');
  });
});
