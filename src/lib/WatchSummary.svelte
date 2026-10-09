<!--
  Orbit Fleet 11.11: "Since 13:20", a short summary of what a session did
  since the person last looked, for a watcher (WatchView) and at the top of
  Details. On demand only: opening a session runs nothing. The hub drafts it
  on the session's host, and hides it when Jev cannot confirm it against the
  transcript.
-->
<script lang="ts">
  import { draftedBy } from './ai_proposal';
  import DraftedLabel from './DraftedLabel.svelte';
  import type { SessionRow } from './sessions';
  import { checkLabel, clock, defaultSince, sessionSummarySince, turnsLabel, type WatchSummary } from './watch_summary';

  let { session }: { session: Pick<SessionRow, 'id' | 'last_viewed_at'> } = $props();

  let summary = $state<WatchSummary | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  /** False on a hub older than 11.11, or for an org that has not consented:
   *  the block says so once and offers nothing. */
  let refused = $state<string | null>(null);

  const since = $derived(defaultSince(session.last_viewed_at, Math.floor(Date.now() / 1000)));

  // A different session starts from nothing. Keyed on the id, not the row:
  // a row event hands the same session back as a new object, and that must
  // not clear the summary (review r16, as #721 fixed at the source).
  const sessionId = $derived(session.id);
  $effect(() => {
    void sessionId;
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
    summary ? draftedBy(summary.model, summary.host_alias, turnsLabel(summary)) : '',
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
      <p class="muted drafted">
        <DraftedLabel testid="watch-summary-drafted" />
        {#if summary.check !== 'passed'}
          <!-- 11.11 with decide.jev.summary_check off (or in shadow): the
               text shows unchecked, and says so beside the pill, not only
               in the small print ("Drafted · Not checked"). -->
          <span class="unchecked" data-testid="watch-summary-unchecked" title="Jev did not check this against the transcript"
            >Not checked</span
          >
        {/if}
        <span data-testid="watch-summary-meta">{meta} · {checkLabel(summary.check)}</span>
      </p>
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
  .drafted {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .unchecked {
    font-size: var(--text-2xs);
    line-height: 16px;
    font-weight: 500;
    padding: 0 6px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--status-waiting);
    color: var(--status-waiting);
    white-space: nowrap;
  }
  .error {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--danger);
  }
</style>
