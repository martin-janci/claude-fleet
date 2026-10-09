<!--
  Redesign step 9.10: Today's Nudge for stuck missions. Each active or
  paused mission fleet calls stuck, with why and the next step Jev
  proposes; its name opens Missions, where a person picks the step. Asking
  runs no LLM: only Jev, whose answer on the same facts is reused.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import ProposedBy from './ProposedBy.svelte';
  import { preselect } from './ai_proposal';
  import { listMissions } from './missions';
  import { asProposal, missionTriage, stepLabel, type Triage } from './mission_triage';
  import { onWorkChangedDebounced } from './work';
  import { openElsewhere } from './control_views';

  /** Most missions asked about; the rest are in Missions. */
  const MAX = 5;

  let stuck = $state<{ id: number; name: string; triage: Triage }[]>([]);
  let gen = 0;

  async function load() {
    const mine = ++gen;
    const list = await listMissions();
    // A hub that predates missions answers nothing.
    if (!list.ok || !Array.isArray(list.value)) return;
    const open = list.value.filter((m) => m.state === 'active' || m.state === 'paused').slice(0, MAX);
    const answers = await Promise.all(open.map((m) => missionTriage(m.id)));
    if (mine !== gen) return;
    stuck = open.flatMap((m, i) => {
      const r = answers[i];
      return r.ok && r.value?.stuck ? [{ id: m.id, name: m.name, triage: r.value }] : [];
    });
  }

  let stop: (() => void) | undefined;
  onMount(() => {
    void load();
    stop = onWorkChangedDebounced(() => void load(), () => 2000, () => 10000);
  });
  onDestroy(() => stop?.());
</script>

{#if stuck.length > 0}
  <section class="nudge" aria-label="Stuck missions" data-testid="mission-nudge">
    <h3>Stuck missions</h3>
    <ul>
      {#each stuck as m (m.id)}
        {@const next = asProposal(m.triage.next)}
        <li data-testid="mission-nudge-row">
          <button type="button" class="link" onclick={() => openElsewhere('missions')}>{m.name}</button>
          <span class="muted">{m.triage.stuck?.why}</span>
          {#if preselect('mission_next_step', next)}
            <span data-testid="mission-nudge-next">Next: {stepLabel(next?.value ?? '')}</span>
            <ProposedBy proposal={next} field="mission_next_step" testid="mission-nudge-next-by" />
          {/if}
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .nudge h3 {
    margin: 0 0 4px;
    font-size: var(--text-sm);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
</style>
