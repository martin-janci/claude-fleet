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
  import { onDestroy, onMount, tick } from 'svelte';
  import {
    ESCALATIONS_SHOWN,
    controlMissionIds,
    controlSessionIds,
    missionEscalations,
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
  import { loadMissionList, missionList } from './mission_room';
  import { openMission } from './missions';
  import { onWorkChangedDebounced } from './work';

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

  // The missions, for those that wait on the person (w04-7).
  onMount(() => {
    void loadMissionList();
    return onWorkChangedDebounced(
      () => void loadMissionList(),
      () => 500,
      () => 3000,
    );
  });

  const mine = $derived(controlSessionIds($visibleFleet.rows, $handoffs, $threadFollows));
  const myMissions = $derived(controlMissionIds($handoffs, $threadFollows));
  const opId = $derived($operatorSession?.id ?? null);
  const fleetAll = $derived([
    ...missionEscalations($missionList.missions, $laterEscalations),
    ...escalations($visibleFleet.rows, $visibleFleet.opts, opId, $laterEscalations, $hosts),
  ]);
  const ours = $derived([
    ...missionEscalations($missionList.missions, $laterEscalations, myMissions),
    ...escalations($visibleFleet.rows, $visibleFleet.opts, opId, $laterEscalations, $hosts, mine),
  ]);
  const all = $derived($escalationScope === 'fleet' ? fleetAll : ours);
  const shown = $derived(all.slice(0, ESCALATIONS_SHOWN));
  const more = $derived(all.length - shown.length);
  // The switch shows only when it would change what is drawn.
  const others = $derived(fleetAll.length - ours.length);

  let list = $state<HTMLElement | null>(null);
  const uid = `esc-${Math.random().toString(36).slice(2, 8)}`;

  /** Later removes the card under the focused button: focus the Later on
   *  the card that takes its place (or the one before it), else a button
   *  in the foot, so focus never falls to the page (review w19-11). The
   *  last card gone unmounts the list: focus goes to the nearest composer
   *  box around it instead. */
  async function later(key: string, at: number) {
    const hadFocus = list?.contains(document.activeElement) ?? false;
    const around = list?.parentElement ?? null;
    putOff(key);
    if (!hadFocus) return;
    await tick();
    const lefts = Array.from(list?.querySelectorAll<HTMLElement>('[data-testid="control-escalation-later"]') ?? []);
    const next =
      lefts[Math.min(at, lefts.length - 1)] ?? list?.querySelector<HTMLElement>('.foot button') ?? composerNear(around);
    next?.focus();
  }

  /** The closest textarea in or above `el`'s ancestors: the composer the
   *  cards sit over. */
  function composerNear(el: HTMLElement | null): HTMLElement | null {
    for (let at = el; at; at = at.parentElement) {
      const box = at.querySelector<HTMLElement>('textarea:not([disabled])');
      if (box) return box;
    }
    return null;
  }
</script>

{#if shown.length > 0 || others > 0}
  <ul class="escalations" data-testid="control-escalations" aria-label="Needs you" aria-live="polite" bind:this={list}>
    {#each shown as e, i (e.key)}
      <li class="card" data-testid="control-escalation" data-bucket={e.bucket}>
        <span class="mark" aria-hidden="true">!</span>
        <div class="text">
          <span class="line" id="{uid}-{i}">{e.line}</span>
          {#if e.detail}<span class="detail" data-testid="control-escalation-detail">{e.detail}</span>{/if}
        </div>
        {#if e.session}
          {@const s = e.session}
          <button
            type="button"
            class="btn btn--chip"
            data-testid="control-escalation-open"
            aria-describedby="{uid}-{i}"
            onclick={() => openInControl(s.id, displayName(s, true))}>Open</button
          >
        {:else if e.missionId != null}
          {@const id = e.missionId}
          <button type="button" class="btn btn--chip" data-testid="control-escalation-open" aria-describedby="{uid}-{i}" onclick={() => openMission(id)}
            >Open</button
          >
        {/if}
        <button
          type="button"
          class="btn btn--quiet"
          data-testid="control-escalation-later"
          aria-describedby="{uid}-{i}"
          onclick={() => void later(e.key, i)}>Later</button
        >
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
