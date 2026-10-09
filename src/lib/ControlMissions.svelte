<!--
  Running missions in Control's Views panel (Orbit Fleet redesign step 9.12,
  Control part): each active mission with its current step, Comet trails
  beside that step while it runs, and "Waits for you" instead of the trails
  while it waits on a person (no loader then). The mission itself lives in
  Work; its name opens it there.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Loader from './Loader.svelte';
  import { getMission, listMissions, missionNow, type MissionNow } from './missions';
  import { onWorkChangedDebounced } from './work';
  import { openElsewhere } from './control_views';

  /** Most missions listed; the rest are in Work. */
  const MAX = 3;

  let running = $state<MissionNow[]>([]);
  let gen = 0;

  async function load() {
    const mine = ++gen;
    const list = await listMissions();
    // A hub that predates missions answers nothing.
    if (!list.ok || !Array.isArray(list.value)) return;
    const active = list.value.filter((m) => m.state === 'active').slice(0, MAX);
    const details = await Promise.all(active.map((m) => getMission(m.id)));
    if (mine !== gen) return;
    running = details.flatMap((d) => (d.ok && d.value?.mission ? [missionNow(d.value)] : []));
  }

  let stop: (() => void) | undefined;
  onMount(() => {
    void load();
    stop = onWorkChangedDebounced(() => void load(), () => 500, () => 3000);
  });
  onDestroy(() => stop?.());
</script>

{#if running.length > 0}
  <section class="missions" aria-label="Running missions" data-testid="control-missions">
    <h4>Running missions</h4>
    <ul>
      {#each running as m (m.id)}
        <li data-testid="control-mission" data-waiting={m.waiting}>
          <button type="button" class="link" onclick={() => openElsewhere('missions')}>{m.name}</button>
          {#if m.waiting}
            <span class="muted">Waits for you</span>
          {:else if m.step}
            {#if m.trails}<Loader name="comet-trails" size={20} label="Working on it" testid="control-mission-trails" />{/if}
            <span class="step">{m.step}</span>
          {/if}
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .missions {
    border-top: 1px solid var(--border);
    padding: 0.4rem 0.6rem;
  }
  h4 {
    margin: 0 0 0.25rem;
    font-size: 0.8rem;
    color: var(--fg-muted);
    font-weight: 600;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  li {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
    font-size: 0.85rem;
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
    min-height: 24px;
  }
  .step {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .muted {
    color: var(--fg-muted);
  }
</style>
