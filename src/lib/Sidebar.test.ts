import { sidebarView } from './work_view';
import { inboxGroupBy, notWaitingSaid } from './inbox';
import { waitingMissions, waitingOf } from './mission_waits';
import { fireEvent, render, screen, within, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { readPref, uiDensity } from './prefs';

// Three sample projects. Sessions are attached per-test so we can verify
// the new "hide projects without sessions" behavior.
const fakeProjects = [
  {
    project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: Math.floor(Date.now() / 1000) - 60, adopted: false, system: false },
    worktrees: [{ id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/r/cf', branch: 'main' }],
  },
  {
    project: { id: 2, owner: 'papayapos', repo: 'pos-frontend', base_path: '/r/pf', last_session_at: Math.floor(Date.now() / 1000) - 60 * 60 * 24 * 14, adopted: false, system: false },
    worktrees: [{ id: 21, project_id: 2, host_alias: 'local', name: 'main', path: '/r/pf', branch: 'main' }],
  },
  {
    project: { id: 3, owner: 'martin-janci', repo: 'phone-manager', base_path: '/r/pm', last_session_at: null, adopted: false, system: false },
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
    friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
  };
}

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}));

// Wrap the memoised index builders in call-through spies so the scale test
// below can assert they run once per render, not once per row.
vi.mock('./sidebar_index', { spy: true });

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { open as mockedOpen } from '@tauri-apps/plugin-dialog';
import { buildSessionsByProject, buildRelatedCountById } from './sidebar_index';
import { get } from 'svelte/store';
import Sidebar from './Sidebar.svelte';
import { projects, loadProjects } from './projects';
import { sessions, loadSessions, showBgAgents, showFriendlyNames, showRowDetails, sidebarGroupBy, resetTombstonesForTests, type SessionRow } from './sessions';
import { selectedSession, selectSession, selectSessionExplicitly } from './selection';
import { sessionFocus, focusSession } from './session_focus';
import { hosts, loadHosts, hostFilter, resetTombstonesForTests as resetHostTombstones } from './hosts';
import { accounts, loadAccounts } from './accounts';
import { onboardingDismissed } from './onboarding';
import { toasts, clearToasts } from './toasts';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { workFilters, mineItemIds, mineLoaded, DEFAULT_WORK_FILTERS } from './work_filters';
import { resetAccessForTests, setMyGrants } from './access';
import { trackers } from './trackers';
import { switcherRequest } from './switcher_request';
import { addProjectRequest } from './app_views';
import { newSessionRequest, clearNewSessionRequest } from './new_session_request';
import { expectAccessible } from './a11y_check';
import { bootstrapError } from './bootstrap_state';
import { agentFilter, scopeTab } from './session_scope';
import { orgs } from './orgs';

/** Open the sidebar's Filters panel (hosts, recency, work filters, include). */
async function openFilters() {
  if (!document.querySelector('[data-testid="filter-panel"]')) {
    await fireEvent.click(screen.getByTestId('filters-open'));
    await tick();
  }
}
/** Open the ⋯ view-options menu (grouping, friendly names, row details). */
async function openViewOptions() {
  if (!document.querySelector('[data-testid="view-options"]')) {
    await fireEvent.click(screen.getByTestId('view-options-open'));
    await tick();
  }
}

/** The Needs you pill, in the Filters panel (step 3.7). */
async function needsYouPill(): Promise<HTMLElement> {
  await openFilters();
  return screen.getByTestId('needs-you-filter');
}
/** The Select several switch, in the ⋯ view-options menu. */
async function selectModeSwitch(): Promise<HTMLElement> {
  await openViewOptions();
  return screen.getByTestId('select-mode');
}
/** Pick a grouping in the list head's Group select. */
async function groupBy(id: string) {
  await fireEvent.change(screen.getByTestId('group-select'), { target: { value: id } });
  await tick();
}

