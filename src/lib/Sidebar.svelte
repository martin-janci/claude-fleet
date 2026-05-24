<script lang="ts">
  import { tick } from 'svelte';
  import { projects, refreshProjects, type ProjectTreeRow } from './projects';
  import {
    sessions,
    loadSessions,
    killSession,
    renameSession,
    restartSession,
    recreateSession,
    dismissGhostSession,
    peekSession,
    newBgSession,
    purgeProject,
    showBgAgents,
    showFriendlyNames,
    type SessionRow,
  } from './sessions';
  import { type ProjectRow } from './projects';
  import { selectedSession, selectSession } from './selection';
  import { forgetSessionUi, migrateSessionUi } from './session_ui';
  import { readPref, writePref } from './prefs';
  import { theme, cycleTheme } from './theme';
  import NewSessionDialog from './NewSessionDialog.svelte';
  import SettingsDialog from './SettingsDialog.svelte';
  import OnboardingCard from './OnboardingCard.svelte';
  import { hosts, hostFilter, hostByAlias } from './hosts';
  import { onboardingDismissed } from './onboarding';
  import { accounts, type AccountRow } from './accounts';

  let showSettings = $state(false);

  // Optional collapse handler injected by the parent (App.svelte). When
  // present, a ‹ button appears in the sidebar header so the user can
  // hide the whole sidebar to make room for the terminal.
  let { onCollapse }: { onCollapse?: () => void } = $props();

  type Recency = 'all' | '8h' | '1d' | '3d' | '7d' | '30d';
  const RECENCY_VALUES: readonly Recency[] = ['all', '8h', '1d', '3d', '7d', '30d'];
  function isRecency(v: unknown): v is Recency {
    return typeof v === 'string' && (RECENCY_VALUES as readonly string[]).includes(v);
  }

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
  // objects (the underlying tmux_name is the stable id).
  let renamingName: string | null = $state(null);
  let renameValue = $state('');
  let renameError: string | null = $state(null);
  // The live rename <input> (only one renders at a time). Bound directly so
  // focus targets the right element — a `data-testid` querySelector would
  // pick the first match if a tree row and an orphan row shared a name.
  let renameInput: HTMLInputElement | undefined;
  // Synchronous in-flight guard: commitRename is wired to BOTH Enter and
  // onblur, and Enter blurs the input — without this the rename IPC fires
  // twice (the `!renamingName` check doesn't help: it's still set during the
  // first call's await).
  let committingRename = false;
  let pendingKill: SessionRow | null = $state(null);
  let pendingRecreate: SessionRow | null = $state(null);
  // Projects intentionally collapsed by the user. Anything not in this set
  // is open by default — most users have one or two projects and want to
  // see their sessions immediately.
  let collapsed: Set<number> = $state(new Set());
  let actionError: string | null = $state(null);

  // Stores are bootstrapped once by App.svelte's onMount; Sidebar just reads
  // them. (A second bootstrap here would double every startup IPC call.)

  async function onRefresh() {
    loading = true;
    loadError = null;
    const pr = await refreshProjects();
    const sr = await loadSessions();
    loading = false;
    if (!pr.ok) loadError = pr.error.message;
    else if (!sr.ok) loadError = sr.error.message;
  }

  const RECENCY_WINDOW: Record<Recency, number | null> = {
    all: null,
    '8h': 60 * 60 * 8,
    '1d': 60 * 60 * 24,
    '3d': 60 * 60 * 24 * 3,
    '7d': 60 * 60 * 24 * 7,
    '30d': 60 * 60 * 24 * 30,
  };

  function matchesRecency(p: ProjectTreeRow, r: Recency): boolean {
    const window = RECENCY_WINDOW[r];
    if (window === null) return true;
    if (p.project.last_session_at === null) return false;
    const ageSec = Math.floor(Date.now() / 1000) - p.project.last_session_at;
    return ageSec >= 0 && ageSec <= window;
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

  // Only show projects that either match the filter directly OR have at least
  // one active session. Without sessions the sidebar would be flooded with
  // every cloned repo on disk — most of which the user isn't working on.
  const filtered = $derived(
    $projects.filter(
      (p) =>
        matchesRecency(p, recency) &&
        matchesSearch(p, searchQuery) &&
        sessionsForProject(p.project.id).length > 0,
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

  // Lookup map for tooltips + components that resolve a host's account.
  const accountByUuid = $derived(
    new Map<string, AccountRow>($accounts.map((a) => [a.uuid, a])),
  );

  function accountLabel(host: { account_uuid: string | null }): string {
    if (!host.account_uuid) return '';
    const acc = accountByUuid.get(host.account_uuid);
    if (!acc) return `\n${host.account_uuid}`;
    const email = acc.email ?? acc.uuid;
    return acc.seat_tier ? `\n${email} (${acc.seat_tier})` : `\n${email}`;
  }

  // --- Memoised indices (rebuilt once per $sessions change, not per row) ---

  // Map: project_id → sessions filtered by current hostFilter. This derived
  // value is read directly in the template so Svelte tracks it reactively —
  // using a plain function via {@const} doesn't establish the dependency.
  const filteredSessionsByProject = $derived.by(() => {
    const m = new Map<number, SessionRow[]>();
    for (const s of $sessions) {
      if (s.project_id == null) continue;
      if ($hostFilter !== 'all' && s.host_alias !== $hostFilter) continue;
      if (!$showBgAgents && s.kind === 'bg') continue;
      if (!m.has(s.project_id)) m.set(s.project_id, []);
      m.get(s.project_id)!.push(s);
    }
    return m;
  });

  // Map: session.id → count of other sessions sharing the same (project, worktree_key)
  const relatedCountById = $derived.by(() => {
    const grouped = new Map<string, SessionRow[]>();
    for (const s of $sessions) {
      if (s.project_id == null || s.worktree_key == null) continue;
      const key = `${s.project_id}:${s.worktree_key}`;
      if (!grouped.has(key)) grouped.set(key, []);
      grouped.get(key)!.push(s);
    }
    const out = new Map<number, number>();
    for (const list of grouped.values()) {
      for (const s of list) out.set(s.id, list.length - 1);
    }
    return out;
  });

  function sessionsForProject(projectId: number): SessionRow[] {
    return filteredSessionsByProject.get(projectId) ?? [];
  }

  function relatedCountFor(sess: SessionRow): number {
    return relatedCountById.get(sess.id) ?? 0;
  }

  // Sessions whose tmux working directory didn't map to any known project.
  const orphanSessions = $derived(
    $sessions.filter(
      (s) =>
        s.project_id === null &&
        ($hostFilter === 'all' || s.host_alias === $hostFilter) &&
        ($showBgAgents || s.kind !== 'bg'),
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

  function onSelectSession(sess: SessionRow) {
    // Stop rename mode if the user clicks away to another row.
    if (renamingName !== null && renamingName !== sess.tmux_name) {
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

  async function beginRename(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    renamingName = sess.tmux_name;
    renameValue = sess.tmux_name;
    renameError = null;
    await tick();
    renameInput?.focus();
    renameInput?.select();
  }

  function cancelRename() {
    renamingName = null;
    renameValue = '';
    renameError = null;
  }

  async function commitRename() {
    if (committingRename || !renamingName) return;
    const next = renameValue.trim();
    if (!next || next === renamingName) {
      cancelRename();
      return;
    }
    committingRename = true;
    try {
      const oldName = renamingName;
      const sess = $sessions.find((s) => s.tmux_name === oldName);
      const hostAlias = sess?.host_alias ?? 'local';
      const r = await renameSession(hostAlias, oldName, next);
      if (!r.ok) {
        renameError = r.error.message;
        return;
      }
      // Persisted UI state (pane widths, collapsed) is keyed by tmux name;
      // bring it along to the new name so the user's layout sticks.
      migrateSessionUi(r.value.host_alias, oldName, r.value.tmux_name);
      // If the renamed session was the selected one, follow the rename.
      const cur = $selectedSession;
      if (cur && cur.tmux_name === oldName) {
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

  async function doRestart(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    actionError = null;
    const r = await restartSession(sess.host_alias, sess.tmux_name);
    if (!r.ok) actionError = r.error.message;
  }

  function askKill(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    pendingKill = sess;
    actionError = null;
  }

  async function confirmKill() {
    if (!pendingKill) return;
    const sess = pendingKill;
    pendingKill = null;
    const r = await killSession(sess.host_alias, sess.tmux_name);
    if (!r.ok) {
      actionError = r.error.message;
      return;
    }
    // Drop persisted layout for the now-dead session — otherwise localStorage
    // grows unbounded over time. (User can still get a fresh layout if they
    // make a session with the same name later; that's intentional.)
    forgetSessionUi(sess.host_alias, sess.tmux_name);
    // If we just killed the selected session, drop the selection so the
    // terminal pane shows the empty state instead of trying to attach.
    const cur = $selectedSession;
    if (cur && cur.tmux_name === sess.tmux_name) {
      selectSession(null);
    }
  }

  function cancelKill() {
    pendingKill = null;
  }

  function askRecreate(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    pendingRecreate = sess;
    actionError = null;
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
      actionError = r.error.message;
      return;
    }
    // kill-session severed the PTY; the tmux_name is unchanged so TerminalView
    // won't auto-reopen. Force a re-attach when this session is selected by
    // dropping and (after the close effect runs) restoring the selection.
    const cur = $selectedSession;
    if (cur && cur.tmux_name === sess.tmux_name) {
      selectSession(null);
      await tick();
      selectSession(r.value);
    }
  }

  async function doRecreate(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    actionError = null;
    const r = await recreateSession(sess.id);
    if (!r.ok) actionError = r.error.message;
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

  async function doDismissGhost(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    actionError = null;
    const r = await dismissGhostSession(sess.id);
    if (!r.ok) {
      actionError = r.error.message;
      return;
    }
    forgetSessionUi(sess.host_alias, sess.tmux_name);
  }

  function hostIsReachable(alias: string): boolean {
    return $hostByAlias.get(alias)?.reachable ?? false;
  }

  // --- New BG Session modal ---
  let showBgModal = $state(false);
  let bgModalHost = $state('local');
  let bgModalName = $state('');
  let bgModalPrompt = $state('');
  let bgModalError = $state<string | null>(null);
  let bgModalLoading = $state(false);

  async function doNewBgSession() {
    bgModalError = null;
    bgModalLoading = true;
    try {
      const result = await newBgSession(bgModalHost, bgModalName, bgModalPrompt);
      if (result.ok) {
        showBgModal = false;
        bgModalName = '';
        bgModalPrompt = '';
      } else {
        bgModalError = result.error.message;
      }
    } catch (e: unknown) {
      bgModalError = e instanceof Error ? e.message : String(e);
    } finally {
      bgModalLoading = false;
    }
  }

  // --- Purge Project ---
  let pendingPurge: ProjectRow | null = $state(null);

  async function confirmPurge() {
    if (!pendingPurge) return;
    const project = pendingPurge;
    pendingPurge = null;
    const result = await purgeProject('local', project.base_path, project.id);
    if (!result.ok) {
      actionError = 'Purge failed: ' + result.error.message;
    } else {
      // Refresh stores since the backend doesn't emit row-level events for project deletion
      await loadSessions();
      await refreshProjects();
    }
  }

  function cancelPurge() {
    pendingPurge = null;
  }

  function timeAgo(unixSecs: number): string {
    const diffMs = Date.now() - unixSecs * 1000;
    const diffMins = Math.floor(diffMs / 60_000);
    if (diffMins < 1) return 'just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    const diffHours = Math.floor(diffMins / 60);
    if (diffHours < 24) return `${diffHours}h ago`;
    const diffDays = Math.floor(diffHours / 24);
    return `${diffDays}d ago`;
  }

  function claudeStatusColor(status: string | null): string {
    switch (status) {
      case 'working': return '#50c86e';   // green — active
      case 'blocked': return '#f0b429';   // yellow — needs input
      case 'completed': return '#6c8ebf'; // blue — done
      case 'failed': return '#e64a4a';    // red
      case 'stopped': return '#888';      // grey — stopped by hook or user
      case 'idle': return '#888';         // grey
      default: return 'transparent';
    }
  }

  function claudeStatusLabel(status: string | null): string {
    switch (status) {
      case 'working': return '⚡ working';
      case 'blocked': return '⏸ blocked';
      case 'completed': return '✓ done';
      case 'failed': return '✗ failed';
      case 'stopped': return '■ stopped';
      case 'idle': return '· idle';
      default: return '';
    }
  }
</script>

<div class="sidebar" data-testid="sidebar-tree">
  {#snippet sessionRow(sess: SessionRow)}
    {@const sessSelected = $selectedSession?.id === sess.id}
    {@const isRenaming = renamingName === sess.tmux_name}
    <div
      class="sess-row"
      class:selected={sessSelected}
      class:renaming={isRenaming}
      data-testid="sess-row"
      role="button"
      tabindex="0"
      ondblclick={(e) => sess.status !== 'ghost' && beginRename(sess, e)}
      onclick={() => !isRenaming && sess.status !== 'ghost' && onSelectSession(sess)}
      onkeydown={(e) => !isRenaming && sess.status !== 'ghost' && onKeySession(e, sess)}
    >
      {#if isRenaming}
        <input
          bind:this={renameInput}
          class="rename-input"
          data-testid="rename-input"
          bind:value={renameValue}
          onkeydown={onRenameKey}
          onblur={commitRename}
        />
      {:else}
        {#if sess.status === 'ghost'}
          <span class="status-dot status-ghost" title="ghost — session lost" aria-hidden="true"></span>
          <span class="host-badge" data-testid="host-badge">[{sess.host_alias}]</span>
          <span class="sess-name" title={sess.tmux_name}>{
            $showFriendlyNames && sess.friendly_name ? sess.friendly_name : sess.tmux_name
          }</span>
          {#if sess.lost_at}
            <span class="lost-at" title="Lost at {new Date(sess.lost_at * 1000).toLocaleString()}">
              lost {timeAgo(sess.lost_at)}
            </span>
          {/if}
          <div class="row-actions">
            <button
              class="icon-btn small"
              data-testid="ghost-recreate"
              onclick={(e) => doRecreate(sess, e)}
              disabled={!hostIsReachable(sess.host_alias)}
              title={hostIsReachable(sess.host_alias) ? 'Recreate tmux session' : 'Host is offline'}
              aria-label="Recreate"
            >↺</button>
            <button
              class="icon-btn small danger"
              data-testid="ghost-dismiss"
              onclick={(e) => doDismissGhost(sess, e)}
              title="Dismiss ghost session"
              aria-label="Dismiss"
            >×</button>
          </div>
        {:else}
          <span class="status-dot status-{sess.status}" title={sess.status} aria-hidden="true"></span>
          {#if relatedCountFor(sess) > 0}
            <span
              class="related-badge"
              data-testid="related-badge"
              role="img"
              title="{relatedCountFor(sess)} related session(s)"
              aria-label="{relatedCountFor(sess)} related sessions"
            >🔗{relatedCountFor(sess)}</span>
          {/if}
          {#if sess.kind === 'review'}
            <span class="review-badge" role="img" title="review session" aria-label="review session">🔍</span>
          {/if}
          {#if sess.kind === 'shell'}
            <span class="shell-badge" title="shell session">▶</span>
          {/if}
          {#if sess.kind === 'bg'}
            <span class="bg-badge" role="img" title="background agent" aria-label="background agent">🤖</span>
          {/if}
          <span class="host-badge" data-testid="host-badge">[{sess.host_alias}]</span>
          <span class="sess-name" title={sess.tmux_name}>{
            $showFriendlyNames && sess.friendly_name ? sess.friendly_name : sess.tmux_name
          }</span>
          {#if sess.claude_status}
            <span
              class="claude-chip"
              style="background: {claudeStatusColor(sess.claude_status)}22; color: {claudeStatusColor(sess.claude_status)}; border-color: {claudeStatusColor(sess.claude_status)}44;"
              title="Claude: {sess.claude_status}{sess.current_activity ? ' — ' + sess.current_activity : ''}"
            >{claudeStatusLabel(sess.claude_status)}</span>
          {/if}
          {#if sess.effort_level}
            <span class="effort-badge" title="Effort: {sess.effort_level}">{sess.effort_level}</span>
          {/if}
          {#if sess.pr_url}
            <a
              class="pr-link"
              href={sess.pr_url}
              onclick={(e) => e.stopPropagation()}
              title="Open pull request"
              target="_blank"
              rel="noreferrer"
            >PR↗</a>
          {/if}
          <div class="row-actions">
            {#if sess.claude_session_id && sess.status !== 'ghost'}
              <button
                class="icon-btn small peek-btn"
                data-testid="peek-session"
                title="Peek at session logs"
                onclick={(e) => { e.stopPropagation(); doPeek(sess); }}
                aria-label="Peek"
              >📋</button>
            {/if}
            <button class="icon-btn small" onclick={(e) => doRestart(sess, e)} title="Restart claude in this session" aria-label="Restart">↻</button>
            <button class="icon-btn small" onclick={(e) => beginRename(sess, e)} title="Rename session" aria-label="Rename">✎</button>
            <button
              class="icon-btn small"
              data-testid="recreate-live"
              onclick={(e) => askRecreate(sess, e)}
              disabled={!hostIsReachable(sess.host_alias)}
              title={hostIsReachable(sess.host_alias)
                ? 'Recreate: kill the tmux session and start it fresh in the same worktree'
                : 'Host is offline'}
              aria-label="Recreate"
            >♻</button>
            <button class="icon-btn small danger" onclick={(e) => askKill(sess, e)} title="Kill session" aria-label="Kill">×</button>
          </div>
        {/if}
      {/if}
    </div>
    {#if isRenaming && renameError}
      <p class="err inline-err">{renameError}</p>
    {/if}
    {#if peekState[sess.id] !== undefined && peekState[sess.id] !== null}
      <div class="peek-panel" data-testid="peek-panel">
        <div class="peek-header">
          <span>Logs — {sess.tmux_name}</span>
          <button onclick={() => closePeek(sess.id)} class="peek-close">✕</button>
        </div>
        {#if peekState[sess.id] === "loading"}
          <p class="peek-loading">Loading…</p>
        {:else}
          <pre class="peek-output">{peekState[sess.id]}</pre>
        {/if}
      </div>
    {/if}
  {/snippet}

  <header class="sidebar-header" data-testid="sidebar-chrome-top">
    <div class="row">
      <input
        class="search"
        placeholder="Search sessions, projects…"
        bind:value={search}
        data-testid="sidebar-search"
      />
      <button class="icon-btn" onclick={onRefresh} disabled={loading} data-testid="sidebar-refresh" title="Refresh">
        {#if loading}…{:else}↻{/if}
      </button>
      {#if onCollapse}
        <button
          class="icon-btn"
          onclick={onCollapse}
          title="Hide sidebar (more room for terminal)"
          aria-label="Hide sidebar"
          data-testid="sidebar-collapse"
        >‹</button>
      {/if}
    </div>

    <nav class="hosts" aria-label="host filter">
      <button
        class="pill"
        class:active={$hostFilter === 'all'}
        onclick={() => hostFilter.set('all')}
      >all</button>
      {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}
        <button
          class="pill"
          class:active={$hostFilter === h.alias}
          onclick={() => hostFilter.set(h.alias)}
          title={`${h.alias}${h.tmux_version ? ` · tmux ${h.tmux_version}` : ''}${h.claude_version ? ` · claude ${h.claude_version}` : ''}${accountLabel(h)}`}
        >
          <span class="host-dot status-{h.reachable ? 'on' : 'off'}"></span>
          {h.alias}
        </button>
      {/each}
      <button
        class="icon-btn"
        onclick={() => (showSettings = true)}
        title="Settings"
        aria-label="Settings"
        aria-expanded={showSettings}
        data-testid="settings-open"
      >⚙</button>
    </nav>

    <nav class="recency" aria-label="recency filter">
      {#each RECENCY_VALUES as opt (opt)}
        <button
          class="pill"
          class:active={recency === opt}
          onclick={() => (recency = opt)}
        >
          {opt}
        </button>
      {/each}
    </nav>

    <nav class="bg-toggle" aria-label="background agents filter">
      <button
        class="pill"
        class:active={$showBgAgents}
        data-testid="bg-toggle"
        aria-pressed={$showBgAgents}
        title={$showBgAgents ? 'Hide background agents' : 'Show background agents'}
        onclick={() => showBgAgents.update((v) => !v)}
      >
        🤖 bg {$showBgAgents ? 'on' : 'off'}
      </button>
      <button
        class="pill"
        class:active={$showFriendlyNames}
        data-testid="friendly-name-toggle"
        aria-pressed={$showFriendlyNames}
        title={$showFriendlyNames
          ? 'Show raw tmux names'
          : 'Show agent-set friendly names'}
        onclick={() => showFriendlyNames.update((v) => !v)}
      >
        🏷 friendly {$showFriendlyNames ? 'on' : 'off'}
      </button>
    </nav>

    {#if loadError}
      <p class="err">{loadError}</p>
    {/if}
    {#if actionError}
      <p class="err">{actionError}</p>
    {/if}
  </header>

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
          ? 'No projects yet — click ↻ to scan ~/projects/github.com.'
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

<svelte:window onkeydown={(e) => {
  if (e.key !== 'Escape') return;
  if (dialogProject) onCancel();
  else if (pendingKill) cancelKill();
  else if (pendingRecreate) cancelRecreate();
  else if (pendingPurge) cancelPurge();
  else if (showBgModal) showBgModal = false;
  else if (showProjectPicker) showProjectPicker = false;
}} />

{#if dialogProject}
  <div class="modal-backdrop" onclick={onCancel} role="presentation">
    <div onclick={(e) => e.stopPropagation()} role="presentation">
      <NewSessionDialog project={dialogProject} onCreate={onCreated} {onCancel} />
    </div>
  </div>
{/if}

{#if pendingKill}
  <div class="modal-backdrop" onclick={cancelKill} role="presentation">
    <div class="confirm" onclick={(e) => e.stopPropagation()} role="presentation">
      <h3>Kill session?</h3>
      <p>This will kill the tmux session <code>{pendingKill.tmux_name}</code> and lose any running claude state inside it. Continue?</p>
      <div class="actions">
        <button onclick={cancelKill}>Cancel</button>
        <button class="danger" onclick={confirmKill} data-testid="confirm-kill">Kill</button>
      </div>
    </div>
  </div>
{/if}

{#if pendingRecreate}
  <div class="modal-backdrop" onclick={cancelRecreate} role="presentation">
    <div class="confirm" onclick={(e) => e.stopPropagation()} role="presentation">
      <h3>Recreate session?</h3>
      <p>
        This kills the tmux session <code>{pendingRecreate.tmux_name}</code> and the
        running claude state inside it, then starts a fresh session in the same
        worktree. Continue?
      </p>
      <div class="actions">
        <button onclick={cancelRecreate}>Cancel</button>
        <button class="danger" onclick={confirmRecreate} data-testid="confirm-recreate">Recreate</button>
      </div>
    </div>
  </div>
{/if}

{#if showSettings}
  <SettingsDialog onClose={() => (showSettings = false)} />
{/if}

{#if pendingPurge}
  <div class="modal-backdrop" onclick={cancelPurge} role="presentation">
    <div class="confirm" onclick={(e) => e.stopPropagation()} role="presentation">
      <h3>Purge project?</h3>
      <p>This will permanently delete all Claude Code state for <code>{pendingPurge.repo}</code>. This is irreversible.</p>
      <div class="actions">
        <button onclick={cancelPurge}>Cancel</button>
        <button class="danger" onclick={confirmPurge} data-testid="confirm-purge">Purge</button>
      </div>
    </div>
  </div>
{/if}

{#if showBgModal}
  <div class="modal-backdrop" onclick={() => (showBgModal = false)} role="presentation">
    <div class="modal" onclick={(e) => e.stopPropagation()} data-testid="bg-session-modal" role="presentation">
      <h3>New Background Session</h3>
      <label class="modal-field">
        <span>Host</span>
        <select bind:value={bgModalHost}>
          {#each $hosts as host (host.alias)}
            <option value={host.alias}>{host.alias}</option>
          {/each}
        </select>
      </label>
      <label class="modal-field">
        <span>Session name</span>
        <input
          type="text"
          bind:value={bgModalName}
          placeholder="e.g. fix-auth-bug"
          data-testid="bg-session-name"
        />
      </label>
      <label class="modal-field">
        <span>Initial prompt</span>
        <textarea
          bind:value={bgModalPrompt}
          rows="4"
          placeholder="What should Claude work on?"
          data-testid="bg-session-prompt"
        ></textarea>
      </label>
      {#if bgModalError}
        <p class="err">{bgModalError}</p>
      {/if}
      <div class="modal-actions">
        <button onclick={() => (showBgModal = false)}>Cancel</button>
        <button
          class="btn-primary"
          onclick={doNewBgSession}
          disabled={bgModalLoading || !bgModalName.trim() || !bgModalPrompt.trim()}
          data-testid="bg-session-submit"
        >
          {bgModalLoading ? 'Launching…' : 'Launch'}
        </button>
      </div>
    </div>
  </div>
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
  .sidebar-header {
    flex: 0 0 auto;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.5rem 0.6rem 0.4rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
  }
  .sidebar-header .row {
    display: flex;
    gap: 0.3rem;
    align-items: center;
  }
  .search {
    flex: 1;
    font-size: 0.85rem;
    padding: 0.3rem 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--fg);
    border-radius: 5px;
  }
  .search::placeholder { color: var(--fg-muted); }

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
  .icon-btn.danger:hover { color: #e64a4a; border-color: #e64a4a; }

  .recency { display: flex; gap: 0.25rem; }
  .bg-toggle { display: flex; gap: 0.25rem; }
  .pill {
    font-size: 0.7rem;
    padding: 0.15rem 0.55rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
  }
  .pill.active { color: var(--fg); border-color: var(--accent); }

  .hosts { display: flex; flex-wrap: wrap; gap: 0.25rem; align-items: center; }
  .host-dot {
    display: inline-block;
    width: 0.4rem;
    height: 0.4rem;
    border-radius: 50%;
    margin-right: 0.3rem;
    vertical-align: middle;
  }
  .host-dot.status-on { background: rgb(80, 200, 110); }
  .host-dot.status-off { background: rgb(220, 130, 130); }

  .host-badge {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    flex-shrink: 0;
  }

  .related-badge {
    font-size: 0.65rem;
    color: var(--fg-muted);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    flex-shrink: 0;
  }

  .review-badge { font-size: 0.7rem; margin-left: 0.2rem; }
  .shell-badge { font-size: 0.7rem; margin-left: 0.2rem; color: var(--fg-muted); }
  .bg-badge { font-size: 0.7rem; margin-left: 0.2rem; }

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

  .err { color: #e64a4a; font-size: 0.8rem; padding: 0.2rem 0; margin: 0; }
  .inline-err { padding-left: 1.6rem; font-size: 0.75rem; }
  .empty { color: var(--fg-muted); font-size: 0.85rem; padding: 0.5rem 0.4rem; }
  .pad { padding: 0.5rem 0.6rem; }

  .sess-row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.82rem;
    padding: 0.22rem 0.4rem 0.22rem 1.4rem;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
    user-select: none;
  }
  .sess-row:hover { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .sess-row.selected { background: color-mix(in srgb, var(--accent) 22%, transparent); }
  .sess-row.renaming { background: var(--bg-pane); }
  .sess-row .row-actions {
    display: none;
    gap: 0.05rem;
  }
  .sess-row:hover .row-actions,
  .sess-row.selected .row-actions { display: flex; }

  .status-dot {
    width: 0.45rem;
    height: 0.45rem;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--fg-muted);
  }
  .status-dot.status-running { background: rgb(80, 200, 110); }
  .status-dot.status-frozen { background: rgb(140, 180, 240); }
  .status-dot.status-orphan { background: rgb(220, 130, 130); }
  .status-dot.status-ghost { background: rgb(160, 120, 200); opacity: 0.55; }
  .lost-at {
    font-size: 0.7em;
    opacity: 0.6;
    margin-left: auto;
    padding-right: 0.25rem;
    white-space: nowrap;
  }

  .claude-chip {
    font-size: 0.65rem;
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    border: 1px solid;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .effort-badge {
    font-size: 0.6rem;
    padding: 0.05rem 0.25rem;
    border-radius: 3px;
    background: color-mix(in srgb, var(--fg) 10%, transparent);
    color: var(--fg-muted);
    flex-shrink: 0;
    white-space: nowrap;
    text-transform: uppercase;
  }
  .pr-link {
    font-size: 0.65rem;
    color: var(--accent);
    text-decoration: none;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .pr-link:hover { text-decoration: underline; }

  .sess-name {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.8rem;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .rename-input {
    flex: 1;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.8rem;
    padding: 0.1rem 0.3rem;
    border: 1px solid var(--accent);
    background: var(--bg);
    color: var(--fg);
    border-radius: 3px;
    outline: none;
    min-width: 0;
  }

  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 10;
  }
  .confirm {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem;
    width: 360px;
    color: var(--fg);
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  .confirm h3 { margin: 0; font-size: 0.95rem; }
  .confirm p { margin: 0; font-size: 0.85rem; color: var(--fg-muted); line-height: 1.4; }
  .confirm code {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    background: var(--bg-pane);
    padding: 0.1rem 0.3rem;
    border-radius: 3px;
    color: var(--fg);
  }
  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button.danger {
    color: #e64a4a;
    border-color: #e64a4a;
  }
  .actions button.danger:hover { background: rgba(230, 74, 74, 0.12); }

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

  .peek-btn {
    opacity: 0.6;
  }
  .peek-btn:hover { opacity: 1; }
  .peek-panel {
    background: var(--color-surface-2, #1e1e2e);
    border: 1px solid var(--border);
    border-radius: 4px;
    margin: 2px 8px 4px 8px;
    padding: 8px;
    font-size: 12px;
  }
  .peek-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 6px;
    font-weight: 600;
  }
  .peek-close {
    background: none;
    border: none;
    cursor: pointer;
    color: var(--fg-muted);
  }
  .peek-loading {
    color: var(--fg-muted);
    font-style: italic;
    margin: 0;
  }
  .peek-output {
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 200px;
    overflow-y: auto;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    margin: 0;
  }

  .modal {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 20px;
    min-width: 320px;
    max-width: 480px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    color: var(--fg);
  }
  .modal h3 { margin: 0; font-size: 0.95rem; }
  .modal-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
  }
  .modal-field input,
  .modal-field select,
  .modal-field textarea {
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-pane);
    color: var(--fg);
    font-family: inherit;
    font-size: 12px;
  }
  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
  .modal-actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .btn-primary {
    color: var(--accent) !important;
    border-color: var(--accent) !important;
  }
  .btn-primary:hover:not(:disabled) { background: color-mix(in srgb, var(--accent) 14%, transparent) !important; }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
