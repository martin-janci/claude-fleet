<script lang="ts">
  // The session's "Send later" prompts that have not gone out yet, above the
  // composer, each with when it goes and a cancel (review w13-9). Without it
  // a scheduled prompt was invisible on the desktop once the Undo toast went:
  // withdrawing it needed the phone or MCP.
  import { cancelQueuedPrompt, queuedPrompts, type QueuedPrompt } from './sessions';
  import { scheduledWhen, waitingPrompts } from './send_later';
  import { heldPreview } from './composer_hold';
  import { pushError } from './toasts';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionIdBlocked } from './share';

  let {
    sessionId,
    refreshKey = 0,
  }: {
    sessionId: number;
    /** Bumped by the host when the list may have changed: a prompt was
     *  scheduled, or a turn started (a waiting prompt may have gone). */
    refreshKey?: number;
  } = $props();

  let waiting = $state<QueuedPrompt[]>([]);
  let cancelling = $state<number | null>(null);
  const readBlocked = $derived(
    hubActionBlocked('queued_prompts', $hubStatus, $hubConnection) ?? $sessionIdBlocked(sessionId, 'queued_prompts'),
  );
  const cancelBlocked = $derived(
    hubActionBlocked('cancel_queued_prompt', $hubStatus, $hubConnection) ??
      $sessionIdBlocked(sessionId, 'cancel_queued_prompt'),
  );

  let seq = 0;
  async function load(id: number) {
    if (readBlocked !== null) return;
    const mine = ++seq;
    const r = await queuedPrompts(id);
    if (mine !== seq) return;
    // A read that fails (an older hub, no access) shows nothing rather
    // than an error above every composer.
    waiting = r.ok ? waitingPrompts(r.value ?? []) : [];
  }
  $effect(() => {
    const id = sessionId;
    void refreshKey;
    if (readBlocked !== null) {
      waiting = [];
      return;
    }
    void load(id);
  });

  async function cancel(q: QueuedPrompt) {
    if (cancelling !== null || cancelBlocked !== null) return;
    cancelling = q.id;
    const r = await cancelQueuedPrompt(sessionId, q.id);
    cancelling = null;
    if (!r.ok) pushError(r.error, 'Could not cancel it');
    await load(sessionId);
  }
</script>

{#if waiting.length > 0}
  <div class="scheduled" data-testid="conv-scheduled">
    <span class="scheduled-head">Scheduled ({waiting.length})</span>
    {#each waiting as q (q.id)}
      <div class="scheduled-chip" data-testid="conv-scheduled-item">
        <span class="scheduled-text" title={q.body}>{heldPreview(q.body)}</span>
        <span class="scheduled-when">· {scheduledWhen(q, Date.now())} ·</span>
        <button
          type="button"
          class="btn btn--quiet scheduled-cancel"
          data-testid="conv-scheduled-cancel"
          title={cancelBlocked ?? 'Take it back before it goes'}
          disabled={cancelling !== null || cancelBlocked !== null}
          onclick={() => void cancel(q)}>cancel</button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .scheduled {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 4px 12px 0;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .scheduled-head {
    font-weight: 600;
  }
  .scheduled-chip {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .scheduled-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .scheduled-when {
    flex: none;
  }
  .scheduled-cancel {
    flex: none;
    font-size: var(--text-xs);
    padding: 0 4px;
  }
</style>
