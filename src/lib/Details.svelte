<script lang="ts">
  import { selectedSession } from './selection';
  import SessionDetails from './SessionDetails.svelte';
</script>

{#if $selectedSession}
  <SessionDetails session={$selectedSession} />
{:else}
  <p class="empty" data-testid="details-empty">Pick a session to see details.</p>
{/if}

<style>
  .empty { color: var(--fg-muted); font-size: 0.9rem; }
</style>
