<!--
  Running missions in Control's Views panel (Orbit Fleet redesign step 9.12,
  Control part): each active mission with its current step, Comet trails
  beside that step while it runs, and "Waits for you" instead of the trails
  while it waits on a person (no loader then), and how many child sessions
  work on it. The mission itself lives in
  Work; its name opens it there.

  The section folds (2026-11-06): its heading toggles it, and the choice is
  kept (`runningMissionsFold` in prefs.ts), so it can give its room to the
  session above it.
-->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Loader from './Loader.svelte';
  import { getMission, listMissions, missionNow, openMission, type MissionNow } from './missions';
  import { onWorkChangedDebounced } from './work';
  import { runningMissionsFold } from './prefs';

  /** Most missions listed; the rest are in Work. */
  const MAX = 3;

  let running = $state<MissionNow[]>([]);
  /** Every active mission, not only the ones listed. */
  let total = $state(0);
  /** Of those past [`MAX`], how many wait on a person (the list's `waiting_on`). */
  let waitingPast = $state(0);
  // The pref is never `auto` here: open unless folded.
  const open = $derived($runningMissionsFold !== 'closed');
  /** What waits for you, said on the folded heading so it is not out of sight. */
  const waiting = $derived(running.filter((m) => m.waiting).length + waitingPast);
  let gen = 0;

  async function load() {
    const mine = ++gen;
    const list = await listMissions();
    // A hub that predates missions answers nothing.
    if (!list.ok || !Array.isArray(list.value)) return;
    const all = list.value.filter((m) => m.state === 'active');
    const active = all.slice(0, MAX);
    const details = await Promise.all(active.map((m) => getMission(m.id)));
    if (mine !== gen) return;
    running = details.flatMap((d) => (d.ok && d.value?.mission ? [missionNow(d.value)] : []));
    total = all.length;
    waitingPast = all.slice(MAX).filter((m) => m.waiting_on != null).length;
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
    <h4>
      <button
        type="button"
        class="toggle"
        aria-expanded={open}
        data-testid="control-missions-toggle"
        onclick={() => runningMissionsFold.set(open ? 'closed' : 'open')}
        ><span class="chev" aria-hidden="true">{open ? '▾' : '▸'}</span>Running missions <span class="n">{total}</span
        >{#if !open && waiting > 0}<span class="wait" data-testid="control-missions-waiting">· {waiting} wait{waiting === 1 ? 's' : ''} for you</span>{/if}</button
      >
    </h4>
    {#if open}
    <ul>
      {#each running as m (m.id)}
        <li data-testid="control-mission" data-waiting={m.waiting}>
          <button type="button" class="link" onclick={() => openMission(m.id)}>{m.name}</button>
          {#if m.waiting}
            <span class="muted">Waits for you</span>
          {:else if m.step}
            {#if m.trails}<Loader name="comet-trails" size={20} label="Working on it" testid="control-mission-trails" />{/if}
            <span class="step">{m.step}</span>
          {/if}
          {#if m.sessions > 0}
            <span class="muted" data-testid="control-mission-sessions">· {m.sessions} {m.sessions === 1 ? 'session' : 'sessions'}</span>
          {/if}
        </li>
      {/each}
    </ul>
    {/if}
  </section>
{/if}

<style>
  .missions {
    border-top: 1px solid var(--border);
    padding: 0.4rem 0.6rem;
    flex: none;
  }
  h4 {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
    font-weight: 600;
  }
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 0;
    background: transparent;
    padding: 2px 0;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .toggle:hover {
    color: var(--fg);
  }
  .toggle:focus-visible {
    outline: var(--ring-w) solid var(--ring);
    outline-offset: 1px;
  }
  .chev {
    width: 10px;
  }
  .n,
  .wait {
    font-weight: 400;
  }
  .wait {
    color: var(--status-waiting);
  }
  ul {
    list-style: none;
    margin: 0.25rem 0 0;
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
    font-size: var(--text-xs);
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
