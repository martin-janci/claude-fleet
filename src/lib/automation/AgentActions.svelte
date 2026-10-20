<script lang="ts">
  // The actions the built-in agents propose and a person answers (agents
  // design step 5): under the Inbox's Needs you, and in Automation's Agents
  // tab. Each says what it will do ("Post a review on o/r#4: request
  // changes, 2 comments"), shows what it will post on Show, and takes
  // Approve or Reject. An approved action is carried out by its agent on
  // the next run pass (`service::agents::execute_approved`); its count is
  // part of the Needs you badge (`pendingAgentCount`). What the person's
  // approvals of the last day came to ("Done: … — created task 42",
  // "Failed: … — why") shows under them (`status.recent`, review w08-3).
  import { shortAge } from '../session_status';
  import { push, pushError } from '../toasts';
  import {
    actionBody,
    actionOutcome,
    actionTitle,
    answerAgentAction,
    pendingAgentActions,
    recentAgentOutcomes,
    type AgentAction,
  } from '../agents';

  // One id per call in flight (review w11-13): a single id let the first
  // call's return re-enable a second item's buttons mid-call.
  let busy = $state<ReadonlySet<number>>(new Set());
  const setBusy = (id: number, on: boolean) => {
    const next = new Set(busy);
    if (on) next.add(id);
    else next.delete(id);
    busy = next;
  };
  let shown = $state<number | null>(null);

  async function answer(a: AgentAction, approve: boolean) {
    setBusy(a.id, true);
    const r = await answerAgentAction(a.id, approve);
    setBusy(a.id, false);
    if (!r.ok) pushError(r.error, approve ? 'Approve failed' : 'Reject failed');
    else push({ kind: 'success', message: approve ? 'Approved: it runs within a minute, and what it did shows here' : 'Rejected' });
  }
</script>

{#if $pendingAgentActions.length > 0}
  <div class="actions-list" data-testid="agent-actions" role="group" aria-label="Agent actions waiting for you">
    {#each $pendingAgentActions as a (a.id)}
      {@const body = actionBody(a)}
      <div class="item" data-testid="agent-action">
        <span class="dot" aria-label="Waiting for you"></span>
        <div class="text">
          <span class="name">{actionTitle(a)} <span class="chip">agent</span></span>
          <span class="line">proposed {shortAge(a.created_at)}</span>
          <span class="buttons">
            <button type="button" class="btn btn--chip" data-testid="agent-action-approve" disabled={busy.has(a.id)} onclick={() => answer(a, true)}
              >Approve</button
            >
            <button type="button" class="btn btn--chip" data-testid="agent-action-reject" disabled={busy.has(a.id)} onclick={() => answer(a, false)}
              >Reject</button
            >
            {#if body}
              <button type="button" class="btn btn--chip" data-testid="agent-action-show" aria-expanded={shown === a.id} onclick={() => (shown = shown === a.id ? null : a.id)}
                >{shown === a.id ? 'Hide' : 'Show'}</button
              >
            {/if}
          </span>
          {#if body && shown === a.id}<pre class="body" data-testid="agent-action-body">{body}</pre>{/if}
        </div>
      </div>
    {/each}
  </div>
{/if}
{#if $recentAgentOutcomes.length > 0}
  <div class="actions-list" data-testid="agent-outcomes" role="group" aria-label="What your approvals did">
    {#each $recentAgentOutcomes as a (a.id)}
      <div class="item" data-testid="agent-outcome">
        <span class="dot" class:failed={a.state === 'failed'} class:done={a.state !== 'failed'} aria-hidden="true"></span>
        <div class="text">
          <span class="outcome">{actionOutcome(a)}</span>
          {#if a.executed_at}<span class="line">{shortAge(a.executed_at)}</span>{/if}
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .actions-list { display: flex; flex-direction: column; gap: 2px; padding: 4px 8px; }
  .item { display: flex; gap: 8px; align-items: flex-start; padding: 6px 4px; border-radius: var(--radius-md); }
  .item:hover { background: var(--bg-hover); }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--status-waiting); margin-top: 5px; flex: none; }
  .dot.done { background: var(--status-done); }
  .dot.failed { background: var(--status-failed); }
  .outcome { color: var(--fg); overflow-wrap: anywhere; font-size: var(--text-xs); }
  .text { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .name { color: var(--fg); font-weight: 500; overflow-wrap: anywhere; }
  .chip { font-size: var(--text-2xs); color: var(--fg-muted); border: 1px solid var(--border); border-radius: var(--radius-pill); padding: 0 6px; font-weight: 400; }
  .line { color: var(--fg-muted); font-size: var(--text-xs); }
  .buttons { display: flex; gap: 6px; margin-top: 4px; }
  .body { max-height: 240px; overflow: auto; white-space: pre-wrap; font-size: var(--text-xs); background: var(--bg-sunk); border-radius: var(--radius-md); padding: 6px; margin: 4px 0 0; }
</style>
