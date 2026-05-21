<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { projects, refreshProjects, bootstrapProjects, type ProjectTreeRow } from './projects';
  import {
    sessions,
    loadSessions,
    killSession,
    renameSession,
    restartSession,
    bootstrapSessions,
    type SessionRow,
  } from './sessions';
  import { selectedSession, selectSession } from './selection';
  import { forgetSessionUi, migrateSessionUi } from './session_ui';
  import { readPref, writePref } from './prefs';
  import { theme, cycleTheme } from './theme';
  import NewSessionDialog from './NewSessionDialog.svelte';
  import SettingsDialog from './SettingsDialog.svelte';
  import { hosts, bootstrapHosts, hostFilter } from './hosts';
  import { accounts, bootstrapAccounts, type AccountRow } from './accounts';

  let showSettings = $state(false);

  // Optional collapse handler injected by the parent (App.svelte). When
  // present, a ‹ button appears in the sidebar header so the user can
  // hide the whole sidebar to make room for the terminal.
  let { onCollapse }: { onCollapse?: () => void } = $props();

  type Recency = 'all' | '1d' | '7d' | '30d';
  const RECENCY_VALUES: readonly Recency[] = ['all', '1d', '7d', '30d'];
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

  // Per-session UI state. Kept here instead of on each row so collapse and
  // rename state survive a sessions store refresh that creates new row
  // objects (the underlying tmux_name is the stable id).
  let renamingName: string | null = $state(null);
  let renameValue = $state('');
  let renameError: string | null = $state(null);
  let pendingKill: SessionRow | null = $state(null);
  // Projects intentionally collapsed by the user. Anything not in this set
  // is open by default — most users have one or two projects and want to
  // see their sessions immediately.
  let collapsed: Set<number> = $state(new Set());
  let actionError: string | null = $state(null);

  onMount(async () => {
    await Promise.all([
      bootstrapProjects(),
      bootstrapSessions(),
      bootstrapHosts(),
      bootstrapAccounts(),
    ]);
  });

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
    // "1d" = active within the last 24h. User-requested replacement of the
    // older "today" pill — the latter was confusing because it didn't behave
    // like a calendar day (it was a 24h sliding window). Same numeric value
    // here, clearer label.
    '1d': 60 * 60 * 24,
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
        s.host_alias.toLowerCase().includes(needle),
    );
  }

  // Only show projects that either match the filter directly OR have at least
  // one active session. Without sessions the sidebar would be flooded with
  // every cloned repo on disk — most of which the user isn't working on.
  const filtered = $derived(
    $projects.filter(
      (p) =>
        matchesRecency(p, recency) &&
        matchesSearch(p, search) &&
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

  // Map: project_id → all sessions for that project (unfiltered by host)
  const sessionsByProject = $derived.by(() => {
    const m = new Map<number, SessionRow[]>();
    for (const s of $sessions) {
      if (s.project_id == null) continue;
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
    const all = sessionsByProject.get(projectId) ?? [];
    if ($hostFilter === 'all') return all;
    return all.filter((s) => s.host_alias === $hostFilter);
  }

  function relatedCountFor(sess: SessionRow): number {
    return relatedCountById.get(sess.id) ?? 0;
  }

  // Sessions whose tmux working directory didn't map to any known project.
  const orphanSessions = $derived(
    $sessions.filter(
      (s) =>
        s.project_id === null &&
        ($hostFilter === 'all' || s.host_alias === $hostFilter),
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
    const input = document.querySelector<HTMLInputElement>('[data-testid="rename-input"]');
    input?.focus();
    input?.select();
  }

  function cancelRename() {
    renamingName = null;
    renameValue = '';
    renameError = null;
  }

  async function commitRename() {
    if (!renamingName) return;
    const next = renameValue.trim();
    if (!next || next === renamingName) {
      cancelRename();
      return;
    }
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
</script>

<div class="sidebar" data-testid="sidebar-tree">
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

    {#if loadError}
      <p class="err">{loadError}</p>
    {/if}
    {#if actionError}
      <p class="err">{actionError}</p>
    {/if}
  </header>

  <div class="scroller">
    {#if filtered.length > 0}
      <ul class="tree">
        {#each filtered as row (row.project.id)}
          {@const projectSessions = sessionsForProject(row.project.id)}
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
            </div>

            {#if !isCollapsed}
              {#each projectSessions as sess (sess.id)}
                {@const sessSelected = $selectedSession?.id === sess.id}
                {@const isRenaming = renamingName === sess.tmux_name}
                <div
                  class="sess-row"
                  class:selected={sessSelected}
                  class:renaming={isRenaming}
                  data-testid="sess-row"
                  role="button"
                  tabindex="0"
                  ondblclick={(e) => beginRename(sess, e)}
                  onclick={() => !isRenaming && onSelectSession(sess)}
                  onkeydown={(e) => !isRenaming && onKeySession(e, sess)}
                >
                  {#if isRenaming}
                    <input
                      class="rename-input"
                      data-testid="rename-input"
                      bind:value={renameValue}
                      onkeydown={onRenameKey}
                      onblur={commitRename}
                    />
                  {:else}
                    <span
                      class="status-dot status-{sess.status}"
                      title={sess.status}
                      aria-hidden="true"
                    ></span>
                    {#if relatedCountFor(sess) > 0}
                      <span
                        class="related-badge"
                        data-testid="related-badge"
                        title="{relatedCountFor(sess)} related session(s)"
                      >🔗{relatedCountFor(sess)}</span>
                    {/if}
                    {#if sess.kind === 'review'}
                      <span class="review-badge" title="review session">🔍</span>
                    {/if}
                    <span class="host-badge" data-testid="host-badge">[{sess.host_alias}]</span>
                    <span class="sess-name">{sess.tmux_name}</span>
                    <div class="row-actions">
                      <button
                        class="icon-btn small"
                        onclick={(e) => doRestart(sess, e)}
                        title="Restart claude in this session"
                        aria-label="Restart"
                      >↻</button>
                      <button
                        class="icon-btn small"
                        onclick={(e) => beginRename(sess, e)}
                        title="Rename session"
                        aria-label="Rename"
                      >✎</button>
                      <button
                        class="icon-btn small danger"
                        onclick={(e) => askKill(sess, e)}
                        title="Kill session"
                        aria-label="Kill"
                      >×</button>
                    </div>
                  {/if}
                </div>
                {#if isRenaming && renameError}
                  <p class="err inline-err">{renameError}</p>
                {/if}
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
          {@const sessSelected = $selectedSession?.id === sess.id}
          {@const isRenaming = renamingName === sess.tmux_name}
          <div
            class="sess-row"
            class:selected={sessSelected}
            class:renaming={isRenaming}
            data-testid="sess-row"
            role="button"
            tabindex="0"
            ondblclick={(e) => beginRename(sess, e)}
            onclick={() => !isRenaming && onSelectSession(sess)}
            onkeydown={(e) => !isRenaming && onKeySession(e, sess)}
          >
            {#if isRenaming}
              <input
                class="rename-input"
                data-testid="rename-input"
                bind:value={renameValue}
                onkeydown={onRenameKey}
                onblur={commitRename}
              />
            {:else}
              <span
                class="status-dot status-{sess.status}"
                title={sess.status}
                aria-hidden="true"
              ></span>
              {#if relatedCountFor(sess) > 0}
                <span
                  class="related-badge"
                  data-testid="related-badge"
                  title="{relatedCountFor(sess)} related session(s)"
                >🔗{relatedCountFor(sess)}</span>
              {/if}
              {#if sess.kind === 'review'}
                <span class="review-badge" title="review session">🔍</span>
              {/if}
              <span class="host-badge" data-testid="host-badge">[{sess.host_alias}]</span>
              <span class="sess-name">{sess.tmux_name}</span>
              <div class="row-actions">
                <button class="icon-btn small" onclick={(e) => doRestart(sess, e)} title="Restart">↻</button>
                <button class="icon-btn small" onclick={(e) => beginRename(sess, e)} title="Rename">✎</button>
                <button class="icon-btn small danger" onclick={(e) => askKill(sess, e)} title="Kill">×</button>
              </div>
            {/if}
          </div>
          {#if isRenaming && renameError}
            <p class="err inline-err">{renameError}</p>
          {/if}
        {/each}
      </div>
    {/if}
  </div>

  <footer class="sidebar-footer" data-testid="sidebar-chrome-bottom">
    <button
      class="new-btn"
      onclick={() => (showProjectPicker = !showProjectPicker)}
      data-testid="new-session-footer"
    >
      + New session
    </button>
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

{#if showSettings}
  <SettingsDialog onClose={() => (showSettings = false)} />
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
  .new-btn {
    width: 100%;
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
