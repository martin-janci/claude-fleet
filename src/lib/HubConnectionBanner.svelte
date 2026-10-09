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
  import Banner from './kit/Banner.svelte';
  import Button from './kit/Button.svelte';
  import StatusDot from './kit/StatusDot.svelte';

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
  // Ticks only while there is a countdown or a lost link to time: a
  // connected hub keeps this banner mounted with nothing to count (review r16).
  $effect(() => {
    if (!retrying && lost === null) return;
    now = Date.now();
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });
  const left = $derived(Math.max(0, Math.ceil((deadline - now) / 1000)));
  // The ticking `now` above turns the well into Signal lost on time.
  const signalLost = $derived(lost !== null && $lostSince !== null && now - $lostSince >= SIGNAL_LOST_AFTER_MS);

  async function retry() {
    pressed = true;
    // A refused retry must not leave the button stuck on "Trying now…".
    if (!(await retryHubNow())) pressed = false;
  }
</script>

{#if text}
  <div class="strip">
    <Banner
      tone={lost ? 'waiting' : 'failed'}
      alert
      headline={text}
      testid="hub-connection-banner"
      state={lost ? (signalLost ? 'signal-lost' : 'reconnecting') : c.state}
    >
      {#snippet lead()}
        {#if lost}
          {#if signalLost}
            <Loader name="signal-lost" size={24} delay={0} testid="hub-lost-loader" />
          {:else}
            <Loader name="gravity-well" size={24} testid="hub-lost-loader" />
          {/if}
        {:else}
          <StatusDot state="failed" label={null} />
        {/if}
      {/snippet}
      {#snippet evidence()}
        {#if retrying}
          <span class="countdown" data-testid="hub-retry-countdown"
            >{pressed || left === 0 ? 'Trying now…' : `Retrying in ${left} s`}</span
          >
        {/if}
        <!-- Review r13: the transport's own words ("error sending request …
             (os error 111)") are for Details, not the line. -->
        {#if lost && lost.reason}<details class="reason" data-testid="hub-lost-reason"><summary>Details</summary>{lost.reason}</details>{/if}
      {/snippet}
      {#snippet action()}
        {#if retrying}
          <Button size="sm" disabled={pressed} testid="hub-retry-now" onclick={() => void retry()}>Retry now</Button>
        {/if}
      {/snippet}
    </Banner>
  </div>
{/if}

<style>
  .strip {
    padding: var(--space-2) var(--space-3) 0;
  }
  .reason {
    overflow-wrap: anywhere;
  }
  .reason summary {
    cursor: pointer;
  }
  .countdown {
    font-variant-numeric: tabular-nums;
  }
</style>
