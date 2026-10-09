<script lang="ts">
  // Rewind… from the session's own actions (gap plan G1.11): the per-turn
  // Rewind under a reply (ReplyActions), with the turn picked from a list
  // instead of from the conversation. The same engine call
  // (`rewind_conversation`, mode `rewind`), the same rule for which turns
  // can be rewound (`replyActionsFor`), the same sentence about files, and
  // the prompt back in the composer afterwards. This sheet IS the
  // confirmation, as the Fork sheet is.
  import DialogSheet from './DialogSheet.svelte';
  import { rewindConversation, type SessionRow } from './sessions';
  import { insertIntoComposer, sessionConversation } from './conversation';
  import { rewindChoices, type RewindChoice } from './reply_actions';
  import { timeAgo } from './session_status';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessionIdBlocked } from './share';
  import { push } from './toasts';

  let { session, onclose }: { session: SessionRow; onclose: () => void } = $props();

  /** How many recent turns the sheet offers. */
  const TURNS = 30;

  let choices = $state<RewindChoice[] | null>(null);
  let loadError = $state<string | null>(null);
  let picked = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  // Rewind is `own` (`share.ts`): it leaves a copy of the transcript and
  // restarts the pane. Re-read at the call, as ReplyActions does, so a
  // revoke or the link dropping while the sheet is open still stops it.
  const blocked = $derived(
    hubActionBlocked('rewind_conversation', $hubStatus, $hubConnection) ??
      $sessionIdBlocked(session.id, 'rewind_conversation'),
  );

  async function load() {
    const r = await sessionConversation(session.id, TURNS);
    if (!r.ok) {
      loadError = r.error.message;
      return;
    }
    choices = rewindChoices(r.value.turns, r.value.truncated);
    picked = choices[0]?.anchor ?? null;
  }
  void load();

  async function rewind() {
    const c = choices?.find((x) => x.anchor === picked);
    if (!c || busy || blocked !== null) return;
    busy = true;
    error = null;
    const r = await rewindConversation(session.id, 'rewind', c.anchor);
    busy = false;
    if (!r.ok) {
      error = r.error.message;
      return;
    }
    if (c.prompt) insertIntoComposer(session.id, c.prompt);
    push({ kind: 'success', message: c.prompt ? 'Rewound. The prompt is back in the composer.' : 'Rewound' });
    onclose();
  }
</script>

<DialogSheet
  title="Rewind the conversation"
  lead="The conversation goes back to before the turn you pick. Your files are left as they are."
  verb="Rewind"
  busyVerb="Rewinding…"
  {busy}
  canConfirm={picked !== null && blocked === null}
  confirmTitle={blocked}
  onconfirm={() => void rewind()}
  {onclose}
  error={error ?? loadError ?? blocked}
  errorTestid="rewind-error"
  confirmTestid="rewind-confirm"
  width="480px"
  testid="rewind-sheet"
>
  {#if choices === null && loadError === null}
    <p class="field-note">Reading the conversation…</p>
  {:else if choices !== null && choices.length === 0}
    <p class="field-note" data-testid="rewind-none">No turn here can be rewound to: the conversation has only its first turn.</p>
  {:else if choices !== null}
    <fieldset class="field turns">
      <legend class="field-label">Back to before</legend>
      {#each choices as c (c.anchor)}
        <label class="turn">
          <input
            type="radio"
            name="rewind-turn"
            data-testid="rewind-turn"
            value={c.anchor}
            checked={picked === c.anchor}
            disabled={busy}
            onchange={() => (picked = c.anchor)}
          />
          <span class="line">{c.line}</span>
          {#if c.at}<span class="when">{timeAgo(Math.floor(Date.parse(c.at) / 1000))}</span>{/if}
        </label>
      {/each}
    </fieldset>
  {/if}
</DialogSheet>

<style>
  .turns {
    border: 0;
    padding: 0;
    margin: 0;
    max-height: 320px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-1, 4px);
  }
  .turn {
    display: flex;
    align-items: baseline;
    gap: var(--space-2, 8px);
    font-size: var(--text-sm, 12.5px);
  }
  .line {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .when {
    color: var(--fg-muted);
    font-size: var(--text-2xs, 11px);
    flex-shrink: 0;
  }
</style>
