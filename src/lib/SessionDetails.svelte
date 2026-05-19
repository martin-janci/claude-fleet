<script lang="ts">
  import type { SessionRow } from './sessions';
  import { killSession } from './sessions';
  import { projects } from './projects';
  import { clearSelection } from './selection';

  let { session }: { session: SessionRow } = $props();

  // Look up the parent project (if any) for context.
  const parentProject = $derived(
    session.project_id === null
      ? null
      : ($projects.find((p) => p.project.id === session.project_id) ?? null),
  );

  // Local-only for v0.2 (Phase 4 will branch on host_alias for remote attach).
  const attachCommand = $derived(`tmux attach -t ${session.tmux_name}`);

  function formatRelative(unix: number): string {
    const ageSec = Math.floor(Date.now() / 1000) - unix;
    if (ageSec < 60) return 'just now';
    if (ageSec < 3600) return `${Math.floor(ageSec / 60)}m ago`;
    if (ageSec < 86400) return `${Math.floor(ageSec / 3600)}h ago`;
    const days = Math.floor(ageSec / 86400);
    if (days < 30) return `${days}d ago`;
    return new Date(unix * 1000).toISOString().slice(0, 10);
  }

  let copied = $state(false);
  let copyError: string | null = $state(null);

  async function onCopy() {
    copyError = null;
    try {
      await navigator.clipboard.writeText(attachCommand);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch (e) {
      copyError = String(e);
    }
  }

  async function onKill() {
    if (!confirm(`Kill tmux session ${session.tmux_name}?`)) return;
    const r = await killSession(session.tmux_name);
    if (r.ok) {
      clearSelection();
    } else {
      copyError = r.error.message;
    }
  }
</script>

<article class="details" data-testid="session-details">
  <header class="header">
    <h2 class="title">{session.tmux_name}</h2>
    <div class="sub">
      <span class="host">{session.host_alias}</span>
      <span class="status status-{session.status}">{session.status}</span>
    </div>
  </header>

  <dl class="meta">
    <dt>Project</dt>
    <dd>
      {#if parentProject}
        {parentProject.project.owner}/{parentProject.project.repo}
      {:else}
        <span class="muted">unmapped (orphan)</span>
      {/if}
    </dd>

    <dt>Created</dt>
    <dd>{formatRelative(session.created_at)}</dd>

    <dt>Last activity</dt>
    <dd>{formatRelative(session.last_activity_at)}</dd>
  </dl>

  <section class="block">
    <h3>Attach</h3>
    <p class="hint">
      Phase 3 will embed a terminal in this pane. For now, run this in your terminal:
    </p>
    <div class="cmd-row">
      <code class="cmd" data-testid="attach-command">{attachCommand}</code>
      <button class="copy" onclick={onCopy} data-testid="copy-attach">
        {copied ? '✓ copied' : 'copy'}
      </button>
    </div>
    {#if copyError}
      <p class="err">copy failed: {copyError}</p>
    {/if}
  </section>

  <section class="block actions">
    <button class="danger" onclick={onKill} data-testid="kill-from-details">
      Kill session
    </button>
  </section>
</article>

<style>
  .details {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    color: var(--fg);
  }
  .header { display: flex; flex-direction: column; gap: 0.3rem; }
  .title {
    margin: 0;
    font-size: 1.1rem;
    font-weight: 600;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .sub { display: flex; gap: 0.5rem; align-items: center; font-size: 0.75rem; }
  .host {
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.1rem 0.4rem;
    border-radius: 999px;
  }
  .status {
    padding: 0.1rem 0.4rem;
    border-radius: 999px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: 0.65rem;
  }
  .status-running { background: rgba(60, 180, 90, 0.18); color: rgba(80, 200, 110, 1); }
  .status-frozen { background: rgba(110, 160, 230, 0.18); color: rgba(140, 180, 240, 1); }
  .status-orphan { background: rgba(180, 100, 100, 0.18); color: rgba(220, 130, 130, 1); }

  .meta {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.25rem 0.75rem;
    margin: 0;
  }
  .meta dt {
    color: var(--fg-muted);
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .meta dd { margin: 0; font-size: 0.9rem; }
  .muted { color: var(--fg-muted); font-style: italic; }

  .block h3 {
    margin: 0 0 0.4rem 0;
    font-size: 0.7rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .hint { margin: 0 0 0.5rem 0; color: var(--fg-muted); font-size: 0.85rem; }

  .cmd-row {
    display: flex;
    align-items: stretch;
    gap: 0.4rem;
  }
  .cmd {
    flex: 1;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.85rem;
    padding: 0.4rem 0.6rem;
    background: var(--bg-pane);
    border: 1px solid var(--border);
    border-radius: 5px;
    color: var(--fg);
    overflow-x: auto;
    white-space: nowrap;
  }
  .copy {
    font-size: 0.8rem;
    padding: 0.4rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 5px;
    cursor: pointer;
    min-width: 4rem;
  }
  .copy:hover { border-color: var(--accent); }

  .actions { display: flex; gap: 0.5rem; }
  .danger {
    font-size: 0.85rem;
    padding: 0.4rem 0.9rem;
    border: 1px solid #e64a4a;
    background: transparent;
    color: #e64a4a;
    border-radius: 5px;
    cursor: pointer;
  }
  .danger:hover { background: rgba(230, 74, 74, 0.1); }

  .err { color: #e64a4a; font-size: 0.8rem; margin: 0.4rem 0 0 0; }
</style>
