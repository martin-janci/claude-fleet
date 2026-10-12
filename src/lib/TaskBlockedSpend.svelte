<!--
  Redesign step 6.3 on a task: the derived Blocked state and the spend of
  its sessions, as the Work and TaskDetail boards draw them. The status-word
  decision (transition plan, "Status words") shows a blocked task as Needs
  you with its reason line, "Blocked on TASK-212"; the attention model keeps
  Blocked as its own state underneath. Shared by the Work list, the grouped
  tree, the board's cards and the task's Details, so the four never word it
  differently.
-->
<script lang="ts">
  import StatusChip from './kit/StatusChip.svelte';
  import { blockedOnLine, taskSpend, type WorkTask } from './work_view';

  let {
    task,
    lookup,
    testid = 'task',
  }: {
    task: Pick<WorkTask, 'blocked' | 'blocked_by' | 'cost_micros'>;
    /** The task a dependency id names, when this view has it loaded. */
    lookup?: (id: string) => Pick<WorkTask, 'key' | 'title'> | null | undefined;
    testid?: string;
  } = $props();

  const reason = $derived(blockedOnLine(task, lookup));
  const spend = $derived(taskSpend(task));
</script>

{#if reason}
  <span class="blocked" data-testid="{testid}-blocked">
    <StatusChip state="waiting" />
    <span class="reason" data-testid="{testid}-blocked-reason">{reason}</span>
  </span>
{/if}
{#if spend}
  <span class="spend" data-testid="{testid}-spend" title="Spend of its sessions, each counted once">{spend}</span>
{/if}

<style>
  .blocked {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .reason {
    color: var(--status-waiting);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spend {
    font-variant-numeric: tabular-nums;
    color: var(--fg-muted);
  }
</style>