function mockBackend(projs: typeof fakeProjects, sess: ReturnType<typeof sessionFor>[]) {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: { args?: { id?: number; new_name?: string; alias?: string } }) => {
    if (cmd === 'list_projects') return projs;
    if (cmd === 'list_sessions') return sess;
    // Existing tests don't care about hosts — return empty so $hosts is a
    // valid array (never null) when Sidebar.svelte does `$hosts.filter(...)`.
    if (cmd === 'list_hosts') return [];
    if (cmd === 'list_accounts') return [];
    // Mutation IPCs now return the affected row (or id for
    // kill). The wrapper then patches the store via mergeSession/removeSession;
    // mergeSession(null) would throw. Return a sentinel that satisfies the
    // patch even though these tests only assert that the IPC was invoked.
    const id = args?.args?.id ?? 0;
    if (cmd === 'kill_session') return id;
    if (cmd === 'dismiss_agent_session') return null;
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
  // Most row tests here read 0.5.4's Comfortable row; Compact (the default
  // since the UX audit) has its own tests in SessionRowDensity / below.
  uiDensity.set('comfortable');
  clearNewSessionRequest();
  resetTombstonesForTests();
  resetHostTombstones();
  projects.set([]);
  sessions.set([]);
  hosts.set([]);
  accounts.set([]);
  hostFilter.set('all');
  showBgAgents.set(true);
  showRowDetails.set(true);
  selectSession(null);
  sessionFocus.set(null);
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  switcherRequest.set(null);
  addProjectRequest.set(null);
  // Multi-user M1: forget who this client is, the way a fresh launch has not
  // asked yet. Standalone (the default above) owns every row regardless, so
  // every existing test in this file is unaffected by the access gate.
  resetAccessForTests();
  // Keep Get started (FirstRun) out of the way of these tests.
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
        project: { id: 1, owner: 'o', repo: 'r', base_path: '/x', last_session_at: 0, adopted: false, system: false },
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

  it('an explicit select on a host hidden by the host filter widens the filter to all', async () => {
    // The persisted host filter outlives the New-session dialog: a session
    // created (and auto-selected) on another host used to vanish from the
    // tree with no feedback, so the user kept creating it again.
    const shown = sessionFor(1, 'dev-local');
    const created = { ...sessionFor(1, 'dev-new'), host_alias: 'mefistos' };
    mockBackend(fakeProjects, [shown, created]);
    hostFilter.set('local');
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(1);

    // Select from outside the tree, as onCreated / the quick switcher do —
    // both go through `selectSessionExplicitly`, which is what marks the
    // pick as reveal-worthy.
    selectSessionExplicitly(created);
    await tick(); await tick(); await Promise.resolve();
    expect(get(hostFilter)).toBe('all');
    const ids = screen.queryAllByTestId('sess-row').map((r) => r.getAttribute('data-session-id'));
    expect(ids).toContain(String(created.id));
    expect(ids).toContain(String(shown.id));
  });

  it('a non-explicit reselect to a session on a hidden host does NOT widen the host filter', async () => {
    // A rename/recreate resync or a completed move's follow reselect can
    // move the selection to a session on a different host without any user
    // "open" action — e.g. `moves.ts` calling `selectSession(target, {
    // follow: true })` once a move finishes. That must never reset a filter
    // the user deliberately set.
    const onMefistos = sessionFor(1, 'dev-mefistos');
    onMefistos.host_alias = 'mefistos';
    const onMac = sessionFor(1, 'dev-mac');
    onMac.host_alias = 'mac';
    mockBackend(fakeProjects, [onMefistos, onMac]);
    hostFilter.set('mefistos');
    render(Sidebar);
    await tick(); await tick();

    // Simulate the store's selection moving to the `mac` session through a
    // non-explicit path (no `selectSessionExplicitly` anywhere in it).
    selectSession(onMac, { follow: true });
    await tick(); await tick(); await Promise.resolve();
    expect(get(hostFilter)).toBe('mefistos');
  });

  it('an explicit re-select of the ALREADY-selected session still widens the filter', async () => {
    // The reveal is keyed on `revealSeq`, not the selected id, precisely so
    // this works: the id doesn't change on a re-select, but the user still
    // asked to open it.
    const onMac = { ...sessionFor(1, 'dev-mac'), host_alias: 'mac' };
    mockBackend(fakeProjects, [sessionFor(1, 'dev-local'), onMac]);
    hostFilter.set('local');
    render(Sidebar);
    await tick(); await tick();

    selectSessionExplicitly(onMac);
    await tick(); await tick(); await Promise.resolve();
    expect(get(hostFilter)).toBe('all');

    // The user narrows the filter back down while `onMac` stays selected...
    hostFilter.set('local');
    await tick();
    // ...then explicitly opens the very same session again (e.g. clicking
    // it again from the quick switcher) — same id, but a fresh ask to see it.
    selectSessionExplicitly(onMac);
    await tick(); await tick(); await Promise.resolve();
    expect(get(hostFilter)).toBe('all');
  });

  it('a non-explicit reselect that follows an earlier explicit one does NOT widen the filter', async () => {
    // A bump can't be "replayed": once the explicit reveal for the first
    // session has been applied, a later non-explicit id change (e.g. a
    // rename resync) must not re-widen the filter on the new session's
    // behalf just because a bump happened at some point in the past.
    const onLocal = sessionFor(1, 'dev-local');
    const onMefistos = { ...sessionFor(1, 'dev-mefistos'), host_alias: 'mefistos' };
    const onMac = { ...sessionFor(1, 'dev-mac'), host_alias: 'mac' };
    mockBackend(fakeProjects, [onLocal, onMefistos, onMac]);
    hostFilter.set('local');
    render(Sidebar);
    await tick(); await tick();

    // Explicit select onto `mefistos` — widens as expected.
    selectSessionExplicitly(onMefistos);
    await tick(); await tick(); await Promise.resolve();
    expect(get(hostFilter)).toBe('all');

    // The user narrows the filter back down...
    hostFilter.set('mefistos');
    await tick();
    // ...then a non-explicit reselect (no `selectSessionExplicitly`) moves
    // the selection to `mac` — must stay put, not widen again.
    selectSession(onMac, { follow: true });
    await tick(); await tick(); await Promise.resolve();
    expect(get(hostFilter)).toBe('mefistos');
  });

  // #223 fix round 2: the Sidebar is destroyed and recreated on
  // collapse/expand (App.svelte's `{#if sidebarCollapsed}`). `revealSeq` is
  // a module-level counter that outlives any one Sidebar instance, so a
  // fresh mount must only react to a bump that happens AFTER it exists —
  // never replay whatever `revealSeq` already was, or it would widen the
  // filter for a session that arrived non-explicitly while collapsed.
  describe('reveal across a Sidebar remount', () => {
    it('an explicit select scrolls the row into view ONCE, not once per effect', async () => {
      // Two effects reveal: the id-keyed one and the `revealSeq` one. An
      // explicit select moves the selection AND bumps the sequence in the
      // same flush, so without a gate both fired for the same session.
      const onLocal = sessionFor(1, 'dev-local');
      const onOther = sessionFor(1, 'dev-other');
      mockBackend(fakeProjects, [onLocal, onOther]);
      hostFilter.set('all');
      render(Sidebar);
      await tick(); await tick();

      const scrolled: string[] = [];
      const orig = Element.prototype.scrollIntoView;
      Element.prototype.scrollIntoView = function (this: Element) {
        scrolled.push((this as HTMLElement).dataset.sessionId ?? '');
      };
      try {
        selectSessionExplicitly(onOther);
        await tick(); await tick(); await Promise.resolve(); await tick();
        expect(scrolled).toEqual([String(onOther.id)]);
      } finally {
        Element.prototype.scrollIntoView = orig;
      }
    });

    it('a bump from before mount is not replayed at mount time', async () => {
      const onMac = { ...sessionFor(1, 'dev-mac'), host_alias: 'mac' };
      mockBackend(fakeProjects, [sessionFor(1, 'dev-local'), onMac]);
      hostFilter.set('mefistos');
      // Bump `revealSeq` BEFORE the Sidebar ever mounts.
      selectSessionExplicitly(onMac);

      render(Sidebar);
      await tick(); await tick(); await Promise.resolve();
      expect(get(hostFilter)).toBe('mefistos');
    });

    it('an explicit select after mount still widens the filter', async () => {
      const onMac = { ...sessionFor(1, 'dev-mac'), host_alias: 'mac' };
      mockBackend(fakeProjects, [sessionFor(1, 'dev-local'), onMac]);
      hostFilter.set('mefistos');
      // Same pre-mount bump as above, so the mounted instance's baseline
      // already accounts for it...
      selectSessionExplicitly(onMac);
      render(Sidebar);
      await tick(); await tick(); await Promise.resolve();
      expect(get(hostFilter)).toBe('mefistos');

      // ...but a NEW explicit select after mount is a fresh bump and must
      // still widen.
      selectSessionExplicitly(onMac);
      await tick(); await tick(); await Promise.resolve();
      expect(get(hostFilter)).toBe('all');
    });

    it('unmount, a non-explicit reselect to another host, then remount: the filter stays put', async () => {
      const onMefistos = { ...sessionFor(1, 'dev-mefistos'), host_alias: 'mefistos' };
      const onMac = { ...sessionFor(1, 'dev-mac'), host_alias: 'mac' };
      mockBackend(fakeProjects, [onMefistos, onMac]);
      // Start on a filter that HIDES the session about to be selected, so
      // the widen below is a real event and not a no-op on a matching host.
      hostFilter.set('mac');

      const first = render(Sidebar);
      await tick(); await tick();
      // An explicit select while mounted widens, as established above.
      selectSessionExplicitly(onMefistos);
      await tick(); await tick(); await Promise.resolve();
      expect(get(hostFilter)).toBe('all');

      hostFilter.set('mefistos');
      first.unmount();

      // While unmounted, a non-explicit reselect (no bump) moves to `mac`.
      selectSession(onMac, { follow: true });

      // Remounting must NOT treat the leftover, already-applied `revealSeq`
      // value as a fresh bump for whatever is selected now.
      render(Sidebar);
      await tick(); await tick(); await Promise.resolve();
      expect(get(hostFilter)).toBe('mefistos');
    });
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

  // UX-132 / round-20 F4. `:focus-within` puts the row's action buttons in
  // the tab order, but `keydown` bubbles from the focused <button> up to the
  // row, and a button's activation is the DEFAULT ACTION of that keydown —
  // so an ancestor calling preventDefault() while the event bubbles cancels
  // it. jsdom does not synthesise the click, but it does model
  // `defaultPrevented` exactly as a browser does, and that flag is the whole
  // mechanism: a cancelled keydown is an action that never happens.
  describe('keyboard events from nested controls', () => {
    function keydown(el: Element, key: string): KeyboardEvent {
      const e = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
      el.dispatchEvent(e);
      return e;
    }

    for (const key of ['Enter', ' ']) {
      it(`${key === ' ' ? 'Space' : key} on a row action button is left to the button`, async () => {
        const sess = sessionFor(1, 'dev-foo');
        mockBackend(fakeProjects, [sess]);
        render(Sidebar);
        await tick(); await tick();
        const btn = await screen.findByTestId('restart-session');
        const e = keydown(btn, key);
        await tick();
        // The row must not cancel the button's default action.
        expect(e.defaultPrevented).toBe(false);
        // …and must not quietly navigate somewhere else either.
        expect(get(selectedSession)).toBeNull();
      });
    }

    it('Enter on the row itself still selects the session', async () => {
      const sess = sessionFor(1, 'dev-foo');
      mockBackend(fakeProjects, [sess]);
      render(Sidebar);
      await tick(); await tick();
      const row = (await screen.findAllByTestId('sess-row'))[0];
      const e = keydown(row, 'Enter');
      await tick();
      // The row is a treeitem: Space/Enter on it must scroll nothing.
      expect(e.defaultPrevented).toBe(true);
      expect(get(selectedSession)?.id).toBe(sess.id);
    });

    it('Enter on the project row\'s + button does not collapse the project', async () => {
      mockBackend(fakeProjects, [sessionFor(1, 'dev-foo')]);
      render(Sidebar);
      await tick(); await tick();
      const projRow = await screen.findByTestId('proj-row');
      const plus = within(projRow).getByLabelText('New session');
      keydown(plus, 'Enter');
      await tick();
      // Collapsing would unmount the children — the button's own action
      // (open the new-session dialog) would then be aimed at a folded tree.
      expect(screen.getAllByTestId('sess-row')).toHaveLength(1);
    });

    it('Enter on the project row itself still collapses it', async () => {
      mockBackend(fakeProjects, [sessionFor(1, 'dev-foo')]);
      render(Sidebar);
      await tick(); await tick();
      const projRow = await screen.findByTestId('proj-row');
      keydown(projRow, 'Enter');
      await tick();
      expect(screen.queryAllByTestId('sess-row')).toHaveLength(0);
    });
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

  // Restart kills the running claude and loses its in-flight work, so it
  // confirms like its neighbours Kill and Recreate do — it used to fire on
  // the first click, wearing the same glyph as a harmless Refresh.
  it('restart asks first and only then invokes restart_session', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-foo')]);
    render(Sidebar);
    await tick(); await tick();
    const sessRow = await screen.findByTestId('sess-row');
    const restartBtn = sessRow.querySelector('button[aria-label="Restart"]') as HTMLButtonElement;
    await fireEvent.click(restartBtn);
    await tick();
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    expect(inv.mock.calls.some((c) => c[0] === 'restart_session')).toBe(false);
    await fireEvent.click(await screen.findByTestId('confirm-restart'));
    await tick(); await tick();
    expect(inv.mock.calls.some((c) => c[0] === 'restart_session')).toBe(true);
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
        project: { id: 4, owner: 'otherperson', repo: 'claude-fleet', base_path: '/x/cf', last_session_at: null, adopted: false, system: false },
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

  it('"+ New…" in the list header opens the switcher in New session mode', async () => {
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('new-session-head'));
    expect(get(switcherRequest)).toEqual({ mode: 'new', host: undefined });
    expect(screen.queryByRole('listbox', { name: 'Pick project for new session' })).toBeNull();
  });

  it('an Add project request opens the dialog prefilled, and the added project is kept', async () => {
    const added = {
      project: { id: 42, owner: 'newowner', repo: 'fresh-repo', base_path: '/r/fresh', last_session_at: null, adopted: false, system: false },
      worktrees: [],
    };
    mockBackend(fakeProjects, []);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (cmd: string, args?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) =>
      cmd === 'add_project' ? added : cmd === 'set_project_pick' ? (args as { args: unknown }).args : base(cmd, args),
    );
    render(Sidebar);
    await tick(); await tick();
    addProjectRequest.set({ cloneUrl: 'newowner/fresh-repo' });
    await tick(); await tick();
    expect((screen.getByTestId('clone-url') as HTMLInputElement).value).toBe('newowner/fresh-repo');
    await fireEvent.click(screen.getByTestId('add-create'));
    await vi.waitFor(() => expect(screen.queryByTestId('add-project-dialog')).toBeNull());
    expect(mockedInvoke).toHaveBeenCalledWith('set_project_pick', {
      args: { owner: 'newowner', repo: 'fresh-repo', pinned: false, vis: 'keep', grp: null },
    });
    expect(get(newSessionRequest)?.project.project.repo).toBe('fresh-repo');
  });

  it('a failed keep write after Add project stays quiet (no toast)', async () => {
    const added = {
      project: { id: 42, owner: 'newowner', repo: 'fresh-repo', base_path: '/r/fresh', last_session_at: null, adopted: false, system: false },
      worktrees: [],
    };
    mockBackend(fakeProjects, []);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (cmd: string, args?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === 'set_project_pick') throw { code: 'E_HUB_PROTOCOL', message: 'the hub refused the set_project_pick call' };
      return cmd === 'add_project' ? added : base(cmd, args);
    });
    clearToasts();
    render(Sidebar);
    await tick(); await tick();
    addProjectRequest.set({ cloneUrl: 'newowner/fresh-repo' });
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('add-create'));
    await vi.waitFor(() => expect(screen.queryByTestId('add-project-dialog')).toBeNull());
    await vi.waitFor(() => expect(mockedInvoke).toHaveBeenCalledWith('set_project_pick', expect.anything()));
    await tick();
    expect(get(toasts)).toEqual([]);
  });

  it('after a successful add, NewSessionDialog opens on the returned project', async () => {
    const added = {
      project: { id: 42, owner: 'newowner', repo: 'fresh-repo', base_path: '/r/fresh', last_session_at: null, adopted: false, system: false },
      worktrees: [],
    };
    mockBackend(fakeProjects, []);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (cmd: string, args?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) =>
      cmd === 'add_project' ? added : base(cmd, args),
    );
    render(Sidebar);
    await tick(); await tick();
    addProjectRequest.set({});
    await tick(); await tick();
    // Add project opens on From GitHub (redesign 6.11); this adds by URL.
    await fireEvent.click(screen.getByTestId('add-mode-clone'));
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'newowner/fresh-repo' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await vi.waitFor(() => expect(screen.queryByTestId('add-project-dialog')).toBeNull());
    // App's one New session mount opens on it (redesign 1.9): the Sidebar
    // publishes the request and mounts no dialog of its own.
    expect(get(newSessionRequest)?.project.project.id).toBe(42);
    expect(screen.queryByRole('dialog', { name: 'New session' })).toBeNull();
    expect(get(projects).some((p) => p.project.id === 42)).toBe(true);
  });

  it('adopting a folder while a remote host is chosen opens NewSessionDialog on local', async () => {
    const added = {
      project: { id: 43, owner: 'me', repo: 'thing', base_path: '/Users/me/code/thing', last_session_at: null, adopted: true, system: false },
      worktrees: [],
    };
    mockBackend(fakeProjects, []);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (cmd: string, args?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) =>
      cmd === 'add_project' ? added : base(cmd, args),
    );
    (mockedOpen as ReturnType<typeof vi.fn>).mockResolvedValue('/Users/me/code/thing');
    hosts.set([
      { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    ]);
    render(Sidebar);
    await tick(); await tick();
    addProjectRequest.set({});
    await tick(); await tick();
    const chipFor = (alias: string) =>
      Array.from(document.querySelectorAll<HTMLButtonElement>('.host-pick')).find((b) => (b as HTMLElement).dataset.alias === alias)!;
    await fireEvent.click(chipFor('mefistos'));
    await fireEvent.click(screen.getByTestId('add-mode-folder'));
    await fireEvent.click(screen.getByTestId('choose-folder'));
    await vi.waitFor(() => expect((screen.getByTestId('add-create') as HTMLButtonElement).disabled).toBe(false));
    await fireEvent.click(screen.getByTestId('add-create'));
    await vi.waitFor(() => expect(screen.queryByTestId('add-project-dialog')).toBeNull());
    expect(get(newSessionRequest)?.project.project.id).toBe(43);
    expect(get(newSessionRequest)?.initialHost).toBe('local');
  });

  it("a project row's + asks App's one New session dialog for that project (redesign 1.9)", async () => {
    mockBackend(fakeProjects, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getAllByTitle('New session in this project')[0]);
    await tick();
    expect(get(newSessionRequest)?.project.project.id).toBe(fakeProjects[0].project.id);
    expect(get(newSessionRequest)?.initialHost).toBeUndefined();
    expect(screen.queryByRole('dialog', { name: 'New session' })).toBeNull();
  });

  it('exposes a "1d" recency pill (replaces older "today")', async () => {
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    await openFilters();
    expect(screen.queryByText('today')).toBeNull();
    expect(screen.getByTestId('recency-1d')).toHaveTextContent('1d');
  });

  it('persists the chosen recency to localStorage', async () => {
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    await openFilters();
    await fireEvent.click(screen.getByTestId('recency-7d'));
    await tick();
    expect(localStorage.getItem('cf:pref:recency')).toBe('"7d"');
  });

  it('hydrates recency from localStorage on mount', async () => {
    localStorage.setItem('cf:pref:recency', '"30d"');
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    // Shown as an active-filter chip without opening the panel…
    expect(screen.getByTestId('facet-recency')).toHaveTextContent('Last 30d');
    // …and pressed in the panel. Scoped to recency — hosts have their own "Any".
    await openFilters();
    const activePill = document.querySelector(".recency [aria-pressed='true']");
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

  it('header (search, filter, + New) and footer (keys, count) stay rendered even with no projects', async () => {
    mockBackend([], []);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('sidebar-chrome-top')).toBeInTheDocument();
    expect(screen.getByTestId('sidebar-chrome-bottom')).toBeInTheDocument();
    expect(screen.getByTestId('sidebar-search')).toBeInTheDocument();
    expect(screen.getByTestId('new-session-head')).toBeInTheDocument();
    // UX audit L4: the footer names the list's keys and how many rows it holds.
    expect(screen.getByTestId('sidebar-keys').textContent).toMatch(/move.*open.*select/);
    expect(screen.getByTestId('sidebar-count').textContent).toBe('0 sessions');
    // Redesign 1.4: the "theme: auto" line is gone from the footer; the
    // picker lives in Settings › Appearance (AppearanceSettings.test.ts).
    expect(screen.queryByTestId('theme-toggle')).toBeNull();
    expect(screen.getByTestId('sidebar-chrome-bottom').textContent).not.toMatch(/theme:/);
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
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    await openFilters();
    const hostsBar = document.querySelector('.hosts');
    expect(hostsBar?.textContent).toContain('Any');
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
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(2);
    await openFilters();
    const pills = document.querySelectorAll('.hosts button');
    // [all, local, mefistos] → click "mefistos"
    const mefistos = Array.from(pills).find((p) => p.textContent?.includes('mefistos'))!;
    await fireEvent.click(mefistos);
    await tick();
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(1);
  });

  it('shows the host in the details line of each session', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [sessionFor(1, 'dev-foo')];
      if (cmd === 'list_hosts') return [
        { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null },
      ];
      return null;
    });
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    const badges = screen.queryAllByTestId('host-badge');
    expect(badges).toHaveLength(1);
    expect(badges[0].textContent).toBe('local');
    expect(badges[0].closest('[data-testid="sess-details"]')).not.toBeNull();
  });

  it('a host almost out of disk gets a red mark on its filter chip and the meter in its tooltip', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'list_projects') return fakeProjects;
      if (cmd === 'list_sessions') return [];
      if (cmd === 'list_hosts') return [
        { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, disk_home_free_kb: 3_600_000, disk_home_total_kb: 150_000_000 },
        { alias: 'oci', ssh_alias: 'oci', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, disk_home_free_kb: 72_000_000, disk_home_total_kb: 96_000_000 },
      ];
      return null;
    });
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    await openFilters();
    expect(screen.getByTestId('filter-host-mefistos-alert')).toBeTruthy();
    expect(screen.queryByTestId('filter-host-oci-alert')).toBeNull();
    const pills = document.querySelectorAll('.hosts button');
    const mef = Array.from(pills).find((p) => p.textContent?.includes('mefistos'));
    expect(mef!.getAttribute('title')).toContain('disk 98%');
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
          email: 'm-janci@users.noreply.github.com',
          display_name: 'Martin Janci',
          organization_name: '32bit',
          organization_uuid: 'org-1',
          seat_tier: 'max',
          last_seen_at: 1,
        },
      ];
      return null;
    });
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    await openFilters();
    const pills = document.querySelectorAll('.hosts button');
    const mef = Array.from(pills).find((p) => p.textContent?.includes('mefistos'));
    expect(mef).toBeDefined();
    expect(mef!.getAttribute('title')).toContain('m-janci@users.noreply.github.com');
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
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    await openFilters();
    const pills = document.querySelectorAll('.hosts button');
    const noaccount = Array.from(pills).find((p) => p.textContent?.includes('noaccount'));
    expect(noaccount).toBeDefined();
    const title = noaccount!.getAttribute('title') ?? '';
    expect(title).not.toContain('@');
    expect(title).not.toContain('(max)');
  });

  it('renders the link-icon N badge for sessions with related siblings', async () => {
    const a = sessionFor(1, 'dev-a');
    a.worktree_key = 'main';
    const b = sessionFor(1, 'dev-b');
    b.worktree_key = 'main';
    mockBackend(fakeProjects, [a, b]);
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    const badges = screen.queryAllByTestId('related-badge');
    expect(badges).toHaveLength(2); // each session sees one sibling
    expect(badges[0].textContent).toContain('1');
  });

  it('omits the related badge for solo sessions', async () => {
    const solo = sessionFor(1, 'dev-solo');
    solo.worktree_key = 'main';
    mockBackend(fakeProjects, [solo]);
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    expect(screen.queryAllByTestId('related-badge')).toHaveLength(0);
  });

  it('omits the related badge for same-project sessions with different worktree_key', async () => {
    const a = sessionFor(1, 'dev-a');
    a.worktree_key = 'main';
    const b = sessionFor(1, 'dev-b');
    b.worktree_key = 'feature-x';
    mockBackend(fakeProjects, [a, b]);
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    expect(screen.queryAllByTestId('related-badge')).toHaveLength(0);
  });

  it('shows a search-icon badge for review sessions', async () => {
    const rev = sessionFor(1, 'dev-foo--review-1');
    rev.kind = 'review';
    rev.reviews_session_id = 999;
    mockBackend(fakeProjects, [sessionFor(1, 'dev-foo'), rev]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByRole('img', { name: 'review session' }).querySelector('[data-icon="search"]')).not.toBeNull();
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
      expect(screen.queryByRole('img', { name: 'background agent' })).not.toBeNull();

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
        project: { id: p, owner: 'o', repo: `r${p}`, base_path: `/r/${p}`, last_session_at: Date.now() / 1000, adopted: false, system: false },
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
          friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
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
    expect(chips[0]).toHaveTextContent('Failed · press Enter');
    // The stuck row shows no claude chip; the healthy one does.
    const rows = screen.getAllByTestId('sess-row');
    const stuckRow = rows.find((r) => r.getAttribute('data-stuck') === 'press_enter')!;
    expect(stuckRow.querySelector('[data-testid="claude-chip"]')).toBeNull();
    expect(screen.getAllByTestId('claude-chip')).toHaveLength(1);
  });

  it('offers the numbered choices on a row blocked on a dialog', async () => {
    // The "Needs you" queue is this same row: a blocked session has to be
    // answerable without opening it first.
    const asking = {
      ...sessionFor(1, 'dev-asking'),
      claude_status: 'blocked' as const,
      pending_input: {
        kind: 'permission' as const,
        question: 'Do you want to proceed?',
        options: [
          { n: 1, label: 'Yes', selected: true },
          { n: 2, label: 'No', selected: false },
        ],
      },
    };
    const quiet = { ...sessionFor(1, 'dev-quiet'), claude_status: 'working' as const };
    mockBackend(fakeProjects, [asking, quiet]);
    render(Sidebar);
    await tick(); await tick();
    const cards = screen.getAllByTestId('answer-card');
    expect(cards).toHaveLength(1);
    const opts = screen.getAllByTestId('answer-option');
    expect(opts).toHaveLength(2);
    expect(opts[0].textContent).toMatch(/1.*Yes/);
    expect(opts[1].textContent).toMatch(/2.*No/);
    // Sidebar density: the choices only — Escape and Open terminal live on
    // the full card in the Conversation panel.
    expect(screen.queryByTestId('answer-esc')).toBeNull();
  });

  it('shows no choices on a blocked row whose dialog the tick has not seen', async () => {
    const blocked = { ...sessionFor(1, 'dev-blocked'), claude_status: 'blocked' as const };
    mockBackend(fakeProjects, [blocked]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('answer-card')).toBeNull();
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

  it('the "Needs you" pill reports the count and toggles the triage filter', async () => {
    const stuck = { ...sessionFor(1, 'dev-stuck'), stuck_kind: 'oom' as const };
    const fine = sessionFor(2, 'dev-fine');
    mockBackend(fakeProjects, [stuck, fine]);
    render(Sidebar);
    await tick(); await tick();
    const pill = await needsYouPill();
    expect(pill).toHaveTextContent('Needs you 1');
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

  it('a focused session (a clicked suggestion) is the only row, past the other filters', async () => {
    const stuck = { ...sessionFor(1, 'dev-stuck'), stuck_kind: 'oom' as const };
    const fine = sessionFor(2, 'dev-fine');
    mockBackend(fakeProjects, [stuck, fine]);
    render(Sidebar);
    await tick(); await tick();
    // Needs you would hide the healthy row; the focus shows it anyway.
    await fireEvent.click(await needsYouPill());
    hostFilter.set('elsewhere');
    focusSession(fine.id, 'dev-fine');
    await tick(); await tick();
    const rows = screen.getAllByTestId('sess-row');
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent('dev-fine');
    expect(get(selectedSession)?.id).toBe(fine.id);
    expect(screen.getByTestId('session-focus-bar')).toHaveTextContent('Showing only dev-fine');
    // The chip lifts it: back to the filters the user had.
    await fireEvent.click(screen.getByTestId('session-focus-clear'));
    await tick();
    expect(get(sessionFocus)).toBeNull();
    expect(screen.queryByTestId('session-focus-bar')).toBeNull();
    hostFilter.set('all');
    await tick();
    const after = screen.getAllByTestId('sess-row');
    expect(after).toHaveLength(1);
    expect(after[0]).toHaveTextContent('dev-stuck');
  });

  // UX-08: at zero there is nothing to warn about, so the glyph and the
  // "(0)" go away — but the pill itself must stay, or the filter becomes
  // unreachable the moment the queue drains.
  it('the "Needs you" pill drops the warning glyph and the count at zero', async () => {
    const fine = { ...sessionFor(1, 'dev-fine'), claude_status: 'working' as const };
    mockBackend(fakeProjects, [fine]);
    render(Sidebar);
    await tick(); await tick();
    const pill = await needsYouPill();
    expect(pill).toBeTruthy();
    expect(pill.textContent?.trim()).toBe('Needs you');
    expect(pill.textContent).not.toContain('⚠');
    expect(pill.textContent).not.toContain('(0)');
    // Still a working filter, not a dead label.
    await fireEvent.click(pill);
    await tick();
    expect(pill.getAttribute('aria-pressed')).toBe('true');
  });

  it('the "Needs you" pill ignores a stuck_kind on an external (Outside fleet) row', async () => {
    const stuck = { ...sessionFor(1, 'dev-stuck'), stuck_kind: 'oom' as const };
    const externalStuck = { ...sessionFor(null, 'claude-desktop-session'), kind: 'external', stuck_kind: 'oom' as const };
    mockBackend(fakeProjects, [stuck, externalStuck]);
    render(Sidebar);
    await tick(); await tick();
    const pill = await needsYouPill();
    expect(pill).toHaveTextContent('Needs you 1');
  });

  it('the "Needs you" queue keeps stuck, safe-kill, ghost and failed rows', async () => {
    const stuck = { ...sessionFor(1, 'dev-stuck'), stuck_kind: 'auth_menu' as const };
    const sk = { ...sessionFor(1, 'dev-sk'), safe_kill_state: 'failed' };
    const ghost = { ...sessionFor(2, 'dev-ghost'), status: 'ghost', lost_at: 5 };
    const failed = { ...sessionFor(2, 'dev-failed'), claude_status: 'failed' as const };
    const fine = { ...sessionFor(2, 'dev-fine'), claude_status: 'working' as const };
    mockBackend(fakeProjects, [stuck, sk, ghost, failed, fine]);
    render(Sidebar);
    await tick(); await tick();
    const pill = await needsYouPill();
    // Redesign 0.4: the safe-kill and ghost rows (Paused) stay in the queue
    // but leave the count; stuck and failed still raise it.
    expect(pill).toHaveTextContent('Needs you 2');
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

  it('bulk Archive archives the rows with work, says what it left, and Undo un-archives (step 1.7)', async () => {
    const work = { link_id: 70, item_id: 5, key: 'TASK-5', title: 'Login', source: 'local' } as SessionRow['work'];
    const a = { ...sessionFor(1, 'dev-a'), work };
    const b = sessionFor(1, 'dev-b');
    mockBackend(fakeProjects, [a, b]);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (cmd: string, raw?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, raw?: unknown) => {
      if (cmd === 'tidy_apply') return { results: [{ session_id: a.id, action: 'archive', ok: true, outcome: 'archived' }] };
      if (cmd === 'unarchive_session_work') return a;
      if (cmd === 'tidy_candidates') return { candidates: [] };
      return base(cmd, raw);
    });
    render(Sidebar);
    await tick(); await tick();
    const rows = screen.getAllByTestId('sess-row');
    await fireEvent.click(rows[0], { shiftKey: true });
    await fireEvent.click(rows[1], { metaKey: true });
    await tick();
    await fireEvent.click(screen.getByTestId('bulk-archive'));
    await waitFor(() => expect(get(toasts).length).toBeGreaterThan(0));
    const apply = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'tidy_apply');
    expect((apply[0][1] as { args: { items: unknown[] } }).args.items).toEqual([{ session_id: a.id, action: 'archive', link_id: 70 }]);
    const t = get(toasts).at(-1)!;
    expect(t.message).toBe('Archived 1 session · 1 left as they were (no work linked)');
    expect(t.action?.label).toBe('Undo');
    t.action!.run();
    await waitFor(() =>
      expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'unarchive_session_work')).toHaveLength(1),
    );
    expect(screen.queryByTestId('bulk-bar')).toBeNull();
  });

  it('bulk Clean up opens the Kill dialog in Clean up mode and never kills', async () => {
    const a = sessionFor(1, 'dev-a');
    const b = sessionFor(1, 'dev-b');
    mockBackend(fakeProjects, [a, b]);
    render(Sidebar);
    await tick(); await tick();
    const rows = screen.getAllByTestId('sess-row');
    await fireEvent.click(rows[0], { shiftKey: true });
    await fireEvent.click(rows[1], { metaKey: true });
    await tick();
    await fireEvent.click(screen.getByTestId('bulk-cleanup'));
    expect(await screen.findByRole('dialog', { name: 'Clean up 2 sessions?' })).toBeTruthy();
    await waitFor(() => expect((screen.getByTestId('kill-cleanup') as HTMLButtonElement).disabled).toBe(false));
    await fireEvent.click(screen.getByTestId('kill-cleanup'));
    await waitFor(() =>
      expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'safe_kill_session')).toHaveLength(2),
    );
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'kill_session')).toHaveLength(0);
  });

  it('select mode shows checkboxes and bulk send opens the prompt dialog', async () => {
    const a = sessionFor(1, 'dev-a');
    mockBackend(fakeProjects, [a]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryAllByTestId('select-box')).toHaveLength(0);
    await fireEvent.click(await selectModeSwitch());
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
    expect(row.querySelector('.sess-line1 .sess-name')).toHaveTextContent('Fix login');
    expect(screen.getByTestId('sess-tmux-name').closest('[data-testid="sess-details"]')).not.toBeNull();
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
    expect(screen.getByTestId('sess-details')).toHaveTextContent('3h 5m');
    expect(meta).toHaveTextContent('Implement the triage filter');
    expect(meta).not.toHaveTextContent('second line');
    expect(screen.getByTestId('ci-badge')).toHaveTextContent('CI');
  });

  it('dims the CI badge when the PR reading is old, and says when it was checked', async () => {
    const now = Math.floor(Date.now() / 1000);
    const fresh = { ...sessionFor(1, 'dev-a'), pr_url: 'https://github.com/o/r/pull/3', ci_status: 'passing' as const, pr_checked_at: now - 60 };
    const old = { ...sessionFor(2, 'dev-b'), pr_url: 'https://github.com/o/r/pull/4', ci_status: 'passing' as const, pr_checked_at: now - 3600 };
    mockBackend(fakeProjects, [fresh, old]);
    render(Sidebar);
    await tick(); await tick();
    const [a, b] = screen.getAllByTestId('ci-badge');
    expect(a).not.toHaveClass('ci-badge--stale');
    expect(a).toHaveAttribute('title', 'CI checks: passing');
    expect(b).toHaveClass('ci-badge--stale');
    expect(b.getAttribute('title')).toBe('CI checks: passing, last checked 1h ago');
  });

  it('line 1 holds the name and one status chip; line 2 holds host, worktree, elapsed and prompt', async () => {
    const now = Math.floor(Date.now() / 1000);
    const s = {
      ...sessionFor(1, 'dev-martin-janci-claude-fleet--fix-login'),
      worktree_key: 'fix-login',
      claude_status: 'working' as const,
      started_at: now - 3600,
      last_prompt: 'Implement the triage filter',
      context_pct: 62,
    };
    mockBackend(fakeProjects, [s]);
    render(Sidebar);
    await tick(); await tick();
    const row = screen.getByTestId('sess-row');
    const line1 = row.querySelector('.sess-line1')!;
    expect(line1.querySelector('.sess-name')).toHaveTextContent('dev-martin-janci-claude-fleet--fix-login');
    expect(line1.querySelector('[data-testid="claude-chip"]')).toHaveTextContent('Working');
    const details = screen.getByTestId('sess-details');
    expect(details.querySelector('[data-testid="host-badge"]')).toHaveTextContent('local');
    // The tmux name already ends in "--fix-login" — showing the worktree
    // key again would just repeat the tail of the name already on line 1.
    expect(screen.queryByTestId('sess-tmux-name')).toBeNull();
    expect(details).toHaveTextContent('1h');
    expect(details.querySelector('[data-testid="context-badge"]')).not.toBeNull();
    expect(screen.getByTestId('sess-meta')).toHaveTextContent('Implement the triage filter');
    expect(line1.querySelector('[data-testid="host-badge"]')).toBeNull();
  });

  it('shows the worktree key on line 2 when the tmux name does not end in it', async () => {
    const s = { ...sessionFor(1, 'dev-a'), worktree_key: 'fix-login' };
    mockBackend(fakeProjects, [s]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('sess-tmux-name')).toHaveTextContent('fix-login');
  });

  it('the details pill hides the second row line and persists', async () => {
    mockBackend(fakeProjects, [{ ...sessionFor(1, 'dev-a'), started_at: Math.floor(Date.now() / 1000) - 60 }]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('sess-details')).toBeInTheDocument();
    await openViewOptions();
    const pill = screen.getByTestId('toggle-row-details');
    expect(pill).toHaveAttribute('aria-checked', 'true');
    await fireEvent.click(pill);
    await tick();
    expect(screen.queryByTestId('sess-details')).toBeNull();
    expect(pill).toHaveAttribute('aria-checked', 'false');
    expect(JSON.parse(localStorage.getItem('cf:pref:rows.details')!)).toBe(false);
    // Clicking again re-shows it — only the hide direction is exercised above.
    await fireEvent.click(pill);
    await tick();
    expect(screen.getByTestId('sess-details')).toBeInTheDocument();
    expect(pill).toHaveAttribute('aria-checked', 'true');
    expect(JSON.parse(localStorage.getItem('cf:pref:rows.details')!)).toBe(true);
  });

  it('shows the details line by default with no stored pref', async () => {
    expect(localStorage.getItem('cf:pref:rows.details')).toBeNull();
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a')]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('sess-details')).toBeInTheDocument();
    await openViewOptions();
    expect(screen.getByTestId('toggle-row-details')).toHaveAttribute('aria-checked', 'true');
  });

  it('a ghost row stays one line with an unbracketed host badge', async () => {
    const ghost = { ...sessionFor(2, 'dev-ghost'), status: 'ghost', lost_at: 5 };
    mockBackend(fakeProjects, [ghost]);
    render(Sidebar);
    await tick(); await tick();
    const row = screen.getByTestId('sess-row');
    expect(row.querySelector('.sess-lines')).toBeNull();
    expect(row.querySelector('.sess-details')).toBeNull();
    const badge = screen.getByTestId('host-badge');
    expect(badge.textContent).toBe('local');
  });

  it('the rename editor hides the details line', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-foo')]);
    render(Sidebar);
    await tick(); await tick();
    const row = await screen.findByTestId('sess-row');
    expect(screen.getByTestId('sess-details')).toBeInTheDocument();
    const btn = row.querySelector('[data-testid="rename-tmux"]') as HTMLButtonElement;
    await fireEvent.click(btn);
    await screen.findByTestId('rename-input');
    expect(screen.queryByTestId('sess-details')).toBeNull();
  });
});

