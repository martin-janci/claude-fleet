<!--
  A folding section of Control's Views panel (boards MissionControl, MCTasks,
  MCViews): "▾ Needs you 4" over its rows. The header is a button; the
  rows stay out of the DOM while folded.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import Count from './kit/Count.svelte';

  let {
    label,
    count,
    open = $bindable(true),
    tone = null,
    testid,
    children,
  }: {
    label: string;
    count: number;
    open?: boolean;
    /** `waiting` colours the label as the attention colour (Needs you). */
    tone?: 'waiting' | null;
    testid: string;
    children: Snippet;
  } = $props();
</script>

<section class="fold" data-testid={testid} data-open={open}>
  <button
    type="button"
    class="head"
    class:waiting={tone === 'waiting'}
    aria-expanded={open}
    data-testid="{testid}-head"
    onclick={() => (open = !open)}
    ><span class="caret" aria-hidden="true">{open ? '▾' : '▸'}</span>{label}<Count n={count} /></button
  >
  {#if open}{@render children()}{/if}
</section>

<style>
  .fold {
    padding: 0 var(--space-1);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    border: 0;
    background: transparent;
    font: inherit;
    font-size: var(--text-2xs);
    font-weight: 500;
    color: var(--fg-muted);
    padding: var(--space-3) var(--space-2) var(--space-1);
    cursor: pointer;
    text-align: left;
  }
  .head.waiting {
    color: var(--status-waiting);
  }
  .head:hover {
    color: var(--fg);
  }
  .head:focus-visible {
    outline: 2px solid var(--ring);
    outline-offset: -2px;
  }
  .caret {
    width: 10px;
  }
</style>
