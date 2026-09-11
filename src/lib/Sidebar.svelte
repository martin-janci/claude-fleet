<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { projects, refreshProjects, type ProjectTreeRow } from './projects';
  import {
    sessions,
    loadSessions,
    killSession,
    renameSession,
    setFriendlyName,
    recreateSession,
    peekSession,
    purgeProject,
    showBgAgents,
    sameSession,
    type SessionRow,
  } from './sessions';
  import { describePurge, purgeHostsForProject } from './purge';
  import { type ProjectRow } from './projects';
  import { selectedSession, selectSession } from './selection';
  import { forgetSessionUi, migrateSessionUi } from './session_ui';
  import { readPref, writePref } from './prefs';
  import { theme, cycleTheme } from './theme';
  import NewSessionDialog from './NewSessionDialog.svelte';
  import SettingsDialog from './SettingsDialog.svelte';
  import OnboardingCard from './OnboardingCard.svelte';
  import { hostFilter } from './hosts';
  import { onboardingDismissed } from './onboarding';
  import { hintAnchor } from './hints';
  import {
    buildSessionsByProject,
    buildRelatedCountById,
    sessionVisible,
    sortProjectsBySeverity,
    type SessionPredicate,
  } from './sidebar_index';
  import {
    attentionReason,
    worstSeverityByProject,
  } from './attention';
  import { attentionIdleMinutes } from './notify';
  import { push, pushError } from './toasts';
  import Modal from './Modal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import BulkPromptDialog from './BulkPromptDialog.svelte';
  import TasksPanel from './TasksPanel.svelte';
  import SidebarFilters from './SidebarFilters.svelte';
  import SessionRowItem from './SessionRowItem.svelte';
  import NewBgSessionDialog from './NewBgSessionDialog.svelte';
  import { isRecency, matchesRecency, type Recency } from './session_status';

  let showSettings = $state(false);
  let showTasks = $state(false);

  // Optional collapse handler injected by the parent (App.svelte). When
  // present, a ‹ button appears in the sidebar header so the user can
  // hide the whole sidebar to make room for the terminal.
  let { onCollapse }: { onCollapse?: () => void } = $props();

  let loadError: string | null = $state(null);
  let loading = $state(false);
  // Recency filter persists across app restarts. Default to "all" the first
  // time the user opens the app; otherwise honor whatever pill they last
  // clicked. The setter writes back to localStorage on every change.
  let recency: Recency = $state(readPref('recency', 'all' as Recency, isRecency));
  $effect(() => {
    writePref('recency', recency);
  });
  let search = $state('');
  // `filtered` re-derives the whole project tree on its dependencies; debounce
  // the search term so each keystroke doesn't trigger a full re-derive +
  // re-render. The input stays bound to `search` for instant feedback.
  let searchQuery = $state('');
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const q = search;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      searchQuery = q;
    }, 150);
    return () => clearTimeout(searchTimer);
  });

  // Per-session UI state. Kept here instead of on each row so collapse and
  // rename state survive a sessions store refresh that creates new row
  // objects. The row being renamed is pinned by its full identity (id +
  // host + old name) — a bare tmux_name is ambiguous across hosts, since
  // default names are project-derived and the same name on two hosts is
  // the normal case.
  //
  // `mode` picks what the inline editor changes: double-click edits the
  // display label (`friendly_name`, empty clears it); the row's ✎ action
  // renames the tmux session itself. `original` is the value the editor
  // opened with, so an unchanged commit is a no-op.
  let renaming: {
    id: number;
    host_alias: string;
    tmux_name: string;
    mode: 'label' | 'tmux';
    original: string;
  } | null = $state(null);
  let renameValue = $state('');
  let renameError: string | null = $state(null);
  // The live rename <input> (only one renders at a time). Bound directly so
  // focus targets the right element — a `data-testid` querySelector would
  // pick the first match if a tree row and an orphan row shared a name.
  let renameInput: HTMLInputElement | undefined = $state();
  // Synchronous in-flight guard: commitRename is wired to BOTH Enter and
  // onblur, and Enter blurs the input — without this the rename IPC fires
  // twice (the `!renamingName` check doesn't help: it's still set during the
  // first call's await).
  let committingRename = false;
  let pendingKill: SessionRow | null = $state(null);
  let pendingRecreate: SessionRow | null = $state(null);

  // ── Triage (FE-3 / FE-4) ──
  // "N stuck" counter doubles as a stuck-only filter; "needs attention" is
  // the wider pill (stuck, safe-kill pending/failed, ghost, failed, idle >
  // N min). Both are session-scoped (not persisted): a filter that hides
  // healthy sessions should not survive a restart unnoticed.
  let stuckOnly = $state(false);
  let attentionOnly = $state(false);
  // Coarse clock for the idle rule; a 30 s tick is plenty for a minutes-level
  // threshold and keeps the derived tree from re-running every second.
  let nowSec = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });
  const attentionOpts = $derived({ idleSecs: $attentionIdleMinutes * 60, now: nowSec });
  const rowPredicate = $derived.by((): SessionPredicate => {
    if (stuckOnly) return (s) => s.stuck_kind !== null;
    if (attentionOnly) {
      const opts = attentionOpts;
      return (s) => attentionReason(s, opts) !== null;
    }
    return null;
  });

  // Multi-select for bulk Kill / Send prompt. Rows are toggled with
  // shift/cmd/ctrl-click, or with the checkboxes once select mode is on.
  let selectMode = $state(false);
  let selectedIds: Set<number> = $state(new Set());
  let bulkKillOpen = $state(false);
  let bulkPromptOpen = $state(false);
  const selectedRows = $derived($sessions.filter((s) => selectedIds.has(s.id)));

  function toggleSelected(sess: SessionRow) {
    const next = new Set(selectedIds);
    if (next.has(sess.id)) next.delete(sess.id);
    else next.add(sess.id);
    selectedIds = next;
  }
  function clearSelected() {
    selectedIds = new Set();
  }
  function toggleSelectMode() {
    selectMode = !selectMode;
    if (!selectMode) clearSelected();
  }
  // Drop ids whose rows left the store (killed / reaped) so the bulk bar
  // never counts phantoms.
  $effect(() => {
    const live = new Set($sessions.map((s) => s.id));
    if ([...selectedIds].some((id) => !live.has(id))) {
      selectedIds = new Set([...selectedIds].filter((id) => live.has(id)));
    }
  });

  async function confirmBulkKill() {
    bulkKillOpen = false;
    const targets = selectedRows;
    clearSelected();
    const results = await Promise.allSettled(
      targets.map(async (sess) => {
        const r = await killSession(sess.host_alias, sess.tmux_name);
        if (!r.ok) {
          pushError(r.error, `Kill ${sess.tmux_name} failed`);
          return;
        }
        forgetSessionUi(sess.host_alias, sess.tmux_name);
        const cur = $selectedSession;
        if (cur && sameSession(cur, sess)) selectSession(null);
      }),
    );
    void results;
  }
  // Projects intentionally collapsed by the user. Anything not in this set
  // is open by default — most users have one or two projects and want to
  // see their sessions immediately.
  let collapsed: Set<number> = $state(new Set());

  // Reveal the selected session wherever the selection came from (quick
  // switcher, restore-on-launch, a fresh New-session create, a click): widen
  // the host filter if it hides the session's host, expand its project if
  // collapsed, then scroll its row into view. Keyed on the id so reconcile
  // updates (a new row object every tick) neither re-scroll nor undo a
  // later collapse or re-filter.
  let sidebarEl: HTMLElement | undefined = $state();
  const revealId = $derived($selectedSession?.id ?? null);
  $effect(() => {
    const id = revealId;
    if (id === null) return;
    const host = untrack(() => $selectedSession?.host_alias ?? null);
    const filter = untrack(() => $hostFilter);
    if (host !== null && filter !== 'all' && filter !== host) hostFilter.set('all');
    const pid = untrack(() => $selectedSession?.project_id ?? null);
    if (pid !== null && untrack(() => collapsed.has(pid))) {
      const next = new Set(untrack(() => collapsed));
      next.delete(pid);
      collapsed = next;
    }
    void tick().then(() => {
      const el = sidebarEl?.querySelector<HTMLElement>(`[data-session-id="${id}"]`);
      if (el && typeof el.scrollIntoView === 'function') el.scrollIntoView({ block: 'nearest' });
    });
  });

  // Stores are bootstrapped once by App.svelte's onMount; Sidebar just reads
  // them. (A second bootstrap here would double every startup IPC call.)

  async function onRefresh() {
    loading = true;
    loadError = null;
    const pr = await refreshProjects();
    // Explicit user refresh: bypass the backend's freshness window.
    const sr = await loadSessions({ force: true });
    loading = false;
    if (!pr.ok) {
      loadError = pr.error.message;
      pushError(pr.error, 'Refresh projects failed');
    } else if (!sr.ok) {
      loadError = sr.error.message;
      pushError(sr.error, 'Refresh sessions failed');
    }
  }

  function matchesSearch(p: ProjectTreeRow, q: string): boolean {
    if (!q) return true;
    const needle = q.toLowerCase();
    if (p.project.owner.toLowerCase().includes(needle)) return true;
    if (p.project.repo.toLowerCase().includes(needle)) return true;
    return sessionsForProject(p.project.id).some(
      (s) =>
        s.tmux_name.toLowerCase().includes(needle) ||
        s.host_alias.toLowerCase().includes(needle) ||
        (s.friendly_name?.toLowerCase().includes(needle) ?? false),
    );
  }

  // Sessions under the host / bg filters only (no triage predicate): the
  // counters must keep reporting while a triage filter is active, and the
  // project sort must weigh every visible session, not just the filtered ones.
  const hostVisibleSessions = $derived(
    $sessions.filter((s) => sessionVisible(s, $hostFilter, $showBgAgents)),
  );
  const stuckCount = $derived(hostVisibleSessions.filter((s) => s.stuck_kind !== null).length);
  const attentionCount = $derived.by(() => {
    const opts = attentionOpts;
    return hostVisibleSessions.filter((s) => attentionReason(s, opts) !== null).length;
  });
  const severityByProject = $derived(worstSeverityByProject(hostVisibleSessions));

  // Only show projects that either match the filter directly OR have at least
  // one active session. Without sessions the sidebar would be flooded with
  // every cloned repo on disk — most of which the user isn't working on.
  const filtered = $derived(
    sortProjectsBySeverity(
      $projects.filter(
        (p) =>
          matchesRecency(p, recency) &&
          matchesSearch(p, searchQuery) &&
          sessionsForProject(p.project.id).length > 0,
      ),
      severityByProject,
    ),
  );

  // Repos that appear under more than one owner — only those need the owner
  // prefix in the sidebar label to disambiguate.
  const collidingRepos = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const r of $projects) {
      counts.set(r.project.repo, (counts.get(r.project.repo) ?? 0) + 1);
    }
    return new Set(Array.from(counts.entries()).filter(([, c]) => c > 1).map(([n]) => n));
  });

  // --- Memoised indices (rebuilt once per $sessions change, not per row) ---

  // Map: project_id → sessions filtered by current hostFilter. This derived
  // value is read directly in the template so Svelte tracks it reactively —
  // using a plain function via {@const} doesn't establish the dependency.
  const filteredSessionsByProject = $derived(
    buildSessionsByProject($sessions, $hostFilter, $showBgAgents, rowPredicate),
  );


  // Map: session.id → count of other sessions sharing the same (project, worktree_key)
  const relatedCountById = $derived(buildRelatedCountById($sessions));

  function sessionsForProject(projectId: number): SessionRow[] {
    return filteredSessionsByProject.get(projectId) ?? [];
  }

  function relatedCountFor(sess: SessionRow): number {
    return relatedCountById.get(sess.id) ?? 0;
  }

  // Sessions whose tmux working directory didn't map to any known project.
  const orphanSessions = $derived(
    $sessions.filter(
      (s) => s.project_id === null && sessionVisible(s, $hostFilter, $showBgAgents, rowPredicate),
    ),
  );

  // Picker for the footer "+ New session" — shows ALL projects regardless
  // of the recency filter or search query. The filter is for the live-
  // sessions tree; when starting a new session the user shouldn't be
  // restricted to projects with recent activity.
  const allProjectsSorted = $derived(
    [...$projects].sort((a, b) => {
      const aLabel = (a.project.owner + '/' + a.project.repo).toLowerCase();
      const bLabel = (b.project.owner + '/' + b.project.repo).toLowerCase();
      return aLabel.localeCompare(bLabel);
    }),
  );

  let dialogProject: ProjectTreeRow | null = $state(null);
  let showProjectPicker = $state(false);

  // Onboarding card actions — open the same flows as existing UI.
  const openAddHost = () => { showSettings = true; };
  const openNewSession = () => { showProjectPicker = true; };

  function openNew(p: ProjectTreeRow, e?: Event) {
    e?.stopPropagation();
    dialogProject = p;
    showProjectPicker = false;
  }

  function onCreated(s: SessionRow) {
    dialogProject = null;
    // Auto-focus the just-created session in the center/terminal panes.
    selectSession(s);
  }

  function onCancel() {
    dialogProject = null;
  }

  function toggleCollapse(projectId: number) {
    if (collapsed.has(projectId)) {
      collapsed.delete(projectId);
    } else {
      collapsed.add(projectId);
    }
    // Reassign so Svelte detects the Set mutation.
    collapsed = new Set(collapsed);
  }

  function onSelectSession(sess: SessionRow, e?: MouseEvent) {
    // Shift / cmd / ctrl-click (or select mode) toggles the row in the
    // multi-select instead of opening it.
    if (selectMode || (e && (e.shiftKey || e.metaKey || e.ctrlKey))) {
      toggleSelected(sess);
      return;
    }
    // Stop rename mode if the user clicks away to another row.
    if (renaming !== null && !sameSession(renaming, sess)) {
      cancelRename();
    }
    const cur = $selectedSession;
    if (cur && cur.id === sess.id) {
      selectSession(null);
    } else {
      selectSession(sess);
    }
  }

  function onKeySession(e: KeyboardEvent, sess: SessionRow) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onSelectSession(sess);
    }
  }

  async function beginEdit(sess: SessionRow, mode: 'label' | 'tmux', e?: Event) {
    e?.stopPropagation();
    const original = mode === 'label' ? (sess.friendly_name ?? '') : sess.tmux_name;
    renaming = { id: sess.id, host_alias: sess.host_alias, tmux_name: sess.tmux_name, mode, original };
    renameValue = original;
    renameError = null;
    await tick();
    renameInput?.focus();
    renameInput?.select();
  }

  /** ✎ action: rename the tmux session (new row identity on the backend). */
  function beginRename(sess: SessionRow, e?: Event) {
    return beginEdit(sess, 'tmux', e);
  }

  /** Double-click: edit the display label. */
  function beginLabelEdit(sess: SessionRow, e?: Event) {
    return beginEdit(sess, 'label', e);
  }

  function cancelRename() {
    renaming = null;
    renameValue = '';
    renameError = null;
  }

  async function commitRename() {
    if (committingRename || !renaming) return;
    const next = renameValue.trim();
    if (renaming.mode === 'label') {
      // Empty is meaningful here: it clears the label.
      if (next === renaming.original.trim()) {
        cancelRename();
        return;
      }
      committingRename = true;
      try {
        const r = await setFriendlyName(renaming.host_alias, renaming.tmux_name, next);
        if (!r.ok) {
          renameError = r.error.message;
          pushError(r.error, 'Label update failed');
          return;
        }
        cancelRename();
      } finally {
        committingRename = false;
      }
      return;
    }
    if (!next || next === renaming.tmux_name) {
      cancelRename();
      return;
    }
    committingRename = true;
    try {
      // Target the exact row that was double-clicked — host + old name from
      // the pinned identity, never a lookup by name alone.
      const target = renaming;
      const { host_alias: hostAlias, tmux_name: oldName } = target;
      const r = await renameSession(hostAlias, oldName, next);
      if (!r.ok) {
        renameError = r.error.message;
        pushError(r.error, 'Rename failed');
        return;
      }
      // Persisted UI state (pane widths, collapsed) is keyed by tmux name;
      // bring it along to the new name so the user's layout sticks.
      migrateSessionUi(r.value.host_alias, oldName, r.value.tmux_name);
      // If the renamed session was the selected one, follow the rename.
      const cur = $selectedSession;
      if (cur && sameSession(cur, target)) {
        selectSession(r.value);
      }
      cancelRename();
    } finally {
      committingRename = false;
    }
  }

  function onRenameKey(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      void commitRename();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancelRename();
    }
  }

  function askKill(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    pendingKill = sess;
  }

  async function confirmKill() {
    if (!pendingKill) return;
    const sess = pendingKill;
    pendingKill = null;
    const r = await killSession(sess.host_alias, sess.tmux_name);
    if (!r.ok) {
      pushError(r.error, 'Kill failed');
      return;
    }
    // Drop persisted layout for the now-dead session — otherwise localStorage
    // grows unbounded over time. (User can still get a fresh layout if they
    // make a session with the same name later; that's intentional.)
    forgetSessionUi(sess.host_alias, sess.tmux_name);
    // If we just killed the selected session, drop the selection so the
    // terminal pane shows the empty state instead of trying to attach.
    const cur = $selectedSession;
    if (cur && sameSession(cur, sess)) {
      selectSession(null);
    }
  }

  function cancelKill() {
    pendingKill = null;
  }

  function askRecreate(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    pendingRecreate = sess;
  }

  function cancelRecreate() {
    pendingRecreate = null;
  }

  async function confirmRecreate() {
    if (!pendingRecreate) return;
    const sess = pendingRecreate;
    pendingRecreate = null;
    const r = await recreateSession(sess.id);
    if (!r.ok) {
      pushError(r.error, 'Recreate failed');
      return;
    }
    // kill-session severed the PTY; the tmux_name is unchanged so TerminalView
    // won't auto-reopen. Force a re-attach when this session is selected by
    // dropping and (after the close effect runs) restoring the selection.
    const cur = $selectedSession;
    if (cur && sameSession(cur, sess)) {
      selectSession(null);
      await tick();
      selectSession(r.value);
    }
  }

  // Per-session peek panel state: row id → log text | "loading" | null
  let peekState = $state<Record<number, string | "loading" | null>>({});

  async function doPeek(sess: SessionRow) {
    if (!sess.claude_session_id) return;
    peekState[sess.id] = "loading";
    try {
      const result = await peekSession(sess.host_alias, sess.claude_session_id);
      if (result.ok) {
        peekState[sess.id] = result.value || "(no output yet)";
      } else {
        peekState[sess.id] = "Error: " + result.error.message;
      }
    } catch (e: unknown) {
      peekState[sess.id] = "Error: " + (e instanceof Error ? e.message : String(e));
    }
  }

  function closePeek(sessId: number) {
    peekState[sessId] = null;
  }

  // --- New BG Session modal ---
  let showBgModal = $state(false);
  let bgModalHost = $state('local');
  let bgModalName = $state('');
  let bgModalPrompt = $state('');
  let bgModalError = $state<string | null>(null);
  let bgModalLoading = $state(false);

  // --- Purge Project ---
  let pendingPurge: ProjectRow | null = $state(null);

  async function confirmPurge() {
    if (!pendingPurge) return;
    const project = pendingPurge;
    pendingPurge = null;
    // Projects carry no host; purge on every host the project's sessions ran
    // on, in one call. The backend keeps the row unless every host succeeds.
    const result = await purgeProject(
      purgeHostsForProject(project.id, $sessions),
      project.base_path,
      project.id,
    );
    if (!result.ok) {
      pushError(result.error, 'Purge failed; project kept');
    } else {
      push(describePurge(result.value));
      // Refresh stores since the backend doesn't emit row-level events for project deletion
      await loadSessions();
      await refreshProjects();
    }
  }

  function cancelPurge() {
    pendingPurge = null;
  }
