<!--
  Orbit Fleet 11.11: "Since 13:20", a short summary of what a session did
  since the person last looked, for a watcher (WatchView) and at the top of
  Details. On demand only: opening a session runs nothing. The hub drafts it
  on the session's host, and hides it when Jev cannot confirm it against the
  transcript.
-->
<script lang="ts">
  import { draftedBy } from './ai_proposal';
  import type { SessionRow } from './sessions';
  import { checkLabel, clock, defaultSince, sessionSummarySince, type WatchSummary } from './watch_summary';

  let { session }: { session: Pick<SessionRow, 'id' | 'last_viewed_at'> } = $props();

  let summary = $state<WatchSummary | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  /** False on a hub older than 11.11, or for an org that has not consented:
   *  the block says so once and offers nothing. */
  let refused = $state<string | null>(null);

  const since = $derived(defaultSince(session.last_viewed_at, Math.floor(Date.now() / 1000)));

  // A different session starts from nothing.
  $effect(() => {
    void session.id;
    summary = null;
    error = null;
    refused = null;
  });

  async function run() {
    const id = session.id;
    busy = true;
    error = null;
    const r = await sessionSummarySince(id, since);
    if (id !== session.id) return;
    busy = false;
    if (r.ok) {
      summary = r.value;
    } else if (r.error.code === 'E_FORBIDDEN') {
      refused = r.error.message;
    } else if (r.error.code === 'E_HUB_PROTOCOL' || r.error.code === 'E_INVALID') {
      refused = 'This hub cannot summarise sessions yet.';
    } else {
      error = r.error.message;
    }
  }

  const meta = $derived(
    summary ? draftedBy(summary.model, summary.host_alias, `from ${summary.turns} turn${summary.turns === 1 ? '' : 's'}`) : '',
  );
</script>

<section class="since" data-testid="watch-summary" aria-label="Summary since {clock(since)}">
  <div class="head">
    <h3>Since {clock(since)}</h3>
    {#if !refused}
      <button
        class="btn btn--quiet"
        type="button"
        disabled={busy}
        data-testid="watch-summary-run"
        onclick={() => void run()}>{busy ? 'Summarising…' : summary ? 'Regenerate' : 'Summarise'}</button
      >
    {/if}
  </div>
  {#if refused}
    <p class="muted" data-testid="watch-summary-refused">{refused}</p>
  {:else if summary}
    {#if summary.turns === 0}
      <p class="muted" data-testid="watch-summary-empty">Nothing happened since {clock(since)}.</p>
    {:else if summary.text}
      <p class="text ai-pre" data-testid="watch-summary-text">{summary.text}</p>
      <p class="muted" data-testid="watch-summary-meta">{meta} · {checkLabel(summary.check)}</p>
    {:else}
      <p class="muted" data-testid="watch-summary-hidden">The summary is {checkLabel(summary.check)}.</p>
    {/if}
  {/if}
  {#if error}
    <p class="error" role="alert" data-testid="watch-summary-error">{error}</p>
  {/if}
</section>

<style>
  .since {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 0.45rem 0.6rem;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  h3 {
    margin: 0;
    font-size: var(--text-xs);
  }
  .text {
    margin: 0;
    white-space: pre-wrap;
    font-size: var(--text-xs);
    line-height: 1.45;
    padding: 6px 8px;
    border-radius: var(--radius-md);
  }
  .muted {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .error {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--danger);
  }
</style>
