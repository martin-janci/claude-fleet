<!-- The row every session list uses (manual: SessionRow): status dot, title
     and age, then line two (what it waits on or did last, never the last
     prompt), then a few chips or `sm` actions. Selected rows carry
     aria-selected: accent-soft fill and the 2 px accent bar. -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import StatusDot from './StatusDot.svelte';
  import { STATE_WORD, type OfState } from './status';

  let {
    state = 'idle',
    stateLabel,
    title,
    age,
    lead,
    line,
    selected = false,
    onselect,
    chips,
    below,
    testid,
  }: {
    state?: OfState;
    /** The dot's label ("Waiting for you", "Paused"); defaults to the state's word. */
    stateLabel?: string;
    title: string;
    age?: string;
    /** Line two's coloured lead ("Waiting for you:", "Failed:"). */
    lead?: string;
    line?: string;
    selected?: boolean;
    onselect?: () => void;
    chips?: Snippet;
    /** Under line two: a Jev proposal (AISuggestion, step 3.11). */
    below?: Snippet;
    testid?: string;
  } = $props();

  const leadClass = $derived(state === 'failed' ? 'f' : 'w');

  function onkeydown(e: KeyboardEvent) {
    if ((e.key === 'Enter' || e.key === ' ') && e.target === e.currentTarget) {
      e.preventDefault();
      onselect?.();
    }
  }
</script>

<div
  class="of of-row"
  role="option"
  aria-selected={selected ? 'true' : 'false'}
  tabindex={onselect ? 0 : undefined}
  onclick={onselect}
  {onkeydown}
  data-testid={testid}
>
  <StatusDot {state} label={stateLabel ?? STATE_WORD[state]} />
  <div class="body">
    <div class="l1">
      <span class="title" {title}>{title}</span>
      {#if age}<span class="meta tnum">{age}</span>{/if}
    </div>
    {#if lead || line}
      <div class="line">{#if lead}<span class={leadClass}>{lead}</span>{' '}{/if}{line ?? ''}</div>
    {/if}
    {@render below?.()}
    {#if chips}<div class="chips">{@render chips()}</div>{/if}
  </div>
</div>
