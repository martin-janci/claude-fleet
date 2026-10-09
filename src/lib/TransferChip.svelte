<script lang="ts">
  import { hubStatus } from './hub';
  import { hubConnection } from './hub_connection';
  import { canMoveSession, moveBlockedReason } from './moveEligibility';
  import { moves, runForSession, stepNumber, transferSheetFor } from './moves';
  import { sessionBlocked } from './share';
  import type { SessionRow } from './sessions';

  // The terminal header's host name, which is also the Transfer button — and,
  // while this session is moving, the live indicator that reopens the sheet.
  let { session }: { session: SessionRow } = $props();

  // Not `$moves.get(session.id)`: a run is keyed by the SOURCE session, and
  // by the time a move finishes that row is usually gone — so the session it
  // produced is the one left to show the result on. Row ids are reused, so
  // the lookup also proves the run belongs to this session.
  const run = $derived(runForSession($moves, session));
  /** This session is the one the move PRODUCED, not the one that moved. */
  const arrived = $derived(run !== undefined && run.sessionId !== session.id);
  // Both halves, in the order `share.ts` documents: what the hub refuses a
  // client outright, then who this client is on THIS row (multi-user M1 — the
  // move lifecycle is spec §4.3's `own` tier, so a grantee may not start one).
  // `SessionDetails`' own "Move to host…" composes exactly these two; this
  // chip is the same control in the terminal header and used to ask only the
  // hub's half, so a watcher's header still offered the move.
  const blocked = $derived(
    moveBlockedReason($hubStatus, $hubConnection) ?? $sessionBlocked(session, 'move_session'),
  );

  function open() {
    if (!run && blocked !== null) return;
    transferSheetFor.set(run ? run.sessionId : session.id);
  }
</script>

<span class="host">
  {#if run}
    <button
      class="btn btn--quiet is-bounded chip live"
      data-state={run.status}
      onclick={open}
      title="Show the transfer to {run.toHost}"
      aria-label="Show the transfer to {run.toHost}"
      data-testid="transfer-live"
    >
      {#if arrived}
        ⇄ moved from {run.fromHost}
      {:else if run.status === 'running'}
        ⇄ moving to {run.toHost} · {stepNumber(run)}/{run.steps.length}
      {:else if run.status === 'waiting'}
        ⇄ waiting to move to {run.toHost}
      {:else if run.status === 'done'}
        ⇄ moved to {run.toHost}
      {:else if run.status === 'partial'}
        ⇄ move incomplete
      {:else}
        ⇄ move failed
      {/if}
    </button>
  {:else if canMoveSession(session)}
    on
    <button
      class="btn btn--quiet is-bounded chip"
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
  .host { color: var(--fg-muted); font-size: var(--text-2xs); }
  .chip.live { color: var(--accent); border-color: currentColor; }
  .chip.live[data-state='failed'], .chip.live[data-state='partial'] { color: var(--danger); }
</style>
