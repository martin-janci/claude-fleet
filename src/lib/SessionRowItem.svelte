<script lang="ts">
  import {
    restartSession,
    recreateSession,
    dismissGhostSession,
    showFriendlyNames,
    formatCostMicros,
    formatTokens,
    sessionUsageTokens,
    type SessionRow,
  } from './sessions';
  import { selectedSession } from './selection';
  import { forgetSessionUi } from './session_ui';
  import { hostByAlias } from './hosts';
  import { hintAnchor } from './hints';
  import {
    claudeStatusColor,
    claudeStatusLabel,
    contextColor,
    contextLevel,
    ciStatusColor,
    ciStatusLabel,
    stuckKindLabel,
    STUCK_COLOR,
  } from './attention';
  import { pushError } from './toasts';
  import { rowMeta, timeAgo } from './session_status';
  import PeekPanel from './PeekPanel.svelte';

  // Rename, selection and peek state stay in the Sidebar (they must survive a
  // sessions store refresh); the row gets them as props and calls back.
  let {
    sess,
    selectMode,
    isChecked,
    isRenaming,
    renameMode = null,
    renameValue = $bindable(),
    renameInput = $bindable(),
    renameError,
    relatedCount,
    nowSec,
    peek,
    onSelectSession,
    onKeySession,
    toggleSelected,
    beginRename,
    beginLabelEdit,
    onRenameKey,
    commitRename,
    askRecreate,
    askKill,
    doPeek,
    closePeek,
  }: {
    sess: SessionRow;
    selectMode: boolean;
    isChecked: boolean;
    isRenaming: boolean;
    /** What the inline editor changes: the display label or the tmux name. */
    renameMode?: 'label' | 'tmux' | null;
    renameValue: string;
    renameInput: HTMLInputElement | undefined;
    renameError: string | null;
    relatedCount: number;
    nowSec: number;
    peek: string | 'loading' | null | undefined;
    onSelectSession: (sess: SessionRow, e?: MouseEvent) => void;
    onKeySession: (e: KeyboardEvent, sess: SessionRow) => void;
    toggleSelected: (sess: SessionRow) => void;
    beginRename: (sess: SessionRow, e?: Event) => unknown;
    beginLabelEdit: (sess: SessionRow, e?: Event) => unknown;
    onRenameKey: (e: KeyboardEvent) => void;
    commitRename: () => unknown;
    askRecreate: (sess: SessionRow, e?: Event) => void;
    askKill: (sess: SessionRow, e?: Event) => void;
    doPeek: (sess: SessionRow) => unknown;
    closePeek: (sessId: number) => void;
  } = $props();

  const sessSelected = $derived($selectedSession?.id === sess.id);
  const ctxLevel = $derived(contextLevel(sess.context_pct));
  const meta = $derived(rowMeta(sess, nowSec));

  async function doRestart(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    const r = await restartSession(sess.host_alias, sess.tmux_name);
    if (!r.ok) pushError(r.error, 'Restart failed');
  }

  async function doRecreate(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    const r = await recreateSession(sess.id);
    if (!r.ok) pushError(r.error, 'Recreate failed');
  }

  async function doDismissGhost(sess: SessionRow, e?: Event) {
    e?.stopPropagation();
    const r = await dismissGhostSession(sess.id);
    if (!r.ok) {
      pushError(r.error, 'Dismiss failed');
      return;
    }
    forgetSessionUi(sess.host_alias, sess.tmux_name);
  }

  function hostIsReachable(alias: string): boolean {
    return $hostByAlias.get(alias)?.reachable ?? false;
  }
</script>

<div
  class="sess-row"
  class:selected={sessSelected}
  class:renaming={isRenaming}
  class:checked={isChecked}
  class:stuck={sess.stuck_kind !== null}
  data-testid="sess-row"
  data-session-id={sess.id}
  data-stuck={sess.stuck_kind ?? undefined}
  role="button"
  tabindex="0"
  ondblclick={(e) => sess.status !== 'ghost' && beginLabelEdit(sess, e)}
  onclick={(e) => !isRenaming && (sess.status !== 'ghost' || selectMode) && onSelectSession(sess, e)}
  onkeydown={(e) => !isRenaming && (sess.status !== 'ghost' || selectMode) && onKeySession(e, sess)}
  use:hintAnchor={{ id: 'session-actions', when: !!sess.claude_session_id && sess.status !== 'ghost' }}
