<script lang="ts">
  import { onDestroy } from 'svelte';
  import { selectedSession, onSessionOpened } from './selection';
  import { todayOpen } from './today';
  import SessionDetails from './SessionDetails.svelte';
  import TodayView from './TodayView.svelte';

  // Today (work graph M9.1) is the empty state, and ⌘⇧T opens it over a
  // selected session; opening a session from anywhere closes it again.
  const off = onSessionOpened(() => todayOpen.set(false));
  onDestroy(off);
</script>

{#if $selectedSession && !$todayOpen}
  <SessionDetails session={$selectedSession} />
{:else}
  <TodayView onclose={$selectedSession ? () => todayOpen.set(false) : undefined} />
{/if}
