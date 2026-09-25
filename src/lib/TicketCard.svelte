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
</style>
