<script lang="ts">
  // A titled group that can fold away: the page renderer's accordion. A
  // native <details>, so keyboard and screen-reader behaviour come free.
  import type { Snippet } from 'svelte';

  let {
    title,
    open = $bindable(true),
    testid,
    badge,
    children,
  }: {
    title: string;
    open?: boolean;
    testid?: string;
    /** A short word after the title, e.g. "Advanced". */
    badge?: string;
    children: Snippet;
  } = $props();
</script>

<details class="disclosure" bind:open data-testid={testid}>
  <summary>
    <span class="title">{title}</span>
    {#if badge}<span class="tag">{badge}</span>{/if}
  </summary>
  <div class="body">{@render children()}</div>
</details>

<style>
  .disclosure {
    border-top: 1px solid var(--border);
    padding: 0.5rem 0 0.25rem;
  }
  summary {
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-muted);
    list-style: none;
  }
  summary::before {
    content: '▸';
    display: inline-block;
    transition: transform var(--dur-fast) ease;
  }
  details[open] > summary::before {
    transform: rotate(90deg);
  }
  summary::-webkit-details-marker {
    display: none;
  }
  .body {
    padding-top: 0.5rem;
  }
  @media (prefers-reduced-motion: reduce) {
    summary::before {
      transition: none;
    }
  }
</style>
