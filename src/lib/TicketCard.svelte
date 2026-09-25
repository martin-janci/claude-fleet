<script lang="ts">
  // The ticket context card in Details (work graph M9.2): the session's
  // ticket — title, status, a link — and its acceptance criteria from the
  // hub's cached description. "Insert into composer" puts the hub-built,
  // fenced text into this session's prompt box and never sends it. Tracker
  // text is rendered as plain text only.
  import type { SessionRow } from './sessions';
  import { insertIntoComposer } from './conversation';
  import { openExternal } from './open_external';
  import { loadTicketCard, canInsertInto, type TicketCard } from './ticket_card';
  import { requestWorkHandover, handoverOutcome, handoverOutcomeLine, type HandoverOutcome } from './work';
  import { sessionHistory } from './timeline';
  import { push, pushError } from './toasts';
  import { hubStatus, hubActionBlocked } from './hub';
  import { hubConnection } from './hub_connection';

  let { session }: { session: SessionRow } = $props();

  const key = $derived(session.work?.key ?? null);
  let card = $state<TicketCard | null>(null);
  let error = $state<string | null>(null);
  let inserted = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  // Reload when the key changes (another session, or its work was relinked).
  let loadedFor: string | null = null;
  $effect(() => {
    const k = key;
    if (k === loadedFor) return;
    loadedFor = k;
    card = null;
    error = null;
    if (!k) return;
    void loadTicketCard(k).then((r) => {
      if (loadedFor !== k) return;
      if (r.ok) card = r.value;
      else error = r.error.message;
    });
  });
  $effect(() => () => clearTimeout(timer));

  const canInsert = $derived(!!card && canInsertInto(session));
  const url = $derived(card?.url ?? session.work?.url ?? null);

  // Work graph M9.3: ask the session to write its hand-off (on demand only).
  // Only an idle REPL is asked: `stopped` or unknown may be a bare shell,
  // where the prompt would run as commands. The hub refuses the rest too.
  let asking = $state(false);
  const askBlocked = $derived(hubActionBlocked('request_work_handover', $hubStatus, $hubConnection));
  const canAsk = $derived(
    askBlocked === null && canInsertInto(session) && session.claude_status === 'idle' && !session.stuck_kind,
  );
  async function askHandover() {
    if (!key || asking) return;
    asking = true;
    const r = await requestWorkHandover(session.id);
    asking = false;
    if (r.ok) push({ kind: 'success', message: `Asked for a ${key} handover; it is kept when the reply ends` });
    else pushError(r.error, 'Asking for a handover failed');
    void loadOutcome(session.id);
  }

  // The latest outcome (M10.1): the newest `handover_*` event on the
  // session's timeline — re-read after an ask and whenever a turn ends
  // (the Stop hook is what settles a request).
  let outcome = $state<HandoverOutcome | null>(null);
  let outcomeSeq = 0;
  async function loadOutcome(id: number) {
    const mine = ++outcomeSeq;
    const r = await sessionHistory(id, 200);
    if (mine !== outcomeSeq) return;
    outcome = r.ok && Array.isArray(r.value) ? handoverOutcome(r.value) : null;
  }
  $effect(() => {
    const id = session.id;
    void session.turn_seq;
    if (!key) return;
    void loadOutcome(id);
  });

  function insert() {
    if (!card || !canInsert) return;
    insertIntoComposer(session.id, card.composer_text);
    inserted = true;
    clearTimeout(timer);
    timer = setTimeout(() => (inserted = false), 1_500);
  }
</script>

{#if key}
  <section class="block ticket-card" data-testid="ticket-card" aria-label="Ticket">
    <h3>
      <span class="key">{key}</span>
      {#if card?.title || session.work?.title}<span class="title">{card?.title || session.work?.title}</span>{/if}
    </h3>
    <div class="meta">
      {#if card?.status_name ?? session.work?.status_name}
        <span class="status" data-testid="ticket-card-status">{card?.status_name ?? session.work?.status_name}</span>
      {/if}
      {#if url}
        <button class="btn btn--quiet link" type="button" data-testid="ticket-card-open" onclick={() => void openExternal(url)}
          >Open ticket</button
        >
      {/if}
    </div>

    {#if error}
      <p class="muted" data-testid="ticket-card-error">{error}</p>
    {:else if card}
      {#if card.acceptance && card.acceptance.length > 0}
        <h4>Acceptance criteria</h4>
        <ul class="criteria" data-testid="ticket-card-criteria">
          {#each card.acceptance as c, i (i)}
            <li>{c}</li>
          {/each}
        </ul>
      {:else if card.excerpt}
        <p class="excerpt" data-testid="ticket-card-excerpt">{card.excerpt}</p>
      {:else if !card.cached}
        <p class="muted">No ticket details are cached for {key}.</p>
      {:else}
        <p class="muted">The ticket has no description.</p>
      {/if}
      <button
        class="btn"
        type="button"
        data-testid="ticket-card-insert"
        disabled={!canInsert}
        title={canInsert
          ? 'Put the ticket into the prompt box (marked as untrusted input); nothing is sent'
          : 'This session has no conversation composer'}
        onclick={insert}>{inserted ? 'Inserted' : 'Insert into composer'}</button
      >
    {/if}
    <button
      class="btn btn--quiet"
      type="button"
      data-testid="ticket-card-handover"
      disabled={!canAsk || asking}
      title={askBlocked ??
        (canAsk
          ? 'Ask this session to write a handover for the next session on this work (it uses one turn)'
          : 'Only an idle Claude session can be asked for a handover')}
      onclick={() => void askHandover()}>Ask for a handover</button
    >
    {#if outcome}
      <p class="muted handover {outcome.state}" data-testid="ticket-card-handover-outcome" data-state={outcome.state}>
        {handoverOutcomeLine(outcome)}
      </p>
    {/if}
  </section>
{/if}

<style>
  .ticket-card h3 {
    display: flex;
    gap: 0.4rem;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .key {
    font-family: var(--font-mono, monospace);
  }
  .title {
    font-weight: normal;
    overflow-wrap: anywhere;
  }
  .meta {
    display: flex;
    gap: 0.4rem;
    align-items: baseline;
    color: var(--fg-muted);
    font-size: 0.85rem;
  }
  h4 {
    margin: 0.5rem 0 0.2rem;
    font-size: 0.8rem;
    color: var(--fg-muted);
  }
  .criteria {
    margin: 0 0 0.5rem;
    padding-left: 1.2rem;
  }
  .criteria li,
  .excerpt {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .muted {
    color: var(--fg-muted);
  }
  .handover {
    margin: 0.3rem 0 0;
    font-size: 0.85rem;
  }
  .handover.missing,
  .handover.failed {
    color: var(--warn, #e0a030);
  }
</style>