</script>

<div class="sidebar" data-testid="sidebar-tree" bind:this={sidebarEl}>
  {#snippet sessionRow(sess: SessionRow)}
    <SessionRowItem
      {sess}
      {selectMode}
      isChecked={selectedIds.has(sess.id)}
      isRenaming={renaming !== null && renaming.id === sess.id}
      renameMode={renaming !== null && renaming.id === sess.id ? renaming.mode : null}
      bind:renameValue
      bind:renameInput
      {renameError}
      relatedCount={relatedCountFor(sess)}
      {nowSec}
      peek={peekState[sess.id]}
      {onSelectSession}
      {onKeySession}
      {toggleSelected}
      {beginRename}
      {beginLabelEdit}
      {onRenameKey}
      {commitRename}
      {askRecreate}
      {askKill}
      {doPeek}
      {closePeek}
    />
  {/snippet}

  <SidebarFilters
    bind:search
    bind:recency
    bind:stuckOnly
    bind:attentionOnly
    {loading}
    {loadError}
    {onRefresh}
    {onCollapse}
    {showTasks}
    {showSettings}
    onOpenTasks={() => (showTasks = true)}
    onOpenSettings={() => (showSettings = true)}
    {stuckCount}
    {attentionCount}
    {selectMode}
    {toggleSelectMode}
    selectedCount={selectedIds.size}
    onBulkSend={() => (bulkPromptOpen = true)}
    onBulkKill={() => (bulkKillOpen = true)}
    {clearSelected}
  />

  <div class="scroller">
    {#if !$onboardingDismissed}
      <OnboardingCard onaddhost={openAddHost} onnewsession={openNewSession} />
    {/if}
    {#if filtered.length > 0}
      <ul class="tree">
        {#each filtered as row (row.project.id)}
          {@const projectSessions = filteredSessionsByProject.get(row.project.id) ?? []}
          {@const isCollapsed = collapsed.has(row.project.id)}
          <li class="proj">
            <div
              class="proj-row"
              data-testid="proj-row"
              title={row.project.base_path}
              role="button"
              tabindex="0"
              onclick={() => toggleCollapse(row.project.id)}
              onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && toggleCollapse(row.project.id)}
            >
              <span class="caret" class:collapsed={isCollapsed}>▾</span>
              <span class="label">
                {#if collidingRepos.has(row.project.repo)}<span class="owner"
                    >{row.project.owner}/</span
                  >{/if}<span class="repo">{row.project.repo}</span>
              </span>
              <span class="count">{projectSessions.length}</span>
              <button
                class="icon-btn"
                onclick={(e) => openNew(row, e)}
                title="New session in this project"
                aria-label="New session"
              >
                +
              </button>
              <button
                class="icon-btn small purge-btn"
                title="Purge Claude Code project state (irreversible)"
                onclick={(e) => { e.stopPropagation(); pendingPurge = row.project; }}
                data-testid="purge-project"
                aria-label="Purge project"
              >🗑️</button>
            </div>

            {#if !isCollapsed}
              {#each projectSessions as sess (sess.id)}
                {@render sessionRow(sess)}
              {/each}
            {/if}
          </li>
        {/each}
      </ul>
    {:else if !loadError && orphanSessions.length === 0}
      <p class="empty">
        {$projects.length === 0
          ? 'No projects yet. Set a projects base in Settings → Projects, or click ↻ to scan.'
          : 'No active sessions. Click + below to start one.'}
      </p>
    {/if}

    {#if orphanSessions.length > 0}
      <div class="orphan-section" data-testid="orphan-sessions">
        <div class="section-header">Other sessions ({orphanSessions.length})</div>
        {#each orphanSessions as sess (sess.id)}
          {@render sessionRow(sess)}
        {/each}
      </div>
    {/if}
  </div>

  <footer class="sidebar-footer" data-testid="sidebar-chrome-bottom">
    <div class="footer-row">
      <button
        class="new-btn"
        onclick={() => (showProjectPicker = !showProjectPicker)}
        data-testid="new-session-footer"
      >
        + New session
      </button>
      <button
        class="icon-btn"
        title="Launch a supervised Claude background session"
        onclick={() => (showBgModal = true)}
        data-testid="new-bg-session-btn"
        use:hintAnchor={{ id: 'bg-session', when: $sessions.some((s) => s.kind !== 'bg') && !$sessions.some((s) => s.kind === 'bg') }}
      >⚡</button>
    </div>
    <button
      class="theme-toggle"
      onclick={cycleTheme}
      title="Theme: {$theme} (click to cycle auto/light/dark)"
      data-testid="theme-toggle"
    >
      theme: {$theme}
    </button>
    {#if showProjectPicker}
      <div class="picker" role="listbox" aria-label="Pick project for new session">
        {#each allProjectsSorted as row (row.project.id)}
          <button class="picker-item" onclick={() => openNew(row)}>
            {#if collidingRepos.has(row.project.repo)}<span class="owner"
                >{row.project.owner}/</span
              >{/if}{row.project.repo}
          </button>
        {/each}
        {#if allProjectsSorted.length === 0}
          <p class="empty pad">No projects. Refresh first.</p>
        {/if}
      </div>
    {/if}
  </footer>
</div>

<!-- The project picker is a popover, not a modal; the modals below handle
     their own Escape through <dialog>'s cancel event. -->
<svelte:window onkeydown={(e) => {
  if (e.key === 'Escape' && showProjectPicker) showProjectPicker = false;
}} />

{#if dialogProject}
  <NewSessionDialog project={dialogProject} onCreate={onCreated} {onCancel} />
{/if}

{#if pendingKill}
  <ConfirmDialog
    title="Kill session?"
    confirmLabel="Kill"
    danger
    onconfirm={confirmKill}
    oncancel={cancelKill}
    confirmTestId="confirm-kill"
  >
    This will kill the tmux session <code>{pendingKill.tmux_name}</code> on
    <code>{pendingKill.host_alias}</code> and lose any running claude state inside it. Continue?
  </ConfirmDialog>
{/if}

{#if bulkKillOpen}
  <ConfirmDialog
    title="Kill {selectedRows.length} session{selectedRows.length === 1 ? '' : 's'}?"
    confirmLabel="Kill all"
    danger
    onconfirm={confirmBulkKill}
    oncancel={() => (bulkKillOpen = false)}
    confirmTestId="confirm-bulk-kill"
  >
    This will kill
    {#each selectedRows as r, i (r.id)}{i > 0 ? ', ' : ''}<code>{r.tmux_name}</code> on <code>{r.host_alias}</code>{/each}
    and lose any running claude state inside them. Continue?
  </ConfirmDialog>
{/if}

{#if bulkPromptOpen}
  <BulkPromptDialog targets={selectedRows} onClose={() => (bulkPromptOpen = false)} />
{/if}

{#if pendingRecreate}
  <ConfirmDialog
    title="Recreate session?"
    confirmLabel="Recreate"
    danger
    onconfirm={confirmRecreate}
    oncancel={cancelRecreate}
    confirmTestId="confirm-recreate"
  >
    This kills the tmux session <code>{pendingRecreate.tmux_name}</code> on
    <code>{pendingRecreate.host_alias}</code> and the running claude state inside it,
    then starts a fresh session in the same worktree. Continue?
  </ConfirmDialog>
{/if}

{#if showSettings}
  <SettingsDialog onClose={() => (showSettings = false)} />
{/if}

{#if showTasks}
  <Modal title="Tasks" onclose={() => (showTasks = false)} width="640px" testid="tasks-dialog">
    <TasksPanel />
  </Modal>
{/if}

{#if pendingPurge}
  <ConfirmDialog
    title="Purge project?"
    confirmLabel="Purge"
    danger
    onconfirm={confirmPurge}
    oncancel={cancelPurge}
    confirmTestId="confirm-purge"
  >
    This will permanently delete all Claude Code state for <code>{pendingPurge.repo}</code>. This is irreversible.
  </ConfirmDialog>
{/if}

{#if showBgModal}
  <NewBgSessionDialog
    bind:bgModalHost
    bind:bgModalName
    bind:bgModalPrompt
    bind:bgModalError
    bind:bgModalLoading
    onClose={() => (showBgModal = false)}
  />
{/if}

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    /* The pane host (App.svelte) renders us inside a full-bleed Pane with
       no padding, so we add our own — and keep header/footer pinned via
       flex layout (header = flex:0, scroller = flex:1, footer = flex:0).
       That way search/filter and theme/new-session are always visible no
       matter how long the project list grows. */
  }

  .icon-btn {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    padding: 0.25rem 0.5rem;
    border-radius: 5px;
    font-size: 0.9rem;
    line-height: 1;
    cursor: pointer;
    min-width: 1.6rem;
  }
  .icon-btn:hover:not(:disabled) {
    color: var(--fg);
    border-color: var(--accent);
    background: var(--bg-pane);
  }
  .icon-btn:disabled { opacity: 0.6; cursor: progress; }
  .icon-btn.small {
    padding: 0.1rem 0.35rem;
    font-size: 0.85rem;
    min-width: 1.4rem;
    border-color: transparent;
  }
  .icon-btn.small:hover { border-color: var(--border); }

  .scroller {
    flex: 1 1 auto;
    overflow: auto;
    min-height: 0;
    padding: 0.4rem 0.6rem;
  }

  .tree { list-style: none; margin: 0; padding: 0; }
  .proj { margin-bottom: 0.15rem; }

  .proj-row {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-weight: 500;
    font-size: 0.85rem;
    padding: 0.3rem 0.4rem;
    border-radius: 4px;
    cursor: pointer;
    user-select: none;
  }
  .proj-row:hover { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .caret {
    color: var(--fg-muted);
    font-size: 0.65rem;
    width: 0.7rem;
    text-align: center;
    transition: transform 0.1s ease;
    display: inline-block;
  }
  .caret.collapsed { transform: rotate(-90deg); }
  .proj-row .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .owner { color: var(--fg-muted); font-weight: 400; }
  .repo { color: var(--fg); }
  .count {
    font-size: 0.7rem;
    color: var(--fg-muted);
    padding: 0.05rem 0.4rem;
    border-radius: 999px;
    background: color-mix(in srgb, var(--fg) 10%, transparent);
    min-width: 1.4rem;
    text-align: center;
  }

  .empty { color: var(--fg-muted); font-size: 0.85rem; padding: 0.5rem 0.4rem; }
  .pad { padding: 0.5rem 0.6rem; }

  .orphan-section {
    border-top: 1px solid var(--border);
    padding-top: 0.35rem;
    margin-top: 0.35rem;
  }
  .section-header {
    font-size: 0.65rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
    padding: 0 0 0.2rem 0.4rem;
  }

  .sidebar-footer {
    flex: 0 0 auto;
    border-top: 1px solid var(--border);
    padding: 0.4rem 0.6rem 0.5rem;
    position: relative;
    background: var(--bg-pane);
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .footer-row {
    display: flex;
    gap: 0.3rem;
    align-items: center;
  }
  .new-btn {
    flex: 1;
    text-align: left;
    font-size: 0.85rem;
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 5px;
    cursor: pointer;
  }
  .new-btn:hover { border-color: var(--accent); background: var(--bg-pane); }

  .purge-btn {
    opacity: 0;
    transition: opacity 0.15s;
    color: var(--color-error, #f44336);
  }
  .proj-row:hover .purge-btn {
    opacity: 0.6;
  }
  .purge-btn:hover { opacity: 1 !important; }
  .theme-toggle {
    width: 100%;
    text-align: left;
    font-size: 0.75rem;
    padding: 0.25rem 0.5rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 4px;
    cursor: pointer;
  }
  .theme-toggle:hover { color: var(--fg); border-color: var(--accent); }

  .picker {
    position: absolute;
    bottom: 100%;
    left: 0;
    right: 0;
    margin-bottom: 0.3rem;
    border: 1px solid var(--border);
    background: var(--bg);
    border-radius: 5px;
    box-shadow: 0 4px 16px rgba(0,0,0,0.3);
    max-height: 240px;
    overflow: auto;
    z-index: 5;
  }
  .picker-item {
    display: block;
    width: 100%;
    text-align: left;
    border: none;
    background: transparent;
    color: var(--fg);
    font-size: 0.85rem;
    padding: 0.4rem 0.6rem;
    cursor: pointer;
  }
  .picker-item:hover { background: var(--bg-pane); }
</style>
