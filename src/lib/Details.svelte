<script lang="ts">
  import { backSession, closeTask, selectedSession } from './selection';
  import { selectedTaskId, sidebarView, taskDetailOpen } from './work_view';
  import SessionDetails from './SessionDetails.svelte';
  import TodayView from './TodayView.svelte';
  import WorkTaskDetail from './WorkTaskDetail.svelte';

  // Today (work graph M9.1) is the empty state; ⌘⇧T opens it in Control.
  // The Work view (M14): a task picked in it shows here until a session is
  // opened (from anywhere) or the task is closed.
  const showTask = $derived($sidebarView === 'work' && !!$selectedTaskId && $taskDetailOpen);
  // Close returns to the session beside the task, else the one picking it put away.
  const backTo = $derived($selectedSession ?? $backSession);
</script>

{#if showTask && $selectedTaskId}
  <WorkTaskDetail
    taskId={$selectedTaskId}
    closeLabel={backTo ? `← Back to ${backTo.friendly_name ?? backTo.tmux_name}` : 'Close'}
    onclose={closeTask}
  />
{:else if $selectedSession}
  <SessionDetails session={$selectedSession} />
{:else}
  <TodayView />
{/if}
