<!--
  Thread room escalations (control_escalations.ts): what in the fleet needs
  the person, as amber cards above Control's composer. Open shows the
  session beside the chat, where its question can be answered; Later puts
  the card off until that session needs you again, across restarts. By
  default only Control's own threads raise a card, and not one the hub
  stopped following or someone muted (F10); "All fleet" shows the rest. More than a few: the rest are counted, and the count opens the
  Needs you tab.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import {
    ESCALATIONS_SHOWN,
    controlSessionIds,
    escalationScope,
    escalations,
    forgetRecoveredHosts,
    laterEscalations,
    putOff,
  } from './control_escalations';
  import { visibleFleet, selectView } from './control_views';
  import { operatorSession } from './operator';
  import { openInControl } from './control_room';
  import { displayName } from './attention';
  import { followHandoffs, handoffs } from './handoffs';
  import { refreshMutes, threadFollows } from './control_journal';
  import { hosts } from './hosts';

  const release = followHandoffs();
  onDestroy(release);
  // A mute or a new brief is a `handoff:changed`: re-read the hub's follow
  // state with the receipts (F10).
  const stopFollows = handoffs.subscribe(() => void refreshMutes());
  onDestroy(stopFollows);
  // A host back up ends the Later on its outage card, so its next outage
  // raises a card again.
  const stopHosts = hosts.subscribe((h) => forgetRecoveredHosts(h));
  onDestroy(stopHosts);

  const mine = $derived(controlSessionIds($visibleFleet.rows, $handoffs, $threadFollows));
  const opId = $derived($operatorSession?.id ?? null);
  const fleetAll = $derived(escalations($visibleFleet.rows, $visibleFleet.opts, opId, $laterEscalations, $hosts));
  const ours = $derived(escalations($visibleFleet.rows, $visibleFleet.opts, opId, $laterEscalations, $hosts, mine));
  const all = $derived($escalationScope === 'fleet' ? fleetAll : ours);
  const shown = $derived(all.slice(0, ESCALATIONS_SHOWN));
  const more = $derived(all.length - shown.length);
  // The switch shows only when it would change what is drawn.
  const others = $derived(fleetAll.length - ours.length);
</script>

{#if shown.length > 0 || others > 0}
  <ul class="escalations" data-testid="control-escalations" aria-label="Needs you" aria-live="polite">
    {#each shown as e (e.key)}
      <li class="card" data-testid="control-escalation" data-bucket={e.bucket}>
        <span class="mark" aria-hidden="true">!</span>
        <div class="text">
          <span class="line">{e.line}</span>
          {#if e.detail}<span class="detail" data-testid="control-escalation-detail">{e.detail}</span>{/if}
        </div>
        {#if e.session}
          {@const s = e.session}
          <button
            type="button"
            class="btn btn--chip"
            data-testid="control-escalation-open"
            onclick={() => openInControl(s.id, displayName(s, true))}>Open</button
          >
        {/if}
        <button type="button" class="btn btn--quiet" data-testid="control-escalation-later" onclick={() => putOff(e.key)}>Later</button>
      </li>
    {/each}
    {#if more > 0 || others > 0}
      <li class="foot">
        {#if more > 0}
          <button type="button" class="more" data-testid="control-escalations-more" onclick={() => selectView('needs-you')}
            >{more} more in Needs you</button
          >
        {/if}
        {#if others > 0 || $escalationScope === 'fleet'}
          <button
            type="button"
            class="more scope"
            data-testid="control-escalations-scope"
            onclick={() => escalationScope.set($escalationScope === 'fleet' ? 'control' : 'fleet')}
            >{$escalationScope === 'fleet' ? "Only Control's threads" : `All fleet (${others} more)`}</button
          >
        {/if}
      </li>
    {/if}
  </ul>
{/if}

<style>
  .escalations {
    flex-basis: 100%;
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .card {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 0.4rem var(--space-3);
    border: 1px solid var(--border);
    border-left: 3px solid var(--usage-warn);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--usage-warn) 8%, var(--bg-pane));
    font-size: var(--text-xs);
  }
  .mark {
    color: var(--usage-warn);
    font-weight: 700;
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .line {
    color: var(--fg);
  }
  .detail {
    color: var(--fg-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .foot {
    display: flex;
    gap: var(--space-3);
  }
  .scope {
    margin-left: auto;
  }
  .more {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font-size: var(--text-xs);
  }
</style>