describe('Outside fleet group', () => {
  it('groups external rows under a collapsed header that toggles and persists', async () => {
    const ext = { ...sessionFor(null, 'claude-desktop-session'), kind: 'external' };
    mockBackend(fakeProjects, [ext]);
    render(Sidebar);
    await tick(); await tick();

    const header = screen.getByTestId('outside-fleet');
    expect(header).toHaveTextContent('Outside fleet (1)');
    expect(header).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByText('claude-desktop-session')).toBeNull();

    await fireEvent.click(header);
    await tick();
    expect(header).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('claude-desktop-session')).toBeInTheDocument();
    expect(JSON.parse(localStorage.getItem('cf:pref:outside-fleet-open')!)).toBe(true);

    await fireEvent.click(header);
    await tick();
    expect(header).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByText('claude-desktop-session')).toBeNull();
    expect(JSON.parse(localStorage.getItem('cf:pref:outside-fleet-open')!)).toBe(false);
  });

  it('an external row shows the name Claude gives it, the bg:<uuid> only as its tooltip', async () => {
    const ext = { ...sessionFor(null, 'bg:c9b22749-d2fd'), kind: 'external', friendly_name: 'Release cut' };
    mockBackend(fakeProjects, [ext]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('outside-fleet'));
    await tick();
    const name = screen.getByText('Release cut');
    expect(name).toHaveAttribute('title', 'bg:c9b22749-d2fd');
    expect(screen.queryByText('bg:c9b22749-d2fd')).toBeNull();
  });

  it('renders external rows read-only: no label/rename/recreate/kill actions', async () => {
    const ext = { ...sessionFor(null, 'claude-desktop-session'), kind: 'external' };
    mockBackend(fakeProjects, [ext]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('outside-fleet'));
    await tick();
    const row = screen.getByText('claude-desktop-session').closest('[data-testid="sess-row"]') as HTMLElement;
    expect(row.querySelector('[data-testid="edit-label"]')).toBeNull();
    expect(row.querySelector('[data-testid="rename-tmux"]')).toBeNull();
    expect(row.querySelector('[data-testid="recreate-live"]')).toBeNull();
    expect(row.querySelector('.row-actions')).toBeNull();

    // Double-click is the label-edit trigger on a normal row; a read-only
    // row must not enter rename mode either.
    await fireEvent.dblClick(row);
    await tick();
    expect(screen.queryByTestId('label-input')).toBeNull();
  });

  it('no session row anywhere shows a peek-session button', async () => {
    mockBackend(fakeProjects, [{ ...sessionFor(1, 'dev-a'), claude_session_id: 'sess-1' }]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('peek-session')).toBeNull();
  });

  it('an inactive bg agent shows an inactive chip and a working remove-from-list action', async () => {
    const bg = { ...sessionFor(1, 'bg:abc'), kind: 'bg', claude_status: 'stopped' as const };
    mockBackend(fakeProjects, [bg]);
    render(Sidebar);
    await tick(); await tick();

    const chip = screen.getByTestId('inactive-chip');
    expect(chip).toHaveTextContent('Idle · process ended');
    expect(screen.queryByTestId('claude-chip')).toBeNull();

    const removeBtn = screen.getByTestId('remove-from-list');
    await fireEvent.click(removeBtn);
    await tick(); await tick();
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'dismiss_agent_session');
    expect(calls).toHaveLength(1);
    expect(calls[0][1]).toEqual({ args: { session_id: bg.id } });
  });

  it('an inactive bg agent row offers no Kill action', async () => {
    const bg = { ...sessionFor(1, 'bg:abc'), kind: 'bg', claude_status: 'stopped' as const };
    mockBackend(fakeProjects, [bg]);
    render(Sidebar);
    await tick(); await tick();
    const row = screen.getByTestId('remove-from-list').closest('[data-testid="sess-row"]') as HTMLElement;
    expect(row.querySelector('[aria-label="Kill"]')).toBeNull();
  });

  it('a live bg agent row keeps its Kill action', async () => {
    const bg = { ...sessionFor(1, 'bg:live'), kind: 'bg', claude_status: 'working' as const };
    mockBackend(fakeProjects, [bg]);
    render(Sidebar);
    await tick(); await tick();
    const row = screen.getByTestId('sess-row');
    expect(row.querySelector('[aria-label="Kill"]')).not.toBeNull();
  });

  it('a ghosted external row is not listed: fleet cannot restore it', async () => {
    const live = { ...sessionFor(null, 'claude-desktop-live'), kind: 'external' };
    const ghost = { ...sessionFor(null, 'claude-desktop-ghost'), kind: 'external', status: 'ghost', lost_at: 1 };
    mockBackend(fakeProjects, [live, ghost]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('outside-fleet')).toHaveTextContent('Outside fleet (1)');
    await fireEvent.click(screen.getByTestId('outside-fleet'));
    await tick();
    const section = screen.getByTestId('outside-fleet-section');
    expect(section.querySelectorAll('[data-testid="sess-row"]')).toHaveLength(1);
    expect(screen.queryByText('claude-desktop-ghost')).toBeNull();
    expect(screen.queryByTestId('ghost-recreate')).toBeNull();
    expect(screen.queryByTestId('ghost-dismiss')).toBeNull();
  });

  it('only ghosted external rows: no Outside fleet group at all', async () => {
    const ghost = { ...sessionFor(null, 'claude-desktop-ghost'), kind: 'external', status: 'ghost', lost_at: 1 };
    mockBackend(fakeProjects, [ghost]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('outside-fleet')).toBeNull();
  });

  it('Outside fleet rows cannot be bulk-selected (modifier click or select mode)', async () => {
    const ext = { ...sessionFor(null, 'claude-desktop-session'), kind: 'external' };
    const work = sessionFor(1, 'dev-a');
    mockBackend(fakeProjects, [ext, work]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('outside-fleet'));
    await tick();
    const extRow = screen.getByText('claude-desktop-session').closest('[data-testid="sess-row"]') as HTMLElement;
    await fireEvent.click(extRow, { shiftKey: true });
    await tick();
    expect(screen.queryByTestId('bulk-bar')).toBeNull();

    await fireEvent.click(await selectModeSwitch());
    await tick();
    expect(extRow.querySelector('[data-testid="select-box"]')).toBeNull();
    await fireEvent.click(extRow);
    await tick();
    expect(screen.queryByTestId('bulk-bar')).toBeNull();
    // A fleet row in the same mode still selects.
    const workRow = screen.getByText('dev-a').closest('[data-testid="sess-row"]') as HTMLElement;
    await fireEvent.click(workRow.querySelector('[data-testid="select-box"]') as HTMLElement);
    await tick();
    expect(screen.getByTestId('bulk-bar')).toHaveTextContent('1 selected');
  });

  it('the bg-session hint needs a session with a tmux pane, not just a non-bg row', async () => {
    const { anchorEl } = await import('./hints');
    const ext = { ...sessionFor(null, 'claude-desktop-session'), kind: 'external' };
    mockBackend(fakeProjects, [ext]);
    const first = render(Sidebar);
    await tick(); await tick();
    expect(anchorEl('bg-session')).toBeUndefined();
    first.unmount();

    mockBackend(fakeProjects, [sessionFor(1, 'dev-a')]);
    render(Sidebar);
    await tick(); await tick();
    expect(anchorEl('bg-session')).toBeDefined();
  });
});

