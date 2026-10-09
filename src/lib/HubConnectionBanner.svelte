<script lang="ts">
  // The design's "banner while disconnected" (states kit, step 10.6). Rendered
  // only by a hub client (App.svelte); see hub_connection.ts for where the
  // state comes from. While the link is down it counts down to the next try
  // live and offers Retry now, which cuts the backoff short.
  //
  // Redesign step 3.14: a lost link reads as the design writes it ("Lost
  // the hub at 14:52 · try 3 · your sessions keep running on their hosts")
  // beside the kit's Gravity well, which turns into Signal lost once the hub
  // has been gone SIGNAL_LOST_AFTER_MS. A strip above the layout, never an
  // overlay: the list under it stays usable. The wire-contract states keep
  // their sentence and take no loader; nothing is being waited for there.
  import { onDestroy } from 'svelte';
  import {
    hubConnection,
    connectionBanner,
    retryHubNow,
    isLost,
    lostLine,
    lostSince,
    SIGNAL_LOST_AFTER_MS,
  } from './hub_connection';
  import Loader from './Loader.svelte';

  let { hubUrl }: { hubUrl: string | null } = $props();
  const c = $derived($hubConnection);
  const lost = $derived(isLost(c) ? c : null);
  const text = $derived(lostLine(c, $lostSince, hubUrl) ?? connectionBanner(c, hubUrl, { live: true }));
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
  // The ticking `now` above turns the well into Signal lost on time.
  const signalLost = $derived(lost !== null && $lostSince !== null && now - $lostSince >= SIGNAL_LOST_AFTER_MS);

  async function retry() {
    pressed = true;
    await retryHubNow();
  }
</script>

{#if text}
  <div
    class="hub-connection-banner"
    role="alert"
    data-testid="hub-connection-banner"
    data-state={lost ? (signalLost ? 'signal-lost' : 'reconnecting') : c.state}
  >
    {#if lost}
      {#if signalLost}
        <Loader name="signal-lost" size={24} delay={0} testid="hub-lost-loader" />
      {:else}
        <Loader name="gravity-well" size={24} testid="hub-lost-loader" />
      {/if}
    {/if}
    <span class="text">{text}</span>
    {#if lost}<span class="reason" title={lost.reason}>{lost.reason}</span>{/if}
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
    background: var(--waiting-faint);
    color: var(--fg);
    font-size: var(--text-2xs);
    border-bottom: 1px solid var(--waiting-line);
  }
  .text {
    flex: 1;
  }
  .reason {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    opacity: 0.75;
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
    border-radius: var(--radius-sm);
    padding: 0.1rem 0.5rem;
    font: inherit;
    cursor: pointer;
  }
  .retry:disabled {
    opacity: 0.6;
    cursor: default;
  }
</style>
