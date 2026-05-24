import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

// Three sample projects. Sessions are attached per-test so we can verify
// the new "hide projects without sessions" behavior.
const fakeProjects = [
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
    friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null,
  };
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import Sidebar from './Sidebar.svelte';
import { projects, bootstrapProjects } from './projects';
import { sessions, bootstrapSessions, showBgAgents, type SessionRow } from './sessions';
import { selectedSession, selectSession } from './selection';
import { hosts, bootstrapHosts, hostFilter } from './hosts';
import { accounts, bootstrapAccounts } from './accounts';
import { onboardingDismissed } from './onboarding';

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
          { id: 11, project_id: 1, name: 'main', path: '/x', branch: 'main' },
          { id: 12, project_id: 1, name: 'feature-x', path: '/x/.worktrees/feature-x', branch: 'feature-x' },
          { id: 13, project_id: 1, name: 'bugfix', path: '/x/.worktrees/bugfix', branch: 'bugfix' },
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

  it('double-click on a session enters rename mode', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-foo')]);
    render(Sidebar);
    await tick(); await tick();
    const sessRow = await screen.findByTestId('sess-row');
    await fireEvent.dblClick(sessRow);
    const input = await screen.findByTestId('rename-input');
    expect((input as HTMLInputElement).value).toBe('dev-foo');
  });

  it('pressing Escape in rename mode cancels without calling backend', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-foo')]);
    render(Sidebar);
    await tick(); await tick();
    const sessRow = await screen.findByTestId('sess-row');
    await fireEvent.dblClick(sessRow);
    const input = await screen.findByTestId('rename-input');
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByTestId('rename-input')).toBeNull();
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls.some((c) => c[0] === 'rename_session')).toBe(false);
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
        worktrees: [{ id: 41, project_id: 4, name: 'main', path: '/x/cf', branch: 'main' }],
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
        worktrees: [{ id: p * 10, project_id: p, name: 'main', path: `/r/${p}`, branch: 'main' }],
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
          friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null,
        });
      }
    }
    mockBackend(projs, sess);
    const start = performance.now();
    render(Sidebar);
    await tick(); await tick();
    const elapsed = performance.now() - start;
    // Durable signal: all 25 projects render their rows (correctness at scale —
    // the memoised indices feed every project row). The timing check below is
    // only a coarse O(N^2) tripwire, NOT a precise benchmark: jsdom wall-clock
    // is load-sensitive (parallel test workers, machine load) so the bound is
    // deliberately generous. A real quadratic regression at this size would
    // blow past it by an order of magnitude; normal runs land in the low
    // hundreds of ms.
    const projRows = await screen.findAllByTestId('proj-row');
    expect(projRows).toHaveLength(25);
    expect(elapsed).toBeLessThan(5000);
  });
});
