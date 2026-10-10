<script lang="ts">
  // The Inbox's missions waiting on a person (gap plan G1.6, the Main
  // board's "Hub federation v2 · sign the autonomy grant" row): each one
  // the hub says waits (`waiting_on`), with why and how long, opening the
  // mission. Their count is part of the Needs you badge.
  import { onMount } from 'svelte';
  import { shortAge } from './session_status';
  import { openMission } from './missions';
  import { loadWaitingMissions, waitWords, waitingMissions } from './mission_waits';

  // `trackWaitingMissions` (main.ts) keeps the list fresh; opening the
  // Inbox reads it once more so it is current as it shows.
  onMount(() => void loadWaitingMissions());
</script>

{#if $waitingMissions.length > 0}
  <div class="waits" data-testid="mission-waits" role="group" aria-label="Missions waiting on you">
    {#each $waitingMissions as m (m.id)}
      <div class="item" data-testid="mission-wait" data-reason={m.waiting_on.reason}>
        <span class="dot" aria-label="Needs you"></span>
        <div class="text">
          <button type="button" class="name" data-testid="mission-wait-open" onclick={() => openMission(m.id)}
            >{m.name} <span class="chip">mission</span></button
          >
          <span class="line">{waitWords(m.waiting_on)} · {shortAge(m.waiting_on.since)}</span>
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .waits { display: flex; flex-direction: column; gap: 2px; padding: 4px 8px; }
  .item { display: flex; gap: 8px; align-items: flex-start; padding: 6px 4px; border-radius: var(--radius-md); }
  .item:hover { background: var(--bg-hover); }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--status-waiting); margin-top: 5px; flex: none; }
  .text { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .name { border: 0; background: none; padding: 0; color: var(--fg); font: inherit; font-weight: 500; text-align: left; cursor: pointer; }
  .chip { font-size: var(--text-2xs); color: var(--fg-muted); border: 1px solid var(--border); border-radius: var(--radius-pill); padding: 0 6px; font-weight: 400; }
  .line { color: var(--fg-muted); font-size: var(--text-xs); overflow-wrap: anywhere; }
</style>
