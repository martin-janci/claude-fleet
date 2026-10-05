<script lang="ts">
  // Summarise (work graph M13.4c, D10): a Claude-written summary of a past
  // session, on demand only. One print-mode fork runs on the session's own
  // host with no tools; the reply is kept in the work journal, where the next
  // resume brief shows it. Here it is shown once, below the row, as text.
  import { linkSessionId, summarizePastWork, type WorkLink } from './work';
  import { plainUntrusted } from './tracker_health';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessions } from './sessions';
  import { sessionIdBlocked } from './share';
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
  /**
   * The access half (multi-user M1; F2a's lookup, made honest in F2b, made ONE
   * rule in F2d). `summarize_past_work` is `own` in `share.ts::SESSION_TIER` —
   * a Claude-written précis of the transcript, kept in the journal, outlives the
   * grant that allowed it — but this button is handed a `WorkLink`, not a
   * session row, so it has no `owner_person_id` to ask about.
   *
   * F2a resolved one out of `$sessions` by the link's snapshot (`snap_host` +
   * `snap_tmux`), F2b made the miss fail closed except on a desktop that owns
   * its fleet, and both halves of that lived here, hand-rolled. The snapshot
   * lookup is `work.ts::linkSessionId` now and the fail-closed rule is
   * `share.ts::sessionIdBlocked`'s, for two reasons:
   *
   *  - a name match is not a session identity — a tmux name is reused, so
   *    `(snap_host, snap_tmux)` can name a row that merely INHERITED the pane
   *    name, and asking about it is asking about the wrong session (see
   *    `linkSessionId`);
   *  - the `backendMode(…) === 'local'` escape hatch was a second copy of
   *    `sessionIdActionBlocked`'s own `local` branch, in a file that must not be
   *    where that rule is decided.
   */
  const sourceId = $derived(linkSessionId(link, $sessions));
  const accessBlocked = $derived($sessionIdBlocked(sourceId, 'summarize_past_work'));
  const disabledReason = $derived(
    blocked ??
      accessBlocked ??
      (link.resumable === false ? 'Its transcripts were purged: there is nothing to summarise' : null),
  );

  async function run(e: MouseEvent) {
    e.stopPropagation();
    // Re-asked at the call: the row can leave the store, and a grant can be
    // narrowed, while this list is on screen.
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
