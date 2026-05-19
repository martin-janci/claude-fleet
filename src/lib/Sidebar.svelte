<script lang="ts">
  import { onMount } from 'svelte';
  import { projects, loadProjects, refreshProjects } from './projects';

  let loadError: string | null = $state(null);
  let loading = $state(false);

  onMount(async () => {
    const r = await loadProjects();
    if (!r.ok) loadError = r.error.message;
  });

  async function onRefresh() {
    loading = true;
    loadError = null;
    const r = await refreshProjects();
    loading = false;
    if (!r.ok) loadError = r.error.message;
  }
</script>

<div class="sidebar" data-testid="sidebar-tree">
  <header class="sidebar-header">
    <button class="refresh" onclick={onRefresh} disabled={loading} data-testid="sidebar-refresh">
      {loading ? 'refreshing…' : 'refresh'}
    </button>
  </header>

  {#if loadError}
    <p class="err">{loadError}</p>
  {:else if $projects.length === 0}
    <p class="empty">No projects yet — click refresh to scan ~/projects/github.com.</p>
  {:else}
    <ul class="tree">
      {#each $projects as row (row.project.id)}
        <li class="proj">
          <div class="proj-row" data-testid="proj-row" title={row.project.base_path}>
            <span class="owner">{row.project.owner}/</span><span class="repo">{row.project.repo}</span>
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
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .sidebar { display: flex; flex-direction: column; height: 100%; }
  .sidebar-header { display: flex; justify-content: flex-end; padding: 0.25rem 0; }
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
  .tree { list-style: none; margin: 0; padding: 0; }
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
</style>
