<script lang="ts">
  import { tick } from 'svelte';
  import { sessions, type SessionRow, type SafeKillInspection } from './sessions';
  import { formatCostMicros, formatTokens, sessionUsageTokens } from './sessions';
  import {
    killSession,
    renameSession,
    restartSession,
    repairSession,
    recreateSession,
    safeKillSession,
    inspectSafeKill,
    discardKillSession,
  } from './sessions';
  import { moveSession } from './moveSession';
  import { projectById } from './projects';
  import { selectSession, clearSelection } from './selection';
  import { hosts, hostByAlias } from './hosts';
  import { accountByUuid, type AccountRow } from './accounts';
  import PromptComposer from './PromptComposer.svelte';
  import ReviewDialog from './ReviewDialog.svelte';
  import Modal from './Modal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import TasksPanel from './TasksPanel.svelte';
  import { push, pushError } from './toasts';
  import {
    ciStatusColor,
    ciStatusLabel,
    claudeStatusColor,
    claudeStatusLabel,
    contextColor,
    contextLevel,
    formatElapsed,
    sessionStart,
    stuckKindLabel,
    STUCK_COLOR,
  } from './attention';

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

  // Coarse clock for the elapsed / idle counters (a minute-level readout
  // does not need a per-second re-render).
  let nowSec = $state(Math.floor(Date.now() / 1000));
  $effect(() => {
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });
  const ctxLevel = $derived(contextLevel(session.context_pct));

  // Title rename state — same UX as the sidebar's inline rename.
  let renaming = $state(false);
  let renameValue = $state('');

  async function onCopy() {
    try {
      await navigator.clipboard.writeText(attachCommand);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch (e) {
      push({ kind: 'error', code: 'E_CLIPBOARD', message: `Copy failed: ${String(e)}` });
    }
  }

  async function beginRename() {
    renaming = true;
    renameValue = session.tmux_name;
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
      pushError(r.error, 'Rename failed');
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
    const r = await restartSession(session.host_alias, session.tmux_name);
    if (!r.ok) pushError(r.error, 'Restart failed');
  }

  // Make the worktree directory + tmux pane healthy again (deleted dir,
  // pruned registration, moved checkout, dead tmux). The backend emits the
  // row events; the toast just says what it did.
  let repairing = $state(false);
  async function onRepair() {
    if (repairing) return;
    repairing = true;
    // Explicit: the user asked, so this may unregister a stale entry, adopt a
    // moved checkout, recreate the branch and respawn a live pane.
    const r = await repairSession(session.id, { explicit: true });
    repairing = false;
    if (!r.ok) {
      pushError(r.error, 'Repair failed');
      return;
    }
    const rep = r.value;
    if (rep.actions.length === 0) {
      const notes = rep.warnings.length > 0 ? ` (${rep.warnings.join('; ')})` : '';
      push({ kind: 'success', message: `Workspace is healthy: ${rep.cwd}${notes}` });
      return;
    }
    const branch = rep.branch_source ? ` [branch: ${rep.branch_source}]` : '';
    push({ kind: 'success', message: `Repaired workspace: ${rep.actions.join('; ')}${branch}` });
    if (rep.tmux === 'created') {
      // A recreated tmux session needs a fresh attach (same tmux_name, so
      // the selection effect would not fire on its own).
      selectSession(null);
      await tick();
      selectSession(session);
    }
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
  let confirmingSafeKill = $state(false);
  let confirmingRecreate = $state(false);

  // Safe-remove inspection state. `null` while loading; populated once the
  // pre-flight git inspect returns. `safe_to_remove` short-circuits the
  // Claude prompt entirely.
  let inspection: SafeKillInspection | null = $state(null);
  let inspectError: string | null = $state(null);
  let busy = $state(false);

  async function askSafeKill() {
    inspection = null;
    inspectError = null;
    confirmingSafeKill = true;
    const r = await inspectSafeKill(session.host_alias, session.tmux_name);
    if (r.ok) {
      inspection = r.value;
    } else {
      inspectError = r.error.message;
    }
  }
  function cancelSafeKill() {
    if (busy) return;
    confirmingSafeKill = false;
    inspection = null;
    inspectError = null;
  }

  // "Let Claude commit it" path: current behavior — send the marker-baked
  // prompt and wait for the Stop hook to finalize.
  async function doSafeKillViaClaude() {
    busy = true;
    const r = await safeKillSession(session.host_alias, session.tmux_name);
    busy = false;
    if (!r.ok) {
      pushError(r.error, 'Safe remove failed');
      return;
    }
    confirmingSafeKill = false;
    inspection = null;
  }

  // Clean+pushed fast path: backend just removes the worktree + kills tmux,
  // no Claude involvement. `force=false` so a surprise dirty file errors out.
  async function doDirectRemove() {
    busy = true;
    const r = await discardKillSession(session.host_alias, session.tmux_name, false);
    busy = false;
    if (r.ok) {
      confirmingSafeKill = false;
      inspection = null;
      clearSelection();
    } else {
      pushError(r.error, 'Remove failed');
    }
  }

  // Explicit discard: user has seen the dirty list / unpushed warning and
  // chose to drop the work anyway.
  async function doDiscardAndKill() {
    busy = true;
    const r = await discardKillSession(session.host_alias, session.tmux_name, true);
    busy = false;
    if (r.ok) {
      confirmingSafeKill = false;
      inspection = null;
      clearSelection();
    } else {
      pushError(r.error, 'Discard & kill failed');
    }
  }
  function askKill() {
    confirmingKill = true;
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
      pushError(r.error, 'Kill failed');
    }
  }

  function askRecreate() {
    confirmingRecreate = true;
  }

  function cancelRecreate() {
    confirmingRecreate = false;
  }

  // Move to host…: continue this conversation on another host (same branch,
  // same Claude session id). Only a worktree-backed work session with a
  // Claude id can move; the backend refuses dirty or unpushed worktrees.
  const canMove = $derived(
    session.kind === 'work' && session.worktree_id !== null && session.claude_session_id !== null,
  );
  const moveTargets = $derived(
    $hosts.filter(
      (h) =>
        h.alias !== session.host_alias &&
        !h.hidden &&
        h.reachable &&
        (h.provisioned || h.alias === 'local'),
    ),
  );
  let moveOpen = $state(false);
  let moveTarget = $state('');
  let moveKeepSource = $state(false);
  let moving = $state(false);

  function openMove() {
    moveTarget = moveTargets[0]?.alias ?? '';
    moveKeepSource = false;
    moveOpen = true;
  }

  function closeMove() {
    if (!moving) moveOpen = false;
  }

  async function doMove() {
    if (moving || !moveTarget) return;
    moving = true;
    const r = await moveSession(session.id, moveTarget, { keepSource: moveKeepSource });
    moving = false;
    if (!r.ok) {
      pushError(r.error, 'Move failed');
      return;
    }
    moveOpen = false;
    const rep = r.value;
    const kept = rep.source_killed ? '' : '; the source keeps running';
    const notes = rep.warnings.length > 0 ? ` (${rep.warnings.join('; ')})` : '';
    push({
      kind: 'success',
      message: `Moved to ${rep.to_host} as ${rep.tmux_name}${kept}${notes}`,
    });
    selectSession(rep.target);
  }

  async function doRecreate() {
    confirmingRecreate = false;
    const r = await recreateSession(session.id);
    if (!r.ok) {
      pushError(r.error, 'Recreate failed');
      return;
    }
    // kill-session severed the PTY; same tmux_name won't auto-reopen. This
    // panel shows the selected session, so force a re-attach.
    selectSession(null);
    await tick();
    selectSession(r.value);
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
    {#if session.friendly_name}
      <p class="friendly" data-testid="details-friendly-name">{session.friendly_name}</p>
    {/if}
    <div class="sub">
      <span class="host">{session.host_alias}</span>
      <span class="status status-{session.status}">{session.status}</span>
      {#if session.stuck_kind}
        <span
          class="chip"
          data-testid="details-stuck"
          style="background: {STUCK_COLOR}22; color: {STUCK_COLOR}; border-color: {STUCK_COLOR}66;"
          title={session.current_activity ?? undefined}
        >⚠ stuck: {stuckKindLabel(session.stuck_kind)}{#if session.stuck_since !== null} · {formatElapsed(session.stuck_since, nowSec)}{/if}</span>
      {:else if session.claude_status}
        <span
          class="chip"
          data-testid="details-claude-status"
          style="background: {claudeStatusColor(session.claude_status)}22; color: {claudeStatusColor(session.claude_status)}; border-color: {claudeStatusColor(session.claude_status)}44;"
          title={session.current_activity ?? undefined}
        >{claudeStatusLabel(session.claude_status)}</span>
      {/if}
      {#if ctxLevel !== null && session.context_pct !== null}
        <span
          class="chip"
          data-testid="details-context"
          data-level={ctxLevel}
          style="color: {contextColor(ctxLevel)}; border-color: {contextColor(ctxLevel)}55;"
          title="Context window used"
        >ctx {Math.round(session.context_pct)}%</span>
      {/if}
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

    <dt>Elapsed</dt>
    <dd data-testid="details-elapsed" title={session.started_at === null ? 'since tmux created the session (fleet did not start it)' : 'since fleet started the session'}>
      {formatElapsed(sessionStart(session), nowSec)}
    </dd>

    {#if session.last_turn_at !== null}
      <dt>Last turn</dt>
      <dd data-testid="details-last-turn">{formatRelative(session.last_turn_at)}</dd>
    {/if}

    {#if session.last_prompt}
      <dt>Last prompt</dt>
      <dd class="last-prompt" data-testid="details-last-prompt">{session.last_prompt}</dd>
    {/if}

    {#if sessionUsageTokens(session) > 0}
      <dt>Usage</dt>
      <dd
        data-testid="details-usage"
        title="Estimated from the Claude Code transcript's token counts and a built-in per-model price table (override: usage.prices_json). Not a bill."
      >
        <span data-testid="details-cost">{#if (session.usage_cost_micros ?? 0) > 0}{formatCostMicros(session.usage_cost_micros)} estimated{:else}unpriced ({session.usage_model ?? 'unknown model'}){/if}</span>
        <span class="muted">· {formatTokens(session.usage_input_tokens)} in · {formatTokens(session.usage_output_tokens)} out · {formatTokens(session.usage_cache_write_tokens)} cache write · {formatTokens(session.usage_cache_read_tokens)} cache read{#if session.usage_model} · {session.usage_model}{/if}</span>
      </dd>
    {/if}

    {#if session.pr_url}
      <dt>Pull request</dt>
      <dd data-testid="details-pr">
        <a class="pr-link" href={session.pr_url} target="_blank" rel="noreferrer">{session.pr_url.replace(/^https:\/\/github\.com\//, '')}</a>
        {#if session.ci_status}
          <span
            class="chip"
            data-testid="details-ci"
            style="color: {ciStatusColor(session.ci_status)}; border-color: {ciStatusColor(session.ci_status)}55;"
            title="CI checks: {session.ci_status}"
          >{ciStatusLabel(session.ci_status)}</span>
        {/if}
      </dd>
    {/if}

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

  <TasksPanel sessionId={session.id} />

  <section class="block">
    <h3>Attach from another terminal</h3>
    <div class="cmd-row">
      <code class="cmd" data-testid="attach-command">{attachCommand}</code>
      <button class="copy" onclick={onCopy} data-testid="copy-attach">
        {copied ? '✓ copied' : 'copy'}
      </button>
    </div>
  </section>

  <section class="block actions">
    <button class="ghost" onclick={beginRename} data-testid="rename-from-details">
      ✎ Rename
    </button>
    <button class="ghost" onclick={onRestart} data-testid="restart-from-details">
      ↻ Restart
    </button>
    {#if session.kind !== 'bg' && session.project_id !== null}
      <button
        class="ghost"
        onclick={onRepair}
        disabled={repairing}
        title="Recreate a deleted worktree directory, re-register it with git, and respawn the pane in it"
        data-testid="repair-from-details"
      >
        🩹 Repair workspace
      </button>
    {/if}
    {#if session.kind !== 'shell'}
      <button class="ghost" onclick={openComposer} data-testid="send-prompt-from-details">
        → Send prompt
      </button>
    {/if}
    <button class="ghost" onclick={() => (reviewOpen = true)} data-testid="open-review">
      🔍 Review
    </button>
    <button class="ghost" onclick={askRecreate} data-testid="recreate-from-details">
      ♻ Recreate
    </button>
    {#if canMove}
      <button
        class="ghost"
        onclick={openMove}
        title="Continue this conversation on another host: same branch, same Claude session"
        data-testid="move-from-details"
      >
        ⇄ Move to host…
      </button>
    {/if}
    {#if session.kind !== 'shell' && session.status === 'running' && session.safe_kill_state !== 'requested'}
      <button class="ghost" onclick={askSafeKill} data-testid="safe-kill-from-details">
        ⏏ Safe remove
      </button>
    {/if}
    <button class="danger" onclick={askKill} data-testid="kill-from-details">
      Kill session
    </button>
  </section>

  {#if session.safe_kill_state === 'requested'}
    <p class="safe-kill-pill pending" data-testid="safe-kill-pending">
      Safe-remove in progress: asked Claude to commit + push. Will delete the
      worktree and kill the session once it reports back.
    </p>
  {:else if session.safe_kill_state === 'failed'}
    <p class="safe-kill-pill failed" data-testid="safe-kill-failed">
      Safe-remove failed: {session.safe_kill_detail ?? 'no reason given'}.
      Resolve in the session, then retry, or use <strong>Kill session</strong>.
    </p>
  {:else if session.safe_kill_state === 'ready'}
    <p class="safe-kill-pill ready" data-testid="safe-kill-ready">
      Safe-remove ready — finalizing.
    </p>
  {/if}
</article>

{#if composerOpen}
  <PromptComposer source={session} onClose={() => (composerOpen = false)} />
{/if}

{#if reviewOpen}
  <ReviewDialog source={session} onClose={() => (reviewOpen = false)} />
{/if}

{#if confirmingKill}
  <ConfirmDialog
    title="Kill session?"
    confirmLabel="Kill"
    danger
    onconfirm={doKill}
    oncancel={cancelKill}
    confirmTestId="confirm-kill-details"
  >
    This will kill the tmux session <code>{session.tmux_name}</code> on
    <code>{session.host_alias}</code> and lose any running claude state inside it. Continue?
  </ConfirmDialog>
{/if}

{#if confirmingSafeKill}
  <Modal label="Safe remove {session.tmux_name}" onclose={cancelSafeKill} width="480px" testid="safe-kill-dialog">
    <div class="confirm wide">
      <h3>Safe remove <code>{session.tmux_name}</code>?</h3>

      {#if inspection === null && inspectError === null}
        <p class="muted">Inspecting worktree…</p>
        <div class="confirm-actions">
          <button onclick={cancelSafeKill}>Cancel</button>
        </div>
      {:else if inspectError}
        <p>
          Couldn't inspect the worktree: <code>{inspectError}</code>. You can
          still ask Claude to persist the work safely.
        </p>
        <div class="confirm-actions">
          <button onclick={cancelSafeKill}>Cancel</button>
          <button
            disabled={busy}
            onclick={doSafeKillViaClaude}
            data-testid="confirm-safe-kill-claude"
          >Ask Claude to commit + push</button>
        </div>
      {:else if inspection && inspection.safe_to_remove}
        <p>
          Worktree is clean and branch
          <code>{inspection.branch ?? '(detached)'}</code> is up-to-date with
          <code>{inspection.upstream ?? 'origin'}</code>. Safe to remove
          immediately.
        </p>
        <div class="confirm-actions">
          <button disabled={busy} onclick={cancelSafeKill}>Cancel</button>
          <button
            class="primary"
            disabled={busy}
            onclick={doDirectRemove}
            data-testid="confirm-safe-kill-direct"
          >Remove worktree + kill</button>
        </div>
      {:else if inspection}
        {#if !inspection.has_worktree}
          <p>
            This session has no tracked worktree. Asking Claude to commit and
            push will surface any unsaved work; the session is then killed.
          </p>
        {:else}
          <div class="inspect-box">
            <p class="inspect-line">
              Branch: <code>{inspection.branch ?? '(detached)'}</code>
              {#if inspection.upstream}
                → <code>{inspection.upstream}</code>
              {:else}
                <span class="muted">(no upstream — not pushed)</span>
              {/if}
            </p>
            {#if inspection.upstream && inspection.unpushed_commits > 0}
              <p class="inspect-line warn">
                {inspection.unpushed_commits} unpushed commit{inspection.unpushed_commits === 1 ? '' : 's'}
                on this branch.
              </p>
            {/if}
            {#if inspection.dirty_files.length > 0}
              <p class="inspect-line warn">
                {inspection.dirty_files.length} uncommitted file{inspection.dirty_files.length === 1 ? '' : 's'}:
              </p>
              <ul class="dirty-list" data-testid="dirty-files">
                {#each inspection.dirty_files.slice(0, 20) as f (f.path)}
                  <li>
                    <code class="status-code">{f.status}</code>
                    <span>{f.path}</span>
                  </li>
                {/each}
                {#if inspection.dirty_files.length > 20}
                  <li class="muted">… and {inspection.dirty_files.length - 20} more</li>
                {/if}
              </ul>
            {/if}
          </div>
          <p class="muted small">
            "Let Claude commit it" sends a prompt asking Claude to commit + push
            (to <code>main</code> or a PR) before fleet removes the worktree.
            "Discard &amp; kill" force-removes the worktree and kills the
            session — local-only changes will be lost.
          </p>
        {/if}
        <div class="confirm-actions">
          <button disabled={busy} onclick={cancelSafeKill}>Cancel</button>
          <button
            disabled={busy}
            onclick={doSafeKillViaClaude}
            data-testid="confirm-safe-kill-claude"
          >Let Claude commit it</button>
          <button
            class="danger"
            disabled={busy}
            onclick={doDiscardAndKill}
            data-testid="confirm-safe-kill-discard"
          >Discard &amp; kill</button>
        </div>
      {/if}
    </div>
  </Modal>
{/if}

{#if moveOpen}
  <Modal title="Move {session.tmux_name} to another host" onclose={closeMove} width="480px" testid="move-dialog">
    <p class="move-note">
      Copies this conversation to the chosen host, creates the worktree there from the same
      branch and resumes it with <code>--resume</code>. This session is killed only once the
      new one is running. The worktree must be clean and pushed; nothing is pushed for you.
    </p>
    {#if moveTargets.length === 0}
      <p class="move-note" data-testid="move-no-targets">No other reachable, provisioned host.</p>
    {:else}
      <label class="move-field">
        Target host
        <select bind:value={moveTarget} disabled={moving} data-testid="move-target">
          {#each moveTargets as h (h.alias)}
            <option value={h.alias}>{h.alias}</option>
          {/each}
        </select>
      </label>
      <label class="move-field">
        <input type="checkbox" bind:checked={moveKeepSource} disabled={moving} data-testid="move-keep-source" />
        Keep this session running
      </label>
    {/if}
    <div class="move-buttons">
      <button onclick={closeMove} disabled={moving}>Cancel</button>
      <button onclick={doMove} disabled={moving || !moveTarget} data-testid="confirm-move">
        {moving ? 'Moving…' : 'Move'}
      </button>
    </div>
  </Modal>
{/if}

{#if confirmingRecreate}
  <ConfirmDialog
    title="Recreate session?"
    confirmLabel="Recreate"
    danger
    onconfirm={doRecreate}
    oncancel={cancelRecreate}
    confirmTestId="confirm-recreate-details"
  >
    This kills the tmux session <code>{session.tmux_name}</code> on
    <code>{session.host_alias}</code> and the running claude state inside it, then
    starts a fresh session in the same worktree. Continue?
  </ConfirmDialog>
{/if}

<style>
  .move-note { margin: 0 0 0.75rem; font-size: 0.85rem; color: var(--fg-muted); }
  .move-field { display: flex; gap: 0.5rem; align-items: center; margin-bottom: 0.6rem; font-size: 0.85rem; }
  .move-buttons { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.75rem; }
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
  .sub { display: flex; gap: 0.5rem; align-items: center; font-size: 0.75rem; flex-wrap: wrap; }
  .friendly { margin: 0; font-size: 0.85rem; color: var(--fg-muted); }
  .chip {
    padding: 0.1rem 0.4rem;
    border-radius: 999px;
    border: 1px solid;
    font-size: 0.65rem;
    white-space: nowrap;
  }
  .last-prompt {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: 0.8rem;
    color: var(--fg-muted);
    max-height: 6rem;
    overflow: auto;
  }
  .pr-link { color: var(--accent); font-size: 0.85rem; overflow-wrap: anywhere; }
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

  .safe-kill-pill {
    margin: 0;
    padding: 0.4rem 0.6rem;
    border-radius: 6px;
    font-size: 0.8rem;
    line-height: 1.35;
  }
  .safe-kill-pill.pending {
    background: rgba(110, 160, 230, 0.14);
    color: rgba(140, 180, 240, 1);
    border: 1px solid rgba(110, 160, 230, 0.4);
  }
  .safe-kill-pill.failed {
    background: rgba(230, 74, 74, 0.12);
    color: #e64a4a;
    border: 1px solid rgba(230, 74, 74, 0.4);
  }
  .safe-kill-pill.ready {
    background: rgba(60, 180, 90, 0.15);
    color: rgba(80, 200, 110, 1);
    border: 1px solid rgba(60, 180, 90, 0.4);
  }

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

  /* Safe-remove body (lives inside Modal, which owns the box chrome). */
  .confirm {
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
  .confirm-actions { display: flex; gap: 0.4rem; justify-content: flex-end; flex-wrap: wrap; }
  .confirm .primary {
    background: var(--accent);
    color: white;
    border: 1px solid var(--accent);
    border-radius: 4px;
    padding: 0.3rem 0.7rem;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .confirm .primary:disabled { opacity: 0.6; cursor: not-allowed; }
  .confirm button:disabled { opacity: 0.6; cursor: not-allowed; }
  .inspect-box {
    background: var(--bg-pane);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 0.5rem 0.7rem;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .inspect-line { margin: 0; font-size: 0.8rem; color: var(--fg); }
  .inspect-line.warn { color: #d29b4a; }
  .dirty-list {
    margin: 0.2rem 0 0 0;
    padding: 0;
    list-style: none;
    max-height: 9rem;
    overflow-y: auto;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.75rem;
  }
  .dirty-list li {
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
    padding: 0.05rem 0;
  }
  .status-code {
    color: #d29b4a;
    width: 2ch;
    flex: 0 0 auto;
    white-space: pre;
  }
  .small { font-size: 0.75rem; }
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
