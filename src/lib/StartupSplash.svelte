<script lang="ts">
  // Redesign step 3.15: the cold-start splash from the Startup board. One
  // loader at a time, the one for what is really happening (startup.ts has
  // the stages). Nothing at all for the first 400 ms, so a launch that is
  // quick never flashes it, and nothing on a warm start. When the last list
  // answers it shrinks away and the app is under it, already loaded. A hub
  // client whose hub is lost can Open offline: this computer's sessions in
  // place of the splash (OfflineFleet, offline.ts) until the hub answers.
  import { onMount, untrack } from 'svelte';
  import Loader, { LOADER_DELAY_MS } from './Loader.svelte';
  import { hubStatus } from './hub';
  import { hubConnection, retryHubNow, SIGNAL_LOST_AFTER_MS } from './hub_connection';
  import { hosts } from './hosts';
  import { sessions } from './sessions';
  import { attentionIdleMinutes } from './notify';
  import { waitingForYou } from './waiting_count';
  import { durationMs } from './motion';
  import { tokenPx } from './layout_tokens';
  import { GIVE_UP_MS, splashShown, startupFacts, startupStage, warmStart } from './startup';
  import OfflineFleet from './OfflineFleet.svelte';
  import { leaveOffline, offlineMode, openOffline } from './offline';

  let { onhubsettings }: { onhubsettings: () => void } = $props();

  const stage = $derived(startupStage($startupFacts, $hubStatus.remote, $hubConnection));

  let waited = $state(false);
  let dismissed = $state(false);
  let now = $state(Date.now());
  let hubSince = $state<number | null>(null);
  const mountedAt = Date.now();

  onMount(() => {
    const d = setTimeout(() => (waited = true), LOADER_DELAY_MS);
    return () => clearTimeout(d);
  });
  // The clock ticks only while startup is not done: the splash stays mounted
  // for the app's life, and a 500 ms wake-up for nothing is not free (review
  // r16). A later drop back to `hub` starts it again.
  $effect(() => {
    if (stage === 'done') return;
    now = Date.now();
    const t = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(t);
  });

  $effect(() => {
    if (stage === 'hub') hubSince ??= Date.now();
    else hubSince = null;
  });

  // A launch that is still not in after 20 s (and is not waiting on the hub,
  // which says so with Retry) gives the window back: the panes under it have
  // their own loading states, and a splash must never hide a broken startup.
  const gaveUp = $derived(stage !== 'hub' && now - mountedAt >= GIVE_UP_MS);
  const wanted = $derived(
    waited && !dismissed && !gaveUp && !$warmStart && stage !== 'done' && !$hubStatus.unavailable,
  );
  // On the way out the mark shrinks toward the header logo (a CSS animation,
  // the length of --dur-slow, so Reduced and Off motion shorten it).
  let shown = $state(false);
  let leaving = $state(false);
  $effect(() => {
    if (wanted) {
      shown = true;
      leaving = false;
      return;
    }
    if (!untrack(() => shown)) return;
    leaving = true;
    const t = setTimeout(() => {
      shown = false;
      leaving = false;
    }, durationMs('slow'));
    return () => clearTimeout(t);
  });
  $effect(() => splashShown.set(shown));
  onMount(() => () => splashShown.set(false));
  const hubLost = $derived(stage === 'hub' && hubSince !== null && now - hubSince >= SIGNAL_LOST_AFTER_MS);
  const hubName = $derived($hubStatus.url ?? $hubStatus.configured_url ?? 'the hub');

  // Loader sizes from the splash tokens (app.css), read once.
  const markPx = tokenPx('--splash-mark', 96);
  const stagePx = tokenPx('--splash-stage', 160);

  const visibleHosts = $derived($hosts.filter((h) => !h.hidden));
  const answered = $derived(visibleHosts.filter((h) => h.reachable).length);
  const workRows = $derived($sessions.length);
  const needYou = $derived(
    waitingForYou($sessions, { idleSecs: $attentionIdleMinutes * 60, now: Math.floor(now / 1000) }),
  );

  const title = $derived(
    stage === 'store'
      ? 'Opening Orbit Fleet'
      : stage === 'hub'
        ? hubLost
          ? `Cannot reach ${hubName}`
          : 'Connecting to the hub'
        : stage === 'hosts'
          ? 'Finding hosts'
          : 'Loading your fleet',
  );
  const detail = $derived(
    stage === 'hub'
      ? hubLost
        ? 'Your sessions keep running on their hosts.'
        : hubName
      : stage === 'hosts' && visibleHosts.length > 0
        ? `${answered} of ${visibleHosts.length} answered`
        : stage === 'sessions' && workRows > 0
          ? `${workRows} ${workRows === 1 ? 'session' : 'sessions'}${needYou > 0 ? ` · ${needYou} need you` : ''}`
          : null,
  );

  function hubSettings() {
    dismissed = true;
    leaveOffline();
    onhubsettings();
  }
  // Open offline (new for a paired desktop): this computer's sessions only,
  // in place of the splash, until the hub answers.
  function offline() {
    dismissed = true;
    void openOffline();
  }