// #195: cold start into a hub whose wire contract this build cannot use
// (`E_HUB_CONTRACT` on every list load) used to leave the sidebar showing
// its ordinary "No projects yet" empty state right beside the banner that
// already explains what is wrong — reading as "you have no sessions".
describe('Sidebar: a hub contract skew', () => {
  const remote: HubStatus = {
    remote: true,
    url: 'https://fleet.example.com',
    client_name: 'laptop',
    client_mode: null,
    configured_url: 'https://fleet.example.com',
    configured_client_name: 'laptop',
    allow_plaintext: false,
    warning: null,
    restart_required: false,
    unavailable: null,
  };

  it('shows the connection banner’s own sentence instead of "No projects yet"', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'hub_too_old', hub_contract: 1, min_contract: 3 });
    mockBackend([], []);
    render(Sidebar);
    await tick(); await tick();
    const empty = await screen.findByTestId('sidebar-empty');
    expect(empty.textContent).not.toContain('No projects yet');
    expect(empty.textContent).toContain('fleet.example.com');
    expect(empty.textContent?.toLowerCase()).toContain('update the hub');
  });

  it('a too-new hub says to update this app instead', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'hub_too_new', hub_contract: 9, max_contract: 3 });
    mockBackend([], []);
    render(Sidebar);
    await tick(); await tick();
    const empty = await screen.findByTestId('sidebar-empty');
    expect(empty.textContent?.toLowerCase()).toContain('update this app');
  });

  it('a connected hub with no skew renders the ordinary empty state', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' });
    mockBackend([], []);
    render(Sidebar);
    await tick(); await tick();
    const empty = await screen.findByTestId('sidebar-empty');
    // No hosts: the ordinary first-run state (review r13), not a skew sentence.
    expect(empty.textContent).toContain('Start with one host');
  });

  it('standalone mode is untouched', async () => {
    mockBackend([], []);
    render(Sidebar);
    await tick(); await tick();
    const empty = await screen.findByTestId('sidebar-empty');
    // No hosts: the ordinary first-run state (review r13), not a skew sentence.
    expect(empty.textContent).toContain('Start with one host');
  });

  it('Add project is enabled on a connected hub client', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' });
    mockBackend(fakeProjects, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    addProjectRequest.set({ cloneUrl: 'acme/widgets' });
    await tick(); await tick();
    expect((screen.getByTestId('add-create') as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByTestId('add-blocked')).toBeNull();
  });

  it('Add project is disabled with the offline sentence while the hub is unreachable', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'reconnecting', attempt: 2, retry_in_secs: 4, reason: 'stream closed' });
    mockBackend(fakeProjects, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    addProjectRequest.set({});
    await tick(); await tick();
    // A valid repository would enable Add anywhere else; the blocked hub holds it.
    await fireEvent.click(screen.getByTestId('add-mode-clone'));
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'acme/widgets' } });
    expect((screen.getByTestId('add-create') as HTMLButtonElement).disabled).toBe(true);
    const why = screen.getByTestId('add-blocked');
    expect(why.textContent).toContain('unreachable');
    expect(why.textContent).toContain('fleet.example.com');
  });
});

describe('Sidebar — group by work (roadmap M1)', () => {
  // A worktree whose branch names a ticket, on project 1.
  const workProjects = [
    {
      ...fakeProjects[0],
      worktrees: [
        ...fakeProjects[0].worktrees,
        { id: 12, project_id: 1, host_alias: 'local', name: 'abc-123-login', path: '/r/cf-wt', branch: 'abc-123-login' },
      ],
    },
    fakeProjects[1],
  ];

  beforeEach(() => {
    sidebarGroupBy.set('project');
    // These cases are about past and archived work: show it (it is hidden
    // by default — see "archived is hidden by default" below).
    workFilters.set({ ...DEFAULT_WORK_FILTERS, archived: true });
  });

  it('project mode keeps the tree, and shows the work key as a chip on the row', async () => {
    const keyed = { ...sessionFor(1, 'dev-login'), worktree_id: 12 };
    mockBackend(workProjects, [keyed, sessionFor(2, 'dev-pos')]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('work-groups')).toBeNull();
    expect(await screen.findAllByTestId('proj-row')).toHaveLength(2);
    const chip = await screen.findByTestId('work-chip');
    expect(chip).toHaveTextContent('ABC-123');
    expect(chip.getAttribute('title')).toContain('branch abc-123-login');
  });

  it('work mode groups keyed sessions by key and leaves the rest under their project', async () => {
    const a = { ...sessionFor(1, 'dev-login'), worktree_id: 12 };
    const b = { ...sessionFor(2, 'dev-pos-fix'), tags: ['ABC-123'] };
    const plain = sessionFor(2, 'dev-pos');
    mockBackend(workProjects, [a, b, plain]);
    render(Sidebar);
    await tick(); await tick();

    await groupBy('work');

    const workRows = await screen.findAllByTestId('work-row');
    expect(workRows).toHaveLength(1);
    expect(workRows[0]).toHaveTextContent('ABC-123');
    expect(workRows[0]).toHaveTextContent('2');
    const group = screen.getByTestId('work-groups');
    expect(within(group).getAllByTestId('sess-row')).toHaveLength(2);
    // Inside its group the key is in the header, not repeated on the row.
    expect(within(group).queryByTestId('work-chip')).toBeNull();

    // Only the unkeyed session remains in the project tree: project 1 had
    // nothing else, so it is gone; project 2 keeps dev-pos.
    const projRows = screen.getAllByTestId('proj-row');
    expect(projRows).toHaveLength(1);
    expect(projRows[0]).toHaveTextContent('pos-frontend');
    expect(projRows[0]).toHaveTextContent('1');

    const isStr = (v: unknown): v is string => typeof v === 'string';
    expect(readPref('sidebar.group', 'unset', isStr)).toBe('work');
  });

  it('a work group header collapses its sessions', async () => {
    const a = { ...sessionFor(1, 'dev-login'), tags: ['PAY-7'] };
    mockBackend(workProjects, [a]);
    sidebarGroupBy.set('work');
    render(Sidebar);
    await tick(); await tick();
    const group = await screen.findByTestId('work-groups');
    expect(within(group).getAllByTestId('sess-row')).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('work-row'));
    await tick();
    expect(within(group).queryAllByTestId('sess-row')).toHaveLength(0);
  });

  it('work mode: a project header names work for the sessions that have none (M11.1)', async () => {
    const keyed = { ...sessionFor(1, 'dev-login'), tags: ['PAY-7'] };
    const p1 = sessionFor(2, 'dev-pos');
    const p2 = sessionFor(2, 'dev-pos-2');
    mockBackend(workProjects, [keyed, p1, p2]);
    render(Sidebar);
    await tick(); await tick();
    // Project mode has no such control.
    expect(screen.queryByTestId('name-work-group')).toBeNull();
    sidebarGroupBy.set('work');
    await tick(); await tick();
    const buttons = await screen.findAllByTestId('name-work-group');
    expect(buttons).toHaveLength(1);
    expect(buttons[0].getAttribute('title')).toContain('2 sessions with no work');
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (
      cmd: string,
      args?: unknown,
    ) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(
      async (cmd: string, args?: { args?: { session_id?: number } }) => {
        if (cmd === 'name_session_work' || cmd === 'link_session_work') {
          const id = args?.args?.session_id ?? 0;
          const row = [p1, p2].find((r) => r.id === id)!;
          return {
            ...row,
            row_version: 9,
            work: { link_id: id, item_id: 70, key: null, title: 'POS cleanup', source: 'manual' },
          };
        }
        return base(cmd, args);
      },
    );
    await fireEvent.click(buttons[0]);
    await tick();
    const dialog = screen.getByTestId('name-work-dialog');
    expect(within(dialog).getAllByTestId('name-work-session')).toHaveLength(2);
    await fireEvent.input(within(dialog).getByTestId('name-work-title'), {
      target: { value: 'POS cleanup' },
    });
    await fireEvent.click(within(dialog).getByTestId('name-work-submit'));
    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith('link_session_work', {
        args: { session_id: p2.id, item_id: 70 },
      }),
    );
    expect(mockedInvoke).toHaveBeenCalledWith('name_session_work', {
      args: { session_id: p1.id, title: 'POS cleanup' },
    });
  });

  // Multi-user M1: `name_session_work` is `drive` in `share.ts::SESSION_TIER`,
  // and `SessionRowItem`'s per-session "Rename…" has composed both halves since
  // F2 while this group header asked only the hub's — the same control, two
  // surfaces, two answers. Narrowed the way select mode's `bulkKillTargets`
  // narrows, so a mixed group still names the sessions that ARE this client's.
  it('work mode: the group header only names work for sessions this client may drive', async () => {
    const REMOTE: HubStatus = {
      ...STANDALONE,
      remote: true,
      url: 'https://fleet.example.com',
      configured_url: 'https://fleet.example.com',
    };
    const mineA = { ...sessionFor(2, 'dev-mine-a'), owner_person_id: 7 };
    const mineB = { ...sessionFor(2, 'dev-mine-b'), owner_person_id: 7 };
    const theirs = { ...sessionFor(2, 'dev-theirs'), owner_person_id: 9 };
    mockBackend(workProjects, [mineA, mineB, theirs]);
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connected' });
    // Watch on the other person's row: a read, so no work link may be written.
    setMyGrants(7, [{ session_id: theirs.id, level: 'watch' }]);
    sidebarGroupBy.set('work');
    render(Sidebar);
    await tick(); await tick();
    const button = (await screen.findAllByTestId('name-work-group'))[0] as HTMLButtonElement;
    expect(button.disabled).toBe(false);
    // Two of the three, not all three: the watched row is not a target.
    expect(button.getAttribute('title')).toContain('2 sessions with no work');
    await fireEvent.click(button);
    await tick();
    const dialog = screen.getByTestId('name-work-dialog');
    expect(within(dialog).getAllByTestId('name-work-session')).toHaveLength(2);
    expect(dialog.textContent).toContain('dev-mine-a');
    expect(dialog.textContent).toContain('dev-mine-b');
    expect(dialog.textContent).not.toContain('dev-theirs');
  });

  it('work mode: with nothing in the group to drive, the header says why', async () => {
    const REMOTE: HubStatus = {
      ...STANDALONE,
      remote: true,
      url: 'https://fleet.example.com',
      configured_url: 'https://fleet.example.com',
    };
    const theirs = { ...sessionFor(2, 'dev-theirs'), owner_person_id: 9 };
    mockBackend(workProjects, [theirs]);
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connected' });
    setMyGrants(7, [{ session_id: theirs.id, level: 'watch' }]);
    sidebarGroupBy.set('work');
    render(Sidebar);
    await tick(); await tick();
    // A shared row sits in Shared with me (5.8); focused, it is back in its
    // group, under the header this test is about.
    focusSession(theirs.id, 'dev-theirs');
    await tick(); await tick();
    const button = (await screen.findAllByTestId('name-work-group'))[0] as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect(button.getAttribute('title')).toMatch(/watch is read-only/i);
    await fireEvent.click(button);
    await tick();
    expect(screen.queryByTestId('name-work-dialog')).toBeNull();
  });

  // Multi-user M1, F2b: the Done section's `archived · show` chip was gated by
  // NEITHER half — `unarchive_session_work` is `drive` and ROUTES, so both
  // apply. Asked per row, because one group's Done can hold rows of more than
  // one owner.
  it('work mode: the archived chip is per row, and a watched row offers none', async () => {
    const REMOTE: HubStatus = {
      ...STANDALONE,
      remote: true,
      url: 'https://fleet.example.com',
      configured_url: 'https://fleet.example.com',
    };
    const work = (archived: number | null) => ({
      link_id: 5, item_id: 9, key: 'PAY-7', title: 'Retry', source: 'manual',
      status_category: 'done', status_name: 'Done', archived_at: archived,
    });
    const mine = { ...sessionFor(1, 'dev-mine'), owner_person_id: 7, work: work(1_700_000_000) };
    const theirs = { ...sessionFor(1, 'dev-theirs'), owner_person_id: 9, work: work(1_700_000_000) };
    mockBackend(workProjects, [mine, theirs]);
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connected' });
    setMyGrants(7, [{ session_id: theirs.id, level: 'watch' }]);
    sidebarGroupBy.set('work');
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(await screen.findByTestId('work-done'));
    await tick();
    // The watched row is not in this Done: it sits in Shared with me (5.8),
    // which offers no un-archive at all.
    const rows = screen.getAllByTestId('archived-session');
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent('dev-mine');
    expect(within(screen.getByTestId('shared-with-me')).getByText(/dev-theirs/)).toBeTruthy();
    expect(within(screen.getByTestId('shared-with-me')).queryByTestId('archived-chip')).toBeNull();
    // The owner's own row keeps its chip, and it sends.
    const ownChip = within(rows[0]).getByTestId('archived-chip') as HTMLButtonElement;
    expect(ownChip.disabled).toBe(false);
    await fireEvent.click(ownChip);
    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith('unarchive_session_work', {
        args: { session_id: mine.id },
      }),
    );
  });

  it('rolls up PRs and the worst CI state on the group header', async () => {
    const a = { ...sessionFor(1, 'dev-a'), tags: ['PAY-7'], pr_url: 'https://x/pull/1', ci_status: 'passing' as const };
    const b = { ...sessionFor(1, 'dev-b'), tags: ['PAY-7'], pr_url: 'https://x/pull/2', ci_status: 'failing' as const };
    mockBackend(workProjects, [a, b]);
    sidebarGroupBy.set('work');
    render(Sidebar);
    await tick(); await tick();
    const pr = await screen.findByTestId('work-pr');
    expect(pr).toHaveTextContent('PR ×2');
    expect(pr.getAttribute('title')).toContain('CI failing');
  });

  it('past work: a live group gets a collapsed Done, past-only work a collapsed group of its own (M2.5)', async () => {
    const nowSec = Math.floor(Date.now() / 1000);
    const ended = (id: number, key: string, name: string) => ({
      id, ref_key: key, state: 'confirmed', source: 'manual', created_at: 1,
      ended_at: nowSec - 3 * 86400, snap_host: 'local', snap_name: name, snap_branch: key.toLowerCase(),
    });
    const a = { ...sessionFor(1, 'dev-a'), tags: ['PAY-7'] };
    mockBackend(workProjects, [a]);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (
      cmd: string,
      args?: unknown,
    ) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: { args?: { key?: string } }) => {
      if (cmd === 'session_work_links') {
        const key = args?.args?.key;
        if (key === 'PAY-7') return [ended(1, 'PAY-7', 'old pay')];
        if (key === undefined) return [ended(2, 'ABC-9', 'login fix'), ended(1, 'PAY-7', 'old pay')];
        return [];
      }
      return base(cmd, args);
    });
    sidebarGroupBy.set('work');
    render(Sidebar);
    const pg = await screen.findByTestId('past-work-group');
    expect(pg).toHaveTextContent('ABC-9');
    expect(pg).toHaveTextContent('1 session, last 3d ago');
    expect(within(pg).getByTestId('resume-button')).toBeTruthy();
    expect(within(pg).queryAllByTestId('past-work-row')).toHaveLength(0);
    await fireEvent.click(within(pg).getByTestId('past-work-header'));
    await tick();
    const row = within(pg).getByTestId('past-work-row');
    expect(row).toHaveTextContent('login fix');
    expect(row).toHaveTextContent('abc-9');
    expect(row).toHaveTextContent('ended 3d ago');

    // The live PAY-7 group: one session, and its past collapsed under Done.
    const done = screen.getByTestId('work-done');
    expect(done).toHaveTextContent('Done · 1');
    const liveGroup = done.closest('li')!;
    expect(within(liveGroup as HTMLElement).queryAllByTestId('past-work-row')).toHaveLength(0);
    await fireEvent.click(done);
    await tick();
    expect(within(liveGroup as HTMLElement).getByTestId('past-work-row')).toHaveTextContent('old pay');
    // PAY-7 is live: it never shows up as a past-only group as well.
    expect(screen.getAllByTestId('past-work-group')).toHaveLength(1);
  });

  it('past work: Resume continues the last conversation from the header, without opening the group (M2.5)', async () => {
    const nowSec = Math.floor(Date.now() / 1000);
    const ended = {
      id: 2, ref_key: 'ABC-9', state: 'confirmed', source: 'manual', created_at: 1,
      ended_at: nowSec - 3 * 86400, snap_host: 'local', snap_name: 'login fix', snap_branch: 'abc-9',
    };
    const resumed = sessionFor(1, 'dev-resumed');
    mockBackend(workProjects, [{ ...sessionFor(1, 'dev-a'), tags: ['PAY-7'] }]);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (c: string, x?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, x?: { args?: { key?: string } }) => {
      if (cmd === 'session_work_links') return x?.args?.key === undefined ? [ended] : [];
      if (cmd === 'work_resume_plan')
        return {
          key: 'ABC-9', live: [], link_id: 2, host_alias: 'local', branch: 'abc-9',
          modes: [{ mode: 'last', ok: true }, { mode: 'brief', ok: true }, { mode: 'fresh', ok: true }],
        };
      if (cmd === 'resume_work') return resumed;
      return base(cmd, x);
    });
    sidebarGroupBy.set('work');
    render(Sidebar);
    const pg = await screen.findByTestId('past-work-group');
    const quick = within(pg).getByTestId('resume-quick');
    expect(quick.title).toBe('Continue the last conversation on local · branch abc-9');
    await fireEvent.click(quick);
    // The plan is read for that link, then the last conversation resumed
    // from the link the plan named — no brief, no host override.
    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith('resume_work', {
        args: { key: 'ABC-9', mode: 'last', link_id: 2, host_alias: null, brief: null },
      }),
    );
    expect(mockedInvoke).toHaveBeenCalledWith('work_resume_plan', {
      args: { key: 'ABC-9', link_id: 2, host_alias: null, with_brief: false },
    });
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'resume_work')).toHaveLength(1);
    // The new session is selected and listed; the click did not toggle the
    // header it sits in, and no dialog opened.
    await waitFor(() => expect(get(selectedSession)?.id).toBe(resumed.id));
    expect(get(sessions).some((r) => r.id === resumed.id)).toBe(true);
    expect(within(pg).queryAllByTestId('past-work-row')).toHaveLength(0);
    expect(screen.queryByTestId('resume-dialog')).toBeNull();
  });

  it('past work: Resume opens the dialog instead when the last conversation cannot be continued; ▾ always does (M2.5)', async () => {
    const nowSec = Math.floor(Date.now() / 1000);
    const ended = {
      id: 2, ref_key: 'ABC-9', state: 'confirmed', source: 'manual', created_at: 1,
      ended_at: nowSec - 3 * 86400, snap_host: 'local', snap_name: 'login fix', resumable: false,
    };
    mockBackend(workProjects, [{ ...sessionFor(1, 'dev-a'), tags: ['PAY-7'] }]);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (c: string, x?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, x?: { args?: { key?: string } }) => {
      if (cmd === 'session_work_links') return x?.args?.key === undefined ? [ended] : [];
      if (cmd === 'work_resume_plan')
        return {
          key: 'ABC-9', live: [], link_id: 2, host_alias: 'local',
          modes: [
            { mode: 'last', ok: false, reason: 'its transcripts were purged' },
            { mode: 'brief', ok: true },
            { mode: 'fresh', ok: true },
          ],
        };
      return base(cmd, x);
    });
    sidebarGroupBy.set('work');
    render(Sidebar);
    const pg = await screen.findByTestId('past-work-group');
    await fireEvent.click(within(pg).getByTestId('resume-quick'));
    const dialog = await screen.findByTestId('resume-dialog');
    const last = await within(dialog).findByTestId('resume-mode-last');
    expect(last.closest('label')).toHaveTextContent('its transcripts were purged');
    expect(mockedInvoke).not.toHaveBeenCalledWith('resume_work', expect.anything());
    expect(get(selectedSession)).toBeNull();
    // Cancel closes it; ▾ reopens it whatever the plan says. (The dialog
    // renders inside the header, so its clicks bubble to the header's own
    // toggle — Cancel here also expands the group; not asserted.)
    await fireEvent.click(within(dialog).getByText('Cancel'));
    await tick();
    expect(screen.queryByTestId('resume-dialog')).toBeNull();
    await fireEvent.click(within(within(pg).getByTestId('past-work-header')).getByTestId('resume-more'));
    expect(await screen.findByTestId('resume-dialog')).toBeTruthy();
    expect(mockedInvoke).not.toHaveBeenCalledWith('resume_work', expect.anything());
  });

  it('archived sessions sit in Done, one click un-archives; reopened and done headers (M7.3)', async () => {
    const work = (archived: number | null) => ({
      link_id: 5, item_id: 9, key: 'PAY-7', title: 'Retry', source: 'manual',
      status_category: 'done', status_name: 'Done', archived_at: archived,
    });
    const live = { ...sessionFor(1, 'dev-live'), work: work(null) };
    const parked = { ...sessionFor(1, 'dev-parked'), work: work(1_700_000_000) };
    mockBackend(workProjects, [live, parked]);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (
      cmd: string,
      args?: unknown,
    ) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === 'work_reopened')
        return [{ item_id: 9, key: 'PAY-7', title: 'Retry', reopened_at: 5, past_sessions: 2 }];
      if (cmd === 'unarchive_session_work') return { ...parked, work: work(null) };
      return base(cmd, args);
    });
    sidebarGroupBy.set('work');
    render(Sidebar);
    const group = await screen.findByTestId('work-groups');
    // Only the live one is listed; the archived one is under Done.
    expect(within(group).getAllByTestId('sess-row')).toHaveLength(1);
    const header = screen.getByTestId('work-row');
    expect(header.classList.contains('work-done')).toBe(true);
    const badge = await screen.findByTestId('work-reopened-badge');
    expect(badge).toHaveTextContent('reopened · 2 past sessions');
    expect(within(header).getByTestId('resume-button')).toBeTruthy();
    const done = screen.getByTestId('work-done');
    expect(done).toHaveTextContent('Done · 1');
    await fireEvent.click(done);
    await tick();
    const archived = screen.getByTestId('archived-session');
    expect(archived).toHaveTextContent('dev-parked');
    // Redesign step 7.2: the chip is a control of the archived row itself,
    // and the whole group (header, Done, rows) passes as a tree.
    const parkedRow = within(archived).getByRole('treeitem');
    expect(within(parkedRow).getByRole('button', { name: 'archived · show' })).toBeTruthy();
    await expectAccessible(group);
    await fireEvent.click(within(archived).getByTestId('archived-chip'));
    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith('unarchive_session_work', {
        args: { session_id: parked.id },
      }),
    );
    await tick();
    expect(within(group).getAllByTestId('sess-row').length).toBeGreaterThanOrEqual(2);
  });

  it('a focused session whose link is archived is shown as a row, not hidden under Done', async () => {
    const work = (archived: number | null) => ({
      link_id: 5, item_id: 9, key: 'PAY-7', title: 'Retry', source: 'manual',
      status_category: 'done', status_name: 'Done', archived_at: archived,
    });
    const live = { ...sessionFor(1, 'dev-live'), work: work(null) };
    const parked = { ...sessionFor(1, 'dev-parked'), work: work(1_700_000_000) };
    mockBackend(workProjects, [live, parked]);
    sidebarGroupBy.set('work');
    render(Sidebar);
    const group = await screen.findByTestId('work-groups');
    expect(within(group).getAllByTestId('sess-row')).toHaveLength(1);
    expect(screen.getByTestId('work-done')).toHaveTextContent('Done · 1');
    // A tidy-up candidate clicked in the sheet: the focus must reveal the row.
    focusSession(parked.id, 'dev-parked');
    await tick(); await tick();
    const rows = screen.getAllByTestId('sess-row');
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent('dev-parked');
    expect(screen.queryByTestId('work-done')).toBeNull();
    expect(screen.getByTestId('session-focus-bar')).toHaveTextContent('Showing only dev-parked');
    // Lifting the focus puts it back under Done.
    await fireEvent.click(screen.getByTestId('session-focus-clear'));
    await tick(); await tick();
    expect(screen.getAllByTestId('sess-row')).toHaveLength(1);
    expect(screen.getByTestId('work-done')).toHaveTextContent('Done · 1');
  });

  it('the purge confirmation names the work that loses its conversations (M2.5)', async () => {
    mockBackend(workProjects, [sessionFor(1, 'dev-a')]);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (
      cmd: string,
      args?: unknown,
    ) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === 'work_purge_impact') return { keys: ['ABC-1', 'PAY-7'] };
      return base(cmd, args);
    });
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click((await screen.findAllByTestId('purge-project'))[0]);
    const warn = await screen.findByTestId('purge-work-keys');
    expect(warn).toHaveTextContent('ABC-1, PAY-7');
  });

  it('with nothing keyed, work mode looks exactly like project mode', async () => {
    mockBackend(workProjects, [sessionFor(1, 'dev-a'), sessionFor(2, 'dev-b')]);
    sidebarGroupBy.set('work');
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('work-groups')).toBeNull();
    expect(await screen.findAllByTestId('proj-row')).toHaveLength(2);
    expect(screen.queryByTestId('sidebar-empty')).toBeNull();
  });
});