>
  {#if selectMode}
    <!-- a11y smell, known: an <input> nested in a role="button" row. The
         row is the click target for open/toggle; the box is a visible
         affordance for the same toggle and stops propagation so the two
         never double-fire. Splitting the row into a real <button> plus a
         sibling checkbox is the proper fix (F5 sidebar split). -->
    <input
      type="checkbox"
      class="select-box"
      checked={isChecked}
      data-testid="select-box"
      aria-label="Select {sess.tmux_name}"
      onclick={(e) => { e.stopPropagation(); toggleSelected(sess); }}
    />
  {/if}
  {#if isRenaming}
    <input
      bind:this={renameInput}
      class="rename-input"
      data-testid={renameMode === 'label' ? 'label-input' : 'rename-input'}
      aria-label={renameMode === 'label'
        ? `Label for ${sess.tmux_name} (empty clears it)`
        : `New tmux session name for ${sess.tmux_name}`}
      placeholder={renameMode === 'label' ? sess.tmux_name : undefined}
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
      {#if relatedCount > 0}
        <span
          class="related-badge"
          data-testid="related-badge"
          role="img"
          title="{relatedCount} related session(s)"
          aria-label="{relatedCount} related sessions"
        >🔗{relatedCount}</span>
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
      <span class="sess-main">
        <span class="sess-name" title={$showFriendlyNames && sess.friendly_name ? sess.tmux_name : undefined}>{
          $showFriendlyNames && sess.friendly_name ? sess.friendly_name : sess.tmux_name
        }</span>
        {#if $showFriendlyNames && sess.friendly_name}
          <span class="sess-secondary" data-testid="sess-tmux-name">{sess.tmux_name}</span>
        {/if}
        {#if meta}
          <span class="sess-meta" data-testid="sess-meta" title={sess.last_prompt ?? undefined}>{meta}</span>
        {/if}
      </span>
      {#if sess.stuck_kind}
        <!-- Stuck outranks claude_status: one red chip, no green "working"
             next to it to soften the signal. -->
        <span
          class="claude-chip stuck-chip"
          data-testid="stuck-chip"
          style="background: {STUCK_COLOR}22; color: {STUCK_COLOR}; border-color: {STUCK_COLOR}66;"
          title="Stuck: {stuckKindLabel(sess.stuck_kind)}{sess.current_activity ? ' — ' + sess.current_activity : ''}"
        >⚠ stuck: {stuckKindLabel(sess.stuck_kind)}</span>
      {:else if sess.claude_status}
        <span
          class="claude-chip"
          data-testid="claude-chip"
          style="background: {claudeStatusColor(sess.claude_status)}22; color: {claudeStatusColor(sess.claude_status)}; border-color: {claudeStatusColor(sess.claude_status)}44;"
          title="Claude: {sess.claude_status}{sess.current_activity ? ' — ' + sess.current_activity : ''}"
        >{claudeStatusLabel(sess.claude_status)}</span>
      {/if}
      {#if ctxLevel !== null && sess.context_pct !== null}
        <span
          class="ctx-badge ctx-{ctxLevel}"
          data-testid="context-badge"
          data-level={ctxLevel}
          style="color: {contextColor(ctxLevel)}; border-color: {contextColor(ctxLevel)}55;"
          title="Context window {Math.round(sess.context_pct)}% used"
          role="meter"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={Math.round(sess.context_pct)}
          aria-label="context usage"
        ><span class="ctx-bar" style="width: {Math.min(100, Math.max(0, sess.context_pct))}%; background: {contextColor(ctxLevel)};"></span><span class="ctx-pct">{Math.round(sess.context_pct)}%</span></span>
      {/if}
      {#if sessionUsageTokens(sess) > 0}
        {@const priced = (sess.usage_cost_micros ?? 0) > 0}
        <span
          class="cost-badge"
          data-testid="cost-badge"
          data-priced={priced}
          title={priced
            ? `Estimated cost ${formatCostMicros(sess.usage_cost_micros)} · ${formatTokens(sessionUsageTokens(sess))} tokens${sess.usage_model ? ' · ' + sess.usage_model : ''}`
            : `Unpriced: no price for ${sess.usage_model ?? 'an unknown model'} · ${formatTokens(sessionUsageTokens(sess))} tokens`}
        >{priced ? formatCostMicros(sess.usage_cost_micros) : 'unpriced'}</span>
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
        {#if sess.ci_status}
          <span
            class="ci-badge"
            data-testid="ci-badge"
            style="color: {ciStatusColor(sess.ci_status)};"
            title="CI checks: {sess.ci_status}"
          >{ciStatusLabel(sess.ci_status)}</span>
        {/if}
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
        <button
          class="icon-btn small"
          data-testid="edit-label"
          onclick={(e) => beginLabelEdit(sess, e)}
          title="Edit label (double-click the row)"
          aria-label="Edit label"
        >🏷</button>
        <button
          class="icon-btn small"
          data-testid="rename-tmux"
          onclick={(e) => beginRename(sess, e)}
          title="Rename tmux session"
          aria-label="Rename tmux session"
        >✎</button>
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
{#if peek !== undefined && peek !== null}
  <PeekPanel tmuxName={sess.tmux_name} {peek} onClose={() => closePeek(sess.id)} />
{/if}

<style>
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

  .select-box { margin: 0; flex-shrink: 0; }
  .sess-row.checked { outline: 1px solid var(--accent); }
  .sess-row.stuck { background: rgba(230, 74, 74, 0.06); }

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

  .err { color: #e64a4a; font-size: 0.8rem; padding: 0.2rem 0; margin: 0; }
  .inline-err { padding-left: 1.6rem; font-size: 0.75rem; }

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
  .stuck-chip { font-weight: 600; }
  .ctx-badge {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 2.6rem;
    height: 0.95rem;
    font-size: 0.6rem;
    border: 1px solid;
    border-radius: 3px;
    overflow: hidden;
    flex-shrink: 0;
    font-variant-numeric: tabular-nums;
  }
  .ctx-bar {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    opacity: 0.25;
  }
  .ctx-pct { position: relative; }
  .cost-badge {
    font-size: 0.6rem;
    flex-shrink: 0;
    white-space: nowrap;
    opacity: 0.75;
    font-variant-numeric: tabular-nums;
  }
  .ci-badge {
    font-size: 0.6rem;
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

  .sess-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.05rem;
  }
  .sess-name {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.8rem;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sess-secondary,
  .sess-meta {
    font-size: 0.65rem;
    color: var(--fg-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sess-secondary { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }

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

  .peek-btn {
    opacity: 0.6;
  }
  .peek-btn:hover { opacity: 1; }
</style>
