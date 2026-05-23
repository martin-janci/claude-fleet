<script lang="ts">
  import { selectedProject } from './selection';
  import { sessions } from './sessions';
  import { deleteWorktree } from './projects';

  function formatRelative(unix: number | null): string {
    if (unix === null) return 'never';
    const ageSec = Math.floor(Date.now() / 1000) - unix;
    if (ageSec < 60) return 'just now';
    if (ageSec < 3600) return `${Math.floor(ageSec / 60)}m ago`;
    if (ageSec < 86400) return `${Math.floor(ageSec / 3600)}h ago`;
    const days = Math.floor(ageSec / 86400);
    if (days < 30) return `${days}d ago`;
    return new Date(unix * 1000).toISOString().slice(0, 10);
  }

  // Worktree id → occupants (host/name) derived from alive sessions in the
  // sessions store. An empty list means the worktree is free to delete.
  function occupantsOf(worktreeId: number) {
    return $sessions.filter(
      (s) => s.worktree_id === worktreeId && s.status === 'running' && s.lost_at === null,
    );
  }

  let pendingDelete: number | null = $state(null);
  let deleteError: string | null = $state(null);

  async function doDelete(id: number) {
    deleteError = null;
    const r = await deleteWorktree(id, false);
    pendingDelete = null;
    if (!r.ok) deleteError = r.error.message;
  }
</script>

{#if $selectedProject}
  {@const p = $selectedProject}
  <article class="details" data-testid="project-details">
    <header class="header">
      <h2 class="title">{p.project.owner}/{p.project.repo}</h2>
      <code class="path" title={p.project.base_path}>{p.project.base_path}</code>
    </header>

    <dl class="meta">
      <dt>Last session</dt>
      <dd>{formatRelative(p.project.last_session_at)}</dd>

      <dt>Worktrees</dt>
      <dd>{p.worktrees.length}</dd>

      <dt>Active sessions</dt>
      <dd>{$sessions.filter((s) => s.project_id === p.project.id).length}</dd>
    </dl>

    {#if p.worktrees.length > 0}
      <section class="block">
        <h3>Worktrees</h3>
        <ul>
          {#each p.worktrees as wt (wt.id)}
            {@const occs = occupantsOf(wt.id)}
            <li class="wt-row">
              <div class="wt-info">
                <span class="mono">{wt.name}</span>
                {#if wt.branch && wt.branch !== wt.name}<span class="branch"> · {wt.branch}</span>{/if}
                {#if occs.length > 0}
                  <span class="occupied" title={occs.map((s) => `${s.host_alias}/${s.tmux_name}`).join(', ')}>
                    in use by {occs.length} session{occs.length === 1 ? '' : 's'}
                  </span>
                {/if}
                <div class="sub-path">{wt.path}</div>
              </div>
              <div class="wt-actions">
                {#if occs.length === 0}
                  <button
                    class="wt-delete"
                    onclick={() => (pendingDelete = wt.id)}
                    data-testid="delete-worktree-{wt.id}"
                  >
                    Delete
                  </button>
                {:else}
                  <span class="wt-busy">occupied</span>
                {/if}
              </div>
            </li>
          {/each}
        </ul>
        {#if deleteError}
          <p class="err">{deleteError}</p>
        {/if}
      </section>
    {/if}

    {#if $sessions.filter((s) => s.project_id === p.project.id).length > 0}
      <section class="block">
        <h3>Sessions</h3>
        <ul>
          {#each $sessions.filter((s) => s.project_id === p.project.id) as sess (sess.id)}
            <li>
              <span class="mono">{sess.tmux_name}</span>
              <span class="branch"> · {sess.status}</span>
              <div class="sub-path">last activity {formatRelative(sess.last_activity_at)}</div>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  </article>
{:else}
  <p class="empty">Pick a project to see details.</p>
{/if}

{#if pendingDelete !== null}
  {@const wt = $selectedProject?.worktrees.find((w) => w.id === pendingDelete) ?? null}
  <div class="modal-backdrop" onclick={() => (pendingDelete = null)} role="presentation">
    <div class="confirm" onclick={(e) => e.stopPropagation()} role="presentation">
      <h3>Delete worktree?</h3>
      {#if wt}
        <p>
          Run <code>git worktree remove</code> for <code class="mono">{wt.path}</code> on its
          host and drop the fleet row. The git command will refuse if the
          worktree has uncommitted changes — your work won't be silently lost.
        </p>
      {/if}
      <div class="confirm-actions">
        <button onclick={() => (pendingDelete = null)}>Cancel</button>
        <button class="danger" onclick={() => doDelete(pendingDelete!)} data-testid="confirm-delete-worktree">
          Delete
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .details {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    color: var(--fg);
  }
  .header { display: flex; flex-direction: column; gap: 0.2rem; }
  .title { margin: 0; font-size: 1.1rem; font-weight: 600; }
  .path {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .meta {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.25rem 0.75rem;
    margin: 0;
  }
  .meta dt { color: var(--fg-muted); font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.04em; }
  .meta dd { margin: 0; font-size: 0.9rem; }
  .block h3 {
    margin: 0 0 0.4rem 0;
    font-size: 0.75rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .block ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.5rem; }
  .block li { font-size: 0.9rem; }
  .mono { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  .branch { color: var(--fg-muted); font-style: italic; }
  .sub-path {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
  }
  .empty { color: var(--fg-muted); font-size: 0.9rem; }

  .wt-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.75rem;
  }
  .wt-info { display: flex; flex-direction: column; gap: 0.1rem; min-width: 0; }
  .wt-actions { flex: 0 0 auto; }
  .occupied {
    font-size: 0.7rem;
    color: var(--fg-muted);
    margin-left: 0.4rem;
    font-style: italic;
  }
  .wt-delete {
    background: transparent;
    border: 1px solid rgba(230, 74, 74, 0.6);
    color: #e64a4a;
    border-radius: 4px;
    padding: 0.2rem 0.55rem;
    font-size: 0.78rem;
    cursor: pointer;
  }
  .wt-delete:hover { background: rgba(230, 74, 74, 0.1); }
  .wt-busy {
    font-size: 0.7rem;
    color: var(--fg-muted);
    font-style: italic;
    padding: 0.2rem 0.4rem;
  }
  .err { color: #e64a4a; font-size: 0.78rem; margin: 0.4rem 0 0 0; }

  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 10;
  }
  .confirm {
    background: var(--bg-pane);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem 1.2rem;
    max-width: 460px;
    color: var(--fg);
  }
  .confirm h3 { margin: 0 0 0.5rem 0; font-size: 1rem; }
  .confirm p { font-size: 0.85rem; line-height: 1.4; margin: 0 0 0.8rem 0; }
  .confirm-actions { display: flex; justify-content: flex-end; gap: 0.5rem; }
  .confirm-actions button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    padding: 0.3rem 0.7rem;
    border-radius: 4px;
    cursor: pointer;
  }
  .confirm-actions button.danger {
    border-color: rgba(230, 74, 74, 0.6);
    color: #e64a4a;
  }
  .confirm-actions button.danger:hover { background: rgba(230, 74, 74, 0.1); }
</style>