describe('Sidebar work filters (work graph M10.4)', () => {
  const w = (key: string, item: number, status: string, archived: number | null = null) => ({
    link_id: item, item_id: item, key, title: key, source: 'manual',
    status_category: status, archived_at: archived,
  });
  const jira = (id: number, prefix: string) => ({
    id, provider: 'jira_cloud', name: `Jira ${prefix}`, site_url: `https://${prefix}.example`,
    state: 'ok', created_at: 0, config: { key_prefixes: [prefix] },
  });
  const names = () => screen.queryAllByTestId('sess-row').map((r) => r.textContent ?? '');

  beforeEach(() => {
    sidebarGroupBy.set('project');
    workFilters.set({ ...DEFAULT_WORK_FILTERS });
    trackers.set([]);
    mineItemIds.set(new Set());
    mineLoaded.set(false);
  });

  it('no work group in the panel without work to filter', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a')]);
    render(Sidebar);
    await tick();
    await openFilters();
    expect(screen.queryByTestId('work-filters-sessions')).toBeNull();
    expect(screen.getByTestId('bg-toggle')).toBeTruthy();
  });

  it('status chips filter the tree, count on the pill, and persist', async () => {
    const a = { ...sessionFor(1, 'dev-a'), work: w('PAY-1', 1, 'in_progress') };
    const b = { ...sessionFor(1, 'dev-b'), work: w('PAY-2', 2, 'todo') };
    mockBackend(fakeProjects, [a, b]);
    render(Sidebar);
    await tick();
    expect(screen.queryByTestId('work-filters-sessions')).toBeNull();
    await openFilters();
    // Has-session is work mode only; one tracker needs no tracker chips.
    expect(screen.queryByTestId('wf-session-no')).toBeNull();
    expect(screen.queryByTestId('wf-tracker-all')).toBeNull();
    await fireEvent.click(screen.getByTestId('wf-status-todo'));
    await tick();
    expect(names().some((n) => n.includes('dev-b'))).toBe(true);
    expect(names().some((n) => n.includes('dev-a'))).toBe(false);
    // Counted on the Filters button, and named in the strip of chips.
    expect(screen.getByTestId('filters-open')).toHaveAttribute('aria-label', 'Filters, 1 active');
    expect(screen.getByTestId('facet-wf-status')).toHaveTextContent('Status: To do');
    const isAny = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null;
    expect(readPref('sidebar.work-filters.v2', {}, isAny)).toMatchObject({ status: 'todo' });
    await fireEvent.click(screen.getByTestId('wf-clear'));
    await tick();
    expect(names()).toHaveLength(2);
    expect(get(workFilters)).toEqual(DEFAULT_WORK_FILTERS);
  });

  it('offers the tracker’s own status names (QA Review) as chips beside the categories', async () => {
    const a = { ...sessionFor(1, 'dev-a'), work: { ...w('PAY-1', 1, 'in_progress'), status_name: 'In Progress' } };
    const b = { ...sessionFor(1, 'dev-b'), work: { ...w('PAY-2', 2, 'in_progress'), status_name: 'QA Review' } };
    const c = { ...sessionFor(1, 'dev-c'), work: { ...w('PAY-3', 3, 'todo'), status_name: 'To Do' } };
    mockBackend(fakeProjects, [a, b, c]);
    render(Sidebar);
    await tick();
    await openFilters();
    const chips = screen.getAllByTestId('wf-status-name').map((c) => c.textContent);
    // Workflow order (to do, in progress), then by name.
    expect(chips).toEqual(['To Do', 'In Progress', 'QA Review']);
    // "in progress" lumps both together; the name picks the one column.
    await fireEvent.click(screen.getByText('QA Review'));
    await tick();
    expect(names().some((n) => n.includes('dev-b'))).toBe(true);
    expect(names().some((n) => n.includes('dev-a'))).toBe(false);
    expect(names().some((n) => n.includes('dev-c'))).toBe(false);
    expect(get(workFilters).status).toBe('name:QA Review');
    // A second click on the active chip turns it off.
    await fireEvent.click(screen.getByText('QA Review'));
    await tick();
    expect(names()).toHaveLength(3);
  });

  it('a stored status name no session is in any more does not empty the tree', async () => {
    workFilters.set({ ...DEFAULT_WORK_FILTERS, status: 'name:Blocked' });
    const a = { ...sessionFor(1, 'dev-a'), work: { ...w('PAY-1', 1, 'in_progress'), status_name: 'In Progress' } };
    mockBackend(fakeProjects, [a]);
    render(Sidebar);
    await tick();
    expect(names()).toHaveLength(1);
  });

  it('mine reads the hub’s mine view and composes with needs-you', async () => {
    const a = { ...sessionFor(1, 'dev-a'), work: w('PAY-1', 1, 'in_progress') };
    const b = { ...sessionFor(1, 'dev-b'), work: w('PAY-2', 2, 'in_progress') };
    mockBackend(fakeProjects, [a, b]);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (c: string, x?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, x?: unknown) =>
      cmd === 'work_tickets' ? [{ id: 2 }] : base(cmd, x),
    );
    render(Sidebar);
    await tick();
    await openFilters();
    await fireEvent.click(screen.getByTestId('wf-mine'));
    await waitFor(() => expect(names()).toHaveLength(1));
    expect(names()[0]).toContain('dev-b');
    expect(mockedInvoke).toHaveBeenCalledWith('work_tickets', { args: { view: 'mine', limit: 200 } });
    // Needs-you on top: nothing of mine needs me.
    await fireEvent.click(await needsYouPill());
    await tick();
    expect(names()).toHaveLength(0);
  });

  it('tracker chips show with two trackers and filter by the key’s tracker', async () => {
    const a = { ...sessionFor(1, 'dev-a'), work: w('PAY-1', 1, 'todo') };
    const b = { ...sessionFor(1, 'dev-b'), work: w('OPS-2', 2, 'todo') };
    mockBackend(fakeProjects, [a, b]);
    trackers.set([jira(1, 'PAY'), jira(2, 'OPS')] as never);
    render(Sidebar);
    await tick();
    await openFilters();
    await fireEvent.click(screen.getByTestId('wf-tracker-2'));
    await tick();
    expect(names()).toHaveLength(1);
    expect(names()[0]).toContain('dev-b');
  });

  it('work mode: archived past work is hidden by default, counted, and one click away', async () => {
    const nowSec = Math.floor(Date.now() / 1000);
    const a = { ...sessionFor(1, 'dev-a'), tags: ['PAY-7'] };
    mockBackend(fakeProjects, [a]);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (c: string, x?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, x?: { args?: { key?: string } }) => {
      if (cmd === 'session_work_links') {
        return x?.args?.key === undefined
          ? [{ id: 2, ref_key: 'ABC-9', state: 'confirmed', source: 'manual', created_at: 1, ended_at: nowSec - 60, snap_host: 'local', snap_name: 'old' }]
          : [];
      }
      return base(cmd, x);
    });
    sidebarGroupBy.set('work');
    render(Sidebar);
    // Hidden, but the list says so and brings it back.
    const row = await screen.findByTestId('archived-row');
    expect(row).toHaveTextContent('1 archived hidden');
    expect(screen.queryByTestId('past-work-group')).toBeNull();
    await fireEvent.click(screen.getByTestId('archived-toggle'));
    await screen.findByTestId('past-work-group');
    expect(screen.getByTestId('archived-row')).toHaveTextContent('Showing archived work');
    expect(names()).toHaveLength(1);
    await openFilters();
    expect(screen.getByTestId('wf-hide-archived')).toHaveAttribute('aria-checked', 'true');
    await fireEvent.click(screen.getByTestId('wf-session-no'));
    await tick();
    expect(names()).toHaveLength(0);
    expect(screen.getByTestId('past-work-group')).toHaveTextContent('ABC-9');
    await fireEvent.click(screen.getByTestId('wf-session-yes'));
    await tick();
    expect(names()).toHaveLength(1);
    expect(screen.queryByTestId('past-work-group')).toBeNull();
    await fireEvent.click(screen.getByTestId('wf-session-any'));
    await fireEvent.click(screen.getByTestId('wf-hide-archived'));
    await tick();
    expect(names()).toHaveLength(1);
    expect(screen.queryByTestId('past-work-group')).toBeNull();
  });
  it('the archived count matches a search the way the list does (project owner / repo, a sibling row)', async () => {
    const live = { ...sessionFor(2, 'dev-a'), work: w('PAY-1', 1, 'in_progress') };
    const parked = { ...sessionFor(2, 'dev-parked'), work: w('PAY-2', 2, 'done', 100) };
    const other = { ...sessionFor(1, 'old-x'), work: w('PAY-3', 3, 'done', 100) };
    mockBackend(fakeProjects, [live, parked, other]);
    render(Sidebar);
    await tick();
    expect(await screen.findByTestId('archived-row')).toHaveTextContent('2 archived hidden');
    // The repo matches: showing archived would list the whole project, so
    // its archived row counts though its own name does not match.
    await fireEvent.input(screen.getByTestId('sidebar-search'), { target: { value: 'pos-frontend' } });
    await waitFor(() => expect(screen.getByTestId('archived-row')).toHaveTextContent('1 archived hidden'));
    // A sibling row matches: the same.
    await fireEvent.input(screen.getByTestId('sidebar-search'), { target: { value: 'nothing' } });
    await waitFor(() => expect(screen.queryByTestId('archived-row')).toBeNull());
    await fireEvent.input(screen.getByTestId('sidebar-search'), { target: { value: 'dev-a' } });
    await waitFor(() => expect(screen.getByTestId('archived-row')).toHaveTextContent('1 archived hidden'));
    await fireEvent.click(screen.getByTestId('archived-toggle'));
    await tick();
    expect(names().map((n) => n.includes('dev-parked') || n.includes('dev-a'))).toEqual([true, true]);
  });

  it('work mode: the archived count matches a work group the way the list does', async () => {
    const live = { ...sessionFor(1, 'dev-a'), tags: ['PAY-7'] };
    const parked = { ...sessionFor(1, 'dev-parked'), tags: ['PAY-7'], work: w('PAY-7', 7, 'done', 100) };
    mockBackend(fakeProjects, [live, parked]);
    sidebarGroupBy.set('work');
    render(Sidebar);
    expect(await screen.findByTestId('archived-row')).toHaveTextContent('1 archived hidden');
    await fireEvent.input(screen.getByTestId('sidebar-search'), { target: { value: 'nothing' } });
    await waitFor(() => expect(screen.queryByTestId('archived-row')).toBeNull());
    // The group matches through its live row: showing archived lists both.
    await fireEvent.input(screen.getByTestId('sidebar-search'), { target: { value: 'dev-a' } });
    await waitFor(() => expect(screen.getByTestId('archived-row')).toHaveTextContent('1 archived hidden'));
  });

  it('nothing archived and no past work: no archived row, in either grouping', async () => {
    const a = { ...sessionFor(1, 'dev-a'), tags: ['PAY-7'], work: w('PAY-7', 7, 'in_progress') };
    mockBackend(fakeProjects, [a, sessionFor(null, 'loose')]);
    const view = render(Sidebar);
    await tick(); await tick();
    expect(names()).toHaveLength(2);
    expect(screen.queryByTestId('archived-row')).toBeNull();
    view.unmount();
    sidebarGroupBy.set('work');
    render(Sidebar);
    await screen.findByTestId('work-groups');
    await tick();
    expect(screen.queryByTestId('archived-row')).toBeNull();
  });

  it('the archived count matches a project by owner the way the list does', async () => {
    const live = { ...sessionFor(1, 'dev-a'), work: w('PAY-1', 1, 'in_progress') };
    const parked = { ...sessionFor(1, 'dev-parked'), work: w('PAY-2', 2, 'done', 100) };
    const other = { ...sessionFor(2, 'old-x'), work: w('PAY-3', 3, 'done', 100) };
    mockBackend(fakeProjects, [live, parked, other]);
    render(Sidebar);
    await tick();
    expect(await screen.findByTestId('archived-row')).toHaveTextContent('2 archived hidden');
    await fireEvent.input(screen.getByTestId('sidebar-search'), { target: { value: 'martin-janci' } });
    await waitFor(() => expect(screen.getByTestId('archived-row')).toHaveTextContent('1 archived hidden'));
    expect(names()).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('archived-toggle'));
    await tick();
    expect(names()).toHaveLength(2);
    expect(names().some((n) => n.includes('dev-parked'))).toBe(true);
  });

  it('work mode: past work counts under a live group only when the group matches; past-only by key or name', async () => {
    const nowSec = Math.floor(Date.now() / 1000);
    const link = (id: number, ref_key: string, snap_name: string) => ({
      id, ref_key, state: 'confirmed', source: 'manual', created_at: 1, ended_at: nowSec - 60, snap_host: 'local', snap_name,
    });
    mockBackend(fakeProjects, [{ ...sessionFor(1, 'dev-a'), tags: ['PAY-7'] }]);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (c: string, x?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, x?: { args?: { key?: string } }) =>
      cmd === 'session_work_links'
        ? x?.args?.key === undefined
          ? [link(2, 'PAY-7', 'done-pay'), link(3, 'ABC-9', 'legacy-thing')]
          : []
        : base(cmd, x),
    );
    sidebarGroupBy.set('work');
    render(Sidebar);
    expect(await screen.findByTestId('archived-row')).toHaveTextContent('2 archived hidden');
    const search = async (value: string) =>
      fireEvent.input(screen.getByTestId('sidebar-search'), { target: { value } });
    // The live group matches through its row: its Done counts, ABC-9 not.
    await search('dev-a');
    await waitFor(() => expect(screen.getByTestId('archived-row')).toHaveTextContent('1 archived hidden'));
    // A past link's own name does not match a live group.
    await search('done-pay');
    await waitFor(() => expect(screen.queryByTestId('archived-row')).toBeNull());
    // A past-only group matches by a link's name, or by its key.
    await search('legacy');
    await waitFor(() => expect(screen.getByTestId('archived-row')).toHaveTextContent('1 archived hidden'));
    await search('abc-9');
    await waitFor(() => expect(screen.getByTestId('archived-row')).toHaveTextContent('1 archived hidden'));
    await fireEvent.click(screen.getByTestId('archived-toggle'));
    const groups = await screen.findAllByTestId('past-work-group');
    expect(groups).toHaveLength(1);
    expect(groups[0]).toHaveTextContent('ABC-9');
  });

  it('showing archived reveals exactly the counted rows, under host, recency and search', async () => {
    const now = Math.floor(Date.now() / 1000);
    const recent = { last_activity_at: now - 60 };
    const done = (item: number) => w(`PAY-${item}`, item, 'done', 100);
    mockBackend(fakeProjects, [
      { ...sessionFor(1, 'dev-a'), ...recent, work: w('PAY-1', 1, 'in_progress') },
      { ...sessionFor(1, 'dev-b'), ...recent, work: done(2) },
      { ...sessionFor(1, 'dev-c'), ...recent, host_alias: 'mefistos', work: done(3) },
      { ...sessionFor(2, 'dev-d'), last_activity_at: now - 40 * 86400, work: done(4) },
      { ...sessionFor(3, 'zzz'), ...recent, work: done(5) },
      { ...sessionFor(null, 'dev-loose'), ...recent, work: done(6) },
      { ...sessionFor(null, 'nomatch'), ...recent, work: done(7) },
      { ...sessionFor(null, 'dev-live'), ...recent },
    ]);
    localStorage.setItem('cf:pref:recency', '"7d"');
    hostFilter.set('local');
    try {
      render(Sidebar);
      await tick(); await tick();
      await fireEvent.input(screen.getByTestId('sidebar-search'), { target: { value: 'dev' } });
      await waitFor(() => expect(screen.getByTestId('archived-row')).toHaveTextContent('2 archived hidden'));
      const before = names().length;
      expect(before).toBe(2);
      await fireEvent.click(screen.getByTestId('archived-toggle'));
      await tick();
      expect(names()).toHaveLength(before + 2);
      expect(names().filter((n) => n.includes('dev-b') || n.includes('dev-loose'))).toHaveLength(2);
    } finally {
      hostFilter.set('all');
      localStorage.removeItem('cf:pref:recency');
    }
  });
});

