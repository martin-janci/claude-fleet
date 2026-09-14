<script lang="ts">
  // Read-only session event timeline (Q9). Renders the `session_events`
  // rows the backend already records (status changes, prompts, stuck,
  // kills, repairs, …) newest first, with filter chips.
  import { untrack } from 'svelte';
  import {
    sessionHistory,
    eventCategory,
    filterEvents,
    kindLabel,
    shortDetail,
    eventTime,
    FILTER_CATEGORIES,
    type EventCategory,
    type SessionEvent,
  } from './timeline';

  let {
    sessionId,
    refreshKey = '',
  }: {
    sessionId: number;
    /** Any change re-fetches (the parent passes turn/status fields so a new
     *  event shows up without a manual refresh). */
    refreshKey?: string;
  } = $props();

  let events = $state<SessionEvent[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let active = $state<Set<EventCategory>>(new Set());
  // Drops a stale response when the session changes mid-flight.
  let seq = 0;

  async function load(id: number) {
    const mine = ++seq;
    loading = events.length === 0;
    const r = await sessionHistory(id);
    if (mine !== seq) return;
    loading = false;
    if (r.ok) {
      events = Array.isArray(r.value) ? r.value : [];
      error = null;
    } else {
      error = r.error.message;
    }
  }

  let lastId: number | null = null;
  // Depend only on the props: `load` reads and writes `events`, so running it
  // tracked would re-trigger this effect on every response (a fetch loop).
  $effect(() => {
    const id = sessionId;
    void refreshKey;
    untrack(() => {
      if (id !== lastId) {
        lastId = id;
        events = [];
      }
      void load(id);
    });
  });

  function toggle(c: EventCategory) {
    const next = new Set(active);
    if (next.has(c)) next.delete(c);
    else next.add(c);
    active = next;
  }

  const shown = $derived(filterEvents(events, active));
</script>

<section class="timeline" data-testid="timeline" aria-label="Session timeline">
  <div class="head">
    <h3>Timeline</h3>
    <div class="chips" role="group" aria-label="Filter timeline events">
      {#each FILTER_CATEGORIES as c (c.id)}
        <button
          type="button"
          class="chip"
          class:on={active.has(c.id)}
          aria-pressed={active.has(c.id)}
          data-testid="timeline-chip-{c.id}"
          onclick={() => toggle(c.id)}
        >{c.label}</button>
      {/each}
    </div>
  </div>

  {#if loading}
    <p class="muted">Loading…</p>
  {:else if error}
    <p class="err" data-testid="timeline-error">{error}</p>
  {:else if shown.length === 0}
    <p class="muted" data-testid="timeline-empty">
      {events.length === 0 ? 'No events recorded yet.' : 'No events match the filter.'}
    </p>
  {:else}
    <ol class="events">
      {#each shown as e (e.id)}
        <li class="ev cat-{eventCategory(e)}" data-testid="timeline-event" data-kind={e.kind}>
          <time datetime={new Date(e.at * 1000).toISOString()}>{eventTime(e.at)}</time>
          <span class="kind">{kindLabel(e.kind)}</span>
          <span class="detail" title={e.detail ?? undefined}>{shortDetail(e.detail)}</span>
        </li>
      {/each}
    </ol>
  {/if}
</section>

<style>
  .timeline {
    border-top: 1px solid var(--border);
    padding-top: 0.6rem;
    margin-top: 0.6rem;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin-bottom: 0.4rem;
  }
  h3 {
    margin: 0;
    font-size: 0.7rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .chips { display: flex; gap: 0.25rem; flex-wrap: wrap; }
  .chip {
    font-size: 0.65rem;
    padding: 0.1rem 0.45rem;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
  }
  .chip.on { border-color: var(--accent); color: var(--fg); }
  .chip:focus-visible { outline: 2px solid var(--accent); outline-offset: 1px; }
  .events {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 18rem;
    overflow-y: auto;
    font-size: 0.75rem;
  }
  .ev {
    display: grid;
    grid-template-columns: auto auto 1fr;
    gap: 0.5rem;
    padding: 0.15rem 0;
    align-items: baseline;
    border-bottom: 1px solid var(--border);
  }
  time {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    color: var(--fg-muted);
    white-space: nowrap;
  }
  .kind { white-space: nowrap; font-weight: 600; }
  .cat-errors .kind { color: #e64a4a; }
  .cat-prompts .kind { color: var(--accent); }
  .detail {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-muted);
  }
  .muted { color: var(--fg-muted); font-style: italic; font-size: 0.75rem; margin: 0; }
  .err { color: #e64a4a; font-size: 0.75rem; margin: 0; }
</style>
