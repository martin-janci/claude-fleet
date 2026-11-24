<!--
  Redesign step 9.10: Today's Nudge for stuck missions. Each active or
  paused mission fleet calls stuck, with why and the next step Jev
  proposes; its name opens Missions, where a person picks the step. Asking
  runs no LLM: only Jev, asked once per mission per state (missionTriageOnce).
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import ProposedBy from './ProposedBy.svelte';
  import { preselect } from './ai_proposal';
  import { listMissions } from './missions';
  import { asProposal, missionTriageOnce, stepLabel, type Triage } from './mission_triage';
  import { onWorkChangedDebounced } from './work';
  import { openElsewhere } from './control_views';
  import { missionInScope } from './mission_waits';
  import { effectiveScope } from './orgs';

  /** Most stuck missions shown; the rest are in Missions. */
  const MAX = 5;
  /** Most missions Jev is asked about in one read (each answer is cached per
   *  mission per state, so a re-read asks only about the ones that moved). */
  const ASK_MAX = 20;

  let stuck = $state<{ id: number; name: string; org_id?: number | null; triage: Triage }[]>([]);
  // Cut to the sidebar's org scope like the rest of Today, and capped only
  // after the stuck ones are picked (review w09-2): a braked mission is no
  // longer updated, so it sinks below the ones still running.
  const shown = $derived(stuck.filter((m) => missionInScope(m, $effectiveScope)).slice(0, MAX));
  let gen = 0;

  async function load() {
    const mine = ++gen;
    const list = await listMissions();
    // A hub that predates missions answers nothing.
    if (!list.ok || !Array.isArray(list.value)) return;
    // Paused ones are picked first when there are too many to ask about: a
    // brake pauses a mission, so they are the likeliest to be stuck.
    const all = list.value.filter((m) => m.state === 'active' || m.state === 'paused');
    const picked = new Set(
      [...all.filter((m) => m.state === 'paused'), ...all.filter((m) => m.state !== 'paused')].slice(0, ASK_MAX),
    );
    const open = all.filter((m) => picked.has(m));
    // Asked once per mission per state: a work change re-reads the list, but
    // Jev is only asked again about a mission that moved (review r15).
    const answers = await Promise.all(open.map((m) => missionTriageOnce(m)));
    if (mine !== gen) return;
    stuck = open.flatMap((m, i) => {
      const r = answers[i];
      return r.ok && r.value?.stuck ? [{ id: m.id, name: m.name, org_id: m.org_id, triage: r.value }] : [];
    });
  }

  let stop: (() => void) | undefined;
  onMount(() => {
    void load();
    stop = onWorkChangedDebounced(() => void load(), () => 2000, () => 10000);
  });
  onDestroy(() => stop?.());
</script>

{#if shown.length > 0}
  <section class="nudge" aria-label="Stuck missions" data-testid="mission-nudge">
    <h3>Stuck missions</h3>
    <ul>
      {#each shown as m (m.id)}
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