describe('Sidebar filters: one set of rules for every section', () => {
  const names = () => screen.queryAllByTestId('sess-row').map((r) => r.textContent ?? '');
  const pastLink = (ref_key: string, snap_host: string) => ({
    id: 9, ref_key, state: 'confirmed', source: 'manual', created_at: 1,
    ended_at: Math.floor(Date.now() / 1000) - 60, snap_host, snap_name: `old-${snap_host}`,
  });
  function withPastWork(links: unknown[]) {
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (c: string, x?: unknown) => Promise<unknown>;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, x?: { args?: { key?: string } }) =>
      cmd === 'session_work_links' ? (x?.args?.key === undefined ? links : []) : base(cmd, x),
    );
  }

  beforeEach(() => {
    sidebarGroupBy.set('project');
    hostFilter.set('all');
    workFilters.set({ ...DEFAULT_WORK_FILTERS, archived: true });
  });

  it('an empty list names the filters that hide it and clears them', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a')]);
    hostFilter.set('mefistos');
    render(Sidebar);
    await tick(); await tick();
    expect(names()).toHaveLength(0);
    expect(screen.getByTestId('sidebar-empty')).toHaveTextContent('No sessions match Host: mefistos');
    expect(screen.getByTestId('facet-host')).toHaveTextContent('Host: mefistos');
    await fireEvent.click(screen.getByTestId('sidebar-empty-clear'));
    await tick();
    expect(names()).toHaveLength(1);
    expect(get(hostFilter)).toBe('all');
  });

  it('a chip in the strip removes its one filter', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a')]);
    hostFilter.set('mefistos');
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('facet-host'));
    await tick();
    expect(screen.queryByTestId('active-filters')).toBeNull();
    expect(names()).toHaveLength(1);
  });

  it('search narrows Other sessions too', async () => {
    mockBackend(fakeProjects, [sessionFor(null, 'loose-alpha'), sessionFor(null, 'loose-beta')]);
    render(Sidebar);
    await tick(); await tick();
    expect(names()).toHaveLength(2);
    await fireEvent.input(screen.getByTestId('sidebar-search'), { target: { value: 'beta' } });
    await waitFor(() => expect(names()).toHaveLength(1));
    expect(names()[0]).toContain('loose-beta');
  });

  it('a work group’s Done follows the host filter, like a past-only group', async () => {
    mockBackend(fakeProjects, [{ ...sessionFor(1, 'dev-a'), tags: ['PAY-7'] }]);
    withPastWork([pastLink('PAY-7', 'mefistos')]);
    sidebarGroupBy.set('work');
    render(Sidebar);
    await screen.findByTestId('work-done');
    hostFilter.set('local');
    await tick(); await tick();
    expect(screen.queryByTestId('work-done')).toBeNull();
    expect(names()).toHaveLength(1);
  });

  it('the selection drops a row a filter hides: bulk actions reach only what you see', async () => {
    const a = sessionFor(1, 'dev-a');
    const b = { ...sessionFor(1, 'dev-b'), host_alias: 'mefistos' };
    mockBackend(fakeProjects, [a, b]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(await selectModeSwitch());
    await tick();
    for (const box of screen.getAllByTestId('select-box')) await fireEvent.click(box);
    await tick();
    expect(screen.getByTestId('bulk-bar')).toHaveTextContent('2 selected');
    hostFilter.set('local');
    await tick(); await tick();
    expect(screen.getByTestId('bulk-bar')).toHaveTextContent('1 selected');
  });

  it('Last active narrows Other sessions too, by each session’s own activity', async () => {
    const now = Math.floor(Date.now() / 1000);
    mockBackend(fakeProjects, [
      { ...sessionFor(null, 'loose-fresh'), last_activity_at: now - 60 },
      { ...sessionFor(null, 'loose-stale'), last_activity_at: now - 40 * 86400 },
    ]);
    localStorage.setItem('cf:pref:recency', '"1d"');
    render(Sidebar);
    await tick(); await tick();
    expect(names()).toHaveLength(1);
    expect(names()[0]).toContain('loose-fresh');
    localStorage.removeItem('cf:pref:recency');
  });

  it('Needs you hides past work: an ended session never waits on you', async () => {
    mockBackend(fakeProjects, [{ ...sessionFor(1, 'dev-stuck'), stuck_kind: 'oom' as const }]);
    withPastWork([pastLink('ABC-9', 'local')]);
    sidebarGroupBy.set('work');
    render(Sidebar);
    await screen.findByTestId('past-work-group');
    await fireEvent.click(await needsYouPill());
    await tick();
    expect(screen.queryByTestId('past-work-group')).toBeNull();
    expect(names()).toHaveLength(1);
  });
});

// ── Multi-user M1 (F2): the bulk paths, and the unclaimed count ─────────────
describe('bulk actions and a shared session', () => {
  /** A paired desktop that knows who it is: person 9, who owns nothing here. */
  function paired(grants: { session_id: number; level: string }[]) {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://fleet.example.com' });
    hubConnection.set({ state: 'connected' });
    setMyGrants(9, grants);
  }
  /** A row owned by somebody else, so the grant map is what decides. */
  const owned = (projectId: number, name: string) => ({
    ...sessionFor(projectId, name),
    visibility: 'private' as const,
    owner_person_id: 1,
  });

  it('bulk kill skips a shared row and says how many it left alone', async () => {
    // Kill is in the spec §4.3 `own` tier: a `drive` grant does not reach it,
    // which is the case this test uses deliberately — a watch-only row would
    // also be skipped by the bulk PROMPT, and then the test would not show
    // that the two actions ask different questions of the same selection.
    const mine = owned(1, 'dev-mine');
    const theirs = owned(1, 'dev-theirs');
    mockBackend(fakeProjects, [mine, theirs]);
    paired([{ session_id: mine.id, level: 'drive' }, { session_id: theirs.id, level: 'drive' }]);
    sessions.set([mine, theirs]);
    render(Sidebar);
    await tick(); await tick();

    const rows = screen.getAllByTestId('sess-row');
    await fireEvent.click(rows[0], { shiftKey: true });
    await fireEvent.click(rows[1], { metaKey: true });
    await tick();
    expect(screen.getByTestId('bulk-bar')).toHaveTextContent('2 selected');
    await fireEvent.click(screen.getByTestId('bulk-kill'));
    await tick();
    // Nothing to kill, and the dialog says so rather than offering a confirm
    // whose click would do nothing.
    expect(screen.getByTestId('bulk-kill-none').textContent).toMatch(/never killed/i);
    await fireEvent.click(screen.getByTestId('confirm-bulk-kill'));
    await tick(); await tick();
    const kills = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter(
      (c) => c[0] === 'kill_session',
    );
    expect(kills).toHaveLength(0);
  });

  it('bulk prompt fans out to the drive rows and leaves a watch-only one out', async () => {
    const driveable = owned(1, 'dev-drive');
    const watchOnly = owned(1, 'dev-watch');
    mockBackend(fakeProjects, [driveable, watchOnly]);
    paired([
      { session_id: driveable.id, level: 'drive' },
      { session_id: watchOnly.id, level: 'watch' },
    ]);
    sessions.set([driveable, watchOnly]);
    render(Sidebar);
    await tick(); await tick();

    await fireEvent.click(await selectModeSwitch());
    await tick();
    for (const box of screen.getAllByTestId('select-box')) await fireEvent.click(box);
    await tick();
    expect(screen.getByTestId('bulk-bar')).toHaveTextContent('2 selected');
    await fireEvent.click(screen.getByTestId('bulk-send'));
    await tick();
    // One rule, two answers: the selection is the same, the targets are not.
    expect(screen.getByTestId('bulk-target-' + driveable.id)).toBeInTheDocument();
    expect(screen.queryByTestId('bulk-target-' + watchOnly.id)).toBeNull();
  });

  it('the bulk paths are untouched when every row is this client’s own', async () => {
    // The single-user shape, pinned so the gate cannot quietly start excluding
    // rows a standalone desktop owns.
    const a = sessionFor(1, 'dev-a');
    const b = sessionFor(1, 'dev-b');
    mockBackend(fakeProjects, [a, b]);
    render(Sidebar);
    await tick(); await tick();
    const rows = screen.getAllByTestId('sess-row');
    await fireEvent.click(rows[0], { shiftKey: true });
    await fireEvent.click(rows[1], { metaKey: true });
    await tick();
    await fireEvent.click(screen.getByTestId('bulk-kill'));
    await tick();
    expect(screen.queryByTestId('bulk-kill-none')).toBeNull();
    expect(screen.queryByTestId('bulk-kill-skipped')).toBeNull();
    await fireEvent.click(screen.getByTestId('confirm-bulk-kill'));
    await tick(); await tick();
    const kills = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter(
      (c) => c[0] === 'kill_session',
    );
    expect(kills).toHaveLength(2);
  });
});

describe('the per-host unclaimed count', () => {
  // A count with no rows and no expand: an unclaimed session is one fleet did
  // not start, and the only thing anyone out of its scope may learn about it is
  // the number (multi-user M1, rule 6).
  const h = (alias: string, unclaimed: number | null | undefined) => ({
    alias,
    ssh_alias: null,
    reachable: true,
    claude_version: null,
    tmux_version: null,
    hidden: false,
    last_pinged_at: 1,
    account_uuid: null,
    provisioned: true,
    transport: 'ssh' as const,
    ...(unclaimed === undefined ? {} : { unclaimed_sessions: unclaimed }),
  });

  it('renders the count, per host, with nothing to click', async () => {
    mockBackend(fakeProjects, []);
    hosts.set([h('mefistos', 2), h('turanga', 1)]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('unclaimed-count').textContent).toContain('Unclaimed (3)');
    const perHost = screen.getByTestId('unclaimed-hosts').textContent ?? '';
    expect(perHost).toContain('mefistos');
    expect(perHost).toContain('turanga');
    // No rows, and nothing that could open any: the section is a label.
    const section = screen.getByTestId('unclaimed-section');
    expect(section.querySelectorAll('[data-testid="sess-row"]')).toHaveLength(0);
    expect(section.querySelectorAll('button')).toHaveLength(0);
  });

  it('renders NOTHING when the hub sends null — not a zero, not a dash', async () => {
    // The normal case on a hub with more than one person (R5-d): a zero would
    // itself be a claim about the host, so the backend serves null and this
    // surface must say nothing at all.
    mockBackend(fakeProjects, []);
    hosts.set([h('mefistos', null), h('turanga', undefined)]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('unclaimed-section')).toBeNull();
    expect(screen.queryByTestId('unclaimed-count')).toBeNull();
  });

  it('a real zero is not rendered either, and the host filter applies', async () => {
    mockBackend(fakeProjects, []);
    hosts.set([h('mefistos', 0)]);
    const first = render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('unclaimed-section')).toBeNull();
    first.unmount();

    hosts.set([h('mefistos', 2), h('turanga', 5)]);
    hostFilter.set('mefistos');
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('unclaimed-count').textContent).toContain('Unclaimed (2)');
  });
});

