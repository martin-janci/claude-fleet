<script lang="ts">
  import { tick } from 'svelte';
  import { sessions, type SessionRow } from './sessions';
  import { killSession, renameSession, restartSession } from './sessions';
  import { projectById } from './projects';
  import { selectSession, clearSelection } from './selection';
  import { hostByAlias } from './hosts';
  import { accountByUuid, type AccountRow } from './accounts';
  import PromptComposer from './PromptComposer.svelte';
  import ReviewDialog from './ReviewDialog.svelte';

  let { session }: { session: SessionRow } = $props();

  // Look up the parent project (if any) for context.
  const parentProject = $derived(
    session.project_id === null
      ? null
      : ($projectById.get(session.project_id) ?? null),
  );

  const hostRow = $derived($hostByAlias.get(session.host_alias) ?? null);
  const accountRow = $derived(
    hostRow?.account_uuid ? ($accountByUuid.get(hostRow.account_uuid) ?? null) : null,
  );
  function accountText(a: AccountRow | null): string {
    if (!a) return '—';
    const email = a.email ?? a.uuid;
    return a.seat_tier ? `${email} (${a.seat_tier})` : email;
  }

  function accountForRow(s: SessionRow): AccountRow | null {
    if (!s.account_uuid) return null;
    return $accountByUuid.get(s.account_uuid) ?? null;
  }

  const related = $derived(
    session.project_id == null || session.worktree_key == null
      ? []
      : $sessions.filter(
          (s) =>
            s.id !== session.id &&
            s.project_id === session.project_id &&
            s.worktree_key === session.worktree_key,
        ),
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
  let actionError: string | null = $state(null);

  // Title rename state — same UX as the sidebar's inline rename.
  let renaming = $state(false);
  let renameValue = $state('');

  async function onCopy() {
    actionError = null;
    try {
      await navigator.clipboard.writeText(attachCommand);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch (e) {
      actionError = String(e);
    }
  }

  async function beginRename() {
    renaming = true;
    renameValue = session.tmux_name;
    actionError = null;
    await tick();
    const input = document.querySelector<HTMLInputElement>('[data-testid="details-rename"]');
    input?.focus();
    input?.select();
  }

  async function commitRename() {
    if (!renaming) return;
    const next = renameValue.trim();
    if (!next || next === session.tmux_name) {
      renaming = false;
      return;
    }
    const r = await renameSession(session.host_alias, session.tmux_name, next);
    if (!r.ok) {
      actionError = r.error.message;
      return;
    }
    selectSession(r.value);
    renaming = false;
  }

  function cancelRename() {
    renaming = false;
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

  async function onRestart() {
    actionError = null;
    const r = await restartSession(session.host_alias, session.tmux_name);
    if (!r.ok) actionError = r.error.message;
  }

  let composerOpen = $state(false);
  function openComposer() {
    composerOpen = true;
  }

  let reviewOpen = $state(false);

  const reviewedSource = $derived.by(() => {
    if (session.kind !== 'review' || session.reviews_session_id == null) return null;
    return $sessions.find((s) => s.id === session.reviews_session_id) ?? null;
  });

  const reviewsOfThis = $derived(
    $sessions.filter((s) => s.kind === 'review' && s.reviews_session_id === session.id),
  );

  let confirmingKill = $state(false);
  function askKill() {
    confirmingKill = true;
    actionError = null;
  }
  function cancelKill() {
    confirmingKill = false;
  }
  async function doKill() {
    confirmingKill = false;
    const r = await killSession(session.host_alias, session.tmux_name);
    if (r.ok) {
      clearSelection();
    } else {
      actionError = r.error.message;
    }
  }
</script>

<article class="details" data-testid="session-details">
  <header class="header">
    {#if renaming}
      <input
        class="title-input"
        data-testid="details-rename"
        bind:value={renameValue}
        onkeydown={onRenameKey}
        onblur={commitRename}
      />
    {:else}
      <h2
        class="title"
        ondblclick={beginRename}
        title="Double-click to rename"
      >{session.tmux_name}</h2>
    {/if}
    <div class="sub">
      <span class="host">{session.host_alias}</span>
      <span class="status status-{session.status}">{session.status}</span>
    </div>
  </header>

  <dl class="meta">
    <dt>Host</dt>
    <dd data-testid="session-host">{session.host_alias}</dd>

    <dt>Account</dt>
    <dd data-testid="session-account">{accountText(accountRow)}</dd>

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

    {#if reviewedSource}
      <dt class="meta-label">Reviewing</dt>
      <dd>
        <button class="link" onclick={() => selectSession(reviewedSource)} data-testid="reviewing-link">
          {reviewedSource.tmux_name}
        </button>
      </dd>
    {/if}
  </dl>

  {#if related.length > 0}
    <section class="related" data-testid="related-sessions">
      <h3>Related sessions ({related.length})</h3>
      <ul class="related-list">
        {#each related as r (r.id)}
          <li>
            <button
              class="related-row"
              data-testid="related-row"
              onclick={() => selectSession(r)}
            >
              <span class="host-badge">[{r.host_alias}]</span>
              <span class="account">{accountText(accountForRow(r))}</span>
              <span class="status-dot status-{r.status}" title={r.status}></span>
              <span class="sess-name">{r.tmux_name}</span>
              <span class="age">{formatRelative(r.last_activity_at)}</span>
            </button>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if reviewsOfThis.length > 0}
    <section class="related" data-testid="reviews-panel">
      <h3>Reviews ({reviewsOfThis.length})</h3>
      <ul class="related-list">
        {#each reviewsOfThis as r (r.id)}
          <li>
            <button
              class="related-row"
              data-testid="reviews-row"
              onclick={() => selectSession(r)}
            >
              <span class="host-badge">[{r.host_alias}]</span>
              <span class="account">{accountText(accountForRow(r))}</span>
              <span class="status-dot status-{r.status}" title={r.status}></span>
              <span class="sess-name">{r.tmux_name}</span>
              <span class="age">{formatRelative(r.last_activity_at)}</span>
            </button>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <section class="block">
    <h3>Attach from another terminal</h3>
    <div class="cmd-row">
      <code class="cmd" data-testid="attach-command">{attachCommand}</code>
      <button class="copy" onclick={onCopy} data-testid="copy-attach">
        {copied ? '✓ copied' : 'copy'}
      </button>
    </div>
  </section>

  {#if actionError}
    <p class="err">{actionError}</p>
  {/if}

  <section class="block actions">
    <button class="ghost" onclick={beginRename} data-testid="rename-from-details">
      ✎ Rename
    </button>
    <button class="ghost" onclick={onRestart} data-testid="restart-from-details">
      ↻ Restart
    </button>
    {#if session.kind !== 'shell'}
      <button class="ghost" onclick={openComposer} data-testid="send-prompt-from-details">
        → Send prompt
      </button>
    {/if}
    <button class="ghost" onclick={() => (reviewOpen = true)} data-testid="open-review">
      🔍 Review
    </button>
    <button class="danger" onclick={askKill} data-testid="kill-from-details">
      Kill session
    </button>
  </section>
</article>

{#if composerOpen}
  <PromptComposer source={session} onClose={() => (composerOpen = false)} />
{/if}

{#if reviewOpen}
  <ReviewDialog source={session} onClose={() => (reviewOpen = false)} />
{/if}

{#if confirmingKill}
  <div class="modal-backdrop" onclick={cancelKill} role="presentation">
    <div class="confirm" onclick={(e) => e.stopPropagation()} role="presentation">
      <h3>Kill session?</h3>
      <p>This will kill the tmux session <code>{session.tmux_name}</code> and lose any running claude state inside it. Continue?</p>
      <div class="confirm-actions">
        <button onclick={cancelKill}>Cancel</button>
        <button class="danger" onclick={doKill} data-testid="confirm-kill-details">Kill</button>
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
  .header { display: flex; flex-direction: column; gap: 0.3rem; }
  .title {
    margin: 0;
    font-size: 1.1rem;
    font-weight: 600;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    cursor: text;
    padding: 0.05rem 0;
    border-radius: 3px;
  }
  .title:hover { background: var(--bg-pane); }
  .title-input {
    font-size: 1.1rem;
    font-weight: 600;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    padding: 0.1rem 0.3rem;
    border: 1px solid var(--accent);
    background: var(--bg);
    color: var(--fg);
    border-radius: 4px;
    outline: none;
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

  .actions { display: flex; gap: 0.5rem; flex-wrap: wrap; }
  .ghost {
    font-size: 0.85rem;
    padding: 0.35rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 5px;
    cursor: pointer;
  }
  .ghost:hover { border-color: var(--accent); }
  .danger {
    font-size: 0.85rem;
    padding: 0.35rem 0.8rem;
    border: 1px solid #e64a4a;
    background: transparent;
    color: #e64a4a;
    border-radius: 5px;
    cursor: pointer;
  }
  .danger:hover { background: rgba(230, 74, 74, 0.1); }

  .err { color: #e64a4a; font-size: 0.8rem; margin: 0; }

  .link {
    background: transparent;
    border: none;
    padding: 0;
    color: var(--accent);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.9rem;
    cursor: pointer;
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .link:hover { opacity: 0.8; }

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
  .confirm-actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .confirm-actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .confirm-actions button.danger {
    color: #e64a4a;
    border-color: #e64a4a;
  }
  .confirm-actions button.danger:hover { background: rgba(230, 74, 74, 0.12); }

  .related {
    border-top: 1px solid var(--border);
    padding-top: 0.6rem;
    margin-top: 0.6rem;
  }
  .related h3 {
    font-size: 0.7rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin: 0 0 0.4rem 0;
  }
  .related-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .related-row {
    width: 100%;
    text-align: left;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.35rem 0.5rem;
    color: var(--fg);
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.82rem;
  }
  .related-row:hover {
    border-color: var(--accent);
    background: var(--bg-pane);
  }
  .related .host-badge {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
  }
  .related .account {
    color: var(--fg-muted);
    font-size: 0.75rem;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .related .sess-name {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.78rem;
  }
  .related .age {
    color: var(--fg-muted);
    font-size: 0.7rem;
  }
</style>
