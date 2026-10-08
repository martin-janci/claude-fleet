<script lang="ts">
  // A session row's details line (the "Row details" toggle), split out of
  // SessionRowItem (redesign step 3.6): host, the name line 1 does not show,
  // elapsed, context, cost, effort, PR and CI, and the last prompt. This is
  // the Comfortable row's second line; a Compact row draws SessionRowMeta.
  import {
    formatCostMicros,
    formatTokens,
    sessionUsageTokens,
    type SessionRow,
  } from './sessions';
  import { contextColor, contextLevel, ciStatusColor, ciStatusLabel } from './attention';
  import { rowElapsed, rowPrompt, timeAgo } from './session_status';
  import { isStale } from './evidence';

  let {
    sess,
    nowSec,
    secondaryName,
  }: {
    sess: SessionRow;
    nowSec: number;
    /** What line 1 does not name: the tmux name under a friendly name, or
     *  the worktree. */
    secondaryName: string | null;
  } = $props();

  const ctxLevel = $derived(contextLevel(sess.context_pct));
  const elapsed = $derived(rowElapsed(sess, nowSec));
  // An old reading describes the past (result evidence): dim the badge
  // rather than let yesterday's `passing` look current.
  const ciStale = $derived(isStale(sess.pr_checked_at, nowSec));
  const promptText = $derived(rowPrompt(sess));
</script>

<div class="sess-details" data-testid="sess-details">
  <span class="host-badge" data-testid="host-badge" aria-label="host {sess.host_alias}">{sess.host_alias}</span>
  {#if secondaryName}
    <span class="sep" aria-hidden="true">·</span>
    <span class="sess-secondary" data-testid="sess-tmux-name">{secondaryName}</span>
  {/if}
  {#if elapsed}
    <span class="sep" aria-hidden="true">·</span>
    <span class="sess-elapsed">{elapsed}</span>
  {/if}
  {#if ctxLevel !== null && sess.context_pct !== null}
    <span class="sep" aria-hidden="true">·</span>
    <span
      class="ctx-badge ctx-{ctxLevel}"
      data-testid="context-badge"
      data-level={ctxLevel}
      style="color: {contextColor(ctxLevel)}; border-color: {contextColor(ctxLevel)};"
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
    <span class="sep" aria-hidden="true">·</span>
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
    <span class="sep" aria-hidden="true">·</span>
    <span class="effort-badge" title="Effort: {sess.effort_level}">{sess.effort_level}</span>
  {/if}
  {#if sess.pr_url}
    <span class="sep" aria-hidden="true">·</span>
    <a
      class="pr-link"
      href={sess.pr_url}
      onclick={(e) => e.stopPropagation()}
      title="Open pull request"
      target="_blank"
      rel="noreferrer"
    >PR↗</a>
    {#if sess.ci_status}
      <span class="sep" aria-hidden="true">·</span>
      <span
        class="ci-badge"
        class:ci-badge--stale={ciStale}
        data-testid="ci-badge"
        style="color: {ciStatusColor(sess.ci_status)};"
        title={ciStale && sess.pr_checked_at != null
          ? `CI checks: ${sess.ci_status}, last checked ${timeAgo(sess.pr_checked_at, nowSec * 1000)}`
          : `CI checks: ${sess.ci_status}`}
      >{ciStatusLabel(sess.ci_status)}</span>
    {/if}
  {/if}
  {#if promptText}
    <span class="sep" aria-hidden="true">·</span>
    <span class="sess-meta" data-testid="sess-meta" title={sess.last_prompt ?? undefined}>{promptText}</span>
  {/if}
</div>

<style>
  .sess-details {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.35rem;
    row-gap: 0.15rem;
    min-width: 0;
    padding-left: 0.85rem;
    font-size: 0.65rem;
    color: var(--fg-muted);
  }
  /* The line wraps instead of clipping — hiding the prompt preview (or any
     badge) with no visible trace that it exists would defeat the point of
     the line. Extra height is the user's choice: they opted into this line
     via the details toggle and can collapse it. */
  .sess-details > * { flex-shrink: 0; }
  .sess-details > .sess-secondary { flex-shrink: 1; min-width: 0; }
  .sess-details > .sess-meta { flex-shrink: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
  .sess-details .sep { color: var(--fg-muted); opacity: 0.6; }
  .host-badge {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    flex-shrink: 0;
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
  .ci-badge--stale {
    opacity: 0.45;
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
</style>