describe('Sidebar: a mass loss folds into one row (redesign 1.1)', () => {
  function lostOn(host: string, n: number, blocked = true): SessionRow[] {
    return Array.from({ length: n }, (_, i) => ({
      ...sessionFor(1, `${host}-lost-${i}`),
      host_alias: host,
      lost_at: 50,
      claude_session_id: `c-${host}-${i}`,
      claude_status: blocked ? ('blocked' as const) : null,
    }));
  }

  it('shows "12 stopped on trn" instead of twelve rows, and keeps them out of the badge', async () => {
    const waiting = { ...sessionFor(2, 'dev-waiting'), claude_status: 'blocked' as const };
    mockBackend(fakeProjects, [...lostOn('trn', 12), waiting]);
    render(Sidebar);
    await tick(); await tick();
    const fold = screen.getByTestId('lost-fold');
    expect(within(fold).getByTestId('lost-fold-toggle')).toHaveTextContent('12 stopped on trn');
    expect(within(fold).getByTestId('lost-fold-restore')).toHaveTextContent('Restore');
    // Only the live waiting session is in the tree and in the count.
    expect(screen.getAllByTestId('sess-row')).toHaveLength(1);
    expect(await needsYouPill()).toHaveTextContent('Needs you 1');
  });

  it('the fold expands to its rows, so every stopped session is still one click away', async () => {
    mockBackend(fakeProjects, lostOn('trn', 3));
    render(Sidebar);
    await tick(); await tick();
    const toggle = screen.getByTestId('lost-fold-toggle');
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(0);
    await fireEvent.click(toggle);
    await tick();
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    const rows = within(screen.getByTestId('lost-fold')).getAllByTestId('sess-row');
    expect(rows.map((r) => r.textContent)).toEqual(
      expect.arrayContaining([expect.stringContaining('trn-lost-0'), expect.stringContaining('trn-lost-2')]),
    );
  });

  it('two lost rows are not a mass loss and stay where they are', async () => {
    mockBackend(fakeProjects, lostOn('trn', 2));
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('lost-fold')).toBeNull();
    expect(screen.getAllByTestId('sess-row')).toHaveLength(2);
  });

  it('Restore plans with restore_host_sessions, confirms, then restores the fold', async () => {
    const lost = lostOn('trn', 3);
    mockBackend(fakeProjects, lost);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (c: string, x?: unknown) => Promise<unknown>;
    const calls: { dry_run: boolean; session_ids: number[] | null }[] = [];
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (c: string, x?: unknown) => {
      if (c === 'restore_host_sessions') {
        const a = (x as { args: { host_alias: string; dry_run: boolean; session_ids: number[] | null } }).args;
        calls.push({ dry_run: a.dry_run, session_ids: a.session_ids });
        const plan = lost.map((s) => ({
          session_id: s.id,
          tmux_name: s.tmux_name,
          cwd: null,
          claude_session_id: s.claude_session_id,
          friendly_name: null,
          action: 'restore',
          reason: null,
        }));
        return {
          host_alias: a.host_alias,
          dry_run: a.dry_run,
          plan,
          results: a.dry_run ? [] : lost.map((s) => ({ session_id: s.id, tmux_name: s.tmux_name, ok: true, error: null })),
        };
      }
      return base(c, x);
    });
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('lost-fold-restore'));
    const confirm = await screen.findByTestId('lost-fold-confirm');
    expect(screen.getByTestId('confirm-dialog')).toHaveTextContent('trn-lost-1');
    expect(calls).toEqual([{ dry_run: true, session_ids: lost.map((s) => s.id) }]);
    await fireEvent.click(confirm);
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1]).toEqual({ dry_run: false, session_ids: lost.map((s) => s.id) });
    await waitFor(() => expect(get(toasts).some((t) => t.message === 'Restored 3 of 3 sessions on trn.')).toBe(true));
  });
});

describe('Group by state, host or agent (redesign step 3.6)', () => {
  beforeEach(() => sidebarGroupBy.set('project'));

  function fleet() {
    const now = Math.floor(Date.now() / 1000);
    return [
      { ...sessionFor(1, 'dev-a'), claude_status: 'working' as const, last_activity_at: now },
      { ...sessionFor(2, 'dev-b'), host_alias: 'nas', claude_status: 'blocked' as const, last_activity_at: now },
      { ...sessionFor(null, 'dev-orphan'), host_alias: 'nas', agent: 'codex' as const, last_activity_at: now },
    ];
  }

  it('the Group select offers State, Host, Agent and Organisation after Project and Work', async () => {
    mockBackend(fakeProjects, fleet());
    render(Sidebar);
    await tick(); await tick();
    const select = screen.getByTestId('group-select') as HTMLSelectElement;
    expect(Array.from(select.options).map((o) => o.textContent?.trim())).toEqual([
      'Project', 'Work', 'State', 'Host', 'Agent', 'Organisation',
    ]);
    await groupBy('host');
    expect(get(sidebarGroupBy)).toBe('host');
    const isStr = (v: unknown): v is string => typeof v === 'string';
    expect(readPref('sidebar.group', 'unset', isStr)).toBe('host');
    sidebarGroupBy.set('project');
  });

  it('state groups hold the same rows as the project tree, orphans included', async () => {
    mockBackend(fakeProjects, fleet());
    render(Sidebar);
    await tick(); await tick();
    const projectMode = screen.getAllByTestId('sess-row').map((r) => r.dataset.sessionId).sort();
    expect(projectMode).toHaveLength(3);

    sidebarGroupBy.set('state');
    await tick();
    expect(screen.queryAllByTestId('proj-row')).toHaveLength(0);
    expect(screen.queryByTestId('orphan-sessions')).toBeNull();
    const headers = screen.getAllByTestId('flat-group');
    expect(headers.map((h) => h.dataset.group)).toEqual(['state:action_required', 'state:working', 'state:idle']);
    expect(headers[0]).toHaveTextContent('Needs you');
    expect(headers[0]).toHaveTextContent('1');
    expect(screen.getAllByTestId('sess-row').map((r) => r.dataset.sessionId).sort()).toEqual(projectMode);
    sidebarGroupBy.set('project');
  });

  it('host and agent groups, and a header collapses its rows', async () => {
    mockBackend(fakeProjects, fleet());
    render(Sidebar);
    await tick(); await tick();
    sidebarGroupBy.set('host');
    await tick();
    let headers = screen.getAllByTestId('flat-group');
    expect(headers.map((h) => h.textContent?.replace(/\s+/g, ' ').trim())).toEqual(['▾ local 1', '▾ nas 2']);

    await fireEvent.click(headers[1]);
    await tick();
    expect(screen.getAllByTestId('sess-row')).toHaveLength(1);
    expect(headers[1].getAttribute('aria-expanded')).toBe('false');

    sidebarGroupBy.set('agent');
    await tick();
    headers = screen.getAllByTestId('flat-group');
    expect(headers.map((h) => h.dataset.group)).toEqual(['agent:claude', 'agent:codex']);
    expect(screen.getAllByTestId('sess-row')).toHaveLength(3);
    sidebarGroupBy.set('project');
  });
});

describe('List keys (redesign step 3.8)', () => {
  beforeEach(() => sidebarGroupBy.set('project'));

  function three() {
    return [
      { ...sessionFor(1, 'dev-one'), claude_status: 'working' as const },
      { ...sessionFor(1, 'dev-two'), claude_status: 'working' as const },
      { ...sessionFor(1, 'dev-three'), claude_status: 'blocked' as const },
    ];
  }

  it('j and k move between rows, x picks one for a bulk action, Enter opens', async () => {
    const rows = three();
    mockBackend(fakeProjects, rows);
    selectSession(null);
    render(Sidebar);
    await tick(); await tick();
    const els = screen.getAllByTestId('sess-row');
    els[0].focus();
    await fireEvent.keyDown(els[0], { key: 'j' });
    expect(document.activeElement).toBe(els[1]);
    await fireEvent.keyDown(els[1], { key: 'ArrowDown' });
    expect(document.activeElement).toBe(els[2]);
    await fireEvent.keyDown(els[2], { key: 'k' });
    expect(document.activeElement).toBe(els[1]);

    await fireEvent.keyDown(els[1], { key: 'x' });
    await tick();
    expect(screen.getAllByTestId('select-box').filter((b) => (b as HTMLInputElement).checked)).toHaveLength(1);

    await fireEvent.keyDown(document.activeElement!, { key: 'x' });
    await tick();
    await fireEvent.click(await selectModeSwitch());
    await tick();
    const row = screen.getAllByTestId('sess-row')[0];
    await fireEvent.keyDown(row, { key: 'Enter' });
    expect(get(selectedSession)?.id).toBe(rows[0].id);
  });

  it('the next-needs-you chord opens the next row that needs you, and wraps', async () => {
    const rows = three();
    mockBackend(fakeProjects, rows);
    selectSession(null);
    render(Sidebar);
    await tick(); await tick();
    const isMac = /Mac/.test(navigator.platform) || /Macintosh/.test(navigator.userAgent);
    const chord = isMac ? { key: 'n', metaKey: true, altKey: true } : { key: 'n', ctrlKey: true, altKey: true };
    await fireEvent.keyDown(document.body, chord);
    expect(get(selectedSession)?.id).toBe(rows[2].id);
    await fireEvent.keyDown(document.body, chord);
    expect(get(selectedSession)?.id).toBe(rows[2].id);
  });

  it('⌘2 opens the second row on a Mac', async () => {
    const plat = Object.getOwnPropertyDescriptor(Navigator.prototype, 'platform');
    Object.defineProperty(navigator, 'platform', { value: 'MacIntel', configurable: true });
    try {
      const rows = three();
      mockBackend(fakeProjects, rows);
      selectSession(null);
      render(Sidebar);
      await tick(); await tick();
      await fireEvent.keyDown(document.body, { key: '2', metaKey: true });
      expect(get(selectedSession)?.id).toBe(rows[1].id);
      // A digit typed into a field is the field's.
      const input = document.createElement('input');
      document.body.appendChild(input);
      await fireEvent.keyDown(input, { key: '3', metaKey: true });
      expect(get(selectedSession)?.id).toBe(rows[1].id);
      input.remove();
    } finally {
      delete (navigator as unknown as Record<string, unknown>).platform;
      if (plat) Object.defineProperty(Navigator.prototype, 'platform', plat);
    }
  });
});

describe('Inbox (redesign step 3.3)', () => {
  afterEach(() => sidebarView.set('sessions'));

  function fleet() {
    const now = Math.floor(Date.now() / 1000);
    return [
      { ...sessionFor(1, 'dev-busy'), claude_status: 'working' as const, last_activity_at: now },
      { ...sessionFor(2, 'dev-asking'), claude_status: 'blocked' as const, last_activity_at: now },
      { ...sessionFor(null, 'dev-crashed'), claude_status: 'failed' as const, last_activity_at: now },
    ];
  }

  it('lists only what needs you, counts the rest and links to All sessions', async () => {
    mockBackend(fakeProjects, fleet());
    sidebarView.set('inbox');
    render(Sidebar);
    await tick(); await tick();
    const shown = screen.getAllByTestId('sess-row').map((r) => r.textContent ?? '');
    expect(shown).toHaveLength(2);
    expect(shown.some((t) => t.includes('dev-busy'))).toBe(false);
    expect(screen.getByTestId('inbox-head').textContent).toContain('2 need you');
    expect(screen.getByTestId('inbox-rest').textContent).toContain('1 running');
    // Links to review and sessions to tidy stay on the Inbox's attention line.
    expect(screen.getByTestId('attention-line')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('inbox-all-sessions'));
    await tick();
    expect(get(sidebarView)).toBe('sessions');
    expect(screen.getAllByTestId('sess-row')).toHaveLength(3);
  });

  // G3.1: the Main board's Inbox, from the one attention model.
  it('groups by state: Needs you with its missions and "+1 proposed", then Failed', async () => {
    const now = Math.floor(Date.now() / 1000);
    const jev = { ...sessionFor(2, 'dev-maybe'), claude_status: 'idle' as const, turn_outcome: 'asked' as const, last_stop_at: now - 30, last_activity_at: now };
    mockBackend(fakeProjects, [...fleet(), jev]);
    inboxGroupBy.set('state');
    sidebarView.set('inbox');
    render(Sidebar);
    await tick(); await tick();
    waitingMissions.set(
      waitingOf([
        { id: 7, name: 'Hub federation v2', goal: 'g', mode: 'finite', state: 'active', level: 2, plan_version: 1, created_at: 1, updated_at: 1, version: 1, waiting_on: { reason: 'sign_grant', since: now - 60, open_cards: 0 } },
      ]),
    );
    await tick();
    const secs = screen.getAllByTestId('inbox-section');
    expect(secs.map((x) => x.dataset.key)).toEqual(['needs_you', 'failed']);
    const [needs, failed] = secs;
    expect(within(needs).getByTestId('inbox-section-head').textContent).toMatch(/Needs you\s*2\s*\+1 proposed/);
    expect(within(needs).getAllByTestId('sess-row').map((r) => r.textContent ?? '').some((t) => t.includes('dev-asking'))).toBe(true);
    expect(within(needs).getByTestId('mission-wait').textContent).toContain('sign the autonomy grant');
    // Jev's row sits at the foot of Needs you with its pill, out of the count.
    const proposed = within(needs).getByTestId('inbox-proposed');
    expect(within(proposed).getByTestId('sess-row').textContent).toContain('dev-maybe');
    expect(within(proposed).getByTestId('inbox-proposed-by').textContent).toContain('Proposed by Jev');
    expect(within(failed).getByTestId('inbox-section-head').textContent).toMatch(/Failed\s*1/);
    expect(within(failed).getByTestId('sess-row').textContent).toContain('dev-crashed');
    expect(screen.getByTestId('inbox-head').textContent).toContain('3 need you');
    // "Not waiting" sets the reading aside: the row and "+1 proposed" go.
    await fireEvent.click(within(proposed).getByTestId('inbox-proposed-by-change'));
    await tick();
    expect(screen.queryByTestId('inbox-proposed')).toBeNull();
    expect(screen.queryByTestId('inbox-proposed-count')).toBeNull();
    waitingMissions.set([]);
    notWaitingSaid.set(new Set());
  });

  it('"Group: None" lists one queue under no section header', async () => {
    mockBackend(fakeProjects, fleet());
    sidebarView.set('inbox');
    inboxGroupBy.set('state');
    render(Sidebar);
    await tick(); await tick();
    const select = screen.getByTestId('inbox-group-select') as HTMLSelectElement;
    expect(select.value).toBe('state');
    await fireEvent.change(select, { target: { value: 'none' } });
    await tick();
    expect(get(inboxGroupBy)).toBe('none');
    expect(screen.getAllByTestId('inbox-section')).toHaveLength(1);
    expect(screen.queryByTestId('inbox-section-head')).toBeNull();
    expect(screen.getAllByTestId('sess-row')).toHaveLength(2);
    // The Sessions list keeps its own grouping.
    expect(get(sidebarGroupBy)).not.toBe('none');
    inboxGroupBy.set('state');
  });

  it('says what finished today and folds a mass loss into one Restore line', async () => {
    const now = Math.floor(Date.now() / 1000);
    const lost = Array.from({ length: 4 }, (_, i) => ({
      ...sessionFor(1, `trn-lost-${i}`),
      host_alias: 'trn',
      lost_at: now - 100,
      claude_session_id: `c-trn-${i}`,
      claude_status: 'idle' as const,
    }));
    const finished = { ...sessionFor(2, 'dev-finished'), claude_status: 'completed' as const, last_stop_at: now - 5, last_activity_at: now - 5 };
    mockBackend(fakeProjects, [...fleet(), finished, ...lost]);
    sidebarView.set('inbox');
    render(Sidebar);
    await tick(); await tick();
    const rest = screen.getByTestId('inbox-rest').textContent ?? '';
    expect(rest).toContain('1 running');
    expect(rest).toContain('1 completed today');
    // The stopped rows are the Restore line, not "4 paused".
    expect(rest).not.toContain('paused');
    const fold = within(screen.getByTestId('inbox')).getByTestId('lost-fold');
    expect(within(fold).getByTestId('lost-fold-toggle')).toHaveTextContent('4 stopped on trn');
    expect(within(fold).getByTestId('lost-fold-restore')).toHaveTextContent('Restore');
  });

  it('has no Today tab: Today lives in Control since step 9.1', async () => {
    mockBackend(fakeProjects, fleet());
    sidebarView.set('inbox');
    render(Sidebar);
    await tick();
    expect(screen.getByTestId('inbox-title').textContent).toContain('Inbox');
    expect(screen.queryByTestId('inbox-tab-today')).toBeNull();
  });
});

describe('Sidebar accessibility (7.2)', () => {
  it('passes the axe and audit checks; a project row says whether it is open', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a'), sessionFor(1, 'dev-b'), sessionFor(2, 'dev-c')]);
    const { container } = render(Sidebar);
    await tick(); await tick();
    const projRows = await screen.findAllByTestId('proj-row');
    for (const r of projRows) expect(r.getAttribute('aria-expanded')).toBe('true');
    await expectAccessible(container);
  });

  it('lists projects and sessions as a tree whose rows keep their own buttons', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a'), sessionFor(2, 'dev-c')]);
    render(Sidebar);
    await tick(); await tick();
    const tree = await screen.findByRole('tree', { name: 'Projects' });
    const project = within(tree).getAllByTestId('proj-row')[0];
    expect(project.getAttribute('role')).toBe('treeitem');
    // A project's sessions are its group; a session row is a treeitem whose
    // actions a screen reader reaches as buttons, not as hidden children.
    const group = within(tree).getAllByRole('group')[0];
    const row = within(group).getAllByRole('treeitem')[0];
    expect(row.dataset.testid).toBe('sess-row');
    expect(within(row).getAllByRole('button').length).toBeGreaterThan(0);
  });
});

describe('Shared with me (redesign step 5.8)', () => {
  const REMOTE: HubStatus = {
    ...STANDALONE,
    remote: true,
    url: 'https://fleet.example.com',
    configured_url: 'https://fleet.example.com',
  };

  function fleet() {
    const mine = { ...sessionFor(1, 'dev-mine'), owner_person_id: 7 };
    const watched = { ...sessionFor(2, 'dev-watched'), owner_person_id: 9 };
    const driven = { ...sessionFor(null, 'dev-driven'), owner_person_id: 9 };
    return { mine, watched, driven };
  }

  it('the New layout lifts shared sessions into their own group', async () => {
    const { mine, watched, driven } = fleet();
    mockBackend(fakeProjects, [mine, watched, driven]);
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connected' });
    setMyGrants(7, [
      { session_id: watched.id, level: 'watch' },
      { session_id: driven.id, level: 'drive' },
    ]);
    render(Sidebar);
    await tick(); await tick();
    const group = await screen.findByTestId('shared-with-me');
    expect(within(group).getByTestId('shared-with-me-toggle').textContent).toContain('Shared with me (2)');
    const inGroup = within(group).getAllByTestId('sess-row').map((r) => r.textContent ?? '');
    expect(inGroup.some((t) => t.includes('dev-watched'))).toBe(true);
    expect(inGroup.some((t) => t.includes('dev-driven'))).toBe(true);
    // Each row shows once: the tree and Other sessions no longer hold them.
    const all = screen.getAllByTestId('sess-row').map((r) => r.textContent ?? '');
    expect(all.filter((t) => t.includes('dev-watched'))).toHaveLength(1);
    expect(all.filter((t) => t.includes('dev-driven'))).toHaveLength(1);
    expect(all.some((t) => t.includes('dev-mine'))).toBe(true);
    expect(within(group).queryByText(/dev-mine/)).toBeNull();
    await fireEvent.click(within(group).getByTestId('shared-with-me-toggle'));
    expect(within(group).queryAllByTestId('sess-row')).toHaveLength(0);
  });
});

