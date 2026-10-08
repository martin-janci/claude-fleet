<script lang="ts">
  // Redesign step 3.14: the status bar's 16 px mark says how the app stands
  // without a word. Breathe while idle and connected (standalone counts:
  // its fleet is right here); Halo once a session waits for you, following
  // the same count as the Sidebar's Needs you; Chase while a hub client is
  // still connecting; Signal lost when the configured hub is unavailable.
  //
  // It shows nothing while the fleet is still arriving (the empty pane's
  // Particle swarm is that screen's loader, step 3.13) or while the link is
  // reconnecting (the banner's Gravity well is), so a screen keeps one
  // loader. The OS tray and dock (Breathe, Chase, Halo, Signal lost there
  // too) need a native tray this app does not have yet.
  import { onMount } from 'svelte';
  import Loader from './Loader.svelte';
  import { hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { sessions, sessionsAnswered } from './sessions';
  import { attentionIdleMinutes } from './notify';
  import { waitingForYou } from './waiting_count';

  let nowSec = $state(Math.floor(Date.now() / 1000));
  onMount(() => {
    // The idle rule ages rows; the Sidebar re-reads it on the same cadence.
    const t = setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  });

  const waiting = $derived(waitingForYou($sessions, { idleSecs: $attentionIdleMinutes * 60, now: nowSec }));

  type Mark = 'breathe' | 'halo' | 'chase' | 'signal-lost';
  const mark = $derived.by((): Mark | null => {
    if ($hubStatus.unavailable) return 'signal-lost';
    if ($hubStatus.remote) {
      if ($hubConnection.state === 'connecting') return 'chase';
      if ($hubConnection.state !== 'connected') return null;
    }
    if (!$sessionsAnswered) return null;
    return waiting > 0 ? 'halo' : 'breathe';
  });

  const label = $derived(
    mark === 'signal-lost'
      ? 'Hub unavailable'
      : mark === 'chase'
        ? 'Connecting to the hub'
        : mark === 'halo'
          ? `${waiting} ${waiting === 1 ? 'session waits' : 'sessions wait'} for you`
          : 'Idle and connected',
  );
</script>

{#if mark}
  <span class="status-mark" data-testid="status-mark" data-mark={mark} title={label}>
    {#if mark === 'breathe'}
      <Loader name="breathe" size={16} delay={0} {label} testid="status-mark-loader" />
    {:else if mark === 'halo'}
      <Loader name="halo" size={16} delay={0} {label} testid="status-mark-loader" />
    {:else if mark === 'chase'}
      <Loader name="chase" size={16} {label} testid="status-mark-loader" />
    {:else}
      <Loader name="signal-lost" size={16} delay={0} {label} testid="status-mark-loader" />
    {/if}
  </span>
{/if}

<style>
  .status-mark {
    display: inline-flex;
    align-items: center;
    flex: none;
  }
</style>
