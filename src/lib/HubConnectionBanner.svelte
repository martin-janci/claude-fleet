<script lang="ts">
  // The design's "banner while disconnected". Rendered only by a hub client
  // (App.svelte); see hub_connection.ts for where the state comes from.
  import { hubConnection, connectionBanner } from './hub_connection';

  let { hubUrl }: { hubUrl: string | null } = $props();
  const text = $derived(connectionBanner($hubConnection, hubUrl));
</script>

{#if text}
  <div class="hub-connection-banner" role="alert" data-testid="hub-connection-banner">{text}</div>
{/if}

<style>
  .hub-connection-banner {
    padding: 0.35rem 0.8rem;
    background: var(--waiting-faint);
    color: var(--fg);
    font-size: 0.8rem;
    border-bottom: 1px solid var(--waiting-line);
  }
</style>
