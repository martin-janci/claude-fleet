<script lang="ts">
  import { selectedProject } from './selection';
  import { sessions } from './sessions';

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
            <li>
              <span class="mono">{wt.name}</span>
              {#if wt.branch && wt.branch !== wt.name}<span class="branch"> · {wt.branch}</span>{/if}
              <div class="sub-path">{wt.path}</div>
            </li>
          {/each}
        </ul>
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
</style>
