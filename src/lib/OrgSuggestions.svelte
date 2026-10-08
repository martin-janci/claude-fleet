<script lang="ts">
  // Proposed orgs (work graph M5.4), from the owners of live sessions and
  // tracker sites: one click creates the org with its owner rule and its
  // tracker. A registered custom item on the generated Organisations page
  // (declarative pages P4) until actions can chain. Only for the process
  // that owns the fleet: a paired desktop cannot create orgs.
  import { onMount } from 'svelte';
  import { createFromSuggestion, loadOrgSuggestions, type OrgSuggestion } from './orgs';
  import { hubStatus, ownsTheFleet } from './hub';
  import { pushError } from './toasts';

  let { onchanged = () => {} }: { onchanged?: () => void } = $props();

  let suggestions = $state<OrgSuggestion[]>([]);
  let busy = $state(false);
  const owns = $derived(ownsTheFleet($hubStatus));

  async function refresh() {
    const s = await loadOrgSuggestions();
    suggestions = s.ok && Array.isArray(s.value) ? s.value : [];
  }

  onMount(() => {
    if (owns) void refresh();
  });

  async function accept(sg: OrgSuggestion) {
    busy = true;
    const r = await createFromSuggestion(sg);
    busy = false;
    if (!r.ok) pushError(r.error, 'Create org failed');
    await refresh();
    onchanged();
  }
</script>

{#if owns && suggestions.length > 0}
  <ul class="suggestions" data-testid="org-suggestions">
    {#each suggestions as sg (sg.name + (sg.tracker_id ?? ''))}
      <li>
        <button class="btn" data-testid="org-suggestion" disabled={busy} title={sg.reason} onclick={() => void accept(sg)}
          >Create org {sg.name}{sg.owner ? ` from ${sg.owner}/*` : ''}{sg.tracker_id != null ? ' with its tracker' : ''}</button
        >
        <span class="hint">{sg.reason}</span>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .suggestions {
    list-style: none;
    padding: 0;
    margin: 0 0 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .hint {
    font-size: 11px;
    color: var(--fg-muted);
    margin-left: 0.35rem;
  }
</style>
