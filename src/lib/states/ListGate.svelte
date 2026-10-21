<script lang="ts">
  // Review w17-1: what an EMPTY store-backed list says before it is a first
  // run. A hub this app cannot use, a read that failed and a read still in
  // flight each say so (the r13 Sidebar fix, for the pages it did not reach);
  // only an idle or loaded empty list renders `children`, the first-run copy.
  import type { Snippet } from 'svelte';
  import EmptyState from './EmptyState.svelte';
  import LoadError from './LoadError.svelte';
  import Skeleton from './Skeleton.svelte';
  import type { ListLoad } from '../list_load';
  import { hubStatus } from '../hub';

  let {
    load,
    what,
    onretry,
    testid,
    rows = 3,
    children,
  }: {
    load: ListLoad;
    /** The list, in words: "hosts", "accounts". */
    what: string;
    onretry: () => void;
    testid: string;
    rows?: number;
    children: Snippet;
  } = $props();
</script>

{#if $hubStatus.unavailable}
  <EmptyState
    kind="none"
    testid="{testid}-unavailable"
    title="Not connected to the hub"
    body={`Your ${what} show here once this app can use its hub; the banner above says what is wrong.`} />
{:else if load.state === 'failed' && load.error}
  <LoadError title={`Couldn't load ${what}`} error={load.error} {onretry} testid="{testid}-load-error" />
{:else if load.state === 'loading'}
  <Skeleton {rows} label={`Loading ${what}`} />
{:else}
  {@render children()}
{/if}
