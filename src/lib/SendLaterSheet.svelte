<!--
  Send later, from the composer (Orbit Fleet gap plan step G2.7, the
  FormsSession board; the phone's SessionLater form, now on the desktop):
  the message, when it goes (idle, in an hour, tomorrow at nine, once the
  usage limit resets, or at a time), and "Skip it if the session is
  archived first". The prompt waits in `deferred_prompts` (G1.8) and is
  typed at the first idle moment once its time has come. Undo takes it back.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import DialogSheet from './DialogSheet.svelte';
  import { cancelQueuedPrompt, queuePrompt } from './sessions';
  import { SEND_LATER_CHOICES, sendLaterPlan, toLocalDateTime, tomorrowAtNine, type SendLaterChoice } from './send_later';
  import { savedWithUndo } from './forms/form_frame';
  import { push, pushError } from './toasts';
  import type { IpcError } from './result';
  import { hubActionBlocked, hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionIdBlocked } from './share';

  let {
    sessionId,
    initial = '',
    blocked = null,
    now = () => Date.now(),
    onscheduled,
    onclose,
  }: {
    sessionId: number;
    /** The composer's draft: the message starts as what was typed. */
    initial?: string;
    /** Why this client may not queue a prompt here (hub or access). */
    blocked?: string | null;
    /** The clock, in ms (tests hold it). */
    now?: () => number;
    /** Queued or sent: the composer clears the draft it came from. */
    onscheduled: (message: string) => void;
    onclose: () => void;
  } = $props();

  // The draft as the sheet opened: what it starts from, and what
  // "changed" is measured against.
  const start = untrack(() => initial);
  let message = $state(start);
  let choice = $state<SendLaterChoice>('hour');
  let at = $state(untrack(() => toLocalDateTime(tomorrowAtNine(now()))));
  let skipIfArchived = $state(true);
  let busy = $state(false);
  let error = $state<string | IpcError | null>(null);

  // The write's own gate (multi-user M1): asked again here, where the
  // queue call is, not only on the composer's clock button.
  const gate = $derived(
    blocked ??
      hubActionBlocked('queue_prompt', $hubStatus, $hubConnection) ??
      $sessionIdBlocked(sessionId, 'queue_prompt'),
  );
  const plan = $derived(sendLaterPlan(choice, now(), at, skipIfArchived));
  const why = $derived(
    gate ?? (message.trim() === '' ? 'Write the message first.' : plan.error),
  );

  async function schedule() {
    if (why !== null || busy) return;
    busy = true;
    error = null;
    const text = message.trim();
    const p = sendLaterPlan(choice, now(), at, skipIfArchived);
    const r = await queuePrompt(sessionId, text, p.timing);
    busy = false;
    if (!r.ok) {
      error = r.error;
      return;
    }
    onscheduled(text);
    onclose();
    const queued = r.value.queued_id;
    if (r.value.delivered || queued == null) {
      push({ kind: 'success', message: 'Sent now: the session was idle' });
      return;
    }
    savedWithUndo(`Scheduled ${p.when}`, async () => {
      if (gate !== null) {
        push({ kind: 'error', message: `Could not take it back: ${gate}` });
        return;
      }
      const u = await cancelQueuedPrompt(sessionId, queued);
      if (!u.ok) pushError(u.error, 'Could not take it back');
    });
  }
</script>

<DialogSheet
  title="Send later"
  lead="The message waits and goes out on its own, at the first idle moment once its time has come."
  verb="Schedule"
  busyVerb="Scheduling…"
  {busy}
  {error}
  dirty={message.trim() !== start.trim()}
  canConfirm={why === null}
  confirmTitle={why}
  onconfirm={() => void schedule()}
  {onclose}
  testid="send-later-sheet"
  confirmTestid="send-later-schedule"
  errorTestid="send-later-error"
>
  <label class="field">
    <span class="field-label">Message</span>
    <textarea data-testid="send-later-message" rows="4" bind:value={message}></textarea>
  </label>
  <fieldset class="field when">
    <legend class="field-label">Send</legend>
    <div role="radiogroup" aria-label="Send" class="choices">
      {#each SEND_LATER_CHOICES as c (c.value)}
        <label class="choice">
          <input
            type="radio"
            name="send-later-when"
            value={c.value}
            data-testid="send-later-{c.value}"
            checked={choice === c.value}
            onchange={() => (choice = c.value)}
          />
          {c.label}
        </label>
      {/each}
    </div>
    {#if choice === 'at'}
      <input type="datetime-local" data-testid="send-later-at" aria-label="Send at" bind:value={at} />
    {/if}
    {#if choice === 'limit'}
      <p class="field-note">It waits while this session's account is at its usage limit.</p>
    {/if}
  </fieldset>
  <label class="check">
    <input type="checkbox" data-testid="send-later-skip" bind:checked={skipIfArchived} />
    Skip it if the session is archived first
  </label>
</DialogSheet>

<style>
  .when {
    border: 0;
    margin: 0;
    padding: 0;
  }
  .choices {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .choice,
  .check {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: var(--text-sm);
    cursor: pointer;
  }
  input[type='datetime-local'] {
    font: inherit;
    font-size: var(--text-sm);
    color: var(--fg);
    background: var(--bg-sunk);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    padding: var(--space-1) var(--space-2);
    align-self: flex-start;
  }
</style>
