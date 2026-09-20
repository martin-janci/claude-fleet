<script lang="ts">
  import { hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { canMoveSession, moveBlockedReason } from './moveEligibility';
  import { moves, stepNumber, transferSheetFor } from './moves';
  import type { SessionRow } from './sessions';

  // The terminal header's host name, which is also the Transfer button — and,
  // while this session is moving, the live indicator that reopens the sheet.
  let { session }: { session: SessionRow } = $props();

  const run = $derived($moves.get(session.id));
  const blocked = $derived(moveBlockedReason($hubStatus, $hubConnection));

  function open() {
    transferSheetFor.set(session.id);
  }
</script>

<span class="host">
  {#if run}
    <button class="chip live" data-state={run.status} onclick={open} data-testid="transfer-live">
      {#if run.status === 'running'}
        ⇄ moving to {run.toHost} · {stepNumber(run)}/{run.steps.length}
      {:else if run.status === 'done'}
        ⇄ moved to {run.toHost}
      {:else}
        ⇄ move failed
      {/if}
    </button>
  {:else if canMoveSession(session)}
    on
    <button
      class="chip"
      onclick={open}
      disabled={blocked !== null}
      title={blocked ?? 'Transfer this session to another host'}
      data-testid="transfer-chip"
    >
      {session.host_alias} ⇄
    </button>
  {:else}
    on {session.host_alias}
  {/if}
</span>

<style>
  .host { color: var(--fg-muted); font-size: 0.75rem; }
  .chip {
    font: inherit;
    color: inherit;
    background: none;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 0.35rem;
    cursor: pointer;
  }
  .chip:disabled { cursor: default; opacity: 0.6; }
  .chip.live { color: var(--accent, inherit); border-color: currentColor; }
  .chip.live[data-state='failed'], .chip.live[data-state='partial'] { color: #e64a4a; }
</style>