describe('Sidebar rows and the lost fold in the New layout (parity P8, H7, H8)', () => {
  beforeEach(() => {
    sidebarGroupBy.set('project');
    showFriendlyNames.set(true);
  });
  afterEach(() => {
    showFriendlyNames.set(true);
  });

  function lostOn(host: string, n: number): SessionRow[] {
    return Array.from({ length: n }, (_, i) => ({
      ...sessionFor(1, `${host}-lost-${i}`),
      host_alias: host,
      lost_at: 50,
      claude_session_id: `c-${host}-${i}`,
      claude_status: 'blocked' as const,
    }));
  }

  it('New layout: a mass loss folds into "12 stopped on trn" with Restore, and only the live row is in the tree', async () => {
    const waiting = { ...sessionFor(2, 'dev-waiting'), claude_status: 'blocked' as const };
    mockBackend(fakeProjects, [...lostOn('trn', 12), waiting]);
    render(Sidebar);
    await tick(); await tick();
    const fold = screen.getByTestId('lost-fold');
    expect(within(fold).getByTestId('lost-fold-toggle')).toHaveTextContent('12 stopped on trn');
    expect(within(fold).getByTestId('lost-fold-restore')).toHaveTextContent('Restore');
    expect(screen.getAllByTestId('sess-row')).toHaveLength(1);
  });

  it('New layout: the fold expands to its rows', async () => {
    mockBackend(fakeProjects, lostOn('trn', 3));
    render(Sidebar);
    await tick(); await tick();
    const toggle = screen.getByTestId('lost-fold-toggle');
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryAllByTestId('sess-row')).toHaveLength(0);
    await fireEvent.click(toggle);
    await tick();
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    const rows = within(screen.getByTestId('lost-fold')).getAllByTestId('sess-row');
    expect(rows.map((r) => r.textContent)).toEqual(
      expect.arrayContaining([expect.stringContaining('trn-lost-0'), expect.stringContaining('trn-lost-2')]),
    );
  });

  it('New layout: two lost rows are not a mass loss and stay where they are', async () => {
    mockBackend(fakeProjects, lostOn('trn', 2));
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('lost-fold')).toBeNull();
    expect(screen.getAllByTestId('sess-row')).toHaveLength(2);
  });

  it('New layout: Restore on the fold plans, confirms, then restores', async () => {
    const lost = lostOn('trn', 3);
    mockBackend(fakeProjects, lost);
    const base = (mockedInvoke as ReturnType<typeof vi.fn>).getMockImplementation() as (c: string, x?: unknown) => Promise<unknown>;
    const calls: { dry_run: boolean; session_ids: number[] | null }[] = [];
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (c: string, x?: unknown) => {
      if (c === 'restore_host_sessions') {
        const a = (x as { args: { host_alias: string; dry_run: boolean; session_ids: number[] | null } }).args;
        calls.push({ dry_run: a.dry_run, session_ids: a.session_ids });
        return {
          host_alias: a.host_alias,
          dry_run: a.dry_run,
          plan: lost.map((s) => ({
            session_id: s.id,
            tmux_name: s.tmux_name,
            cwd: null,
            claude_session_id: s.claude_session_id,
            friendly_name: null,
            action: 'restore',
            reason: null,
          })),
          results: a.dry_run ? [] : lost.map((s) => ({ session_id: s.id, tmux_name: s.tmux_name, ok: true, error: null })),
        };
      }
      return base(c, x);
    });
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('lost-fold-restore'));
    const confirm = await screen.findByTestId('lost-fold-confirm');
    expect(screen.getByTestId('confirm-dialog')).toHaveTextContent('trn-lost-1');
    expect(calls).toEqual([{ dry_run: true, session_ids: lost.map((s) => s.id) }]);
    await fireEvent.click(confirm);
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1]).toEqual({ dry_run: false, session_ids: lost.map((s) => s.id) });
    await waitFor(() => expect(get(toasts).some((t) => t.message === 'Restored 3 of 3 sessions on trn.')).toBe(true));
  });

  it('New layout: a ghost row stays one line with an unbracketed host badge', async () => {
    const ghost = { ...sessionFor(2, 'dev-ghost'), status: 'ghost', lost_at: 5 };
    mockBackend(fakeProjects, [ghost]);
    render(Sidebar);
    await tick(); await tick();
    const row = screen.getByTestId('sess-row');
    expect(row.querySelector('.sess-lines')).toBeNull();
    expect(row.querySelector('.sess-details')).toBeNull();
    expect(screen.getByTestId('host-badge').textContent).toBe('local');
  });

  it('New layout: the row shows the friendly name with the tmux name secondary', async () => {
    const named = { ...sessionFor(1, 'dev-martin-janci-claude-fleet--fix-login'), friendly_name: 'Fix login' };
    mockBackend(fakeProjects, [named]);
    render(Sidebar);
    await tick(); await tick();
    const row = screen.getByTestId('sess-row');
    expect(row.querySelector('.sess-line1 .sess-name')).toHaveTextContent('Fix login');
    const tmux = screen.getByTestId('sess-tmux-name');
    expect(tmux).toHaveTextContent('dev-martin-janci-claude-fleet--fix-login');
    expect(tmux.closest('[data-testid="sess-details"]')).not.toBeNull();
  });

  it('New layout: the Friendly names switch in ⋯ swaps the row to the tmux name and persists', async () => {
    const named = { ...sessionFor(1, 'dev-martin-janci-claude-fleet--fix-login'), friendly_name: 'Fix login' };
    mockBackend(fakeProjects, [named]);
    render(Sidebar);
    await tick(); await tick();
    await openViewOptions();
    const sw = within(screen.getByTestId('view-options')).getByTestId('friendly-name-toggle');
    expect(sw).toHaveAttribute('aria-checked', 'true');
    await fireEvent.click(sw);
    await tick();
    expect(sw).toHaveAttribute('aria-checked', 'false');
    const row = screen.getByTestId('sess-row');
    expect(row.querySelector('.sess-line1 .sess-name')).toHaveTextContent('dev-martin-janci-claude-fleet--fix-login');
    expect(row).not.toHaveTextContent('Fix login');
    expect(JSON.parse(localStorage.getItem('cf:pref:show-friendly-names')!)).toBe(false);
    await fireEvent.click(sw);
    await tick();
    expect(sw).toHaveAttribute('aria-checked', 'true');
    expect(row.querySelector('.sess-line1 .sess-name')).toHaveTextContent('Fix login');
    expect(JSON.parse(localStorage.getItem('cf:pref:show-friendly-names')!)).toBe(true);
  });

  it('New layout: the Row details switch in ⋯ hides the second row line and persists', async () => {
    mockBackend(fakeProjects, [{ ...sessionFor(1, 'dev-a'), started_at: Math.floor(Date.now() / 1000) - 60 }]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('sess-details')).toBeInTheDocument();
    await openViewOptions();
    const pill = within(screen.getByTestId('view-options')).getByTestId('toggle-row-details');
    expect(pill).toHaveAttribute('aria-checked', 'true');
    await fireEvent.click(pill);
    await tick();
    expect(screen.queryByTestId('sess-details')).toBeNull();
    expect(pill).toHaveAttribute('aria-checked', 'false');
    expect(JSON.parse(localStorage.getItem('cf:pref:rows.details')!)).toBe(false);
    await fireEvent.click(pill);
    await tick();
    expect(screen.getByTestId('sess-details')).toBeInTheDocument();
    expect(pill).toHaveAttribute('aria-checked', 'true');
    expect(JSON.parse(localStorage.getItem('cf:pref:rows.details')!)).toBe(true);
  });
});


// Review round 13: an empty list is only empty when the list could load. A
// hub that cannot be used, or a failed startup load, is said as a failure;
// a quiet Inbox says what is going on and where to go.
describe('Sidebar empty states (review r13)', () => {
  afterEach(() => {
    bootstrapError.set(null);
    hubStatus.set(STANDALONE);
    sidebarView.set('sessions');
  });

  it('a hub that cannot be used is not "No projects yet"', async () => {
    hubStatus.set({ ...STANDALONE, unavailable: 'cannot read the client token (locked)' });
    mockBackend([], []);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('sidebar-empty')).toBeNull();
    expect(screen.getByTestId('sessions-unavailable').textContent).toContain('Not connected to the hub');
  });

  it('a failed startup load says so with Retry, which refreshes', async () => {
    bootstrapError.set({ code: 'E_HUB_UNREACHABLE', message: 'connection refused' });
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    const err = screen.getByTestId('sessions-load-error');
    expect(err.textContent).toContain("Couldn't load sessions");
    expect(screen.getByTestId('sessions-load-error-text').textContent).toBe("Couldn't reach the hub.");
    await fireEvent.click(screen.getByTestId('sessions-load-error-retry'));
    await waitFor(() => expect(screen.queryByTestId('sessions-load-error')).toBeNull());
    expect(get(bootstrapError)).toBeNull();
  });

  it('the Inbox does not say "Nothing needs you" when sessions could not load', async () => {
    bootstrapError.set({ code: 'E_HUB_TIMEOUT', message: 'deadline' });
    mockBackend(fakeProjects, []);
    sidebarView.set('inbox');
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('inbox-head').textContent).not.toContain('Nothing needs you');
    expect(screen.getByTestId('inbox-load-error')).toBeTruthy();
  });

  it('a calm Inbox offers See running and Today', async () => {
    mockBackend(fakeProjects, [sessionFor(1)]);
    sidebarView.set('inbox');
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('inbox-calm').dataset.kind).toBe('calm');
    await fireEvent.click(screen.getByTestId('inbox-calm-running'));
    expect(get(sidebarView)).toBe('sessions');
  });

  it('a first run starts with one host', async () => {
    mockBackend([], []);
    render(Sidebar);
    await tick(); await tick();
    const empty = screen.getByTestId('sidebar-empty');
    expect(empty.dataset.kind).toBe('first');
    expect(screen.getByTestId('sidebar-empty-add-host')).toBeTruthy();
    expect(screen.getByTestId('sidebar-empty-pair')).toBeTruthy();
  });
});

describe('the Sessions board: tabs, agent, organisation, the running cap', () => {
  const REMOTE: HubStatus = {
    ...STANDALONE,
    remote: true,
    url: 'https://fleet.example.com',
    configured_url: 'https://fleet.example.com',
  };
  beforeEach(() => {
    scopeTab.set('all');
    agentFilter.set('any');
    orgs.set([]);
    sidebarGroupBy.set('project');
  });
  afterEach(() => {
    scopeTab.set('all');
    agentFilter.set('any');
    orgs.set([]);
    sidebarGroupBy.set('project');
  });

  function fleet() {
    const mine = { ...sessionFor(1, 'dev-mine'), owner_person_id: 7 };
    const watched = { ...sessionFor(2, 'dev-watched'), owner_person_id: 9 };
    const steered = { ...sessionFor(null, 'dev-steered'), owner_person_id: 11 };
    mockBackend(fakeProjects, [mine, watched, steered]);
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connected' });
    setMyGrants(7, [
      { session_id: watched.id, level: 'watch' },
      { session_id: steered.id, level: 'drive' },
    ]);
  }
  const names = () => screen.queryAllByTestId('sess-row').map((r) => r.textContent ?? '');

  it('All / Mine / Shared with me: counts on All and Shared, each tab narrows the list', async () => {
    fleet();
    render(Sidebar);
    await tick(); await tick();
    const tabs = await screen.findByTestId('scope-tabs');
    expect(within(tabs).getByTestId('scope-tab-all').textContent).toBe('All3');
    expect(within(tabs).getByTestId('scope-tab-mine').textContent).toBe('Mine');
    expect(within(tabs).getByTestId('scope-tab-shared').textContent).toBe('Shared with me2');
    expect(within(tabs).getByTestId('scope-tab-all').getAttribute('aria-selected')).toBe('true');
    expect(names()).toHaveLength(3);

    await fireEvent.click(within(tabs).getByTestId('scope-tab-mine'));
    await tick();
    expect(names().map((t) => t.includes('dev-mine'))).toEqual([true]);
    expect(screen.queryByTestId('shared-with-me')).toBeNull();
    expect(JSON.parse(localStorage.getItem('cf:pref:sessions.scope-tab')!)).toBe('mine');

    await fireEvent.click(within(tabs).getByTestId('scope-tab-shared'));
    await tick();
    const shown = names();
    expect(shown).toHaveLength(2);
    expect(shown.some((t) => t.includes('dev-mine'))).toBe(false);
    expect(screen.getByTestId('shared-with-me')).toBeTruthy();
    // The arrows move between the tabs (wrapping), as every tablist does.
    await fireEvent.keyDown(within(tabs).getByTestId('scope-tab-shared'), { key: 'ArrowRight' });
    await tick();
    expect(get(scopeTab)).toBe('all');
  });

  it('a standalone desktop owns every row, so it shows no tabs and the whole list', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a'), sessionFor(2, 'dev-b')]);
    scopeTab.set('shared');
    render(Sidebar);
    await tick(); await tick();
    expect(screen.queryByTestId('scope-tabs')).toBeNull();
    expect(names()).toHaveLength(2);
  });

  it('a shared row says who shared it and at what level (Watch board); an unknown sharer reads "Shared with you"', async () => {
    fleet();
    orgs.set([{ id: 1, name: 'Acme', members: [{ person_id: 9, name: 'petra', display_name: 'Petra', role: 'member' }] }] as never);
    render(Sidebar);
    await tick(); await tick();
    const group = await screen.findByTestId('shared-with-me');
    const lines = within(group).getAllByTestId('shared-by').map((e) => e.textContent);
    expect(lines.sort()).toEqual(['Petra · Read', 'Shared with you · Steer']);
    // Only shared rows carry the line.
    expect(screen.getAllByTestId('shared-by')).toHaveLength(2);
  });

  it('a share at the answer level is shared too, not left in the tree', async () => {
    const theirs = { ...sessionFor(1, 'dev-answer'), owner_person_id: 9 };
    mockBackend(fakeProjects, [theirs]);
    hubStatus.set(REMOTE);
    hubConnection.set({ state: 'connected' });
    setMyGrants(7, [{ session_id: theirs.id, level: 'answer' }]);
    render(Sidebar);
    await tick(); await tick();
    const group = await screen.findByTestId('shared-with-me');
    expect(within(group).getByTestId('shared-by').textContent).toBe('Shared with you · Answer');
    expect(screen.queryAllByTestId('proj-row')).toHaveLength(0);
  });

  it('Agent: the facet narrows the list to one agent and shows as a chip that clears', async () => {
    const codex = { ...sessionFor(1, 'dev-codex'), agent: 'codex' as const };
    mockBackend(fakeProjects, [sessionFor(1, 'dev-claude'), codex]);
    render(Sidebar);
    await tick(); await tick();
    await openFilters();
    await fireEvent.click(screen.getByTestId('filter-agent-codex'));
    await tick();
    expect(names().map((t) => t.includes('dev-codex'))).toEqual([true]);
    expect(get(agentFilter)).toBe('codex');
    const chip = screen.getAllByText('Agent: Codex');
    expect(chip.length).toBeGreaterThan(0);
    await fireEvent.click(screen.getByTestId('filter-agent-any'));
    await tick();
    expect(names()).toHaveLength(2);
  });

  it('Group by Organisation: one group per org, owner scopes next, Unassigned last', async () => {
    const acme = { ...sessionFor(1, 'dev-acme'), org_id: 3 };
    const owned = sessionFor(2, 'dev-owned');
    const loose = sessionFor(null, 'dev-loose');
    mockBackend(fakeProjects, [loose, owned, acme]);
    orgs.set([{ id: 3, name: 'Acme', color: null }] as never);
    render(Sidebar);
    await tick(); await tick();
    await groupBy('org');
    const groups = screen.getAllByTestId('flat-group');
    expect(groups.map((g) => g.querySelector('.label')?.textContent)).toEqual(['Acme', fakeProjects[1].project.owner, 'Unassigned']);
    expect(screen.getByTestId('flat-groups').dataset.groupBy).toBe('org');
  });

  it('grouped by state, Working shows two rows and "4 more running ›" opens the rest', async () => {
    const rows = Array.from({ length: 6 }, (_, i) => ({
      ...sessionFor(1, `dev-run-${i}`),
      claude_status: 'working' as const,
      last_activity_at: Math.floor(Date.now() / 1000),
    }));
    mockBackend(fakeProjects, rows);
    render(Sidebar);
    await tick(); await tick();
    await groupBy('state');
    expect(names()).toHaveLength(2);
    const more = screen.getByTestId('group-more');
    expect(more.textContent).toBe('4 more running ›');
    await fireEvent.click(more);
    await tick();
    expect(names()).toHaveLength(6);
    expect(screen.queryByTestId('group-more')).toBeNull();
  });
});
