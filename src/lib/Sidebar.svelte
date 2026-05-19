<script lang="ts">
  import { onMount } from 'svelte';
  import { projects, loadProjects, refreshProjects, type ProjectTreeRow } from './projects';
  import { sessions, loadSessions, killSession, type SessionRow } from './sessions';
  import NewSessionDialog from './NewSessionDialog.svelte';

  type Recency = 'all' | 'today' | '7d' | '30d';

  let loadError: string | null = $state(null);
  let loading = $state(false);
  let recency: Recency = $state('all');
  let search = $state('');

  onMount(async () => {
    const pr = await loadProjects();
    if (!pr.ok) loadError = pr.error.message;
    const sr = await loadSessions();
    if (!sr.ok) loadError = sr.error.message;
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
    today: 60 * 60 * 24,
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
    return p.worktrees.some(
      (w) =>
        w.name.toLowerCase().includes(needle) ||
        (w.branch?.toLowerCase().includes(needle) ?? false),
    );
  }

  const filtered = $derived(
    $projects.filter((p) => matchesRecency(p, recency) && matchesSearch(p, search)),
  );

  async function onKill(name: string) {
    if (!confirm(`Kill tmux session ${name}?`)) return;
    const r = await killSession(name);
    if (!r.ok) loadError = r.error.message;
  }

  function sessionsForProject(projectId: number): SessionRow[] {
    return $sessions.filter((s) => s.project_id === projectId);
  }

  // Sessions whose tmux working directory didn't map to any known project
  // (created outside ~/projects/github.com/, or before the user clicked
  // refresh). Render them in a dedicated section so they aren't invisible.
  const orphanSessions = $derived($sessions.filter((s) => s.project_id === null));

  let dialogProject: ProjectTreeRow | null = $state(null);

  function openNew(p: ProjectTreeRow) {
    dialogProject = p;
  }

  function onCreated(_s: SessionRow) {
    dialogProject = null;
  }

  function onCancel() {
    dialogProject = null;
  }
</script>

<div class="sidebar" data-testid="sidebar-tree">
  <header class="sidebar-header">
    <input
      class="search"
      placeholder="Search projects, branches…"
      bind:value={search}
      data-testid="sidebar-search"
    />
    <button class="refresh" onclick={onRefresh} disabled={loading} data-testid="sidebar-refresh">
      {loading ? 'refreshing…' : 'refresh'}
    </button>
  </header>

  <nav class="recency" aria-label="recency filter">
    {#each ['all', 'today', '7d', '30d'] as opt (opt)}
      <button
        class="pill"
        class:active={recency === opt}
        onclick={() => (recency = opt as Recency)}
      >
        {opt}
      </button>
    {/each}
  </nav>

  {#if loadError}
    <p class="err">{loadError}</p>
  {/if}

  {#if filtered.length > 0}
    <ul class="tree">
      {#each filtered as row (row.project.id)}
        <li class="proj">
          <div class="proj-row" data-testid="proj-row" title={row.project.base_path}>
            <span class="owner">{row.project.owner}/</span><span class="repo">{row.project.repo}</span>
            <button class="add-session" onclick={() => openNew(row)} title="New session">+</button>
          </div>
          {#if row.worktrees.length > 0}
            <ul class="worktrees">
              {#each row.worktrees as wt (wt.id)}
                <li class="wt" data-testid="wt-row" title={wt.path}>
                  <span class="wt-bullet">└</span>
                  <span class="wt-name">{wt.name}</span>
                  {#if wt.branch && wt.branch !== wt.name}
                    <span class="wt-branch">({wt.branch})</span>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
          {#each sessionsForProject(row.project.id) as sess (sess.id)}
            <div class="sess-row" data-testid="sess-row">
              <span class="sess-name">{sess.tmux_name}</span>
              <button class="kill" onclick={() => onKill(sess.tmux_name)} title="Kill session">×</button>
            </div>
          {/each}
        </li>
      {/each}
    </ul>
  {:else if !loadError && orphanSessions.length === 0}
    <p class="empty">
      {$projects.length === 0
        ? 'No projects yet — click refresh to scan ~/projects/github.com.'
        : 'No projects match the current filter.'}
    </p>
  {/if}

  {#if orphanSessions.length > 0}
    <div class="orphan-section" data-testid="orphan-sessions">
      <div class="section-header">Other sessions ({orphanSessions.length})</div>
      {#each orphanSessions as sess (sess.id)}
        <div class="sess-row" data-testid="sess-row">
          <span class="sess-name">{sess.tmux_name}</span>
          <button class="kill" onclick={() => onKill(sess.tmux_name)} title="Kill session">×</button>
        </div>
      {/each}
    </div>
  {/if}
</div>

<svelte:window onkeydown={(e) => dialogProject && e.key === 'Escape' && onCancel()} />

{#if dialogProject}
  <div class="modal-backdrop" onclick={onCancel} role="presentation">
    <div onclick={(e) => e.stopPropagation()} role="presentation">
      <NewSessionDialog project={dialogProject} onCreate={onCreated} {onCancel} />
    </div>
  </div>
{/if}

<style>
  .sidebar { display: flex; flex-direction: column; height: 100%; gap: 0.4rem; }
  .sidebar-header { display: flex; gap: 0.3rem; align-items: center; padding: 0.25rem 0; }
  .search {
    flex: 1;
    font-size: 0.8rem;
    padding: 0.2rem 0.4rem;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--fg);
    border-radius: 4px;
  }
  .search::placeholder { color: var(--fg-muted); }
  .refresh {
    font-size: 0.75rem;
    padding: 0.2rem 0.5rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 4px;
    cursor: pointer;
  }
  .refresh:hover:not(:disabled) { color: var(--fg); border-color: var(--accent); }
  .refresh:disabled { opacity: 0.6; cursor: progress; }
  .recency { display: flex; gap: 0.25rem; }
  .pill {
    font-size: 0.7rem;
    padding: 0.15rem 0.5rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
  }
  .pill.active { color: var(--fg); border-color: var(--accent); }
  .tree { list-style: none; margin: 0; padding: 0; flex: 1; overflow: auto; }
  .proj { margin-bottom: 0.4rem; }
  .proj-row { font-weight: 500; padding: 0.15rem 0; }
  .owner { color: var(--fg-muted); }
  .worktrees { list-style: none; margin: 0; padding-left: 0.6rem; }
  .wt { font-size: 0.85rem; padding: 0.1rem 0; color: var(--fg-muted); display: flex; gap: 0.3rem; }
  .wt-bullet { color: var(--border); }
  .wt-name { color: var(--fg); }
  .wt-branch { font-style: italic; }
  .err { color: #e64a4a; font-size: 0.85rem; padding: 0.25rem 0; }
  .empty { color: var(--fg-muted); font-size: 0.85rem; }
  .sess-row { display: flex; gap: 0.3rem; font-size: 0.8rem; padding: 0.1rem 0 0.1rem 0.6rem; color: var(--fg); }
  .sess-name { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .kill {
    margin-left: auto;
    background: transparent;
    border: none;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 0 0.3rem;
  }
  .kill:hover { color: #e64a4a; }
  .proj-row { display: flex; align-items: center; }
  .add-session {
    margin-left: auto;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg-muted);
    padding: 0 0.4rem;
    border-radius: 4px;
    font-size: 0.8rem;
    cursor: pointer;
  }
  .add-session:hover { color: var(--fg); border-color: var(--accent); }
  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 10;
  }
  .orphan-section {
    border-top: 1px solid var(--border);
    padding-top: 0.4rem;
    margin-top: 0.4rem;
  }
  .section-header {
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-muted);
    padding: 0 0 0.2rem 0;
  }
</style>
