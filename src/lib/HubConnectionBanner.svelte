<script lang="ts">
  // The design's "banner while disconnected" (states kit, step 10.6). Rendered
  // only by a hub client (App.svelte); see hub_connection.ts for where the
  // state comes from. While the link is down it counts down to the next try
  // live and offers Retry now, which cuts the backoff short.
  import { onDestroy } from 'svelte';
  import { hubConnection, connectionBanner, retryHubNow } from './hub_connection';

  let { hubUrl }: { hubUrl: string | null } = $props();
  const c = $derived($hubConnection);
  const text = $derived(connectionBanner(c, hubUrl, { live: true }));
  const retrying = $derived(c.state === 'reconnecting' || c.state === 'offline');

  // The next try, as a deadline on this window's clock: set each time the
  // backend reports a new wait, counted down once a second.
  let deadline = $state(0);
  let now = $state(Date.now());
  let pressed = $state(false);
  $effect(() => {
    if (c.state === 'reconnecting' || c.state === 'offline') {
      deadline = Date.now() + c.retry_in_secs * 1000;
      now = Date.now();
      pressed = false;
    }
  });
  const timer = setInterval(() => (now = Date.now()), 1000);
  onDestroy(() => clearInterval(timer));
  const left = $derived(Math.max(0, Math.ceil((deadline - now) / 1000)));

  async function retry() {
    pressed = true;
    await retryHubNow();
  }
</script>

{#if text}
  <div class="hub-connection-banner" role="alert" data-testid="hub-connection-banner">
    <span class="text">{text}</span>
    {#if retrying}
      <span class="countdown" data-testid="hub-retry-countdown"
        >{pressed || left === 0 ? 'Trying now…' : `Retrying in ${left} s`}</span>
      <button type="button" class="retry" disabled={pressed} data-testid="hub-retry-now" onclick={() => void retry()}>Retry now</button>
    {/if}
  </div>
{/if}

<style>
  .hub-connection-banner {
    display: flex;
    align-items: center;
    gap: 0.8rem;
    padding: 0.35rem 0.8rem;
    background: #5a3a12;
    color: #ffe2b8;
    font-size: 0.8rem;
    border-bottom: 1px solid #8a5a1c;
  }
  .text {
    flex: 1;
  }
  .countdown {
    flex: none;
    font-variant-numeric: tabular-nums;
  }
  .retry {
    flex: none;
    background: transparent;
    color: inherit;
    border: 1px solid currentColor;
    border-radius: 4px;
    padding: 0.1rem 0.5rem;
    font: inherit;
    cursor: pointer;
  }
  .retry:disabled {
    opacity: 0.6;
    cursor: default;
  }
</style>
