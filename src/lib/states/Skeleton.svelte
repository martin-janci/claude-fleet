<script lang="ts">
  // The states kit's loading state: grey bars in the shape of what is coming,
  // so the layout stays still. Nothing shows for the first LOADING_DELAY_MS;
  // a quick load never flashes. `slow` names what is slow once it is
  // ("mercury is slow (2.4 s)").
  import { onMount } from 'svelte';
  import { LOADING_DELAY_MS } from './states';

  let {
    rows = 3,
    delay = LOADING_DELAY_MS,
    label = 'Loading',
    slow = null,
  }: { rows?: number; delay?: number; label?: string; slow?: string | null } = $props();

  // svelte-ignore state_referenced_locally
  let shown = $state(delay <= 0);
  onMount(() => {
    if (shown) return;
    const t = setTimeout(() => (shown = true), delay);
    return () => clearTimeout(t);
  });

  const WIDTHS = [92, 74, 84, 60, 88];
</script>

{#if shown}
  <div class="skeleton" role="status" aria-busy="true" aria-label={label} data-testid="skeleton">
    {#each { length: rows } as _, i (i)}
      <div class="bar" style:width="{WIDTHS[i % WIDTHS.length]}%"></div>
    {/each}
    {#if slow}<p class="slow" data-testid="skeleton-slow">{slow}</p>{/if}
  </div>
{/if}

<style>
  .skeleton {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.5rem 0;
  }
  .bar {
    height: 0.7rem;
    border-radius: 4px;
    background: var(--bg-hover, var(--border));
    animation: pulse 1.6s ease-in-out infinite;
  }
  .slow {
    margin: 0.2rem 0 0;
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  @keyframes pulse {
    50% { opacity: 0.55; }
  }
  @media (prefers-reduced-motion: reduce) {
    .bar { animation: none; }
  }
</style>
