import { fireEvent, render, screen, within, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { readPref } from './prefs';

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
import { sessions, loadSessions, showBgAgents, showRowDetails, sidebarGroupBy, resetTombstonesForTests, type SessionRow } from './sessions';
import { selectedSession, selectSession, selectSessionExplicitly } from './selection';
import { sessionFocus, focusSession } from './session_focus';
import { hosts, loadHosts, hostFilter, resetTombstonesForTests as resetHostTombstones } from './hosts';
import { accounts, loadAccounts } from './accounts';
import { onboardingDismissed } from './onboarding';
import { toasts, clearToasts } from './toasts';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { workFilters, mineItemIds, DEFAULT_WORK_FILTERS } from './work_filters';
import { trackers } from './trackers';

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
      // The row IS a role="button": Space/Enter on it must scroll nothing.
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

  it('the project picker hides the UX agent\'s system project, but the tree still shows its session', async () => {
    // The design: "flagged `system` and hidden from the project picker".
    // Starting an ordinary session in `~/.claude-fleet/operator` — not a
    // repository, and the agent's own working directory — is never what
    // "+ New session" means. The tree is a different question: the operator
    // session is meant to be visible, attachable and restartable there.
    const operatorProject = {
      project: {
        id: 9,
        owner: 'fleet',
        repo: 'operator',
        base_path: '/home/u/.claude-fleet/operator',
        last_session_at: Math.floor(Date.now() / 1000),
        adopted: false,
        system: true,
      },
      worktrees: [],
    };
    mockBackend([...fakeProjects, operatorProject], [sessionFor(9, 'fleet-operator')]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByText('fleet-operator')).toBeInTheDocument();

    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick();
    const listbox = screen.getByRole('listbox');
    expect(listbox.textContent).toContain('claude-fleet');
    expect(listbox.textContent).not.toContain('operator');
  });

  it('the project picker offers Add project, which opens the dialog', async () => {
    mockBackend(fakeProjects, [sessionFor(1)]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick();
    const addRow = screen.getByTestId('add-project-row');
    // Pinned first, above the projects.
    expect(screen.getByRole('listbox').firstElementChild).toBe(addRow);
    await fireEvent.click(addRow);
    await tick();
    expect(screen.getByTestId('add-project-dialog')).toBeInTheDocument();
    // The popover closes behind the dialog.
    expect(screen.queryByTestId('add-project-row')).toBeNull();
  });

  it('Add project is reachable with no projects at all', async () => {
    mockBackend([], []);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick();
    await fireEvent.click(screen.getByTestId('add-project-row'));
    await tick();
    expect(screen.getByTestId('add-project-dialog')).toBeInTheDocument();
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
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick();
    await fireEvent.click(screen.getByTestId('add-project-row'));
    await tick();
    await fireEvent.input(screen.getByTestId('clone-url'), { target: { value: 'newowner/fresh-repo' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await vi.waitFor(() => expect(screen.queryByTestId('add-project-dialog')).toBeNull());
    expect(screen.getByRole('heading', { name: /New session/ }).textContent).toContain('newowner/fresh-repo');
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
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick();
    await fireEvent.click(screen.getByTestId('add-project-row'));
    await tick();
    const chipFor = (alias: string) =>
      Array.from(document.querySelectorAll<HTMLButtonElement>('.host-pick')).find((b) => (b as HTMLElement).dataset.alias === alias)!;
    await fireEvent.click(chipFor('mefistos'));
    await fireEvent.click(screen.getByTestId('add-mode-folder'));
    await fireEvent.click(screen.getByTestId('choose-folder'));
    await vi.waitFor(() => expect((screen.getByTestId('add-create') as HTMLButtonElement).disabled).toBe(false));
    await fireEvent.click(screen.getByTestId('add-create'));
    await vi.waitFor(() => expect(screen.queryByTestId('add-project-dialog')).toBeNull());
    expect(screen.getByRole('heading', { name: /New session/ }).textContent).toContain('me/thing');
    expect(document.querySelector(".host-pick[aria-pressed='true']")?.getAttribute('data-alias')).toBe('local');
  });

  it('a native <dialog> close on NewSessionDialog still closes it (Modal reopen only when the parent declines)', async () => {
    mockBackend(fakeProjects, []);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('new-session-footer'));
    await tick();
    await fireEvent.click(screen.getByText('claude-fleet'));
    await tick();
    const dlg = screen.getByRole('dialog', { name: 'New session' }) as HTMLDialogElement;
    dlg.removeAttribute('open');
    dlg.dispatchEvent(new Event('close'));
    await tick(); await tick();
    expect(screen.queryByRole('dialog', { name: 'New session' })).toBeNull();
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
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
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
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
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
    const pills = document.querySelectorAll('.hosts .pill');
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
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
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
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
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
    await Promise.all([loadProjects(), loadSessions(), loadHosts(), loadAccounts()]);
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
    expect(chips[0]).toHaveTextContent('stuck: press Enter');
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
    expect(screen.getAllByTestId('answer-option').map((o) => o.getAttribute('data-n'))).toEqual(['1', '2']);
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
    const pill = screen.getByTestId('needs-you-filter');
    expect(pill).toHaveTextContent('Needs you (1)');
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
    await fireEvent.click(screen.getByTestId('needs-you-filter'));
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
    const pill = screen.getByTestId('needs-you-filter');
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
    const pill = screen.getByTestId('needs-you-filter');
    expect(pill).toHaveTextContent('Needs you (1)');
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
    const pill = screen.getByTestId('needs-you-filter');
    expect(pill).toHaveTextContent('Needs you (4)');
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
    expect(line1.querySelector('[data-testid="claude-chip"]')).toHaveTextContent('working');
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
    const pill = screen.getByTestId('toggle-row-details');
    expect(pill).toHaveAttribute('aria-pressed', 'true');
    await fireEvent.click(pill);
    await tick();
    expect(screen.queryByTestId('sess-details')).toBeNull();
    expect(pill).toHaveAttribute('aria-pressed', 'false');
    expect(JSON.parse(localStorage.getItem('cf:pref:rows.details')!)).toBe(false);
    // Clicking again re-shows it — only the hide direction is exercised above.
    await fireEvent.click(pill);
    await tick();
    expect(screen.getByTestId('sess-details')).toBeInTheDocument();
    expect(pill).toHaveAttribute('aria-pressed', 'true');
    expect(JSON.parse(localStorage.getItem('cf:pref:rows.details')!)).toBe(true);
  });

  it('shows the details line by default with no stored pref', async () => {
    expect(localStorage.getItem('cf:pref:rows.details')).toBeNull();
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a')]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('sess-details')).toBeInTheDocument();
    expect(screen.getByTestId('toggle-row-details')).toHaveAttribute('aria-pressed', 'true');
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
    expect(chip).toHaveTextContent('inactive');
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

  it('a ghosted external row stays read-only: no Recreate / Dismiss', async () => {
    const ext = { ...sessionFor(null, 'claude-desktop-ghost'), kind: 'external', status: 'ghost', lost_at: 1 };
    mockBackend(fakeProjects, [ext]);
    render(Sidebar);
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('outside-fleet'));
    await tick();
    const section = screen.getByTestId('outside-fleet-section');
    expect(section.querySelectorAll('[data-testid="sess-row"]')).toHaveLength(1);
    expect(section.querySelector('[data-testid="ghost-recreate"]')).toBeNull();
    expect(section.querySelector('[data-testid="ghost-dismiss"]')).toBeNull();
    expect(section.querySelector('.row-actions')).toBeNull();
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

    await fireEvent.click(screen.getByTestId('select-mode'));
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
    expect(empty.textContent).toContain('No projects yet');
  });

  it('standalone mode is untouched', async () => {
    mockBackend([], []);
    render(Sidebar);
    await tick(); await tick();
    const empty = await screen.findByTestId('sidebar-empty');
    expect(empty.textContent).toContain('No projects yet');
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

    await fireEvent.click(screen.getByTestId('group-by-toggle'));
    await tick();

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
  });

  it('no chrome without work to filter', async () => {
    mockBackend(fakeProjects, [sessionFor(1, 'dev-a')]);
    render(Sidebar);
    await tick();
    expect(screen.queryByTestId('work-filters-toggle')).toBeNull();
  });

  it('status chips filter the tree, count on the pill, and persist', async () => {
    const a = { ...sessionFor(1, 'dev-a'), work: w('PAY-1', 1, 'in_progress') };
    const b = { ...sessionFor(1, 'dev-b'), work: w('PAY-2', 2, 'todo') };
    mockBackend(fakeProjects, [a, b]);
    render(Sidebar);
    await tick();
    expect(screen.queryByTestId('work-filters')).toBeNull();
    await fireEvent.click(screen.getByTestId('work-filters-toggle'));
    // Has-session is work mode only; one tracker needs no tracker chips.
    expect(screen.queryByTestId('wf-session-no')).toBeNull();
    expect(screen.queryByTestId('wf-tracker-all')).toBeNull();
    await fireEvent.click(screen.getByTestId('wf-status-todo'));
    await tick();
    expect(names().some((n) => n.includes('dev-b'))).toBe(true);
    expect(names().some((n) => n.includes('dev-a'))).toBe(false);
    expect(screen.getByTestId('work-filters-toggle')).toHaveTextContent('⚑ work (1)');
    const isAny = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null;
    expect(readPref('sidebar.work-filters', {}, isAny)).toMatchObject({ status: 'todo' });
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
    await fireEvent.click(screen.getByTestId('work-filters-toggle'));
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
    await fireEvent.click(screen.getByTestId('work-filters-toggle'));
    await fireEvent.click(screen.getByTestId('wf-mine'));
    await waitFor(() => expect(names()).toHaveLength(1));
    expect(names()[0]).toContain('dev-b');
    expect(mockedInvoke).toHaveBeenCalledWith('work_tickets', { args: { view: 'mine', limit: 200 } });
    // Needs-you on top: nothing of mine needs me.
    await fireEvent.click(screen.getByTestId('needs-you-filter'));
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
    await fireEvent.click(screen.getByTestId('work-filters-toggle'));
    await fireEvent.click(screen.getByTestId('wf-tracker-2'));
    await tick();
    expect(names()).toHaveLength(1);
    expect(names()[0]).toContain('dev-b');
  });

  it('work mode: past only and hide archived act on past work', async () => {
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
    await screen.findByTestId('past-work-group');
    expect(names()).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('work-filters-toggle'));
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
});
