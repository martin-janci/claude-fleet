<script lang="ts">
  import { onDestroy } from 'svelte';
  import { selectedSession, onSessionOpened } from './selection';
  import { todayOpen } from './today';
  import { selectedTaskId, sidebarView, taskDetailOpen } from './work_view';
  import SessionDetails from './SessionDetails.svelte';
  import TodayView from './TodayView.svelte';
  import WorkTaskDetail from './WorkTaskDetail.svelte';

  // Today (work graph M9.1) is the empty state, and ⌘⇧T opens it over a
  // selected session; opening a session from anywhere closes it again.
  const off = onSessionOpened(() => todayOpen.set(false));
  onDestroy(off);

  // The Work view (M14): a task picked in it shows here until a session is
  // opened (from anywhere) or the task is closed; ⌘⇧T still shows Today.
  const showTask = $derived($sidebarView === 'work' && !!$selectedTaskId && $taskDetailOpen && !$todayOpen);
</script>

{#if showTask && $selectedTaskId}
  <WorkTaskDetail
    taskId={$selectedTaskId}
    closeLabel={$selectedSession ? `← Back to ${$selectedSession.friendly_name ?? $selectedSession.tmux_name}` : 'Close'}
    onclose={() => taskDetailOpen.set(false)}
  />
{:else if $selectedSession && !$todayOpen}
  <SessionDetails session={$selectedSession} />
{:else}
  <TodayView onclose={$selectedSession ? () => todayOpen.set(false) : undefined} />
{/if}
