<script lang="ts">
  import { backSession, closeTask, selectedSession } from './selection';
  import { selectedTaskId, sidebarView, taskDetailOpen } from './work_view';
  import SessionDetails from './SessionDetails.svelte';
  import WorkTaskDetail from './WorkTaskDetail.svelte';
  import Button from './kit/Button.svelte';
  import { openToday } from './control';
  import { openNewSessionPicker } from './switcher_request';
  import { shortcutLabel } from './shortcuts';
  import { detectMac } from './terminal_keys';

  // The Work view (M14): a task picked in it shows here until a session is
  // opened (from anywhere) or the task is closed. With nothing picked the
  // column is a quiet empty state: Today lives in Control (⌘⇧T), not here
  // (UX audit 2026-10-09, N4).
  const showTask = $derived($sidebarView === 'work' && !!$selectedTaskId && $taskDetailOpen);
  // Close returns to the session beside the task, else the one picking it put away.
  const backTo = $derived($selectedSession ?? $backSession);
  const mac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
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
  <section class="empty" data-testid="no-session" aria-label="No session open">
    <h2>No session open</h2>
    <p>
      Pick one from the list{$sidebarView === 'work' ? ', or a task to see its sessions' : ''}. The morning
      brief and standup are in Control's Today.
    </p>
    <div class="actions">
      <Button testid="no-session-new" onclick={() => openNewSessionPicker()}
        >New session <span class="chord">{shortcutLabel('new-session', mac)}</span></Button
      >
      <Button variant="quiet" testid="no-session-today" onclick={openToday}
        >Today <span class="chord">{shortcutLabel('today', mac)}</span></Button
      >
    </div>
  </section>
{/if}

<style>
  .empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    padding: var(--space-6);
    text-align: center;
    color: var(--fg-muted);
  }
  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: 600;
    color: var(--fg);
  }
  p {
    margin: 0;
    max-width: 360px;
    font-size: var(--text-sm);
  }
  .actions {
    display: flex;
    gap: var(--space-2);
    margin-top: var(--space-2);
  }
  .chord {
    color: var(--fg-muted);
    font-size: var(--text-2xs);
  }
</style>
