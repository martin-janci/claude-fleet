<script lang="ts">
  // A `fix` block (docs/chat-blocks.md): Control names an autofix finding by
  // id and the person decides it here. Everything drawn comes from the hub's
  // own row (`autofix_findings`), never from the block, so an agent cannot
  // invent a fix or reword what it changes. A finding that is resolved, or
  // that this hub never had, is one quiet line; so is a read that failed,
  // which says so rather than calling the problem resolved.
  import { onMount } from 'svelte';
  import FixCard from '../FixCard.svelte';
  import { findings, findingsError, findingsLoaded, fixedRecently, watchFindings } from '../autofix';
  import type { UiBlock } from '../rich_blocks';

  let { block }: { block: Extract<UiBlock, { kind: 'fix' }> } = $props();

  onMount(() => watchFindings());

  const live = $derived($findings.find((f) => f.id === block.finding_id) ?? null);
  const fixed = $derived($fixedRecently.find((f) => f.id === block.finding_id) ?? null);
</script>

{#if live}
  <div class="wrap" data-testid="rich-fix">
    <FixCard finding={live} />
  </div>
{:else if $findingsError}
  <p class="quiet" data-testid="rich-fix-error">Could not read the problem from the hub; it will try again shortly.</p>
{:else if $findingsLoaded}
  <p class="quiet" data-testid="rich-fix-resolved">
    {#if fixed}Fixed: {fixed.title ?? fixed.kind}.{:else}Already resolved: this problem is no longer reported.{/if}
  </p>
{:else}
  <p class="quiet" data-testid="rich-fix-loading">Reading the problem…</p>
{/if}

<style>
  .wrap {
    margin: 0.4em 0 0.7em;
  }
  .quiet {
    margin: 0.3em 0 0.6em;
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
</style>
