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
    evidence,
    lead,
    alert,
    action,
    testid,
    state,
  }: {
    tone: 'waiting' | 'failed';
    headline: string;
    meta?: string;
    /** The meta line when it carries more than words: a URL in code, a
     *  Details disclosure with the raw error, a countdown. */
    evidence?: Snippet;
    /** In place of the dot, the loader that is this banner's state (Gravity
     *  well while reconnecting, Signal lost once the link is gone). */
    lead?: Snippet;
    /** Announce it as an alert whatever its tone (a lost hub link). */
    alert?: boolean;
    /** At most one action, usually a `Button size="sm"`. */
    action?: Snippet;
    testid?: string;
    /** `data-state`, for the consumer's own state name. */
    state?: string;
  } = $props();
</script>

<div
  class="of of-banner {tone}"
  role={alert || tone === 'failed' ? 'alert' : 'status'}
  data-testid={testid}
  data-state={state}
>
  {#if lead}{@render lead()}{:else}<StatusDot state={tone} label={null} />{/if}
  <div style:flex="1 1 auto" style:min-width="0">
    <b>{headline}</b>
    {#if meta || evidence}<div class="meta">{meta ?? ''}{@render evidence?.()}</div>{/if}
  </div>
  {@render action?.()}
</div>