</script>

{#if shown}
  <div
    class="startup"
    data-testid="startup-splash"
    data-stage={hubLost ? 'hub-lost' : stage}
    role="status"
    aria-live="polite"
    class:leaving
  >
    <div class="mark">
      {#key hubLost ? 'hub-lost' : stage}
        {#if stage === 'store'}
          <Loader name="draw-on" size={markPx} testid="startup-loader" />
        {:else if stage === 'hub' && hubLost}
          <Loader name="signal-lost" size={markPx} delay={0} testid="startup-loader" />
        {:else if stage === 'hub'}
          <Loader name="chase" size={markPx} testid="startup-loader" />
        {:else if stage === 'hosts'}
          <Loader name="radar" size={stagePx} count={answered} testid="startup-loader" />
        {:else}
          <Loader name="assemble" size={stagePx} count={workRows} testid="startup-loader" />
        {/if}
      {/key}
    </div>
    <p class="title" data-testid="startup-title">{title}</p>
    {#if detail}<p class="detail" data-testid="startup-detail">{detail}</p>{/if}
    {#if stage === 'hosts' && visibleHosts.length > 0}
      <ul class="hosts" data-testid="startup-hosts">
        {#each visibleHosts.slice(0, 6) as h (h.alias)}
          <li data-answered={h.reachable}>{h.alias} <span aria-hidden="true">{h.reachable ? '✓' : '…'}</span></li>
        {/each}
      </ul>
    {/if}
    {#if hubLost}
      <div class="actions">
        <button type="button" class="btn" data-testid="startup-offline" onclick={offline}>Open offline</button>
        <button type="button" class="btn btn--primary" data-testid="startup-retry" onclick={() => void retryHubNow()}>Retry</button>
        <button type="button" class="btn" data-testid="startup-hub-settings" onclick={hubSettings}>Hub settings…</button>
      </div>
    {/if}
  </div>
{/if}

{#if $offlineMode}
  <OfflineFleet onhubsettings={hubSettings} />
{/if}

<style>
  .startup {
    position: fixed;
    inset: 0;
    z-index: 900;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    background: var(--bg);
    color: var(--fg);
  }
  .startup.leaving {
    pointer-events: none;
    animation: startup-out var(--dur-slow) ease-in forwards;
  }
  @keyframes startup-out {
    to {
      opacity: 0;
      transform: translate(-45vw, -45vh) scale(0.1);
    }
  }
  .mark {
    min-height: var(--splash-stage);
    display: flex;
    align-items: center;
    justify-content: center;
    margin-bottom: var(--space-2);
  }
  .title {
    margin: 0;
    font-size: var(--text-md);
    font-weight: 600;
  }
  .detail {
    margin: 0;
    font-size: var(--text-sm);
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .hosts {
    list-style: none;
    margin: var(--space-2) 0 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: var(--space-1) var(--space-3);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--fg-muted);
  }
  .hosts li[data-answered='true'] span {
    color: var(--status-done);
  }
  .actions {
    display: flex;
    gap: var(--space-2);
    margin-top: var(--space-3);
  }
</style>
