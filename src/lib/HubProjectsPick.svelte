<script lang="ts">
  // Settings › Hub & sync › Projects on the hub (gap plan G4.6): which of
  // the hub's projects this desktop lists and starts sessions in. This
  // device only; the hub and the other devices are unchanged.
  import { allProjects } from './projects';
  import { hubStatus } from './hub';
  import { hubProjectFilter, hubProjectsLabel, pickHubProjects } from './hub_projects';

  const listed = $derived($allProjects.filter((p) => !p.project.system));
  const shown = $derived(
    $hubProjectFilter ? listed.filter((p) => $hubProjectFilter.has(p.project.id)) : listed,
  );
  let open = $state(false);

  function toggle(id: number, on: boolean) {
    const url = $hubStatus.url;
    if (!url) return;
    const ids = new Set(shown.map((p) => p.project.id));
    if (on) ids.add(id);
    else ids.delete(id);
    // Leaving none would hide the hub's whole fleet: the last one stays.
    if (ids.size === 0) return;
    pickHubProjects(url, [...ids], listed.length);
  }
</script>

<div class="pref" data-testid="hub-projects">
  <div class="pref-text">
    <span class="lbl">Projects on the hub</span>
    <p class="hook-desc">Which of the hub's projects this desktop lists and starts sessions in. Other devices are unchanged.</p>
  </div>
  <button
    class="hook-btn"
    type="button"
    aria-expanded={open}
    data-testid="hub-projects-toggle"
    disabled={listed.length === 0}
    onclick={() => (open = !open)}>{hubProjectsLabel(shown.length, listed.length)} ▾</button
  >
</div>
{#if open}
  <ul class="picks" data-testid="hub-projects-list">
    {#each listed as p (p.project.id)}
      {@const on = shown.some((s) => s.project.id === p.project.id)}
      <li>
        <label>
          <input
            type="checkbox"
            checked={on}
            disabled={on && shown.length === 1}
            data-testid={`hub-projects-${p.project.id}`}
            onchange={(e) => toggle(p.project.id, (e.currentTarget as HTMLInputElement).checked)} />
          {p.project.owner}/{p.project.repo}
        </label>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .pref {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: var(--space-3) 0;
  }
  .pref-text { flex: 1; min-width: 0; }
  .lbl { font-size: var(--text-sm); font-weight: 500; }
  .hook-desc {
    margin: 2px 0 0;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .picks {
    list-style: none;
    margin: 0 0 var(--space-3);
    padding: 0;
    max-height: 240px;
    overflow: auto;
    font-size: var(--text-sm);
  }
  .picks label { display: flex; gap: var(--space-2); align-items: center; padding: 2px 0; }
</style>
