<!-- A full-width notice for a state that affects everything below it
     (manual: Banner): a bold headline, one meta line with the evidence, at
     most one action. Lead with what happened, then what still works. -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import StatusDot from './StatusDot.svelte';

  let {
    tone,
    headline,
    meta,
    action,
    testid,
  }: {
    tone: 'waiting' | 'failed';
    headline: string;
    meta?: string;
    /** At most one action, usually a `Button size="sm"`. */
    action?: Snippet;
    testid?: string;
  } = $props();
</script>

<div class="of of-banner {tone}" role={tone === 'failed' ? 'alert' : 'status'} data-testid={testid}>
  <StatusDot state={tone} label={null} />
  <div style:flex="1 1 auto">
    <b>{headline}</b>
    {#if meta}<div class="meta">{meta}</div>{/if}
  </div>
  {@render action?.()}
</div>
