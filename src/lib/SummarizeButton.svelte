<script lang="ts">
  // Summarise (work graph M13.1, D10): a Claude-written summary of a past
  // session, on demand only. One print-mode fork runs on the session's own
  // host with no tools; the reply is kept in the work journal, where the next
  // resume brief shows it. Here it is shown once, below the row, as text.
  import { summarizePastWork, type WorkLink } from './work';
  import { plainUntrusted } from './tracker_health';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { pushError } from './toasts';

  let {
    workKey,
    link,
  }: {
    workKey: string;
    /** The ended link whose last conversation is summarised. */
    link: WorkLink;
  } = $props();

  let busy = $state(false);
  let text = $state<string | null>(null);
  let truncated = $state(false);

  const blocked = $derived(hubActionBlocked('summarize_past_work', $hubStatus, $hubConnection));
  const disabledReason = $derived(
    blocked ?? (link.resumable === false ? 'Its transcripts were purged: there is nothing to summarise' : null),
  );

  async function run(e: MouseEvent) {
    e.stopPropagation();
    if (busy || disabledReason) return;
    busy = true;
    const r = await summarizePastWork(workKey, link.id);
    busy = false;
    if (!r.ok) {
      pushError(r.error, `Summarising ${workKey} failed`);
      return;
    }
    text = plainUntrusted(r.value.summary);
    truncated = r.value.truncated === true;
  }

  function close(e: MouseEvent) {
    e.stopPropagation();
    text = null;
  }
</script>

<button
  type="button"
  class="summarize"
  disabled={busy || disabledReason !== null}
  title={disabledReason ??
    `Ask Claude to summarise this session's last conversation (one model call on ${link.snap_host ?? 'its host'}); the next resume brief includes it`}
  data-testid="summarize-button"
  onclick={run}>{busy ? 'Summarising…' : 'Summarise'}</button
>

{#if text !== null}
  <div class="summary" data-testid="past-summary" role="note" aria-label="Summary of {workKey}">
    <div class="head">
      <span>Written by Claude from the transcript{truncated ? ' (cut to 4,000 characters)' : ''}</span>
      <button type="button" class="x" aria-label="Close the summary" data-testid="past-summary-close" onclick={close}>×</button>
    </div>
    <pre>{text}</pre>
  </div>
{/if}

<style>
  .summarize {
    flex: none;
    font-size: 11px;
    padding: 1px 6px;
  }
  .summary {
    flex-basis: 100%;
    margin-top: 4px;
    border: 1px solid var(--border, #444);
    border-radius: 4px;
    padding: 4px 6px;
    font-size: 11px;
  }
  .summary .head {
    display: flex;
    justify-content: space-between;
    gap: 6px;
    opacity: 0.75;
  }
  .summary pre {
    margin: 4px 0 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: inherit;
  }
  .x {
    border: none;
    background: none;
    cursor: pointer;
    padding: 0 2px;
  }
</style>
