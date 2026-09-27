<script lang="ts">
  import { onDestroy } from 'svelte';
  import { selectedSession, onSessionOpened } from './selection';
  import { todayOpen } from './today';
  import { openTaskId } from './work_tree';
  import SessionDetails from './SessionDetails.svelte';
  import TodayView from './TodayView.svelte';
  import WorkTaskDetail from './WorkTaskDetail.svelte';

  // Today (work graph M9.1) is the empty state, and ⌘⇧T opens it over a
  // selected session; opening a session from anywhere closes it again. A
  // task picked in the Work view (M14.2) takes the pane the same way, and
  // opening a session closes it too.
  const off = onSessionOpened(() => {
    todayOpen.set(false);
    openTaskId.set(null);
  });
  onDestroy(off);
</script>

{#if $openTaskId && !$todayOpen}
  {#key $openTaskId}
    <WorkTaskDetail taskId={$openTaskId} onclose={() => openTaskId.set(null)} />
  {/key}
{:else if $selectedSession && !$todayOpen}
  <SessionDetails session={$selectedSession} />
{:else}
  <TodayView onclose={$selectedSession ? () => todayOpen.set(false) : undefined} />
{/if}
