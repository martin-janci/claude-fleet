<!--
  Control's activity above the composer, as one row of counts (compact
  Control, 2026-10-30, option A): "4 need you · 2 handed off · 1 suggested".
  A count opens its list right above the row, the same cards as before
  (ControlEscalations, HandoffCards, ControlSuggestions) with every action
  they had; pressing it again or Escape folds it back. The lists stay
  mounted while folded, so their counts, follows and receipts keep running.

  A plan Control proposed (a tree receipt with proposals left to tick) opens
  the handed-off list by itself: it is the answer to what was just asked.
  The confirms, route and command receipts are not here: Control waits on
  the first, and the other two answer the line just sent (AgentPanel).
-->
<script lang="ts">
  import ControlEscalations from './ControlEscalations.svelte';
  import HandoffCards from './HandoffCards.svelte';
  import ControlSuggestions from './ControlSuggestions.svelte';
  import { untrack } from 'svelte';

  let { sessionId = null }: { sessionId?: number | null } = $props();

  type Section = 'needs' | 'handoffs' | 'suggested';

  let needs = $state(0);
  let handed = $state(0);
  let waiting = $state(0);
  let suggested = $state(0);
  let open = $state<Section | null>(null);
  /** Counts that just went up, marked for a moment. */
  let fresh = $state<Section[]>([]);

  const uid = `ctl-activity-${Math.random().toString(36).slice(2, 8)}`;
  const counts = $derived({ needs, handoffs: handed, suggested: sessionId === null ? 0 : suggested });

  function toggle(s: Section) {
    open = open === s ? null : s;
  }

  // An open list whose last card went closes; a new plan to tick opens its list.
  let lastWaiting = 0;
  $effect(() => {
    const c = counts;
    const w = waiting;
    untrack(() => {
      if (open && c[open] === 0) open = null;
      if (w > lastWaiting) open = 'handoffs';
      lastWaiting = w;
    });
  });

  let last: Record<Section, number> = { needs: 0, handoffs: 0, suggested: 0 };
  $effect(() => {
    const c = counts;
    untrack(() => {
      const up = (Object.keys(c) as Section[]).filter((s) => c[s] > last[s] && open !== s);
      last = { ...c };
      if (up.length === 0) return;
      fresh = [...new Set([...fresh, ...up])];
      setTimeout(() => (fresh = fresh.filter((s) => !up.includes(s))), 1600);
    });
  });

  function onKey(e: KeyboardEvent) {
    if (e.key !== 'Escape' || open === null) return;
    const s = open;
    open = null;
    e.stopPropagation();
    document.getElementById(`${uid}-${s}-btn`)?.focus();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="activity" data-testid="control-activity" onkeydown={onKey}>
  <div class="drawer" id="{uid}-needs" data-testid="control-activity-needs" hidden={open !== 'needs'}>
    <ControlEscalations bind:count={needs} />
  </div>
  <div class="drawer" id="{uid}-handoffs" data-testid="control-activity-handoffs" hidden={open !== 'handoffs'}>
    <HandoffCards bind:count={handed} bind:waiting />
  </div>
  {#if sessionId !== null}
    <div class="drawer" id="{uid}-suggested" data-testid="control-activity-suggested" hidden={open !== 'suggested'}>
      <ControlSuggestions {sessionId} bind:count={suggested} />
    </div>
  {/if}
  {#if counts.needs + counts.handoffs + counts.suggested > 0}
    <div class="strip" role="group" aria-label="Control's activity" data-testid="control-activity-strip">
      {#if counts.needs > 0}
        <button
          type="button"
          id="{uid}-needs-btn"
          class="count warn"
          class:fresh={fresh.includes('needs')}
          data-testid="control-activity-needs-btn"
          aria-expanded={open === 'needs'}
          aria-controls="{uid}-needs"
          title="What in the fleet waits for your answer"
          onclick={() => toggle('needs')}><span class="mark" aria-hidden="true">!</span>{counts.needs} need you</button
        >
      {/if}
      {#if counts.handoffs > 0}
        <button
          type="button"
          id="{uid}-handoffs-btn"
          class="count"
          class:warn={waiting > 0}
          class:fresh={fresh.includes('handoffs')}
          data-testid="control-activity-handoffs-btn"
          aria-expanded={open === 'handoffs'}
          aria-controls="{uid}-handoffs"
          title="What Control handed to sessions, tasks and missions"
          onclick={() => toggle('handoffs')}
          >{#if waiting > 0}{waiting} {waiting === 1 ? 'plan' : 'plans'} to review ·
          {/if}{counts.handoffs} handed off</button
        >
      {/if}
      {#if counts.suggested > 0}
        <button
          type="button"
          id="{uid}-suggested-btn"
          class="count"
          class:fresh={fresh.includes('suggested')}
          data-testid="control-activity-suggested-btn"
          aria-expanded={open === 'suggested'}
          aria-controls="{uid}-suggested"
          title="Suggested from your fleet"
          onclick={() => toggle('suggested')}><span class="mark" aria-hidden="true">✦</span>{counts.suggested} suggested</button
        >
      {/if}
      <span class="sr-only" role="status" aria-live="polite"
        >{counts.needs > 0 ? `${counts.needs} need you. ` : ''}{waiting > 0 ? `${waiting} plan to review.` : ''}</span
      >
    </div>
  {/if}
</div>

<style>
  .activity {
    flex-basis: 100%;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    min-width: 0;
  }
  .drawer {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    min-width: 0;
  }
  .drawer[hidden] {
    display: none;
  }
  .strip {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    align-items: center;
    min-width: 0;
  }
  .count {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.1rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg-muted);
    font: inherit;
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    cursor: pointer;
  }
  .count:hover {
    color: var(--fg);
    border-color: var(--accent);
  }
  .count[aria-expanded='true'] {
    color: var(--fg);
    border-color: var(--accent);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .count.warn {
    color: var(--usage-warn);
    border-color: color-mix(in srgb, var(--usage-warn) 45%, var(--border));
    background: color-mix(in srgb, var(--usage-warn) 9%, var(--bg));
    font-weight: 600;
  }
  .count:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: 1px;
  }
  .mark {
    font-weight: 700;
  }
  .fresh {
    animation: count-fresh calc(var(--dur-slow) * 5) ease-out;
  }
  @keyframes count-fresh {
    0%,
    40% {
      box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 35%, transparent);
    }
    100% {
      box-shadow: 0 0 0 0 transparent;
    }
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
  @media (prefers-reduced-motion: reduce) {
    .fresh {
      animation: none;
    }
  }
</style>
